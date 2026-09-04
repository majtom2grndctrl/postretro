// Tests: slide params parsing.

use super::super::*;
use super::common::*;

/// JS movement block with a `slide` sub-object spliced into the `movement`
/// object. `slide_body` is the inner `{ ... }` text (no `slide:` key).
fn js_movement_with_slide(slide_body: &str) -> String {
    format!(
        r#"({{
            canonicalName: "player",
            components: {{
                movement: {{
                    capsule: {{ radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 }},
                    ground: {{ speed: {{ walk: 7.0, run: 11.0, crouch: 3.0 }}, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 }},
                    air: {{ forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 }},
                    fall: {{ terminalVelocity: 40.0 }},
                    slide: {slide_body}
                }}
            }}
        }})"#
    )
}

/// Luau movement block with a `slide` sub-table spliced into the `movement`
/// table. `slide_body` is the inner `{ ... }` text (no `slide =` key).
fn lua_movement_with_slide(slide_body: &str) -> String {
    format!(
        r#"return {{
            canonicalName = "player",
            components = {{
                movement = {{
                    capsule = {{ radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 }},
                    ground = {{ speed = {{ walk = 7.0, run = 11.0, crouch = 3.0 }}, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 }},
                    air = {{ forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 }},
                    fall = {{ terminalVelocity = 40.0 }},
                    slide = {slide_body}
                }}
            }}
        }}"#
    )
}

const JS_SLIDE_FULL: &str = r#"{ minSpeed: 8.0, slideDrag: 12.0, slopeAssist: 1.5, steerRate: 180.0, entryBoost: 2.0, minDurationMs: 120.0 }"#;
const LUA_SLIDE_FULL: &str = r#"{ minSpeed = 8.0, slideDrag = 12.0, slopeAssist = 1.5, steerRate = 180.0, entryBoost = 2.0, minDurationMs = 120.0 }"#;

#[test]
fn js_movement_slide_absent_is_valid_and_disabled() {
    let d = eval_js(JS_PLAYER_MOVEMENT, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap()
    });
    assert!(d.movement.expect("movement present").slide.is_none());
}

#[test]
fn lua_movement_slide_absent_is_valid_and_disabled() {
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
    assert!(d.movement.expect("movement present").slide.is_none());
}

#[test]
fn js_movement_slide_full_shape_parses() {
    let src = js_movement_with_slide(JS_SLIDE_FULL);
    let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let slide = d.movement.unwrap().slide.expect("slide present");
    assert_eq!(slide.min_speed, 8.0);
    assert_eq!(slide.slide_drag, 12.0);
    assert_eq!(slide.slope_assist, 1.5);
    assert_eq!(slide.steer_rate, 180.0);
    assert_eq!(slide.entry_boost, 2.0);
    assert_eq!(slide.min_duration_ms, 120.0);
}

#[test]
fn lua_movement_slide_full_shape_parses() {
    let src = lua_movement_with_slide(LUA_SLIDE_FULL);
    let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
    let slide = d.movement.unwrap().slide.expect("slide present");
    assert_eq!(slide.min_speed, 8.0);
    assert_eq!(slide.slide_drag, 12.0);
    assert_eq!(slide.slope_assist, 1.5);
    assert_eq!(slide.steer_rate, 180.0);
    assert_eq!(slide.entry_boost, 2.0);
    assert_eq!(slide.min_duration_ms, 120.0);
}

#[test]
fn js_movement_slide_accepts_zero_optional_effect_rates() {
    let src = js_movement_with_slide(
        r#"{ minSpeed: 8.0, slideDrag: 0.0, slopeAssist: 0.0, steerRate: 0.0, entryBoost: 2.0, minDurationMs: 120.0 }"#,
    );
    let d = eval_js(&src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let slide = d.movement.unwrap().slide.expect("slide present");
    assert_eq!(slide.slide_drag, 0.0);
    assert_eq!(slide.slope_assist, 0.0);
    assert_eq!(slide.steer_rate, 0.0);
}

#[test]
fn lua_movement_slide_accepts_zero_optional_effect_rates() {
    let src = lua_movement_with_slide(
        r#"{ minSpeed = 8.0, slideDrag = 0.0, slopeAssist = 0.0, steerRate = 0.0, entryBoost = 2.0, minDurationMs = 120.0 }"#,
    );
    let d = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap());
    let slide = d.movement.unwrap().slide.expect("slide present");
    assert_eq!(slide.slide_drag, 0.0);
    assert_eq!(slide.slope_assist, 0.0);
    assert_eq!(slide.steer_rate, 0.0);
}

