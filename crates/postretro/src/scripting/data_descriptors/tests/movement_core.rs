// Tests: movement core + stuck-stop parsing.

use super::super::*;
use super::common::*;

#[test]
fn js_movement_descriptor_full_shape_parses() {
    let d = eval_js(JS_PLAYER_MOVEMENT, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap()
    });
    let m = d.movement.expect("movement present");
    assert_eq!(m.capsule.radius, 0.4);
    assert_eq!(m.capsule.half_height, 0.8);
    assert_eq!(m.capsule.eye_height, 0.5);
    assert_eq!(m.ground.speed.walk, 7.0);
    assert_eq!(m.ground.speed.run, 11.0);
    assert_eq!(m.ground.speed.crouch, 3.0);
    assert_eq!(m.ground.max_slope, 45.0);
    assert_eq!(m.air.forward_steer, 0.0);
    assert!(!m.air.bunny_hop);
    assert_eq!(m.air.jumps, 0);
    assert_eq!(m.fall.terminal_velocity, 40.0);
}

#[test]
fn js_movement_speed_missing_run_reports_missing_field() {
    // `ground.speed` is a nested { walk, run } object; both required.
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                ground: { speed: { walk: 7.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 }
            }
        }
    })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert_eq!(err, DescriptorError::MissingField { field: "run" });
}

#[test]
fn js_movement_speed_negative_run_is_rejected() {
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                ground: { speed: { walk: 7.0, run: -1.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 }
            }
        }
    })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_speed_missing_run_reports_missing_field() {
    let src = r#"return {
        canonicalName = "player",
        components = {
            movement = {
                capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                ground = { speed = { walk = 7.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                fall = { terminalVelocity = 40.0 }
            }
        }
    }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert_eq!(err, DescriptorError::MissingField { field: "run" });
}

#[test]
fn js_movement_speed_missing_crouch_reports_missing_field() {
    // `ground.speed.crouch` is required when `ground.speed` is present.
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                ground: { speed: { walk: 7.0, run: 11.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 }
            }
        }
    })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert_eq!(err, DescriptorError::MissingField { field: "crouch" });
}

#[test]
fn lua_movement_speed_missing_crouch_reports_missing_field() {
    let src = r#"return {
        canonicalName = "player",
        components = {
            movement = {
                capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                ground = { speed = { walk = 7.0, run = 11.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                fall = { terminalVelocity = 40.0 }
            }
        }
    }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert_eq!(err, DescriptorError::MissingField { field: "crouch" });
}

#[test]
fn js_movement_speed_negative_crouch_is_rejected() {
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: -1.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 }
            }
        }
    })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_speed_negative_crouch_is_rejected() {
    let src = r#"return {
        canonicalName = "player",
        components = {
            movement = {
                capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                ground = { speed = { walk = 7.0, run = 11.0, crouch = -1.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                fall = { terminalVelocity = 40.0 }
            }
        }
    }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_speed_zero_crouch_is_accepted() {
    // Zero crouch speed is legitimate (non-negative finite).
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: 0.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 }
            }
        }
    })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    assert_eq!(d.movement.unwrap().ground.speed.crouch, 0.0);
}

#[test]
fn js_movement_missing_air_field_reports_missing_field() {
    // `bunnyHop` removed from `air`.
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 }
            }
        }
    })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert_eq!(err, DescriptorError::MissingField { field: "bunnyHop" });
}

#[test]
fn js_movement_jumps_positive_without_ceiling_errors() {
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 2, jumpVelocity: 5.5 },
                fall: { terminalVelocity: 40.0 }
            }
        }
    })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert_eq!(
        err,
        DescriptorError::MissingField {
            field: "jumpCeiling",
        }
    );
}

#[test]
fn js_movement_capsule_radius_zero_is_rejected() {
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.0, halfHeight: 0.8 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 }
            }
        }
    })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_max_slope_out_of_range_is_rejected() {
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 95.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 }
            }
        }
    })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_forward_steer_out_of_range_is_rejected() {
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 1.5, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 }
            }
        }
    })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_terminal_velocity_zero_is_rejected() {
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 0.0 }
            }
        }
    })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_eye_height_zero_is_rejected() {
    // eye_height must be strictly positive: 0.0 sits at the capsule center,
    // not a sensible eye position.
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.0 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 }
            }
        }
    })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_eye_height_above_capsule_top_is_rejected() {
    // capsule top = half_height + radius = 1.2; eye_height = 1.5 exceeds it.
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 1.5 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 }
            }
        }
    })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_eye_height_at_capsule_top_is_accepted() {
    // Exactly at capsule top (half_height + radius = 1.2) is permitted.
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 1.2 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 }
            }
        }
    })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    assert_eq!(d.movement.unwrap().capsule.eye_height, 1.2);
}

#[test]
fn js_movement_missing_eye_height_reports_missing_field() {
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 }
            }
        }
    })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert_eq!(err, DescriptorError::MissingField { field: "eyeHeight" });
}

