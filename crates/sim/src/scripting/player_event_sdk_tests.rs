// Player events through the SDK: what `players().on`, `on.player`,
// `byPlayer(on.player)` and fluent `updateState` emit in TypeScript and Luau,
// installed and fired at the sim seam.
// See: context/lib/scripting.md §12 (Player events)

use std::path::{Path, PathBuf};

use postretro_entities::reactions::system_commands::SystemReactionCommand;
use postretro_entities::{
    ReplicationScope, ScriptCtx, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
};
use postretro_foundation::Seat;
use postretro_level_format::data_script::DataScriptSection;
use postretro_scripting_core::data_descriptors::LevelManifest;
use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use postretro_scripting_core::runtime::{ScriptRuntime, ScriptRuntimeConfig};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_test_log_capture::LogCapture;

use crate::player_events::tests::World;
use crate::residual_drain::{ResidualRegistries, drain_frame_residuals};
use crate::scripting::reactions::system_commands::register_system_reaction_primitives;
use crate::scripting_systems::reaction_scheduler::ReactionScheduler;
use crate::trigger_system::TriggerSystem;

const XP: &str = "progression.xp";
const LEVEL: &str = "leveling.level";
const LAST_LEVEL_UP_XP: &str = "leveling.lastLevelUpXp";

fn dev_script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/scripts")
        .join(name)
}

/// Run a level script's `setupLevel` the way level load does. A `.ts` entry is
/// bundled through the `scripts-build` library first.
fn run_level_script(entry: &Path) -> LevelManifest {
    let is_luau = entry.extension().is_some_and(|ext| ext == "luau");
    let source = if is_luau {
        std::fs::read_to_string(entry).expect("Luau level script reads")
    } else {
        postretro_script_compiler::bundle_entry(entry).expect("TS level script bundles")
    };
    let runtime = ScriptRuntime::new(
        &PrimitiveRegistry::new(),
        &ScriptRuntimeConfig::default(),
        &ScriptCtx::new(),
    )
    .expect("script runtime constructs");
    runtime.run_data_script(
        &DataScriptSection {
            compiled_bytes: source.into_bytes(),
            source_path: entry.to_string_lossy().into_owned(),
        },
        entry.parent().unwrap(),
    )
}

/// Write `source` to a temp level script named `file` and run it.
fn run_inline(file: &str, source: &str) -> LevelManifest {
    let dir = tempfile::tempdir().expect("script dir");
    let entry = dir.path().join(file);
    std::fs::write(&entry, source).expect("level script writes");
    run_level_script(&entry)
}

fn declare(world: &World, name: &str, default: f32, network: ReplicationScope, per_owner: bool) {
    world
        .script_ctx
        .slot_table
        .borrow_mut()
        .insert(
            name.to_string(),
            SlotRecord::new(SlotSchema {
                slot_type: SlotType::Number,
                default: Some(SlotValue::Number(default)),
                range: None,
                persist: false,
                readonly: false,
                ownership: SlotOwnership::Mod,
                network,
                per_owner,
                accumulate: None,
            }),
        )
        .unwrap();
}

fn number(value: Option<&SlotValue>) -> Option<f32> {
    match value {
        Some(SlotValue::Number(value)) => Some(*value),
        _ => None,
    }
}

fn global(world: &World, name: &str) -> f32 {
    number(
        world
            .script_ctx
            .slot_table
            .borrow()
            .get(name)
            .unwrap()
            .value
            .as_ref(),
    )
    .unwrap_or_else(|| panic!("{name} holds no number"))
}

fn per_seat(world: &World, name: &str, seat: Seat) -> Option<f32> {
    number(
        world
            .script_ctx
            .slot_table
            .borrow()
            .get(name)
            .unwrap()
            .per_seat_value(seat),
    )
}

fn set_per_seat(world: &World, name: &str, seat: Seat, value: f32) {
    world
        .script_ctx
        .slot_table
        .borrow_mut()
        .get_mut(name)
        .unwrap()
        .set_per_seat_value(seat, SlotValue::Number(value));
}

/// A world with two seat-bound players and the example's store slots. The
/// hand-written declarations mirror the dev mod's stores: `progression.xp`
/// (`content/dev/scripts/combat-lifecycle.ts`) and `leveling.level` /
/// `leveling.lastLevelUpXp` (`content/dev/scripts/leveling.ts`). Each slot's
/// `per_owner` and `network` follow those modules: xp and level are per-owner
/// `ownerPrivate`, `lastLevelUpXp` is one `shared` value. Keep them in step.
fn example_world() -> World {
    let world = World::new();
    world.spawn_player(Some(Seat(1)), 100.0);
    world.spawn_player(Some(Seat(2)), 100.0);
    declare(&world, XP, 0.0, ReplicationScope::OwnerPrivatePlayer, true);
    declare(
        &world,
        LEVEL,
        1.0,
        ReplicationScope::OwnerPrivatePlayer,
        true,
    );
    declare(
        &world,
        LAST_LEVEL_UP_XP,
        0.0,
        ReplicationScope::SharedGlobal,
        false,
    );
    world
}

