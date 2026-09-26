// Tests: descriptor sound fields parse identically in both runtimes, round-trip,
// reject unknown keys, and are optional throughout.
// See: context/lib/audio.md §4

use super::super::*;
use super::common::*;

const JS_WEAPON: &str =
    r#"damage: 12, range: 64, fireRateMs: 180, fireMode: "semi", resolution: "hitscan""#;
const LUA_WEAPON: &str =
    r#"damage = 12, range = 64, fireRateMs = 180, fireMode = "semi", resolution = "hitscan""#;

fn js_weapon(extra: &str) -> String {
    format!("({{ components: {{ weapon: {{ {JS_WEAPON}{extra} }} }} }})")
}

fn lua_weapon(extra: &str) -> String {
    format!("return {{ components = {{ weapon = {{ {LUA_WEAPON}{extra} }} }} }}")
}

#[test]
fn weapon_sounds_parse_identically_in_both_runtimes_and_round_trip() {
    let js = eval_js(
        &js_weapon(
            r#", sounds: { fire: "sfx/shotgun_fire", dryFire: "sfx/click", impact: "sfx/pellet_hit",
                reloadStart: "sfx/shotgun_open", reloadShell: "sfx/shell_in", reloadComplete: "sfx/shotgun_pump" }"#,
        ),
        |ctx, v| entity_descriptor_from_js(ctx, v).unwrap(),
    );
    let lua = eval_lua(
        &lua_weapon(
            r#", sounds = { fire = "sfx/shotgun_fire", dryFire = "sfx/click", impact = "sfx/pellet_hit",
                reloadStart = "sfx/shotgun_open", reloadShell = "sfx/shell_in", reloadComplete = "sfx/shotgun_pump" }"#,
        ),
        |v| entity_descriptor_from_lua(v).unwrap(),
    );
    let js = js.weapon.unwrap().sounds.expect("sounds present");
    let lua = lua.weapon.unwrap().sounds.expect("sounds present");
    assert_eq!(js, lua);
    assert_eq!(js.reload_shell.as_deref(), Some("sfx/shell_in"));

    let wire = serde_json::to_value(&js).unwrap();
    assert_eq!(wire["dryFire"], "sfx/click");
    assert_eq!(serde_json::from_value::<WeaponSounds>(wire).unwrap(), js);
}

#[test]
fn weapon_sounds_are_optional_throughout() {
    let absent = eval_js(&js_weapon(""), |ctx, v| {
        entity_descriptor_from_js(ctx, v).unwrap()
    });
    assert_eq!(absent.weapon.unwrap().sounds, None);
    let partial = eval_lua(&lua_weapon(r#", sounds = { fire = "sfx/pew" }"#), |v| {
        entity_descriptor_from_lua(v).unwrap()
    });
    let sounds = partial.weapon.unwrap().sounds.unwrap();
    assert_eq!(sounds.fire.as_deref(), Some("sfx/pew"));
    assert_eq!(sounds.impact, None);
}

