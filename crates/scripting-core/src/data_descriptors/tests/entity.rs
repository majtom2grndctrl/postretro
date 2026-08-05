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

fn parse_js_weapon_stats(stats: &str) -> Result<EntityTypeDescriptor, DescriptorError> {
    let src = format!(
        r#"({{ components: {{ weapon: {{ damage: 12, range: 64, fireRateMs: 180, fireMode: "semi", resolution: "hitscan"{stats} }} }} }})"#
    );
    eval_js(&src, entity_descriptor_from_js)
}

fn parse_lua_weapon_stats(stats: &str) -> Result<EntityTypeDescriptor, DescriptorError> {
    let src = format!(
        r#"return {{ components = {{ weapon = {{ damage = 12, range = 64, fireRateMs = 180, fireMode = "semi", resolution = "hitscan"{stats} }} }} }}"#
    );
    eval_lua(&src, entity_descriptor_from_lua)
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
    assert!(d.inventory.is_none());
    assert!(d.light.is_none());
    assert!(d.emitter.is_none());
    assert!(d.weapon.is_none());
    assert!(d.touchable.is_none());
}

#[test]
fn touchable_descriptor_has_js_luau_parity_for_defaults_and_radius_validation() {
    let js = eval_js(
        r#"({ components: { touchable: { mode: "press" } } })"#,
        |ctx, value| entity_descriptor_from_js(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { components = { touchable = { mode = "press" } } }"#,
        |value| entity_descriptor_from_lua(value).unwrap(),
    );
    let js_touchable = js.touchable.expect("QuickJS touchable descriptor parses");
    let lua_touchable = lua.touchable.expect("Luau touchable descriptor parses");
    assert_eq!(js_touchable, lua_touchable);
    assert_eq!(js_touchable.mode, TouchMode::Press);
    assert!((js_touchable.radius - 40.0).abs() <= f32::EPSILON);

    for (js_source, lua_source) in [
        (
            r#"({ components: { touchable: { radius: 0 } } })"#,
            r#"return { components = { touchable = { radius = 0 } } }"#,
        ),
        (
            r#"({ components: { touchable: { radius: -1 } } })"#,
            r#"return { components = { touchable = { radius = -1 } } }"#,
        ),
    ] {
        let js_error = eval_js(js_source, |ctx, value| {
            entity_descriptor_from_js(ctx, value).unwrap_err()
        });
        let lua_error = eval_lua(lua_source, |value| {
            entity_descriptor_from_lua(value).unwrap_err()
        });
        assert!(js_error.to_string().contains("components.touchable.radius"));
        assert_eq!(js_error.to_string(), lua_error.to_string());
    }
}

#[test]
fn js_entity_descriptor_with_inventory_and_weapon_component_deserializes() {
    let src = r#"({
        canonicalName: "player",
        components: {
            inventory: { loadout: ["reference_pistol"] },
            weapon: {
                damage: 12.0,
                range: 64.0,
                fireRateMs: 180.0,
                lowerMs: 25,
                raiseMs: 40,
                fireMode: "semi",
                resolution: "hitscan",
                creditSource: "player.reference-pistol:primary"
            }
        }
    })"#;
    let d = eval_js(src, |ctx, v| entity_descriptor_from_js(ctx, v).unwrap());
    assert_eq!(d.inventory.unwrap().loadout, ["reference_pistol"]);
    let weapon = d.weapon.expect("weapon present");
    assert_eq!(weapon.damage, 12.0);
    assert_eq!(weapon.range, 64.0);
    assert_eq!(weapon.cooldown_ms, 180.0);
    assert_eq!(weapon.lower_ms, 25);
    assert_eq!(weapon.raise_ms, 40);
    assert_eq!(weapon.fire_mode, FireMode::Semi);
    assert_eq!(weapon.resolution, ResolutionMode::Hitscan);
    assert_eq!(
        weapon.credit_source.as_deref(),
        Some("player.reference-pistol:primary")
    );
}

