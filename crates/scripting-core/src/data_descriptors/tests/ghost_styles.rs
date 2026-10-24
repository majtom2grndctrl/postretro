// Tests: `ghostStyles` manifest drains in both runtimes and their warn-and-drop policy.

use super::super::*;
use super::common::*;
use postretro_test_log_capture::LogCapture;

const SCOPE: &str = "mod manifest";

fn drain_js(src: &str) -> GhostStyleRegistry {
    eval_js(src, |ctx, value| {
        let obj = rquickjs::Object::from_value(value).unwrap();
        let styles = drain_ghost_styles_js(ctx, &obj, SCOPE).unwrap();
        assert!(!ctx.has_exception(), "the drain left an exception pending");
        styles
    })
}

fn drain_lua(src: &str) -> GhostStyleRegistry {
    eval_lua(src, |value| {
        let LuaValue::Table(table) = value else {
            panic!("manifest is a table");
        };
        drain_ghost_styles_lua(&table, SCOPE).unwrap()
    })
}

/// Run one list of entries, each written in JS object syntax, through both
/// drains. The Luau spelling swaps `:` for ` =` and `[...]` for `{...}`.
fn drain_both(entries: &[&str]) -> (GhostStyleRegistry, GhostStyleRegistry) {
    let js = format!("({{ ghostStyles: [{}] }})", entries.join(", "));
    let lua_entries: Vec<String> = entries
        .iter()
        .map(|entry| {
            entry
                .replace(": ", " = ")
                .replace('[', "{")
                .replace(']', "}")
        })
        .collect();
    let lua = format!(
        "return {{ ghostStyles = {{ {} }} }}",
        lua_entries.join(", ")
    );
    (drain_js(&js), drain_lua(&lua))
}

fn warnings(capture: &LogCapture, needle: &str) -> Vec<String> {
    capture
        .records()
        .into_iter()
        .filter(|record| record.level == log::Level::Warn && record.message.contains(needle))
        .map(|record| record.message)
        .collect()
}

const ECHO: &str = r#"{ name: "echo", tint: [0.6, 0.8, 1.0], opacity: 0.45, saturation: 0.2, audio: true, impactAudio: false }"#;

