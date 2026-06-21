// Tests: health component parsing.

use super::super::*;
use super::common::*;

// --- health component (both parsers) ------------------------------------

#[test]
fn js_entity_descriptor_parses_health_with_hitbox() {
    let src = r#"({
        canonicalName: "dummy",
        components: { health: { max: 80, hitbox: { halfExtents: [0.5, 1.0, 0.5], offset: [0, 0.9, 0] } } }
    })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let health = d.health.expect("health parsed");
    assert_eq!(health.max, 80.0);
    let hitbox = health.hitbox.expect("hitbox parsed");
    assert_eq!(hitbox.half_extents, [0.5, 1.0, 0.5]);
    assert_eq!(hitbox.offset, Some([0.0, 0.9, 0.0]));
}

#[test]
fn lua_entity_descriptor_parses_health_without_hitbox() {
    // Luau parity: a missing arm would silently drop `health`. Assert the
    // arm exists and a hitbox-less block parses (offset/hitbox optional).
    let src = r#"return {
        canonicalName = "player",
        components = { health = { max = 100 } }
    }"#;
    let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
    let health = d.health.expect("health parsed by the Luau arm");
    assert_eq!(health.max, 100.0);
    assert!(health.hitbox.is_none());
}

#[test]
fn js_health_max_below_one_is_rejected() {
    let src = r#"({ components: { health: { max: 0.5 } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn lua_health_non_finite_max_is_rejected() {
    let src = r#"return { components = { health = { max = 1/0 } } }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_health_non_positive_hitbox_extent_is_rejected() {
    let src =
        r#"({ components: { health: { max: 50, hitbox: { halfExtents: [0.5, 0.0, 0.5] } } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_health_non_finite_offset_is_rejected() {
    let src = r#"({ components: { health: { max: 50, hitbox: { halfExtents: [0.5, 0.5, 0.5], offset: [0, 1/0, 0] } } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}

#[test]
fn js_health_parses_zone_multipliers() {
    let src =
        r#"({ components: { health: { max: 80, zoneMultipliers: { head: 1.5, leg: 0.5 } } } })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    let health = d.health.expect("health parsed");
    assert_eq!(health.zone_multipliers.get("head"), Some(&1.5));
    assert_eq!(health.zone_multipliers.get("leg"), Some(&0.5));
}

#[test]
fn lua_health_parses_zone_multipliers() {
    // Luau parity: the same shape flows through the Luau arm for free.
    let src =
        r#"return { components = { health = { max = 80, zoneMultipliers = { head = 2.0 } } } }"#;
    let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
    let health = d.health.expect("health parsed by the Luau arm");
    assert_eq!(health.zone_multipliers.get("head"), Some(&2.0));
}

#[test]
fn js_health_negative_zone_multiplier_is_rejected() {
    let src = r#"({ components: { health: { max: 50, zoneMultipliers: { head: -0.1 } } } })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(matches!(err, DescriptorError::InvalidShape { .. }));
}
