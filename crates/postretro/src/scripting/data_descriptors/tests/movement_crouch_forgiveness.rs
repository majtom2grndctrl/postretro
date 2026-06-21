// Tests: crouch + forgiveness parsing.

use super::super::*;
use super::common::*;

// --- CrouchParams parsing ----------------------------------------------

/// JS movement block with a `crouch` sub-object spliced into the `movement`
/// object. `crouch_body` is the inner `{ ... }` text (no `crouch:` key).
/// Capsule radius is 0.4 so the crouched `eyeHeight` upper bound is
/// `crouch.halfHeight + 0.4`.
fn js_movement_with_crouch(crouch_body: &str) -> String {
    format!(
        r#"({{
            canonicalName: "player",
            components: {{
                movement: {{
                    capsule: {{ radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 }},
                    ground: {{ speed: {{ walk: 7.0, run: 11.0, crouch: 3.0 }}, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 }},
                    air: {{ forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 }},
                    fall: {{ terminalVelocity: 40.0 }},
                    crouch: {crouch_body}
                }}
            }}
        }})"#
    )
}

/// Luau movement block with a `crouch` sub-table spliced in.
fn lua_movement_with_crouch(crouch_body: &str) -> String {
    format!(
        r#"return {{
            canonicalName = "player",
            components = {{
                movement = {{
                    capsule = {{ radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 }},
                    ground = {{ speed = {{ walk = 7.0, run = 11.0, crouch = 3.0 }}, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 }},
                    air = {{ forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 }},
                    fall = {{ terminalVelocity = 40.0 }},
                    crouch = {crouch_body}
                }}
            }}
        }}"#
    )
}

const JS_CROUCH_FULL: &str = r#"{ halfHeight: 0.4, eyeHeight: 0.3, transitionRate: 8.0 }"#;
const LUA_CROUCH_FULL: &str = r#"{ halfHeight = 0.4, eyeHeight = 0.3, transitionRate = 8.0 }"#;

#[test]
fn js_movement_crouch_absent_is_valid_and_disabled() {
    let d = eval_js(JS_PLAYER_MOVEMENT, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap()
    });
    assert!(d.movement.expect("movement present").crouch.is_none());
}

#[test]
fn lua_movement_crouch_absent_is_valid_and_disabled() {
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
    assert!(d.movement.expect("movement present").crouch.is_none());
}

#[test]
fn js_movement_crouch_full_shape_parses() {
    let src = js_movement_with_crouch(JS_CROUCH_FULL);
    let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let crouch = d.movement.unwrap().crouch.expect("crouch present");
    assert_eq!(crouch.half_height, 0.4);
    assert_eq!(crouch.eye_height, 0.3);
    assert_eq!(crouch.transition_rate, 8.0);
}

#[test]
fn lua_movement_crouch_full_shape_parses() {
    let src = lua_movement_with_crouch(LUA_CROUCH_FULL);
    let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
    let crouch = d.movement.unwrap().crouch.expect("crouch present");
    assert_eq!(crouch.half_height, 0.4);
    assert_eq!(crouch.eye_height, 0.3);
    assert_eq!(crouch.transition_rate, 8.0);
}

