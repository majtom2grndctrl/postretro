//! The host accumulator pass over a registry: pair evaluation, real-command
//! detection, one completion per target, the progress fields, and the
//! Orderings rows whose host half lives in the pass.
//! See: context/lib/scripting.md §12 (Assist)

use std::collections::BTreeMap;

use glam::Vec3;
use postretro_entities::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use postretro_entities::{
    EntityTypeDescriptor, LifeStatesDescriptor, ReplicationScope, ScriptCtx, SlotOwnership,
    SlotRecord, SlotSchema, SlotType, SlotValue,
};
use postretro_foundation::{
    Assistable, IrNode, IrValue, LifeStateProfile, NumberOrIr, Seat, UNDECLARED_LIFE_STATE,
};
use postretro_test_log_capture::LogCapture;

use super::*;
use crate::life_state_commands::set_life_state;
use crate::sim::TickEvents;
use crate::sim::assist::{PROGRESS_UNIT, increment, required_count};
use crate::sim::assist_pairs::PairValues;
use crate::sim::tests::{remote_command, sim_command, trigger_movement};

// Sorted mod-wide names: `alive` < `down` < `wounded`.
const ALIVE: u8 = 0;
const DOWN: u8 = 1;
const WOUNDED: u8 = 2;

fn input(name: &str, owner: Option<&str>) -> IrNode {
    IrNode::Input {
        name: name.into(),
        owner: owner.map(str::to_string),
    }
}

fn literal(radius: f32, hold_ms: f32) -> Assistable {
    Assistable {
        radius: NumberOrIr::Literal(radius),
        hold_ms: NumberOrIr::Literal(hold_ms),
    }
}

fn record(default: f32, per_owner: bool) -> SlotRecord {
    SlotRecord::new(SlotSchema {
        slot_type: SlotType::Number,
        default: Some(SlotValue::Number(default)),
        range: None,
        persist: false,
        readonly: false,
        ownership: SlotOwnership::Mod,
        network: if per_owner {
            ReplicationScope::OwnerPrivatePlayer
        } else {
            ReplicationScope::None
        },
        per_owner,
        accumulate: None,
    })
}

/// `alive` (the default profile), `down` (no `use`, assistable per `down`) and
/// `wounded` (`use` and assistable, literal 1.5 m / `down`'s hold when literal).
fn descriptor(down: Option<Assistable>) -> EntityTypeDescriptor {
    EntityTypeDescriptor {
        canonical_name: Some("player".into()),
        ghost: false,
        faction: None,
        tolerance: None,
        inventory: None,
        light: None,
        emitter: None,
        movement: None,
        weapon: None,
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
        life_states: Some(LifeStatesDescriptor {
            initial: "alive".into(),
            states: BTreeMap::from([
                ("alive".into(), LifeStateProfile::default()),
                (
                    "down".into(),
                    LifeStateProfile {
                        use_enabled: false,
                        assistable: down,
                        ..Default::default()
                    },
                ),
                (
                    "wounded".into(),
                    LifeStateProfile {
                        assistable: Some(literal(1.5, 100.0)),
                        ..Default::default()
                    },
                ),
            ]),
            rejoin: Default::default(),
            state_ids: Default::default(),
            table_revision: 0,
        }),
    }
}

/// One pawn's command this tick: `None` is a remote pawn with no resolved
/// command. The local pawn (index 0) is always real.
#[derive(Clone, Copy)]
struct Cmd {
    held: bool,
    real: bool,
}

const HOLD: Option<Cmd> = Some(Cmd {
    held: true,
    real: true,
});
const GAP: Option<Cmd> = Some(Cmd {
    held: true,
    real: false,
});
const RELEASE: Option<Cmd> = Some(Cmd {
    held: false,
    real: true,
});
const NEUTRAL: Option<Cmd> = Some(Cmd {
    held: false,
    real: false,
});
const IDLE: Option<Cmd> = RELEASE;

struct World {
    ctx: ScriptCtx,
    pawns: Vec<EntityId>,
    state: AssistState,
}

impl World {
    /// `count` pawns along x, each seat-bound in spawn order (`players()`
    /// order), pawn 0 marked local; every pawn enters `alive`.
    fn new(down: Option<Assistable>, count: usize) -> Self {
        let ctx = ScriptCtx::new();
        {
            let mut table = ctx.slot_table.borrow_mut();
            table
                .insert_namespace(
                    "world",
                    vec![
                        ("radius".into(), record(2.0, false)),
                        ("hold".into(), record(100.0, false)),
                    ],
                )
                .unwrap();
            table
                .insert_namespace("stats", vec![("heal".into(), record(0.0, true))])
                .unwrap();
        }
        ctx.data_registry
            .borrow_mut()
            .replace_entity_types(vec![descriptor(down)]);
        let pawns: Vec<EntityId> = (0..count).map(|index| spawn(&ctx, index)).collect();
        ctx.registry
            .borrow_mut()
            .mark_local_player_pawn(pawns[0])
            .unwrap();
        let mut world = Self {
            ctx,
            pawns,
            state: AssistState::default(),
        };
        for index in 0..count {
            world.enter(index, "alive");
        }
        world.state.rebind(&world.ctx);
        world
    }

    fn enter(&self, index: usize, state: &str) {
        set_life_state(
            &self.ctx,
            &mut self.ctx.registry.borrow_mut(),
            &[self.pawns[index]],
            state,
        );
    }

    /// Re-enter `state` on pawn `index` even when it already holds it, as a
    /// respawn into that state does.
    fn reenter(&self, index: usize, state: &str) {
        let data = self.ctx.data_registry.borrow();
        let block = data.entities[0].life_states.as_ref().unwrap();
        let (id, profile) = block.state(state).unwrap();
        self.edit(index, |movement| {
            postretro_foundation::enter_life_state(movement, id, profile, block.table_revision);
        });
    }

    fn place(&self, index: usize, x: f32) {
        let mut registry = self.ctx.registry.borrow_mut();
        let mut transform = *registry
            .get_component::<Transform>(self.pawns[index])
            .unwrap();
        transform.position = Vec3::new(x, 0.0, 0.0);
        registry
            .set_component(self.pawns[index], transform)
            .unwrap();
    }

