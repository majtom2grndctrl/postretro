// Entity addressing SDK surface in both runtimes: group opacity, the verbs
// each target carries, and the wire each lowers to.
// See: context/lib/scripting.md §12 (Entity addressing)
use super::*;

/// Run a Luau chunk whose body is expected to raise; return the error text.
fn luau_error(body: &str) -> String {
    let source = format!(
        r#"
        local Postretro = require("postretro")
        local ok, err = pcall(function()
            {body}
        end)
        assert(not ok, "the chunk must raise")
        return tostring(err)
        "#
    );
    let value = luau_fixture_value(&source);
    value.as_str().expect("error text").to_string()
}

// S3 (Luau half): a group is opaque. `#group`, `group[1]`, `group.length` and
// a verb its kind lacks each raise naming the call that built the group; none
// reads as 0 or nil.
#[test]
fn luau_groups_raise_naming_the_call_on_length_index_and_missing_verbs() {
    for (body, call, what) in [
        (
            "return #Postretro.npcs({ tag = \"boss\" })",
            "npcs({ tag = \"boss\" })",
            "has no length",
        ),
        ("return Postretro.npcs()[1]", "npcs()", "cannot be indexed"),
        ("return Postretro.players().length", "players()", "`length`"),
        ("return Postretro.npcs():fire()", "npcs()", "`fire`"),
        (
            "return Postretro.players():update({ aggro = true })",
            "players()",
            "`update`",
        ),
        (
            "return npcs({ tag = \"closet\" }).members",
            "npcs({ tag = \"closet\" })",
            "`members`",
        ),
    ] {
        let error = luau_error(body);
        assert!(
            error.contains(call) && error.contains(what),
            "`{body}` must raise naming `{call}` ({what}): {error}"
        );
    }
}

// S3 (Luau half): the retired free verbs and group constructors are gone, so a
// call raises instead of emitting a tag-keyed descriptor.
#[test]
fn luau_retired_free_verbs_and_world_are_absent() {
    let value = luau_fixture_value(
        r#"
        local Postretro = require("postretro")
        return {
          moduleDamage = Postretro.damage == nil,
          globalDamage = damage == nil,
          enemies = enemies == nil and Postretro.enemies == nil,
          spawner = spawner == nil and Postretro.spawner == nil,
          armTrigger = armTrigger == nil and disarmTrigger == nil,
          grants = grantHealth == nil and grantAmmo == nil and addSlot == nil,
          onTriggerEvent = onTriggerEvent == nil,
          world = world == nil and Postretro.world == nil,
        }
        "#,
    );
    for (name, absent) in value.as_object().expect("table") {
        assert_eq!(absent, &serde_json::json!(true), "`{name}` must be retired");
    }
}

// M3 (runtime half): an unknown map kind raises naming the call in both
// runtimes before any engine query runs.
#[test]
fn get_map_entities_rejects_an_unknown_kind_naming_the_call_in_both_runtimes() {
    for kind in ["npc", "transform"] {
        let ts = quickjs_fixture_value(&format!(
            r#"
            import {{ getMapEntities }} from "postretro";
            let message = "no-throw";
            try {{ getMapEntities("{kind}" as never); }} catch (e) {{ message = String(e); }}
            JSON.stringify(message);
            "#
        ));
        let luau = luau_error(&format!("return Postretro.getMapEntities(\"{kind}\")"));
        for (runtime, error) in [("ts", ts.as_str().unwrap().to_string()), ("luau", luau)] {
            assert!(
                error.contains("getMapEntities") && error.contains(kind),
                "{runtime}: `getMapEntities(\"{kind}\")` must raise naming the call: {error}"
            );
        }
    }
}

