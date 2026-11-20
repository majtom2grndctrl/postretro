// E25 level-seam Scripting surface example in both runtimes: the brief's
// TypeScript `setupLevel` and its Luau mirror bundle, evaluate against map
// members, and parse into identical level manifests.
// See: context/lib/entity_model.md §7e (Level Seams) · context/lib/scripting.md §12

use std::fs;

use postretro_entities::EntityId;

use super::tests::temp_mod_root;
use super::*;
use crate::data_descriptors::{LevelManifest, ReactionDescriptor, SequenceTarget};
use crate::primitive_adapters::{JsonValue, WorldQueryFilterInput};
use crate::primitives_registry::{ContextScope, PrimitiveRegistry};
use crate::quickjs::QuickJsSubsystem;

const AIRLOCK_A: u32 = 40;
const AIRLOCK_B: u32 = 41;
const BUTTON: u32 = 50;

// The brief's example, verbatim, plus one line that publishes what
// `setupLevel` returns for the test to parse.
const SEAM_SURFACE_TS: &str = r#"
import { defineReaction, getMapEntities } from "postretro";
import type { SeamEventParams } from "postretro";

export function setupLevel() {
  const airlock = getMapEntities("seam", { tag: "airlock" });
  const cycle = defineReaction("airlock.cycle", { sequence: airlock.flatMap((s) => s.arm()) });
  const sealed = defineReaction("airlock.sealed", (on: SeamEventParams) => on.seam.disarm());
  return {
    reactions: [cycle, sealed],
    triggerEvents: getMapEntities("trigger", { tag: "airlock_button" }).map((t) => t.on("enter", [cycle])),
    seamEvents: airlock.map((s) => s.on("arrive", [sealed])),
  };
}

(globalThis as any).__seamManifest = setupLevel();
"#;

// The Luau mirror: `s:arm()`, `s:disarm()`, `s:on("arrive", { reaction })`,
// `on.seam:disarm()`, `seamEvents` under `setupLevel`.
const SEAM_SURFACE_LUAU: &str = r#"
local function setupLevel()
  local airlock = getMapEntities("seam", { tag = "airlock" })
  local steps = {}
  for _, s in ipairs(airlock) do
    for _, step in ipairs(s:arm()) do
      table.insert(steps, step)
    end
  end
  local cycle = defineReaction("airlock.cycle", { sequence = steps })
  local sealed = defineReaction("airlock.sealed", function(on) return on.seam:disarm() end)
  local triggerEvents = {}
  for i, t in ipairs(getMapEntities("trigger", { tag = "airlock_button" })) do
    triggerEvents[i] = t:on("enter", { cycle })
  end
  local seamEvents = {}
  for i, s in ipairs(airlock) do
    seamEvents[i] = s:on("arrive", { sealed })
  end
  return {
    reactions = { cycle, sealed },
    triggerEvents = triggerEvents,
    seamEvents = seamEvents,
  }
end

return setupLevel()
"#;

fn snapshot(id: u32, tag: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "position": { "x": 0, "y": 0, "z": 0 },
        "tags": [tag],
    })
}

/// A registry whose `worldQuery` answers two seams tagged `airlock` and one
/// trigger tagged `airlock_button`, so `getMapEntities` wraps real handles.
fn airlock_registry() -> PrimitiveRegistry {
    let mut registry = PrimitiveRegistry::new();
    registry
        .register(
            "worldQuery",
            |filter: WorldQueryFilterInput| -> Result<JsonValue, ScriptError> {
                Ok(JsonValue(match filter.component.as_str() {
                    "seam" => serde_json::json!([
                        snapshot(AIRLOCK_A, "airlock"),
                        snapshot(AIRLOCK_B, "airlock"),
                    ]),
                    "trigger_volume" => serde_json::json!([snapshot(BUTTON, "airlock_button")]),
                    _ => serde_json::json!([]),
                }))
            },
        )
        .scope(ContextScope::Both)
        .param("filter", "WorldQueryFilter")
        .finish();
    registry
}

fn bundle_typescript(source: &str) -> String {
    let dir = temp_mod_root("seam_surface_ts");
    fs::write(dir.join("airlock.ts"), source).expect("TypeScript module writes");
    let entry = fs::canonicalize(dir.join("airlock.ts")).expect("entry exists");
    postretro_script_compiler::bundle_entry(&entry).expect("the seam surface example bundles")
}

#[test]
fn seam_scripting_surface_example_parses_identically_in_both_runtimes() {
    let registry = airlock_registry();
    let bundled = bundle_typescript(SEAM_SURFACE_TS);
    let subsystem = QuickJsSubsystem::new(&registry, &QuickJsConfig::default()).unwrap();
    let ts = subsystem.definition_ctx().with(|ctx| {
        run_script::<()>(&ctx, &bundled, "airlock.js").expect("the level script evaluates");
        let value: rquickjs::Value = ctx.globals().get("__seamManifest").unwrap();
        LevelManifest::from_js_value(&ctx, value).expect("the level manifest parses")
    });
    let primitives: Vec<_> = registry.iter().cloned().collect();
    let lua = crate::luau::build_lua_state(&primitives, None, None).expect("luau state");
    let value: mlua::Value = lua
        .load(SEAM_SURFACE_LUAU)
        .set_name("airlock.luau")
        .eval()
        .expect("the level script evaluates");
    let luau = LevelManifest::from_lua_value(value).expect("the level manifest parses");

    assert_eq!(ts, luau, "TypeScript and Luau seam manifests diverged");

    let names: Vec<_> = ts.reactions.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["airlock.cycle", "airlock.sealed"]);
    let ReactionDescriptor::Sequence(steps) = &ts.reactions[0].descriptor else {
        panic!(
            "airlock.cycle is a sequence: {:?}",
            ts.reactions[0].descriptor
        );
    };
    assert_eq!(steps.len(), 2, "one arm step per airlock seam: {steps:?}");
    for (step, id) in steps.iter().zip([AIRLOCK_A, AIRLOCK_B]) {
        assert_eq!(step.id, SequenceTarget::Entity(EntityId::from_raw(id)));
        assert_eq!(step.primitive, "armSeam");
    }
    let ReactionDescriptor::Primitive(sealed) = &ts.reactions[1].descriptor else {
        panic!(
            "airlock.sealed is a token command: {:?}",
            ts.reactions[1].descriptor
        );
    };
    assert_eq!(sealed.primitive, "disarmSeam");
    assert_eq!(sealed.target.as_deref(), Some("@seam"));

    assert_eq!(ts.trigger_events.len(), 1);
    assert_eq!(ts.trigger_events[0].trigger, EntityId::from_raw(BUTTON));
    assert_eq!(ts.trigger_events[0].event, "enter");
    assert_eq!(ts.trigger_events[0].fire, ["airlock.cycle"]);

    let seams: Vec<_> = ts.seam_events.iter().map(|event| event.seam).collect();
    assert_eq!(
        seams,
        [EntityId::from_raw(AIRLOCK_A), EntityId::from_raw(AIRLOCK_B)]
    );
    for event in &ts.seam_events {
        assert_eq!(event.event, "arrive");
        assert_eq!(event.fire, ["airlock.sealed"]);
    }
    assert!(ts.teleport_events.is_empty());
}
