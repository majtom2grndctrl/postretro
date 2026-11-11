// Host tick rig for map-member request timing: traps, spawners and attack pools, triggers,
// teleporters, the frame-end drain and the scheduler.
// See: context/lib/scripting.md §12 (Entity addressing)

use glam::Vec3;
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::components::projectile_trap::ProjectileTrapComponent;
use postretro_entities::components::spawner::SpawnerComponent;
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, NamedReaction, ReactionDescriptor, ScriptCtx,
    SequenceStep, SequenceTarget, TeleportEventDescriptor, TeleporterComponent, Transform,
    TriggerActivation, TriggerCommand, TriggerFireMode, TriggerVolumeComponent,
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
use crate::attack_pools::commands::register_sequenced_attack_pool_primitives;
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

pub(super) const DT: f32 = 1.0 / 60.0;
/// Trap recovery: six ticks at the fixed rate.
pub(super) const RECOVERY_MS: f32 = 90.0;
pub(super) const SPAWN_COUNT: u32 = 2;
pub(super) const AWAY: Vec3 = Vec3::new(-40.0, 0.0, 0.0);

pub(super) fn reaction(name: &str, steps: Vec<SequenceStep>) -> NamedReaction {
    NamedReaction {
        name: name.into(),
        descriptor: ReactionDescriptor::Sequence(steps),
    }
}

pub(super) fn step(target: EntityId, primitive: &str) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Entity(target),
        primitive: primitive.into(),
        args: serde_json::Value::Null,
    }
}

pub(super) fn start(target: EntityId, interval_ms: f32, phase_ms: f32) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Entity(target),
        primitive: "startTrap".into(),
        args: serde_json::json!({ "intervalMs": interval_ms, "phaseMs": phase_ms }),
    }
}

pub(super) fn wait(ms: f32) -> SequenceStep {
    SequenceStep {
        id: SequenceTarget::Wait,
        primitive: "wait".into(),
        args: serde_json::json!({ "durationMs": ms, "interruptible": false }),
    }
}

/// One brain's attack-pool assignment as an AI pass saw it.
pub(super) type PoolAssignment = (EntityId, Option<EntityId>);

/// The host seam with installed traps, spawners, attack pools, triggers,
/// bindings and the scheduler.
pub(super) struct Rig {
    pub(super) ctx: ScriptCtx,
    pub(super) world: CollisionWorld,
    pub(super) triggers: TriggerBindingTable,
    pub(super) teleports: TeleporterBindingTable,
    pub(super) bridge: TriggerVolumeBridge,
    pub(super) system: TriggerSystem,
    pub(super) spawn_context: SpawnContext,
    pub(super) scheduler: ReactionScheduler,
    pub(super) steps: SequencedPrimitiveRegistry,
    pub(super) progress: ProgressTracker,
    pub(super) movers: MoverTickStateTable,
    pub(super) hit_zones: HitZoneStore,
    pub(super) remote: Vec<RemotePawnCommand>,
    pub(super) tick: u32,
    /// `(tick, trap)` per launched trap shot.
    pub(super) shots: Vec<(u32, EntityId)>,
    /// Trigger residuals awaiting this frame's drain.
    /// Each AI pass's view of every brain's attack-pool assignment, by tick.
    pub(super) ai_seen: Vec<(u32, Vec<PoolAssignment>)>,
    pub(super) residuals: Vec<(
        crate::trigger_bindings::TriggerResidualHandle,
        EntityId,
        PlayerId,
        Vec<PlayerId>,
    )>,
}

