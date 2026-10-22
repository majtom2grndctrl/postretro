// Projectile-trap request timing through the authoritative fixed-tick seam:
// which sources act on their own tick, which latch to the next, and how they
// order. Pin ids (P#) name the brief's ordering pins.
// See: context/lib/scripting.md §12 (Entity addressing) · context/lib/entity_model.md §Components

use glam::Vec3;
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::components::projectile_trap::ProjectileTrapComponent;
use postretro_entities::components::spawner::SpawnerComponent;
use postretro_entities::{
    ComponentKind, EntityId, NamedReaction, ReactionDescriptor, ScriptCtx, SequenceStep,
    SequenceTarget, TeleportEventDescriptor, TeleporterComponent, Transform, TriggerActivation,
    TriggerCommand, TriggerFireMode, TriggerVolumeComponent,
};
use postretro_foundation::Seat;
use postretro_level_format::teleporters::TeleportersSection;
use postretro_scripting_core::reaction_dispatch::ProgressTracker;
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

use super::teleport_tick_tests::classic;
use super::tests::{remote_command, sim_command, trigger_movement};
use super::{
    PostMovementCommand, RemotePawnCommand, TickEvents, TriggerTickContext, simulate_tick,
};
use crate::ai_host::AiTickResult;
use crate::collision::CollisionWorld;
use crate::kinematic_mover::MoverTickStateTable;
use crate::player_events::PlayerEventTable;
use crate::residual_drain::{ResidualRegistries, drain_frame_residuals};
use crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::scripting_systems::reaction_scheduler::{
    ReactionScheduler, register_reaction_control_primitives,
};
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::spawner::{SpawnContext, register_sequenced_spawner_primitives};
use crate::teleporter_system::TeleporterBindingTable;
use crate::traps::commands::register_sequenced_trap_primitives;
use crate::traps::install::resolve_traps_for_level;
use crate::traps::trap_test_fixtures::{bolt, entity};
use crate::trigger_bindings::TriggerBindingTable;
use crate::trigger_system::{PlayerId, TriggerSystem};

const DT: f32 = 1.0 / 60.0;
/// Trap recovery: six ticks at the fixed rate.
const RECOVERY_MS: f32 = 90.0;
const SPAWN_COUNT: u32 = 2;
const AWAY: Vec3 = Vec3::new(-40.0, 0.0, 0.0);

fn reaction(name: &str, steps: Vec<SequenceStep>) -> NamedReaction {
    NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Sequence(steps),
    }
}

fn step(target: EntityId, primitive: &str) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Entity(target),
        primitive: primitive.into(),
        args: serde_json::Value::Null,
    }
}

fn start(target: EntityId, interval_ms: f32, phase_ms: f32) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Entity(target),
        primitive: "startTrap".into(),
        args: serde_json::json!({ "intervalMs": interval_ms, "phaseMs": phase_ms }),
    }
}

fn wait(ms: f32) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Wait,
        primitive: "wait".into(),
        args: serde_json::json!({ "durationMs": ms, "interruptible": false }),
    }
}

/// The host seam with installed traps, triggers, bindings and the scheduler.
struct Rig {
    ctx: ScriptCtx,
    world: CollisionWorld,
    triggers: TriggerBindingTable,
    teleports: TeleporterBindingTable,
    bridge: TriggerVolumeBridge,
    system: TriggerSystem,
    spawn_context: SpawnContext,
    scheduler: ReactionScheduler,
    steps: SequencedPrimitiveRegistry,
    progress: ProgressTracker,
    movers: MoverTickStateTable,
    hit_zones: HitZoneStore,
    remote: Vec<RemotePawnCommand>,
    tick: u32,
    /// `(tick, trap)` per launched trap shot.
    shots: Vec<(u32, EntityId)>,
    /// Trigger residuals awaiting this frame's drain.
    residuals: Vec<(
        crate::trigger_bindings::TriggerResidualHandle,
        EntityId,
        PlayerId,
        Vec<PlayerId>,
    )>,
}

