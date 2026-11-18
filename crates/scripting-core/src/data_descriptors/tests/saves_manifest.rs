// Tests: manifest `saves` drain, QuickJS and Luau twins.

use super::super::*;
use super::common::*;
use crate::runtime::{MAX_CHARACTER_SLOTS, MAX_RUN_SLOTS};
use log::Level;
use postretro_test_log_capture::LogCapture;

const SCOPE: &str = "test manifest";

fn drain_js(manifest: &str) -> Option<SavesManifest> {
    eval_js(&format!("({manifest})"), |_, value| {
        let obj = Object::from_value(value).expect("fixture must evaluate to an object");
        drain_saves_js(&obj, SCOPE).expect("saves never rejects the manifest")
    })
}

fn drain_lua(manifest: &str) -> Option<SavesManifest> {
    eval_lua(&format!("return {manifest}"), |value| {
        let LuaValue::Table(table) = value else {
            panic!("fixture must evaluate to a table");
        };
        drain_saves_lua(&table, SCOPE).expect("saves never rejects the manifest")
    })
}

fn warnings(capture: &LogCapture) -> Vec<String> {
    capture
        .records()
        .into_iter()
        .filter(|record| record.level == Level::Warn)
        .map(|record| record.message)
        .collect()
}

fn saves(character_slots: usize, run_slots: usize) -> Option<SavesManifest> {
    Some(SavesManifest {
        character_slots,
        run_slots,
    })
}

#[test]
fn saves_block_absent_resolves_to_none_without_a_warning() {
    let capture = LogCapture::start();
    assert_eq!(drain_js("{}"), None);
    assert_eq!(drain_lua("{}"), None);
    assert_eq!(drain_js("{ saves: undefined }"), None);
    assert!(warnings(&capture).is_empty());
}

#[test]
fn saves_block_valid_counts_resolve_in_both_vms_without_a_warning() {
    let capture = LogCapture::start();
    assert_eq!(
        drain_js("{ saves: { characterSlots: 4, runSlots: 3 } }"),
        saves(4, 3)
    );
    assert_eq!(
        drain_lua("{ saves = { characterSlots = 4, runSlots = 3 } }"),
        saves(4, 3)
    );
    let at_cap = format!(
        "{{ saves: {{ characterSlots: {MAX_CHARACTER_SLOTS}, runSlots: {MAX_RUN_SLOTS} }} }}"
    );
    assert_eq!(drain_js(&at_cap), saves(MAX_CHARACTER_SLOTS, MAX_RUN_SLOTS));
    assert!(warnings(&capture).is_empty());
}

// R18 (declaration half): a count above the engine cap is clamped with a
// warning naming the field and the cap.
#[test]
fn saves_count_above_the_engine_cap_clamps_with_a_warning() {
    let over_characters = MAX_CHARACTER_SLOTS + 1;
    let over_runs = MAX_RUN_SLOTS + 84;
    for (vm, manifest) in [
        (
            "js",
            format!("{{ saves: {{ characterSlots: {over_characters}, runSlots: {over_runs} }} }}"),
        ),
        (
            "luau",
            format!(
                "{{ saves = {{ characterSlots = {over_characters}, runSlots = {over_runs} }} }}"
            ),
        ),
    ] {
        let capture = LogCapture::start();
        let drained = if vm == "js" {
            drain_js(&manifest)
        } else {
            drain_lua(&manifest)
        };
        assert_eq!(drained, saves(MAX_CHARACTER_SLOTS, MAX_RUN_SLOTS), "{vm}");
        let warnings = warnings(&capture);
        for (field, cap) in [
            ("saves.characterSlots", MAX_CHARACTER_SLOTS),
            ("saves.runSlots", MAX_RUN_SLOTS),
        ] {
            assert_eq!(
                warnings
                    .iter()
                    .filter(|w| w.contains(field) && w.contains(&format!("engine cap of {cap}")))
                    .count(),
                1,
                "{vm}: expected one clamp warning for `{field}`, got {warnings:?}"
            );
        }
    }
}

#[test]
fn saves_malformed_counts_warn_and_use_one() {
    for (js, lua, expected, warned_fields) in [
        (
            "{ saves: { runSlots: 2 } }",
            "{ saves = { runSlots = 2 } }",
            saves(1, 2),
            &["saves.characterSlots"][..],
        ),
        (
            "{ saves: { characterSlots: 2.5, runSlots: 0 } }",
            "{ saves = { characterSlots = 2.5, runSlots = 0 } }",
            saves(1, 1),
            &["saves.characterSlots", "saves.runSlots"][..],
        ),
        (
            "{ saves: { characterSlots: \"4\", runSlots: -3 } }",
            "{ saves = { characterSlots = \"4\", runSlots = -3 } }",
            saves(1, 1),
            &["saves.characterSlots", "saves.runSlots"][..],
        ),
        (
            "{ saves: {} }",
            "{ saves = {} }",
            saves(1, 1),
            &["saves.characterSlots", "saves.runSlots"][..],
        ),
    ] {
        let capture = LogCapture::start();
        assert_eq!(drain_js(js), expected, "{js}");
        assert_eq!(drain_lua(lua), expected, "{lua}");
        let warnings = warnings(&capture);
        for field in warned_fields {
            assert_eq!(
                warnings.iter().filter(|w| w.contains(field)).count(),
                2,
                "both VMs warn once for `{field}` in {js}, got {warnings:?}"
            );
        }
    }
}

#[test]
fn saves_block_that_is_not_an_object_warns_and_declares_one_of_each() {
    let capture = LogCapture::start();
    assert_eq!(drain_js("{ saves: 4 }"), saves(1, 1));
    assert_eq!(drain_js("{ saves: [4, 2] }"), saves(1, 1));
    assert_eq!(drain_lua("{ saves = true }"), saves(1, 1));
    assert_eq!(
        warnings(&capture)
            .iter()
            .filter(|w| w.contains("`saves` must be an object"))
            .count(),
        3
    );
}
