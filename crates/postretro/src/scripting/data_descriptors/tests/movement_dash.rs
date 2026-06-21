// Tests: dash params + expression parsing.

use super::super::*;
use super::common::*;

// --- DashParams parsing ------------------------------------------------

/// JS movement block with a `dash` sub-object spliced into the `movement`
/// object. `dash_body` is the inner `{ ... }` text (no `dash:` key).
fn js_movement_with_dash(dash_body: &str) -> String {
    format!(
        r#"({{
            canonicalName: "player",
            components: {{
                movement: {{
                    capsule: {{ radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 }},
                    ground: {{ speed: {{ walk: 7.0, run: 11.0, crouch: 3.0 }}, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 }},
                    air: {{ forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 }},
                    fall: {{ terminalVelocity: 40.0 }},
                    dash: {dash_body}
                }}
            }}
        }})"#
    )
}

/// Luau movement block with a `dash` sub-table spliced in.
fn lua_movement_with_dash(dash_body: &str) -> String {
    format!(
        r#"return {{
            canonicalName = "player",
            components = {{
                movement = {{
                    capsule = {{ radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 }},
                    ground = {{ speed = {{ walk = 7.0, run = 11.0, crouch = 3.0 }}, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 }},
                    air = {{ forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 }},
                    fall = {{ terminalVelocity = 40.0 }},
                    dash = {dash_body}
                }}
            }}
        }}"#
    )
}

const JS_DASH_FULL: &str = r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#;
const LUA_DASH_FULL: &str = r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = false }"#;

#[test]
fn js_movement_dash_absent_is_valid_and_disabled() {
    let d = eval_js(JS_PLAYER_MOVEMENT, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap()
    });
    assert!(d.movement.expect("movement present").dash.is_none());
}

#[test]
fn lua_movement_dash_absent_is_valid_and_disabled() {
    let src = r#"return {
        canonicalName = "player",
        components = {
            movement = {
                capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                fall = { terminalVelocity = 40.0 }
            }
        }
    }"#;
    let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
    assert!(d.movement.expect("movement present").dash.is_none());
}

#[test]
fn js_movement_dash_full_shape_parses() {
    let src = js_movement_with_dash(JS_DASH_FULL);
    let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let dash = d.movement.unwrap().dash.expect("dash present");
    assert_eq!(dash.boost_speed, 18.0);
    assert_eq!(dash.momentum_retention, 0.5);
    assert_eq!(dash.steer_control, 0.25);
    assert_eq!(dash.dash_drag, 60.0);
    assert_eq!(dash.cooldown_ms, 800.0);
    assert_eq!(dash.air_dashes, 1);
    assert_eq!(dash.preserve_vertical, false);
}

#[test]
fn lua_movement_dash_full_shape_parses() {
    let src = lua_movement_with_dash(LUA_DASH_FULL);
    let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
    let dash = d.movement.unwrap().dash.expect("dash present");
    assert_eq!(dash.boost_speed, 18.0);
    assert_eq!(dash.momentum_retention, 0.5);
    assert_eq!(dash.steer_control, 0.25);
    assert_eq!(dash.dash_drag, 60.0);
    assert_eq!(dash.cooldown_ms, 800.0);
    assert_eq!(dash.air_dashes, 1);
    assert_eq!(dash.preserve_vertical, false);
}

#[test]
fn js_movement_dash_zero_dash_drag_and_cooldown_accepted() {
    // dashDrag and cooldownMs both legitimately permit 0.
    let src = js_movement_with_dash(
        r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 0.0, cooldownMs: 0.0, airDashes: 0, preserveVertical: true }"#,
    );
    let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let dash = d.movement.unwrap().dash.expect("dash present");
    assert_eq!(dash.dash_drag, 0.0);
    assert_eq!(dash.cooldown_ms, 0.0);
    assert_eq!(dash.preserve_vertical, true);
}

