//! The two real VM bridges must preserve the same authored combat/movement contract.
use super::super::*;
use super::common::*;

fn weapon_pair(js: &str, lua: &str) -> [Result<EntityTypeDescriptor, DescriptorError>; 2] {
    [
        eval_js(
            &format!(
                r#"({{ components: {{ weapon: {{ damage: 0, range: 64, fireRateMs: 180, fireMode: "semi", resolution: "hitscan"{js} }} }} }})"#
            ),
            entity_descriptor_from_js,
        ),
        eval_lua(
            &format!(
                r#"return {{ components = {{ weapon = {{ damage = 0, range = 64, fireRateMs = 180, fireMode = "semi", resolution = "hitscan"{lua} }} }} }}"#
            ),
            entity_descriptor_from_lua,
        ),
    ]
}

#[test]
fn knockback_direct_parsers_preserve_independent_zero_damage_push_and_defaults() {
    for result in weapon_pair(", knockback: { speed: 12 }", ", knockback = { speed = 12 }") {
        let weapon = result.unwrap().weapon.unwrap();
        let impulse = weapon.knockback.unwrap();
        assert!(weapon.damage.abs() < f32::EPSILON);
        assert!((impulse.speed - 12.0).abs() < f32::EPSILON);
        assert!(impulse.upward_bias.abs() < f32::EPSILON);
    }
    for result in weapon_pair("", "") {
        assert!(result.unwrap().weapon.unwrap().knockback.is_none());
    }
}

#[test]
fn knockback_direct_parsers_reject_missing_nonfinite_out_of_range_and_malformed_fields() {
    for (js, lua) in [
        ("{}", "{}"),
        ("[12]", "{12}"),
        ("{ speed: -1 }", "{ speed = -1 }"),
        ("{ speed: 1001 }", "{ speed = 1001 }"),
        ("{ speed: Infinity }", "{ speed = math.huge }"),
        ("{ speed: NaN }", "{ speed = 0/0 }"),
        (
            "{ speed: 1, upwardBias: 1.1 }",
            "{ speed = 1, upwardBias = 1.1 }",
        ),
        ("{ speed: '12' }", "{ speed = '12' }"),
        (
            "{ speed: 1, upward_bias: 1 }",
            "{ speed = 1, upward_bias = 1 }",
        ),
    ] {
        for result in weapon_pair(
            &format!(", knockback: {js}"),
            &format!(", knockback = {lua}"),
        ) {
            assert!(result.is_err(), "invalid impulse accepted: {js}");
        }
    }
}

fn splash_pair(js: &str, lua: &str) -> [Result<EntityTypeDescriptor, DescriptorError>; 2] {
    [
        eval_js(
            &format!(
                r#"({{ components: {{ weapon: {{ damage: 0, range: 64, fireRateMs: 180, fireMode: "semi", resolution: "projectile", projectile: {{ speed: 40, radius: 0.1, lifetimeMs: 2000, visual: {{ body: {{ kind: "sprite", sprite: "orb.png" }} }} }}, splash: {{ radius: 6, selfDamage: false, knockback: {js} }} }} }} }})"#
            ),
            entity_descriptor_from_js,
        ),
        eval_lua(
            &format!(
                r#"return {{ components = {{ weapon = {{ damage = 0, range = 64, fireRateMs = 180, fireMode = "semi", resolution = "projectile", projectile = {{ speed = 40, radius = 0.1, lifetimeMs = 2000, visual = {{ body = {{ kind = "sprite", sprite = "orb.png" }} }} }}, splash = {{ radius = 6, selfDamage = false, knockback = {lua} }} }} }} }}"#
            ),
            entity_descriptor_from_lua,
        ),
    ]
}

