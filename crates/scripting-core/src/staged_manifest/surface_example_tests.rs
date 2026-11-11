// The E10 Scripting surface example (row S1) in both runtimes: the token
// module, the `defineMod` registration with its `arena` profile, the reference
// enemy's draws, the rifle trooper's strafe-burst-reposition graph, and the
// level script's `arena.start` reaction parse identically.
// See: context/lib/scripting.md §2 (Attack token declarations) · context/lib/entity_model.md §7c

use std::fs;

use postretro_foundation::attack_budget::TokenBudget;

use super::tests::temp_mod_root;
use super::*;
use crate::data_descriptors::{
    ReactionDescriptor, named_reaction_from_js, named_reaction_from_lua,
};
use crate::primitive_adapters::{JsonValue, WorldQueryFilterInput};
use crate::primitives_registry::{ContextScope, PrimitiveRegistry};
use crate::quickjs::QuickJsSubsystem;

const POOL_ID: u32 = 23;

const BUDGET_TS: &str = r#"
import { defineAttackToken } from "postretro";
// budget.ts — tokens named by their bindings; each object is the token's default budget
export const melee  = defineAttackToken({ limit: 3 });
export const ranged = defineAttackToken({ limit: 2 });
export const heavy  = defineAttackToken({ limit: 1, refreshMs: 4000 });
"#;

const START_SCRIPT_TS: &str = r#"
import { attackTurnFree, brain, defineMod } from "postretro";
import { heavy, melee, ranged } from "./budget";

export default defineMod({
  name: "Surface", id: "e10.surface", version: "1",
  attackTokens: [melee, ranged, heavy],
  attackBudgets: {
    arena: [melee.with({ limit: 1 }), ranged.with({ limit: 1 })],
  },
  entities: [
    { canonicalName: "reference_enemy", components: { behavior: {
      initial: "engage", moveSpeed: 3,
      attacks: {
        jab:  { draws: [melee], damage: 8, maxRange: 2, cooldownMs: 1200 },
        slam: { draws: [melee.with({ cost: 2 }), heavy], damage: 14, maxRange: 3.5, cooldownMs: 1800 },
      },
      activities: {
        engage: { animation: "walk", motion: "chaseTarget", action: { attack: "jab" } },
      },
      transitions: {},
    } } },
    { canonicalName: "rifle_trooper", components: { behavior: {
      initial: "engage", moveSpeed: 4,
      attacks: {
        shoot: { draws: [ranged], weapon: "enemy_rifle_burst", standoffDistance: 6 },
      },
      activities: {
        engage: {
          animation: "walk",
          layers: {
            offense: {
              initial: "wait",
              activities: {
                wait:  { animation: "walk", motion: "strafe" },
                burst: { animation: "idle_aiming", motion: "hold", action: { attack: "shoot" }, attackTurn: "shoot" },
                move:  { animation: "run", motion: "reposition", facing: "travel",
                         reposition: { arc: [60, 120] } },
              },
              transitions: {
                wait:  [{ to: "burst", when: attackTurnFree("shoot").and(brain.targetVisible) }],
                burst: [{ to: "move", when: brain.attacksFiredInActivity.ge(1).and(brain.timeInActivityMs.ge(900)) },
                        { to: "wait", when: brain.hasAttackTurn.not().and(brain.timeInActivityMs.ge(100)) }],
                move:  [{ to: "wait", when: brain.timeInActivityMs.ge(1500) }],
              },
            },
          },
        },
      },
      transitions: {},
    } } },
  ],
});
"#;

const BUDGET_LUAU: &str = r#"
return {
  melee = defineAttackToken("melee", { limit = 3 }),
  ranged = defineAttackToken("ranged", { limit = 2 }),
  heavy = defineAttackToken("heavy", { limit = 1, refreshMs = 4000 }),
}
"#;

const START_SCRIPT_LUAU: &str = r#"
local Budget = require("./budget.luau")
local melee, ranged, heavy = Budget.melee, Budget.ranged, Budget.heavy