#[test]
fn js_movement_dash_boost_speed_zero_is_rejected() {
    let src = js_movement_with_dash(
        r#"{ boostSpeed: 0.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
    );
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_dash_boost_speed_negative_is_rejected() {
    let src = js_movement_with_dash(
        r#"{ boostSpeed: -1.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
    );
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_dash_momentum_retention_out_of_range_is_rejected() {
    let src = js_movement_with_dash(
        r#"{ boostSpeed: 18.0, momentumRetention: 1.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
    );
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_dash_steer_control_out_of_range_is_rejected() {
    let src = js_movement_with_dash(
        r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: -0.1, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
    );
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_dash_negative_drag_is_rejected() {
    let src = js_movement_with_dash(
        r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: -1.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
    );
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_dash_negative_cooldown_is_rejected() {
    let src = js_movement_with_dash(
        r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: -5.0, airDashes: 1, preserveVertical: false }"#,
    );
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_dash_air_dashes_non_integer_is_rejected() {
    let src = js_movement_with_dash(
        r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1.5, preserveVertical: false }"#,
    );
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_dash_air_dashes_negative_is_rejected() {
    let src = js_movement_with_dash(
        r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: -1, preserveVertical: false }"#,
    );
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_dash_preserve_vertical_wrong_type_is_rejected() {
    let src = js_movement_with_dash(
        r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: "yes" }"#,
    );
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_dash_missing_field_reports_missing_field() {
    // boostSpeed omitted.
    let src = js_movement_with_dash(
        r#"{ momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
    );
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert_eq!(
        err,
        DescriptorError::MissingField {
            field: "boostSpeed",
        }
    );
}

#[test]
fn lua_movement_dash_boost_speed_zero_is_rejected() {
    let src = lua_movement_with_dash(
        r#"{ boostSpeed = 0.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = false }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_dash_steer_control_out_of_range_is_rejected() {
    let src = lua_movement_with_dash(
        r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 1.5, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = false }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_dash_negative_cooldown_is_rejected() {
    let src = lua_movement_with_dash(
        r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = -5.0, airDashes = 1, preserveVertical = false }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_dash_zero_drag_and_cooldown_accepted() {
    let src = lua_movement_with_dash(
        r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 0.0, cooldownMs = 0.0, airDashes = 0, preserveVertical = true }"#,
    );
    let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
    let dash = d.movement.unwrap().dash.expect("dash present");
    assert_eq!(dash.dash_drag, 0.0);
    assert_eq!(dash.cooldown_ms, 0.0);
    assert_eq!(dash.air_dashes, 0);
}

#[test]
fn lua_movement_dash_air_dashes_non_integer_is_rejected() {
    let src = lua_movement_with_dash(
        r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1.5, preserveVertical = false }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_dash_preserve_vertical_wrong_type_is_rejected() {
    let src = lua_movement_with_dash(
        r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = "yes" }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_dash_missing_field_reports_missing_field() {
    // preserveVertical omitted.
    let src = lua_movement_with_dash(
        r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1 }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert_eq!(
        err,
        DescriptorError::MissingField {
            field: "preserveVertical",
        }
    );
}

// --- Dash expression-capable fields ------------------------------------

#[test]
fn js_movement_dash_number_field_accepts_expression() {
    // `boostSpeed` is a clamped read of the movement-local `speed` input — a
    // well-typed Number-rooted expression. It must bind and round-trip as an
    // `Ir` variant (literal() is therefore None).
    let src = js_movement_with_dash(
        r#"{ boostSpeed: { op: "clamp", x: { op: "input", name: "speed" }, lo: { op: "const", value: 0.0 }, hi: { op: "const", value: 30.0 } }, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
    );
    let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let dash = d.movement.unwrap().dash.expect("dash present");
    assert!(
        matches!(dash.boost_speed, NumberOrIr::Ir(_)),
        "expression field parses to the Ir variant"
    );
    assert_eq!(dash.boost_speed.literal(), None);
    // The other (literal) fields keep their literal sugar / behavior.
    assert_eq!(dash.momentum_retention, 0.5);
    assert_eq!(dash.air_dashes, 1);
    assert_eq!(dash.preserve_vertical, false);
}

#[test]
fn js_movement_dash_bool_field_accepts_expression() {
    // `preserveVertical` as a Bool-rooted comparison over `verticalSpeed`.
    let src = js_movement_with_dash(
        r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: { op: "gt", a: { op: "input", name: "verticalSpeed" }, b: { op: "const", value: 0.0 } } }"#,
    );
    let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let dash = d.movement.unwrap().dash.expect("dash present");
    assert!(matches!(dash.preserve_vertical, BoolOrIr::Ir(_)));
    assert_eq!(dash.preserve_vertical.literal(), None);
}

#[test]
fn js_movement_dash_expression_unknown_read_is_rejected() {
    let src = js_movement_with_dash(
        r#"{ boostSpeed: { op: "input", name: "notARealInput" }, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
    );
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_dash_expression_type_table_violation_is_rejected() {
    // `clamp` requires number operands; a boolean `grounded` input violates
    // the type table. Bind rejects without panicking.
    let src = js_movement_with_dash(
        r#"{ boostSpeed: { op: "clamp", x: { op: "input", name: "grounded" }, lo: { op: "const", value: 0.0 }, hi: { op: "const", value: 1.0 } }, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
    );
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_dash_expression_root_type_mismatch_is_rejected() {
    // A boolean-rooted expression in a Number field: bind alone (no output)
    // never checks the root type, so this exercises the explicit root-type
    // check in `validate_dash_expr`.
    let src = js_movement_with_dash(
        r#"{ boostSpeed: { op: "gt", a: { op: "input", name: "speed" }, b: { op: "const", value: 1.0 } }, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
    );
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_dash_expression_number_rooted_in_bool_field_is_rejected() {
    // A number-rooted expression in the Boolean field `preserveVertical`
    // must be rejected: `validate_dash_expr` checks the root type against
    // `IrType::Bool` and returns `InvalidShape` on mismatch.
    let src = js_movement_with_dash(
        r#"{ boostSpeed: 18.0, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: { op: "input", name: "speed" } }"#,
    );
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_dash_expression_number_rooted_in_bool_field_is_rejected() {
    // Mirror of the JS case: a number-rooted expression placed in the
    // Boolean field `preserveVertical` is rejected via the same
    // `validate_dash_expr(node, IrType::Bool, …)` arm.
    let src = lua_movement_with_dash(
        r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = { op = "input", name = "speed" } }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_dash_malformed_node_object_is_rejected() {
    // An object that is not a recognizable node shape (no valid `op`).
    let src = js_movement_with_dash(
        r#"{ boostSpeed: { notAnOp: 1 }, momentumRetention: 0.5, steerControl: 0.25, dashDrag: 60.0, cooldownMs: 800.0, airDashes: 1, preserveVertical: false }"#,
    );
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_dev_player_dash_expressions_bind() {
    // Guards the dev player descriptor (`content/dev/scripts/player.ts`): the
    // exact IR the authored `runtime.*` builders emit for `momentumRetention`
    // and `steerControl` must bind without a `DescriptorError`. If the scope
    // names, op vocabulary, or root types ever drift, this fails before the
    // dev map does. `momentumRetention` is a `select` on the boolean
    // `grounded` input; `steerControl` clamps an `elapsedMs / 150` ramp into
    // [0, 1] — the 150 ms window sits inside the engine's 200 ms DASH_MAX_MS
    // bound so the ramp stays observable.
    let src = js_movement_with_dash(
        r#"{
            boostSpeed: 22.0,
            momentumRetention: { op: "select", cond: { op: "input", name: "grounded" }, a: { op: "const", value: 0.4 }, b: { op: "const", value: 0.7 } },
            steerControl: { op: "clamp", x: { op: "div", a: { op: "input", name: "elapsedMs" }, b: { op: "const", value: 150.0 } }, lo: { op: "const", value: 0.0 }, hi: { op: "const", value: 1.0 } },
            dashDrag: 0,
            cooldownMs: 600,
            airDashes: 1,
            preserveVertical: false
        }"#,
    );
    let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let dash = d.movement.unwrap().dash.expect("dash present");
    // Both authored fields bind as expressions; the literal fields keep sugar.
    assert!(matches!(dash.momentum_retention, NumberOrIr::Ir(_)));
    assert!(matches!(dash.steer_control, NumberOrIr::Ir(_)));
    assert_eq!(dash.boost_speed, 22.0);
    assert_eq!(dash.air_dashes, 1);
    assert_eq!(dash.preserve_vertical, false);
}

#[test]
fn lua_movement_dash_number_field_accepts_expression() {
    let src = lua_movement_with_dash(
        r#"{ boostSpeed = { op = "clamp", x = { op = "input", name = "speed" }, lo = { op = "const", value = 0.0 }, hi = { op = "const", value = 30.0 } }, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = false }"#,
    );
    let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
    let dash = d.movement.unwrap().dash.expect("dash present");
    assert!(matches!(dash.boost_speed, NumberOrIr::Ir(_)));
    assert_eq!(dash.boost_speed.literal(), None);
    assert_eq!(dash.momentum_retention, 0.5);
}

#[test]
fn lua_movement_dash_bool_field_accepts_expression() {
    let src = lua_movement_with_dash(
        r#"{ boostSpeed = 18.0, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = { op = "gt", a = { op = "input", name = "verticalSpeed" }, b = { op = "const", value = 0.0 } } }"#,
    );
    let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
    let dash = d.movement.unwrap().dash.expect("dash present");
    assert!(matches!(dash.preserve_vertical, BoolOrIr::Ir(_)));
}

#[test]
fn lua_movement_dash_expression_unknown_read_is_rejected() {
    let src = lua_movement_with_dash(
        r#"{ boostSpeed = { op = "input", name = "notARealInput" }, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = false }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_dash_expression_type_table_violation_is_rejected() {
    let src = lua_movement_with_dash(
        r#"{ boostSpeed = { op = "clamp", x = { op = "input", name = "grounded" }, lo = { op = "const", value = 0.0 }, hi = { op = "const", value = 1.0 } }, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = false }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_dash_expression_root_type_mismatch_is_rejected() {
    let src = lua_movement_with_dash(
        r#"{ boostSpeed = { op = "gt", a = { op = "input", name = "speed" }, b = { op = "const", value = 1.0 } }, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = false }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_dash_malformed_node_object_is_rejected() {
    let src = lua_movement_with_dash(
        r#"{ boostSpeed = { notAnOp = 1 }, momentumRetention = 0.5, steerControl = 0.25, dashDrag = 60.0, cooldownMs = 800.0, airDashes = 1, preserveVertical = false }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}
