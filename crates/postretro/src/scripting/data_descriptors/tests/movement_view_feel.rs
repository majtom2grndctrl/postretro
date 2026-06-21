// Tests: view-feel (bob/tilt/sway) parsing.

use super::super::*;
use super::common::*;

// --- ViewFeelParams parsing --------------------------------------------

/// JS movement block with a `viewFeel` sub-object spliced in. `body` is the
/// inner `{ ... }` text (no `viewFeel:` key).
fn js_movement_with_view_feel(body: &str) -> String {
    format!(
        r#"({{
            canonicalName: "player",
            components: {{
                movement: {{
                    capsule: {{ radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 }},
                    ground: {{ speed: {{ walk: 7.0, run: 11.0, crouch: 3.0 }}, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 }},
                    air: {{ forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 }},
                    fall: {{ terminalVelocity: 40.0 }},
                    viewFeel: {body}
                }}
            }}
        }})"#
    )
}

/// Luau movement block with a `viewFeel` sub-table spliced in.
fn lua_movement_with_view_feel(body: &str) -> String {
    format!(
        r#"return {{
            canonicalName = "player",
            components = {{
                movement = {{
                    capsule = {{ radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 }},
                    ground = {{ speed = {{ walk = 7.0, run = 11.0, crouch = 3.0 }}, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 }},
                    air = {{ forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 }},
                    fall = {{ terminalVelocity = 40.0 }},
                    viewFeel = {body}
                }}
            }}
        }}"#
    )
}

const JS_BOB_FULL: &str = r#"{ verticalFrequency: 1.8, lateralFrequency: 0.9, verticalAmplitude: 0.06, lateralAmplitude: 0.04, speedThreshold: 0.5 }"#;
const JS_TILT_FULL: &str = r#"{ maxAngle: 3.0, speedReference: 8.0, tension: 12.0 }"#;
const JS_SWAY_FULL: &str = r#"{ amplitude: 0.5, frequency: 0.4, speedScale: 0.2 }"#;

// Absent `viewFeel` → no ViewFeelParams materialized.

#[test]
fn js_movement_view_feel_absent_is_valid_and_disabled() {
    let d = eval_js(JS_PLAYER_MOVEMENT, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap()
    });
    assert!(d.movement.expect("movement present").view_feel.is_none());
}

#[test]
fn lua_movement_view_feel_absent_is_valid_and_disabled() {
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
    assert!(d.movement.expect("movement present").view_feel.is_none());
}

// Present `viewFeel` with all three motions absent is valid (empty bundle).

#[test]
fn js_movement_view_feel_present_empty_disables_each_motion() {
    let src = js_movement_with_view_feel("{}");
    let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let vf = d.movement.unwrap().view_feel.expect("viewFeel present");
    assert!(vf.bob.is_none());
    assert!(vf.tilt.is_none());
    assert!(vf.sway.is_none());
}

#[test]
fn lua_movement_view_feel_present_empty_disables_each_motion() {
    let src = lua_movement_with_view_feel("{}");
    let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
    let vf = d.movement.unwrap().view_feel.expect("viewFeel present");
    assert!(vf.bob.is_none());
    assert!(vf.tilt.is_none());
    assert!(vf.sway.is_none());
}

// Full shapes parse and `groundedOnly` defaults apply (bob/tilt true, sway false).