#[test]
fn js_loadout_builder_rejects_invalid_descriptor_references() {
    let rt = rquickjs::Runtime::new().expect("QuickJS runtime creates");
    let ctx = rquickjs::Context::full(&rt).expect("QuickJS context creates");

    ctx.with(|jsctx| {
        crate::quickjs::evaluate_prelude(&jsctx).expect("SDK prelude evaluates");

        for (label, source, expected) in [
            (
                "non-descriptor",
                r#"defineEntity({ components: { inventory: { loadout: [42] } } });"#,
                "must reference an entity descriptor",
            ),
            (
                "descriptor without weapon",
                r#"defineEntity({ components: { inventory: { loadout: [{ canonicalName: "not_weapon", components: {} }] } } });"#,
                "must reference a descriptor with a weapon block",
            ),
            (
                "descriptor with non-object weapon",
                r#"defineEntity({ components: { inventory: { loadout: [{ canonicalName: "not_weapon", components: { weapon: [] } }] } } });"#,
                "components.inventory.loadout[0] must reference a descriptor with a weapon block",
            ),
            (
                "descriptor without canonical name",
                r#"defineEntity({ components: { inventory: { loadout: [{ components: { weapon: {} } }] } } });"#,
                "must reference a descriptor with a canonical name",
            ),
        ] {
            let error = crate::quickjs::run_script::<()>(&jsctx, source, label)
                .expect_err("invalid loadout reference must reject");
            assert!(
                error.to_string().contains(expected),
                "QuickJS {label} rejection should contain {expected:?}, got: {error}"
            );
        }
    });
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
fn paired_weapon_pellet_stats_accept_authored_values_and_default_legacy_weapons() {
    let js_default = parse_js_weapon_stats("").unwrap();
    let lua_default = parse_lua_weapon_stats("").unwrap();
    let js_default = js_default.weapon.expect("QuickJS weapon parses");
    let lua_default = lua_default.weapon.expect("Luau weapon parses");
    assert_eq!(js_default, lua_default);
    assert_eq!(js_default.pellet_count, 1);
    assert!((js_default.spread_degrees - 0.0).abs() < f32::EPSILON);

    let js_authored = parse_js_weapon_stats(", pelletCount: 8, spreadDegrees: 4").unwrap();
    let lua_authored = parse_lua_weapon_stats(", pelletCount = 8, spreadDegrees = 4").unwrap();
    let js_authored = js_authored.weapon.expect("QuickJS weapon parses");
    let lua_authored = lua_authored.weapon.expect("Luau weapon parses");
    assert_eq!(js_authored, lua_authored);
    assert_eq!(js_authored.pellet_count, 8);
    assert!((js_authored.spread_degrees - 4.0).abs() < f32::EPSILON);
}

#[test]
fn paired_weapon_pellet_stats_reject_out_of_range_values() {
    for (label, js_stats, lua_stats, expected) in [
        (
            "zero pellet count",
            ", pelletCount: 0",
            ", pelletCount = 0",
            "pelletCount",
        ),
        (
            "pellet count over cap",
            ", pelletCount: 33",
            ", pelletCount = 33",
            "pelletCount",
        ),
        (
            "negative spread",
            ", spreadDegrees: -0.1",
            ", spreadDegrees = -0.1",
            "spreadDegrees",
        ),
        (
            "spread over cap",
            ", spreadDegrees: 45.1",
            ", spreadDegrees = 45.1",
            "spreadDegrees",
        ),
    ] {
        let js_error = parse_js_weapon_stats(js_stats).unwrap_err().to_string();
        let lua_error = parse_lua_weapon_stats(lua_stats).unwrap_err().to_string();
        assert!(js_error.contains(expected), "QuickJS {label}: {js_error}");
        assert!(lua_error.contains(expected), "Luau {label}: {lua_error}");
        assert_eq!(js_error, lua_error, "{label}");
    }
}

#[test]
fn paired_weapon_pellet_spread_rejects_non_finite_values_at_the_conversion_boundary() {
    for (js_stats, lua_stats) in [
        (", spreadDegrees: Infinity", ", spreadDegrees = math.huge"),
        (", spreadDegrees: -Infinity", ", spreadDegrees = -math.huge"),
        (", spreadDegrees: NaN", ", spreadDegrees = 0/0"),
    ] {
        let js_error = parse_js_weapon_stats(js_stats).unwrap_err().to_string();
        let lua_error = parse_lua_weapon_stats(lua_stats).unwrap_err().to_string();
        for error in [&js_error, &lua_error] {
            assert!(
                error.contains("non-finite number at `spreadDegrees`"),
                "{error}"
            );
        }
    }
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
fn optional_weapon_model_paths_reject_empty_js_and_luau_values() {
    for field in ["thirdPersonModel", "viewmodel"] {
        let js = format!(
            r#"({{ components: {{ weapon: {{ damage: 12, range: 64, fireRateMs: 180, fireMode: "semi", resolution: "hitscan", {field}: "" }} }} }})"#
        );
        let lua = format!(
            r#"return {{ components = {{ weapon = {{ damage = 12, range = 64, fireRateMs = 180, fireMode = "semi", resolution = "hitscan", {field} = "" }} }} }}"#
        );
        let js_error = eval_js(&js, entity_descriptor_from_js).unwrap_err();
        let lua_error = eval_lua(&lua, entity_descriptor_from_lua).unwrap_err();
        assert!(js_error.to_string().contains("content-relative model path"));
        assert!(
            lua_error
                .to_string()
                .contains("content-relative model path")
        );
    }
}

#[test]
fn optional_weapon_model_paths_reject_unsupported_vm_values_with_field_errors() {
    for field in ["thirdPersonModel", "viewmodel"] {
        let js = format!(
            r#"({{ components: {{ weapon: {{ damage: 12, range: 64, fireRateMs: 180, fireMode: "semi", resolution: "hitscan", {field}: () => {{}} }} }} }})"#
        );
        let lua = format!(
            r#"return {{ components = {{ weapon = {{ damage = 12, range = 64, fireRateMs = 180, fireMode = "semi", resolution = "hitscan", {field} = function() end }} }} }}"#
        );
        let js_error = eval_js(&js, entity_descriptor_from_js)
            .unwrap_err()
            .to_string();
        let lua_error = eval_lua(&lua, entity_descriptor_from_lua)
            .unwrap_err()
            .to_string();
        assert!(
            js_error.contains(&format!("components.weapon.{field}")),
            "{js_error}"
        );
        assert!(
            lua_error.contains(&format!("components.weapon.{field}")),
            "{lua_error}"
        );
        assert!(js_error.contains("must be a string"), "{js_error}");
        assert!(lua_error.contains("must be a string"), "{lua_error}");
    }
}

#[test]
fn optional_weapon_model_paths_reject_escape_and_platform_absolute_forms_in_both_vms() {
    for invalid in [
        "/tmp/model.gltf",
        "../model.gltf",
        "models/../model.gltf",
        r"..\model.gltf",
        r"C:\models\model.gltf",
        "C:/models/model.gltf",
        r"\\server\share\model.gltf",
    ] {
        for field in ["thirdPersonModel", "viewmodel"] {
            let js = format!(
                r#"({{ components: {{ weapon: {{ damage: 12, range: 64, fireRateMs: 180, fireMode: "semi", resolution: "hitscan", {field}: {invalid:?} }} }} }})"#
            );
            let lua_value = invalid.replace('\\', "\\\\").replace('"', "\\\"");
            let lua = format!(
                r#"return {{ components = {{ weapon = {{ damage = 12, range = 64, fireRateMs = 180, fireMode = "semi", resolution = "hitscan", {field} = "{lua_value}" }} }} }}"#
            );
            let js_error = eval_js(&js, entity_descriptor_from_js)
                .unwrap_err()
                .to_string();
            let lua_error = eval_lua(&lua, entity_descriptor_from_lua)
                .unwrap_err()
                .to_string();
            assert!(
                js_error.contains(&format!("components.weapon.{field}")),
                "{js_error}"
            );
            assert!(
                lua_error.contains(&format!("components.weapon.{field}")),
                "{lua_error}"
            );
            assert!(js_error.contains("content-relative"), "{js_error}");
            assert!(lua_error.contains("content-relative"), "{lua_error}");
        }
    }
}

#[test]
fn paired_weapon_ammo_resource_defaults_match() {
    let js = ammo_resource(
        parse_js_ammo_resource(
            r#"{ kind: "ammo", type: "bullets.light", magazine: 12, costPerShot: undefined, reserve: 48, reloadMs: undefined, reloadStyle: undefined }"#,
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
    assert_eq!(js.reload_style, ReloadStyle::Magazine);
}

#[test]
fn paired_weapon_ammo_resource_accepts_reload_style_values_and_rejects_unknown() {
    for (js_resource, lua_resource, expected) in [
        (
            r#"{ kind: "ammo", type: "shells", magazine: 8, reserve: 32, reloadStyle: "magazine" }"#,
            r#"{ kind = "ammo", type = "shells", magazine = 8, reserve = 32, reloadStyle = "magazine" }"#,
            ReloadStyle::Magazine,
        ),
        (
            r#"{ kind: "ammo", type: "shells", magazine: 8, reserve: 32, reloadStyle: "perShell" }"#,
            r#"{ kind = "ammo", type = "shells", magazine = 8, reserve = 32, reloadStyle = "perShell" }"#,
            ReloadStyle::PerShell,
        ),
    ] {
        let js = ammo_resource(parse_js_ammo_resource(js_resource).unwrap());
        let lua = ammo_resource(parse_lua_ammo_resource(lua_resource).unwrap());
        assert_eq!(js.reload_style, expected);
        assert_eq!(js, lua);
    }

    let js_error = parse_js_ammo_resource(
        r#"{ kind: "ammo", type: "shells", magazine: 8, reserve: 32, reloadStyle: "belt" }"#,
    );
    let lua_error = parse_lua_ammo_resource(
        r#"{ kind = "ammo", type = "shells", magazine = 8, reserve = 32, reloadStyle = "belt" }"#,
    );
    let js_error = js_error.unwrap_err().to_string();
    let lua_error = lua_error.unwrap_err().to_string();
    assert!(js_error.contains("unknown variant"), "{js_error}");
    assert_eq!(js_error, lua_error);
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
    ] {
        assert_ammo_pair_rejects(field, js_resource, lua_resource, "expected u32");
    }

    // Non-finite values are rejected one layer earlier, by the JSON bridge, so
    // they never reach the u32 serde error. Both runtimes name the field.
    for (field, js_resource, lua_resource) in [
        (
            "magazine",
            r#"{ kind: "ammo", type: "cells", magazine: Infinity, costPerShot: 1, reserve: 32, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = math.huge, costPerShot = 1, reserve = 32, reloadMs = 1000 }"#,
        ),
        (
            "costPerShot",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: Infinity, reserve: 32, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = math.huge, reserve = 32, reloadMs = 1000 }"#,
        ),
        (
            "reserve",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: 1, reserve: -Infinity, reloadMs: 1000 }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = 1, reserve = -math.huge, reloadMs = 1000 }"#,
        ),
        (
            "reloadMs",
            r#"{ kind: "ammo", type: "cells", magazine: 8, costPerShot: 1, reserve: 32, reloadMs: NaN }"#,
            r#"{ kind = "ammo", type = "cells", magazine = 8, costPerShot = 1, reserve = 32, reloadMs = 0/0 }"#,
        ),
    ] {
        assert_ammo_pair_rejects(
            field,
            js_resource,
            lua_resource,
            &format!("non-finite number at `resource.{field}`"),
        );
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
fn lua_entity_descriptor_with_inventory_and_weapon_component_deserializes() {
    let src = r#"return {
        canonicalName = "player",
        components = {
            inventory = { loadout = { "reference_pistol" } },
            weapon = {
                damage = 12.0,
                range = 64.0,
                fireRateMs = 180.0,
                lowerMs = 25,
                raiseMs = 40,
                fireMode = "auto",
                resolution = "hitscan",
                creditSource = "player.reference-pistol:alt",
            }
        }
    }"#;
    let d = eval_lua(src, |v| entity_descriptor_from_lua(v).unwrap());
    assert_eq!(d.inventory.unwrap().loadout, ["reference_pistol"]);
    let weapon = d.weapon.expect("weapon present");
    assert_eq!(weapon.damage, 12.0);
    assert_eq!(weapon.cooldown_ms, 180.0);
    assert_eq!(weapon.lower_ms, 25);
    assert_eq!(weapon.raise_ms, 40);
    assert_eq!(weapon.fire_mode, FireMode::Auto);
    assert_eq!(weapon.resolution, ResolutionMode::Hitscan);
    assert_eq!(
        weapon.credit_source.as_deref(),
        Some("player.reference-pistol:alt")
    );
}

#[test]
fn luau_loadout_builder_rejects_invalid_descriptor_references() {
    const DATA_SCRIPT_LUAU: &str = include_str!("../../../../../sdk/lib/data_script.luau");

    let lua = mlua::Lua::new();
    let sdk: mlua::Table = lua
        .load(DATA_SCRIPT_LUAU)
        .set_name("data_script.luau")
        .eval()
        .expect("data-script SDK evaluates");
    lua.globals()
        .set("Postretro", sdk)
        .expect("SDK installs for test");

    for (label, source, expected) in [
        (
            "non-descriptor",
            r#"Postretro.defineEntity({ components = { inventory = { loadout = { 42 } } } })"#,
            "must reference an entity descriptor",
        ),
        (
            "descriptor without weapon",
            r#"Postretro.defineEntity({ components = { inventory = { loadout = { { canonicalName = "not_weapon", components = {} } } } } })"#,
            "must reference a descriptor with a weapon block",
        ),
        (
            "descriptor with non-table weapon",
            r#"Postretro.defineEntity({ components = { inventory = { loadout = { { canonicalName = "not_weapon", components = { weapon = 42 } } } } } })"#,
            "components.inventory.loadout[0] must reference a descriptor with a weapon block",
        ),
        (
            "descriptor with array-like weapon",
            r#"Postretro.defineEntity({ components = { inventory = { loadout = { { canonicalName = "not_weapon", components = { weapon = { 42 } } } } } } })"#,
            "components.inventory.loadout[0] must reference a descriptor with a weapon block",
        ),
        (
            "descriptor without canonical name",
            r#"Postretro.defineEntity({ components = { inventory = { loadout = { { components = { weapon = {} } } } } } })"#,
            "must reference a descriptor with a canonical name",
        ),
    ] {
        let error = lua
            .load(source)
            .set_name(label)
            .exec()
            .expect_err("invalid loadout reference must reject");
        assert!(
            error.to_string().contains(expected),
            "Luau {label} rejection should contain {expected:?}, got: {error}"
        );
    }
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