#[test]
fn js_movement_slide_rejects_invalid_field_ranges() {
    for slide in [
        r#"{ minSpeed: 0.0, slideDrag: 12.0, slopeAssist: 1.5, steerRate: 180.0, entryBoost: 2.0, minDurationMs: 120.0 }"#,
        r#"{ minSpeed: 8.0, slideDrag: -1.0, slopeAssist: 1.5, steerRate: 180.0, entryBoost: 2.0, minDurationMs: 120.0 }"#,
        r#"{ minSpeed: 8.0, slideDrag: 12.0, slopeAssist: -1.0, steerRate: 180.0, entryBoost: 2.0, minDurationMs: 120.0 }"#,
        r#"{ minSpeed: 8.0, slideDrag: 12.0, slopeAssist: 1.5, steerRate: -1.0, entryBoost: 2.0, minDurationMs: 120.0 }"#,
        r#"{ minSpeed: 8.0, slideDrag: 12.0, slopeAssist: 1.5, steerRate: 180.0, entryBoost: -1.0, minDurationMs: 120.0 }"#,
        r#"{ minSpeed: 8.0, slideDrag: 12.0, slopeAssist: 1.5, steerRate: 180.0, entryBoost: 2.0, minDurationMs: -1.0 }"#,
    ] {
        let src = js_movement_with_slide(slide);
        let err = eval_js(&src, |ctx, v| {
            entity_descriptor_from_js(ctx, v).unwrap_err()
        });
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }
}

#[test]
fn lua_movement_slide_rejects_invalid_field_ranges() {
    for slide in [
        r#"{ minSpeed = 0.0, slideDrag = 12.0, slopeAssist = 1.5, steerRate = 180.0, entryBoost = 2.0, minDurationMs = 120.0 }"#,
        r#"{ minSpeed = 8.0, slideDrag = -1.0, slopeAssist = 1.5, steerRate = 180.0, entryBoost = 2.0, minDurationMs = 120.0 }"#,
        r#"{ minSpeed = 8.0, slideDrag = 12.0, slopeAssist = -1.0, steerRate = 180.0, entryBoost = 2.0, minDurationMs = 120.0 }"#,
        r#"{ minSpeed = 8.0, slideDrag = 12.0, slopeAssist = 1.5, steerRate = -1.0, entryBoost = 2.0, minDurationMs = 120.0 }"#,
        r#"{ minSpeed = 8.0, slideDrag = 12.0, slopeAssist = 1.5, steerRate = 180.0, entryBoost = -1.0, minDurationMs = 120.0 }"#,
        r#"{ minSpeed = 8.0, slideDrag = 12.0, slopeAssist = 1.5, steerRate = 180.0, entryBoost = 2.0, minDurationMs = -1.0 }"#,
    ] {
        let src = lua_movement_with_slide(slide);
        let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
        assert!(matches!(err, DescriptorError::InvalidShape { .. }));
    }
}

#[test]
fn js_movement_slide_missing_field_reports_missing_field() {
    let src = js_movement_with_slide(
        r#"{ minSpeed: 8.0, slideDrag: 12.0, slopeAssist: 1.5, steerRate: 180.0, minDurationMs: 120.0 }"#,
    );
    let err = eval_js(&src, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap_err()
    });
    assert_eq!(
        err,
        DescriptorError::MissingField {
            field: "entryBoost"
        }
    );
}

#[test]
fn lua_movement_slide_missing_field_reports_missing_field() {
    let src = lua_movement_with_slide(
        r#"{ minSpeed = 8.0, slideDrag = 12.0, slopeAssist = 1.5, steerRate = 180.0, entryBoost = 2.0 }"#,
    );
    let err = eval_lua(&src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert_eq!(
        err,
        DescriptorError::MissingField {
            field: "minDurationMs"
        }
    );
}
