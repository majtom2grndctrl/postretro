// Tests: descriptor sound fields parse identically in both runtimes, round-trip,
// reject unknown keys, and are optional throughout.
// See: context/lib/audio.md §4

use super::super::*;
use super::common::*;

const JS_WEAPON: &str = r#"damage: 12, range: 64, primary : { trigger : "press", recoveryMs : 180, steps : [{ kind: "shot" }] },  resolution: "hitscan""#;
const LUA_WEAPON: &str = r#"damage = 12, range = 64, primary = { trigger = "press", recoveryMs = 180, steps = { { kind = "shot" } } },  resolution = "hitscan""#;

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

#[test]
fn a_function_valued_weapon_sound_key_is_rejected_in_luau() {
    // Regression: Luau's generic JSON bridge maps a function/userdata/thread
    // to JSON null, so `sounds.fire = function() end` silently read as an
    // absent key instead of an authoring error, unlike QuickJS.
    let lua = eval_lua(
        &lua_weapon(r#", sounds = { fire = function() end }"#),
        entity_descriptor_from_lua,
    )
    .unwrap_err()
    .to_string();
    assert!(lua.contains("components.weapon.sounds.fire"), "{lua}");
    assert!(lua.contains("must be a string"), "{lua}");
}

#[test]
fn a_function_valued_weapon_sounds_table_is_rejected_in_luau() {
    // Regression: the whole `sounds` field, not just one of its keys, can
    // also be handed a function; it must be rejected rather than silently
    // read as an absent sound table.
    let lua = eval_lua(
        &lua_weapon(r#", sounds = function() end"#),
        entity_descriptor_from_lua,
    )
    .unwrap_err()
    .to_string();
    assert!(lua.contains("components.weapon.sounds"), "{lua}");
    assert!(lua.contains("must be an object"), "{lua}");
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

#[test]
fn a_function_valued_attack_sound_is_rejected_in_luau() {
    // Regression: `attacks.bite.sound = function() end` degraded to JSON
    // null through Luau's generic bridge and read as an absent attack sound
    // instead of an authoring error, unlike QuickJS.
    let lua = eval_lua(
        &lua_graph(r#", sound = function() end"#, ""),
        entity_descriptor_from_lua,
    )
    .unwrap_err()
    .to_string();
    assert!(
        lua.contains("components.behavior.attacks.bite.sound"),
        "{lua}"
    );
    assert!(lua.contains("must be a string"), "{lua}");
}

#[test]
fn a_function_valued_activity_sound_is_rejected_in_luau() {
    // Regression: `activities.alerted.sound = function() end` degraded to
    // JSON null and read as an absent activity sound instead of an
    // authoring error, unlike QuickJS.
    let lua = eval_lua(
        &lua_graph("", r#", sound = function() end"#),
        entity_descriptor_from_lua,
    )
    .unwrap_err()
    .to_string();
    assert!(
        lua.contains("components.behavior.activities.alerted.sound"),
        "{lua}"
    );
    assert!(lua.contains("must be a string"), "{lua}");
}

const FIXTURE_TS_SRC: &str = include_str!("../../../../../content/dev/scripts/positional-sound.ts");
const FIXTURE_LUAU_SRC: &str =
    include_str!("../../../../../content/dev/scripts/positional-sound.luau");

fn fixture_entity(export_name: &str) -> (EntityTypeDescriptor, EntityTypeDescriptor) {
    (
        super::behavior::shipped_reference_descriptor_from_typescript(
            FIXTURE_TS_SRC,
            export_name,
            "content/dev/scripts/positional-sound.ts",
        ),
        super::behavior::shipped_reference_descriptor_from_luau(
            FIXTURE_LUAU_SRC,
            export_name,
            "content/dev/scripts/positional-sound.luau",
        ),
    )
}

/// One export of the TypeScript fixture, bundled through `scripts-build` and
/// lowered to JSON.
fn fixture_ts_json(export_name: &str) -> serde_json::Value {
    let directory = std::env::temp_dir().join(format!(
        "postretro-positional-sound-{}-{export_name}",
        std::process::id(),
    ));
    std::fs::create_dir_all(&directory).expect("create fixture directory");
    let entry = directory.join("fixture.ts");
    std::fs::write(
        &entry,
        format!("{FIXTURE_TS_SRC}\nglobalThis.__fixtureExport = {export_name};"),
    )
    .expect("write fixture");
    let entry = std::fs::canonicalize(&entry).expect("canonicalize fixture");
    let bundled = postretro_script_compiler::bundle_entry(&entry).expect("the fixture bundles");
    let _ = std::fs::remove_dir_all(&directory);
    let registry = crate::primitives_registry::PrimitiveRegistry::new();
    let subsystem =
        crate::quickjs::QuickJsSubsystem::new(&registry, &crate::quickjs::QuickJsConfig::default())
            .expect("quickjs definition context");
    subsystem.definition_ctx().with(|ctx| {
        let _: JsValue = crate::quickjs::run_script(&ctx, &bundled, "positional-sound.ts")
            .expect("the fixture evaluates");
        let value: JsValue = crate::quickjs::run_script(&ctx, "globalThis.__fixtureExport", "read")
            .expect("the fixture exported the value");
        conv::js_to_json(&ctx, value).expect("the export lowers to JSON")
    })
}

/// One export of the Luau fixture, evaluated with the engine prelude and lowered to JSON.
fn fixture_luau_json(export_name: &str) -> serde_json::Value {
    let lua = crate::luau::build_lua_state(
        &[],
        None,
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))),
    )
    .expect("mod-rooted luau state");
    let source =
        format!("local M = (function()\n{FIXTURE_LUAU_SRC}\nend)()\nreturn M.{export_name}");
    let value: LuaValue = lua
        .load(&source)
        .set_name("positional-sound.luau")
        .eval()
        .expect("the fixture evaluates");
    conv::lua_to_json(value).expect("the export lowers to JSON")
}

// Row 27: the scripting surface installs as a dev fixture in both runtimes,
// under its own canonical names, and every sound key it names ships.
#[test]
fn the_positional_sound_fixture_is_identical_in_both_authorings() {
    for export in ["positionalSoundShotgunEntity", "positionalSoundGruntEntity"] {
        let (ts, luau) = fixture_entity(export);
        assert_eq!(ts, luau, "`{export}` differs between TS and Luau");
    }
    let (shotgun, _) = fixture_entity("positionalSoundShotgunEntity");
    assert_eq!(
        shotgun.canonical_name.as_deref(),
        Some("positional_sound_shotgun")
    );
    let (grunt, _) = fixture_entity("positionalSoundGruntEntity");
    assert_eq!(
        grunt.canonical_name.as_deref(),
        Some("positional_sound_grunt")
    );

    for export in [
        "positionalSoundReactions",
        "positionalSoundMovementSounds",
        "positionalSoundAttenuation",
    ] {
        assert_eq!(
            fixture_ts_json(export),
            fixture_luau_json(export),
            "`{export}` differs between TS and Luau",
        );
    }
    let reactions = fixture_ts_json("positionalSoundReactions");
    let door = reactions
        .as_array()
        .and_then(|reactions| {
            reactions
                .iter()
                .find(|reaction| reaction["name"] == "door.open")
        })
        .expect("the fixture defines door.open");
    assert_eq!(door["primitive"], "playSound");
    assert_eq!(
        door["args"],
        serde_json::json!({ "sound": "fixtures/door_open", "at": "@emitter" }),
        "`at: on.emitter` lowers to the emitter token",
    );
}

#[test]
fn every_sound_key_the_positional_sound_fixture_names_ships_in_content_dev() {
    let (shotgun, _) = fixture_entity("positionalSoundShotgunEntity");
    let (grunt, _) = fixture_entity("positionalSoundGruntEntity");
    let mut keys: Vec<String> = shotgun
        .weapon
        .unwrap()
        .sounds
        .unwrap()
        .keys()
        .map(|(_, key)| key.to_string())
        .collect();
    let graph = grunt.behavior.unwrap();
    keys.extend(
        graph
            .attacks
            .values()
            .filter_map(|attack| attack.sound.clone()),
    );
    keys.extend(
        graph
            .envelope
            .activities
            .values()
            .filter_map(|activity| activity.sound.clone()),
    );
    let movement = fixture_ts_json("positionalSoundMovementSounds");
    keys.extend(
        movement
            .as_object()
            .unwrap()
            .values()
            .map(|key| key.as_str().unwrap().to_string()),
    );
    let reactions = fixture_ts_json("positionalSoundReactions");
    keys.extend(
        reactions
            .as_array()
            .unwrap()
            .iter()
            .map(|reaction| reaction["args"]["sound"].as_str().unwrap().to_string()),
    );
    assert_eq!(keys.len(), 12, "the fixture names twelve sounds: {keys:?}");

    let sounds_root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev/sounds");
    for key in keys {
        let ships = ["wav", "ogg"]
            .iter()
            .any(|extension| sounds_root.join(format!("{key}.{extension}")).is_file());
        assert!(
            ships,
            "sound key `{key}` has no file under content/dev/sounds"
        );
    }
}

/// `playSound` evaluated through the engine's QuickJS definition context, which
/// carries the SDK prelude. `Err` holds the thrown message.
fn play_sound_ts(call: &str) -> Result<serde_json::Value, String> {
    let registry = crate::primitives_registry::PrimitiveRegistry::new();
    let subsystem =
        crate::quickjs::QuickJsSubsystem::new(&registry, &crate::quickjs::QuickJsConfig::default())
            .expect("quickjs definition context");
    subsystem.definition_ctx().with(|ctx| {
        let value: JsValue = crate::quickjs::run_script(&ctx, call, "play-sound.js")
            .map_err(|error| error.to_string())?;
        Ok(conv::js_to_json(&ctx, value).expect("the body lowers to JSON"))
    })
}

/// `playSound` evaluated through the engine's Luau state and its
/// `postretro/ui` module. `Err` holds the raised message.
fn play_sound_luau(call: &str) -> Result<serde_json::Value, String> {
    let lua = crate::luau::build_lua_state(
        &[],
        None,
        Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))),
    )
    .expect("mod-rooted luau state");
    let source = format!("local UI = require(\"postretro/ui\")\nreturn {call}");
    let value: LuaValue = lua
        .load(&source)
        .set_name("play-sound.luau")
        .eval()
        .map_err(|error| error.to_string())?;
    Ok(conv::lua_to_json(value).expect("the body lowers to JSON"))
}