#[test]
fn js_movement_view_feel_full_shape_parses_with_grounded_only_defaults() {
    let body = format!(r#"{{ bob: {JS_BOB_FULL}, tilt: {JS_TILT_FULL}, sway: {JS_SWAY_FULL} }}"#);
    let src = js_movement_with_view_feel(&body);
    let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let vf = d.movement.unwrap().view_feel.expect("viewFeel present");
    let bob = vf.bob.expect("bob present");
    assert_eq!(bob.vertical_frequency, 1.8);
    assert_eq!(bob.lateral_frequency, 0.9);
    assert_eq!(bob.vertical_amplitude, 0.06);
    assert_eq!(bob.lateral_amplitude, 0.04);
    assert_eq!(bob.speed_threshold, 0.5);
    assert!(bob.grounded_only, "bob groundedOnly defaults true");
    let tilt = vf.tilt.expect("tilt present");
    assert_eq!(tilt.max_angle, 3.0);
    assert_eq!(tilt.speed_reference, 8.0);
    assert_eq!(tilt.tension, 12.0);
    assert!(tilt.grounded_only, "tilt groundedOnly defaults true");
    let sway = vf.sway.expect("sway present");
    assert_eq!(sway.amplitude, 0.5);
    assert_eq!(sway.frequency, 0.4);
    assert_eq!(sway.speed_scale, 0.2);
    assert!(!sway.grounded_only, "sway groundedOnly defaults false");
}

#[test]
fn lua_movement_view_feel_full_shape_parses_with_grounded_only_defaults() {
    let src = lua_movement_with_view_feel(
        r#"{
            bob = { verticalFrequency = 1.8, lateralFrequency = 0.9, verticalAmplitude = 0.06, lateralAmplitude = 0.04, speedThreshold = 0.5 },
            tilt = { maxAngle = 3.0, speedReference = 8.0, tension = 12.0 },
            sway = { amplitude = 0.5, frequency = 0.4, speedScale = 0.2 }
        }"#,
    );
    let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
    let vf = d.movement.unwrap().view_feel.expect("viewFeel present");
    let bob = vf.bob.expect("bob present");
    assert_eq!(bob.vertical_frequency, 1.8);
    assert_eq!(bob.lateral_frequency, 0.9);
    assert!(bob.grounded_only, "bob groundedOnly defaults true");
    let tilt = vf.tilt.expect("tilt present");
    assert_eq!(tilt.tension, 12.0);
    assert!(tilt.grounded_only, "tilt groundedOnly defaults true");
    let sway = vf.sway.expect("sway present");
    assert_eq!(sway.frequency, 0.4);
    assert!(!sway.grounded_only, "sway groundedOnly defaults false");
}

// Explicit `groundedOnly` overrides the per-motion default in both paths.

#[test]
fn js_movement_view_feel_grounded_only_explicit_overrides_default() {
    let body = r#"{ bob: { verticalFrequency: 1.8, lateralFrequency: 0.9, verticalAmplitude: 0.06, lateralAmplitude: 0.04, speedThreshold: 0.5, groundedOnly: false }, sway: { amplitude: 0.5, frequency: 0.4, speedScale: 0.2, groundedOnly: true } }"#;
    let src = js_movement_with_view_feel(body);
    let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let vf = d.movement.unwrap().view_feel.unwrap();
    assert!(
        !vf.bob.unwrap().grounded_only,
        "explicit false overrides bob default true"
    );
    assert!(
        vf.sway.unwrap().grounded_only,
        "explicit true overrides sway default false"
    );
}

#[test]
fn lua_movement_view_feel_grounded_only_explicit_overrides_default() {
    let src = lua_movement_with_view_feel(
        r#"{ bob = { verticalFrequency = 1.8, lateralFrequency = 0.9, verticalAmplitude = 0.06, lateralAmplitude = 0.04, speedThreshold = 0.5, groundedOnly = false }, sway = { amplitude = 0.5, frequency = 0.4, speedScale = 0.2, groundedOnly = true } }"#,
    );
    let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
    let vf = d.movement.unwrap().view_feel.unwrap();
    assert!(!vf.bob.unwrap().grounded_only);
    assert!(vf.sway.unwrap().grounded_only);
}