#[test]
fn lua_movement_descriptor_full_shape_parses() {
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
    let m = d.movement.expect("movement present");
    assert_eq!(m.capsule.eye_height, 0.5);
    assert_eq!(m.air.jump_velocity, 5.5);
    assert_eq!(m.air.jumps, 0);
    assert_eq!(m.fall.terminal_velocity, 40.0);
}

#[test]
fn lua_movement_eye_height_above_capsule_top_is_rejected() {
    let src = r#"return {
        canonicalName = "player",
        components = {
            movement = {
                capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 1.5 },
                ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                fall = { terminalVelocity = 40.0 }
            }
        }
    }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_jumps_positive_without_ceiling_errors() {
    let src = r#"return {
        canonicalName = "player",
        components = {
            movement = {
                capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 2, jumpVelocity = 5.5 },
                fall = { terminalVelocity = 40.0 }
            }
        }
    }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert_eq!(
        err,
        DescriptorError::MissingField {
            field: "jumpCeiling",
        }
    );
}

#[test]
fn lua_movement_missing_capsule_block_reports_missing_field() {
    let src = r#"return {
        canonicalName = "player",
        components = {
            movement = {
                ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                fall = { terminalVelocity = 40.0 }
            }
        }
    }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert_eq!(err, DescriptorError::MissingField { field: "capsule" });
}

#[test]
fn js_movement_jumps_zero_without_ceiling_is_valid() {
    // `jumpCeiling` is only meaningful when `air.jumps > 0`; omitting it
    // when jumps == 0 should succeed with jump_ceiling defaulting to 0.0.
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5 },
                fall: { terminalVelocity: 40.0 }
            }
        }
    })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let m = d.movement.expect("movement present");
    assert_eq!(m.air.jumps, 0);
    assert_eq!(m.air.jump_ceiling, 0.0);
}

#[test]
fn lua_movement_jumps_zero_without_ceiling_is_valid() {
    // `jumpCeiling` is only meaningful when `air.jumps > 0`; omitting it
    // when jumps == 0 should succeed with jump_ceiling defaulting to 0.0.
    let src = r#"return {
        canonicalName = "player",
        components = {
            movement = {
                capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5 },
                fall = { terminalVelocity = 40.0 }
            }
        }
    }"#;
    let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
    let m = d.movement.expect("movement present");
    assert_eq!(m.air.jumps, 0);
    assert_eq!(m.air.jump_ceiling, 0.0);
}

// --- stuck_stop_* parsing ----------------------------------------------

#[test]
fn js_movement_stuck_stop_defaults_when_omitted() {
    // The deadzone fields are optional on the wire; omitting them yields
    // the canonical defaults (enabled, 1e-3 threshold).
    let d = eval_js(JS_PLAYER_MOVEMENT, |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap()
    });
    let m = d.movement.expect("movement present");
    assert!(m.stuck_stop_enabled);
    assert_eq!(m.stuck_stop_threshold, 1.0e-3);
}

#[test]
fn js_movement_stuck_stop_explicit_values_parse() {
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 },
                stuckStopEnabled: false,
                stuckStopThreshold: 0.005
            }
        }
    })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let m = d.movement.expect("movement present");
    assert!(!m.stuck_stop_enabled);
    assert_eq!(m.stuck_stop_threshold, 0.005);
}

#[test]
fn js_movement_stuck_stop_threshold_negative_is_rejected() {
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 },
                stuckStopThreshold: -0.1
            }
        }
    })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_movement_stuck_stop_enabled_wrong_type_is_rejected() {
    let src = r#"({
        canonicalName: "player",
        components: {
            movement: {
                capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
                ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
                air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
                fall: { terminalVelocity: 40.0 },
                stuckStopEnabled: "yes"
            }
        }
    })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_movement_stuck_stop_defaults_when_omitted() {
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
    let m = d.movement.expect("movement present");
    assert!(m.stuck_stop_enabled);
    assert_eq!(m.stuck_stop_threshold, 1.0e-3);
}

#[test]
fn lua_movement_stuck_stop_explicit_values_parse() {
    let src = r#"return {
        canonicalName = "player",
        components = {
            movement = {
                capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                fall = { terminalVelocity = 40.0 },
                stuckStopEnabled = false,
                stuckStopThreshold = 0.005
            }
        }
    }"#;
    let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
    let m = d.movement.expect("movement present");
    assert!(!m.stuck_stop_enabled);
    assert_eq!(m.stuck_stop_threshold, 0.005);
}

#[test]
fn lua_movement_stuck_stop_threshold_negative_is_rejected() {
    let src = r#"return {
        canonicalName = "player",
        components = {
            movement = {
                capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
                ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
                air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
                fall = { terminalVelocity = 40.0 },
                stuckStopThreshold = -0.1
            }
        }
    }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}