// AC: `updateState` with a `read(…)` over `byPlayer(on.player)` writes the
// event player's value, not the host's, in both runtimes. The shipped
// player-events example is the script under test.
#[test]
fn update_state_reading_by_player_on_player_writes_the_event_players_value_in_both_runtimes() {
    for script in ["player-events.ts", "player-events.luau"] {
        let manifest = run_level_script(&dev_script(script));
        assert_eq!(
            manifest.player_events.len(),
            3,
            "{script}: every player event parses"
        );
        let mut world = example_world();
        let capture = LogCapture::start();
        world.install(manifest.reactions, manifest.player_events);
        let noisy: Vec<_> = capture
            .records()
            .into_iter()
            .filter(|record| record.level <= log::Level::Warn)
            .collect();
        assert!(
            noisy.is_empty(),
            "{script}: installing the shipped example logs no warning or error: {noisy:?}"
        );
        drop(capture);
        set_per_seat(&world, XP, Seat(1), 20.0);
        set_per_seat(&world, XP, Seat(2), 140.0);
        world.tick();
        // The fanfare and gold flash present on the crossing player's machine,
        // in listed order.
        let mut system = SystemReactionRegistry::new();
        register_system_reaction_primitives(&mut system);
        let sequence = SequencedPrimitiveRegistry::new();
        let reaction = ReactionPrimitiveRegistry::new();
        let mut follow_ups = Vec::new();
        drain_frame_residuals(
            &[],
            &Default::default(),
            &TriggerSystem::default(),
            &world.residuals,
            &world.table,
            &ReactionScheduler::default(),
            ResidualRegistries {
                sequence: &sequence,
                reaction: &reaction,
                system: &system,
            },
            &world.script_ctx,
            &mut follow_ups,
        );
        let routed = world.script_ctx.system_commands.take_routed();
        assert_eq!(routed.len(), 2, "{script}: fanfare and goldFlash route");
        assert_eq!(routed[0].0, Seat(2), "{script}: fanfare seat");
        assert!(
            matches!(&routed[0].1, SystemReactionCommand::PlaySound { sound, .. } if sound == "sfx/test_tone"),
            "{script}: fanfare first: {:?}",
            routed[0].1
        );
        assert_eq!(routed[1].0, Seat(2), "{script}: goldFlash seat");
        assert!(
            matches!(
                &routed[1].1,
                SystemReactionCommand::FlashScreen { color, duration_ms }
                    if *color == [1.0, 0.9, 0.3, 0.4] && *duration_ms == 300.0
            ),
            "{script}: goldFlash second: {:?}",
            routed[1].1
        );
        assert_eq!(
            global(&world, LAST_LEVEL_UP_XP),
            140.0,
            "{script}: recordLevelUp writes the crossing player's XP"
        );
        assert_eq!(
            per_seat(&world, LEVEL, Seat(2)),
            Some(2.0),
            "{script}: on.player levels the crosser"
        );
        assert_ne!(
            per_seat(&world, LEVEL, Seat(1)),
            Some(2.0),
            "{script}: the other player is untouched"
        );
        world.tick();
        assert_eq!(
            per_seat(&world, LEVEL, Seat(2)),
            Some(2.0),
            "{script}: the guard holds the milestone"
        );
    }
}

// AC: the same write reaches engine per-player refs through their generated
// `byPlayer`, and a literal value writes as before.
#[test]
fn update_state_reads_an_engine_slot_by_player_and_writes_literals_unchanged_in_both_runtimes() {
    const TS: &str = r#"
import { players, becomes, read, defineReaction, defineStore, getGameState } from "postretro";
import type { PlayerEventParams } from "postretro";
import { updateState } from "postretro/ui";
const s = defineStore("leveling", { lastLevelUpXp: { type: "number", default: 0, network: "shared" } });
const hp = getGameState().player.health;
const record = defineReaction("record", (on: PlayerEventParams) =>
  updateState(s.lastLevelUpXp, read(hp.byPlayer(on.player)).plus(1)));
const literal = defineReaction("literal", updateState(s.lastLevelUpXp, 7));
export function setupLevel() {
  return {
    reactions: [record, literal],
    playerEvents: [
      players().on(becomes(read(hp).lt(50)), [record]),
      players().on(becomes(read(hp).lt(10)), [literal]),
    ],
  };
}
"#;
    const LUAU: &str = r#"
local Postretro = require("postretro")
local UI = require("postretro/ui")
local s = Postretro.defineStore("leveling", { lastLevelUpXp = { type = "number", default = 0, network = "shared" } })
local hp = Postretro.getGameState().player.health
local record = Postretro.defineReaction("record", function(on)
  return UI.updateState(s.lastLevelUpXp, Postretro.read(hp:byPlayer(on.player)):plus(1))
end)
local literal = Postretro.defineReaction("literal", UI.updateState(s.lastLevelUpXp, 7))
function setupLevel(_ctx)
  return {
    reactions = { record, literal },
    playerEvents = {
      Postretro.players():on(Postretro.becomes(Postretro.read(hp):lt(50)), { record }),
      Postretro.players():on(Postretro.becomes(Postretro.read(hp):lt(10)), { literal }),
    },
  }
end
"#;
    for (file, source) in [("level.ts", TS), ("level.luau", LUAU)] {
        let manifest = run_inline(file, source);
        assert_eq!(manifest.player_events.len(), 2, "{file}");
        let mut world = World::new();
        world.spawn_player(Some(Seat(1)), 90.0);
        let remote = world.spawn_player(Some(Seat(2)), 90.0);
        declare(
            &world,
            LAST_LEVEL_UP_XP,
            0.0,
            ReplicationScope::SharedGlobal,
            false,
        );
        // The host's own HUD projection says 90; the event player is at 30.
        world
            .script_ctx
            .slot_table
            .borrow_mut()
            .get_mut("player.health")
            .unwrap()
            .write_value(Some(SlotValue::Number(90.0)));
        world.install(manifest.reactions, manifest.player_events);
        world.set_health(remote, 30.0);
        world.tick();
        assert_eq!(
            global(&world, LAST_LEVEL_UP_XP),
            31.0,
            "{file}: reads the event player, not the host"
        );
        world.set_health(remote, 5.0);
        world.tick();
        assert_eq!(
            global(&world, LAST_LEVEL_UP_XP),
            7.0,
            "{file}: a literal writes as before"
        );
    }
}

