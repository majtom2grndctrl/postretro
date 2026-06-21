// Tests: entity-descriptor component parsing.

use super::super::*;
use super::common::*;

#[test]
fn entity_descriptor_with_emitter_only_deserializes() {
    let src = r#"({
        canonicalName: "smoke_pillar",
        components: {
            emitter: {
                rate: 12.0,
                burst: null,
                spread: 0.3,
                lifetime: 4.0,
                velocity: [0, 1, 0],
                buoyancy: 0.5,
                drag: 0.5,
                size_over_lifetime: [0.5, 1.0],
                opacity_over_lifetime: [0.0, 1.0, 0.0],
                color: [0.7, 0.7, 0.7],
                sprite: "smoke",
                spin_rate: 0.0
            }
        }
    })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    assert_eq!(d.canonical_name.as_deref(), Some("smoke_pillar"));
    assert!(d.light.is_none());
    let e = d.emitter.expect("emitter present");
    assert_eq!(e.rate, 12.0);
    assert_eq!(e.sprite, "smoke");
}

#[test]
fn entity_descriptor_with_light_only_deserializes() {
    let src = r#"({
        canonicalName: "campfire",
        components: {
            light: {
                color: [1.0, 0.6, 0.2],
                intensity: 4.0,
                range: 10.0,
                is_dynamic: false
            }
        }
    })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    assert_eq!(d.canonical_name.as_deref(), Some("campfire"));
    assert!(d.emitter.is_none());
    let l = d.light.expect("light present");
    assert_eq!(l.color, [1.0, 0.6, 0.2]);
    assert_eq!(l.intensity, 4.0);
    assert_eq!(l.range, 10.0);
    assert!(!l.is_dynamic);
}

#[test]
fn entity_descriptor_with_both_components_deserializes() {
    let src = r#"({
        canonicalName: "torch",
        components: {
            light: { color: [1, 1, 1], intensity: 2.0, range: 6.0, is_dynamic: true },
            emitter: {
                rate: 4.0, burst: null, spread: 0.1, lifetime: 1.5,
                velocity: [0, 1, 0], buoyancy: 0.3, drag: 0.4,
                size_over_lifetime: [1.0], opacity_over_lifetime: [1.0, 0.0],
                color: [1, 1, 1], sprite: "ember", spin_rate: 0.0
            }
        }
    })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    assert_eq!(d.canonical_name.as_deref(), Some("torch"));
    assert!(d.light.is_some());
    assert!(d.emitter.is_some());
}

#[test]
fn entity_descriptor_without_components_field_deserializes() {
    let src = r#"({ canonicalName: "vignette" })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    assert_eq!(d.canonical_name.as_deref(), Some("vignette"));
    assert!(d.default_weapon.is_none());
    assert!(d.light.is_none());
    assert!(d.emitter.is_none());
    assert!(d.weapon.is_none());
}

#[test]
fn js_entity_descriptor_with_default_weapon_and_weapon_component_deserializes() {
    let src = r#"({
        canonicalName: "player",
        defaultWeapon: "reference_pistol",
        components: {
            weapon: {
                damage: 12.0,
                range: 64.0,
                fireRateMs: 180.0,
                fireMode: "semi",
                resolution: "hitscan"
            }
        }
    })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    assert_eq!(d.default_weapon.as_deref(), Some("reference_pistol"));
    let weapon = d.weapon.expect("weapon present");
    assert_eq!(weapon.damage, 12.0);
    assert_eq!(weapon.range, 64.0);
    assert_eq!(weapon.cooldown_ms, 180.0);
    assert_eq!(weapon.fire_mode, FireMode::Semi);
    assert_eq!(weapon.resolution, ResolutionMode::Hitscan);
}

#[test]
fn js_top_level_weapon_key_is_not_a_component_alias() {
    let src = r#"({
        canonicalName: "player",
        weapon: {
            damage: 12.0,
            range: 64.0,
            fireRateMs: 180.0,
            fireMode: "semi",
            resolution: "hitscan"
        }
    })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    assert!(d.weapon.is_none());
}

#[test]
fn entity_descriptor_with_emitter_only_deserializes_lua() {
    let src = r#"return {
        canonicalName = "smoke_pillar",
        components = {
            emitter = {
                rate = 12.0,
                spread = 0.3,
                lifetime = 4.0,
                velocity = { 0, 1, 0 },
                buoyancy = 0.5,
                drag = 0.5,
                size_over_lifetime = { 0.5, 1.0 },
                opacity_over_lifetime = { 0.0, 1.0, 0.0 },
                color = { 0.7, 0.7, 0.7 },
                sprite = "smoke",
                spin_rate = 0.0,
            }
        }
    }"#;
    let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
    assert_eq!(d.canonical_name.as_deref(), Some("smoke_pillar"));
    assert!(d.emitter.is_some());
}

#[test]
fn entity_descriptor_with_light_only_deserializes_lua() {
    let src = r#"return {
        canonicalName = "campfire",
        components = {
            light = { color = { 1.0, 0.6, 0.2 }, intensity = 4.0, range = 10.0, is_dynamic = false }
        }
    }"#;
    let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
    assert_eq!(d.canonical_name.as_deref(), Some("campfire"));
    let l = d.light.expect("light present");
    assert_eq!(l.intensity, 4.0);
}

#[test]
fn lua_entity_descriptor_with_default_weapon_and_weapon_component_deserializes() {
    let src = r#"return {
        canonicalName = "player",
        defaultWeapon = "reference_pistol",
        components = {
            weapon = {
                damage = 12.0,
                range = 64.0,
                fireRateMs = 180.0,
                fireMode = "auto",
                resolution = "hitscan",
            }
        }
    }"#;
    let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
    assert_eq!(d.default_weapon.as_deref(), Some("reference_pistol"));
    let weapon = d.weapon.expect("weapon present");
    assert_eq!(weapon.damage, 12.0);
    assert_eq!(weapon.cooldown_ms, 180.0);
    assert_eq!(weapon.fire_mode, FireMode::Auto);
    assert_eq!(weapon.resolution, ResolutionMode::Hitscan);
}