return defineMod({
  name = "Surface", id = "e10.surface", version = "1",
  attackTokens = { melee, ranged, heavy },
  attackBudgets = {
    arena = { melee:with({ limit = 1 }), ranged:with({ limit = 1 }) },
  },
  entities = {
    { canonicalName = "reference_enemy", components = { behavior = {
      initial = "engage", moveSpeed = 3,
      attacks = {
        jab = { draws = { melee }, damage = 8, maxRange = 2, cooldownMs = 1200 },
        slam = { draws = { melee:with({ cost = 2 }), heavy }, damage = 14, maxRange = 3.5, cooldownMs = 1800 },
      },
      activities = {
        engage = { animation = "walk", motion = "chaseTarget", action = { attack = "jab" } },
      },
      transitions = {},
    } } },
    { canonicalName = "rifle_trooper", components = { behavior = {
      initial = "engage", moveSpeed = 4,
      attacks = {
        shoot = { draws = { ranged }, weapon = "enemy_rifle_burst", standoffDistance = 6 },
      },
      activities = {
        engage = {
          animation = "walk",
          layers = {
            offense = {
              initial = "wait",
              activities = {
                wait = { animation = "walk", motion = "strafe" },
                burst = { animation = "idle_aiming", motion = "hold", action = { attack = "shoot" }, attackTurn = "shoot" },
                move = { animation = "run", motion = "reposition", facing = "travel",
                         reposition = { arc = { 60, 120 } } },
              },
              transitions = {
                wait = { { to = "burst", when = attackTurnFree("shoot")["and"](attackTurnFree("shoot"), brain.targetVisible) } },
                burst = {
                  { to = "move", when = brain.attacksFiredInActivity:ge(1)["and"](brain.attacksFiredInActivity:ge(1), brain.timeInActivityMs:ge(900)) },
                  { to = "wait", when = brain.hasAttackTurn["not"](brain.hasAttackTurn)["and"](brain.hasAttackTurn["not"](brain.hasAttackTurn), brain.timeInActivityMs:ge(100)) },
                },
                move = { { to = "wait", when = brain.timeInActivityMs:ge(1500) } },
              },
            },
          },
        },
      },
      transitions = {},
    } } },
  },
})
"#;

// level script (setupLevel) — the reaction a trigger or script fires.
const ARENA_START_TS: &str = r#"
import { defineReaction, getMapEntities } from "postretro";
(globalThis as any).__arenaStart = defineReaction("arena.start", {
  sequence: getMapEntities("attackPool", { tag: "arena" }).flatMap((p) => p.start()),
});
"#;

const ARENA_START_LUAU: &str = r#"
local steps = {}
for _, p in ipairs(getMapEntities("attackPool", { tag = "arena" })) do
  table.insert(steps, p:start())
end
return defineReaction("arena.start", { sequence = steps })
"#;

fn bundle_typescript(label: &str, modules: &[(&str, &str)]) -> String {
    let dir = temp_mod_root(&format!("surface_ts_{label}"));
    for (file, source) in modules {
        fs::write(dir.join(file), source).expect("TypeScript module writes");
    }
    let entry = fs::canonicalize(dir.join(modules[0].0)).expect("entry exists");
    postretro_script_compiler::bundle_entry(&entry).expect("the surface example bundles")
}

fn built(label: &str, files: &[(&str, &str)]) -> Box<transfer::StagedManifest> {
    let dir = temp_mod_root(&format!("surface_{label}"));
    for (file, source) in files {
        fs::write(dir.join(file), source).expect("fixture should be written");
    }
    let result = build_staged_manifest(&dir, 1, &StagedManifestBuildConfig::default());
    match result.status {
        StagedManifestBuildStatus::Built(manifest) => manifest,
        other => panic!("{label} must build: {other:?} {:?}", result.diagnostics),
    }
}

