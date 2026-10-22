// Teleporter ordering proofs through the authoritative fixed-tick seam.
// See: context/lib/movement.md §6 (Teleport) · context/lib/scripting.md §12

use glam::{Quat, Vec3};
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::projectile::ProjectileComponent;
use postretro_entities::{
    CrossingCondition, CrossingDescriptor, EntityId, EntityRegistry, FireOrigin, FireSubject,
    KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode, NamedReaction, NumericRange,
    PrimitiveDescriptor, ProgressDescriptor, ReactionDescriptor, ScriptCtx, SequenceStep,
    SequenceTarget, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    TeleportEventDescriptor, TeleporterComponent, Transform, TriggerActivation, TriggerCommand,
    TriggerFireMode, TriggerVolumeComponent,
};
use postretro_foundation::{PlayerMovementComponent, Seat};
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::teleporters::{TeleportMode, TeleporterRecord, TeleportersSection};
use postretro_scripting_core::reaction_dispatch::{
    ProgressTracker, dispatch_deferred_named_events_with_sequences,
};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_scripting_core::state_crossings::CrossingDetector;

use super::tests::{remote_command, sim_command, trigger_movement};
use super::{
    PostMovementCommand, RemotePawnCommand, TickEvents, TriggerTickContext, simulate_tick,
};
use crate::ai_host::AiTickResult;
use crate::collision::CollisionWorld;
use crate::collision::moving::MoverCollider;
use crate::kinematic_mover::MoverTickStateTable;
use crate::residual_drain::ResidualRegistries;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::scripting_systems::reaction_scheduler::ReactionScheduler;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::teleporter_commands::register_sequenced_teleporter_primitives;
use crate::teleporter_system::{TeleporterBindingTable, drain_teleport_residuals};
use crate::trigger_bindings::TriggerBindingTable;
use crate::trigger_system::{PlayerId, TriggerEventEdge, TriggerSystem};

const DT: f32 = 1.0 / 60.0;
const ARM: bool = true;
const DISARM: bool = false;

fn brush(center: Vec3, half: f32) -> EnvironmentBrush {
    let min = center - Vec3::splat(half);
    let max = center + Vec3::splat(half);
    let planes = (0..3)
        .flat_map(|axis| {
            let mut normal = Vec3::ZERO;
            normal[axis] = 1.0;
            [
                EnvironmentPlane {
                    normal: normal.to_array(),
                    distance: max[axis],
                },
                EnvironmentPlane {
                    normal: (-normal).to_array(),
                    distance: -min[axis],
                },
            ]
        })
        .collect();
    EnvironmentBrush {
        aabb_min: min.to_array(),
        aabb_max: max.to_array(),
        planes,
    }
}

/// A cube region of `half` extent around `center`, sending to `destination`.
fn classic(center: Vec3, half: f32, destination: Vec3, armed: bool) -> TeleporterRecord {
    TeleporterRecord {
        source_entity: 0,
        mode: TeleportMode::Classic,
        enabled_on_spawn: armed,
        brushes: vec![brush(center, half)],
        destination_origin: destination.to_array(),
        destination_yaw: 0.0,
        landmark_origin: [0.0; 3],
        landmark_yaw: 0.0,
        arrival_sound: String::new(),
        on_teleport: String::new(),
    }
}

fn reaction(name: &str, descriptor: ReactionDescriptor) -> NamedReaction {
    NamedReaction {
        name: name.into(),
        descriptor,
    }
}

/// A presentation-only body: it leaves one frame-end residual per event fire,
/// which is how these proofs count authoritative `teleport` events.
fn flash(name: &str) -> NamedReaction {
    reaction(
        name,
        ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: "flashScreen".into(),
            target: None,
            args: serde_json::json!({ "color": [1.0, 1.0, 1.0, 1.0], "durationMs": 30.0 }),
            kind: None,
            tag: None,
            on_complete: None,
        }),
    )
}

fn disarm_fired_teleporter(name: &str) -> NamedReaction {
    reaction(
        name,
        ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: "disarmTeleporter".into(),
            target: Some("@teleporter".into()),
            args: serde_json::json!({}),
            kind: None,
            tag: None,
            on_complete: None,
        }),
    )
}

fn member_sequence(name: &str, gate: EntityId, armed: bool) -> NamedReaction {
    reaction(
        name,
        ReactionDescriptor::Sequence(vec![SequenceStep {
            id: SequenceTarget::Entity(gate),
            primitive: if armed {
                "armTeleporter"
            } else {
                "disarmTeleporter"
            }
            .into(),
            args: serde_json::json!({}),
        }]),
    )
}

/// The authoritative host seam with installed teleporters, script bindings,
/// triggers and movers, driven one fixed tick at a time.
struct Rig {
    ctx: ScriptCtx,
    world: CollisionWorld,
    teleporters: Vec<EntityId>,
    bindings: TeleporterBindingTable,
    triggers: TriggerBindingTable,
    bridge: TriggerVolumeBridge,
    trigger_system: TriggerSystem,
    progress: ProgressTracker,
    movers: MoverTickStateTable,
    mover_colliders: Vec<MoverCollider>,
    hit_zones: HitZoneStore,
    steps: SequencedPrimitiveRegistry,
    remote: Vec<RemotePawnCommand>,
}

impl Rig {
    fn new(records: Vec<TeleporterRecord>) -> Self {
        Self::with_world(CollisionWorld::new(), records)
    }

