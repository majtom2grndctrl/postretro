// Tests: entity-descriptor component parsing.

use super::super::*;
use super::common::*;

fn parse_js_ammo_resource(resource: &str) -> Result<EntityTypeDescriptor, DescriptorError> {
    let src = format!(
        r#"({{ components: {{ weapon: {{ damage: 12, range: 64, fireRateMs: 180, fireMode: "semi", resolution: "hitscan", resource: {resource} }} }} }})"#
    );
    eval_js(&src, entity_descriptor_from_js)
}

fn parse_lua_ammo_resource(resource: &str) -> Result<EntityTypeDescriptor, DescriptorError> {
    let src = format!(
        r#"return {{ components = {{ weapon = {{ damage = 12, range = 64, fireRateMs = 180, fireMode = "semi", resolution = "hitscan", resource = {resource} }} }} }}"#
    );
    eval_lua(&src, entity_descriptor_from_lua)
}

fn ammo_resource(descriptor: EntityTypeDescriptor) -> AmmoResource {
    let Some(WeaponResource::Ammo(ammo)) = descriptor.weapon.unwrap().resource else {
        panic!("expected ammo resource");
    };
    ammo
}

fn assert_ammo_pair_rejects(
    label: &str,
    js_resource: &str,
    lua_resource: &str,
    expected_error: &str,
) {
    let js_error = parse_js_ammo_resource(js_resource).unwrap_err();
    let lua_error = parse_lua_ammo_resource(lua_resource).unwrap_err();
    let js_error = js_error.to_string();
    let lua_error = lua_error.to_string();
    assert!(
        js_error.contains(expected_error),
        "QuickJS {label} error should contain {expected_error:?}, got: {js_error}"
    );
    assert!(
        lua_error.contains(expected_error),
        "Luau {label} error should contain {expected_error:?}, got: {lua_error}"
    );
}

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
                resolution: "hitscan",
                creditSource: "player.reference-pistol:primary"
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
    assert_eq!(
        weapon.credit_source.as_deref(),
        Some("player.reference-pistol:primary")
    );
}

#[test]
fn js_weapon_descriptor_without_credit_source_parses_as_none() {
    let src = r#"({
        canonicalName: "reference_pistol",
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
    let weapon = d.weapon.expect("weapon present");
    assert_eq!(weapon.credit_source, None);
    assert_eq!(weapon.resource, None);
}

#[test]
fn weapon_model_paths_have_js_luau_parity() {
    let js = eval_js(
        r#"({ components: { weapon: {
            damage: 12, range: 64, fireRateMs: 180, fireMode: "semi", resolution: "hitscan",
            thirdPersonModel: "models/smg/model.gltf", viewmodel: "models/smg/model.gltf"
        } } })"#,
        |ctx, v| entity_descriptor_from_js(ctx, v).unwrap(),
    );
    let lua = eval_lua(
        r#"return { components = { weapon = {
            damage = 12, range = 64, fireRateMs = 180, fireMode = "semi", resolution = "hitscan",
            thirdPersonModel = "models/smg/model.gltf", viewmodel = "models/smg/model.gltf"
        } } }"#,
        |v| entity_descriptor_from_lua(v).unwrap(),
    );
    let js = js.weapon.unwrap();
    let lua = lua.weapon.unwrap();
    assert_eq!(js.third_person_model, lua.third_person_model);
    assert_eq!(js.viewmodel, lua.viewmodel);
    assert_eq!(
        js.third_person_model.as_deref(),
        Some("models/smg/model.gltf")
    );
}

#[test]
fn paired_weapon_ammo_resource_defaults_match() {
    let js = ammo_resource(
        parse_js_ammo_resource(
            r#"{ kind: "ammo", type: "bullets.light", magazine: 12, costPerShot: undefined, reserve: 48, reloadMs: undefined }"#,
        )
        .unwrap(),
    );
    let lua = ammo_resource(
        parse_lua_ammo_resource(
            r#"{ kind = "ammo", type = "bullets.light", magazine = 12, reserve = 48 }"#,
        )
        .unwrap(),
    );
    assert_eq!(js, lua);
    assert_eq!(js.ammo_type, "bullets.light");
    assert_eq!(js.magazine, 12);
    assert_eq!(js.cost_per_shot, 1);
    assert_eq!(js.reserve, 48);
    assert_eq!(js.reload_ms, 1000);
}

