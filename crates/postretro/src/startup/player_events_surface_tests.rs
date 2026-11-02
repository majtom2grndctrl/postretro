// The player-events level script — the per-player events surface example —
// run through the production script runtime in both languages: the shipped
// `content/dev/scripts/player-events.{ts,luau}` twins must emit identical wire
// data, parse to identical player events, and install and fire through the
// level-load pipeline.
// See: context/lib/scripting.md §12 (Player events)

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use postretro_entities::{
    Recipients, ScriptCtx, ScriptError, SlotValue, SystemReactionCommand, Transform,
};
use postretro_foundation::Seat;
use postretro_level_format::data_script::DataScriptSection;
use postretro_scripting_core::data_descriptors::{LevelManifest, PlayerEventEdge};
use postretro_scripting_core::primitive_adapters::StoreSchemaJson;
use postretro_scripting_core::primitives_registry::{ContextScope, PrimitiveRegistry};
use postretro_scripting_core::runtime::{ScriptRuntime, ScriptRuntimeConfig};
use postretro_sim::residual_drain::{ResidualRegistries, drain_frame_residuals};
use postretro_test_log_capture::{CapturedRecord, LogCapture};
use serde_json::{Value, json};

use crate::coop_fixture_tests::{assert_merged_player_event_bodies, initialize_dev_mod};
use crate::scripting::primitives::register_all;
use crate::startup::lifecycle::tests::{install_capture_world, level_world, script_ctx, test_app};

