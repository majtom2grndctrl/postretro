// Tests: `fluids` manifest drains in both runtimes and their warn-and-drop policy.

use super::super::*;
use super::common::*;
use postretro_test_log_capture::LogCapture;

const SCOPE: &str = "mod manifest";

fn drain_js(src: &str) -> FluidRegistry {
    eval_js(src, |ctx, value| {
        let obj = rquickjs::Object::from_value(value).unwrap();
        drain_fluids_js(ctx, &obj, SCOPE).unwrap()
    })
}

fn drain_lua(src: &str) -> FluidRegistry {
    eval_lua(src, |value| {
        let LuaValue::Table(table) = value else {
            panic!("manifest is a table");
        };
        drain_fluids_lua(&table, SCOPE).unwrap()
    })
}

/// One fluid entry in JS object syntax; Luau syntax swaps `:` for ` =` and
/// `[...]` for `{...}`.
fn js_fluid(name: &str, overrides: &[(&str, &str)]) -> String {
    let defaults = [
        ("viscosity", "2.0"),
        ("viewTint", "[0.1, 0.3, 0.4, 0.35]"),
        ("muffle", "0.8"),
        ("surfaceOpacity", "0.6"),
        ("enterSound", "\"sfx/splash_in\""),
    ];
    let mut fields = vec![format!("name: \"{name}\"")];
    for (key, value) in defaults {
        let value = overrides
            .iter()
            .find(|(k, _)| *k == key)
            .map_or(value, |(_, v)| *v);
        fields.push(format!("{key}: {value}"));
    }
    format!("{{ {} }}", fields.join(", "))
}

fn lua_fluid(name: &str, overrides: &[(&str, &str)]) -> String {
    js_fluid(name, overrides)
        .replace(": ", " = ")
        .replace('[', "{")
        .replace(']', "}")
}

fn js_manifest(entries: &[String]) -> String {
    format!("({{ fluids: [{}] }})", entries.join(", "))
}

fn lua_manifest(entries: &[String]) -> String {
    format!("return {{ fluids = {{ {} }} }}", entries.join(", "))
}

/// Run the same entries through both drains; return both registries.
fn drain_both(entries: &[(&str, &[(&str, &str)])]) -> (FluidRegistry, FluidRegistry) {
    let js: Vec<String> = entries.iter().map(|(n, o)| js_fluid(n, o)).collect();
    let lua: Vec<String> = entries.iter().map(|(n, o)| lua_fluid(n, o)).collect();
    (drain_js(&js_manifest(&js)), drain_lua(&lua_manifest(&lua)))
}

fn warnings(capture: &LogCapture, needle: &str) -> Vec<String> {
    capture
        .records()
        .into_iter()
        .filter(|record| record.level == log::Level::Warn && record.message.contains(needle))
        .map(|record| record.message)
        .collect()
}

#[test]
fn declared_fluids_index_by_declaration_order_in_both_runtimes() {
    let (js, lua) = drain_both(&[("water", &[]), ("sludge", &[("viscosity", "6.0")])]);
    assert_eq!(js, lua);
    assert_eq!(js.index_of("water"), Some(0));
    assert_eq!(js.index_of("sludge"), Some(1));
    let water = js.get(0).unwrap();
    assert_eq!(water.view_tint, [0.1, 0.3, 0.4, 0.35]);
    assert_eq!(water.enter_sound.as_deref(), Some("sfx/splash_in"));
    assert_eq!(water.exit_sound, None);
}

#[test]
fn absent_fluids_declare_none() {
    assert!(drain_js("({})").is_empty());
    assert!(drain_lua("return {}").is_empty());
}