impl Rig {
    fn new() -> Self {
        let ctx = ScriptCtx::new();
        let spawn_context = SpawnContext::default();
        spawn_context.replace_level_data(
            [("cultist".to_string(), behavior_enemy_descriptor("cultist"))]
                .into_iter()
                .collect(),
            None,
        );
        let scheduler = ReactionScheduler::default();
        scheduler.set_enabled(true);
        let mut steps = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut steps, scheduler.clone());
        register_sequenced_trap_primitives(&mut steps, ctx.clone());
        register_sequenced_spawner_primitives(&mut steps, ctx.clone(), spawn_context.clone());
        Self {
            ctx,
            world: CollisionWorld::new(),
            triggers: TriggerBindingTable::default(),
            teleports: TeleporterBindingTable::default(),
            bridge: TriggerVolumeBridge::new(),
            system: TriggerSystem::default().with_spawn_context(spawn_context.clone()),
            spawn_context,
            scheduler,
            steps,
            progress: ProgressTracker::new(),
            movers: MoverTickStateTable::default(),
            hit_zones: HitZoneStore::new(),
            remote: Vec::new(),
            tick: 0,
            shots: Vec::new(),
            residuals: Vec::new(),
        }
    }

    fn trap(&mut self, tag: &str) -> EntityId {
        let mut registry = self.ctx.registry.borrow_mut();
        let id = registry
            .try_spawn(Transform::default(), &[tag.to_string()])
            .unwrap();
        registry
            .set_component(
                id,
                ProjectileTrapComponent::new("bolt".into(), 1000.0, 0.0, false),
            )
            .unwrap();
        resolve_traps_for_level(&mut registry, &[entity("bolt", bolt(RECOVERY_MS))], true);
        id
    }

    fn spawner(&mut self, tag: &str) -> EntityId {
        let mut registry = self.ctx.registry.borrow_mut();
        let id = registry
            .try_spawn(Transform::default(), &[tag.to_string()])
            .unwrap();
        registry
            .set_component(
                id,
                SpawnerComponent {
                    archetype_name: "cultist".into(),
                    count: SPAWN_COUNT,
                    spawned_tags: vec!["spawned".into()],
                    resolved: true,
                },
            )
            .unwrap();
        id
    }

    fn spawned(&self) -> usize {
        self.ctx
            .registry
            .borrow()
            .query_by_component_and_tag(ComponentKind::Brain, Some("spawned"))
            .count()
    }

    /// A touch trigger at the origin with a direct `command` on `tag` and an
    /// `on_fire` reaction name.
    fn trigger(&mut self, tag: &str, command: TriggerCommand, on_fire: &str) -> EntityId {
        let mut registry = self.ctx.registry.borrow_mut();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                TriggerVolumeComponent::new(
                    TriggerActivation::Touch,
                    tag.into(),
                    on_fire.into(),
                    String::new(),
                    command,
                    TriggerFireMode::Multiple,
                    0.0,
                    true,
                ),
            )
            .unwrap();
        self.bridge
            .insert_for_test(id, Vec3::splat(-2.0), Vec3::splat(2.0));
        id
    }

    /// One teleporter at `center`, and its `teleport` event bound to `fire`.
    fn teleporter(&mut self, center: Vec3, fire: &str) -> EntityId {
        let mut record = classic(center, 2.0, center + Vec3::new(0.0, 0.0, 60.0), true);
        record.source_entity = 1;
        let section = TeleportersSection::from_bytes(
            &TeleportersSection {
                teleporters: vec![record],
            }
            .to_bytes(),
        )
        .unwrap();
        self.world.teleporters =
            postretro_physics::teleporters::Teleporters::from_section(&section);
        let id = {
            let mut registry = self.ctx.registry.borrow_mut();
            let id = registry.spawn(Transform::default());
            registry
                .set_component(
                    id,
                    TeleporterComponent {
                        index: 0,
                        source_entity: 1,
                        armed: true,
                        arrival_sound: String::new(),
                        on_teleport: String::new(),
                    },
                )
                .unwrap();
            id
        };
        self.world.teleporters.bind_entity(0, id);
        self.ctx
            .data_registry
            .borrow_mut()
            .set_level_teleport_events(vec![TeleportEventDescriptor {
                teleporter: id,
                event: "teleport".into(),
                fire: vec![fire.into()],
            }]);
        id
    }

    fn bind(&mut self, reactions: Vec<NamedReaction>) {
        self.ctx
            .data_registry
            .borrow_mut()
            .populate_level(reactions, Vec::new(), &[]);
        let teleport_events = self.ctx.data_registry.borrow().teleport_events().to_vec();
        if !teleport_events.is_empty() {
            self.ctx
                .data_registry
                .borrow_mut()
                .set_level_teleport_events(teleport_events);
        }
        self.triggers = TriggerBindingTable::build_with_script_ctx_and_diagnostics(
            &self.ctx.registry.borrow(),
            &self.ctx.data_registry.borrow(),
            &self.ctx,
            Default::default(),
            self.spawn_context.clone(),
        );
        self.teleports = TeleporterBindingTable::build(
            &self.ctx,
            Default::default(),
            self.spawn_context.clone(),
        );
    }

    fn pawn(&self, at: Vec3) -> EntityId {
        let mut registry = self.ctx.registry.borrow_mut();
        let pawn = registry.spawn(Transform {
            position: at,
            ..Transform::default()
        });
        registry.set_component(pawn, trigger_movement()).unwrap();
        pawn
    }

    fn local(&self, at: Vec3) -> EntityId {
        let pawn = self.pawn(at);
        self.ctx
            .registry
            .borrow_mut()
            .mark_local_player_pawn(pawn)
            .unwrap();
        pawn
    }

    fn remote(&mut self, at: Vec3, client: u64) -> EntityId {
        let pawn = self.pawn(at);
        self.ctx
            .registry
            .borrow_mut()
            .bind_pawn_seat(pawn, Seat(client as u16 + 1));
        let mut command = remote_command(pawn, None, client as u32 + 1, 0, false, false);
        command.owner_client_id = client;
        self.remote.push(command);
        pawn
    }

    fn place(&self, pawn: EntityId, at: Vec3) {
        let mut registry = self.ctx.registry.borrow_mut();
        let mut transform = *registry.get_component::<Transform>(pawn).unwrap();
        transform.position = at;
        registry.set_component(pawn, transform).unwrap();
    }

    /// One authoritative tick, as the frame loop runs it inside a frame. Its
    /// trigger residuals wait for the frame's drain.
    fn tick(&mut self) -> TickEvents {
        let use_edges = Default::default();
        let events = simulate_tick(
            self.ctx.registry.clone(),
            &self.world,
            &self.hit_zones,
            None,
            postretro_collision::EnvironmentView::level_only(0.0),
            None,
            0.0,
            &mut self.progress,
            |_, _, _, _| AiTickResult {
                events: Vec::new(),
                projectile_spawns: Vec::new(),
            },
            &[],
            &mut self.movers,
            &self.remote,
            &sim_command(false, false),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            DT,
            Some(TriggerTickContext {
                teleporters: Some(&self.teleports),
                system: &mut self.system,
                bridge: &self.bridge,
                bindings: &self.triggers,
                slot_table: self.ctx.slot_table.clone(),
                script_ctx: Some(self.ctx.clone()),
                auto_close_timers: None,
                use_edges: &use_edges,
            }),
            |_| {},
        );
        for spawn in &events.trap_projectile_spawns {
            let trap = self
                .ctx
                .registry
                .borrow()
                .get_component::<ProjectileComponent>(spawn.projectile)
                .unwrap()
                .owner_pawn;
            self.shots.push((self.tick, trap));
        }
        self.scheduler.evaluate(&events.trigger_exit_fires);
        self.residuals
            .extend(events.trigger_residuals.iter().cloned());
        self.tick += 1;
        events
    }

    /// The frame-end drain: trigger residuals, then `wait` landings.
    fn drain(&mut self) {
        let reactions = ReactionPrimitiveRegistry::new();
        let systems = SystemReactionRegistry::new();
        let registries = || ResidualRegistries {
            sequence: &self.steps,
            reaction: &reactions,
            system: &systems,
        };
        let mut follow_ups = Vec::new();
        drain_frame_residuals(
            &std::mem::take(&mut self.residuals),
            &self.triggers,
            &self.system,
            &[],
            &PlayerEventTable::default(),
            &self.scheduler,
            registries(),
            &self.ctx,
            &mut follow_ups,
            |player, registry| match player {
                PlayerId::Local(pawn) => registry.seat_for_pawn(pawn),
                PlayerId::Remote(client) => Some(Seat(client as u16 + 1)),
            },
        );
        self.scheduler.drain_landings(
            &self.ctx.data_registry.borrow(),
            &self.steps,
            &ReactionPrimitiveRegistry::new(),
            &SystemReactionRegistry::new(),
            &self.ctx,
        );
    }

    /// One frame of `ticks` authoritative ticks, then the frame-end drain.
    fn frame_of(&mut self, ticks: usize) {
        self.scheduler.begin_frame();
        for _ in 0..ticks {
            self.tick();
        }
        self.drain();
    }

    fn frame(&mut self) {
        self.frame_of(1);
    }

    fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.frame();
        }
    }

    fn shot_ticks(&self, trap: EntityId) -> Vec<u32> {
        self.shots
            .iter()
            .filter(|(_, owner)| *owner == trap)
            .map(|(tick, _)| *tick)
            .collect()
    }

    fn running(&self, trap: EntityId) -> bool {
        self.ctx
            .registry
            .borrow()
            .get_component::<ProjectileTrapComponent>(trap)
            .unwrap()
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.schedule.is_some())
    }

    /// Issue a member step the way the frame-end drain does: outside any tick.
    fn latch(&self, target: EntityId, step: &SequenceStep) {
        self.steps.get(&step.primitive).unwrap()(target, &step.args).unwrap();
    }
}