#[test]
fn js_movement_view_feel_grounded_only_non_boolean_is_rejected() {
    let body = r#"{ bob: { verticalFrequency: 1.8, lateralFrequency: 0.9, verticalAmplitude: 0.06, lateralAmplitude: 0.04, speedThreshold: 0.5, groundedOnly: "yes" } }"#;
    let src = js_movement_with_view_feel(body);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_view_feel_grounded_only_non_boolean_is_rejected() {
    let src = lua_movement_with_view_feel(
        r#"{ bob = { verticalFrequency = 1.8, lateralFrequency = 0.9, verticalAmplitude = 0.06, lateralAmplitude = 0.04, speedThreshold = 0.5, groundedOnly = "yes" } }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

// Present-then-all-required: a missing required field is rejected in both paths.

#[test]
fn js_movement_view_feel_bob_missing_field_reports_missing_field() {
    // verticalFrequency omitted from bob.
    let body = r#"{ bob: { lateralFrequency: 0.9, verticalAmplitude: 0.06, lateralAmplitude: 0.04, speedThreshold: 0.5 } }"#;
    let src = js_movement_with_view_feel(body);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert_eq!(
        err,
        DescriptorError::MissingField {
            field: "verticalFrequency"
        }
    );
}

#[test]
fn lua_movement_view_feel_tilt_missing_field_reports_missing_field() {
    // tension omitted from tilt.
    let src = lua_movement_with_view_feel(r#"{ tilt = { maxAngle = 3.0, speedReference = 8.0 } }"#);
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert_eq!(err, DescriptorError::MissingField { field: "tension" });
}

#[test]
fn js_movement_view_feel_sway_missing_field_reports_missing_field() {
    // speedScale omitted from sway.
    let body = r#"{ sway: { amplitude: 0.5, frequency: 0.4 } }"#;
    let src = js_movement_with_view_feel(body);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert_eq!(
        err,
        DescriptorError::MissingField {
            field: "speedScale"
        }
    );
}

// Value validation: positive-finite, non-negative-finite, and the
// tilt.maxAngle [0,90] range, symmetric across JS and Luau.

#[test]
fn js_movement_view_feel_bob_vertical_frequency_zero_is_rejected() {
    let body = r#"{ bob: { verticalFrequency: 0.0, lateralFrequency: 0.9, verticalAmplitude: 0.06, lateralAmplitude: 0.04, speedThreshold: 0.5 } }"#;
    let src = js_movement_with_view_feel(body);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_view_feel_bob_negative_amplitude_is_rejected() {
    let body = r#"{ bob: { verticalFrequency: 1.8, lateralFrequency: 0.9, verticalAmplitude: -0.01, lateralAmplitude: 0.04, speedThreshold: 0.5 } }"#;
    let src = js_movement_with_view_feel(body);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_view_feel_bob_zero_amplitude_is_accepted() {
    // Amplitudes and speedThreshold permit 0 (non-negative finite).
    let body = r#"{ bob: { verticalFrequency: 1.8, lateralFrequency: 0.9, verticalAmplitude: 0.0, lateralAmplitude: 0.0, speedThreshold: 0.0 } }"#;
    let src = js_movement_with_view_feel(body);
    let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let bob = d.movement.unwrap().view_feel.unwrap().bob.unwrap();
    assert_eq!(bob.vertical_amplitude, 0.0);
    assert_eq!(bob.speed_threshold, 0.0);
}

#[test]
fn js_movement_view_feel_tilt_max_angle_above_90_is_rejected() {
    let body = r#"{ tilt: { maxAngle: 95.0, speedReference: 8.0, tension: 12.0 } }"#;
    let src = js_movement_with_view_feel(body);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_view_feel_tilt_max_angle_negative_is_rejected() {
    let body = r#"{ tilt: { maxAngle: -1.0, speedReference: 8.0, tension: 12.0 } }"#;
    let src = js_movement_with_view_feel(body);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_view_feel_tilt_tension_zero_is_rejected() {
    let body = r#"{ tilt: { maxAngle: 3.0, speedReference: 8.0, tension: 0.0 } }"#;
    let src = js_movement_with_view_feel(body);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_view_feel_sway_frequency_zero_is_rejected() {
    let body = r#"{ sway: { amplitude: 0.5, frequency: 0.0, speedScale: 0.2 } }"#;
    let src = js_movement_with_view_feel(body);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_view_feel_tilt_max_angle_above_90_is_rejected() {
    let src = lua_movement_with_view_feel(
        r#"{ tilt = { maxAngle = 95.0, speedReference = 8.0, tension = 12.0 } }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_view_feel_tilt_speed_reference_zero_is_rejected() {
    let src = lua_movement_with_view_feel(
        r#"{ tilt = { maxAngle = 3.0, speedReference = 0.0, tension = 12.0 } }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_view_feel_sway_negative_amplitude_is_rejected() {
    let src = lua_movement_with_view_feel(
        r#"{ sway = { amplitude = -0.1, frequency = 0.4, speedScale = 0.2 } }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_view_feel_sway_frequency_zero_is_rejected() {
    let src = lua_movement_with_view_feel(
        r#"{ sway = { amplitude = 0.5, frequency = 0.0, speedScale = 0.2 } }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_view_feel_bob_missing_field_reports_missing_field() {
    // lateralFrequency omitted from bob.
    let src = lua_movement_with_view_feel(
        r#"{ bob = { verticalFrequency = 1.8, verticalAmplitude = 0.06, lateralAmplitude = 0.04, speedThreshold = 0.5 } }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert_eq!(
        err,
        DescriptorError::MissingField {
            field: "lateralFrequency",
        }
    );
}

#[test]
fn lua_movement_view_feel_bob_lateral_frequency_zero_is_rejected() {
    let src = lua_movement_with_view_feel(
        r#"{ bob = { verticalFrequency = 1.8, lateralFrequency = 0.0, verticalAmplitude = 0.06, lateralAmplitude = 0.04, speedThreshold = 0.5 } }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

// Wrong-typed sub-object is rejected (a present sub-object must be an object/table).

#[test]
fn js_movement_view_feel_bob_not_an_object_is_rejected() {
    let body = r#"{ bob: 3 }"#;
    let src = js_movement_with_view_feel(body);
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_view_feel_sway_not_a_table_is_rejected() {
    let src = lua_movement_with_view_feel(r#"{ sway = 3 }"#);
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}