/// A registry whose `worldQuery` answers one attack pool tagged `arena`, so
/// `getMapEntities("attackPool")` wraps a real member handle.
fn arena_registry() -> PrimitiveRegistry {
    let mut registry = PrimitiveRegistry::new();
    registry
        .register(
            "worldQuery",
            |filter: WorldQueryFilterInput| -> Result<JsonValue, ScriptError> {
                Ok(JsonValue(match filter.component.as_str() {
                    "attack_pool" => serde_json::json!([{
                        "id": POOL_ID,
                        "position": { "x": 0, "y": 0, "z": 0 },
                        "tags": ["arena"],
                    }]),
                    _ => serde_json::json!([]),
                }))
            },
        )
        .scope(ContextScope::Both)
        .param("filter", "WorldQueryFilter")
        .finish();
    registry
}

// S1: the Scripting surface example parses identically in TypeScript and
// Luau: one registration of the three tokens and the `arena` profile, the same
// reference enemy and rifle trooper descriptors, and an `arena.start` that
// starts the one tagged pool.
#[test]
fn scripting_surface_example_parses_identically_in_both_runtimes() {
    let ts = built(
        "ts",
        &[(
            "start-script.js",
            &bundle_typescript(
                "manifest",
                &[
                    ("start-script.ts", START_SCRIPT_TS),
                    ("budget.ts", BUDGET_TS),
                ],
            ),
        )],
    );
    let luau = built(
        "luau",
        &[
            ("start-script.luau", START_SCRIPT_LUAU),
            ("budget.luau", BUDGET_LUAU),
        ],
    );
    assert_eq!(ts.attack_budgets, luau.attack_budgets);
    assert_eq!(ts.entities, luau.entities);

    let tokens = &ts.attack_budgets;
    let declared: Vec<TokenBudget> = tokens.declared_budgets().collect();
    let budget = |name: &str| declared[usize::from(tokens.token_index(name).unwrap())];
    assert_eq!(
        budget("melee"),
        TokenBudget {
            limit: 3,
            refresh_ms: 0.0
        }
    );
    assert_eq!(
        budget("ranged"),
        TokenBudget {
            limit: 2,
            refresh_ms: 0.0
        }
    );
    assert_eq!(
        budget("heavy"),
        TokenBudget {
            limit: 1,
            refresh_ms: 4000.0
        }
    );
    let arena = tokens
        .profile_budgets("arena")
        .expect("the `arena` profile");
    let melee = usize::from(tokens.token_index("melee").unwrap());
    let ranged = usize::from(tokens.token_index("ranged").unwrap());
    let heavy = usize::from(tokens.token_index("heavy").unwrap());
    assert_eq!((arena[melee].limit, arena[ranged].limit), (1, 1));
    assert_eq!(
        arena[heavy], declared[heavy],
        "a token the profile leaves out"
    );

    let registry = arena_registry();
    let bundled = bundle_typescript("reaction", &[("arena.ts", ARENA_START_TS)]);
    let subsystem = QuickJsSubsystem::new(&registry, &QuickJsConfig::default()).unwrap();
    let ts_reaction = subsystem.definition_ctx().with(|ctx| {
        run_script::<()>(&ctx, &bundled, "arena.js").expect("the level script evaluates");
        let value: rquickjs::Value = ctx.globals().get("__arenaStart").unwrap();
        named_reaction_from_js(&ctx, value).expect("arena.start parses")
    });
    let primitives: Vec<_> = registry.iter().cloned().collect();
    let lua = crate::luau::build_lua_state(&primitives, None, None).expect("luau state");
    let value: mlua::Value = lua
        .load(ARENA_START_LUAU)
        .set_name("arena.luau")
        .eval()
        .expect("the level script evaluates");
    let luau_reaction = named_reaction_from_lua(value).expect("arena.start parses");
    assert_eq!(ts_reaction, luau_reaction);
    assert_eq!(ts_reaction.name, "arena.start");
    let ReactionDescriptor::Sequence(steps) = &ts_reaction.descriptor else {
        panic!("arena.start is a sequence: {:?}", ts_reaction.descriptor);
    };
    assert_eq!(
        steps.len(),
        1,
        "one step for the one tagged pool: {steps:?}"
    );
    let step = format!("{:?}", steps[0]);
    assert!(
        step.contains("startAttackPool") && step.contains(&POOL_ID.to_string()),
        "arena.start starts the tagged pool: {step}"
    );
}
