// Tests: manifest `uiImages`, `loading`, and catalog `loadingTree` drains,
// QuickJS and Luau twins.

use std::collections::BTreeMap;

use super::super::*;
use super::common::*;
use log::Level;
use postretro_test_log_capture::LogCapture;

const SCOPE: &str = "test manifest";

/// The drained `uiImages`, `loading`, and per-map `loadingTree` pools of one
/// manifest.
type Drained = (BTreeMap<String, String>, ModLoading, Vec<Vec<String>>);

fn drain_js(manifest: &str) -> Drained {
    eval_js(&format!("({manifest})"), |_, value| {
        let obj = Object::from_value(value).expect("fixture must evaluate to an object");
        (
            drain_ui_images_js(&obj, SCOPE).expect("uiImages never rejects the manifest"),
            drain_loading_js(&obj, SCOPE).expect("loading never rejects the manifest"),
            drain_maps_js(&obj, SCOPE)
                .expect("maps never rejects the manifest")
                .into_iter()
                .map(|map| map.loading_tree)
                .collect(),
        )
    })
}

fn drain_lua(manifest: &str) -> Drained {
    eval_lua(&format!("return {manifest}"), |value| {
        let LuaValue::Table(table) = value else {
            panic!("fixture must evaluate to a table");
        };
        (
            drain_ui_images_lua(&table, SCOPE).expect("uiImages never rejects the manifest"),
            drain_loading_lua(&table, SCOPE).expect("loading never rejects the manifest"),
            drain_maps_lua(&table, SCOPE)
                .expect("maps never rejects the manifest")
                .into_iter()
                .map(|map| map.loading_tree)
                .collect(),
        )
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

fn names(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn images(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(name, path)| (name.to_string(), path.to_string()))
        .collect()
}

#[test]
fn ui_images_drain_in_both_vms() {
    let expected = images(&[("hud/logo", "ui/logo.png"), ("splash", "./art/splash.png")]);
    let js = drain_js(r#"{ uiImages: { "hud/logo": "ui/logo.png", splash: "./art/splash.png" } }"#);
    let lua = drain_lua(
        r#"{ uiImages = { ["hud/logo"] = "ui/logo.png", splash = "./art/splash.png" } }"#,
    );
    assert_eq!(js.0, expected);
    assert_eq!(lua.0, expected);
}

#[test]
fn ui_images_reject_engine_prefix_with_a_warning() {
    for (vm, manifest) in [
        (
            "js",
            r#"{ uiImages: { "engine/splashLogo": "ui/fake.png", kept: "ui/kept.png" } }"#,
        ),
        (
            "luau",
            r#"{ uiImages = { ["engine/splashLogo"] = "ui/fake.png", kept = "ui/kept.png" } }"#,
        ),
    ] {
        let capture = LogCapture::start();
        let drained = if vm == "js" {
            drain_js(manifest)
        } else {
            drain_lua(manifest)
        };
        assert_eq!(drained.0, images(&[("kept", "ui/kept.png")]), "{vm}");
        let warnings = warnings(&capture);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("uiImages.engine/splashLogo") && w.contains("engine/")),
            "{vm}: expected a reserved-prefix warning, got {warnings:?}"
        );
    }
}

#[test]
fn ui_images_skip_non_string_and_escaping_paths() {
    let js = drain_js(
        r#"{ uiImages: {
            num: 3, nul: null, obj: { path: "x.png" },
            parent: "../outside.png", nested: "ui/../../outside.png",
            absolute: "/abs/x.png", empty: "", ok: "ui/ok.png",
        } }"#,
    );
    let lua = drain_lua(
        r#"{ uiImages = {
            num = 3, obj = { path = "x.png" },
            parent = "../outside.png", nested = "ui/../../outside.png",
            absolute = "/abs/x.png", empty = "", ok = "ui/ok.png",
        } }"#,
    );
    let expected = images(&[("ok", "ui/ok.png")]);
    assert_eq!(js.0, expected);
    assert_eq!(lua.0, expected);
}