// AC P10: an out-of-range presentation field rejects that fluid only, naming the field.
#[test]
fn out_of_range_view_tint_muffle_or_surface_opacity_rejects_that_fluid_naming_the_field() {
    for (field, js_value) in [
        ("viewTint", "[0.1, 0.3, 1.4, 0.35]"),
        ("muffle", "1.2"),
        ("surfaceOpacity", "-0.25"),
    ] {
        let capture = LogCapture::start();
        let js = drain_js(&js_manifest(&[
            js_fluid("water", &[]),
            js_fluid("bad", &[(field, js_value)]),
            js_fluid("sludge", &[]),
        ]));
        let lua = drain_lua(&lua_manifest(&[
            lua_fluid("water", &[]),
            // `lua_fluid` swaps brackets itself; pass the JS spelling.
            lua_fluid("bad", &[(field, js_value)]),
            lua_fluid("sludge", &[]),
        ]));
        assert_eq!(js, lua);
        assert_eq!(js.names(), ["water", "sludge"], "{field}: the rest stands");
        let rejected = warnings(&capture, &format!("fluids[1].{field}"));
        assert_eq!(rejected.len(), 2, "one warning per runtime: {rejected:?}");
        assert_eq!(
            rejected[0], rejected[1],
            "the runtimes' diagnostics diverged"
        );
        assert!(
            rejected[0].contains("fluid 'bad' was rejected"),
            "{}",
            rejected[0]
        );
    }
}

// AC P9 (validation half): negative viscosity rejects the fluid, naming the field.
#[test]
fn negative_viscosity_rejects_the_fluid_naming_the_field() {
    let capture = LogCapture::start();
    let (js, lua) = drain_both(&[("water", &[("viscosity", "-1.0")]), ("sludge", &[])]);
    assert_eq!(js, lua);
    assert_eq!(js.names(), ["sludge"]);
    assert_eq!(js.index_of("sludge"), Some(0));
    let rejected = warnings(&capture, "fluids[0].viscosity must be a finite value >= 0");
    assert_eq!(rejected.len(), 2, "{rejected:?}");
}

// AC P10: a second declaration of a name is rejected; the first declaration stands.
#[test]
fn a_second_define_fluid_with_an_existing_name_is_rejected_and_the_first_stands() {
    let capture = LogCapture::start();
    let (js, lua) = drain_both(&[
        ("water", &[("viscosity", "2.0")]),
        ("sludge", &[]),
        ("water", &[("viscosity", "9.0")]),
    ]);
    assert_eq!(js, lua);
    assert_eq!(js.names(), ["water", "sludge"]);
    assert_eq!(
        js.get(0).unwrap().viscosity,
        2.0,
        "the first declaration stands"
    );
    let duplicate = warnings(
        &capture,
        "fluids[2].name duplicates fluid 'water' declared at fluids[0]; the first declaration stands",
    );
    assert_eq!(duplicate.len(), 2, "{duplicate:?}");
}

#[test]
fn a_malformed_fluid_entry_is_skipped_and_its_siblings_keep_declaration_order() {
    let capture = LogCapture::start();
    let js = drain_js(
        r#"({ fluids: [
        { name: "water", viscosity: 2, viewTint: [0, 0, 0, 0], muffle: 0, surfaceOpacity: 1 },
        { name: "bad", viscosity: 2, viewTint: [0, 0, 0], muffle: 0, surfaceOpacity: 1 },
        { name: "sludge", viscosity: Infinity, viewTint: [0, 0, 0, 0], muffle: 0, surfaceOpacity: 1 },
        { name: "lava", viscosity: 9, viewTint: [1, 0.2, 0, 0.8], muffle: 0.5, surfaceOpacity: 1, glow: 3 },
        { name: "oil", viscosity: 4, viewTint: [0, 0, 0, 0.9], muffle: 0.2, surfaceOpacity: 1 }
    ] })"#,
    );
    let lua = drain_lua(
        r#"return { fluids = {
        { name = "water", viscosity = 2, viewTint = {0, 0, 0, 0}, muffle = 0, surfaceOpacity = 1 },
        { name = "bad", viscosity = 2, viewTint = {0, 0, 0}, muffle = 0, surfaceOpacity = 1 },
        { name = "sludge", viscosity = math.huge, viewTint = {0, 0, 0, 0}, muffle = 0, surfaceOpacity = 1 },
        { name = "lava", viscosity = 9, viewTint = {1, 0.2, 0, 0.8}, muffle = 0.5, surfaceOpacity = 1, glow = 3 },
        { name = "oil", viscosity = 4, viewTint = {0, 0, 0, 0.9}, muffle = 0.2, surfaceOpacity = 1 }
    } }"#,
    );
    assert_eq!(js, lua);
    assert_eq!(js.names(), ["water", "oil"]);
    for index in 1..=3 {
        let skipped = warnings(
            &capture,
            &format!("fluids[{index}] is malformed and was skipped"),
        );
        assert_eq!(skipped.len(), 2, "entry {index}: {skipped:?}");
    }
}