    fn with_world(mut world: CollisionWorld, mut records: Vec<TeleporterRecord>) -> Self {
        for (index, record) in records.iter_mut().enumerate() {
            record.source_entity = index as u32 + 1;
        }
        // Round-trip through the PRL codec, as level load does.
        let section = TeleportersSection {
            teleporters: records,
        };
        let section = TeleportersSection::from_bytes(&section.to_bytes()).unwrap();
        world.teleporters = postretro_physics::teleporters::Teleporters::from_section(&section);
        let ctx = ScriptCtx::new();
        let teleporters = {
            let mut registry = ctx.registry.borrow_mut();
            section
                .teleporters
                .iter()
                .enumerate()
                .map(|(index, record)| {
                    let id = registry.spawn(Transform::default());
                    registry
                        .set_component(
                            id,
                            TeleporterComponent {
                                index: index as u16,
                                source_entity: record.source_entity,
                                armed: record.enabled_on_spawn,
                                arrival_sound: String::new(),
                                on_teleport: String::new(),
                            },
                        )
                        .unwrap();
                    world.teleporters.bind_entity(index, id);
                    id
                })
                .collect()
        };
        let mut steps = SequencedPrimitiveRegistry::new();
        register_sequenced_teleporter_primitives(&mut steps, ctx.clone());
        Self {
            ctx,
            world,
            teleporters,
            bindings: TeleporterBindingTable::default(),
            triggers: TriggerBindingTable::default(),
            bridge: TriggerVolumeBridge::new(),
            trigger_system: TriggerSystem::default(),
            progress: ProgressTracker::new(),
            movers: MoverTickStateTable::default(),
            mover_colliders: Vec::new(),
            hit_zones: HitZoneStore::new(),
            steps,
            remote: Vec::new(),
        }
    }

    /// Install level reactions, `teleportEvents` as `(teleporter, reactions)`,
    /// and crossings, then build both binding tables as level install does.
    fn bind(
        &mut self,
        reactions: Vec<NamedReaction>,
        teleport_events: &[(usize, &[&str])],
        crossings: Vec<CrossingDescriptor>,
    ) {
        let events = teleport_events
            .iter()
            .map(|(index, fire)| TeleportEventDescriptor {
                teleporter: self.teleporters[*index],
                event: "teleport".into(),
                fire: fire.iter().map(|name| (*name).into()).collect(),
            })
            .collect();
        {
            let mut data = self.ctx.data_registry.borrow_mut();
            data.populate_level(reactions, crossings, &[]);
            data.set_level_teleport_events(events);
        }
        self.bindings =
            TeleporterBindingTable::build(&self.ctx, Default::default(), Default::default());
        self.triggers = TriggerBindingTable::build_with_script_ctx(
            &self.ctx.registry.borrow(),
            &self.ctx.data_registry.borrow(),
            &self.ctx,
        );
    }