// Regression: a level PRL baked before `playSound`'s second argument became an
// options object embeds `playSound("sfx/test_tone", "sfx")`. The TypeScript
// lowering read `"sfx"?.at` — `String.prototype.at`, a function — as an
// authored `at` and emitted `at: "@invalid"`, so install dropped an unanchored
// reaction with a misleading emitter-token error. A non-object options value
// must fail at the call with a message naming the shape.
#[test]
fn play_sound_rejects_a_non_object_options_value_in_both_runtimes() {
    let unanchored = serde_json::json!({
        "primitive": "playSound",
        "args": { "sound": "sfx/test_tone", "bus": "sfx" },
    });
    assert_eq!(
        play_sound_ts(r#"playSound("sfx/test_tone", { bus: "sfx" })"#),
        Ok(unanchored.clone()),
        "an options object without `at` lowers no `at`",
    );
    assert_eq!(
        play_sound_luau(r#"UI.playSound("sfx/test_tone", { bus = "sfx" })"#),
        Ok(unanchored),
        "an options table without `at` lowers no `at`",
    );

    for bad in [r#""sfx""#, "7", "true", "[]"] {
        let ts = play_sound_ts(&format!(r#"playSound("sfx/test_tone", {bad})"#))
            .expect_err(&format!("TS `{bad}` options must throw"));
        assert!(
            ts.contains("playSound: options must be an object"),
            "TS `{bad}`: {ts}"
        );
    }
    for bad in [r#""sfx""#, "7", "true"] {
        let luau = play_sound_luau(&format!(r#"UI.playSound("sfx/test_tone", {bad})"#))
            .expect_err(&format!("Luau `{bad}` options must raise"));
        assert!(
            luau.contains("playSound: options must be a table"),
            "Luau `{bad}`: {luau}"
        );
    }
}
