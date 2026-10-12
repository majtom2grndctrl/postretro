// Tests: `movement.swim` parsing and validation in both runtimes.

use super::super::*;
use super::common::*;

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

/// The brief's Scripting surface block, one `(key, value)` per field.
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

fn js_error(overrides: &[(&str, &str)]) -> String {
    let src = js_movement_with_swim(&js_swim(overrides));
    eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err().to_string()
    })
}

fn lua_error(overrides: &[(&str, &str)]) -> String {
    let src = lua_movement_with_swim(&lua_swim(overrides));
    eval_lua(&src, |v| {
        entity_descriptor_from_lua(v).unwrap_err().to_string()
    })
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

#[test]
fn swim_block_with_exit_depth_at_or_above_enter_depth_is_rejected_naming_exit_depth() {
    for exit in ["0.5", "0.75"] {
        for error in [
            js_error(&[("exitDepth", exit)]),
            lua_error(&[("exitDepth", exit)]),
        ] {
            assert!(
                error.contains("movement.swim.exitDepth"),
                "the rejection names the field: {error}"
            );
        }
    }
}

#[test]
fn swim_block_rejects_each_out_of_range_field_by_name() {
    for (field, value) in [
        ("speed", "-1.0"),
        ("accel", "-0.5"),
        ("sinkSpeed", "-0.1"),
        ("enterDepth", "1.5"),
        ("exitDepth", "-0.1"),
        ("fluidJumpVelocity", "-6.5"),
    ] {
        let expected = format!("movement.swim.{field}");
        for error in [js_error(&[(field, value)]), lua_error(&[(field, value)])] {
            assert!(error.contains(&expected), "{field}: {error}");
        }
    }
}

#[test]
fn swim_block_requires_every_field() {
    let js_src = js_movement_with_swim("{ speed: 4.5, accel: 6.0 }");
    let js = eval_js(&js_src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(
        js,
        DescriptorError::MissingField { field: "sinkSpeed" }
    ));
    let lua_src = lua_movement_with_swim("{ speed = 4.5, accel = 6.0 }");
    let lua = eval_lua(&lua_src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(
        lua,
        DescriptorError::MissingField { field: "sinkSpeed" }
    ));
}