#[test]
fn knockback_splash_parsers_preserve_self_push_without_self_damage_and_separate_falloff() {
    for result in splash_pair("{ speed: 20 }", "{ speed = 20 }") {
        let splash = result.unwrap().weapon.unwrap().splash.unwrap();
        assert!(!splash.self_damage);
        let push = splash.knockback.unwrap();
        assert!((push.self_scale - 1.0).abs() < f32::EPSILON);
        assert!(push.min_fraction.abs() < f32::EPSILON);
    }
    for result in splash_pair(
        "{ speed: 20, minFraction: 0.5, selfScale: 0, upwardBias: 0.4 }",
        "{ speed = 20, minFraction = 0.5, selfScale = 0, upwardBias = 0.4 }",
    ) {
        let splash = result.unwrap().weapon.unwrap().splash.unwrap();
        assert!(splash.min_fraction.abs() < f32::EPSILON);
        let push = splash.knockback.unwrap();
        assert!((push.min_fraction - 0.5).abs() < f32::EPSILON);
        assert!(push.self_scale.abs() < f32::EPSILON);
        assert!((push.upward_bias - 0.4).abs() < f32::EPSILON);
    }
    for (js, lua) in [
        (
            "{ speed: 1, selfScale: 11 }",
            "{ speed = 1, selfScale = 11 }",
        ),
        (
            "{ speed: 1, minFraction: -1 }",
            "{ speed = 1, minFraction = -1 }",
        ),
    ] {
        for result in splash_pair(js, lua) {
            assert!(result.is_err());
        }
    }
}

fn response_pair(
    js: &str,
    lua: &str,
    behavior: bool,
) -> [Result<EntityTypeDescriptor, DescriptorError>; 2] {
    if behavior {
        [
            eval_js(
                &format!(
                    r#"({{ components: {{ behavior: {{ initial: "idle", activities: {{ idle: {{ animation: "idle", motion: "hold" }} }}, transitions: {{}}, moveSpeed: 3, knockback: {js} }} }} }})"#
                ),
                entity_descriptor_from_js,
            ),
            eval_lua(
                &format!(
                    r#"return {{ components = {{ behavior = {{ initial = "idle", activities = {{ idle = {{ animation = "idle", motion = "hold" }} }}, transitions = {{}}, moveSpeed = 3, knockback = {lua} }} }} }}"#
                ),
                entity_descriptor_from_lua,
            ),
        ]
    } else {
        let js_source =
            JS_PLAYER_MOVEMENT.replace("movement: {", &format!("movement: {{ knockback: {js},"));
        let lua_source = format!(
            r#"return {{ components = {{ movement = {{ knockback = {lua}, capsule = {{ radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 }}, ground = {{ speed = {{ walk = 7, run = 11, crouch = 3 }}, accel = 10, stepHeight = 0.3, maxSlope = 45 }}, air = {{ forwardSteer = 0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0 }}, fall = {{ terminalVelocity = 40 }} }} }} }}"#
        );
        [
            eval_js(&js_source, entity_descriptor_from_js),
            eval_lua(&lua_source, entity_descriptor_from_lua),
        ]
    }
}

#[test]
fn knockback_response_parsers_match_player_and_enemy_defaults_overrides_and_rejection() {
    for behavior in [false, true] {
        for (js, lua, expected_scale, expected_drag) in [
            ("{}", "{}", 1.0, 8.0),
            (
                "{ scale: 0.2, groundDrag: 0, control: 0 }",
                "{ scale = 0.2, groundDrag = 0, control = 0 }",
                0.2,
                0.0,
            ),
        ] {
            for result in response_pair(js, lua, behavior) {
                let descriptor = result.unwrap();
                let response = if behavior {
                    descriptor.behavior.unwrap().knockback
                } else {
                    descriptor.movement.unwrap().knockback
                };
                assert!((response.scale - expected_scale).abs() < f32::EPSILON);
                assert!((response.ground_drag - expected_drag).abs() < f32::EPSILON);
                assert!(response.air_drag.abs() < f32::EPSILON);
            }
        }
        for (js, lua) in [
            ("[1]", "{1}"),
            ("{ scale: -1 }", "{ scale = -1 }"),
            ("{ groundDrag: 1001 }", "{ groundDrag = 1001 }"),
            ("{ airDrag: -1 }", "{ airDrag = -1 }"),
            ("{ control: 1.1 }", "{ control = 1.1 }"),
            ("{ control: Infinity }", "{ control = math.huge }"),
            ("{ ground_drag: 1 }", "{ ground_drag = 1 }"),
        ] {
            for result in response_pair(js, lua, behavior) {
                assert!(result.is_err(), "invalid response accepted: {js}");
            }
        }
    }
}