const XP: &str = "progression.xp";
const LEVEL: &str = "leveling.level";
const LAST_LEVEL_UP_XP: &str = "leveling.lastLevelUpXp";

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
    assert_eq!(ts_manifest.reactions.len(), 4);
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
            (PlayerEventEdge::Becomes, vec!["leveling.levelUp"]),
            (PlayerEventEdge::Becomes, vec!["heat.scald"]),
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
    // One body: the host's writes, then the presentation, as system steps.
    let level_up = reaction("leveling.levelUp")["sequence"]
        .as_array()
        .expect("the level-up is one sequence body");
    assert_eq!(
        level_up
            .iter()
            .map(|step| step["primitive"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "addSlot",
            "setState",
            "playSound",
            "flashScreen",
            "screenShake"
        ]
    );
    assert_eq!(
        level_up[0]["target"],
        json!("@player"),
        "on.player is a subject token"
    );
    assert_eq!(
        level_up[1]["args"]["value"],
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

/// Install one twin the way `App::install_level_payload` does on a host: the
/// production dev mod, whose stores declare `progression` and `leveling`; then
/// `install_world_cpu`, which runs the sequence-primitive check, Pass A, a fresh
/// `SystemReactionIrBindings` and Pass B; then the session's IR-binding and
/// player-event rebuilds. Seats 1 and 2 each hold a pawn before the table
/// binds. Returns the app and every record the install logged.
fn install_level(script: &str) -> (crate::App, Vec<CapturedRecord>) {
    let mut app = test_app();
    let ctx = script_ctx(&app);
    let mut primitives = PrimitiveRegistry::new();
    register_all(&mut primitives, ctx.clone());
    initialize_dev_mod(&mut app, &primitives);

    let path = dev_script(script);
    let source = if script.ends_with(".luau") {
        std::fs::read_to_string(&path).expect("Luau twin reads")
    } else {
        postretro_script_compiler::bundle_entry(&path).expect("player-events.ts bundles")
    };
    let mut world = level_world("player-events", 1);
    world.data_script = Some(DataScriptSection {
        compiled_bytes: source.into_bytes(),
        source_path: path.to_string_lossy().into_owned(),
    });

    let capture = LogCapture::start();
    install_capture_world(&mut app, &world);
    for seat in [Seat(1), Seat(2)] {
        let mut registry = ctx.registry.borrow_mut();
        let pawn = registry.spawn(Transform::default());
        registry.bind_pawn_seat(pawn, seat);
    }
    app.rebuild_active_system_reaction_bindings();
    app.rebuild_player_events(false);
    let records = capture.records();
    (app, records)
}

/// One host frame's player-event work, in `frame_loop` order: evaluation of
/// the settled tick and the fires' in-tick commands, then the frame-end
/// residual drain. Returns the presentation routed to a player's machine.
fn player_event_frame(app: &mut crate::App) -> Vec<(Recipients, SystemReactionCommand)> {
    let ctx = script_ctx(app);
    let mut residuals = Vec::new();
    app.player_events.run_tick(&ctx, &mut residuals);
    let session = app.session.as_ref().expect("test app session installed");
    drain_frame_residuals(
        &[],
        &app.trigger_bindings,
        &session.trigger_system,
        &residuals,
        &app.player_events,
        &session.scripting.scheduler,
        ResidualRegistries {
            sequence: &session.scripting.sequence_registry,
            reaction: &session.scripting.reaction_registry,
            system: &session.scripting.system_registry,
        },
        &ctx,
        &mut Vec::new(),
        |_, _| None,
    );
    ctx.system_commands.take_routed()
}

fn number(value: Option<&SlotValue>) -> Option<f32> {
    match value {
        Some(SlotValue::Number(value)) => Some(*value),
        _ => None,
    }
}

fn per_seat(ctx: &ScriptCtx, name: &str, seat: Seat) -> Option<f32> {
    let slots = ctx.slot_table.borrow();
    let record = slots
        .get(name)
        .unwrap_or_else(|| panic!("the dev mod declares `{name}`"));
    number(record.per_seat_value(seat))
}

fn global(ctx: &ScriptCtx, name: &str) -> Option<f32> {
    let slots = ctx.slot_table.borrow();
    let record = slots
        .get(name)
        .unwrap_or_else(|| panic!("the dev mod declares `{name}`"));
    number(record.value.as_ref())
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1e-5
}

// Regression: a sequence body carrying `on.player` passed the sim-side
// `World::install` but was dropped by the production install passes it skips.
// The shipped twins install through the level-load pipeline without a
// warning, keep both merged bodies whole, and the level-up fires once, for
// the crossing player only, with its fanfare on that player's machine.
#[test]
fn player_events_twins_install_through_level_load_and_level_up_only_the_crossing_player() {
    let mut installed = Vec::new();
    for script in ["player-events.ts", "player-events.luau"] {
        let (mut app, records) = install_level(script);
        // The dev mod's descriptors warn about hit-zone tags their models
        // lack when the mod's descriptors install; those name no reaction.
        let loud: Vec<_> = records
            .iter()
            .filter(|record| record.level <= log::Level::Warn)
            .filter(|record| !record.message.starts_with("[HitZones]"))
            .collect();
        assert!(
            loud.is_empty(),
            "{script}: the level install logs nothing above info: {loud:#?}"
        );
        let ctx = script_ctx(&app);
        let reactions = ctx.data_registry.borrow().reactions.clone();
        assert_merged_player_event_bodies(&reactions, script);
        installed.push(reactions);

        {
            let mut slots = ctx.slot_table.borrow_mut();
            let xp = slots
                .get_mut(XP)
                .expect("the dev mod declares progression.xp");
            xp.set_per_seat_value(Seat(1), SlotValue::Number(20.0));
            xp.set_per_seat_value(Seat(2), SlotValue::Number(140.0));
        }
        // Whatever install queued (a `levelLoad` fire) is not this frame's.
        ctx.system_commands.discard_all();

        let routed = player_event_frame(&mut app);
        let seat_two = Recipients::Seats(vec![Seat(2)]);
        assert_eq!(
            routed.iter().map(|(to, _)| to).collect::<Vec<_>>(),
            [&seat_two, &seat_two, &seat_two],
            "{script}: the fanfare, flash and shake route to the crossing player's seat: {routed:?}"
        );
        assert!(
            matches!(&routed[0].1, SystemReactionCommand::PlaySound { sound, .. } if sound == "sfx/test_tone"),
            "{script}: the fanfare plays first: {:?}",
            routed[0].1
        );
        assert!(
            matches!(
                &routed[1].1,
                SystemReactionCommand::FlashScreen { color, duration_ms, .. }
                    if color.iter().zip([1.0, 0.9, 0.3, 0.4]).all(|(a, b)| close(*a, b))
                        && close(*duration_ms, 300.0)
            ),
            "{script}: the gold flash is second: {:?}",
            routed[1].1
        );
        assert!(
            matches!(
                &routed[2].1,
                SystemReactionCommand::ScreenShake {
                    amplitude,
                    duration_ms,
                    frequency: Some(frequency),
                    ..
                } if close(*amplitude, 6.0) && close(*duration_ms, 350.0) && close(*frequency, 18.0)
            ),
            "{script}: the screen shake is third: {:?}",
            routed[2].1
        );
        assert_eq!(
            per_seat(&ctx, LEVEL, Seat(2)),
            Some(2.0),
            "{script}: on.player levels the crossing player"
        );
        assert_eq!(
            global(&ctx, LAST_LEVEL_UP_XP),
            Some(140.0),
            "{script}: the level-up records the crossing player's XP"
        );
        assert_eq!(
            per_seat(&ctx, LEVEL, Seat(1)),
            Some(1.0),
            "{script}: the other player keeps the declared default level"
        );

        let routed = player_event_frame(&mut app);
        assert!(
            routed.is_empty(),
            "{script}: the guard holds, so nothing re-fires: {routed:?}"
        );
        assert_eq!(
            per_seat(&ctx, LEVEL, Seat(2)),
            Some(2.0),
            "{script}: the milestone does not level twice"
        );
    }
    assert_eq!(
        installed[0], installed[1],
        "both twins install the same validated reaction set"
    );
}