#[test]
fn declared_ghost_styles_drain_alike_in_both_runtimes() {
    let capture = LogCapture::start();
    let (js, lua) = drain_both(&[ECHO, r#"{ name: "veil", impactAudio: true }"#]);
    assert_eq!(js, lua);
    assert_eq!(js.names(), ["echo", "veil"]);
    let echo = js.get("echo").expect("echo is declared");
    assert_eq!(echo.tint, [0.6, 0.8, 1.0]);
    assert_eq!(echo.opacity, 0.45);
    assert_eq!(echo.saturation, 0.2);
    assert!(echo.audio);
    assert!(!echo.impact_audio);
    // Every omitted field takes the floor style's value; `impactAudio` is
    // stored as authored, and only takes effect with `audio`.
    let veil = js.get("veil").expect("veil is declared");
    assert_eq!(
        *veil,
        GhostStyle {
            impact_audio: true,
            ..GhostStyle::FLOOR
        }
    );
    assert!(!veil.carries_impact_audio());
    assert!(warnings(&capture, "ghostStyles").is_empty());
}

#[test]
fn absent_or_null_ghost_styles_declare_none_without_a_warning() {
    let capture = LogCapture::start();
    assert!(drain_js("({})").is_empty());
    assert!(drain_js("({ ghostStyles: null })").is_empty());
    assert!(drain_js("({ ghostStyles: {} })").is_empty());
    assert!(drain_lua("return {}").is_empty());
    assert!(drain_lua("return { ghostStyles = {} }").is_empty());
    assert!(warnings(&capture, "ghostStyles").is_empty());
}

#[test]
fn null_optional_fields_read_as_omitted_in_both_runtimes() {
    let (js, _) = drain_both(&[r#"{ name: "echo", tint: null, opacity: null }"#]);
    let lua = drain_lua(r#"return { ghostStyles = { { name = "echo", tint = nil } } }"#);
    assert_eq!(js, lua);
    assert_eq!(js.get("echo"), Some(&GhostStyle::FLOOR));
}

// A style with one malformed field drops whole with a
// warning naming `ghostStyles[i].<field>`; its siblings stand, and both
// runtimes word the warning identically.
#[test]
fn a_style_with_one_malformed_field_drops_whole_naming_the_field_in_both_runtimes() {
    for (field, bad) in [
        ("tint", r#"tint: [0.5, 1.5, 0.5]"#),
        ("tint", r#"tint: [-0.1, 0.5, 0.5]"#),
        ("tint", r#"tint: [0.5, 0.5]"#),
        ("tint", r#"tint: [0.5, 0.5, 0.5, 0.5]"#),
        ("tint", r#"tint: [0.5, "red", 0.5]"#),
        ("tint", r#"tint: 0.5"#),
        ("opacity", r#"opacity: 1.5"#),
        ("opacity", r#"opacity: -0.25"#),
        ("opacity", r#"opacity: "half""#),
        ("saturation", r#"saturation: 2"#),
        ("saturation", r#"saturation: true"#),
        ("audio", r#"audio: 1"#),
        ("audio", r#"audio: "yes""#),
        ("impactAudio", r#"impactAudio: 0"#),
        ("name", r#"name: """#),
        ("name", r#"name: 7"#),
        ("glow", r#"glow: 3"#),
    ] {
        let capture = LogCapture::start();
        // A later `name` key would override an earlier one; put the bad field
        // where it replaces the entry's own name for the name cases.
        let entry = if field == "name" {
            format!("{{ {bad}, opacity: 0.3 }}")
        } else {
            format!(r#"{{ name: "bad", {bad} }}"#)
        };
        let (js, lua) = drain_both(&[ECHO, &entry, r#"{ name: "veil" }"#]);
        assert_eq!(js, lua, "{bad}");
        assert_eq!(js.names(), ["echo", "veil"], "{bad}: the rest stands");
        assert!(js.get("bad").is_none(), "{bad}: dropped whole");
        let rejected = warnings(&capture, &format!("ghostStyles[1].{field}"));
        assert_eq!(rejected.len(), 2, "{bad}: one per runtime: {rejected:?}");
        assert_eq!(
            rejected[0], rejected[1],
            "{bad}: the runtimes' diagnostics diverged"
        );
        assert!(
            warnings(&capture, "ghostStyles[0]").is_empty()
                && warnings(&capture, "ghostStyles[2]").is_empty(),
            "{bad}: only the malformed entry warns"
        );
    }
}

#[test]
fn a_missing_name_drops_the_style_naming_the_field() {
    let capture = LogCapture::start();
    let (js, lua) = drain_both(&[r#"{ opacity: 0.3 }"#, ECHO]);
    assert_eq!(js, lua);
    assert_eq!(js.names(), ["echo"]);
    let rejected = warnings(
        &capture,
        "mod manifest: ghostStyles[0] is malformed and was skipped: `ghostStyles[0].name` is required",
    );
    assert_eq!(rejected.len(), 2, "{rejected:?}");
}

#[test]
fn an_out_of_range_field_names_the_style_it_rejected() {
    let capture = LogCapture::start();
    drain_both(&[r#"{ name: "glare", opacity: 1.01 }"#]);
    let rejected = warnings(
        &capture,
        "mod manifest: ghostStyles[0].opacity must be a finite value in [0, 1], got 1.01; ghost style 'glare' was rejected",
    );
    assert_eq!(rejected.len(), 2, "{rejected:?}");
}

// A later duplicate name drops with a warning, and the
// first declaration stands, in both runtimes.
#[test]
fn a_later_duplicate_style_name_drops_and_the_first_declaration_stands() {
    let capture = LogCapture::start();
    let (js, lua) = drain_both(&[
        ECHO,
        r#"{ name: "veil" }"#,
        r#"{ name: "echo", opacity: 0.9, audio: false }"#,
    ]);
    assert_eq!(js, lua);
    assert_eq!(js.names(), ["echo", "veil"]);
    let echo = js.get("echo").unwrap();
    assert_eq!(echo.opacity, 0.45, "the first declaration stands");
    assert!(echo.audio);
    let duplicate = warnings(
        &capture,
        "ghostStyles[2].name duplicates ghost style 'echo' declared at ghostStyles[0]; the first declaration stands",
    );
    assert_eq!(duplicate.len(), 2, "one per runtime: {duplicate:?}");
    assert_eq!(duplicate[0], duplicate[1]);
}

// A malformed first declaration of a name does not claim it: a later valid
// declaration of the same name is the first that stands.
#[test]
fn a_dropped_entry_never_claims_its_name() {
    let (js, lua) = drain_both(&[
        r#"{ name: "echo", opacity: 3 }"#,
        r#"{ name: "echo", opacity: 0.25 }"#,
    ]);
    assert_eq!(js, lua);
    assert_eq!(js.get("echo").map(|style| style.opacity), Some(0.25));
}

#[test]
fn a_hole_in_ghost_styles_warns_for_that_entry_only_in_both_runtimes() {
    let capture = LogCapture::start();
    let js = drain_js(&format!(
        "({{ ghostStyles: [{ECHO}, undefined, {{ name: \"veil\" }}] }})"
    ));
    let lua = drain_lua(&format!(
        "return {{ ghostStyles = {{ {}, nil, {{ name = \"veil\" }} }} }}",
        ECHO.replace(": ", " = ")
            .replace('[', "{")
            .replace(']', "}")
    ));
    assert_eq!(js, lua);
    assert_eq!(js.names(), ["echo", "veil"]);
    let skipped = warnings(&capture, "ghostStyles[1] is malformed and was skipped");
    assert_eq!(skipped.len(), 2, "{skipped:?}");
    assert_eq!(skipped[0], skipped[1]);
}

// `ghostStyles` itself is never a reason to reject: a non-list warns once and
// declares none.
#[test]
fn a_non_array_ghost_styles_field_warns_and_declares_none() {
    for (js, lua) in [
        ("({ ghostStyles: 3 })", "return { ghostStyles = 3 }"),
        (
            "({ ghostStyles: \"echo\" })",
            "return { ghostStyles = \"echo\" }",
        ),
        (
            "({ ghostStyles: { echo: { name: \"echo\" } } })",
            "return { ghostStyles = { echo = { name = \"echo\" } } }",
        ),
    ] {
        let capture = LogCapture::start();
        assert!(drain_js(js).is_empty(), "{js}");
        assert!(drain_lua(lua).is_empty(), "{lua}");
        let ignored = warnings(
            &capture,
            "`ghostStyles` must be an array of ghost styles; ignoring the field",
        );
        assert_eq!(ignored.len(), 2, "{js}: {ignored:?}");
    }
}

#[test]
fn a_ghost_styles_array_past_4096_slots_warns_once_and_is_ignored_in_both_runtimes() {
    let capture = LogCapture::start();
    let js = drain_js(
        "({ ghostStyles: (() => { const a = []; a[4096] = { name: 'x' }; return a; })() })",
    );
    let lua = drain_lua("return { ghostStyles = { [4097] = { name = 'x' } } }");
    assert!(js.is_empty());
    assert!(lua.is_empty());
    let oversized = warnings(
        &capture,
        "`ghostStyles` exceeds 4096 array slots; ignoring the field",
    );
    assert_eq!(oversized.len(), 2, "{oversized:?}");
}
