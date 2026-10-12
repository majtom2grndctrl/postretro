// Tests: `movement.swim` parsing and validation in both runtimes.

use super::super::*;
use super::common::*;
use postretro_test_log_capture::LogCapture;

fn js_movement_with_swim(swim_body: &str) -> String {
    format!(
        r#"({{
            canonicalName: "player",
            components: {{
                movement: {{
                    capsule: {{ radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 }},
                    ground: {{ speed: {{ walk: 7.0, run: 11.0, crouch: 3.0 }}, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 }},
                    air: {{ forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 }},
                    fall: {{ terminalVelocity: 40.0 }},
                    swim: {swim_body}
                }}
            }}
        }})"#
    )
}

fn lua_movement_with_swim(swim_body: &str) -> String {
    format!(
        r#"return {{
            canonicalName = "player",
            components = {{
                movement = {{
                    capsule = {{ radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 }},
                    ground = {{ speed = {{ walk = 7.0, run = 11.0, crouch = 3.0 }}, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 }},
                    air = {{ forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 }},
                    fall = {{ terminalVelocity = 40.0 }},
                    swim = {swim_body}
                }}
            }}
        }}"#
    )
}

/// content/dev's example swim block (`scripts/player.ts`), one `(key, value)` per field.
const SWIM: [(&str, &str); 6] = [
    ("speed", "4.5"),
    ("accel", "6.0"),
    ("sinkSpeed", "0.8"),
    ("enterDepth", "0.5"),
    ("exitDepth", "0.35"),
    ("fluidJumpVelocity", "6.5"),
];

fn js_swim(overrides: &[(&str, &str)]) -> String {
    let fields: Vec<String> = SWIM
        .iter()
        .map(|(key, value)| {
            let value = overrides
                .iter()
                .find(|(k, _)| k == key)
                .map_or(*value, |(_, v)| *v);
            format!("{key}: {value}")
        })
        .collect();
    format!("{{ {} }}", fields.join(", "))
}

fn lua_swim(overrides: &[(&str, &str)]) -> String {
    js_swim(overrides).replace(": ", " = ")
}

/// Parse a descriptor whose swim block is `js_body` (QuickJS) and `lua_body`
/// (Luau) and return each runtime's parsed swim block with the
/// `movement.swim` warnings it logged. Both descriptors must load.
fn parse_both(js_body: &str, lua_body: &str) -> [(Option<SwimParams>, Vec<String>); 2] {
    let js = {
        let capture = LogCapture::start();
        let descriptor = eval_js(&js_movement_with_swim(js_body), |ctx, v| {
            entity_descriptor_from_js(ctx, v).expect("the descriptor loads")
        });
        (
            descriptor.movement.expect("movement present").swim,
            swim_warnings(&capture),
        )
    };
    let lua = {
        let capture = LogCapture::start();
        let descriptor = eval_lua(&lua_movement_with_swim(lua_body), |v| {
            entity_descriptor_from_lua(v).expect("the descriptor loads")
        });
        (
            descriptor.movement.expect("movement present").swim,
            swim_warnings(&capture),
        )
    };
    [js, lua]
}

fn swim_warnings(capture: &LogCapture) -> Vec<String> {
    capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level == log::Level::Warn && record.message.contains("movement.swim")
        })
        .map(|record| record.message)
        .collect()
}

/// Assert both runtimes dropped the block with one identical warning naming
/// `needle`, and return that warning.
fn assert_dropped_naming(js_body: &str, lua_body: &str, needle: &str) -> String {
    let [(js_swim, js_warnings), (lua_swim, lua_warnings)] = parse_both(js_body, lua_body);
    assert_eq!(js_swim, None, "QuickJS drops the block ({needle})");
    assert_eq!(lua_swim, None, "Luau drops the block ({needle})");
    assert_eq!(js_warnings.len(), 1, "one QuickJS warning: {js_warnings:?}");
    assert_eq!(
        js_warnings, lua_warnings,
        "the runtimes' diagnostics diverged"
    );
    let warning = js_warnings.into_iter().next().unwrap();
    assert!(warning.contains(needle), "names `{needle}`: {warning}");
    assert!(
        warning.contains("the movement.swim block was dropped, so the player wades"),
        "{warning}"
    );
    warning
}