impl Rig {
    pub(super) fn new() -> Self {
        let ctx = ScriptCtx::new();
        let spawn_context = SpawnContext::default();
        spawn_context.replace_level_data(
            [("cultist".to_string(), behavior_enemy_descriptor("cultist"))]
                .into_iter()
                .collect(),
            None,
        );
        let scheduler = ReactionScheduler::default();
        scheduler.set_role(crate::scripting_systems::reaction_scheduler::SchedulerRole::Host);
        let mut steps = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut steps, scheduler.clone());
        register_sequenced_trap_primitives(&mut steps, ctx.clone());
        register_sequenced_spawner_primitives(&mut steps, ctx.clone(), spawn_context.clone());
        register_sequenced_attack_pool_primitives(&mut steps, ctx.clone());
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
            ai_seen: Vec::new(),
            residuals: Vec::new(),
        }
    }

    pub(super) fn trap(&mut self, tag: &str) -> EntityId {
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

    pub(super) fn spawner(&mut self, tag: &str) -> EntityId {
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

    pub(super) fn spawned(&self) -> usize {
        self.ctx
            .registry
            .borrow()
            .query_by_component_and_tag(ComponentKind::Brain, Some("spawned"))
            .count()
    }

    /// A touch trigger at the origin with a direct `command` on `tag` and an
    /// `on_fire` reaction name.
    pub(super) fn trigger(
        &mut self,
        tag: &str,
        command: TriggerCommand,
        on_fire: &str,
    ) -> EntityId {
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
    pub(super) fn teleporter(&mut self, center: Vec3, fire: &str) -> EntityId {
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

    pub(super) fn bind(&mut self, reactions: Vec<NamedReaction>) {
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

    pub(super) fn pawn(&self, at: Vec3) -> EntityId {
        let mut registry = self.ctx.registry.borrow_mut();
        let pawn = registry.spawn(Transform {
            position: at,
            ..Transform::default()
        });
        registry.set_component(pawn, trigger_movement()).unwrap();
        pawn
    }

    pub(super) fn local(&self, at: Vec3) -> EntityId {
        let pawn = self.pawn(at);
        self.ctx
            .registry
            .borrow_mut()
            .mark_local_player_pawn(pawn)
            .unwrap();
        pawn
    }

    pub(super) fn remote(&mut self, at: Vec3, client: u64) -> EntityId {
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

    pub(super) fn place(&self, pawn: EntityId, at: Vec3) {
        let mut registry = self.ctx.registry.borrow_mut();
        let mut transform = *registry.get_component::<Transform>(pawn).unwrap();
        transform.position = at;
        registry.set_component(pawn, transform).unwrap();
    }

    /// One authoritative tick, as the frame loop runs it inside a frame. Its
    /// trigger residuals wait for the frame's drain.
    pub(super) fn tick(&mut self) -> TickEvents {
        let use_edges = Default::default();
        let mut seen = Vec::new();
        let events = simulate_tick(
            self.ctx.registry.clone(),
            &self.world,
            &self.hit_zones,
            None,
            postretro_collision::EnvironmentView::level_only(0.0),
            None,
            0.0,
            &mut self.progress,
            |registry, _, _, _| {
                seen.extend(registry.iter_with_kind(ComponentKind::Brain).filter_map(
                    |(id, value)| match value {
                        ComponentValue::Brain(brain) => Some((id, brain.attack_pool)),
                        _ => None,
                    },
                ));
                AiTickResult {
                    events: Vec::new(),
                    projectile_spawns: Vec::new(),
                }
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
        self.ai_seen.push((self.tick, seen));
        self.scheduler.evaluate(&events.trigger_exit_fires);
        self.residuals
            .extend(events.trigger_residuals.iter().cloned());
        self.tick += 1;
        events
    }

    /// The frame-end drain: trigger residuals, then `wait` landings.
    pub(super) fn drain(&mut self) {
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
    pub(super) fn frame_of(&mut self, ticks: usize) {
        self.scheduler.begin_frame();
        for _ in 0..ticks {
            self.tick();
        }
        self.drain();
    }

    pub(super) fn frame(&mut self) {
        self.frame_of(1);
    }

    pub(super) fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.frame();
        }
    }

    pub(super) fn shot_ticks(&self, trap: EntityId) -> Vec<u32> {
        self.shots
            .iter()
            .filter(|(_, owner)| *owner == trap)
            .map(|(tick, _)| *tick)
            .collect()
    }

    pub(super) fn running(&self, trap: EntityId) -> bool {
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
    pub(super) fn latch(&self, target: EntityId, step: &SequenceStep) {
        self.steps.get(&step.primitive).unwrap()(target, &step.args).unwrap();
    }
}