#[test]
fn ui_images_wrong_type_degrades_to_empty() {
    let capture = LogCapture::start();
    assert!(drain_js(r#"{ uiImages: "ui/logo.png" }"#).0.is_empty());
    assert!(drain_lua(r#"{ uiImages = "ui/logo.png" }"#).0.is_empty());
    assert_eq!(
        warnings(&capture)
            .iter()
            .filter(|w| w.contains("`uiImages` must be an object"))
            .count(),
        2
    );
    assert!(drain_js("{}").0.is_empty());
    assert!(drain_lua("{}").0.is_empty());
}

#[test]
fn loading_tree_accepts_a_string() {
    let expected = ModLoading {
        tree: names(&["modLoading"]),
    };
    assert_eq!(
        drain_js(r#"{ loading: { tree: "modLoading" } }"#).1,
        expected
    );
    assert_eq!(
        drain_lua(r#"{ loading = { tree = "modLoading" } }"#).1,
        expected
    );
}

#[test]
fn loading_tree_accepts_an_array_and_deduplicates_in_order() {
    let expected = ModLoading {
        tree: names(&["loadB", "loadA"]),
    };
    assert_eq!(
        drain_js(r#"{ loading: { tree: ["loadB", "loadA", "loadB"] } }"#).1,
        expected
    );
    assert_eq!(
        drain_lua(r#"{ loading = { tree = { "loadB", "loadA", "loadB" } } }"#).1,
        expected
    );
}

#[test]
fn malformed_loading_degrades_to_absent_with_a_warning() {
    for (js, lua) in [
        (
            r#"{ loading: "modLoading" }"#,
            r#"{ loading = "modLoading" }"#,
        ),
        (
            r#"{ loading: { tree: 7 } }"#,
            r#"{ loading = { tree = 7 } }"#,
        ),
        (
            r#"{ loading: { tree: ["ok", 7] } }"#,
            r#"{ loading = { tree = { "ok", 7 } } }"#,
        ),
        (r#"{ loading: {} }"#, r#"{ loading = { other = true } }"#),
    ] {
        let capture = LogCapture::start();
        assert_eq!(drain_js(js).1, ModLoading::default(), "{js}");
        assert_eq!(drain_lua(lua).1, ModLoading::default(), "{lua}");
        let warnings = warnings(&capture);
        assert!(
            warnings.iter().filter(|w| w.contains("loading")).count() >= 2,
            "both VMs warn for {js} / {lua}, got {warnings:?}"
        );
    }
}

#[test]
fn empty_loading_tree_array_is_absent_without_a_warning() {
    let capture = LogCapture::start();
    assert_eq!(
        drain_js(r#"{ loading: { tree: [] } }"#).1,
        ModLoading::default()
    );
    assert_eq!(
        drain_lua(r#"{ loading = { tree = {} } }"#).1,
        ModLoading::default()
    );
    assert_eq!(drain_js("{}").1, ModLoading::default());
    assert!(warnings(&capture).is_empty());
}

#[test]
fn catalog_loading_tree_string_array_and_malformed() {
    let js = drain_js(
        r#"{ maps: [
            { id: "a", path: "maps/a.prl", name: "A", loadingTree: "loadA" },
            { id: "b", path: "maps/b.prl", name: "B", loadingTree: ["loadA", "loadB"] },
            { id: "c", path: "maps/c.prl", name: "C", loadingTree: 4 },
            { id: "d", path: "maps/d.prl", name: "D", loadingTree: ["loadA", false] },
            { id: "e", path: "maps/e.prl", name: "E", loadingTree: [] },
            { id: "f", path: "maps/f.prl", name: "F" },
        ] }"#,
    );
    let lua = drain_lua(
        r#"{ maps = {
            { id = "a", path = "maps/a.prl", name = "A", loadingTree = "loadA" },
            { id = "b", path = "maps/b.prl", name = "B", loadingTree = { "loadA", "loadB" } },
            { id = "c", path = "maps/c.prl", name = "C", loadingTree = 4 },
            { id = "d", path = "maps/d.prl", name = "D", loadingTree = { "loadA", false } },
            { id = "e", path = "maps/e.prl", name = "E", loadingTree = {} },
            { id = "f", path = "maps/f.prl", name = "F" },
        } }"#,
    );
    let expected = vec![
        names(&["loadA"]),
        names(&["loadA", "loadB"]),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ];
    assert_eq!(js.2, expected, "a malformed pool keeps the map entry");
    assert_eq!(lua.2, expected, "a malformed pool keeps the map entry");
}

#[test]
fn malformed_catalog_loading_tree_warns_naming_the_entry() {
    let capture = LogCapture::start();
    drain_js(r#"{ maps: [{ id: "c", path: "maps/c.prl", name: "C", loadingTree: { x: 1 } }] }"#);
    let warnings = warnings(&capture);
    assert!(
        warnings.iter().any(|w| w.contains("maps[0].loadingTree")),
        "expected a warning naming the entry, got {warnings:?}"
    );
}
