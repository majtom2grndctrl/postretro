// The impact source's life-state read and effect in both SDK runtimes: the
// Borderlands second-wind row lowers to one wire.
// See: context/lib/scripting.md §2 (Impact-policy evaluation), §12 (Player life states)
use super::*;

#[test]
fn impact_source_life_state_builders_lower_identically_across_runtimes() {
    const TYPESCRIPT_FIXTURE: &str = r#"
        import { defineImpactEvent } from "postretro";
        const secondWind = defineImpactEvent("second-wind", { tag: "enemy" }, (impact) => {
          const { target, source } = impact;
          const killed = target.healthBefore.gt(0).and(target.healthAfter.le(0));
          return [{
            when: killed.and(source.inLifeState("down")),
            do: [source.setLifeState("alive"), source.grantHealth(40)],
          }];
        });
        JSON.stringify((secondWind as unknown as { policy: unknown[] }).policy);
    "#;
    const LUAU_FIXTURE: &str = r#"
        local Postretro = require("postretro")
        local secondWind = Postretro.defineImpactEvent("second-wind", { tag = "enemy" }, function(impact)
          local target, source = impact.target, impact.source
          local alive = target.healthBefore:gt(0)
          local killed = alive["and"](alive, target.healthAfter:le(0))
          return {
            {
              when = killed["and"](killed, source:inLifeState("down")),
              ["do"] = { source:setLifeState("alive"), source:grantHealth(40) },
            },
          }
        end)
        return secondWind.policy
    "#;

    let typescript = quickjs_fixture_value(TYPESCRIPT_FIXTURE);
    let luau = luau_fixture_value(LUAU_FIXTURE);
    assert_eq!(
        serde_json::to_vec(&typescript).expect("serialize TypeScript wire"),
        serde_json::to_vec(&luau).expect("serialize Luau wire"),
        "the source life-state builders must lower byte-identically"
    );

    let group = &typescript[0];
    assert_eq!(
        group["when"]["a"],
        serde_json::json!({
            "op": "input",
            "name": "player.inLifeState:down",
            "owner": "@impact.source",
        }),
        "`and` lowers to select(killed, inLifeState, false): {group}"
    );
    assert_eq!(
        group["do"][0],
        serde_json::json!({
            "primitive": "setLifeState",
            "target": "@impact.source",
            "args": { "state": "alive" },
        })
    );
}
