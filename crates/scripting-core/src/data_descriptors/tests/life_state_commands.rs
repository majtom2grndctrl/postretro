// Both descriptor converters enforce the player-only life-state command surface.
use super::super::*;
use super::common::*;

fn parse_pair(js: &str, lua: &str) -> (NamedReaction, NamedReaction) {
    (
        eval_js(js, |ctx, value| named_reaction_from_js(ctx, value).unwrap()),
        eval_lua(lua, |value| named_reaction_from_lua(value).unwrap()),
    )
}

#[test]
fn life_state_command_bodies_and_steps_accept_player_targets_in_both_vms() {
    for (js, lua) in [
        (
            r#"({ name: "down", primitive: "setLifeState", target: "@player", args: { state: "down" } })"#,
            r#"return { name = "down", primitive = "setLifeState", target = "@player", args = { state = "down" } }"#,
        ),
        (
            r#"({ name: "respawn", primitive: "respawn", kind: "player", args: {} })"#,
            r#"return { name = "respawn", primitive = "respawn", kind = "player", args = {} }"#,
        ),
        (
            r#"({ name: "restore", sequence: [{ kind: "player", primitive: "respawn", args: { health: -1, state: "alive" } }] })"#,
            r#"return { name = "restore", sequence = { { kind = "player", primitive = "respawn", args = { health = -1, state = "alive" } } } }"#,
        ),
    ] {
        let (js, lua) = parse_pair(js, lua);
        assert_eq!(js, lua);
    }
}

#[test]
fn life_state_commands_reject_npc_kinds_and_bad_payloads_identically_in_both_vms() {
    for (js, lua, expected) in [
        (
            r#"({ name: "bad", primitive: "setLifeState", kind: "npc", args: { state: "down" } })"#,
            r#"return { name = "bad", primitive = "setLifeState", kind = "npc", args = { state = "down" } }"#,
            "targets players",
        ),
        (
            r#"({ name: "bad", sequence: [{ kind: "npc", primitive: "respawn", args: {} }] })"#,
            r#"return { name = "bad", sequence = { { kind = "npc", primitive = "respawn", args = {} } } }"#,
            "targets players",
        ),
        (
            r#"({ name: "bad", primitive: "setLifeState", kind: "player", args: { state: "bad state" } })"#,
            r#"return { name = "bad", primitive = "setLifeState", kind = "player", args = { state = "bad state" } }"#,
            "args.state",
        ),
        (
            r#"({ name: "bad", sequence: [{ target: "@player", primitive: "respawn", args: { health: "full" } }] })"#,
            r#"return { name = "bad", sequence = { { target = "@player", primitive = "respawn", args = { health = "full" } } } }"#,
            "args.health",
        ),
    ] {
        let js_error = eval_js(js, |ctx, value| {
            named_reaction_from_js(ctx, value).unwrap_err()
        })
        .to_string();
        let lua_error =
            eval_lua(lua, |value| named_reaction_from_lua(value).unwrap_err()).to_string();
        assert_eq!(js_error, lua_error);
        assert!(
            js_error.contains("reaction `bad`") && js_error.contains(expected),
            "{js_error}"
        );
    }
}