#[test]
fn paired_weapon_ammo_resource_accepts_identifier_and_u32_boundaries() {
    let type_64 = "a".repeat(64);
    let js = ammo_resource(
        parse_js_ammo_resource(&format!(
            r#"{{ kind: "ammo", type: "{type_64}", magazine: 4294967295, costPerShot: 4294967295, reserve: 4294967295, reloadMs: 4294967295 }}"#
        ))
        .unwrap(),
    );
    let lua = ammo_resource(
        parse_lua_ammo_resource(&format!(
            r#"{{ kind = "ammo", type = "{type_64}", magazine = 4294967295, costPerShot = 4294967295, reserve = 4294967295, reloadMs = 4294967295 }}"#
        ))
        .unwrap(),
    );
    assert_eq!(js, lua);
    assert_eq!(js.ammo_type.len(), 64);
    assert_eq!(js.magazine, u32::MAX);
    assert_eq!(js.cost_per_shot, u32::MAX);
    assert_eq!(js.reserve, u32::MAX);
    assert_eq!(js.reload_ms, u32::MAX);
}

#[test]
fn paired_weapon_ammo_resource_rejects_wrong_type_and_negative_u32_fields() {
    for (field, js_resource, lua_resource) in [
        (
            "magazine wrong type",
            r#"{ kind: "ammo", type: "cells", magazine: "8", costPerShot: 1, reserve: 32, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = "8", costPerShot = 1, reserve = 32, reloadMs = 1000 }"#,
        ),
        (
            "costPerShot wrong type",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: "1", reserve: 32, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = "1", reserve = 32, reloadMs = 1000 }"#,
        ),
        (
            "reserve wrong type",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: 1, reserve: "32", reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = 1, reserve = "32", reloadMs = 1000 }"#,
        ),
        (
            "reloadMs wrong type",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: 1, reserve: 32, reloadMs: "1000" }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = 1, reserve = 32, reloadMs = "1000" }"#,
        ),
        (
            "magazine negative",
            r#"{ kind: "ammo", type: "cells", magazine: -1, costPerShot: 1, reserve: 32, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = -1, costPerShot = 1, reserve = 32, reloadMs = 1000 }"#,
        ),
        (
            "costPerShot negative",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: -1, reserve: 32, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = -1, reserve = 32, reloadMs = 1000 }"#,
        ),
        (
            "reserve negative",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: 1, reserve: -1, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = 1, reserve = -1, reloadMs = 1000 }"#,
        ),
        (
            "reloadMs negative",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: 1, reserve: 32, reloadMs: -1 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = 1, reserve = 32, reloadMs = -1 }"#,
        ),
    ] {
        assert_ammo_pair_rejects(field, js_resource, lua_resource, "expected u32");
    }
}

#[test]
fn paired_weapon_ammo_resource_rejects_u32_overflow() {
    for (field, js_resource, lua_resource) in [
        (
            "magazine",
            r#"{ kind: "ammo", type: "cells", magazine: 4294967296, costPerShot: 1, reserve: 32, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 4294967296, costPerShot = 1, reserve = 32, reloadMs = 1000 }"#,
        ),
        (
            "costPerShot",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: 4294967296, reserve: 32, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = 4294967296, reserve = 32, reloadMs = 1000 }"#,
        ),
        (
            "reserve",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: 1, reserve: 4294967296, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = 1, reserve = 4294967296, reloadMs = 1000 }"#,
        ),
        (
            "reloadMs",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: 1, reserve: 32, reloadMs: 4294967296 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = 1, reserve = 32, reloadMs = 4294967296 }"#,
        ),
    ] {
        assert_ammo_pair_rejects(field, js_resource, lua_resource, "expected u32");
    }
}

#[test]
fn paired_weapon_ammo_resource_rejects_fractional_and_non_finite_u32_fields() {
    for (field, js_resource, lua_resource) in [
        (
            "fractional magazine",
            r#"{ kind: "ammo", type: "cells", magazine: 1.5, costPerShot: 1, reserve: 32, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 1.5, costPerShot = 1, reserve = 32, reloadMs = 1000 }"#,
        ),
        (
            "fractional costPerShot",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: 1.5, reserve: 32, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = 1.5, reserve = 32, reloadMs = 1000 }"#,
        ),
        (
            "fractional reserve",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: 1, reserve: 1.5, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = 1, reserve = 1.5, reloadMs = 1000 }"#,
        ),
        (
            "fractional reloadMs",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: 1, reserve: 32, reloadMs: 1.5 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = 1, reserve = 32, reloadMs = 1.5 }"#,
        ),
        (
            "non-finite magazine",
            r#"{ kind: "ammo", type: "cells", magazine: Infinity, costPerShot: 1, reserve: 32, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = math.huge, costPerShot = 1, reserve = 32, reloadMs = 1000 }"#,
        ),
        (
            "non-finite costPerShot",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: Infinity, reserve: 32, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = math.huge, reserve = 32, reloadMs = 1000 }"#,
        ),
        (
            "non-finite reserve",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: 1, reserve: Infinity, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = 1, reserve = math.huge, reloadMs = 1000 }"#,
        ),
        (
            "non-finite reloadMs",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: 1, reserve: 32, reloadMs: Infinity }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = 1, reserve = 32, reloadMs = math.huge }"#,
        ),
    ] {
        assert_ammo_pair_rejects(field, js_resource, lua_resource, "expected u32");
    }
}