    fn movement(&self, index: usize) -> PlayerMovementComponent {
        self.ctx
            .registry
            .borrow()
            .get_component::<PlayerMovementComponent>(self.pawns[index])
            .unwrap()
            .clone()
    }

    fn edit(&self, index: usize, edit: impl FnOnce(&mut PlayerMovementComponent)) {
        let mut movement = self.movement(index);
        edit(&mut movement);
        self.ctx
            .registry
            .borrow_mut()
            .set_component(self.pawns[index], movement)
            .unwrap();
    }

    fn set(&self, name: &str, value: f32) {
        self.ctx
            .slot_table
            .borrow_mut()
            .get_mut(name)
            .unwrap()
            .write_value(Some(SlotValue::Number(value)));
    }

    fn set_owned(&self, name: &str, seat: u16, value: f32) {
        self.ctx
            .slot_table
            .borrow_mut()
            .get_mut(name)
            .unwrap()
            .set_per_seat_value(Seat(seat), SlotValue::Number(value));
    }

    /// One host tick: every live pawn in a declared state advances its elapsed
    /// (the movement stage's clock), then the pass runs.
    fn tick(&mut self, commands: &[Option<Cmd>]) -> Vec<AssistCompletion> {
        self.advance_clocks();
        self.pass(commands)
    }

    /// The movement stage's clock: every live pawn in a declared state
    /// advances its elapsed by one tick.
    fn advance_clocks(&self) {
        {
            let registry = self.ctx.registry.borrow();
            let live: Vec<EntityId> = self
                .pawns
                .iter()
                .copied()
                .filter(|pawn| registry.exists(*pawn))
                .collect();
            drop(registry);
            for pawn in live {
                let mut registry = self.ctx.registry.borrow_mut();
                let mut movement = registry
                    .get_component::<PlayerMovementComponent>(pawn)
                    .unwrap()
                    .clone();
                if movement.life_state != UNDECLARED_LIFE_STATE {
                    movement.life_state_elapsed_ticks += 1;
                }
                registry.set_component(pawn, movement).unwrap();
            }
        }
    }

    /// The accumulator pass alone, with the session's slot table.
    fn pass(&mut self, commands: &[Option<Cmd>]) -> Vec<AssistCompletion> {
        let slot_table = self.ctx.slot_table.clone();
        self.pass_with(commands, Some(&slot_table.borrow()))
    }

    /// The accumulator pass alone, with `slot_table` (or none).
    fn pass_with(
        &mut self,
        commands: &[Option<Cmd>],
        slot_table: Option<&postretro_scripting_core::slot_table::SlotTable>,
    ) -> Vec<AssistCompletion> {
        let mut local = sim_command(false, false);
        let mut remote = Vec::new();
        for (index, command) in commands.iter().enumerate() {
            let Some(command) = command else { continue };
            if index == 0 {
                local.assist_held = command.held;
                continue;
            }
            let mut entry = remote_command(self.pawns[index], None, index as u32, 1, false, false);
            entry.owner_client_id = index as u64;
            entry.real_command = command.real;
            entry.command.assist_held = command.held;
            remote.push(entry);
        }
        self.state.run_host_pass(
            &mut self.ctx.registry.borrow_mut(),
            slot_table,
            &local,
            &remote,
        )
    }

    /// Tick `commands` until a completion, returning the 1-based tick it
    /// landed on, or `None` within `limit`.
    fn ticks_to_complete(&mut self, commands: &[Option<Cmd>], limit: u32) -> Option<u32> {
        (1..=limit).find(|_| !self.tick(commands).is_empty())
    }

    fn progress(&self, index: usize) -> u64 {
        self.state
            .helper(self.pawns[index])
            .map_or(0, |record| record.progress.progress)
    }

    fn latched(&self, index: usize) -> bool {
        self.state
            .helper(self.pawns[index])
            .is_some_and(|record| record.progress.latched)
    }
}

impl World {
    /// One real fixed tick with the local pawn's `assist` level. `ingest` runs
    /// where ready remote hits land, in the Projectiles stage, before the
    /// accumulator pass.
    fn real_tick(
        &mut self,
        assist_held: bool,
        ingest: impl FnMut(&mut EntityRegistry, &mut dyn FnMut(&mut EntityRegistry)),
    ) -> TickEvents {
        use crate::ai_host::AiTickResult;
        use crate::collision::CollisionWorld;
        use crate::kinematic_mover::MoverTickStateTable;
        use crate::scripting_systems::hit_zones::HitZoneStore;
        use crate::sim::touch::TouchSystem;
        use postretro_entities::FactionRegistry;
        use postretro_scripting_core::reaction_dispatch::ProgressTracker;

        let collision = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let edges = std::collections::HashMap::new();
        let mut progress = ProgressTracker::new();
        let mut movers = MoverTickStateTable::default();
        let mut touch = TouchSystem::default();
        let mut system = crate::sim::TriggerSystem::default();
        let bridge = crate::sim::TriggerVolumeBridge::new();
        let bindings = crate::sim::TriggerBindingTable::default();
        let mut command = sim_command(false, false);
        command.assist_held = assist_held;
        crate::sim::simulate_tick_with_presentation_aim(
            self.ctx.registry.clone(),
            &collision,
            &hit_zones,
            None,
            postretro_collision::EnvironmentView::level_only(0.0),
            false,
            0.0,
            (0.0, 0.0),
            &mut progress,
            |_, _, _, _| AiTickResult {
                events: vec![],
                projectile_spawns: vec![],
            },
            &[],
            &mut movers,
            &[],
            &command,
            |_| crate::sim::PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            1.0 / 60.0,
            &mut touch,
            &mut self.state,
            &[],
            0,
            &FactionRegistry::default(),
            &self.ctx.faction_sentiment,
            None,
            &edges,
            &edges,
            Some(crate::sim::TriggerTickContext {
                teleporters: None,
                system: &mut system,
                bridge: &bridge,
                bindings: &bindings,
                slot_table: self.ctx.slot_table.clone(),
                script_ctx: None,
                auto_close_timers: None,
                use_edges: &edges,
            }),
            ingest,
            |_| {},
            crate::sim::TimingGate::OFF,
        )
    }
}

