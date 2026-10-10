// The player-events level script — the per-player events surface example —
// run through the production script runtime in both languages: the shipped
// `content/dev/scripts/player-events.{ts,luau}` twins must emit identical wire
// data and parse to identical player events.
// See: context/lib/scripting.md §12 (Player events)

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use postretro_entities::{ScriptCtx, ScriptError};
use postretro_level_format::data_script::DataScriptSection;
use postretro_scripting_core::data_descriptors::{LevelManifest, PlayerEventEdge};
use postretro_scripting_core::primitive_adapters::StoreSchemaJson;
use postretro_scripting_core::primitives_registry::{ContextScope, PrimitiveRegistry};
use postretro_scripting_core::runtime::{ScriptRuntime, ScriptRuntimeConfig};
use serde_json::{Value, json};

use crate::scripting::primitives::register_all;

/// Wraps `setupLevel` so the raw value it returns — before Rust-side parsing —
/// reaches `__playerEventsCapture`.
const JS_CAPTURE: &str = "\n;(function () {\n  const original = globalThis.setupLevel;\n  globalThis.setupLevel = function (ctx) {\n    const result = original(ctx);\n    __playerEventsCapture(result);\n    return result;\n  };\n})();\n";
const LUAU_CAPTURE: &str = "\ndo\n  local original = setupLevel\n  setupLevel = function(ctx)\n    local result = original(ctx)\n    __playerEventsCapture(result)\n    return result\n  end\nend\n";

fn dev_script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/scripts")
        .join(name)
}

/// Keys sorted; values untouched, so an explicit `null` shows as divergence.
fn canonical(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonical(value)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonical).collect()),
        other => other,
    }
}

/// Run one twin's `setupLevel` the way level load does; return the parsed
/// manifest and the raw returned value.
fn run(
    runtime: &ScriptRuntime,
    captured: &RefCell<Option<Value>>,
    script: &str,
) -> (LevelManifest, Value) {
    let path = dev_script(script);
    let is_luau = path.extension().is_some_and(|ext| ext == "luau");
    let mut source = if is_luau {
        std::fs::read_to_string(&path).expect("Luau twin reads")
    } else {
        postretro_script_compiler::bundle_entry(&path).expect("player-events.ts bundles")
    };
    source.push_str(if is_luau { LUAU_CAPTURE } else { JS_CAPTURE });
    let manifest = runtime.run_data_script(
        &DataScriptSection {
            compiled_bytes: source.into_bytes(),
            source_path: path.to_string_lossy().into_owned(),
        },
        path.parent().unwrap(),
    );
    let raw = captured
        .borrow_mut()
        .take()
        .unwrap_or_else(|| panic!("{script}: setupLevel never returned"));
    (manifest, canonical(raw))
}

#[test]
fn player_events_twins_emit_byte_identical_wire_data() {
    let ctx = ScriptCtx::new();
    let captured: Rc<RefCell<Option<Value>>> = Rc::new(RefCell::new(None));
    let mut primitives = PrimitiveRegistry::new();
    register_all(&mut primitives, ctx.clone());
    primitives
        .register("__playerEventsCapture", {
            let captured = captured.clone();
            move |value: StoreSchemaJson| -> Result<(), ScriptError> {
                *captured.borrow_mut() = Some(value.0);
                Ok(())
            }
        })
        .scope(ContextScope::Both)
        .doc("test capture of the raw setupLevel return")
        .param("value", "unknown")
        .finish();
    let runtime = ScriptRuntime::new(&primitives, &ScriptRuntimeConfig::default(), &ctx)
        .expect("script runtime constructs");

    let (ts_manifest, ts_wire) = run(&runtime, &captured, "player-events.ts");
    let (luau_manifest, luau_wire) = run(&runtime, &captured, "player-events.luau");
    assert_eq!(
        serde_json::to_string(&ts_wire).unwrap(),
        serde_json::to_string(&luau_wire).unwrap(),
        "TS and Luau wire diverged"
    );
    assert_eq!(ts_manifest.reactions, luau_manifest.reactions);
    assert_eq!(ts_manifest.player_events, luau_manifest.player_events);
    assert_eq!(ts_manifest.crossings, luau_manifest.crossings);
    assert_eq!(ts_manifest.reactions.len(), 8);
    assert_eq!(
        ts_manifest.crossings.len(),
        1,
        "the low-health vignette stays a local crossing"
    );

    let edges: Vec<(PlayerEventEdge, Vec<&str>)> = ts_manifest
        .player_events
        .iter()
        .map(|event| (event.edge, event.fire.iter().map(String::as_str).collect()))
        .collect();
    assert_eq!(
        edges,
        vec![
            (
                PlayerEventEdge::Becomes,
                vec![
                    "leveling.levelUp",
                    "leveling.recordLevelUp",
                    "leveling.fanfare",
                    "leveling.goldFlash"
                ]
            ),
            (
                PlayerEventEdge::Becomes,
                vec!["heat.scald", "heat.scaldHiss"]
            ),
            (PlayerEventEdge::Ceases, vec!["heat.cooled"]),
        ]
    );
    assert!(
        ts_manifest
            .player_events
            .iter()
            .all(|event| event.levels.is_empty()),
        "a level script's entries carry no `levels`"
    );

    // The wire the builders emit: `on.player` targets, a fluent `updateState`
    // value over `byPlayer(on.player)`, and a guarded Bool condition.
    let reaction = |name: &str| -> &Value {
        ts_wire["reactions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|reaction| reaction["name"] == name)
            .unwrap_or_else(|| panic!("missing reaction {name}"))
    };
    assert_eq!(
        reaction("leveling.levelUp")["target"],
        json!("@player"),
        "on.player is a subject token"
    );
    assert_eq!(
        reaction("leveling.recordLevelUp")["args"]["value"],
        json!({ "op": "input", "name": "progression.xp", "owner": "@player" }),
        "updateState lowers the fluent read to IR"
    );
    assert_eq!(
        ts_wire["playerEvents"][0]["condition"],
        canonical(json!({
            "op": "select",
            "cond": { "op": "ge", "a": { "op": "input", "name": "progression.xp" }, "b": { "op": "const", "value": 100 } },
            "a": { "op": "lt", "a": { "op": "input", "name": "leveling.level" }, "b": { "op": "const", "value": 2 } },
            "b": { "op": "const", "value": false },
        })),
        "a plain per-player read in the condition carries no owner"
    );
}