#[test]
fn movement_swim_block_parses_identically_in_both_runtimes() {
    let js = eval_js(&js_movement_with_swim(&js_swim(&[])), |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap()
    });
    let lua = eval_lua(&lua_movement_with_swim(&lua_swim(&[])), |v| {
        entity_descriptor_from_lua(v).unwrap()
    });
    let swim = js
        .movement
        .as_ref()
        .unwrap()
        .swim
        .clone()
        .expect("swim present");
    assert_eq!(
        swim,
        SwimParams {
            speed: 4.5,
            accel: 6.0,
            sink_speed: 0.8,
            enter_depth: 0.5,
            exit_depth: 0.35,
            fluid_jump_velocity: 6.5,
        }
    );
    assert_eq!(lua.movement.unwrap().swim, Some(swim));
}

#[test]
fn movement_without_a_swim_block_cannot_swim() {
    let js = eval_js(JS_PLAYER_MOVEMENT, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap()
    });
    assert!(js.movement.expect("movement present").swim.is_none());
}

// A swim block whose `exitDepth` is not below `enterDepth` is dropped with a
// warning naming the field; the rest of the descriptor loads and the player
// wades.
#[test]
fn swim_block_with_exit_depth_at_or_above_enter_depth_is_dropped_naming_exit_depth() {
    for exit in ["0.5", "0.75"] {
        let overrides = [("exitDepth", exit)];
        assert_dropped_naming(
            &js_swim(&overrides),
            &lua_swim(&overrides),
            "`movement.swim.exitDepth` must be less than `movement.swim.enterDepth`",
        );
    }
}

#[test]
fn swim_block_drops_each_out_of_range_field_by_name() {
    for (field, value) in [
        ("speed", "-1.0"),
        ("accel", "-0.5"),
        ("sinkSpeed", "-0.1"),
        ("enterDepth", "1.5"),
        ("exitDepth", "-0.1"),
        ("fluidJumpVelocity", "-6.5"),
    ] {
        let overrides = [(field, value)];
        assert_dropped_naming(
            &js_swim(&overrides),
            &lua_swim(&overrides),
            &format!("`movement.swim.{field}`"),
        );
    }
}

// A missing or wrongly typed field names `movement.swim.<field>`, never the
// generic descriptor wording.
#[test]
fn swim_block_missing_a_field_is_dropped_naming_that_field() {
    let warning = assert_dropped_naming(
        "{ speed: 4.5, accel: 6.0 }",
        "{ speed = 4.5, accel = 6.0 }",
        "`movement.swim.sinkSpeed` is required",
    );
    assert!(!warning.contains("reaction descriptor"), "{warning}");
    assert_dropped_naming(
        &js_swim(&[("exitDepth", "null")]),
        &lua_swim(&[("exitDepth", "nil")]),
        "`movement.swim.exitDepth` is required",
    );
}

#[test]
fn swim_block_with_a_non_number_field_is_dropped_naming_that_field() {
    let overrides = [("accel", "\"fast\"")];
    assert_dropped_naming(
        &js_swim(&overrides),
        &lua_swim(&overrides),
        "`movement.swim.accel` must be a number",
    );
}

#[test]
fn a_swim_value_that_is_not_an_object_is_dropped() {
    assert_dropped_naming("4", "4", "`movement.swim` must be an object");
}

#[test]
fn a_valid_or_absent_swim_block_logs_no_swim_warning() {
    let [(js, js_warnings), (lua, lua_warnings)] = parse_both(&js_swim(&[]), &lua_swim(&[]));
    assert!(js.is_some() && lua.is_some());
    assert!(js_warnings.is_empty() && lua_warnings.is_empty());
    let [(js, js_warnings), (lua, lua_warnings)] = parse_both("undefined", "nil");
    assert_eq!((js, lua), (None, None));
    assert!(js_warnings.is_empty() && lua_warnings.is_empty());
}