    fn spawn_pawn(&self, position: Vec3) -> EntityId {
        let mut registry = self.ctx.registry.borrow_mut();
        let pawn = registry.spawn(Transform {
            position,
            ..Transform::default()
        });
        registry.set_component(pawn, trigger_movement()).unwrap();
        registry
            .set_component(
                pawn,
                HealthComponent {
                    max: 100.0,
                    current: 100.0,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
        pawn
    }

    fn local_pawn(&self, position: Vec3) -> EntityId {
        let pawn = self.spawn_pawn(position);
        self.ctx
            .registry
            .borrow_mut()
            .mark_local_player_pawn(pawn)
            .unwrap();
        pawn
    }

    fn remote_pawn(&mut self, position: Vec3, client: u64) -> EntityId {
        let pawn = self.spawn_pawn(position);
        self.ctx
            .registry
            .borrow_mut()
            .bind_pawn_seat(pawn, Seat(client as u16 + 1));
        let mut command = remote_command(pawn, None, client as u32 + 1, 0, false, false);
        command.owner_client_id = client;
        self.remote.push(command);
        pawn
    }

    fn trigger(&mut self, center: Vec3, half: f32) -> EntityId {
        let trigger = {
            let mut registry = self.ctx.registry.borrow_mut();
            let trigger = registry.spawn(Transform::default());
            registry
                .set_component(
                    trigger,
                    TriggerVolumeComponent::new(
                        TriggerActivation::Touch,
                        String::new(),
                        "edge".into(),
                        "edge".into(),
                        TriggerCommand::Start,
                        TriggerFireMode::Multiple,
                        0.0,
                        true,
                    ),
                )
                .unwrap();
            trigger
        };
        self.bridge.insert_for_test(
            trigger,
            center - Vec3::splat(half),
            center + Vec3::splat(half),
        );
        trigger
    }

    fn place(&self, pawn: EntityId, position: Vec3) {
        let mut registry = self.ctx.registry.borrow_mut();
        let mut transform = *registry.get_component::<Transform>(pawn).unwrap();
        transform.position = position;
        registry.set_component(pawn, transform).unwrap();
    }

    fn position(&self, entity: EntityId) -> Vec3 {
        self.ctx
            .registry
            .borrow()
            .get_component::<Transform>(entity)
            .unwrap()
            .position
    }

    fn movement(&self, pawn: EntityId) -> PlayerMovementComponent {
        self.ctx
            .registry
            .borrow()
            .get_component::<PlayerMovementComponent>(pawn)
            .unwrap()
            .clone()
    }

    fn armed(&self, index: usize) -> bool {
        self.ctx
            .registry
            .borrow()
            .get_component::<TeleporterComponent>(self.teleporters[index])
            .unwrap()
            .armed
    }

    fn health(&self, pawn: EntityId) -> f32 {
        self.ctx
            .registry
            .borrow()
            .get_component::<HealthComponent>(pawn)
            .unwrap()
            .current
    }

    fn tick(&mut self) -> TickEvents {
        self.tick_with(&[], |_| {})
    }

    /// One fixed tick. `batch` runs as script member steps after movement (in
    /// the post-movement stage), in listed order; `ai` runs at the AI stage.
    fn tick_with(
        &mut self,
        batch: &[(usize, bool)],
        mut ai: impl FnMut(&mut EntityRegistry),
    ) -> TickEvents {
        let use_edges = Default::default();
        let steps = &self.steps;
        let teleporters = &self.teleporters;
        simulate_tick(
            self.ctx.registry.clone(),
            &self.world,
            &self.hit_zones,
            None,
            postretro_collision::EnvironmentView::level_only(0.0),
            None,
            0.0,
            &mut self.progress,
            |registry, _, _, _| {
                ai(registry);
                AiTickResult {
                    events: Vec::new(),
                    projectile_spawns: Vec::new(),
                }
            },
            &self.mover_colliders,
            &mut self.movers,
            &self.remote,
            &sim_command(false, false),
            |_| {
                for &(index, armed) in batch {
                    let primitive = if armed {
                        "armTeleporter"
                    } else {
                        "disarmTeleporter"
                    };
                    steps.get(primitive).unwrap()(teleporters[index], &serde_json::json!({}))
                        .unwrap();
                }
                PostMovementCommand {
                    aim_origin: Vec3::ZERO,
                    aim_direction: Vec3::NEG_Z,
                }
            },
            DT,
            Some(TriggerTickContext {
                teleporters: Some(&self.bindings),
                system: &mut self.trigger_system,
                bridge: &self.bridge,
                bindings: &self.triggers,
                slot_table: self.ctx.slot_table.clone(),
                script_ctx: Some(self.ctx.clone()),
                auto_close_timers: None,
                use_edges: &use_edges,
            }),
            |_| {},
        )
    }

    /// The frame-end teleport residual drain production runs after a frame's ticks.
    fn drain(&self, residuals: &[crate::teleporter_system::TeleportResidual]) {
        let mut follow_ups = Vec::new();
        drain_teleport_residuals(
            residuals,
            &self.bindings,
            &ReactionScheduler::default(),
            ResidualRegistries {
                sequence: &self.steps,
                reaction: &ReactionPrimitiveRegistry::new(),
                system: &SystemReactionRegistry::new(),
            },
            &self.ctx,
            &mut follow_ups,
        );
    }
}

fn teleported(events: &TickEvents) -> Vec<(EntityId, u16)> {
    events
        .teleports
        .iter()
        .map(|(pawn, event)| (*pawn, event.teleporter))
        .collect()
}

fn event_pawns(events: &TickEvents) -> Vec<(EntityId, u16)> {
    events
        .teleport_residuals
        .iter()
        .map(|residual| (residual.pawn, residual.index))
        .collect()
}

fn near(a: Vec3, b: Vec3) {
    assert!((a - b).length() < 1.0e-4, "{a:?} != {b:?}");
}

const DEST: Vec3 = Vec3::new(40.0, 0.0, 0.0);

// a change applies at the start of the next tick, before the firing
// test, so it never governs the tick that issued it.
#[test]
fn teleport_next_tick_mutation_arm_inside_fires_next_tick_and_crossing_disarm_stops_only_the_next()
{
    let mut rig = Rig::new(vec![classic(Vec3::ZERO, 2.0, DEST, DISARM)]);
    rig.bind(Vec::new(), &[], Vec::new());
    let local = rig.local_pawn(Vec3::ZERO);
    let remote = rig.remote_pawn(Vec3::new(-10.0, 0.0, 0.0), 7);

    assert!(teleported(&rig.tick()).is_empty(), "disarmed never fires");
    let issuing = rig.tick_with(&[(0, ARM)], |_| {});
    assert!(teleported(&issuing).is_empty(), "arm governs the next tick");
    assert!(
        !rig.armed(0),
        "queued, not applied, during the issuing tick"
    );
    assert_eq!(teleported(&rig.tick()), vec![(local, 0)]);
    near(rig.position(local), DEST);

    // The remote crosses in on the tick a disarm is issued: that tick's
    // teleport stands, and the disarm stops the next crossing.
    rig.place(remote, Vec3::new(0.5, 0.0, 0.0));
    let crossing = rig.tick_with(&[(0, DISARM)], |_| {});
    assert_eq!(teleported(&crossing), vec![(remote, 0)]);
    near(rig.position(remote), DEST);
    rig.place(local, Vec3::ZERO);
    assert!(teleported(&rig.tick()).is_empty());
    assert!(!rig.armed(0));
    near(rig.position(local), Vec3::ZERO);
}

// arrival in a teleporter armed at the start of the teleport tick
// latches; exit then re-entry fires it.
#[test]
fn teleport_arrival_latch_same_tick_arm_latches_and_exit_reentry_fires() {
    let onward = Vec3::new(80.0, 0.0, 0.0);
    let mut rig = Rig::new(vec![
        classic(Vec3::ZERO, 2.0, DEST, ARM),
        classic(DEST, 2.0, onward, DISARM),
    ]);
    rig.bind(Vec::new(), &[], Vec::new());
    let pawn = rig.local_pawn(Vec3::new(-10.0, 0.0, 0.0));

    // Issued during this tick; applies at the start of the crossing tick.
    rig.tick_with(&[(1, ARM)], |_| {});
    rig.place(pawn, Vec3::ZERO);
    assert_eq!(teleported(&rig.tick()), vec![(pawn, 0)]);
    near(rig.position(pawn), DEST);
    assert_eq!(rig.movement(pawn).teleport.arrival_latches, vec![1]);

    for _ in 0..3 {
        assert!(teleported(&rig.tick()).is_empty(), "latched until exit");
    }
    rig.place(pawn, DEST + Vec3::X * 10.0);
    assert!(teleported(&rig.tick()).is_empty());
    assert!(rig.movement(pawn).teleport.arrival_latches.is_empty());
    rig.place(pawn, DEST);
    assert_eq!(teleported(&rig.tick()), vec![(pawn, 1)]);
    near(rig.position(pawn), onward);
}

// batches apply in listed order at the next tick start; only an
// armed edge clears latches, and an arm/disarm pair on a disarmed one is inert.
#[test]
fn teleport_ordered_mutation_batch_clears_latches_once_and_leaves_inert_pairs_disarmed() {
    let onward = Vec3::new(80.0, 0.0, 0.0);
    let side = Vec3::new(0.0, 0.0, 40.0);
    let mut rig = Rig::new(vec![
        classic(Vec3::ZERO, 2.0, DEST, ARM),
        classic(DEST, 2.0, onward, ARM),
        classic(side, 2.0, side * 2.0, DISARM),
    ]);
    rig.bind(Vec::new(), &[], Vec::new());
    let pawn = rig.local_pawn(Vec3::ZERO);
    let bystander = rig.remote_pawn(side, 7);
    let relatch = |rig: &mut Rig| {
        rig.place(pawn, Vec3::ZERO);
        assert_eq!(teleported(&rig.tick()), vec![(pawn, 0)]);
        assert_eq!(rig.movement(pawn).teleport.arrival_latches, vec![1]);
    };
    relatch(&mut rig);

    // arm() on an armed teleporter changes nothing: the latch holds.
    assert!(teleported(&rig.tick_with(&[(1, ARM)], |_| {})).is_empty());
    for _ in 0..2 {
        assert!(teleported(&rig.tick()).is_empty());
    }
    assert_eq!(rig.movement(pawn).teleport.arrival_latches, vec![1]);

    // [disarm, arm] in one tick: the arm edge clears the latch; occupants fire
    // on the next tick.
    assert!(teleported(&rig.tick_with(&[(1, DISARM), (1, ARM)], |_| {})).is_empty());
    assert_eq!(teleported(&rig.tick()), vec![(pawn, 1)]);
    near(rig.position(pawn), onward);

    // The same pair across ticks.
    relatch(&mut rig);
    assert!(teleported(&rig.tick_with(&[(1, DISARM)], |_| {})).is_empty());
    assert!(teleported(&rig.tick_with(&[(1, ARM)], |_| {})).is_empty());
    assert!(!rig.armed(1), "disarmed while the arm is queued");
    assert_eq!(teleported(&rig.tick()), vec![(pawn, 1)]);

    // [arm, disarm] on a disarmed one leaves it disarmed and fires nothing.
    assert!(teleported(&rig.tick_with(&[(2, ARM), (2, DISARM)], |_| {})).is_empty());
    for _ in 0..2 {
        assert!(teleported(&rig.tick()).is_empty());
    }
    assert!(!rig.armed(2));
    near(rig.position(bystander), side);
}

// every pawn crossing on one tick teleports, one event each, and
// arming an occupied seamless region shifts all occupants on the next tick.
#[test]
fn teleport_two_pawn_entry_and_arm_moves_both_and_fires_the_event_once_each() {
    let room = Vec3::new(0.0, 0.0, 40.0);
    let copy = Vec3::new(100.0, 4.0, 100.0);
    let yaw = std::f32::consts::FRAC_PI_2;
    let mut seamless = classic(room, 3.0, copy, DISARM);
    seamless.mode = TeleportMode::Seamless;
    seamless.landmark_origin = room.to_array();
    seamless.destination_yaw = yaw;
    let mut rig = Rig::new(vec![classic(Vec3::ZERO, 2.0, DEST, ARM), seamless]);
    rig.bind(
        vec![flash("seen")],
        &[(0, &["seen"]), (1, &["seen"])],
        Vec::new(),
    );
    let local = rig.local_pawn(Vec3::new(0.5, 0.0, 0.0));
    let remote = rig.remote_pawn(Vec3::new(-0.5, 0.0, 0.0), 7);

    let both = rig.tick();
    let mut moved = teleported(&both);
    moved.sort_unstable();
    let mut expected = vec![(local, 0), (remote, 0)];
    expected.sort_unstable();
    assert_eq!(moved, expected);
    let mut fired = event_pawns(&both);
    fired.sort_unstable();
    assert_eq!(fired, expected, "the event fires once per pawn");
    near(rig.position(local), DEST);
    near(rig.position(remote), DEST);

    let offsets = [Vec3::new(1.0, 0.5, 1.0), Vec3::new(-1.5, 0.0, -0.5)];
    rig.place(local, room + offsets[0]);
    rig.place(remote, room + offsets[1]);
    assert!(teleported(&rig.tick()).is_empty(), "occupied but disarmed");
    assert!(teleported(&rig.tick_with(&[(1, ARM)], |_| {})).is_empty());
    let shifted = rig.tick();
    let mut moved = teleported(&shifted);
    moved.sort_unstable();
    let mut expected = vec![(local, 1), (remote, 1)];
    expected.sort_unstable();
    assert_eq!(moved, expected);
    let mut fired = event_pawns(&shifted);
    fired.sort_unstable();
    assert_eq!(fired, expected);
    let turn = Quat::from_rotation_y(yaw);
    near(rig.position(local), copy + turn * offsets[0]);
    near(rig.position(remote), copy + turn * offsets[1]);
    for _ in 0..2 {
        let later = rig.tick();
        assert!(teleported(&later).is_empty() && later.teleport_residuals.is_empty());
    }
}

// a world-blocked arrival is refused with no move, event or sound
// edge, and each teleporter warns once however often it refuses.
#[test]
fn teleport_blocked_arrival_by_world_refuses_silently_and_warns_once_per_teleporter() {
    let logs = postretro_test_log_capture::LogCapture::start();
    let back = Vec3::new(-40.0, 0.0, 0.0);
    let wall = |x: f32| {
        [
            Vec3::new(x, -10.0, -10.0),
            Vec3::new(x, 10.0, -10.0),
            Vec3::new(x, 0.0, 10.0),
        ]
    };
    let world = CollisionWorld::from_triangles_for_test(
        wall(DEST.x).into_iter().chain(wall(back.x)).collect(),
        vec![[0, 1, 2], [3, 4, 5]],
    );
    let second = Vec3::new(0.0, 0.0, 20.0);
    let mut rig = Rig::with_world(
        world,
        vec![
            classic(Vec3::ZERO, 2.0, DEST, ARM),
            classic(second, 2.0, back, ARM),
        ],
    );
    rig.bind(
        vec![flash("seen")],
        &[(0, &["seen"]), (1, &["seen"])],
        Vec::new(),
    );
    let local = rig.local_pawn(Vec3::ZERO);
    let remote = rig.remote_pawn(second, 7);
    for _ in 0..3 {
        let events = rig.tick();
        assert!(teleported(&events).is_empty(), "no teleport, no sound edge");
        assert!(events.teleport_residuals.is_empty(), "no event");
        near(rig.position(local), Vec3::ZERO);
        near(rig.position(remote), second);
        assert_eq!(rig.movement(local).teleport.generation, 0);
        assert_eq!(rig.movement(remote).teleport.generation, 0);
    }
    logs.assert_logged_once(
        log::Level::Warn,
        "teleporter entity 1 arrival blocked; teleport refused",
    );
    logs.assert_logged_once(
        log::Level::Warn,
        "teleporter entity 2 arrival blocked; teleport refused",
    );
}

// the mover stage runs before player movement, so the arrival
// clearance sees each mover where it ends this tick.
#[test]
fn teleport_blocked_arrival_by_mover_refuses_one_moving_in_and_accepts_one_moving_out() {
    let clear = DEST + Vec3::new(0.0, 0.0, 30.0);
    for (from, to, blocked) in [(clear, DEST, true), (DEST, clear, false)] {
        let mut rig = Rig::new(vec![classic(Vec3::ZERO, 2.0, DEST, ARM)]);
        rig.bind(vec![flash("seen")], &[(0, &["seen"])], Vec::new());
        let mover = {
            let mut registry = rig.ctx.registry.borrow_mut();
            let mover = registry.spawn(Transform {
                position: from,
                ..Transform::default()
            });
            registry
                .set_component(
                    mover,
                    KinematicMoverComponent::new(
                        7,
                        KinematicMoverConfig {
                            waypoints: vec![from, to],
                            waypoint_names: vec!["from".into(), "to".into()],
                            // 50 m per tick: the whole 30 m leg lands this tick.
                            speed_mps: 3000.0,
                            wait_ms: 0.0,
                            mode: KinematicMoverMode::Once,
                            started: true,
                            spin_axis: Vec3::ZERO,
                            initial_spin_rate_rad_s: 0.0,
                            spin_accel_rad_s2: 0.0,
                            carry_yaw: false,
                        },
                    ),
                )
                .unwrap();
            mover
        };
        let corners = [
            Vec3::new(-5.0, -5.0, -5.0),
            Vec3::new(5.0, -5.0, -5.0),
            Vec3::new(5.0, 5.0, -5.0),
            Vec3::new(-5.0, 5.0, -5.0),
            Vec3::new(-5.0, -5.0, 5.0),
            Vec3::new(5.0, -5.0, 5.0),
            Vec3::new(5.0, 5.0, 5.0),
            Vec3::new(-5.0, 5.0, 5.0),
        ];
        let faces = [
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [3, 7, 6],
            [3, 6, 2],
            [0, 4, 7],
            [0, 7, 3],
            [1, 2, 6],
            [1, 6, 5],
        ];
        rig.mover_colliders =
            vec![MoverCollider::from_local_triangles(7, &corners, &faces).unwrap()];
        let pawn = rig.local_pawn(Vec3::ZERO);
        let events = rig.tick();
        near(rig.position(mover), to);
        assert_eq!(
            teleported(&events).is_empty(),
            blocked,
            "{from:?} -> {to:?}"
        );
        assert_eq!(events.teleport_residuals.is_empty(), blocked);
        near(rig.position(pawn), if blocked { Vec3::ZERO } else { DEST });
    }
}

// only live player pawns teleport. Liveness is read before
// movement, after deferred health writes land; a later kill keeps the teleport.
#[test]
fn teleport_liveness_and_body_kinds_bar_dead_and_non_players_but_not_later_kills() {
    let mut rig = Rig::new(vec![classic(Vec3::ZERO, 3.0, DEST, ARM)]);
    rig.bind(vec![flash("seen")], &[(0, &["seen"])], Vec::new());
    let outside = Vec3::new(-20.0, 0.0, 0.0);

    // An enemy and a projectile inside the armed region.
    let enemy_at = Vec3::new(1.0, 0.0, 0.0);
    let shot_at = Vec3::new(-1.0, 0.0, 1.0);
    let (enemy, shot) = {
        let mut registry = rig.ctx.registry.borrow_mut();
        let enemy = registry.spawn(Transform {
            position: enemy_at,
            ..Transform::default()
        });
        registry
            .set_component(
                enemy,
                postretro_entities::components::agent::AgentComponent::new(0.35, 1.8, 0.4, 4.0),
            )
            .unwrap();
        registry
            .set_component(
                enemy,
                HealthComponent {
                    max: 50.0,
                    current: 50.0,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
        let owner_pawn = registry.spawn(Transform::default());
        let owner_weapon = registry.spawn(Transform::default());
        let shot = registry.spawn(Transform {
            position: shot_at,
            ..Transform::default()
        });
        registry
            .set_component(
                shot,
                ProjectileComponent {
                    source_sounds: None,
                    predicted_visible: true,
                    source_action: None,
                    source_shot: None,
                    knockback_impulse: [0.0; 3],
                    direction: Vec3::NEG_Z.to_array(),
                    speed: 1.0,
                    radius: 0.0,
                    remaining_range: 10.0,
                    remaining_lifetime: 10.0,
                    damage: 0.0,
                    credit_source: "test.projectile".into(),
                    owner_pawn,
                    owner_weapon,
                    attackerless: false,
                    spawned: false,
                    predicted_shot_id: None,
                    elapsed_flight_age: 0.0,
                    flipbook_active: false,
                    impact_light: None,
                    splash: None,
                    source_weapon: None,
                    activation: None,
                },
            )
            .unwrap();
        (enemy, shot)
    };
    let events = rig.tick();
    assert!(teleported(&events).is_empty() && events.teleport_residuals.is_empty());
    near(rig.position(enemy), enemy_at);
    let flown = rig.position(shot);
    assert!(
        (flown - shot_at).length() <= 1.0 / 60.0 + 1.0e-4,
        "the projectile keeps its own flight: {flown:?}"
    );

    // A player dead at tick start stays put, tick after tick.
    let dead = rig.remote_pawn(Vec3::new(0.5, 0.0, 0.0), 7);
    crate::impact_effects::set_health(&mut rig.ctx.registry.borrow_mut(), dead, 0.0, None);
    for _ in 0..2 {
        let events = rig.tick();
        assert!(teleported(&events).is_empty() && events.teleport_residuals.is_empty());
    }
    near(rig.position(dead), Vec3::new(0.5, 0.0, 0.0));

    // A deferred health write landing at the start of the crossing tick bars it.
    let deferred = rig.local_pawn(outside);
    crate::impact_effects::set_health(&mut rig.ctx.registry.borrow_mut(), deferred, 0.0, Some(1.0));
    rig.place(deferred, Vec3::ZERO);
    assert!(rig.health(deferred) > 0.0, "fixture: alive before the tick");
    let events = rig.tick();
    assert_eq!(
        rig.health(deferred),
        0.0,
        "fixture: the write landed this tick"
    );
    assert!(teleported(&events).is_empty() && events.teleport_residuals.is_empty());
    near(rig.position(deferred), Vec3::ZERO);

    // A player killed later in its crossing tick still teleports, with its event.
    let late = rig.remote_pawn(outside, 8);
    rig.place(late, Vec3::new(-0.5, 0.0, 0.0));
    let events = rig.tick_with(&[], |registry| {
        crate::impact_effects::set_health(registry, late, 0.0, None);
    });
    assert_eq!(teleported(&events), vec![(late, 0)]);
    assert_eq!(event_pawns(&events), vec![(late, 0)]);
    assert_eq!(rig.health(late), 0.0, "fixture: the kill landed this tick");
    near(rig.position(late), DEST);
}

// the trigger tick runs after player movement, so it sees the
// arrival pose on the teleport tick.
#[test]
fn teleport_trigger_arrival_edges_enter_at_arrival_and_exit_at_source_on_the_teleport_tick() {
    let mut rig = Rig::new(vec![classic(Vec3::ZERO, 1.0, DEST, ARM)]);
    let source = rig.trigger(Vec3::ZERO, 4.0);
    let arrival = rig.trigger(DEST, 4.0);
    rig.bind(vec![flash("edge")], &[], Vec::new());
    let pawn = rig.local_pawn(Vec3::new(3.0, 0.0, 0.0));
    let edges = |events: &TickEvents| -> Vec<(EntityId, TriggerEventEdge)> {
        events
            .trigger_fires
            .iter()
            .filter(|event| event.fire.player == PlayerId::Local(pawn))
            .map(|event| (event.fire.trigger, event.edge))
            .collect()
    };
    let standing = rig.tick();
    assert!(teleported(&standing).is_empty());
    assert_eq!(edges(&standing), vec![(source, TriggerEventEdge::Enter)]);

    rig.place(pawn, Vec3::ZERO);
    let crossing = rig.tick();
    assert_eq!(teleported(&crossing), vec![(pawn, 0)]);
    let fired = edges(&crossing);
    assert_eq!(fired.len(), 2, "{fired:?}");
    assert!(
        fired.contains(&(source, TriggerEventEdge::Exit)),
        "{fired:?}"
    );
    assert!(
        fired.contains(&(arrival, TriggerEventEdge::Enter)),
        "{fired:?}"
    );
}

// the event's disarm runs in-tick and applies at the next tick
// start, so catch-up ticks in one frame cannot admit a second player. The
// frame driver lives in the app; a frame here is N ticks then the drain.
#[test]
fn teleport_one_way_catchup_gate_never_admits_the_second_player_in_any_tick_position() {
    for ticks_per_frame in [1, 3] {
        for first_tick in 0..ticks_per_frame {
            let mut rig = Rig::new(vec![classic(Vec3::ZERO, 2.0, DEST, ARM)]);
            rig.bind(
                vec![disarm_fired_teleporter("used")],
                &[(0, &["used"])],
                Vec::new(),
            );
            let first = rig.local_pawn(Vec3::new(-10.0, 0.0, 0.0));
            let second = rig.remote_pawn(Vec3::new(-10.0, 5.0, 0.0), 7);
            let mut moved = Vec::new();
            for frame in 0..3 {
                let mut residuals = Vec::new();
                for tick in 0..ticks_per_frame {
                    let index = frame * ticks_per_frame + tick;
                    if index == first_tick {
                        rig.place(first, Vec3::ZERO);
                    }
                    if index == first_tick + 1 {
                        rig.place(second, Vec3::new(0.5, 0.0, 0.0));
                    }
                    let events = rig.tick();
                    moved.extend(teleported(&events));
                    residuals.extend(events.teleport_residuals);
                }
                rig.drain(&residuals);
            }
            assert_eq!(
                moved,
                vec![(first, 0)],
                "{ticks_per_frame} tick(s) per frame, first crossing on tick {first_tick}"
            );
            assert!(!rig.armed(0));
            near(rig.position(second), Vec3::new(0.5, 0.0, 0.0));
        }
    }
}

// arm and disarm from frame-end sources (a kill `progress`, a state
// crossing) queue at the drain and govern the first tick after it. The app owns
// where that drain sits in the frame; this proves the sim side of the seam.
#[test]
fn teleport_frame_end_mutation_progress_arm_and_crossing_disarm_govern_the_first_tick_after_drain()
{
    let mut rig = Rig::new(vec![classic(Vec3::ZERO, 2.0, DEST, DISARM)]);
    let gate = rig.teleporters[0];
    rig.ctx
        .slot_table
        .borrow_mut()
        .insert_namespace(
            "gate",
            vec![(
                "flag".into(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(0.0)),
                    range: Some(NumericRange { min: 0.0, max: 1.0 }),
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: Default::default(),
                    per_owner: false,
                    accumulate: None,
                }),
            )],
        )
        .unwrap();
    rig.bind(
        vec![
            member_sequence("gate.open", gate, ARM),
            member_sequence("gate.shut", gate, DISARM),
            reaction(
                "closet.cleared",
                ReactionDescriptor::Progress(ProgressDescriptor {
                    tag: "closet".into(),
                    at: 1.0,
                    fire: "gate.open".into(),
                }),
            ),
        ],
        &[],
        vec![CrossingDescriptor {
            slot: Some("gate.flag".into()),
            condition: CrossingCondition::Above { threshold: 0.5 },
            max: 1.0,
            edge: Some("both".into()),
            fire: vec!["gate.shut".into()],
        }],
    );
    let monster = {
        let mut registry = rig.ctx.registry.borrow_mut();
        let monster = registry.spawn(Transform::default());
        registry.set_tags(monster, vec!["closet".into()]).unwrap();
        registry
            .set_component(
                monster,
                HealthComponent {
                    max: 20.0,
                    current: 20.0,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
        monster
    };
    rig.progress
        .initialize(&rig.ctx.data_registry.borrow(), &rig.ctx.registry.borrow());
    let mut crossings = CrossingDetector::new();
    crossings.initialize(
        &rig.ctx.data_registry.borrow(),
        &rig.ctx.slot_table.borrow(),
        &rig.ctx,
    );
    let pawn = rig.local_pawn(Vec3::ZERO);
    assert!(teleported(&rig.tick()).is_empty());

    // Frame end: the closet's last monster is reaped and its progress fires.
    {
        let mut registry = rig.ctx.registry.borrow_mut();
        crate::impact_effects::set_health(&mut registry, monster, 0.0, None);
        crate::impact_effects::despawn(&mut registry, monster, None);
    }
    let mut fired = Vec::new();
    crate::impact_effects::run_end_of_frame_removal_pass(
        &mut rig.ctx.registry.borrow_mut(),
        |removal| fired.extend(removal.report_to_progress(&mut rig.progress)),
    );
    assert_eq!(fired, vec!["gate.open".to_string()]);
    dispatch_deferred_named_events_with_sequences(
        fired
            .into_iter()
            .map(|name| (name, FireOrigin::HostOnly(FireSubject::None))),
        &rig.ctx.data_registry.borrow(),
        &rig.steps,
        &ReactionPrimitiveRegistry::new(),
        &SystemReactionRegistry::new(),
        &rig.ctx,
    );
    assert!(
        !rig.armed(0),
        "queued at the drain, applied by the next tick"
    );
    assert_eq!(teleported(&rig.tick()), vec![(pawn, 0)]);

    // Frame end: a state crossing disarms; the next crosser is refused on the
    // first tick after the drain.
    rig.ctx
        .slot_table
        .borrow_mut()
        .get_mut("gate.flag")
        .unwrap()
        .value = Some(SlotValue::Number(1.0));
    let crossed = crate::scripting::reactions::dispatch_state_crossings_with_sequences(
        &mut crossings,
        &rig.ctx.slot_table.borrow(),
        &rig.ctx.data_registry.borrow(),
        &rig.steps,
        &ReactionPrimitiveRegistry::new(),
        &SystemReactionRegistry::new(),
        &rig.ctx,
    );
    assert_eq!(crossed, vec!["gate.shut".to_string()]);
    assert!(rig.armed(0));
    rig.place(pawn, Vec3::ZERO);
    assert!(teleported(&rig.tick()).is_empty());
    assert!(!rig.armed(0));
    near(rig.position(pawn), Vec3::ZERO);
}

// Hot reload is a descriptor swap: the pawn's teleport record (generation,
// accumulated yaw, last teleporter, latches) is live state, not tuning, so a
// reload while latched neither re-fires the arrival region nor forgets it.
#[test]
fn teleport_record_survives_a_movement_descriptor_hot_reload() {
    use postretro_entities::{
        DescriptorComponentKind, DescriptorProvenance, DescriptorSpawnPath, EntityTypeDescriptor,
    };
    use postretro_scripting_core::refresh_plan::{
        apply_descriptor_refresh_plan, plan_descriptor_refresh,
    };
    let descriptor = |jumps: u32| {
        let mut movement = trigger_movement_descriptor();
        movement.air.jumps = jumps;
        EntityTypeDescriptor {
            faction: None,
            tolerance: None,
            canonical_name: Some("player".into()),
            inventory: None,
            light: None,
            emitter: None,
            movement: Some(movement),
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    };
    // Raised off the origin floor plane, which the reload's capsule guard checks.
    let lift = Vec3::Y * 5.0;
    let onward = Vec3::new(80.0, 5.0, 0.0);
    let mut rig = Rig::new(vec![
        classic(lift, 2.0, DEST + lift, ARM),
        classic(DEST + lift, 2.0, onward, ARM),
    ]);
    rig.bind(Vec::new(), &[], Vec::new());
    let pawn = rig.local_pawn(lift);
    rig.ctx
        .registry
        .borrow_mut()
        .set_component(
            pawn,
            DescriptorProvenance {
                canonical_name: "player".into(),
                owned_components: [DescriptorComponentKind::Movement].into_iter().collect(),
                map_overrides: Default::default(),
                spawn_path: DescriptorSpawnPath::MapPlacement,
            },
        )
        .unwrap();
    assert_eq!(teleported(&rig.tick()), vec![(pawn, 0)]);
    let before = rig.movement(pawn).teleport;
    assert_eq!(before.generation, 1);
    assert_eq!(before.arrival_latches, vec![1]);

    let plan = plan_descriptor_refresh(
        &[descriptor(0)],
        &[descriptor(2)],
        &rig.ctx.registry.borrow(),
    );
    let summary = apply_descriptor_refresh_plan(&plan, &mut rig.ctx.registry.borrow_mut()).unwrap();
    assert_eq!(summary.changed_movement_entities, vec![pawn]);
    let after = rig.movement(pawn);
    assert_eq!(after.air.jumps, 2, "fixture: tuning reseeded");
    assert_eq!(after.teleport, before);

    assert!(teleported(&rig.tick()).is_empty(), "the latch still holds");
    rig.place(pawn, DEST + lift + Vec3::X * 10.0);
    rig.tick();
    rig.place(pawn, DEST + lift);
    assert_eq!(teleported(&rig.tick()), vec![(pawn, 1)]);
    assert_eq!(rig.movement(pawn).teleport.generation, 2);
}

/// The movement descriptor `trigger_movement` builds its component from.
fn trigger_movement_descriptor() -> postretro_foundation::PlayerMovementDescriptor {
    use postretro_foundation::{
        AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementDescriptor, SpeedParams,
    };
    let descriptor = PlayerMovementDescriptor {
        sounds: None,
        knockback: Default::default(),
        capsule: CapsuleParams {
            radius: 0.4,
            half_height: 0.8,
            eye_height: 0.5,
        },
        ground: GroundParams {
            speed: SpeedParams {
                walk: 7.0,
                run: 11.0,
                crouch: 3.0,
            },
            accel: 10.0,
            step_height: 0.3,
            max_slope: 45.0,
            friction: GroundParams::DEFAULT_FRICTION,
            stop_speed: GroundParams::DEFAULT_STOP_SPEED,
        },
        air: AirParams {
            forward_steer: 0.0,
            accel: 0.7,
            max_control_speed: 0.5,
            bunny_hop: false,
            jumps: 0,
            jump_velocity: 5.5,
            jump_ceiling: 0.0,
        },
        fall: FallParams {
            terminal_velocity: 40.0,
        },
        stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
        stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
        dash: None,
        forgiveness: None,
        crouch: None,
        slide: None,
        swim: None,
        vault: None,
        view_feel: None,
    };
    assert_eq!(
        PlayerMovementComponent::from_descriptor(&descriptor),
        trigger_movement(),
        "fixture: mirrors the rig's pawn"
    );
    descriptor
}