fn spawn(ctx: &ScriptCtx, index: usize) -> EntityId {
    let mut registry = ctx.registry.borrow_mut();
    let pawn = registry.spawn(Transform {
        position: Vec3::new(index as f32, 0.0, 0.0),
        ..Transform::default()
    });
    registry.set_component(pawn, trigger_movement()).unwrap();
    registry
        .set_component(
            pawn,
            DescriptorProvenance {
                canonical_name: "player".into(),
                owned_components: Default::default(),
                map_overrides: Default::default(),
                spawn_path: DescriptorSpawnPath::PlayerSpawn,
            },
        )
        .unwrap();
    registry.bind_pawn_seat(pawn, Seat(index as u16));
    pawn
}

/// holdMs 100 is six ticks.
const SIX: u32 = 6;

fn helper_and_downed_target() -> World {
    let world = World::new(Some(literal(1.5, 100.0)), 2);
    world.enter(1, "down");
    world
}

/// AC5 permitted: in range on an assistable state completes on the n-th held
/// tick, recording helper, target, and the target's state and entry count.
#[test]
fn assist_pass_ac5_in_range_completes_on_the_nth_tick_with_the_target_life_at_record() {
    let mut world = helper_and_downed_target();
    for tick in 1..SIX {
        assert!(world.tick(&[HOLD, IDLE]).is_empty(), "tick {tick}");
        assert_eq!(
            world.movement(0).assist_progress,
            reported(u64::from(tick) * increment(SIX))
        );
        assert_eq!(
            world.movement(1).assist_received_progress,
            world.movement(0).assist_progress
        );
    }
    let completions = world.tick(&[HOLD, IDLE]);
    let target = world.movement(1);
    assert_eq!(
        completions,
        vec![AssistCompletion {
            helper: world.pawns[0],
            target: world.pawns[1],
            life_state: DOWN,
            life_state_entry: target.life_state_entry,
        }]
    );
    assert_eq!(world.movement(0).assist_progress, 0.0);
    assert_eq!(target.assist_received_progress, 0.0);
    assert!(world.latched(0));
    // Completion itself changes no life state.
    assert_eq!(target.life_state, DOWN);
}

/// AC5 refused: out of range, a non-assistable state, radius at or below 0,
/// a helper whose state forbids `use`, and the helper itself; and the permit
/// edge `holdMs: 0` completing on the first held tick.
#[test]
fn assist_pass_ac5_refuses_each_edge_and_zero_hold_completes_on_the_first_tick() {
    // Out of range.
    let mut world = helper_and_downed_target();
    world.place(1, 1.6);
    assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 20), None);
    assert_eq!(world.progress(0), 0);
    world.place(1, 1.5);
    assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 20), Some(SIX));

    // A target in a non-assistable state.
    let mut world = World::new(Some(literal(1.5, 100.0)), 2);
    assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 20), None);
    assert_eq!(
        world.state.pair_values(world.pawns[0], world.pawns[1]),
        None
    );

    // Radius at or below 0: this helper cannot assist this target.
    for radius in [0.0, -1.0] {
        let mut world = World::new(Some(literal(radius, 100.0)), 2);
        world.enter(1, "down");
        world.place(1, 0.0);
        assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 20), None);
    }

    // A helper whose state has `use: false`.
    let mut world = helper_and_downed_target();
    world.enter(0, "down");
    assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 20), None);
    assert_eq!(
        world.state.pair_values(world.pawns[0], world.pawns[1]),
        None,
        "a helper without `use` evaluates no pair"
    );

    // A pawn whose state permits `use` and declares `assistable` never
    // assists itself.
    let mut world = World::new(Some(literal(1.5, 100.0)), 1);
    world.enter(0, "wounded");
    assert_eq!(world.ticks_to_complete(&[HOLD], 20), None);
    assert_eq!(
        world.state.pair_values(world.pawns[0], world.pawns[0]),
        None
    );
    assert_eq!(world.movement(0).assist_received_progress, 0.0);

    // `holdMs: 0`.
    let mut world = World::new(Some(literal(1.5, 0.0)), 2);
    world.enter(1, "down");
    assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 1), Some(1));
}

/// AC5: a release one held tick before the required count is refused, and
/// the next hold restarts from 0.
#[test]
fn assist_pass_ac5_release_one_tick_early_is_refused() {
    let mut world = helper_and_downed_target();
    for _ in 1..SIX {
        assert!(world.tick(&[HOLD, IDLE]).is_empty());
    }
    assert!(world.tick(&[RELEASE, IDLE]).is_empty());
    assert_eq!(world.progress(0), 0);
    assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 20), Some(SIX));
}

/// AC6 / O37: two helpers completing on one target in one tick produce one
/// completion for the earlier helper in `players()` order; the other resets
/// unlatched and completes no earlier than a full required count later; the
/// winner completes nothing more without a release and re-press.
#[test]
fn assist_pass_ac6_o37_one_completion_per_target_earlier_helper_wins() {
    let mut world = World::new(Some(literal(1.5, 100.0)), 3);
    world.enter(2, "down");
    world.place(0, 1.0);
    world.place(1, 3.0);
    let both = [HOLD, HOLD, IDLE];
    for _ in 1..SIX {
        assert!(world.tick(&both).is_empty());
    }
    let completions = world.tick(&both);
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].helper, world.pawns[0]);
    assert_eq!(completions[0].target, world.pawns[2]);
    assert!(world.latched(0));
    assert!(!world.latched(1), "the losing helper resets unlatched");
    assert_eq!(world.progress(1), 0);
    assert_eq!(
        world.state.row6_record(world.pawns[1]),
        Row6Record {
            advanced_without_completing: false,
            required_ticks: SIX
        },
        "the reset loser did not advance, so a listen host draws no alpha term above its 0"
    );

    // Heal-only fire list: T stays down. B completes after a full count; A
    // completes nothing more while it keeps holding.
    let next = world.ticks_to_complete(&both, 20);
    assert_eq!(next, Some(SIX));
    assert!(world.latched(1));
    assert!(world.latched(0));
}