#[test]
fn an_unknown_key_inside_weapon_sounds_is_rejected_in_both_runtimes() {
    let js = eval_js(
        &js_weapon(r#", sounds: { fyre: "sfx/pew" }"#),
        entity_descriptor_from_js,
    )
    .unwrap_err()
    .to_string();
    let lua = eval_lua(
        &lua_weapon(r#", sounds = { fyre = "sfx/pew" }"#),
        entity_descriptor_from_lua,
    )
    .unwrap_err()
    .to_string();
    for error in [&js, &lua] {
        assert!(error.contains("fyre"), "{error}");
    }
}

#[test]
fn a_malformed_weapon_sound_key_names_its_field() {
    let js = eval_js(
        &js_weapon(r#", sounds: { impact: "../hit" }"#),
        entity_descriptor_from_js,
    )
    .unwrap_err()
    .to_string();
    assert!(js.contains("components.weapon.sounds.impact"), "{js}");
}

const JS_MOVEMENT: &str = r#"capsule: { radius: 0.4, halfHeight: 0.8, eyeHeight: 0.5 },
    ground: { speed: { walk: 7.0, run: 11.0, crouch: 3.0 }, accel: 10.0, stepHeight: 0.3, maxSlope: 45.0 },
    air: { forwardSteer: 0.0, accel: 0.7, maxControlSpeed: 0.5, bunnyHop: false, jumps: 0, jumpVelocity: 5.5, jumpCeiling: 0.0 },
    fall: { terminalVelocity: 40.0 }"#;
const LUA_MOVEMENT: &str = r#"capsule = { radius = 0.4, halfHeight = 0.8, eyeHeight = 0.5 },
    ground = { speed = { walk = 7.0, run = 11.0, crouch = 3.0 }, accel = 10.0, stepHeight = 0.3, maxSlope = 45.0 },
    air = { forwardSteer = 0.0, accel = 0.7, maxControlSpeed = 0.5, bunnyHop = false, jumps = 0, jumpVelocity = 5.5, jumpCeiling = 0.0 },
    fall = { terminalVelocity = 40.0 }"#;

#[test]
fn movement_sounds_parse_identically_and_reject_unknown_keys() {
    let js = eval_js(
        &format!(
            r#"({{ components: {{ movement: {{ {JS_MOVEMENT}, sounds: {{ land: "sfx/land", jump: "sfx/jump" }} }} }} }})"#
        ),
        |ctx, v| entity_descriptor_from_js(ctx, v).unwrap(),
    );
    let lua = eval_lua(
        &format!(
            r#"return {{ components = {{ movement = {{ {LUA_MOVEMENT}, sounds = {{ land = "sfx/land", jump = "sfx/jump" }} }} }} }}"#
        ),
        |v| entity_descriptor_from_lua(v).unwrap(),
    );
    let js = js.movement.unwrap().sounds.expect("sounds present");
    assert_eq!(Some(&js), lua.movement.unwrap().sounds.as_ref());
    assert_eq!(js.jump.as_deref(), Some("sfx/jump"));

    let absent = eval_js(
        &format!("({{ components: {{ movement: {{ {JS_MOVEMENT} }} }} }})"),
        |ctx, v| entity_descriptor_from_js(ctx, v).unwrap(),
    );
    assert_eq!(absent.movement.unwrap().sounds, None);

    let js_error = eval_js(
        &format!(r#"({{ components: {{ movement: {{ {JS_MOVEMENT}, sounds: {{ landing: "sfx/land" }} }} }} }})"#),
        entity_descriptor_from_js,
    )
    .unwrap_err()
    .to_string();
    let lua_error = eval_lua(
        &format!(r#"return {{ components = {{ movement = {{ {LUA_MOVEMENT}, sounds = {{ landing = "sfx/land" }} }} }} }}"#),
        entity_descriptor_from_lua,
    )
    .unwrap_err()
    .to_string();
    for error in [&js_error, &lua_error] {
        assert!(
            error.contains("movement.sounds") && error.contains("landing"),
            "{error}"
        );
    }
}

fn js_graph(attack_extra: &str, activity_extra: &str) -> String {
    format!(
        r#"({{ components: {{ behavior: {{
            initial: "idle", moveSpeed: 3,
            attacks: {{ bite: {{ damage: 10, maxRange: 2, cooldownMs: 800{attack_extra} }} }},
            activities: {{
                idle: {{ animation: "idle", motion: "hold" }},
                alerted: {{ animation: "idle", motion: "hold"{activity_extra} }}
            }},
            transitions: {{ "*": [] }}
        }} }} }})"#
    )
}

fn lua_graph(attack_extra: &str, activity_extra: &str) -> String {
    format!(
        r#"return {{ components = {{ behavior = {{
            initial = "idle", moveSpeed = 3,
            attacks = {{ bite = {{ damage = 10, maxRange = 2, cooldownMs = 800{attack_extra} }} }},
            activities = {{
                idle = {{ animation = "idle", motion = "hold" }},
                alerted = {{ animation = "idle", motion = "hold"{activity_extra} }}
            }},
            transitions = {{ ["*"] = {{}} }}
        }} }} }}"#
    )
}

#[test]
fn attack_and_activity_sounds_parse_identically_in_both_runtimes() {
    let js = eval_js(
        &js_graph(r#", sound: "sfx/bite""#, r#", sound: "sfx/growl""#),
        |ctx, v| entity_descriptor_from_js(ctx, v).unwrap(),
    );
    let lua = eval_lua(
        &lua_graph(r#", sound = "sfx/bite""#, r#", sound = "sfx/growl""#),
        |v| entity_descriptor_from_lua(v).unwrap(),
    );
    let (js, lua) = (js.behavior.unwrap(), lua.behavior.unwrap());
    assert_eq!(js.attacks["bite"].sound.as_deref(), Some("sfx/bite"));
    assert_eq!(js.attacks["bite"].sound, lua.attacks["bite"].sound);
    assert_eq!(
        js.envelope.activities["alerted"].sound.as_deref(),
        Some("sfx/growl"),
        "an entry sound needs no onEnter",
    );
    assert_eq!(
        js.envelope.activities["alerted"].sound,
        lua.envelope.activities["alerted"].sound
    );
    assert_eq!(js.envelope.activities["idle"].sound, None);
}