// G3 (SDK half), T8 (SDK half): `players()` emits kind-bearing group commands
// for every player; `on.activators` and `on.trigger` lower to the existing
// sentinel wire for only that fire's subjects. TS and Luau agree byte for byte,
// Luau through colon calls.
#[test]
fn group_and_subject_token_commands_lower_to_their_wire_in_both_runtimes() {
    let ts = quickjs_fixture_value(
        r#"
        import { defineReaction, players, type TriggerEventParams } from "postretro";
        const everyPlayer = defineReaction("everyPlayer", players().grantHealth(25));
        const thisFire = defineReaction("thisFire", (on: TriggerEventParams) => on.activators.grantHealth(25));
        const hurt = defineReaction("hurt", (on: TriggerEventParams) => on.activators.damage(5));
        const ammo = defineReaction("ammo", (on: TriggerEventParams) => on.activators.grantAmmo("shells.buck", 8));
        const disarm = defineReaction("disarm", (on: TriggerEventParams) => ({ sequence: on.trigger.disarm() }));
        const rearm = defineReaction("rearm", (on: TriggerEventParams) => ({ sequence: on.trigger.arm() }));
        JSON.stringify([everyPlayer, thisFire, hurt, ammo, disarm, rearm]);
        "#,
    );
    let luau = luau_fixture_value(
        r#"
        local Postretro = require("postretro")
        return {
          Postretro.defineReaction("everyPlayer", Postretro.players():grantHealth(25)),
          Postretro.defineReaction("thisFire", function(on) return on.activators:grantHealth(25) end),
          Postretro.defineReaction("hurt", function(on) return on.activators:damage(5) end),
          Postretro.defineReaction("ammo", function(on) return on.activators:grantAmmo("shells.buck", 8) end),
          Postretro.defineReaction("disarm", function(on) return { sequence = on.trigger:disarm() } end),
          Postretro.defineReaction("rearm", function(on) return { sequence = on.trigger:arm() } end),
        }
        "#,
    );
    assert_eq!(
        serde_json::to_vec(&ts).unwrap(),
        serde_json::to_vec(&luau).unwrap(),
        "TS and Luau addressing wire diverged"
    );
    assert_eq!(
        ts,
        serde_json::json!([
            { "name": "everyPlayer", "primitive": "grantHealth", "kind": "player", "args": { "amount": 25 } },
            { "name": "thisFire", "primitive": "grantHealth", "target": "@activators", "args": { "amount": 25 } },
            { "name": "hurt", "primitive": "applyDamage", "target": "@activators", "args": { "amount": 5 } },
            { "name": "ammo", "primitive": "grantAmmo", "target": "@activators", "args": { "type": "shells.buck", "amount": 8 } },
            { "name": "disarm", "sequence": [{ "id": "@trigger", "primitive": "disarmTrigger", "args": {} }] },
            { "name": "rearm", "sequence": [{ "id": "@trigger", "primitive": "armTrigger", "args": {} }] },
        ])
    );
}

// A group command is one descriptor legal both as a reaction body and,
// unspread, as a sequence entry. W1 (wire half): no SDK builder emits an entry
// carrying both `id` and `kind`.
#[test]
fn group_commands_are_sequence_entries_and_never_carry_an_id() {
    let ts = quickjs_fixture_value(
        r#"
        import { defineReaction, npcs, players, wait } from "postretro";
        const closet = npcs({ tag: "closet" });
        JSON.stringify(defineReaction("reveal", {
          sequence: [...wait(800, { interruptible: true }), closet.update({ aggro: true }), closet.damage(5), players().grantAmmo("shells.buck", 8)],
        }));
        "#,
    );
    let luau = luau_fixture_value(
        r#"
        local Postretro = require("postretro")
        local closet = Postretro.npcs({ tag = "closet" })
        local steps = Postretro.wait(800, { interruptible = true })
        table.insert(steps, closet:update({ aggro = true }))
        table.insert(steps, closet:damage(5))
        table.insert(steps, Postretro.players():grantAmmo("shells.buck", 8))
        return Postretro.defineReaction("reveal", { sequence = steps })
        "#,
    );
    assert_eq!(ts, luau, "TS and Luau group sequences diverged");
    let steps = ts["sequence"].as_array().expect("sequence");
    assert_eq!(steps.len(), 4);
    for step in &steps[1..] {
        assert!(
            step.get("kind").is_some(),
            "group step carries `kind`: {step}"
        );
        assert!(
            step.get("id").is_none(),
            "a group step never carries `id`: {step}"
        );
        assert!(
            step.get("target").is_none(),
            "a group step never carries `target`: {step}"
        );
    }
    assert_eq!(steps[1]["tag"], "closet");
    assert!(steps[3].get("tag").is_none(), "`players()` takes no tag");
}

// `defineTriggerEvent` lowers reaction handles to names and keeps the
// tag-keyed mod-global wire, `levels` included, in both runtimes.
#[test]
fn define_trigger_event_builds_the_tag_keyed_wire_in_both_runtimes() {
    let ts = quickjs_fixture_value(
        r#"
        import { defineReaction, defineTriggerEvent } from "postretro";
        const beat = defineReaction("story.beat", { primitive: "playSound", args: { sound: "beat" } });
        JSON.stringify([
          defineTriggerEvent({ tag: "story", event: "enter", fire: [beat, "story.choice"] }),
          defineTriggerEvent({ tag: "story", event: "exit", fire: [beat], levels: ["campaign"] }),
        ]);
        "#,
    );
    let luau = luau_fixture_value(
        r#"
        local beat = defineReaction("story.beat", { primitive = "playSound", args = { sound = "beat" } })
        return {
          defineTriggerEvent({ tag = "story", event = "enter", fire = { beat, "story.choice" } }),
          defineTriggerEvent({ tag = "story", event = "exit", fire = { beat }, levels = { "campaign" } }),
        }
        "#,
    );
    assert_eq!(ts, luau, "TS and Luau mod-global trigger events diverged");
    assert_eq!(
        ts,
        serde_json::json!([
            { "tag": "story", "event": "enter", "fire": ["story.beat", "story.choice"] },
            { "tag": "story", "event": "exit", "fire": ["story.beat"], "levels": ["campaign"] },
        ])
    );
}