/// AC6: held `assist` survives a short input gap: gap-held ticks keep progress
/// without advancing it, and only real ticks advance it.
#[test]
fn assist_pass_ac6_gap_held_ticks_keep_progress_and_only_real_ticks_advance() {
    let mut world = World::new(Some(literal(1.5, 100.0)), 2);
    world.enter(0, "down");
    // The remote pawn 1 is the helper; the local pawn 0 is the target.
    for _ in 0..3 {
        world.tick(&[IDLE, HOLD]);
    }
    let kept = world.progress(1);
    assert_eq!(kept, 3 * increment(SIX));
    for _ in 0..4 {
        assert!(world.tick(&[IDLE, GAP]).is_empty());
        assert_eq!(world.progress(1), kept);
    }
    // Three more real held ticks complete the six.
    assert!(world.tick(&[IDLE, HOLD]).is_empty());
    assert!(world.tick(&[IDLE, HOLD]).is_empty());
    assert_eq!(world.tick(&[IDLE, HOLD]).len(), 1);
}

/// A remote pawn with no resolved command holds nothing; a real held command
/// from it advances.
#[test]
fn assist_pass_remote_pawn_with_no_resolved_command_holds_nothing() {
    let mut world = World::new(Some(literal(1.5, 100.0)), 2);
    world.enter(0, "down");
    world.tick(&[IDLE, HOLD]);
    assert_eq!(world.progress(1), increment(SIX));
    world.tick(&[IDLE, None]);
    assert_eq!(world.progress(1), 0);
    assert!(!world.latched(1));
}

/// O12 (host half): `INPUT_HOLD_TICKS` gap-held ticks, then a synthesized
/// neutral: it resets progress as a release does and leaves the latch as it
/// was; the resumed hold starts from 0.
#[test]
fn assist_pass_o12_neutral_after_the_grace_resets_progress_and_the_hold_restarts() {
    let mut world = World::new(Some(literal(1.5, 100.0)), 2);
    world.enter(0, "down");
    world.tick(&[IDLE, HOLD]);
    world.tick(&[IDLE, HOLD]);
    for _ in 0..4 {
        world.tick(&[IDLE, GAP]);
    }
    assert_eq!(world.progress(1), 2 * increment(SIX));
    world.tick(&[IDLE, NEUTRAL]);
    assert_eq!(world.progress(1), 0);
    assert!(!world.latched(1));
    assert_eq!(world.ticks_to_complete(&[IDLE, HOLD], 20), Some(SIX));
}

/// O29: after a completion, gap-held ticks then synthesized neutrals reset
/// progress but leave the latch set; nothing accumulates until a real release
/// and a re-press.
#[test]
fn assist_pass_o29_neutrals_after_a_completion_keep_the_latch() {
    let mut world = World::new(Some(literal(1.5, 100.0)), 2);
    world.enter(0, "down");
    assert_eq!(world.ticks_to_complete(&[IDLE, HOLD], 20), Some(SIX));
    for _ in 0..4 {
        assert!(world.tick(&[IDLE, GAP]).is_empty());
    }
    for _ in 0..3 {
        world.tick(&[IDLE, NEUTRAL]);
        assert!(world.latched(1));
        assert_eq!(world.progress(1), 0);
    }
    for _ in 0..10 {
        assert!(world.tick(&[IDLE, HOLD]).is_empty());
        assert_eq!(world.progress(1), 0);
    }
    world.tick(&[IDLE, RELEASE]);
    assert!(!world.latched(1));
    assert_eq!(world.ticks_to_complete(&[IDLE, HOLD], 20), Some(SIX));
}

/// O4 / O10: one completion; while the helper keeps holding none follows, even
/// though the target stays assistable; after a release and re-press the next
/// comes after another full required count.
#[test]
fn assist_pass_o4_o10_completion_ends_the_hold_until_release_and_re_press() {
    for (hold_ms, required) in [(0.0, 1), (100.0, SIX)] {
        let mut world = World::new(Some(literal(1.5, hold_ms)), 2);
        world.enter(1, "down");
        assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 20), Some(required));
        assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 30), None);
        assert_eq!(world.movement(1).life_state, DOWN);
        world.tick(&[RELEASE, IDLE]);
        assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 20), Some(required));
    }
}

/// O28 (host half): after a completion on A, a second downed target B in range
/// accumulates nothing until a release and a re-press.
#[test]
fn assist_pass_o28_after_a_completion_nothing_accumulates_on_a_second_target() {
    let mut world = World::new(Some(literal(1.5, 100.0)), 3);
    world.enter(1, "down");
    world.enter(2, "down");
    world.place(1, 0.5);
    world.place(2, 1.0);
    let commands = [HOLD, IDLE, IDLE];
    assert_eq!(world.ticks_to_complete(&commands, 20), Some(SIX));
    // A crawls out of range; B is the only candidate.
    world.place(1, 5.0);
    for _ in 0..20 {
        assert!(world.tick(&commands).is_empty());
        assert_eq!(world.progress(0), 0);
        assert_eq!(world.movement(2).assist_received_progress, 0.0);
    }
    world.tick(&[RELEASE, IDLE, IDLE]);
    let completions = loop {
        let completions = world.tick(&commands);
        if !completions.is_empty() {
            break completions;
        }
    };
    assert_eq!(completions[0].target, world.pawns[2]);
}

/// O33 (helper half): a recorded completion the player-event pass drops (the
/// target moved to another assistable state; here simply never applied) still
/// ends the winner's hold; the losing helper accumulates from 0.
#[test]
fn assist_pass_o33_a_dropped_completion_still_latches_the_winner() {
    let mut world = World::new(Some(literal(1.5, 100.0)), 3);
    world.enter(2, "down");
    world.place(0, 1.0);
    world.place(1, 3.0);
    let both = [HOLD, HOLD, IDLE];
    assert_eq!(world.ticks_to_complete(&both, 20), Some(SIX));
    // The flush moves T to another assistable state; the completion is
    // dropped, never applied.
    world.enter(2, "wounded");
    for tick in 1..=SIX {
        let completions = world.tick(&both);
        assert!(world.latched(0));
        assert_eq!(world.progress(0), 0);
        if tick < SIX {
            assert!(completions.is_empty());
            assert_eq!(world.progress(1), u64::from(tick) * increment(SIX));
        } else {
            assert_eq!(completions[0].helper, world.pawns[1]);
            assert_eq!(completions[0].life_state, WOUNDED);
        }
    }
    world.tick(&[RELEASE, HOLD, IDLE]);
    assert!(!world.latched(0));
}