// P7 and P1: a trigger-bound reaction's step before its first `wait` acts on
// the Enter tick, and a no-script `fire` on the same trap that tick dedupes.
#[test]
fn trigger_bound_trap_step_acts_on_the_enter_tick_and_dedupes_with_the_direct_fire() {
    let mut rig = Rig::new();
    let trap = rig.trap("a");
    rig.trigger("a", TriggerCommand::Fire, "plate");
    rig.bind(vec![reaction("plate", vec![step(trap, "fireTrap")])]);
    let pawn = rig.local(AWAY);
    rig.frames(2);
    rig.place(pawn, Vec3::ZERO);
    rig.frame();
    assert_eq!(
        rig.shot_ticks(trap),
        vec![2],
        "one execution on the Enter tick"
    );
}

// P24: the direct command is issued first, then the bound step.
#[test]
fn direct_command_precedes_bound_trap_steps() {
    for (command, bound, running) in [
        (TriggerCommand::Start, "stopTrap", false),
        (TriggerCommand::Stop, "startTrap", true),
    ] {
        let mut rig = Rig::new();
        let trap = rig.trap("a");
        rig.trigger("a", command, "plate");
        rig.bind(vec![reaction("plate", vec![step(trap, bound)])]);
        let pawn = rig.local(AWAY);
        rig.frame();
        rig.place(pawn, Vec3::ZERO);
        rig.frame();
        assert_eq!(
            rig.running(trap),
            running,
            "{bound} after the direct command"
        );
        if !running {
            assert!(rig.shots.is_empty(), "start then stop fires nothing");
        }
    }
}

