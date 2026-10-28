//! Parser-twin proof for authored life-state profiles and diagnostics.

use serde_json::{Value, json};

use super::super::*;
use super::common::{JS_PLAYER_MOVEMENT, eval_js, eval_lua};

fn lua_value(value: &Value) -> String {
    match value {
        Value::Object(fields) => format!(
            "{{ {} }}",
            fields
                .iter()
                .map(|(key, value)| {
                    format!(
                        "[{}] = {}",
                        serde_json::to_string(key).unwrap(),
                        lua_value(value)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Null => "nil".to_owned(),
        value => serde_json::to_string(value).unwrap(),
    }
}

fn parse(
    js_block: &str,
    lua_block: &str,
    movement: bool,
) -> (
    Result<EntityTypeDescriptor, DescriptorError>,
    Result<EntityTypeDescriptor, DescriptorError>,
) {
    let js = format!(
        "(() => {{ const entity = {JS_PLAYER_MOVEMENT}; entity.components.lifeStates = {js_block}; {} return entity; }})()",
        if movement {
            ""
        } else {
            "delete entity.components.movement;"
        }
    );
    let lua_base = JS_PLAYER_MOVEMENT
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .replace(':', "=");
    let lua = format!(
        "local entity = {lua_base}\nentity.components.lifeStates = {lua_block}\n{}\nreturn entity",
        if movement {
            ""
        } else {
            "entity.components.movement = nil"
        }
    );
    (
        eval_js(&js, entity_descriptor_from_js),
        eval_lua(&lua, entity_descriptor_from_lua),
    )
}

#[test]
fn life_state_parser_twins_lower_defaults_and_optional_rejoin() {
    for rejoin in [false, true] {
        let mut block = json!({"initial":"alive", "states":{"alive":{}, "down":{"fire":false,"movement":{"speedScale":0.3,"abilities":false}}}});
        if rejoin {
            block["rejoin"] = json!({"down":"alive"});
        }
        let (js, lua) = parse(&block.to_string(), &lua_value(&block), true);
        let js = js.unwrap().life_states.unwrap();
        assert_eq!(js, lua.unwrap().life_states.unwrap());
        assert_eq!(
            js.state("alive").unwrap().1,
            &postretro_foundation::LifeStateProfile::default()
        );
        assert!(!js.state("down").unwrap().1.fire);
        assert_eq!(js.rejoin.get("down").is_some(), rejoin);
    }
}

#[test]
fn life_state_parser_twins_accept_forced_crouch_and_each_closed_capability() {
    let block = json!({"initial":"down", "states":{"down":{
        "movement":{"speedScale":0,"abilities":false,"posture":"crouch"},
        "look":false,"fire":false,"switchWeapon":false,"reload":false,
        "use":false,"touch":false,"occupancy":false,"damageable":false
    }}});
    let movement = JS_PLAYER_MOVEMENT.replace(
        "fall: { terminalVelocity: 40.0 }",
        "fall: { terminalVelocity: 40.0 }, crouch: { halfHeight: 0.4, eyeHeight: 0.3, transitionRate: 8.0 }",
    );
    let js = format!(
        "(() => {{ const entity = {movement}; entity.components.lifeStates = {block}; return entity; }})()"
    );
    let lua_base = movement
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .replace(':', "=");
    let lua = format!(
        "local entity = {lua_base}\nentity.components.lifeStates = {}\nreturn entity",
        lua_value(&block)
    );
    let js = eval_js(&js, entity_descriptor_from_js).unwrap();
    let lua = eval_lua(&lua, entity_descriptor_from_lua).unwrap();
    assert_eq!(js.life_states, lua.life_states);
    let block = js.life_states.unwrap();
    let profile = block.initial_state().1;
    assert_eq!(
        profile.movement.posture,
        postretro_foundation::LifeStatePosture::Crouch
    );
    assert!(!profile.use_enabled && !profile.switch_weapon && !profile.damageable);
    assert_eq!(profile.movement.speed_scale, 0.0);
}

#[test]
fn life_state_parser_twins_reject_each_invalid_contract_with_its_full_path() {
    for (block, path, movement) in [
        (
            json!({"initial":"missing","states":{"alive":{}}}),
            "components.lifeStates.initial",
            true,
        ),
        (
            json!({"initial":"bad name","states":{"bad name":{}}}),
            "components.lifeStates.states.bad name",
            true,
        ),
        (
            json!({"initial":"alive","states":{"alive":{"ghost":false}}}),
            "components.lifeStates.states.alive.ghost",
            true,
        ),
        (
            json!({"initial":"alive","states":{"alive":{"movement":{"speedScale":1.1}}}}),
            "components.lifeStates.states.alive.movement.speedScale",
            true,
        ),
        (
            json!({"initial":"alive","states":{"alive":{"movement":{"speedScale":-0.1}}}}),
            "components.lifeStates.states.alive.movement.speedScale",
            true,
        ),
        (
            json!({"initial":"alive","states":{"alive":{"movement":{"speedScale":"slow"}}}}),
            "components.lifeStates.states.alive.movement.speedScale",
            true,
        ),
        (
            json!({"initial":"alive","states":{"alive":{"movement":{"posture":"prone"}}}}),
            "components.lifeStates.states.alive.movement.posture",
            true,
        ),
        (
            json!({"initial":"alive","states":{"alive":{"movement":{"posture":"crouch"}}}}),
            "components.lifeStates.states.alive.movement.posture",
            true,
        ),
        (
            json!({"initial":"alive","states":{"alive":{"fire":0}}}),
            "components.lifeStates.states.alive.fire",
            true,
        ),
        (
            json!({"initial":"alive","states":{"alive":{}},"rejoin":{"gone":"alive"}}),
            "components.lifeStates.rejoin.gone",
            true,
        ),
        (
            json!({"initial":"alive","states":{"alive":{}},"rejoin":{"alive":"gone"}}),
            "components.lifeStates.rejoin.alive",
            true,
        ),
        (
            json!({"initial":"alive","states":{"alive":{}}}),
            "components.lifeStates",
            false,
        ),
    ] {
        let (js, lua) = parse(&block.to_string(), &lua_value(&block), movement);
        let js = js.unwrap_err();
        assert_eq!(js, lua.unwrap_err(), "{block}");
        assert!(js.to_string().contains(path), "{js}");
    }
}

#[test]
fn life_state_parser_twins_reject_nonfinite_numbers_before_json_conversion() {
    for (js_number, lua_number) in [
        ("NaN", "0/0"),
        ("Infinity", "math.huge"),
        ("-Infinity", "-math.huge"),
    ] {
        let js =
            format!("{{initial:'alive',states:{{alive:{{movement:{{speedScale:{js_number}}}}}}}}}");
        let lua = format!(
            "{{initial='alive',states={{alive={{movement={{speedScale={lua_number}}}}}}}}}"
        );
        let (js, lua) = parse(&js, &lua, true);
        let js = js.unwrap_err();
        assert_eq!(js, lua.unwrap_err());
        assert!(
            js.to_string()
                .contains("components.lifeStates.states.alive.movement.speedScale")
        );
    }
}
