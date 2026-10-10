// Tests: JS/Luau `playerEvents` parsing for level scripts and the mod manifest.

use super::super::*;
use super::common::*;
use crate::ir::IrNode;
use postretro_entities::data_descriptors::{PlayerEventDescriptor, PlayerEventEdge};
use postretro_test_log_capture::LogCapture;

fn health_below(threshold: f32) -> IrNode {
    serde_json::from_value(serde_json::json!({
        "op": "lt",
        "a": { "op": "input", "name": "player.health" },
        "b": { "op": "const", "value": threshold },
    }))
    .unwrap()
}

// Regression guard for both runtimes agreeing: one shared JSON parser.
#[test]
fn level_player_events_parse_identically_and_reject_a_levels_entry_naming_the_script() {
    let capture = LogCapture::start();
    let js = eval_js(
        r#"({ playerEvents: [
            { edge: "becomes",
              condition: { op: "lt", a: { op: "input", name: "player.health" }, b: { op: "const", value: 25 } },
              fire: ["bleed"] },
            { edge: "ceases",
              condition: { op: "lt", a: { op: "input", name: "player.health" }, b: { op: "const", value: 25 } },
              fire: ["mend"], levels: ["arena"] },
            { edge: "changes", condition: { op: "const", value: true }, fire: ["bad"] },
            { edge: "ceases",
              condition: { op: "lt", a: { op: "input", name: "player.health" }, b: { op: "const", value: 10 } },
              fire: [] }
        ] })"#,
        |ctx, value| LevelManifest::from_js_value(ctx, value).unwrap(),
    );
    let lua = eval_lua(
        r#"return { playerEvents = {
            { edge = "becomes",
              condition = { op = "lt", a = { op = "input", name = "player.health" }, b = { op = "const", value = 25 } },
              fire = { "bleed" } },
            { edge = "ceases",
              condition = { op = "lt", a = { op = "input", name = "player.health" }, b = { op = "const", value = 25 } },
              fire = { "mend" }, levels = { "arena" } },
            { edge = "changes", condition = { op = "const", value = true }, fire = { "bad" } },
            { edge = "ceases",
              condition = { op = "lt", a = { op = "input", name = "player.health" }, b = { op = "const", value = 10 } },
              fire = {} }
        } }"#,
        |value| LevelManifest::from_lua_value(value).unwrap(),
    );

    assert_eq!(js.player_events, lua.player_events);
    assert_eq!(
        js.player_events,
        vec![
            PlayerEventDescriptor {
                edge: PlayerEventEdge::Becomes,
                condition: health_below(25.0),
                fire: vec!["bleed".into()],
                levels: Vec::new(),
            },
            PlayerEventDescriptor {
                edge: PlayerEventEdge::Ceases,
                condition: health_below(10.0),
                fire: Vec::new(),
                levels: Vec::new(),
            },
        ]
    );
    let messages: Vec<String> = capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level == log::Level::Warn
                && record.message.contains("level script `setupLevel`")
                && record.message.contains("playerEvents[1] carries `levels`")
        })
        .map(|record| record.message)
        .collect();
    assert_eq!(messages.len(), 2, "one rejection per runtime: {messages:?}");
    assert_eq!(
        messages[0], messages[1],
        "the runtimes' diagnostics diverged"
    );
    let unknown_edge = capture
        .records()
        .into_iter()
        .filter(|record| record.message.contains("playerEvents[2] is malformed"))
        .count();
    assert_eq!(
        unknown_edge, 2,
        "an unknown edge word skips that entry in each runtime"
    );
}

#[test]
fn mod_player_events_keep_their_levels_scope_in_both_runtimes() {
    let js = eval_js(
        r#"({ playerEvents: [
            { edge: "becomes",
              condition: { op: "lt", a: { op: "input", name: "player.health" }, b: { op: "const", value: 25 } },
              fire: ["bleed"], levels: ["arena", "campaign"] }
        ] })"#,
        |ctx, value| {
            let obj = rquickjs::Object::from_value(value).unwrap();
            drain_player_events_js(ctx, &obj, false, "mod manifest").unwrap()
        },
    );
    let lua = eval_lua(
        r#"return { playerEvents = {
            { edge = "becomes",
              condition = { op = "lt", a = { op = "input", name = "player.health" }, b = { op = "const", value = 25 } },
              fire = { "bleed" }, levels = { "arena", "campaign" } }
        } }"#,
        |value| {
            let LuaValue::Table(table) = value else {
                panic!("manifest is a table");
            };
            drain_player_events_lua(&table, false, "mod manifest").unwrap()
        },
    );
    assert_eq!(js, lua);
    assert_eq!(js.len(), 1);
    assert_eq!(
        js[0].levels,
        vec!["arena".to_string(), "campaign".to_string()]
    );
}
