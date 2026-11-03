// The assist pair handles in both SDK runtimes: owner-token lowering at every
// `byPlayer` site and the `assistable` expression profile shape.
// See: context/lib/scripting.md §11 (interaction-pair scope), §12 (Assist)
use super::*;

/// `byPlayer(assist.helper)` lowers to `@assist.helper` at every recognizing
/// site in both runtimes: the engine-leaf installer (`getGameState()`), the
/// store `storeRef`, and the existing owners still lower; an unknown handle
/// lowers to `@invalid`.
#[test]
fn by_player_assist_handles_lower_to_their_tokens_at_every_site_in_both_runtimes() {
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { assist, defineImpactEvent, defineStore, getGameState } from "postretro";
        const stats = defineStore("stats", {
          heal: { type: "number", default: 0, perOwner: true, network: "ownerPrivate" },
        });
        const health = getGameState().player.health;
        let source: unknown = undefined;
        defineImpactEvent("probe", { tag: "enemy" }, (impact) => {
          source = { leaf: health.byPlayer(impact.source).owner, store: stats.heal.byPlayer(impact.source).owner };
          return [];
        });
        const bogus = Object.freeze({ __wire: "@bogus" }) as any;
        JSON.stringify({
          leafHelper: health.byPlayer(assist.helper).owner,
          leafTarget: health.byPlayer(assist.target).owner,
          storeHelper: stats.heal.byPlayer(assist.helper).owner,
          storeTarget: stats.heal.byPlayer(assist.target).owner,
          source,
          leafBogus: health.byPlayer(bogus).owner,
          storeBogus: stats.heal.byPlayer(bogus).owner,
          helperFrozen: Object.isFrozen(assist.helper),
          helperKeys: Object.keys(assist.helper),
        });
    "#;
    const LUAU_FIXTURE: &str = r#"
        local Postretro = require("postretro")
        local assist = Postretro.assist
        local stats = Postretro.defineStore("stats", {
          heal = { type = "number", default = 0, perOwner = true, network = "ownerPrivate" },
        })
        local health = Postretro.getGameState().player.health
        local source = nil
        Postretro.defineImpactEvent("probe", { tag = "enemy" }, function(impact)
          source = { leaf = health:byPlayer(impact.source).owner, store = stats.heal:byPlayer(impact.source).owner }
          return {}
        end)
        local bogus = table.freeze({ __wire = "@bogus" })
        return {
          leafHelper = health:byPlayer(assist.helper).owner,
          leafTarget = health:byPlayer(assist.target).owner,
          storeHelper = stats.heal:byPlayer(assist.helper).owner,
          storeTarget = stats.heal:byPlayer(assist.target).owner,
          source = source,
          leafBogus = health:byPlayer(bogus).owner,
          storeBogus = stats.heal:byPlayer(bogus).owner,
          helperFrozen = table.isfrozen(assist.helper),
          helperKeys = {},
        }
    "#;
    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    for (runtime, value) in [("TypeScript", &typescript), ("Luau", &luau)] {
        assert_eq!(value["leafHelper"], "@assist.helper", "{runtime}: {value}");
        assert_eq!(value["leafTarget"], "@assist.target", "{runtime}: {value}");
        assert_eq!(value["storeHelper"], "@assist.helper", "{runtime}: {value}");
        assert_eq!(value["storeTarget"], "@assist.target", "{runtime}: {value}");
        assert_eq!(
            value["source"],
            serde_json::json!({ "leaf": "@impact.source", "store": "@impact.source" }),
            "{runtime}: {value}"
        );
        assert_eq!(value["leafBogus"], "@invalid", "{runtime}: {value}");
        assert_eq!(value["storeBogus"], "@invalid", "{runtime}: {value}");
        assert_eq!(value["helperFrozen"], true, "{runtime}: {value}");
    }
    // `__wire` stays non-enumerable in TypeScript: the handle serializes empty.
    assert_eq!(typescript["helperKeys"], serde_json::json!(["inLifeState"]));
}

/// A TS and a Luau `assistable` expression profile, authored through the SDK
/// `assist` handles and lowered by `defineEntity`, carry one shape: the
/// healer recipe (per-owner `heal` on the helper, `downMedic` on the target).
#[test]
fn assistable_expression_profile_lowers_identically_across_runtimes() {
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { assist, defineEntity, defineStore, read } from "postretro";
        const stats = defineStore("stats", {
          heal: { type: "number", default: 0, perOwner: true, network: "ownerPrivate" },
        });
        const player = defineEntity({
          canonicalName: "player",
          components: {
            lifeStates: {
              initial: "alive",
              states: {
                alive: {},
                literal: { assistable: { radius: 2, holdMs: 5000 } },
                defaults: { assistable: {} },
                down: {
                  assistable: {
                    holdMs: read(stats.heal.byPlayer(assist.helper)).times(-20).plus(3000),
                    radius: assist.target.inLifeState("downMedic").select(3, 1.5),
                  },
                },
              },
            },
          },
        } as any);
        JSON.stringify((player as any).components.lifeStates.states);
    "#;
    const LUAU_FIXTURE: &str = r#"
        local Postretro = require("postretro")
        local assist = Postretro.assist
        local stats = Postretro.defineStore("stats", {
          heal = { type = "number", default = 0, perOwner = true, network = "ownerPrivate" },
        })
        local helperHeal = Postretro.read(stats.heal:byPlayer(assist.helper))
        local player = Postretro.defineEntity({
          canonicalName = "player",
          components = {
            lifeStates = {
              initial = "alive",
              states = {
                alive = {},
                literal = { assistable = { radius = 2, holdMs = 5000 } },
                defaults = { assistable = {} },
                down = {
                  assistable = {
                    holdMs = helperHeal:times(-20):plus(3000),
                    radius = assist.target:inLifeState("downMedic"):select(3, 1.5),
                  },
                },
              },
            },
          },
        })
        return player.components.lifeStates.states
    "#;
    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    // Luau serializes an empty table as an array; compare the meaningful rows.
    for state in ["literal", "down"] {
        assert_eq!(
            serde_json::to_string(&typescript[state]).unwrap(),
            serde_json::to_string(&luau[state]).unwrap(),
            "`{state}` must lower byte-identically"
        );
    }
    let down = &typescript["down"]["assistable"];
    assert_eq!(
        down["radius"],
        serde_json::json!({
            "op": "select",
            "cond": { "op": "input", "owner": "@assist.target", "name": "player.inLifeState:downMedic" },
            "a": { "op": "const", "value": 3 },
            "b": { "op": "const", "value": 1.5 },
        })
    );
    assert_eq!(
        down["holdMs"]["a"]["a"],
        serde_json::json!({ "op": "input", "name": "stats.heal", "owner": "@assist.helper" }),
        "{down}"
    );
    assert_eq!(
        typescript["literal"]["assistable"],
        serde_json::json!({ "radius": 2, "holdMs": 5000 })
    );
}