// P21: a teleport-bound trap step acts on the teleport's tick.
#[test]
fn teleport_bound_trap_step_acts_on_the_teleport_tick() {
    let mut rig = Rig::new();
    let trap = rig.trap("a");
    let gate = Vec3::new(20.0, 0.0, 0.0);
    rig.teleporter(gate, "jump");
    rig.bind(vec![reaction("jump", vec![step(trap, "fireTrap")])]);
    let pawn = rig.local(AWAY);
    rig.frames(3);
    rig.place(pawn, gate);
    let events = rig.tick();
    assert_eq!(events.teleports.len(), 1, "the pawn teleported this tick");
    assert_eq!(rig.shot_ticks(trap), vec![3]);
}

// P14 and P15: a post-`wait` step latches to the next authoritative tick, and
// landings from different ticks of one multi-tick frame all act on that one
// next tick. Both tails park at the Enter frame's drain.
#[test]
fn post_wait_steps_latch_to_the_next_tick_even_from_a_catch_up_frame() {
    let mut rig = Rig::new();
    let first = rig.trap("a");
    let second = rig.trap("b");
    let third = rig.trap("c");
    rig.trigger("plate", TriggerCommand::Start, "plate");
    rig.bind(vec![
        reaction(
            "plate",
            vec![step(first, "fireTrap"), wait(1.0), step(second, "fireTrap")],
        ),
        reaction(
            "plate",
            vec![wait(2.0 * DT * 1000.0 + 1.0), step(third, "fireTrap")],
        ),
    ]);
    let pawn = rig.local(AWAY);
    rig.frame();
    rig.place(pawn, Vec3::ZERO);
    rig.frame();
    assert_eq!(
        rig.shot_ticks(first),
        vec![1],
        "the pre-wait step on the Enter tick"
    );
    // One catch-up frame of ticks 2..=5: the waits land on ticks 2 and 4.
    rig.frame_of(4);
    assert!(
        rig.shot_ticks(second).is_empty() && rig.shot_ticks(third).is_empty(),
        "landings run at the frame end"
    );
    rig.frame();
    assert_eq!(rig.shot_ticks(second), vec![6], "acts on the next tick");
    assert_eq!(
        rig.shot_ticks(third),
        vec![6],
        "a landing from a later tick of that frame acts on the same next tick"
    );
}