#[test]
fn paired_weapon_ammo_resource_rejects_invalid_kind_type_and_positive_bounds() {
    let type_65 = "a".repeat(65);
    let cases = [
        (
            "unknown kind",
            r#"{ kind: "cell", type: "cells", magazine: 8, reserve: 32 }"#.to_string(),
            r#"{ kind = "cell", type = "cells", magazine = 8, reserve = 32 }"#.to_string(),
            "unknown variant",
        ),
        (
            "empty type",
            r#"{ kind: "ammo", type: "", magazine: 8, reserve: 32 }"#.to_string(),
            r#"{ kind = "ammo", type = "", magazine = 8, reserve = 32 }"#.to_string(),
            "non-empty ASCII identifier",
        ),
        (
            "65-byte type",
            format!(r#"{{ kind: "ammo", type: "{type_65}", magazine: 8, reserve: 32 }}"#),
            format!(r#"{{ kind = "ammo", type = "{type_65}", magazine = 8, reserve = 32 }}"#),
            "at most 64 bytes",
        ),
        (
            "illegal type character",
            r#"{ kind: "ammo", type: "bad ammo", magazine: 8, reserve: 32 }"#.to_string(),
            r#"{ kind = "ammo", type = "bad ammo", magazine = 8, reserve = 32 }"#.to_string(),
            "resource.type",
        ),
        (
            "non-ASCII type",
            r#"{ kind: "ammo", type: "célls", magazine: 8, reserve: 32 }"#.to_string(),
            r#"{ kind = "ammo", type = "célls", magazine = 8, reserve = 32 }"#.to_string(),
            "and be ASCII",
        ),
        (
            "zero magazine",
            r#"{ kind: "ammo", type: "cells", magazine: 0, reserve: 32 }"#.to_string(),
            r#"{ kind = "ammo", type = "cells", magazine = 0, reserve = 32 }"#.to_string(),
            "resource.magazine",
        ),
        (
            "zero costPerShot",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: 0, reserve: 32 }"#
                .to_string(),
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = 0, reserve = 32 }"#
                .to_string(),
            "resource.costPerShot",
        ),
        (
            "zero reloadMs",
            r#"{ kind: "ammo", type: "cells", magazine: 8, reserve: 32, reloadMs: 0 }"#.to_string(),
            r#"{ kind = "ammo", type = "cells", magazine = 8, reserve = 32, reloadMs = 0 }"#
                .to_string(),
            "resource.reloadMs",
        ),
    ];
    for (label, js_resource, lua_resource, expected_error) in cases {
        assert_ammo_pair_rejects(label, &js_resource, &lua_resource, expected_error);
    }
}

#[test]
fn js_weapon_ammo_resource_explicit_null_does_not_default() {
    for field in ["costPerShot", "reloadMs"] {
        let resource = format!(
            r#"{{ kind: "ammo", type: "cells", magazine: 8, costPerShot: 1, reserve: 32, reloadMs: 1000, {field}: null }}"#
        );
        let error = parse_js_ammo_resource(&resource).unwrap_err().to_string();
        assert!(
            error.contains("expected u32"),
            "unexpected {field} error: {error}"
        );
    }
}

#[test]
fn js_weapon_descriptor_rejects_invalid_credit_source() {
    let src = r#"({
        canonicalName: "reference_pistol",
        components: {
            weapon: {
                damage: 12.0,
                range: 64.0,
                fireRateMs: 180.0,
                fireMode: "semi",
                resolution: "hitscan",
                creditSource: "bad source"
            }
        }
    })"#;
    let err = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap_err());
    assert!(
        err.to_string().contains("creditSource"),
        "unexpected error: {err}"
    );
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
                creditSource = "player.reference-pistol:alt",
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
    assert_eq!(
        weapon.credit_source.as_deref(),
        Some("player.reference-pistol:alt")
    );
}

#[test]
fn lua_weapon_descriptor_without_credit_source_parses_as_none() {
    let src = r#"return {
        canonicalName = "reference_pistol",
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
    let weapon = d.weapon.expect("weapon present");
    assert_eq!(weapon.credit_source, None);
    assert_eq!(weapon.resource, None);
}

#[test]
fn lua_weapon_descriptor_rejects_invalid_credit_source() {
    let src = r#"return {
        canonicalName = "reference_pistol",
        components = {
            weapon = {
                damage = 12.0,
                range = 64.0,
                fireRateMs = 180.0,
                fireMode = "auto",
                resolution = "hitscan",
                creditSource = "rocket/primary",
            }
        }
    }"#;
    let err = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap_err());
    assert!(
        err.to_string().contains("creditSource"),
        "unexpected error: {err}"
    );
}