/// O5: a helper harvested mid-hold drops its progress; the target is
/// unchanged and reads no received progress.
#[test]
fn assist_pass_o5_harvested_helper_drops_its_progress() {
    let mut world = World::new(Some(literal(1.5, 100.0)), 2);
    world.enter(0, "down");
    world.tick(&[IDLE, HOLD]);
    world.tick(&[IDLE, HOLD]);
    assert!(world.movement(0).assist_received_progress > 0.0);
    world
        .ctx
        .registry
        .borrow_mut()
        .despawn(world.pawns[1])
        .unwrap();
    world.tick(&[IDLE, None]);
    assert!(world.state.helper(world.pawns[1]).is_none());
    assert_eq!(world.movement(0).assist_received_progress, 0.0);
    assert_eq!(world.movement(0).life_state, DOWN);
}

/// O5 (seat hold): a pawn that leaves the group but stays in the registry
/// drops its progress and its fields read 0.
#[test]
fn assist_pass_o5_pawn_leaving_the_group_reads_zero_progress() {
    let mut world = World::new(Some(literal(1.5, 100.0)), 2);
    world.enter(0, "down");
    world.tick(&[IDLE, HOLD]);
    assert!(world.movement(1).assist_progress > 0.0);
    world
        .ctx
        .registry
        .borrow_mut()
        .clear_pawn_seat(world.pawns[1]);
    world.tick(&[IDLE, HOLD]);
    assert!(world.state.helper(world.pawns[1]).is_none());
    assert_eq!(world.movement(1).assist_progress, 0.0);
    assert_eq!(world.movement(0).assist_received_progress, 0.0);
}

/// O19 (host half): the target despawns before tick N: the helper has no
/// target, its progress resets, and nothing is recorded.
#[test]
fn assist_pass_o19_target_disconnect_resets_the_helper() {
    let mut world = helper_and_downed_target();
    for _ in 1..SIX {
        world.tick(&[HOLD, IDLE]);
    }
    world
        .ctx
        .registry
        .borrow_mut()
        .despawn(world.pawns[1])
        .unwrap();
    assert!(world.tick(&[HOLD, None]).is_empty());
    assert_eq!(world.progress(0), 0);
    assert_eq!(
        world.state.helper(world.pawns[0]).unwrap().progress.target,
        None
    );
}

/// O20: a target that rejoins as a new pawn carried into `down` starts from 0,
/// as a target switch.
#[test]
fn assist_pass_o20_rejoined_target_starts_from_zero() {
    let mut world = helper_and_downed_target();
    for _ in 1..SIX {
        world.tick(&[HOLD, IDLE]);
    }
    world
        .ctx
        .registry
        .borrow_mut()
        .despawn(world.pawns[1])
        .unwrap();
    let rejoined = spawn(&world.ctx, 1);
    world.pawns[1] = rejoined;
    world.enter(1, "down");
    assert!(world.tick(&[HOLD, IDLE]).is_empty());
    assert_eq!(world.progress(0), increment(SIX));
    assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 20), Some(SIX - 1));
}

/// O8: an in-tick self-revive before the pass (here Spec A's `set_life_state`
/// directly): the target is `alive` at the pass, nothing is recorded, every
/// helper's progress on it resets, and it reads no received progress.
#[test]
fn assist_pass_o8_in_tick_self_revive_records_nothing_and_resets_every_helper() {
    let mut world = World::new(Some(literal(1.5, 100.0)), 3);
    world.enter(2, "down");
    world.place(0, 1.0);
    world.place(1, 3.0);
    for _ in 1..SIX {
        world.tick(&[HOLD, HOLD, IDLE]);
    }
    world.enter(2, "alive");
    assert!(world.tick(&[HOLD, HOLD, IDLE]).is_empty());
    assert_eq!(world.movement(2).life_state, ALIVE);
    assert_eq!(world.progress(0), 0);
    assert_eq!(world.progress(1), 0);
    assert_eq!(world.movement(2).assist_received_progress, 0.0);
}

/// O9: the target leaves and re-enters its assistable state between passes;
/// at the next pass the life-state entry count has moved past the stored
/// one, so progress restarts and no completion comes before a further full
/// count.
#[test]
fn assist_pass_o9_leave_and_reenter_between_passes_restarts_progress() {
    let mut world = helper_and_downed_target();
    for _ in 0..30 {
        world.tick(&[IDLE, IDLE]);
    }
    for _ in 1..SIX {
        world.tick(&[HOLD, IDLE]);
    }
    world.enter(1, "alive");
    world.enter(1, "down");
    assert!(world.tick(&[HOLD, IDLE]).is_empty());
    assert_eq!(world.progress(0), increment(SIX));
    assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 20), Some(SIX - 1));
}

/// O25: a helper in a declared state that respawns into that same state
/// within radius restarts from 0; one whose descriptor declares no states
/// keeps its progress across the same respawn.
#[test]
fn assist_pass_o25_declared_helper_respawn_restarts_undeclared_keeps() {
    let mut world = helper_and_downed_target();
    for _ in 0..3 {
        world.tick(&[HOLD, IDLE]);
    }
    world.reenter(0, "alive");
    world.place(0, 0.2);
    world.tick(&[HOLD, IDLE]);
    assert_eq!(world.progress(0), increment(SIX));

    let mut world = helper_and_downed_target();
    world.edit(0, |movement| {
        postretro_foundation::clear_life_state(movement, 0);
    });
    for _ in 0..3 {
        world.tick(&[HOLD, IDLE]);
    }
    world.edit(0, |movement| {
        postretro_foundation::clear_life_state(movement, 0);
    });
    world.place(0, 0.2);
    world.tick(&[HOLD, IDLE]);
    assert_eq!(world.progress(0), 4 * increment(SIX));
}

/// O25: a respawn into the same state mid-hold, with no tick in between to
/// advance either clock, restarts progress, for the target as for the helper:
/// the entry count moves on even though the state and elapsed match a
/// continuation's.
#[test]
fn assist_pass_o25_respawn_into_the_same_state_mid_hold_restarts_progress() {
    let mut world = helper_and_downed_target();
    for _ in 0..3 {
        world.tick(&[HOLD, IDLE]);
    }
    world.pass(&[HOLD, IDLE]);
    assert_eq!(world.progress(0), 4 * increment(SIX));
    world.reenter(1, "down");
    world.pass(&[HOLD, IDLE]);
    assert_eq!(world.progress(0), increment(SIX));
    assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 20), Some(SIX - 1));
}