// AC: a `players().on` descriptor built but not returned under `playerEvents`
// registers nothing.
#[test]
fn an_unreturned_players_on_descriptor_registers_nothing_in_both_runtimes() {
    const TS: &str = r#"
import { players, becomes, read, defineReaction, getGameState } from "postretro";
const hp = getGameState().player.health;
const hurt = defineReaction("hurt", players().damage(1));
const built = players().on(becomes(read(hp).lt(50)), [hurt]);
export function setupLevel() {
  const alsoBuilt = players().on(becomes(read(hp).lt(25)), [hurt]);
  return { reactions: [hurt] };
}
"#;
    const LUAU: &str = r#"
local Postretro = require("postretro")
local hp = Postretro.getGameState().player.health
local hurt = Postretro.defineReaction("hurt", Postretro.players():damage(1))
local built = Postretro.players():on(Postretro.becomes(Postretro.read(hp):lt(50)), { hurt })
function setupLevel(_ctx)
  local alsoBuilt = Postretro.players():on(Postretro.becomes(Postretro.read(hp):lt(25)), { hurt })
  return { reactions = { hurt } }
end
"#;
    for (file, source) in [("level.ts", TS), ("level.luau", LUAU)] {
        let manifest = run_inline(file, source);
        assert_eq!(manifest.reactions.len(), 1, "{file}: the script ran");
        assert!(
            manifest.player_events.is_empty(),
            "{file}: nothing registered"
        );
        let mut world = World::new();
        let pawn = world.spawn_player(Some(Seat(1)), 10.0);
        world.install(manifest.reactions, manifest.player_events);
        world.tick();
        assert_eq!(world.health(pawn), 10.0, "{file}: nothing fired");
    }
}

// AC: writing through a `byPlayer` ref is refused at script evaluation, in both
// runtimes. The data script fails as a whole: it logs the throw and yields an
// empty manifest.
#[test]
fn update_state_on_a_by_player_ref_throws_in_both_runtimes() {
    const MESSAGE: &str = "updateState: a byPlayer ref cannot be written; write a per-owner slot with on.player.addSlot";
    const TS: &str = r#"
import { players, becomes, read, defineReaction, defineStore, getGameState } from "postretro";
import type { PlayerEventParams } from "postretro";
import { updateState } from "postretro/ui";
const s = defineStore("leveling", { level: { type: "number", default: 1, perOwner: true, network: "ownerPrivate" } });
const hp = getGameState().player.health;
const bad = defineReaction("bad", (on: PlayerEventParams) => updateState(s.level.byPlayer(on.player), 1));
export function setupLevel() {
  return {
    reactions: [bad],
    playerEvents: [players().on(becomes(read(hp).lt(50)), [bad])],
  };
}
"#;
    const LUAU: &str = r#"
local Postretro = require("postretro")
local UI = require("postretro/ui")
local s = Postretro.defineStore("leveling", { level = { type = "number", default = 1, perOwner = true, network = "ownerPrivate" } })
local hp = Postretro.getGameState().player.health
local bad = Postretro.defineReaction("bad", function(on)
  return UI.updateState(s.level:byPlayer(on.player), 1)
end)
function setupLevel(_ctx)
  return {
    reactions = { bad },
    playerEvents = { Postretro.players():on(Postretro.becomes(Postretro.read(hp):lt(50)), { bad }) },
  }
end
"#;
    for (file, source) in [("level.ts", TS), ("level.luau", LUAU)] {
        let capture = LogCapture::start();
        let manifest = run_inline(file, source);
        assert!(
            manifest.player_events.is_empty() && manifest.reactions.is_empty(),
            "{file}: a throwing script yields an empty manifest"
        );
        capture.assert_logged(log::Level::Warn, MESSAGE);
    }
}