// P17: on a tick, latched requests apply before that tick's trigger-bound ones.
#[test]
fn latched_requests_apply_before_trigger_requests() {
    // A latched start, then a trigger stop on the next tick: nothing fires.
    let mut rig = Rig::new();
    let trap = rig.trap("a");
    rig.trigger("a", TriggerCommand::Stop, "");
    rig.bind(Vec::new());
    let pawn = rig.local(AWAY);
    rig.frame();
    rig.latch(trap, &start(trap, 100.0, 0.0));
    rig.place(pawn, Vec3::ZERO);
    rig.frames(20);
    assert!(rig.shots.is_empty() && !rig.running(trap));

    // A latched stop on a running trap, then a trigger start: re-anchored on
    // the Enter tick, which fires at phase 0.
    let mut rig = Rig::new();
    let trap = rig.trap("a");
    rig.trigger("a", TriggerCommand::Start, "");
    rig.bind(Vec::new());
    let pawn = rig.local(AWAY);
    rig.latch(trap, &start(trap, 1000.0, 0.0));
    rig.frames(10);
    assert_eq!(rig.shot_ticks(trap), vec![0]);
    rig.latch(trap, &step(trap, "stopTrap"));
    rig.place(pawn, Vec3::ZERO);
    rig.frame();
    assert_eq!(
        rig.shot_ticks(trap),
        vec![0, 10],
        "re-anchored on the Enter tick"
    );
}

// P18: a frame of several ticks runs every tick's due check, and a mid-frame
// trigger-bound start anchors on its own tick: shot ticks match one tick per frame.
#[test]
fn catch_up_frame_matches_one_tick_per_frame() {
    let run = |ticks_per_frame: usize| {
        let mut rig = Rig::new();
        let repeating = rig.trap("a");
        let triggered = rig.trap("b");
        rig.trigger("b", TriggerCommand::Start, "");
        rig.bind(Vec::new());
        let pawn = rig.local(AWAY);
        rig.latch(repeating, &start(repeating, 1000.0 / 30.0 + 1.0, 0.0));
        let mut ticks = 0;
        while ticks < 60 {
            if ticks == 10 {
                rig.place(pawn, Vec3::ZERO);
            }
            rig.frame_of(ticks_per_frame);
            ticks += ticks_per_frame;
        }
        (rig.shot_ticks(repeating), rig.shot_ticks(triggered))
    };
    let single = run(1);
    assert_eq!(
        single.1.first(),
        Some(&10),
        "the start anchors on the Enter tick"
    );
    assert_eq!(run(5), single);
}