/// O9 from a stored elapsed of zero: the target enters `down` mid-tick, after
/// the movement stage, so the pass stores it at elapsed 0. In the flush it
/// leaves and re-enters `down` (elapsed 0 again), and the next tick's
/// movement raises elapsed to 1 as a continuation would. Progress still
/// restarts, and the hold completes only after a further full required count.
/// A genuine continuation from the same start does not reset.
#[test]
fn assist_pass_o9_reentry_from_a_stored_elapsed_of_zero_restarts_progress() {
    for reenters in [false, true] {
        let mut world = World::new(Some(literal(1.5, 100.0)), 2);
        world.advance_clocks();
        world.enter(1, "down");
        world.pass(&[HOLD, IDLE]);
        assert_eq!(world.movement(1).life_state_elapsed_ticks, 0);
        assert_eq!(world.progress(0), increment(SIX));
        if reenters {
            world.enter(1, "alive");
            world.enter(1, "down");
        }
        assert!(world.tick(&[HOLD, IDLE]).is_empty());
        assert_eq!(world.movement(1).life_state_elapsed_ticks, 1);
        if reenters {
            assert_eq!(world.progress(0), increment(SIX), "the re-entry restarts");
            assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 20), Some(SIX - 1));
        } else {
            assert_eq!(world.progress(0), 2 * increment(SIX), "a continuation");
            assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 20), Some(SIX - 2));
        }
    }
}

/// With no slot table the pass seeds every per-owner side value as missing:
/// an expression reading one faults (the pair reads out of range) instead of
/// reusing the previous pass's value, while engine-slot side reads still
/// evaluate.
#[test]
fn assist_pass_without_a_slot_table_reads_per_owner_sides_missing() {
    // radius = 2 × the helper's `stats.heal`.
    let mut world = World::new(
        Some(Assistable {
            radius: NumberOrIr::Ir(IrNode::Mul {
                a: Box::new(input("stats.heal", Some("@assist.helper"))),
                b: Box::new(IrNode::Const {
                    value: IrValue::Number(2.0),
                }),
            }),
            hold_ms: NumberOrIr::Literal(100.0),
        }),
        2,
    );
    world.enter(1, "down");
    world.set_owned("stats.heal", 0, 1.0);
    world.tick(&[HOLD, IDLE]);
    assert_eq!(world.progress(0), increment(SIX));
    world.advance_clocks();
    world.pass_with(&[HOLD, IDLE], None);
    assert_eq!(
        world.state.pair_values(world.pawns[0], world.pawns[1]),
        Some(PairValues {
            radius: 0.0,
            required_ticks: SIX
        }),
        "the per-owner read faulted to out of range"
    );
    assert_eq!(world.progress(0), 0);

    // radius = 1.5 + 0 × the target's `player.lifeStateMs`: an engine slot.
    let mut world = World::new(
        Some(Assistable {
            radius: NumberOrIr::Ir(IrNode::Add {
                a: Box::new(IrNode::Const {
                    value: IrValue::Number(1.5),
                }),
                b: Box::new(IrNode::Mul {
                    a: Box::new(input("player.lifeStateMs", Some("@assist.target"))),
                    b: Box::new(IrNode::Const {
                        value: IrValue::Number(0.0),
                    }),
                }),
            }),
            hold_ms: NumberOrIr::Literal(100.0),
        }),
        2,
    );
    world.enter(1, "down");
    world.advance_clocks();
    world.pass_with(&[HOLD, IDLE], None);
    assert_eq!(world.progress(0), increment(SIX));
}

/// O21 / O51: a level unload clears progress, latches, row-6 records, pair
/// values and stored life states but keeps the bound table; after the
/// install's rebind the first tick evaluates the expression and the assist
/// completes after a full required count.
#[test]
fn assist_pass_o21_o51_level_unload_clears_progress_and_keeps_the_bound_table() {
    let mut world = World::new(
        Some(Assistable {
            radius: NumberOrIr::Ir(input("world.radius", None)),
            hold_ms: NumberOrIr::Ir(input("world.hold", None)),
        }),
        2,
    );
    world.enter(1, "down");
    for _ in 1..SIX {
        world.tick(&[HOLD, IDLE]);
    }
    world.state.clear_for_level_unload();
    assert!(world.state.helper(world.pawns[0]).is_none());
    assert_eq!(
        world.state.row6_record(world.pawns[0]),
        Row6Record::default()
    );
    assert_eq!(
        world.state.pair_values(world.pawns[0], world.pawns[1]),
        None
    );
    assert!(world.state.pair_table().declares("player", DOWN));

    // The next level: the same entity ids recur, the target carried into
    // `down` and the helper holding.
    world.enter(1, "alive");
    world.enter(1, "down");
    world.state.rebind(&world.ctx);
    assert!(world.tick(&[HOLD, IDLE]).is_empty());
    assert_eq!(world.progress(0), increment(SIX));
    assert_eq!(
        world.state.pair_values(world.pawns[0], world.pawns[1]),
        Some(PairValues {
            radius: 2.0,
            required_ticks: SIX
        })
    );
    assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 20), Some(SIX - 1));
}

/// O22 (host half): a reload removes `assistable` from the target's state
/// mid-hold; the next pass finds no target and resets progress.
#[test]
fn assist_pass_o22_reload_removing_assistable_resets_progress() {
    let mut world = helper_and_downed_target();
    world.tick(&[HOLD, IDLE]);
    world.tick(&[HOLD, IDLE]);
    world
        .ctx
        .data_registry
        .borrow_mut()
        .replace_entity_types(vec![descriptor(None)]);
    world.edit(1, |movement| movement.assistable = false);
    world.state.rebind(&world.ctx);
    world.tick(&[HOLD, IDLE]);
    assert_eq!(world.progress(0), 0);
    assert_eq!(
        world.state.helper(world.pawns[0]).unwrap().progress.target,
        None
    );
}

fn healer_world() -> World {
    // holdMs = world.hold − 10 × the helper's `stats.heal`.
    World::new(
        Some(Assistable {
            radius: NumberOrIr::Literal(1.5),
            hold_ms: NumberOrIr::Ir(IrNode::Add {
                a: Box::new(input("world.hold", None)),
                b: Box::new(IrNode::Mul {
                    a: Box::new(input("stats.heal", Some("@assist.helper"))),
                    b: Box::new(IrNode::Const {
                        value: IrValue::Number(-10.0),
                    }),
                }),
            }),
        }),
        2,
    )
}