#[test]
fn js_movement_crouch_eye_height_at_capsule_top_is_accepted() {
    // Inclusive upper bound: halfHeight (0.4) + radius (0.4) = 0.8.
    let src =
        js_movement_with_crouch(r#"{ halfHeight: 0.4, eyeHeight: 0.8, transitionRate: 8.0 }"#);
    let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    assert_eq!(d.movement.unwrap().crouch.unwrap().eye_height, 0.8);
}

#[test]
fn js_movement_crouch_half_height_zero_is_rejected() {
    let src =
        js_movement_with_crouch(r#"{ halfHeight: 0.0, eyeHeight: 0.3, transitionRate: 8.0 }"#);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_crouch_half_height_negative_is_rejected() {
    let src =
        js_movement_with_crouch(r#"{ halfHeight: -0.4, eyeHeight: 0.3, transitionRate: 8.0 }"#);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_crouch_eye_height_zero_is_rejected() {
    // Exclusive lower bound: 0 is rejected.
    let src =
        js_movement_with_crouch(r#"{ halfHeight: 0.4, eyeHeight: 0.0, transitionRate: 8.0 }"#);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_crouch_eye_height_above_capsule_top_is_rejected() {
    // halfHeight (0.4) + radius (0.4) = 0.8; 0.9 exceeds the crouched top.
    let src =
        js_movement_with_crouch(r#"{ halfHeight: 0.4, eyeHeight: 0.9, transitionRate: 8.0 }"#);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_crouch_transition_rate_zero_is_rejected() {
    let src =
        js_movement_with_crouch(r#"{ halfHeight: 0.4, eyeHeight: 0.3, transitionRate: 0.0 }"#);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_crouch_transition_rate_negative_is_rejected() {
    let src =
        js_movement_with_crouch(r#"{ halfHeight: 0.4, eyeHeight: 0.3, transitionRate: -1.0 }"#);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_crouch_missing_field_reports_missing_field() {
    // transitionRate omitted.
    let src = js_movement_with_crouch(r#"{ halfHeight: 0.4, eyeHeight: 0.3 }"#);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert_eq!(
        err,
        DescriptorError::MissingField {
            field: "transitionRate",
        }
    );
}

#[test]
fn lua_movement_crouch_half_height_zero_is_rejected() {
    let src =
        lua_movement_with_crouch(r#"{ halfHeight = 0.0, eyeHeight = 0.3, transitionRate = 8.0 }"#);
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_crouch_half_height_negative_is_rejected() {
    let src =
        lua_movement_with_crouch(r#"{ halfHeight = -0.4, eyeHeight = 0.3, transitionRate = 8.0 }"#);
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_crouch_eye_height_zero_is_rejected() {
    // Exclusive lower bound: 0 is rejected.
    let src =
        lua_movement_with_crouch(r#"{ halfHeight = 0.4, eyeHeight = 0.0, transitionRate = 8.0 }"#);
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_crouch_eye_height_at_capsule_top_is_accepted() {
    // Inclusive upper bound: halfHeight (0.4) + radius (0.4) = 0.8.
    let src =
        lua_movement_with_crouch(r#"{ halfHeight = 0.4, eyeHeight = 0.8, transitionRate = 8.0 }"#);
    let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
    assert_eq!(d.movement.unwrap().crouch.unwrap().eye_height, 0.8);
}

#[test]
fn lua_movement_crouch_eye_height_above_capsule_top_is_rejected() {
    let src =
        lua_movement_with_crouch(r#"{ halfHeight = 0.4, eyeHeight = 0.9, transitionRate = 8.0 }"#);
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_crouch_transition_rate_zero_is_rejected() {
    let src =
        lua_movement_with_crouch(r#"{ halfHeight = 0.4, eyeHeight = 0.3, transitionRate = 0.0 }"#);
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_crouch_transition_rate_negative_is_rejected() {
    let src =
        lua_movement_with_crouch(r#"{ halfHeight = 0.4, eyeHeight = 0.3, transitionRate = -1.0 }"#);
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_crouch_missing_field_reports_missing_field() {
    // eyeHeight omitted.
    let src = lua_movement_with_crouch(r#"{ halfHeight = 0.4, transitionRate = 8.0 }"#);
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert_eq!(err, DescriptorError::MissingField { field: "eyeHeight" });
}

// --- ForgivenessParams parsing -----------------------------------------

/// JS movement block with a `forgiveness` sub-object spliced in. `body` is
/// the inner `{ ... }` text (no `forgiveness:` key).
fn js_movement_with_forgiveness(body: &str) -> String {
    format!(
        r#"({{
            canonicalName: "player",
            components: {{
                movement: {{
                    capsule: {{ radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 }},
                    ground: {{ speed: {{ walk: 7.0, run: 11.0, crouch: 3.0 }}, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 }},
                    air: {{ forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 }},
                    fall: {{ terminalVelocity: 40.0 }},
                    forgiveness: {body}
                }}
            }}
        }})"#
    )
}

/// Luau movement block with a `forgiveness` sub-table spliced in.
fn lua_movement_with_forgiveness(body: &str) -> String {
    format!(
        r#"return {{
            canonicalName = "player",
            components = {{
                movement = {{
                    capsule = {{ radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 }},
                    ground = {{ speed = {{ walk = 7.0, run = 11.0, crouch = 3.0 }}, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 }},
                    air = {{ forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 }},
                    fall = {{ terminalVelocity = 40.0 }},
                    forgiveness = {body}
                }}
            }}
        }}"#
    )
}

#[test]
fn js_movement_forgiveness_absent_is_none() {
    // No `forgiveness` key → None; `from_descriptor` applies engine defaults.
    let d = eval_js(JS_PLAYER_MOVEMENT, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap()
    });
    assert!(d.movement.expect("movement present").forgiveness.is_none());
}

#[test]
fn js_movement_forgiveness_explicit_values_parse() {
    let src = js_movement_with_forgiveness(r#"{ coyoteMs: 120.0, jumpBufferMs: 80.0 }"#);
    let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let f = d
        .movement
        .unwrap()
        .forgiveness
        .expect("forgiveness present");
    assert_eq!(f.coyote_ms, 120.0);
    assert_eq!(f.jump_buffer_ms, 80.0);
}

#[test]
fn js_movement_forgiveness_omitted_fields_default_per_field() {
    // Present object, each field optional with its own engine default.
    let src = js_movement_with_forgiveness(r#"{ coyoteMs: 0.0 }"#);
    let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let f = d
        .movement
        .unwrap()
        .forgiveness
        .expect("forgiveness present");
    assert_eq!(f.coyote_ms, 0.0, "explicit 0 disables coyote");
    assert_eq!(
        f.jump_buffer_ms,
        ForgivenessParams::DEFAULT_JUMP_BUFFER_MS,
        "omitted jumpBufferMs falls back to its engine default"
    );
}

#[test]
fn js_movement_forgiveness_negative_is_rejected() {
    let src = js_movement_with_forgiveness(r#"{ coyoteMs: -5.0, jumpBufferMs: 80.0 }"#);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_forgiveness_explicit_values_parse() {
    let src = lua_movement_with_forgiveness(r#"{ coyoteMs = 120.0, jumpBufferMs = 80.0 }"#);
    let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
    let f = d
        .movement
        .unwrap()
        .forgiveness
        .expect("forgiveness present");
    assert_eq!(f.coyote_ms, 120.0);
    assert_eq!(f.jump_buffer_ms, 80.0);
}

#[test]
fn lua_movement_forgiveness_omitted_fields_default_per_field() {
    let src = lua_movement_with_forgiveness(r#"{ jumpBufferMs = 0.0 }"#);
    let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
    let f = d
        .movement
        .unwrap()
        .forgiveness
        .expect("forgiveness present");
    assert_eq!(f.jump_buffer_ms, 0.0, "explicit 0 disables jump buffer");
    assert_eq!(
        f.coyote_ms,
        ForgivenessParams::DEFAULT_COYOTE_MS,
        "omitted coyoteMs falls back to its engine default"
    );
}

#[test]
fn lua_movement_forgiveness_negative_is_rejected() {
    let src = lua_movement_with_forgiveness(r#"{ coyoteMs = 100.0, jumpBufferMs = -1.0 }"#);
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}