// P2: duplicate fire steps dedupe on a trap but not on a spawner, as do two
// activators entering a zero-re-arm `multiple` trigger on one tick.
#[test]
fn duplicate_fire_dedupes_on_traps_but_not_spawners() {
    let mut rig = Rig::new();
    let trap = rig.trap("closet");
    let spawner = rig.spawner("closet");
    rig.trigger("unused", TriggerCommand::Start, "plate");
    rig.bind(vec![reaction(
        "plate",
        vec![
            step(trap, "fireTrap"),
            step(trap, "fireTrap"),
            step(spawner, "spawnFromSpawner"),
            step(spawner, "spawnFromSpawner"),
        ],
    )]);
    let pawn = rig.local(AWAY);
    rig.frame();
    rig.place(pawn, Vec3::ZERO);
    rig.frame();
    assert_eq!(rig.shot_ticks(trap), vec![1]);
    assert_eq!(rig.spawned(), 2 * SPAWN_COUNT as usize);

    let mut rig = Rig::new();
    let trap = rig.trap("closet");
    rig.spawner("closet");
    rig.trigger("closet", TriggerCommand::Fire, "");
    rig.bind(Vec::new());
    let host = rig.local(AWAY);
    let client = rig.remote(AWAY, 7);
    rig.frame();
    rig.place(host, Vec3::ZERO);
    rig.place(client, Vec3::ZERO);
    rig.frame();
    assert_eq!(rig.shot_ticks(trap), vec![1], "the trap fires once");
    assert_eq!(
        rig.spawned(),
        2 * SPAWN_COUNT as usize,
        "the spawner spawns a batch per Enter"
    );
}

// P8: two players on a zero-re-arm sweep plate on one tick produce one sweep;
// a second player entering while the traps recover adds no shot.
#[test]
fn two_activators_on_a_zero_rearm_plate_fire_one_sweep() {
    let sweep = |rig: &mut Rig| {
        let first = rig.trap("a");
        let second = rig.trap("b");
        rig.trigger("plate", TriggerCommand::Start, "sweep");
        rig.bind(vec![reaction(
            "sweep",
            vec![
                step(first, "fireTrap"),
                wait(20.0),
                step(second, "fireTrap"),
            ],
        )]);
        (first, second)
    };
    let mut rig = Rig::new();
    let (first, second) = sweep(&mut rig);
    let host = rig.local(AWAY);
    let client = rig.remote(AWAY, 7);
    rig.frame();
    rig.place(host, Vec3::ZERO);
    rig.place(client, Vec3::ZERO);
    rig.frames(10);
    assert_eq!(rig.shot_ticks(first).len(), 1);
    assert_eq!(
        rig.shot_ticks(second).len(),
        1,
        "two parked tails, one execution"
    );

    let mut rig = Rig::new();
    let (first, second) = sweep(&mut rig);
    let host = rig.local(AWAY);
    let client = rig.remote(AWAY, 7);
    rig.frame();
    rig.place(host, Vec3::ZERO);
    rig.frames(2);
    rig.place(client, Vec3::ZERO);
    rig.frames(10);
    assert_eq!(
        rig.shot_ticks(first).len(),
        1,
        "the second Enter met recovery"
    );
    assert_eq!(rig.shot_ticks(second).len(), 1, "its tail met recovery too");
}

// P11: a connected client issues no trap request from any member step.
#[test]
fn client_trap_steps_issue_and_latch_nothing() {
    let rig = Rig::new();
    let mut rig = rig;
    let trap = rig.trap("a");
    rig.ctx.owner_slot_writes_enabled.set(false);
    for step in [step(trap, "fireTrap"), start(trap, 100.0, 0.0)] {
        rig.latch(trap, &step);
    }
    let pending = rig
        .ctx
        .registry
        .borrow()
        .get_component::<ProjectileTrapComponent>(trap)
        .unwrap()
        .pending;
    assert_eq!(pending, Default::default(), "nothing latched on a client");
}