/// O39: a per-owner value the helper's `holdMs` reads changes between passes:
/// progress is kept and the new increment applies from the next pass;
/// `pair_values` and the row-6 record carry the new count.
#[test]
fn assist_pass_o39_changed_per_owner_value_keeps_progress_and_applies_the_new_increment() {
    let mut world = healer_world();
    world.enter(1, "down");
    world.set("world.hold", 1000.0);
    let slow = required_count(1000.0);
    for _ in 0..10 {
        world.tick(&[HOLD, IDLE]);
    }
    let kept = world.progress(0);
    assert_eq!(kept, 10 * increment(slow));
    world.set_owned("stats.heal", 0, 50.0);
    let fast = required_count(500.0);
    world.tick(&[HOLD, IDLE]);
    assert_eq!(world.progress(0), kept + increment(fast));
    assert_eq!(
        world
            .state
            .pair_values(world.pawns[0], world.pawns[1])
            .unwrap()
            .required_ticks,
        fast
    );
    assert_eq!(
        world.state.row6_record(world.pawns[0]),
        Row6Record {
            advanced_without_completing: true,
            required_ticks: fast
        }
    );
    // The completion lands on the first tick whose progress reaches one whole.
    let mut progress = kept + increment(fast);
    let mut expected = 1;
    while progress + increment(fast) < PROGRESS_UNIT {
        progress += increment(fast);
        expected += 1;
    }
    assert_eq!(world.ticks_to_complete(&[HOLD, IDLE], 100), Some(expected));
}

/// O54: a write made inside the tick before the pass (an impact effect in the
/// Projectiles stage) is read by that tick's own pass. The helper's impact
/// policy writes its per-owner `stats.heal` while tick N runs; the pass that
/// closes tick N already prices the hold with it, where a between-ticks write
/// would first show in tick N+1.
#[test]
fn assist_pass_o54_in_tick_write_is_read_by_the_same_pass() {
    use postretro_entities::components::health::{
        DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
    };
    use postretro_foundation::{DamagePayload, ImpactEventDescriptor};
    use serde_json::json;

    let mut world = healer_world();
    world.enter(1, "down");
    world.set("world.hold", 1000.0);
    let target = {
        let mut registry = world.ctx.registry.borrow_mut();
        let target = registry.spawn(Transform::default());
        registry
            .set_component(
                target,
                HealthComponent::from_descriptor(
                    &postretro_entities::data_descriptors::HealthDescriptor {
                        max: 100.0,
                        hitbox: None,
                        zone_multipliers: Default::default(),
                    },
                ),
            )
            .unwrap();
        registry
            .set_tags(target, vec!["enemy".to_string()])
            .unwrap();
        target
    };
    let mut runtime = crate::impact_policy::ImpactPolicyRuntime::new(world.ctx.clone());
    runtime.replace_global_events(vec![ImpactEventDescriptor {
        id: "heal-boost".into(),
        is_override: false,
        levels: Vec::new(),
        filter_tag: Some("enemy".into()),
        policy: vec![json!({
            "primitive": "slot.set",
            "target": "@impact.source",
            "args": { "slot": "stats.heal", "value": { "op": "const", "value": 90.0 } },
        })],
    }]);
    let helper = world.pawns[0];

    world.real_tick(true, |_, _| {});
    assert_eq!(world.progress(0), increment(required_count(1000.0)));

    // Tick N: the hit lands where ready remote hits land, ahead of the pass.
    world.real_tick(true, |registry, _| {
        let mut context = DamageContext::new("o54", DamageProducer::InTick);
        context.attacker = Some(helper);
        apply_damage_with_context(
            registry,
            target,
            &DamagePayload {
                amount: 1.0,
                impulse: Vec3::ZERO,
            },
            context,
        );
        runtime.evaluate_pending_in_registry(registry);
    });
    assert_eq!(
        world.state.pair_values(world.pawns[0], world.pawns[1]),
        Some(PairValues {
            radius: 1.5,
            required_ticks: required_count(100.0)
        })
    );
    assert_eq!(
        world.progress(0),
        increment(required_count(1000.0)) + increment(required_count(100.0))
    );
}

/// O44: a `holdMs` fault keeps progress without advancing it and warns once;
/// advancing resumes from it on recovery. A `radius` fault puts the pair out
/// of range, so row 4 resets progress; one warning.
#[test]
fn assist_pass_o44_faults_keep_or_reset_progress_and_warn_once() {
    let hold_fault = IrNode::Div {
        a: Box::new(input("world.hold", None)),
        b: Box::new(input("world.radius", None)),
    };
    let mut world = World::new(
        Some(Assistable {
            radius: NumberOrIr::Literal(1.5),
            hold_ms: NumberOrIr::Ir(hold_fault),
        }),
        2,
    );
    world.enter(1, "down");
    // 100 / 1 = 100 ms: six ticks.
    world.set("world.radius", 1.0);
    world.tick(&[HOLD, IDLE]);
    world.tick(&[HOLD, IDLE]);
    let kept = world.progress(0);
    let capture = LogCapture::start();
    world.set("world.radius", 0.0);
    for _ in 0..5 {
        assert!(world.tick(&[HOLD, IDLE]).is_empty());
        assert_eq!(world.progress(0), kept);
    }
    assert_eq!(
        capture
            .records()
            .iter()
            .filter(|record| record.message.contains("assistable.holdMs` faulted"))
            .count(),
        1
    );
    world.set("world.radius", 1.0);
    world.tick(&[HOLD, IDLE]);
    assert_eq!(world.progress(0), kept + increment(SIX));
    drop(capture);

    let mut world = World::new(
        Some(Assistable {
            radius: NumberOrIr::Ir(IrNode::Div {
                a: Box::new(input("world.radius", None)),
                b: Box::new(input("world.hold", None)),
            }),
            hold_ms: NumberOrIr::Literal(100.0),
        }),
        2,
    );
    world.enter(1, "down");
    world.set("world.radius", 150.0);
    world.tick(&[HOLD, IDLE]);
    assert_eq!(world.progress(0), increment(SIX));
    let capture = LogCapture::start();
    world.set("world.hold", 0.0);
    world.tick(&[HOLD, IDLE]);
    world.tick(&[HOLD, IDLE]);
    assert_eq!(world.progress(0), 0);
    assert_eq!(
        capture
            .records()
            .iter()
            .filter(|record| record.message.contains("assistable.radius` faulted"))
            .count(),
        1
    );
}

