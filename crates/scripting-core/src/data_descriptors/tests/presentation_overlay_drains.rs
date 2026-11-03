// Tests: `presentationOverlays` accepts one descriptor or an array, at most
// one per overlay kind, identically in both runtimes.

use super::super::*;
use super::common::*;
use log::Level;
use postretro_test_log_capture::LogCapture;

fn drain_js(manifest: &str) -> Vec<PresentationOverlay> {
    eval_js(manifest, |ctx, value| {
        let object = Object::from_value(value).unwrap();
        drain_presentation_overlays_js(ctx, &object, "test manifest").unwrap()
    })
}

fn drain_lua(manifest: &str) -> Vec<PresentationOverlay> {
    eval_lua(manifest, |value| {
        let table = lua_table(value, "test manifest").unwrap();
        drain_presentation_overlays_lua(&table, "test manifest").unwrap()
    })
}

fn kinds(overlays: &[PresentationOverlay]) -> Vec<PresentationOverlayKind> {
    overlays.iter().map(|overlay| overlay.over.kind()).collect()
}

#[test]
fn single_descriptor_and_one_element_array_lower_alike_in_both_runtimes() {
    let single_js = drain_js(
        r#"({ presentationOverlays: { over: { kind: "damagedEnemies", lingerMs: 1, hideAtFull: false }, template: "enemyStatus", maxVisible: 2 } })"#,
    );
    let array_js = drain_js(
        r#"({ presentationOverlays: [{ over: { kind: "damagedEnemies", lingerMs: 1, hideAtFull: false }, template: "enemyStatus", maxVisible: 2 }] })"#,
    );
    let single_lua = drain_lua(
        r#"return { presentationOverlays = { over = { kind = "damagedEnemies", lingerMs = 1, hideAtFull = false }, template = "enemyStatus", maxVisible = 2 } }"#,
    );
    let array_lua = drain_lua(
        r#"return { presentationOverlays = { { over = { kind = "damagedEnemies", lingerMs = 1, hideAtFull = false }, template = "enemyStatus", maxVisible = 2 } } }"#,
    );
    assert_eq!(single_js.len(), 1);
    assert_eq!(single_js, array_js);
    assert_eq!(single_js, single_lua);
    assert_eq!(single_js, array_lua);
    assert_eq!(
        serde_json::to_value(&array_js).unwrap(),
        serde_json::to_value(&array_lua).unwrap()
    );
}

#[test]
fn repeated_kind_keeps_the_first_descriptor_and_warns_naming_the_kind() {
    let capture = LogCapture::start();
    let js = drain_js(
        r#"({ presentationOverlays: [
            { over: { kind: "damagedEnemies", lingerMs: 1, hideAtFull: false }, template: "first", maxVisible: 2 },
            { over: { kind: "damagedEnemies", lingerMs: 9, hideAtFull: true }, template: "second", maxVisible: 3 },
        ] })"#,
    );
    capture.assert_logged_once(
        Level::Warn,
        "`presentationOverlays` repeats kind `damagedEnemies`; only the first `damagedEnemies` overlay is kept",
    );
    drop(capture);

    let capture = LogCapture::start();
    let lua = drain_lua(
        r#"return { presentationOverlays = {
            { over = { kind = "damagedEnemies", lingerMs = 1, hideAtFull = false }, template = "first", maxVisible = 2 },
            { over = { kind = "damagedEnemies", lingerMs = 9, hideAtFull = true }, template = "second", maxVisible = 3 },
        } }"#,
    );
    capture.assert_logged_once(Level::Warn, "repeats kind `damagedEnemies`");
    assert_eq!(js, lua);
    assert_eq!(kinds(&js), [PresentationOverlayKind::DamagedEnemies]);
    assert_eq!(js[0].template, "first");
}

#[test]
fn malformed_entries_are_skipped_and_a_malformed_field_is_ignored() {
    let capture = LogCapture::start();
    let js = drain_js(
        r#"({ presentationOverlays: [
            { over: { kind: "damagedEnemies", lingerMs: 1, hideAtFull: false }, template: "", maxVisible: 2 },
            { over: { kind: "damagedEnemies", lingerMs: 1, hideAtFull: false }, template: "kept", maxVisible: 2 },
        ] })"#,
    );
    capture.assert_logged_once(
        Level::Warn,
        "`presentationOverlays[0]` is malformed and was skipped",
    );
    assert_eq!(js.len(), 1);
    assert_eq!(js[0].template, "kept");
    drop(capture);

    let capture = LogCapture::start();
    let lua = drain_lua(
        r#"return { presentationOverlays = {
            { over = { kind = "damagedEnemies", lingerMs = 1, hideAtFull = false }, template = "", maxVisible = 2 },
            { over = { kind = "damagedEnemies", lingerMs = 1, hideAtFull = false }, template = "kept", maxVisible = 2 },
        } }"#,
    );
    capture.assert_logged_once(
        Level::Warn,
        "`presentationOverlays[0]` is malformed and was skipped",
    );
    assert_eq!(js, lua);
    drop(capture);

    let capture = LogCapture::start();
    assert!(drain_js(r#"({ presentationOverlays: 3 })"#).is_empty());
    assert!(drain_lua(r#"return { presentationOverlays = { template = "x" } }"#).is_empty());
    capture.assert_logged(
        Level::Warn,
        "`presentationOverlays` must be an overlay descriptor or an array of them; the field was ignored",
    );
    drop(capture);

    assert!(drain_js(r#"({ presentationOverlays: [] })"#).is_empty());
    assert!(drain_lua(r#"return { presentationOverlays = {} }"#).is_empty());
    assert!(drain_js(r#"({})"#).is_empty());
    assert!(drain_lua(r#"return {}"#).is_empty());
}