#[test]
fn a_non_array_fluids_field_warns_and_declares_none() {
    let capture = LogCapture::start();
    assert!(drain_js("({ fluids: 3 })").is_empty());
    assert!(drain_lua("return { fluids = 3 }").is_empty());
    assert_eq!(warnings(&capture, "`fluids` must be an array").len(), 2);
}

// A hole in the list costs only its own slot, in both runtimes: the entries
// around it keep their declaration order and their diagnostic indices.
#[test]
fn a_hole_in_fluids_warns_for_that_entry_only_in_both_runtimes() {
    let capture = LogCapture::start();
    let water = js_fluid("water", &[]);
    let sludge = js_fluid("sludge", &[]);
    let js = drain_js(&format!("({{ fluids: [{water}, undefined, {sludge}] }})"));
    let js_sparse = drain_js(&format!("({{ fluids: [{water}, , {sludge}] }})"));
    let lua = drain_lua(&format!(
        "return {{ fluids = {{ {}, nil, {} }} }}",
        lua_fluid("water", &[]),
        lua_fluid("sludge", &[])
    ));
    assert_eq!(js, lua);
    assert_eq!(js_sparse, lua);
    assert_eq!(lua.names(), ["water", "sludge"]);
    let skipped = warnings(&capture, "fluids[1] is malformed and was skipped");
    assert_eq!(skipped.len(), 3, "one per drain: {skipped:?}");
    assert!(
        skipped.iter().all(|warning| *warning == skipped[0]),
        "the runtimes' diagnostics diverged: {skipped:?}"
    );
    assert!(
        warnings(&capture, "must be an array").is_empty(),
        "the field itself stands"
    );
}

// `fluids: {}` is an empty list in QuickJS, as an empty Luau table is.
#[test]
fn an_empty_fluids_object_declares_none_without_a_warning() {
    let capture = LogCapture::start();
    assert!(drain_js("({ fluids: {} })").is_empty());
    assert!(drain_lua("return { fluids = {} }").is_empty());
    assert!(warnings(&capture, "fluids").is_empty());
}

// A missing or mistyped field names `fluids[i].<field>` in the fluid's own
// wording, never the generic reaction-descriptor message.
#[test]
fn a_missing_or_mistyped_fluid_field_names_the_entry_field() {
    for (js_entry, lua_entry, needle) in [
        (
            "{ name: \"water\", viewTint: [0, 0, 0, 0], muffle: 0, surfaceOpacity: 1 }",
            "{ name = \"water\", viewTint = {0, 0, 0, 0}, muffle = 0, surfaceOpacity = 1 }",
            "`fluids[0].viscosity` is required",
        ),
        (
            "{ name: \"water\", viscosity: \"thick\", viewTint: [0, 0, 0, 0], muffle: 0, surfaceOpacity: 1 }",
            "{ name = \"water\", viscosity = \"thick\", viewTint = {0, 0, 0, 0}, muffle = 0, surfaceOpacity = 1 }",
            "`fluids[0].viscosity` must be a number",
        ),
        (
            "{ name: \"water\", viscosity: 2, viewTint: [0, 0, 0, 0], muffle: 0, surfaceOpacity: 1, exitSound: 4 }",
            "{ name = \"water\", viscosity = 2, viewTint = {0, 0, 0, 0}, muffle = 0, surfaceOpacity = 1, exitSound = 4 }",
            "`fluids[0].exitSound` must be a string",
        ),
    ] {
        let capture = LogCapture::start();
        assert!(drain_js(&format!("({{ fluids: [{js_entry}] }})")).is_empty());
        assert!(drain_lua(&format!("return {{ fluids = {{ {lua_entry} }} }}")).is_empty());
        let skipped = warnings(&capture, needle);
        assert_eq!(skipped.len(), 2, "{needle}: {skipped:?}");
        assert_eq!(skipped[0], skipped[1], "the runtimes' diagnostics diverged");
        assert!(
            !skipped[0].contains("reaction descriptor"),
            "{}",
            skipped[0]
        );
    }
}