/// O45 (host half): helper H's pair radius is 0 on T, medic M's is 2. H never
/// selects T and accumulates nothing; M selects T and completes. The pass
/// keeps H's pair values with radius 0.
#[test]
fn assist_pass_o45_radius_zero_pair_never_selects_and_the_medic_completes() {
    // radius = 2 × the helper's `stats.heal` (H has 0, M has 1).
    let mut world = World::new(
        Some(Assistable {
            radius: NumberOrIr::Ir(IrNode::Mul {
                a: Box::new(input("stats.heal", Some("@assist.helper"))),
                b: Box::new(IrNode::Const {
                    value: IrValue::Number(2.0),
                }),
            }),
            hold_ms: NumberOrIr::Literal(100.0),
        }),
        3,
    );
    world.enter(2, "down");
    world.set_owned("stats.heal", 0, 0.0);
    world.set_owned("stats.heal", 1, 1.0);
    world.place(0, 1.0);
    world.place(1, 3.0);
    let both = [HOLD, HOLD, IDLE];
    let completions = loop {
        let completions = world.tick(&both);
        assert_eq!(world.progress(0), 0);
        if !completions.is_empty() {
            break completions;
        }
    };
    assert_eq!(completions[0].helper, world.pawns[1]);
    assert_eq!(
        world.state.pair_values(world.pawns[0], world.pawns[2]),
        Some(PairValues {
            radius: 0.0,
            required_ticks: SIX
        })
    );
    assert_eq!(
        world.state.row6_record(world.pawns[0]),
        Row6Record::default(),
        "no target: required ticks 0"
    );
}

/// `pair_values` returns the last pass's values for every evaluated pair —
/// whether or not the helper holds — and nothing for a pair that pass skipped.
#[test]
fn assist_pass_pair_values_are_the_last_pass_values_and_none_for_a_skipped_pair() {
    let mut world = World::new(Some(literal(1.5, 100.0)), 3);
    world.enter(1, "down");
    let (a, b, c) = (world.pawns[0], world.pawns[1], world.pawns[2]);
    assert_eq!(world.state.pair_values(a, b), None, "no pass yet");
    world.tick(&[IDLE, IDLE, IDLE]);
    let values = Some(PairValues {
        radius: 1.5,
        required_ticks: SIX,
    });
    assert_eq!(world.state.pair_values(a, b), values);
    assert_eq!(world.state.pair_values(c, b), values);
    // Skipped: a non-assistable target, a helper without `use`, and self.
    assert_eq!(world.state.pair_values(a, c), None);
    assert_eq!(world.state.pair_values(b, a), None);
    assert_eq!(world.state.pair_values(b, b), None);
    // The next pass replaces them.
    world.enter(1, "alive");
    world.tick(&[IDLE, IDLE, IDLE]);
    assert_eq!(world.state.pair_values(a, b), None);
}

/// The row-6 record: set while advancing without completing, cleared by a
/// completion, and carrying the chosen pair's required ticks whatever row ran.
#[test]
fn assist_pass_row_6_record_tracks_the_last_pass() {
    let mut world = World::new(Some(literal(1.5, 100.0)), 2);
    world.enter(0, "down");
    let helper = world.pawns[1];
    world.tick(&[IDLE, HOLD]);
    assert_eq!(
        world.state.row6_record(helper),
        Row6Record {
            advanced_without_completing: true,
            required_ticks: SIX
        }
    );
    world.tick(&[IDLE, GAP]);
    assert_eq!(
        world.state.row6_record(helper),
        Row6Record {
            advanced_without_completing: false,
            required_ticks: SIX
        },
        "a gap-held tick still names the chosen pair"
    );
    assert_eq!(world.ticks_to_complete(&[IDLE, HOLD], 20), Some(SIX - 1));
    assert_eq!(
        world.state.row6_record(helper),
        Row6Record {
            advanced_without_completing: false,
            required_ticks: SIX
        }
    );
    world.tick(&[IDLE, RELEASE]);
    assert_eq!(world.state.row6_record(helper), Row6Record::default());
}

/// A pawn helping while being helped reports both; received progress is the
/// highest among its helpers.
#[test]
fn assist_pass_received_progress_is_the_highest_helper_and_a_helper_reports_both() {
    let mut world = World::new(Some(literal(1.5, 100.0)), 3);
    // 0 helps 1 (wounded: may use and is assistable); 2 helps 1 later.
    world.enter(1, "wounded");
    world.enter(0, "wounded");
    world.place(0, 0.0);
    world.place(1, 1.0);
    world.place(2, 2.0);
    world.tick(&[HOLD, IDLE, IDLE]);
    world.tick(&[HOLD, HOLD, HOLD]);
    // 0 → 1 at 2 ticks; 2 → 1 at 1 tick; 1 → 0 at 1 tick.
    let one = world.movement(1);
    assert_eq!(one.assist_received_progress, reported(2 * increment(SIX)));
    assert_eq!(one.assist_progress, reported(increment(SIX)));
    assert_eq!(
        world.movement(0).assist_received_progress,
        reported(increment(SIX))
    );
    assert_eq!(world.movement(2).assist_received_progress, 0.0);
}

/// The pass runs inside the fixed tick, after the Weapons stage and the death
/// sweep, and its completions reach `TickEvents::assist_completions`; the
/// tick wrappers that take no state build a default and record none.
#[test]
fn assist_pass_runs_in_the_fixed_tick_and_reports_completions_on_tick_events() {
    let mut world = World::new(Some(literal(1.5, 0.0)), 2);
    world.enter(1, "down");
    let events = world.real_tick(true, |_, _| {});
    assert_eq!(events.assist_completions.len(), 1);
    assert_eq!(events.assist_completions[0].helper, world.pawns[0]);
    assert_eq!(events.assist_completions[0].target, world.pawns[1]);
    assert_eq!(events.assist_completions[0].life_state, DOWN);
}

#[path = "assist_snapshot_tests.rs"]
mod snapshot_tests;
