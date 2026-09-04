// Determinism coverage for the headless fixed-tick seam.
// See: context/lib/entity_model.md §5

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use glam::{EulerRot, Vec2, Vec3};
use parry3d::math::{Isometry, Point};
use parry3d::shape::TriMesh;
use postretro_level_format::navmesh::{NAVMESH_VERSION, NavMeshSection, NavRegion};
use proptest::prelude::*;

use super::{RemotePawnCommand, SimCommand, TickEvents, simulate_tick};
use crate::collision::CollisionWorld;
use crate::collision::moving::MoverCollider;
use crate::kinematic_mover::MoverTickStateTable;
use crate::movement::MovementInput;
use crate::nav::NavGraph;
use crate::netcode::ShotId;
use crate::scripting_systems::hit_zones::{HitZoneStore, model_matrix};
use crate::scripting_systems::reaction_scheduler::{
    ReactionScheduler, register_reaction_control_primitives,
};
use crate::scripting_systems::slot_accumulators::{
    SlotAccumulatorBindings, evaluate_slot_accumulators,
};
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::trigger_bindings::{
    BoundTriggerCommandKind, TriggerBindingTable, TriggerResidualHandle,
};
use crate::trigger_pools::{TriggerPoolSeedPolicy, install_trigger_pools};
use crate::trigger_system::{PlayerId, TriggerEvent, TriggerEventEdge, TriggerSystem};
use crate::weapon::FireButtonState;
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::health::{HealthComponent, Hitbox};
use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::mesh::{
    AnimationState, InterruptPolicy, MeshAnimation, MeshComponent, RATE_CHANGE_EPSILON, RATE_MAX,
    RATE_MIN, resolve_pending_animation_stamps,
};
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::data_descriptors::{
    ActionVerb, AttackParams, BehaviorActivityDescriptor, BehaviorGraphDescriptor,
    BehaviorGraphEnvelope, MotionVerb,
};
use postretro_entities::provenance::{
    DescriptorComponentKind, DescriptorProvenance, DescriptorSpawnPath,
};
use postretro_entities::{
    CrossingCondition, CrossingDescriptor, DataRegistry, EntityId, EntityRegistry, MoverCommand,
    NamedReaction, PrimitiveDescriptor, ReactionDescriptor, ReplicationScope, ScriptCtx,
    SlotOwnership, SlotRecord, SlotSchema, SlotTable, SlotType, SlotValue, Transform,
    TriggerActivation, TriggerFireMode, TriggerPoolArm, TriggerPoolDescriptor,
    TriggerVolumeComponent,
};
use postretro_entities::{SequenceStep, SequenceTarget};
use postretro_foundation::pose::{FootProbe, MAX_FEET};
use postretro_foundation::{
    AirParams, CapsuleParams, FallParams, FireMode, ForgivenessParams, GroundParams, IrNode,
    IrValue, PlayerMovementComponent, PlayerMovementDescriptor, ResolutionMode, SpeedParams,
    WeaponDescriptor,
};
use postretro_scripting_core::reaction_dispatch::{
    ProgressTracker, fire_named_event_with_sequences,
};
use postretro_scripting_core::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionRegistry,
};
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
use postretro_scripting_core::state_crossings::CrossingDetector;

/// Fixture selector for [`SimHarness::new`]. `Determinism` is the shipped
/// trap-pool + crossing fixture; `LevelLoadWait` additionally installs and fires
/// a `levelLoad` sequence `[note(presA), wait(ms), note(presB)]` for timed-
/// reaction coverage (Task 1's O61; Task 5/7 extend this enum for multi-player
/// plate rows).
#[derive(Debug, Clone, Copy)]
pub(crate) enum SimFixture {
    Determinism,
    LevelLoadWait { duration_ms: f64 },
}

const TICK_COUNT: usize = 600;
const DT: f32 = 1.0 / 60.0;
const GRAVITY: f32 = -20.0;
const POSITION_EPSILON: f32 = 0.001;
const VELOCITY_EPSILON: f32 = 0.001;
const ALERT_STATE: &str = "alert";
const ATTACK_STATE: &str = "attack";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Role {
    Alpha,
    Beta,
}

impl Role {
    fn start_position(self) -> Vec3 {
        match self {
            Role::Alpha => Vec3::new(-2.0, 1.21, 0.0),
            Role::Beta => Vec3::new(2.0, 1.21, 0.0),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum SpawnOrder {
    AlphaThenBeta,
    BetaThenAlpha,
}

impl SpawnOrder {
    fn roles(self) -> [Role; 2] {
        match self {
            SpawnOrder::AlphaThenBeta => [Role::Alpha, Role::Beta],
            SpawnOrder::BetaThenAlpha => [Role::Beta, Role::Alpha],
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RecordedCommand {
    wish_dir: Vec2,
    jump_pressed: bool,
    dash_pressed: bool,
    running: bool,
    crouch_intent: bool,
    facing_yaw: f32,
    fire_pressed: bool,
    fire_active: bool,
}

impl RecordedCommand {
    fn to_sim_command(self) -> SimCommand {
        SimCommand {
            movement: MovementInput {
                wish_dir: self.wish_dir,
                jump_pressed: self.jump_pressed,
                dash_pressed: self.dash_pressed,
                running: self.running,
                crouch_intent: self.crouch_intent,
                facing_yaw: self.facing_yaw,
                use_pressed: false,
                drop_pressed: false,
            },
            fire_button: FireButtonState {
                pressed: self.fire_pressed,
                active: self.fire_active,
            },
            reload: false,
            firing_slot: 0,
            select_slot: None,
            use_pressed: false,
            drop_pressed: false,
        }
    }

    fn to_post_movement_command(self) -> super::PostMovementCommand {
        super::PostMovementCommand {
            aim_origin: Vec3::new(0.0, 2.0, -20.0),
            aim_direction: Vec3::new(self.facing_yaw.sin(), 0.0, -self.facing_yaw.cos())
                .normalize(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct PawnOutcome {
    position: Vec3,
    velocity: Vec3,
}

/// Stable name for every entity this harness spawns. `EntityId` indices are
/// handed out in spawn order, so an id compared across spawn orders is not the
/// same entity; the sibling stages sidestep this by reporting names, and every
/// id leaving the tick is resolved to a label for the same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntityLabel {
    Pawn(Role),
    Enemy,
    Weapon,
    TriggerSource,
    TriggerArmTarget,
}

/// Spawn-order-independent activator identity. `PlayerId::Remote` is already
/// peer-stable; only the local pawn carries an `EntityId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlayerLabel {
    Local(Role),
    Remote(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedTriggerFire {
    trigger: EntityLabel,
    player: PlayerLabel,
    event_name: String,
    edge: TriggerEventEdge,
}

#[derive(Debug, PartialEq, Eq)]
struct RecordedCommandFire {
    fire: RecordedTriggerFire,
    commands: Vec<BoundTriggerCommandKind>,
}

#[derive(Debug, PartialEq)]
struct RecordedShot {
    shot_id: ShotId,
    owner_client_id: u64,
    pawn: EntityLabel,
    weapon: EntityLabel,
    fire_tick: u32,
    damage: f32,
    range: f32,
    pellet_count: usize,
    credit_source: String,
}

#[derive(Debug, PartialEq, Eq)]
struct RecordedReload {
    pawn: EntityLabel,
    weapon: EntityLabel,
}

/// One tick of `TickEvents` projected onto stable labels. This is the only
/// shape the determinism gate compares.
#[derive(Debug, PartialEq)]
struct RecordedTick {
    movement: Vec<&'static str>,
    ai: Vec<Cow<'static, str>>,
    weapon: Vec<&'static str>,
    weapon_impact_points: Vec<Vec3>,
    death: Vec<String>,
    authorized_shots: Vec<RecordedShot>,
    reload_deliveries: Vec<RecordedReload>,
    trigger_residuals: Vec<TriggerResidualHandle>,
    /// This tick's paired-trigger Exit fires, projected onto spawn-order-stable
    /// labels. Mirrors the production `TickEvents.trigger_exit_fires` the scheduler
    /// consumes to cancel interruptible instances.
    trigger_exit_fires: Vec<(EntityLabel, PlayerLabel)>,
    trigger_fires: Vec<RecordedTriggerFire>,
    trigger_command_fires: Vec<RecordedCommandFire>,
    predicate_crossing_fires: Vec<(String, bool)>,
    accumulator_slot: Option<SlotValue>,
}

#[derive(Debug)]
struct SimRun {
    pawns: Vec<(Role, PawnOutcome)>,
    selected_player_health: f32,
    enemy_state: String,
    events: Vec<RecordedTick>,
    trigger_residual_counts: Vec<usize>,
    trigger_slot: Option<SlotValue>,
    ir_slot_timeline: Vec<Option<SlotValue>>,
    predicate_crossing_sequence: Vec<Vec<(String, bool)>>,
    trigger_arm_target_armed: bool,
    role_health_ledger: Vec<(Role, f32)>,
    trap_pool_source_selected: bool,
}

struct SimHarness {
    registry: Rc<RefCell<EntityRegistry>>,
    world: CollisionWorld,
    hit_zones: HitZoneStore,
    active_wieldable: EntityId,
    progress: ProgressTracker,
    ai_runtime: crate::scripting_systems::ai::AiRuntime,
    mover_colliders: Vec<MoverCollider>,
    mover_states: MoverTickStateTable,
    trigger_system: TriggerSystem,
    trigger_bridge: TriggerVolumeBridge,
    trigger_bindings: TriggerBindingTable,
    trigger_script_ctx: ScriptCtx,
    trigger_slots: Rc<RefCell<SlotTable>>,
    crossing_detector: CrossingDetector,
    slot_accumulator_bindings: SlotAccumulatorBindings,
    tick_index: usize,
    role_ids: Vec<(Role, EntityId)>,
    labels: HashMap<EntityId, EntityLabel>,
    selected_player: EntityId,
    remote_player: EntityId,
    enemy: EntityId,
    trigger_arm_target: EntityId,
    trap_pool_source_selected: bool,
    // E18 timed-reaction wiring. Empty/no-op for the determinism fixture; the
    // `LevelLoadWait` fixture enrolls a wait in `new` and lands it in `frame`.
    scheduler: ReactionScheduler,
    dispatch_data: DataRegistry,
    dispatch_sequence_registry: SequencedPrimitiveRegistry,
    dispatch_reaction_registry: ReactionPrimitiveRegistry,
    dispatch_system_registry: SystemReactionRegistry,
    /// Ordered log of `note` sequence-step labels — proves presA runs at install
    /// and presB lands at the drain of the frame the countdown reaches zero in.
    note_log: Rc<RefCell<Vec<String>>>,
}

impl SimHarness {
    pub(crate) fn new(spawn_order: SpawnOrder, fixture: SimFixture) -> Self {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let mut role_ids = Vec::new();
        let (active_wieldable, enemy) = {
            let mut registry = registry.borrow_mut();
            for role in spawn_order.roles() {
                let id = spawn_player(&mut registry, role.start_position());
                if role == Role::Alpha {
                    // Phase 0 still has one local sim command, not keyed
                    // per-pawn commands. Mark Alpha as the local pawn so
                    // full-seam AI/health paths remain order-observable while
                    // comparing outcomes by stable test role.
                    registry
                        .mark_local_player_pawn(id)
                        .expect("alpha role can be marked as local player");
                }
                role_ids.push((role, id));
            }
            let enemy = spawn_enemy(&mut registry, Vec3::new(-1.0, 1.0, 0.0));
            let weapon = spawn_determinism_weapon(&mut registry);
            let secondary_weapon = spawn_determinism_weapon(&mut registry);
            let alpha = role_ids
                .iter()
                .find_map(|(role, id)| (*role == Role::Alpha).then_some(*id))
                .expect("alpha role is always spawned");
            let mut inventory = Inventory::default();
            inventory.wieldables[0] = Some(weapon);
            inventory.wieldables[1] = Some(secondary_weapon);
            registry.set_component(alpha, inventory).unwrap();
            (weapon, enemy)
        };
        let selected_player = role_ids
            .iter()
            .find_map(|(role, id)| (*role == Role::Alpha).then_some(*id))
            .expect("alpha role is always spawned");
        let remote_player = role_ids
            .iter()
            .find_map(|(role, id)| (*role == Role::Beta).then_some(*id))
            .expect("beta role is always spawned");

        // The green-and-stays-green determinism gate includes a real trigger
        // firing sequence: two IR state increments, one arm command, and a
        // presentation residual. It also observes the resulting slots through
        // an IR predicate after every tick, pinning both write and observer
        // determinism at their production seams.
        let trigger_script_ctx = ScriptCtx::new();
        *trigger_script_ctx.slot_table.borrow_mut() = determinism_trigger_slots();
        let trigger_slots = trigger_script_ctx.slot_table.clone();
        let (trigger_source, trigger_arm_target) = {
            let mut registry = registry.borrow_mut();
            let source = registry.spawn(Transform::default());
            registry
                .set_component(
                    source,
                    TriggerVolumeComponent::new(
                        TriggerActivation::Touch,
                        String::new(),
                        "determinismTrigger".to_string(),
                        String::new(),
                        MoverCommand::Start,
                        TriggerFireMode::Multiple,
                        0.0,
                        true,
                    ),
                )
                .expect("determinism trigger attaches");
            registry
                .set_tags(source, vec!["determinism-trap-pool".to_string()])
                .expect("determinism trigger accepts its pool tag");

            // Keep three non-overlapping peers in the same fixture pool. The
            // fixed seed below selects the source, so
            // the existing tick sequence proves load-time pool selection and
            // ordinary trigger dispatch compose without adding a test-only wire
            // or tick field.
            for _ in 0..3 {
                let peer = registry.spawn(Transform::default());
                registry
                    .set_tags(peer, vec!["determinism-trap-pool".to_string()])
                    .expect("determinism pool peer accepts tag");
                registry
                    .set_component(
                        peer,
                        TriggerVolumeComponent::new(
                            TriggerActivation::Touch,
                            String::new(),
                            String::new(),
                            String::new(),
                            MoverCommand::Start,
                            TriggerFireMode::Multiple,
                            0.0,
                            false,
                        ),
                    )
                    .expect("determinism pool peer attaches");
            }

            let arm_target = registry.spawn(Transform::default());
            registry
                .set_tags(arm_target, vec!["determinism-arm-target".to_string()])
                .expect("determinism trigger target accepts tag");
            registry
                .set_component(
                    arm_target,
                    TriggerVolumeComponent::new(
                        TriggerActivation::Touch,
                        String::new(),
                        String::new(),
                        String::new(),
                        MoverCommand::Start,
                        TriggerFireMode::Multiple,
                        0.0,
                        false,
                    ),
                )
                .expect("determinism trigger arm target attaches");
            (source, arm_target)
        };
        let trap_pool_source_selected = {
            let report = install_trigger_pools(
                &mut registry.borrow_mut(),
                &[TriggerPoolDescriptor {
                    tag: "determinism-trap-pool".to_string(),
                    arm: TriggerPoolArm::Count(1),
                    levels: Vec::new(),
                }],
                TriggerPoolSeedPolicy::Seeded(6),
                &Default::default(),
                &Default::default(),
            );
            report.pools[0].selected == [trigger_source]
        };
        let mut trigger_data = DataRegistry::new();
        trigger_data.populate_level(
            vec![
                deterministic_trigger_primitive(
                    "setState",
                    None,
                    serde_json::json!({
                        "slot": "determinism.triggered",
                        "value": {
                            "op": "add",
                            "a": { "op": "input", "name": "determinism.triggered" },
                            "b": { "op": "const", "value": 1.0 }
                        }
                    }),
                ),
                deterministic_trigger_primitive(
                    "setState",
                    None,
                    serde_json::json!({
                        "slot": "determinism.triggered",
                        "value": {
                            "op": "add",
                            "a": { "op": "input", "name": "determinism.triggered" },
                            "b": { "op": "const", "value": 1.0 }
                        }
                    }),
                ),
                // Presser damage on the fire's activators. Both pawns stand in
                // the volume, so each takes 25 HP on their tick-one enter; this
                // is what the damage-ledger determinism test observes.
                deterministic_trigger_activator_damage(25.0),
                deterministic_trigger_primitive(
                    "armTrigger",
                    Some("determinism-arm-target"),
                    serde_json::json!({}),
                ),
                deterministic_trigger_primitive(
                    "flashScreen",
                    None,
                    serde_json::json!({ "color": [1.0, 0.0, 0.0, 1.0], "durationMs": 1.0 }),
                ),
            ],
            vec![
                CrossingDescriptor {
                    slot: None,
                    // `triggered >= 4 && enabled >= 1`, expressed using the
                    // shipped comparison/select vocabulary. Two activators each
                    // execute two IR increments on tick one, so this yields one
                    // false -> true observer edge per run.
                    condition: CrossingCondition::Ir(IrNode::Select {
                        cond: Box::new(IrNode::Ge {
                            a: Box::new(IrNode::Input {
                                name: "determinism.triggered".to_string(),
                                owner: None,
                            }),
                            b: Box::new(IrNode::Const {
                                value: IrValue::Number(4.0),
                            }),
                        }),
                        a: Box::new(IrNode::Ge {
                            a: Box::new(IrNode::Input {
                                name: "determinism.enabled".to_string(),
                                owner: None,
                            }),
                            b: Box::new(IrNode::Const {
                                value: IrValue::Number(1.0),
                            }),
                        }),
                        b: Box::new(IrNode::Const {
                            value: IrValue::Bool(false),
                        }),
                    }),
                    max: 1.0,
                    edge: None,
                    fire: vec!["determinismReady".to_string()],
                },
                CrossingDescriptor {
                    slot: Some("determinism.accumulated".to_string()),
                    condition: CrossingCondition::Above { threshold: 0.0 },
                    max: 1.0,
                    edge: Some("both".to_string()),
                    fire: vec!["accumulatorEdge".to_string()],
                },
            ],
            &[],
        );
        let trigger_bindings = TriggerBindingTable::build_with_script_ctx(
            &registry.borrow(),
            &trigger_data,
            &trigger_script_ctx,
        );
        let mut crossing_detector = CrossingDetector::new();
        crossing_detector.initialize(
            &trigger_data,
            &trigger_script_ctx.slot_table.borrow(),
            &trigger_script_ctx,
        );
        let mut slot_accumulator_bindings = SlotAccumulatorBindings::default();
        slot_accumulator_bindings.rebuild(&trigger_script_ctx);
        let mut trigger_bridge = TriggerVolumeBridge::new();
        // Keep both pawns inside this harness-only trigger for the entire
        // stream. The determinism gate should pin its initial same-tick IR
        // accumulation, not introduce random-command-dependent re-entry
        // writes after that first pair of activations.
        trigger_bridge.insert_for_test(trigger_source, Vec3::splat(-1_000.0), Vec3::splat(1_000.0));

        let mut labels = HashMap::new();
        for (role, id) in &role_ids {
            labels.insert(*id, EntityLabel::Pawn(*role));
        }
        labels.insert(enemy, EntityLabel::Enemy);
        labels.insert(active_wieldable, EntityLabel::Weapon);
        labels.insert(trigger_source, EntityLabel::TriggerSource);
        labels.insert(trigger_arm_target, EntityLabel::TriggerArmTarget);

        // E18 timed-reaction wiring. Built for every fixture (cheap, no-op when no
        // wait enrolls); the `LevelLoadWait` fixture installs and fires a
        // `levelLoad` sequence so presA runs now and the wait enrolls its tail.
        let note_log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let scheduler = ReactionScheduler::default();
        scheduler.set_enabled(true);
        let mut dispatch_sequence_registry = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut dispatch_sequence_registry, scheduler.clone());
        {
            let note_log = note_log.clone();
            dispatch_sequence_registry.register("note", move |_id, args| {
                if let Some(label) = args.get("label").and_then(serde_json::Value::as_str) {
                    note_log.borrow_mut().push(label.to_string());
                }
                Ok(())
            });
        }
        let dispatch_reaction_registry = ReactionPrimitiveRegistry::new();
        let dispatch_system_registry = SystemReactionRegistry::new();
        let mut dispatch_data = DataRegistry::new();
        if let SimFixture::LevelLoadWait { duration_ms } = fixture {
            let (pre, post) = {
                let mut reg = trigger_script_ctx.registry.borrow_mut();
                (
                    reg.spawn(Transform::default()),
                    reg.spawn(Transform::default()),
                )
            };
            dispatch_data.populate_level(
                vec![NamedReaction {
                    name: "levelLoad".to_string(),
                    descriptor: ReactionDescriptor::Sequence(vec![
                        SequenceStep {
                            id: SequenceTarget::Entity(pre),
                            primitive: "note".to_string(),
                            args: serde_json::json!({ "label": "presA" }),
                        },
                        SequenceStep {
                            id: SequenceTarget::Wait,
                            primitive: "wait".to_string(),
                            args: serde_json::json!({
                                "durationMs": duration_ms,
                                "interruptible": false
                            }),
                        },
                        SequenceStep {
                            id: SequenceTarget::Entity(post),
                            primitive: "note".to_string(),
                            args: serde_json::json!({ "label": "presB" }),
                        },
                    ]),
                }],
                Vec::new(),
                &[],
            );
            // Install-time fire: presA runs now; the wait enrolls its tail with the
            // scheduler at frame counter 0 and stops the drain before presB.
            let chained = fire_named_event_with_sequences(
                "levelLoad",
                &dispatch_data,
                &dispatch_sequence_registry,
                &dispatch_reaction_registry,
                &dispatch_system_registry,
                &trigger_script_ctx,
                None,
            );
            debug_assert!(
                chained.is_empty(),
                "presentation-only levelLoad chains nothing"
            );
        }

        Self {
            registry,
            world: determinism_world(),
            hit_zones: HitZoneStore::new(),
            active_wieldable,
            progress: ProgressTracker::new(),
            ai_runtime: crate::scripting_systems::ai::AiRuntime::new(),
            mover_colliders: Vec::new(),
            mover_states: MoverTickStateTable::default(),
            trigger_system: TriggerSystem::default(),
            trigger_bridge,
            trigger_bindings,
            trigger_script_ctx,
            trigger_slots,
            crossing_detector,
            slot_accumulator_bindings,
            tick_index: 0,
            role_ids,
            labels,
            selected_player,
            remote_player,
            enemy,
            trigger_arm_target,
            trap_pool_source_selected,
            scheduler,
            dispatch_data,
            dispatch_sequence_registry,
            dispatch_reaction_registry,
            dispatch_system_registry,
            note_log,
        }
    }

    pub(crate) fn tick(&mut self, command: RecordedCommand) -> RecordedTick {
        self.tick_index += 1;
        if self.tick_index == 301 {
            self.trigger_slots
                .borrow_mut()
                .get_mut("determinism.rate")
                .expect("determinism accumulator rate remains declared")
                .value = Some(SlotValue::Number(-1.0));
        }
        let mut sim_command = command.to_sim_command();
        // The first shell fires from slot zero. Complete a zero-duration switch
        // well before the next shell, so the second same-archetype instance
        // samples with slot one's salt.
        if self.tick_index == 100 {
            sim_command.select_slot = Some(1);
        }
        let remote_pawn_commands = [RemotePawnCommand {
            pawn: self.remote_player,
            owner_client_id: 1,
            weapon: None,
            shot_id: None,
            fire_tick: 0,
            client_tick: 0,
            aim_pitch: 0.0,
            command: command.to_sim_command(),
        }];
        let trigger_use_edges = HashMap::new();
        let events = simulate_tick(
            self.registry.clone(),
            &self.world,
            &self.hit_zones,
            None,
            GRAVITY,
            Some(self.active_wieldable),
            0.0,
            &mut self.progress,
            &mut self.ai_runtime,
            &self.mover_colliders,
            &mut self.mover_states,
            &remote_pawn_commands,
            &sim_command,
            |_| command.to_post_movement_command(),
            DT,
            Some(super::TriggerTickContext {
                system: &mut self.trigger_system,
                bridge: &self.trigger_bridge,
                bindings: &self.trigger_bindings,
                slot_table: self.trigger_slots.clone(),
                script_ctx: Some(self.trigger_script_ctx.clone()),
                auto_close_timers: None,
                use_edges: &trigger_use_edges,
            }),
            |_| {},
        );
        evaluate_slot_accumulators(&mut self.slot_accumulator_bindings, DT);
        // Advance timed-reaction countdowns for this tick, cancelling any
        // interruptible instance whose paired Exit fired this tick before the
        // countdown advances (O4). Empty (no-op) for the determinism fixture; the
        // enrollment-frame stamp keeps a just-enrolled instance from advancing.
        self.scheduler.evaluate(&events.trigger_exit_fires);
        let predicate_crossing_fires = self
            .crossing_detector
            .detect(&self.trigger_slots.borrow())
            .into_iter()
            .map(|fire| (fire.reaction, fire.rising))
            .collect();
        self.record(events, predicate_crossing_fires)
    }

    /// Run one production-shaped frame: advance the scheduler after install but
    /// before gameplay, run one `tick` per supplied command, then drain timed
    /// landings through the shipped residual path. This matches the redraw order:
    /// install-time `levelLoad` enrollments advance on the first post-install
    /// tick, while later same-frame UI enrollments stamp the current frame and are
    /// skipped until the next one.
    pub(crate) fn frame(&mut self, commands: &[RecordedCommand]) -> Vec<RecordedTick> {
        self.scheduler.begin_frame();
        let mut ticks = Vec::with_capacity(commands.len());
        for command in commands {
            ticks.push(self.tick(*command));
        }
        // Resume landings through the shipped residual path, one instance at a
        // time with its own deferred-dispatch call and its own resume context —
        // exactly as `main.rs` does, so depth attribution (O65) matches.
        self.scheduler.drain_landings(
            &self.dispatch_data,
            &self.dispatch_sequence_registry,
            &self.dispatch_reaction_registry,
            &self.dispatch_system_registry,
            &self.trigger_script_ctx,
        );
        ticks
    }

    /// Snapshot of the `note` step log — presA at install, presB once its tail
    /// lands. Ascending call order proves the landing frame.
    pub(crate) fn note_log(&self) -> Vec<String> {
        self.note_log.borrow().clone()
    }

    /// E18 Task 7 (O25): enroll an instance directly, bypassing the
    /// control-arm dispatch, with a one-step `note(address)` tail so its
    /// landing is order-observable through `note_log`. Test-only.
    ///
    /// The `note` step's target must be an entity in `trigger_script_ctx`'s
    /// registry — the scheduler's dispatch calls (`drain_landings`) resolve
    /// steps against `self.trigger_script_ctx`, a SEPARATE `ScriptCtx` /
    /// `EntityRegistry` from `self.registry` where `selected_player` and the
    /// other gameplay entities live (see the `LevelLoadWait` fixture branch
    /// above, which spawns its own `pre`/`post` entities into
    /// `trigger_script_ctx.registry` for the same reason). Using
    /// `selected_player` here would fail `dispatch_sequence`'s
    /// `script_ctx.registry.borrow().exists(id)` guard and silently skip the
    /// step with a warn.
    #[cfg(test)]
    pub(crate) fn enroll_for_test(
        &self,
        address: &str,
        body_ordinal: usize,
        origin: Option<(EntityId, PlayerId)>,
        ticks: u32,
    ) {
        let target = self
            .trigger_script_ctx
            .registry
            .borrow_mut()
            .spawn(Transform::default());
        let step = SequenceStep {
            id: SequenceTarget::Entity(target),
            primitive: "note".to_string(),
            args: serde_json::json!({ "label": address }),
        };
        self.scheduler
            .enroll(address, body_ordinal, origin, vec![step], ticks, false);
    }

    /// E18 Task 7 (O25): the `note_log` snapshot, named for its use as a
    /// landing-order witness at this call site. Test-only.
    #[cfg(test)]
    pub(crate) fn landed_order_for_test(&self) -> Vec<String> {
        self.note_log()
    }

    /// Resolve every raw id a tick reports before it reaches the comparison.
    pub(crate) fn record(
        &self,
        events: TickEvents,
        predicate_crossing_fires: Vec<(String, bool)>,
    ) -> RecordedTick {
        RecordedTick {
            movement: events.movement,
            ai: events.ai,
            weapon: events.weapon,
            weapon_impact_points: events.weapon_impact_points,
            death: events.death,
            authorized_shots: events
                .authorized_shots
                .iter()
                .map(|open| RecordedShot {
                    shot_id: open.shot.shot_id,
                    owner_client_id: open.owner_client_id,
                    pawn: self.label(open.shot.pawn),
                    weapon: self.label(open.shot.weapon),
                    fire_tick: open.shot.fire_tick,
                    damage: open.shot.damage,
                    range: open.shot.range,
                    pellet_count: open.shot.pellet_count,
                    credit_source: open.shot.credit_source.clone(),
                })
                .collect(),
            reload_deliveries: events
                .reload_deliveries
                .iter()
                .map(|delivery| RecordedReload {
                    pawn: self.label(delivery.pawn),
                    weapon: self.label(delivery.weapon),
                })
                .collect(),
            trigger_residuals: events
                .trigger_residuals
                .iter()
                .map(|(handle, _, _)| *handle)
                .collect(),
            trigger_exit_fires: events
                .trigger_exit_fires
                .iter()
                .map(|(trigger, player)| (self.label(*trigger), self.player_label(*player)))
                .collect(),
            trigger_fires: events
                .trigger_fires
                .iter()
                .map(|event| self.record_trigger_fire(event))
                .collect(),
            trigger_command_fires: events
                .trigger_command_fires
                .iter()
                .map(|fire| RecordedCommandFire {
                    fire: self.record_trigger_fire(&fire.event),
                    commands: fire.commands.clone(),
                })
                .collect(),
            predicate_crossing_fires,
            accumulator_slot: self
                .trigger_slots
                .borrow()
                .get("determinism.accumulated")
                .and_then(|record| record.value.clone()),
        }
    }

    fn record_trigger_fire(&self, event: &TriggerEvent) -> RecordedTriggerFire {
        RecordedTriggerFire {
            trigger: self.label(event.fire.trigger),
            player: self.player_label(event.fire.player),
            event_name: event.fire.event_name.clone(),
            edge: event.edge,
        }
    }

    fn label(&self, id: EntityId) -> EntityLabel {
        *self
            .labels
            .get(&id)
            .expect("recorded events only reference entities this harness spawned")
    }

    fn player_label(&self, player: PlayerId) -> PlayerLabel {
        match player {
            PlayerId::Local(pawn) => match self.label(pawn) {
                EntityLabel::Pawn(role) => PlayerLabel::Local(role),
                other => panic!("local player must resolve to a spawned pawn, got {other:?}"),
            },
            PlayerId::Remote(client_id) => PlayerLabel::Remote(client_id),
        }
    }

    fn role_outcomes(&self) -> Vec<(Role, PawnOutcome)> {
        let registry = self.registry.borrow();
        let mut outcomes = self
            .role_ids
            .iter()
            .map(|(role, id)| {
                let transform = *registry
                    .get_component::<Transform>(*id)
                    .expect("role entity must keep its transform");
                let movement = registry
                    .get_component::<PlayerMovementComponent>(*id)
                    .expect("role entity must keep its movement component");
                (
                    *role,
                    PawnOutcome {
                        position: transform.position,
                        velocity: movement.velocity,
                    },
                )
            })
            .collect::<Vec<_>>();
        outcomes.sort_by_key(|(role, _)| *role);
        outcomes
    }

    /// Per-pawn health after the run, keyed by stable test role. The damage
    /// ledger the determinism gate compares: a spawn-order-independent view of
    /// what the presser (and any other deterministic damage) left each pawn at.
    fn role_health_ledger(&self) -> Vec<(Role, f32)> {
        let registry = self.registry.borrow();
        let mut ledger = self
            .role_ids
            .iter()
            .map(|(role, id)| {
                let health = registry
                    .get_component::<HealthComponent>(*id)
                    .expect("role entity must keep its health component")
                    .current;
                (*role, health)
            })
            .collect::<Vec<_>>();
        ledger.sort_by_key(|(role, _)| *role);
        ledger
    }

    fn selected_player_health(&self) -> f32 {
        self.registry
            .borrow()
            .get_component::<HealthComponent>(self.selected_player)
            .expect("selected player keeps health")
            .current
    }

    fn enemy_state(&self) -> String {
        self.registry
            .borrow()
            .get_component::<BrainComponent>(self.enemy)
            .expect("enemy keeps brain")
            .state_name()
            .expect("enemy sits in a declared graph state")
            .to_string()
    }

    fn trigger_slot(&self) -> Option<SlotValue> {
        self.trigger_slots
            .borrow()
            .get("determinism.triggered")
            .and_then(|record| record.value.clone())
    }

    fn trigger_arm_target_armed(&self) -> bool {
        self.registry
            .borrow()
            .get_component::<TriggerVolumeComponent>(self.trigger_arm_target)
            .expect("determinism arm target remains present")
            .armed
    }
}

fn determinism_trigger_slots() -> SlotTable {
    let mut slots = SlotTable::new();
    slots
        .insert_namespace(
            "determinism",
            vec![
                (
                    "triggered".to_string(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(0.0)),
                        range: None,
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: ReplicationScope::None,
                        per_owner: false,
                        accumulate: None,
                    }),
                ),
                (
                    "enabled".to_string(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(1.0)),
                        range: None,
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: ReplicationScope::None,
                        per_owner: false,
                        accumulate: None,
                    }),
                ),
                (
                    "accumulated".to_string(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(-2.0)),
                        range: Some(postretro_entities::NumericRange {
                            min: -10.0,
                            max: 10.0,
                        }),
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: ReplicationScope::None,
                        per_owner: false,
                        accumulate: Some(IrNode::Mul {
                            a: Box::new(IrNode::Input {
                                name: "@dt".to_string(),
                                owner: None,
                            }),
                            b: Box::new(IrNode::Input {
                                name: "determinism.rate".to_string(),
                                owner: None,
                            }),
                        }),
                    }),
                ),
                (
                    "rate".to_string(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(1.0)),
                        range: None,
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: ReplicationScope::None,
                        per_owner: false,
                        accumulate: None,
                    }),
                ),
            ],
        )
        .expect("determinism namespace is unique");
    slots
}

fn deterministic_trigger_primitive(
    primitive: &str,
    tag: Option<&str>,
    args: serde_json::Value,
) -> NamedReaction {
    NamedReaction {
        name: "determinismTrigger".to_string(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: primitive.to_string(),
            target: None,
            tag: tag.map(str::to_string),
            on_complete: None,
            args,
        }),
    }
}

/// The presser half of the determinism trigger: `damage(on.activators, amount)`.
/// Unlike the tag/system primitives above it targets the fire's activator pawns
/// through the `@activators` sentinel, so both pressers take the hit each run.
fn deterministic_trigger_activator_damage(amount: f32) -> NamedReaction {
    NamedReaction {
        name: "determinismTrigger".to_string(),
        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
            primitive: "applyDamage".to_string(),
            target: Some("@activators".to_string()),
            tag: None,
            on_complete: None,
            args: serde_json::json!({ "amount": amount }),
        }),
    }
}

fn spawn_player(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
    let id = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    registry
        .set_component(
            id,
            PlayerMovementComponent::from_descriptor(&player_descriptor()),
        )
        .expect("player movement component should attach");
    registry
        .set_component(
            id,
            HealthComponent {
                max: 100.0,
                current: 100.0,
                hitbox: Some(Hitbox {
                    half_extents: Vec3::splat(0.5),
                    offset: Vec3::ZERO,
                }),
                death_handled: false,
                pending_kill_credit: None,
                zone_multipliers: Default::default(),
                contributor_ledger: Default::default(),
            },
        )
        .expect("player health component should attach");
    id
}

fn spawn_enemy(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
    let id = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    let mut brain = BrainComponent::from_graph(&enemy_graph(0.0, "alert"));
    brain.home_anchor = position;
    registry
        .set_component(id, brain)
        .expect("enemy brain component should attach");
    registry
        .entity_state_mut(id)
        .expect("spawned enemy carries entity state")
        .set(
            crate::scripting_systems::ai::FACTION_STATE_FIELD,
            crate::scripting_systems::ai::ENEMY_DEFAULT_FACTION,
        );
    registry
        .set_component(
            id,
            HealthComponent {
                max: 20.0,
                current: 20.0,
                hitbox: Some(Hitbox {
                    half_extents: Vec3::splat(0.5),
                    offset: Vec3::ZERO,
                }),
                death_handled: false,
                pending_kill_credit: None,
                zone_multipliers: Default::default(),
                contributor_ledger: Default::default(),
            },
        )
        .expect("enemy health component should attach");
    id
}

fn spawn_target(registry: &mut EntityRegistry, position: Vec3) -> EntityId {
    let id = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    registry
        .set_component(
            id,
            HealthComponent {
                max: 20.0,
                current: 20.0,
                hitbox: Some(Hitbox {
                    half_extents: Vec3::splat(0.5),
                    offset: Vec3::ZERO,
                }),
                death_handled: false,
                pending_kill_credit: None,
                zone_multipliers: Default::default(),
                contributor_ledger: Default::default(),
            },
        )
        .expect("target health component should attach");
    id
}

fn spawn_weapon(registry: &mut EntityRegistry) -> EntityId {
    let id = registry.spawn(Transform::default());
    registry
        .set_component(
            id,
            WeaponComponent::from_descriptor(&WeaponDescriptor {
                damage: 10.0,
                pellet_count: 1,
                spread_degrees: 0.0,
                range: 30.0,
                cooldown_ms: 80.0,
                fire_mode: FireMode::Semi,
                resolution: ResolutionMode::Hitscan,
                projectile: None,
                credit_source: None,
                third_person_model: None,
                viewmodel: None,
                placement: None,
                muzzle_offset: None,
                resource: None,
                lower_ms: 0,
                raise_ms: 0,
                block_during_reload: None,
            }),
        )
        .expect("weapon component should attach");
    id
}

fn spawn_determinism_weapon(registry: &mut EntityRegistry) -> EntityId {
    let weapon = spawn_weapon(registry);
    let mut component = registry
        .get_component::<WeaponComponent>(weapon)
        .expect("determinism weapon component attaches")
        .clone();
    component.pellet_count = 8;
    component.spread_degrees = 4.0;
    registry
        .set_component(weapon, component)
        .expect("determinism weapon tuning updates");
    registry
        .set_component(
            weapon,
            DescriptorProvenance {
                canonical_name: "weapon.determinism-shotgun".to_string(),
                owned_components: std::collections::BTreeSet::from([
                    DescriptorComponentKind::Weapon,
                ]),
                map_overrides: Default::default(),
                spawn_path: DescriptorSpawnPath::DefaultWeapon,
            },
        )
        .expect("determinism weapon provenance attaches");
    weapon
}

/// Install the local ownership relationship the weapon stage resolves at runtime.
/// `simulate_tick` no longer uses its retired legacy wieldable argument.
fn spawn_local_active_weapon(registry: &mut EntityRegistry) -> EntityId {
    let pawn = spawn_player(registry, Vec3::ZERO);
    registry
        .mark_local_player_pawn(pawn)
        .expect("test pawn can be marked local");
    let weapon = spawn_weapon(registry);
    let mut inventory = Inventory::default();
    inventory.wieldables[0] = Some(weapon);
    registry
        .set_component(pawn, inventory)
        .expect("test pawn inventory should attach");
    weapon
}

fn player_descriptor() -> PlayerMovementDescriptor {
    PlayerMovementDescriptor {
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
        forgiveness: Some(ForgivenessParams {
            coyote_ms: 0.0,
            jump_buffer_ms: 0.0,
        }),
        crouch: None,
        slide: None,
        view_feel: None,
    }
}

fn floor_world() -> CollisionWorld {
    sloped_floor_world(0.0)
}

/// The regular floor plus a broad vertical backstop. The fixed command stream
/// aims horizontally, so this gives every spread pellet an observable world
/// impact rather than silently missing above the floor.
fn determinism_world() -> CollisionWorld {
    let points = vec![
        Point::new(-500.0, 0.0, -500.0),
        Point::new(500.0, 0.0, -500.0),
        Point::new(500.0, 0.0, 500.0),
        Point::new(-500.0, 0.0, 500.0),
        Point::new(-500.0, 0.0, -40.0),
        Point::new(500.0, 0.0, -40.0),
        Point::new(500.0, 500.0, -40.0),
        Point::new(-500.0, 500.0, -40.0),
    ];
    let triangles = vec![[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7]];
    CollisionWorld {
        mesh: TriMesh::new(points, triangles),
        isometry: Isometry::identity(),
    }
}

/// A large ground plane tilted about the world Z axis: surface height `y =
/// slope * x`. `slope == 0.0` is the flat [`floor_world`]. The triangle winding
/// matches `floor_world`, so the upward-facing normal is `(-slope, 1, 0)`
/// normalized — a non-`Vec3::Y` tilt for any non-zero slope, staying walkable
/// while `1/sqrt(1 + slope^2) >= COS_WALKABLE`.
fn sloped_floor_world(slope: f32) -> CollisionWorld {
    let y = |x: f32| slope * x;
    let points = vec![
        Point::new(-500.0, y(-500.0), -500.0),
        Point::new(500.0, y(500.0), -500.0),
        Point::new(500.0, y(500.0), 500.0),
        Point::new(-500.0, y(-500.0), 500.0),
    ];
    let triangles = vec![[0, 2, 1], [0, 3, 2]];
    CollisionWorld {
        mesh: TriMesh::new(points, triangles),
        isometry: Isometry::identity(),
    }
}

fn open_floor_nav_graph() -> NavGraph {
    NavGraph::from_section(&NavMeshSection {
        version: NAVMESH_VERSION,
        origin: [0.0, 0.0, 0.0],
        cell_size: 1.0,
        dim_x: 64,
        dim_z: 64,
        agent_radius: 0.35,
        agent_height: 1.8,
        step_height: 0.4,
        max_slope_deg: 45.0,
        regions: vec![NavRegion {
            x0: 0,
            z0: 0,
            x1: 64,
            z1: 64,
            floor_y_min: 0.0,
            floor_y_max: 0.25,
        }],
        portals: vec![],
    })
}

/// A direct-graph brain staged directly into one of its declared states.
fn brain_in_state(graph: &BehaviorGraphDescriptor, state: &str) -> BrainComponent {
    let mut brain = BrainComponent::from_graph(graph);
    assert!(
        brain.enter_activity_at(
            0,
            brain
                .graph
                .envelope
                .activities
                .keys()
                .position(|name| name == state)
                .expect("the graph declares the activity"),
        )
    );
    brain
}

fn enemy_graph(move_speed: f32, locomotion_animation: &str) -> BehaviorGraphDescriptor {
    BehaviorGraphDescriptor {
        envelope: BehaviorGraphEnvelope {
            initial: "idle".to_string(),
            activities: std::collections::BTreeMap::from([
                (
                    "idle".to_string(),
                    BehaviorActivityDescriptor {
                        animation: Some("idle".to_string()),
                        motion: Some(MotionVerb::Hold),
                        action: None,
                        on_enter: None,
                        layers: BTreeMap::new(),
                    },
                ),
                (
                    ALERT_STATE.to_string(),
                    BehaviorActivityDescriptor {
                        animation: Some(locomotion_animation.to_string()),
                        motion: Some(MotionVerb::ChaseTarget),
                        action: None,
                        on_enter: None,
                        layers: BTreeMap::new(),
                    },
                ),
                (
                    ATTACK_STATE.to_string(),
                    BehaviorActivityDescriptor {
                        animation: Some("attack".to_string()),
                        motion: Some(MotionVerb::ChaseTarget),
                        action: Some(ActionVerb::Attack("attack".to_string())),
                        on_enter: None,
                        layers: BTreeMap::new(),
                    },
                ),
                (
                    "death".to_string(),
                    BehaviorActivityDescriptor {
                        animation: Some("death".to_string()),
                        motion: Some(MotionVerb::Freeze),
                        action: None,
                        on_enter: None,
                        layers: BTreeMap::new(),
                    },
                ),
            ]),
            transitions: BTreeMap::new(),
        },
        candidate_filter: None,
        patrol: None,
        attacks: BTreeMap::from([(
            "attack".to_string(),
            AttackParams {
                damage: 7.0,
                max_range: 2.0,
                cooldown_ms: 1000.0,
                engagement_radius: None,
                standoff_distance: None,
            },
        )]),
        engagement_radius: None,
        move_speed,
    }
}

fn driven_agent_graph() -> BehaviorGraphDescriptor {
    enemy_graph(3.5, "locomotion")
}

fn driven_agent_mesh(current_state: &str) -> MeshComponent {
    let state = |clip: &str, clip_index| AnimationState {
        clip: clip.to_string(),
        looping: true,
        crossfade_ms: 0.0,
        interrupt: InterruptPolicy::Smooth,
        travel_speed: None,
        clip_index: Some(clip_index),
    };
    let mut states = std::collections::HashMap::new();
    states.insert("idle".to_string(), state("idle", 0));
    states.insert("locomotion".to_string(), state("walk", 1));
    states.insert("attack".to_string(), state("attack", 2));
    states.insert("death".to_string(), state("death", 3));

    MeshComponent::animated(
        "driven-agent".to_string(),
        MeshAnimation::new(states, current_state.to_string()),
    )
}

/// Spawn a behavior-graph enemy staged directly into one of its declared states.
fn spawn_driven_agent(
    registry: &mut EntityRegistry,
    position: Vec3,
    state: &str,
    animation_state: &str,
) -> EntityId {
    let enemy = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    let mut brain = brain_in_state(&driven_agent_graph(), state);
    brain.home_anchor = position;
    registry
        .set_component(enemy, brain)
        .expect("driven agent brain should attach");
    registry
        .entity_state_mut(enemy)
        .expect("driven agent carries entity state")
        .set(
            crate::scripting_systems::ai::FACTION_STATE_FIELD,
            crate::scripting_systems::ai::ENEMY_DEFAULT_FACTION,
        );
    registry
        .set_component(enemy, AgentComponent::new(0.35, 1.8, 0.4, 3.5))
        .expect("driven agent steering component should attach");
    registry
        .set_component(enemy, driven_agent_mesh(animation_state))
        .expect("driven agent mesh should attach");
    enemy
}

#[test]
fn determinism_enemy_helpers_seed_home_anchor_from_spawn_position() {
    let mut registry = EntityRegistry::new();
    let enemy_position = Vec3::new(-3.0, 1.0, 2.0);
    let driven_position = Vec3::new(4.0, 1.0, -5.0);
    let enemy = spawn_enemy(&mut registry, enemy_position);
    let driven = spawn_driven_agent(&mut registry, driven_position, ALERT_STATE, "locomotion");

    assert_eq!(
        registry
            .get_component::<BrainComponent>(enemy)
            .expect("enemy keeps its brain")
            .home_anchor,
        enemy_position
    );
    assert_eq!(
        registry
            .get_component::<BrainComponent>(driven)
            .expect("driven agent keeps its brain")
            .home_anchor,
        driven_position
    );
}

#[allow(clippy::too_many_arguments)]
fn run_driven_agent_sim_tick(
    registry: Rc<RefCell<EntityRegistry>>,
    world: &CollisionWorld,
    hit_zones: &HitZoneStore,
    nav_graph: &NavGraph,
    anim_time: f64,
    progress: &mut ProgressTracker,
    ai_runtime: &mut crate::scripting_systems::ai::AiRuntime,
    mover_states: &mut MoverTickStateTable,
) {
    let command = SimCommand {
        movement: MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            use_pressed: false,
            drop_pressed: false,
        },
        fire_button: FireButtonState {
            pressed: false,
            active: false,
        },
        reload: false,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    };
    let _ = simulate_tick(
        registry,
        world,
        hit_zones,
        Some(nav_graph),
        GRAVITY,
        None,
        anim_time,
        progress,
        ai_runtime,
        &[],
        mover_states,
        &[],
        &command,
        |_| super::PostMovementCommand {
            aim_origin: Vec3::ZERO,
            aim_direction: Vec3::Z,
        },
        DT,
        None,
        |_| {},
    );
}

fn reference_enemy_walking_hit_zones() -> Option<(String, usize, HitZoneStore)> {
    use crate::scripting_systems::hit_zones::ModelHitZones;
    use postretro_model::ModelHandle;

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/dev/models/reference_enemy_kaykit_knight/scene.gltf");
    if !path.exists() {
        eprintln!("skipping: model asset not present at {}", path.display());
        return None;
    }
    let model = postretro_model::gltf_loader::load_model(&path)
        .expect("shipped reference enemy model loads");
    let walking_index = model
        .clips
        .iter()
        .position(|clip| clip.name == "Walking_A")
        .expect("reference enemy declares Walking_A");
    assert_eq!(
        model.clips[walking_index].travel_speed, None,
        "E10 fallback is justified only when the shipped Walking_A clip derives no travel speed",
    );

    let model_key = path.to_string_lossy().into_owned();
    let mut store = HitZoneStore::new();
    store.insert_for_test(
        ModelHandle::from(model_key.clone()),
        ModelHitZones {
            skeleton: Arc::new(model.skeleton),
            clips: Arc::new(model.clips),
            joint_zones: model.joint_zones,
            sockets: model.sockets,
            derived_bound: None,
            legs: model.legs,
            pose_stack: Arc::new(model.pose_stack),
        },
    );
    Some((model_key, walking_index, store))
}

#[test]
fn simulate_tick_scales_walk_rate_from_post_steering_velocity_and_skips_sub_epsilon_writes() {
    let world = floor_world();
    let nav_graph = open_floor_nav_graph();
    let mut progress = ProgressTracker::new();
    let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
    let mut mover_states = MoverTickStateTable::default();
    let empty_hit_zones = HitZoneStore::new();

    // Attack is not the alert-mapped locomotion state, so the sim-tick path
    // must restore its previously scaled playback rate to one.
    let non_walk_registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let non_walk_enemy = {
        let mut registry = non_walk_registry.borrow_mut();
        spawn_player(&mut registry, Vec3::new(7.0, 1.21, 5.0));
        let enemy = spawn_driven_agent(
            &mut registry,
            Vec3::new(5.0, 1.21, 5.0),
            ATTACK_STATE,
            "attack",
        );
        let mut mesh = registry
            .get_component::<MeshComponent>(enemy)
            .expect("driven agent keeps mesh")
            .clone();
        mesh.animation.as_mut().unwrap().rate = RATE_MIN;
        registry
            .set_component(enemy, mesh)
            .expect("scaled non-walk mesh should update");
        enemy
    };
    run_driven_agent_sim_tick(
        non_walk_registry.clone(),
        &world,
        &empty_hit_zones,
        &nav_graph,
        1.0,
        &mut progress,
        &mut ai_runtime,
        &mut mover_states,
    );
    assert_eq!(
        non_walk_registry
            .borrow()
            .get_component::<MeshComponent>(non_walk_enemy)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .rate,
        1.0,
        "non-walk states rest at the authored playback rate",
    );

    let Some((reference_model, walking_index, hit_zones)) = reference_enemy_walking_hit_zones()
    else {
        return;
    };
    // This is a separate registry whose entity ids begin again at the same
    // values as the non-walk setup above. A runtime cache is registry-scoped,
    // so use a fresh one rather than intentionally treating this new entity as
    // a hot graph replacement.
    ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let enemy = {
        let mut registry = registry.borrow_mut();
        // Keep acquisition in its near band so this isolates same-tick
        // steering/rate scaling from acquisition-stride timing.
        spawn_player(&mut registry, Vec3::new(10.0, 1.21, 5.0));
        let enemy = spawn_driven_agent(
            &mut registry,
            Vec3::new(5.0, 1.21, 5.0),
            ALERT_STATE,
            "locomotion",
        );
        let mut mesh = registry
            .get_component::<MeshComponent>(enemy)
            .expect("driven agent keeps mesh")
            .clone();
        mesh.model = reference_model;
        let locomotion = mesh
            .animation
            .as_mut()
            .expect("driven agent keeps animation")
            .states
            .get_mut("locomotion")
            .expect("driven agent declares locomotion state");
        locomotion.clip = "Walking_A".to_string();
        locomotion.clip_index = Some(walking_index);
        registry
            .set_component(enemy, mesh)
            .expect("reference enemy walk mesh should update");
        enemy
    };

    // The first tick builds the route and drives actual steering. The second
    // tick makes the AI select the walk state from that driven velocity.
    run_driven_agent_sim_tick(
        registry.clone(),
        &world,
        &hit_zones,
        &nav_graph,
        1.0,
        &mut progress,
        &mut ai_runtime,
        &mut mover_states,
    );
    run_driven_agent_sim_tick(
        registry.clone(),
        &world,
        &hit_zones,
        &nav_graph,
        2.0,
        &mut progress,
        &mut ai_runtime,
        &mut mover_states,
    );
    {
        let registry = registry.borrow();
        let agent = registry
            .get_component::<AgentComponent>(enemy)
            .expect("driven agent keeps steering state");
        let mesh = registry
            .get_component::<MeshComponent>(enemy)
            .expect("driven agent keeps mesh");
        let animation = mesh.animation.as_ref().unwrap();
        let speed_xz = Vec3::new(agent.velocity.x, 0.0, agent.velocity.z).length();
        // E10 premise: the shipped Walking_A clip is authored in-place, so Task
        // 2 derives no travel speed and this state declares no override. This
        // uses the actual shipped model store rather than an empty-store stand-in.
        assert_eq!(
            super::effective_travel_speed(animation, mesh, &hit_zones),
            None,
            "in-place E10 walk resolves to no effective travel speed (degenerate fallback)",
        );
        let expected_rate = (speed_xz / agent.move_speed).clamp(RATE_MIN, RATE_MAX);
        assert_eq!(animation.current_state, "locomotion");
        assert!(speed_xz > 0.0, "scenario must be driven by steering");
        assert!(
            (animation.rate - expected_rate).abs() <= 1.0e-6,
            "walk rate must use this tick's resolved steering velocity",
        );
    }

    let before = {
        let mut registry = registry.borrow_mut();
        let mut mesh = registry
            .get_component::<MeshComponent>(enemy)
            .expect("driven agent keeps mesh")
            .clone();
        let animation = mesh.animation.as_mut().unwrap();
        animation.rate = RATE_MIN + RATE_CHANGE_EPSILON * 0.5;
        animation.rebase_time = Some(2.0);
        animation.rebase_elapsed = 1.0;
        registry
            .set_component(enemy, mesh)
            .expect("sub-epsilon setup should update mesh");
        registry
            .get_component::<MeshComponent>(enemy)
            .expect("driven agent keeps mesh")
            .animation
            .clone()
    };
    run_driven_agent_sim_tick(
        registry.clone(),
        &world,
        &hit_zones,
        &nav_graph,
        3.0,
        &mut progress,
        &mut ai_runtime,
        &mut mover_states,
    );
    assert_eq!(
        registry
            .borrow()
            .get_component::<MeshComponent>(enemy)
            .expect("driven agent keeps mesh")
            .animation
            .as_ref(),
        before.as_ref(),
        "a sub-epsilon post-steering rate change must leave rebase state untouched",
    );
}

/// Build a hit-zone store whose `model` entry carries `clip_index + 1` clips,
/// with the clip at `clip_index` stamped with `travel_speed` — standing in for a
/// model whose walk clip did (or did not) derive a stride from root motion at
/// load. The skeleton/zones are empty: the sim tick never raycasts this store,
/// it only reads the clip's derived travel speed.
fn hit_zone_store_with_clip_travel_speed(
    model: &str,
    clip_index: usize,
    travel_speed: Option<f32>,
) -> HitZoneStore {
    use crate::scripting_systems::hit_zones::ModelHitZones;
    use postretro_model::skeleton::{AnimationClip, Skeleton};
    use std::sync::Arc;

    let mut clips: Vec<AnimationClip> =
        (0..=clip_index).map(|_| AnimationClip::default()).collect();
    clips[clip_index].travel_speed = travel_speed;
    let mut store = HitZoneStore::new();
    store.insert_for_test(
        postretro_model::ModelHandle::from(model.to_string()),
        ModelHitZones {
            skeleton: Arc::new(Skeleton::default()),
            clips: Arc::new(clips),
            joint_zones: Vec::new(),
            sockets: std::collections::HashMap::new(),
            derived_bound: None,
            legs: Vec::new(),
            pose_stack: Arc::new(postretro_model::pose_modifier::PoseModifierStack::default()),
        },
    );
    store
}

// A locomotion state whose clip carries a load-derived travel speed calibrates
// playback to `measured_ground_speed / travel_speed`, not `speed_xz / move_speed`
// — moving faster than the authored stride plays it proportionally faster.
#[test]
fn update_brain_playback_rate_scales_from_clip_travel_speed() {
    let mut registry = EntityRegistry::new();
    let enemy = spawn_driven_agent(
        &mut registry,
        Vec3::new(5.0, 1.21, 5.0),
        ALERT_STATE,
        "locomotion",
    );
    // Force a known post-steering velocity; the producer reads
    // `path_state().velocity` directly, so no steering run is needed here.
    let mut agent = registry
        .get_component::<AgentComponent>(enemy)
        .unwrap()
        .clone();
    agent.velocity = Vec3::new(3.0, 0.0, 0.0);
    registry.set_component(enemy, agent).unwrap();

    // Walk clip index 1 (the "locomotion" state's clip) authors a 2.5 u/s stride.
    let store = hit_zone_store_with_clip_travel_speed("driven-agent", 1, Some(2.5));
    super::update_brain_animation_playback_rates(&mut registry, &store, 1.0);

    let rate = registry
        .get_component::<MeshComponent>(enemy)
        .unwrap()
        .animation
        .as_ref()
        .unwrap()
        .rate;
    let expected = (3.0f32 / 2.5).clamp(RATE_MIN, RATE_MAX);
    assert!(
        (rate - expected).abs() <= 1.0e-6,
        "rate must calibrate to measured/travel_speed ({expected}), got {rate}",
    );
    // Distinct from the degenerate move_speed reference (3.0 / 3.5 ≈ 0.857),
    // proving the clip stride — not `move_speed` — drove the calibration.
    let degenerate = (3.0f32 / 3.5).clamp(RATE_MIN, RATE_MAX);
    assert!(
        (rate - degenerate).abs() > 1.0e-3,
        "travel-speed calibration must differ from the move_speed fallback",
    );
    assert!(
        rate > 1.0,
        "moving faster than the authored stride plays faster"
    );
}

// `speedScale: false` disables rate-scaling entirely: even with a calibrated
// clip stride and a nonzero measured speed, the rate holds at the authored 1.0.
#[test]
fn update_brain_playback_rate_skips_scaling_when_speed_scale_off() {
    let mut registry = EntityRegistry::new();
    let enemy = spawn_driven_agent(
        &mut registry,
        Vec3::new(5.0, 1.21, 5.0),
        ALERT_STATE,
        "locomotion",
    );
    let mut mesh = registry
        .get_component::<MeshComponent>(enemy)
        .unwrap()
        .clone();
    mesh.animation = mesh.animation.map(|anim| anim.with_speed_scale(false));
    registry.set_component(enemy, mesh).unwrap();
    let mut agent = registry
        .get_component::<AgentComponent>(enemy)
        .unwrap()
        .clone();
    agent.velocity = Vec3::new(3.0, 0.0, 0.0);
    registry.set_component(enemy, agent).unwrap();

    // A calibrated stride that would otherwise scale to 1.2 — the assertion that
    // the rate stays 1.0 catches a producer that ignores `speed_scale`.
    let store = hit_zone_store_with_clip_travel_speed("driven-agent", 1, Some(2.5));
    super::update_brain_animation_playback_rates(&mut registry, &store, 1.0);

    let rate = registry
        .get_component::<MeshComponent>(enemy)
        .unwrap()
        .animation
        .as_ref()
        .unwrap()
        .rate;
    assert!(
        (rate - 1.0).abs() <= 1.0e-6,
        "speedScale: false must hold the authored rate, got {rate}",
    );
}

#[test]
fn local_locomotion_rate_precedence_matrix_is_override_then_derived_then_fallback() {
    struct Case {
        label: &'static str,
        override_speed: Option<f32>,
        derived_speed: Option<f32>,
        speed_scale: bool,
        expected: f32,
    }
    let measured = 3.0_f32;
    let move_speed = 3.5_f32;
    let cases = [
        Case {
            label: "override wins over derived",
            override_speed: Some(4.0),
            derived_speed: Some(2.5),
            speed_scale: true,
            expected: measured / 4.0,
        },
        Case {
            label: "derived clip speed",
            override_speed: None,
            derived_speed: Some(2.5),
            speed_scale: true,
            expected: measured / 2.5,
        },
        Case {
            label: "E10 move-speed fallback",
            override_speed: None,
            derived_speed: None,
            speed_scale: true,
            expected: measured / move_speed,
        },
        Case {
            label: "speedScale false",
            override_speed: Some(4.0),
            derived_speed: Some(2.5),
            speed_scale: false,
            expected: 1.0,
        },
    ];

    for case in cases {
        let mut registry = EntityRegistry::new();
        let enemy = spawn_driven_agent(
            &mut registry,
            Vec3::new(5.0, 1.21, 5.0),
            ALERT_STATE,
            "locomotion",
        );
        let mut mesh = registry
            .get_component::<MeshComponent>(enemy)
            .unwrap()
            .clone();
        let animation = mesh.animation.as_mut().unwrap();
        animation.speed_scale = case.speed_scale;
        animation.states.get_mut("locomotion").unwrap().travel_speed = case.override_speed;
        registry.set_component(enemy, mesh).unwrap();
        let mut agent = registry
            .get_component::<AgentComponent>(enemy)
            .unwrap()
            .clone();
        agent.velocity = Vec3::new(measured, 0.0, 0.0);
        registry.set_component(enemy, agent).unwrap();

        let store = hit_zone_store_with_clip_travel_speed("driven-agent", 1, case.derived_speed);
        super::update_brain_animation_playback_rates(&mut registry, &store, 1.0);
        let actual = registry
            .get_component::<MeshComponent>(enemy)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .rate;
        let expected = case.expected.clamp(RATE_MIN, RATE_MAX);
        assert!(
            (actual - expected).abs() <= 1.0e-6,
            "{}: expected {expected}, got {actual}",
            case.label
        );
    }
}

// Regression: host-owned player pawns have no Brain component, so their descriptor
// default used to remain idle on every serialized snapshot while clients predicted
// walking from velocity.
#[test]
fn host_player_locomotion_selects_walk_and_calibrates_rate_before_replication() {
    let mut registry = EntityRegistry::new();
    let pawn = registry.spawn(Transform::default());
    let mut movement = PlayerMovementComponent::from_descriptor(&player_descriptor());
    movement.velocity = Vec3::new(3.0, 0.0, 0.0);
    registry.set_component(pawn, movement).unwrap();

    let mut states = HashMap::new();
    states.insert(
        "idle".to_string(),
        AnimationState {
            clip: "idle".to_string(),
            looping: true,
            crossfade_ms: 50.0,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: None,
            clip_index: Some(0),
        },
    );
    states.insert(
        "walk_forward".to_string(),
        AnimationState {
            clip: "walk".to_string(),
            looping: true,
            crossfade_ms: 50.0,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: Some(2.0),
            clip_index: Some(1),
        },
    );
    registry
        .set_component(
            pawn,
            MeshComponent::animated(
                "player-model".to_string(),
                MeshAnimation::new(states, "idle".to_string()),
            ),
        )
        .unwrap();
    // The normal frame path resolves the descriptor's initial animation stamp
    // before locomotion can crossfade away from it.
    resolve_pending_animation_stamps(&mut registry, 0.5);

    super::update_player_animation_locomotion(&mut registry, &HitZoneStore::new(), 1.0);

    let animation = registry
        .get_component::<MeshComponent>(pawn)
        .unwrap()
        .animation
        .as_ref()
        .unwrap();
    assert_eq!(animation.current_state, "walk_forward");
    assert!(
        animation.previous_state.is_some(),
        "switch uses authored crossfade"
    );
    assert!((animation.rate - 1.5).abs() <= 1.0e-6);

    let mut replicable = crate::netcode::ReplicableSet::new();
    replicable.register(pawn);
    let mut allocator = crate::netcode::NetworkIdAllocator::new();
    let snapshots = crate::netcode::produce_owned_snapshots(
        &registry,
        &replicable,
        &mut allocator,
        &crate::netcode::MovementOwners::new(),
        &crate::netcode::HostCommandQueues::new(),
    );
    assert!(snapshots[0].components.iter().any(|payload| matches!(
        payload,
        postretro_net::wire::ComponentPayload::MeshAnimationState(state)
            if state.current_state == "walk_forward"
    )));
}

// Regression: runtime-spawned host enemies enter the first rate pass before
// the outer app loop drains their clip-index resolve queue.
#[test]
fn spawner_path_first_rate_pass_uses_derived_clip_calibration_before_index_resolve() {
    use crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor;
    use crate::spawner::SpawnContext;
    use postretro_entities::components::spawner::SpawnerComponent;
    use postretro_model::skeleton::{AnimationClip, Skeleton};

    let mut descriptor = behavior_enemy_descriptor("runtime_enemy");
    let mesh_desc = descriptor.mesh.as_mut().unwrap();
    mesh_desc.animations.insert(
        "walk".to_string(),
        AnimationState {
            clip: "walk_clip".to_string(),
            looping: true,
            crossfade_ms: 0.0,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: None,
            clip_index: None,
        },
    );
    mesh_desc.default_state = Some("walk".to_string());
    descriptor
        .behavior
        .as_mut()
        .expect("behavior enemy fixture declares a graph")
        .envelope
        .activities
        .insert(
            "walk".to_string(),
            BehaviorActivityDescriptor {
                animation: Some("walk".to_string()),
                motion: Some(MotionVerb::ChaseTarget),
                action: None,
                on_enter: None,
                layers: BTreeMap::new(),
            },
        );

    let context = SpawnContext::default();
    context.replace_level_data(
        [("runtime_enemy".to_string(), descriptor)]
            .into_iter()
            .collect(),
        None,
    );
    let mut registry = EntityRegistry::new();
    let spawner = registry.spawn(Transform::default());
    registry
        .set_component(
            spawner,
            SpawnerComponent {
                archetype_name: "runtime_enemy".to_string(),
                count: 1,
                resolved: true,
            },
        )
        .unwrap();
    crate::spawner::spawn_from_spawner_targets(&mut registry, &[spawner], &context);
    let enemy = registry
        .iter_with_kind(postretro_entities::ComponentKind::Brain)
        .map(|(id, _)| id)
        .next()
        .expect("spawner path materializes one AI enemy");
    assert_eq!(
        registry
            .get_component::<MeshComponent>(enemy)
            .unwrap()
            .animation
            .as_ref()
            .unwrap()
            .states["walk"]
            .clip_index,
        None,
        "first rate pass occurs before the queued index fill",
    );

    // Enter the graph's locomotion state, which is what the rate pass scales.
    // Spawn seeds `current_state` from the graph's rest animation rather than the
    // mesh `defaultState` (`components::brain::validate_brain_animation_states`),
    // so a freshly spawned enemy is at rest with nothing to rate-scale. Driving
    // the state here is what the brain itself does once the enemy starts chasing,
    // and it keeps this test about calibration ordering rather than spawn seeding.
    let mut mesh = registry
        .get_component::<MeshComponent>(enemy)
        .unwrap()
        .clone();
    mesh.animation.as_mut().unwrap().current_state = "walk".to_string();
    registry.set_component(enemy, mesh).unwrap();

    let mut agent = registry
        .get_component::<AgentComponent>(enemy)
        .unwrap()
        .clone();
    agent.velocity = Vec3::new(3.0, 0.0, 0.0);
    registry.set_component(enemy, agent).unwrap();

    let mut store = HitZoneStore::new();
    store.insert_for_test(
        postretro_model::ModelHandle::from("decraniated"),
        crate::scripting_systems::hit_zones::ModelHitZones {
            skeleton: Arc::new(Skeleton::default()),
            clips: Arc::new(vec![AnimationClip {
                name: "walk_clip".to_string(),
                duration: 1.0,
                joints: Vec::new(),
                travel_speed: Some(2.0),
            }]),
            joint_zones: Vec::new(),
            sockets: std::collections::HashMap::new(),
            derived_bound: None,
            legs: Vec::new(),
            pose_stack: Arc::new(postretro_model::pose_modifier::PoseModifierStack::default()),
        },
    );

    super::update_brain_animation_playback_rates(&mut registry, &store, 1.0);
    let rate = registry
        .get_component::<MeshComponent>(enemy)
        .unwrap()
        .animation
        .as_ref()
        .unwrap()
        .rate;
    assert!(
        (rate - 1.5).abs() <= 1.0e-6,
        "derived first-tick rate = {rate}"
    );
    assert_eq!(context.take_pending_mesh_clip_resolves(), vec![enemy]);
}

#[test]
fn simulate_tick_writes_target_aim_and_tick_end_heading_pose_inputs() {
    let world = floor_world();
    let nav_graph = open_floor_nav_graph();
    let mut progress = ProgressTracker::new();
    let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
    let mut mover_states = MoverTickStateTable::default();
    let hit_zones = HitZoneStore::new();
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (enemy, target) = {
        let mut registry = registry.borrow_mut();
        let target = spawn_player(&mut registry, Vec3::new(6.0, 1.21, 6.0));
        let enemy = spawn_driven_agent(
            &mut registry,
            Vec3::new(5.0, 1.21, 5.0),
            ATTACK_STATE,
            "attack",
        );
        let mut brain = registry
            .get_component::<BrainComponent>(enemy)
            .unwrap()
            .clone();
        brain.acquired_target = Some(target);
        registry.set_component(enemy, brain).unwrap();
        (enemy, target)
    };

    run_driven_agent_sim_tick(
        registry.clone(),
        &world,
        &hit_zones,
        &nav_graph,
        1.0,
        &mut progress,
        &mut ai_runtime,
        &mut mover_states,
    );

    let registry = registry.borrow();
    let self_transform = *registry.get_component::<Transform>(enemy).unwrap();
    let target_transform = *registry.get_component::<Transform>(target).unwrap();
    let direction = target_transform.position - self_transform.position;
    let horizontal = (direction.x * direction.x + direction.z * direction.z).sqrt();
    let (heading_yaw, _, _) = self_transform.rotation.to_euler(EulerRot::YXZ);
    let inputs = registry
        .get_component::<MeshComponent>(enemy)
        .unwrap()
        .pose_inputs
        .expect("animated AI receives same-tick pose inputs");

    assert!((inputs.aim_yaw - direction.x.atan2(direction.z)).abs() <= 1.0e-6);
    assert!((inputs.aim_pitch - direction.y.atan2(horizontal)).abs() <= 1.0e-6);
    assert!((inputs.heading_yaw - heading_yaw).abs() <= 1.0e-6);
}

#[test]
fn pose_inputs_fallbacks_and_vertical_targets_remain_finite() {
    fn inputs_for(
        target_offset: Option<Vec3>,
        stale_target: bool,
    ) -> postretro_entities::PoseInputs {
        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform {
            rotation: glam::Quat::from_rotation_y(0.6),
            ..Transform::default()
        });
        registry
            .set_component(entity, driven_agent_mesh("attack"))
            .unwrap();
        let acquired_target = target_offset.map(|offset| {
            let target = registry.spawn(Transform {
                position: offset,
                ..Transform::default()
            });
            if stale_target {
                registry.despawn(target).unwrap();
            }
            target
        });
        let brain = BrainComponent {
            acquired_target,
            ..brain_in_state(&driven_agent_graph(), ATTACK_STATE)
        };
        registry.set_component(entity, brain).unwrap();

        super::update_pose_inputs(
            &mut registry,
            (0.0, 0.0),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        registry
            .get_component::<MeshComponent>(entity)
            .unwrap()
            .pose_inputs
            .unwrap()
    }

    let no_target = inputs_for(None, false);
    let missing_target = inputs_for(Some(Vec3::X), true);
    for fallback in [no_target, missing_target] {
        assert!(fallback.aim_pitch.abs() <= 1.0e-6);
        assert!((fallback.aim_yaw - 0.6).abs() <= 1.0e-6);
        assert!((fallback.heading_yaw - 0.6).abs() <= 1.0e-6);
    }

    let coincident = inputs_for(Some(Vec3::ZERO), false);
    let straight_up = inputs_for(Some(Vec3::Y), false);
    let straight_down = inputs_for(Some(-Vec3::Y), false);
    for inputs in [coincident, straight_up, straight_down] {
        assert!(inputs.aim_pitch.is_finite());
        assert!(inputs.aim_yaw.is_finite());
        assert!(inputs.heading_yaw.is_finite());
        assert!((inputs.aim_yaw - inputs.heading_yaw).abs() <= 1.0e-6);
    }
    assert!(coincident.aim_pitch.abs() <= 1.0e-6);
    assert!((straight_up.aim_pitch - std::f32::consts::FRAC_PI_2).abs() <= 1.0e-6);
    assert!((straight_down.aim_pitch + std::f32::consts::FRAC_PI_2).abs() <= 1.0e-6);
}

#[test]
fn local_player_pose_inputs_use_camera_aim_and_movement_heading() {
    let (mut registry, pawn, _) = leg_probe_fixture(Vec3::ZERO, -0.4);
    let mut movement = PlayerMovementComponent::from_descriptor(&player_descriptor());
    movement.velocity = Vec3::X * 3.0;
    registry.set_component(pawn, movement).unwrap();
    registry.mark_local_player_pawn(pawn).unwrap();

    super::update_pose_inputs(
        &mut registry,
        (-0.35, 1.1),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
    );

    let inputs = registry
        .get_component::<MeshComponent>(pawn)
        .unwrap()
        .pose_inputs
        .unwrap();
    assert!((inputs.aim_pitch + 0.35).abs() <= 1.0e-6);
    assert!((inputs.aim_yaw - 1.1).abs() <= 1.0e-6);
    assert!((inputs.heading_yaw + std::f32::consts::FRAC_PI_2).abs() <= 1.0e-6);
}

#[test]
fn remote_player_pose_inputs_use_interpolated_aim_and_velocity_heading_fallback() {
    let transform_yaw = 0.6;
    let (mut registry, pawn, _) = leg_probe_fixture(Vec3::ZERO, transform_yaw);
    let network_id = postretro_net::wire::NetworkId(77);
    let mut remote_network_ids = HashMap::new();
    remote_network_ids.insert(pawn, network_id);
    let mut remote_aim_pitches = HashMap::new();
    remote_aim_pitches.insert(network_id, -0.25);
    let mut remote_heading_yaws = HashMap::new();
    remote_heading_yaws.insert(network_id, -1.2);

    super::update_pose_inputs(
        &mut registry,
        (0.0, 0.0),
        &HashMap::new(),
        &remote_aim_pitches,
        &remote_heading_yaws,
        &remote_network_ids,
    );
    let moving = registry
        .get_component::<MeshComponent>(pawn)
        .unwrap()
        .pose_inputs
        .unwrap();
    assert!((moving.aim_pitch + 0.25).abs() <= 1.0e-6);
    assert!((moving.aim_yaw - transform_yaw).abs() <= 1.0e-6);
    assert!((moving.heading_yaw + 1.2).abs() <= 1.0e-6);

    super::update_pose_inputs(
        &mut registry,
        (0.0, 0.0),
        &HashMap::new(),
        &remote_aim_pitches,
        &HashMap::new(),
        &remote_network_ids,
    );
    let stationary = registry
        .get_component::<MeshComponent>(pawn)
        .unwrap()
        .pose_inputs
        .unwrap();
    assert!(
        (stationary.heading_yaw - transform_yaw).abs() <= 1.0e-6,
        "stationary remote avatar falls back to displayed transform yaw"
    );
}

#[test]
fn listen_host_remote_player_pose_inputs_use_resolved_client_camera_aim() {
    let (mut registry, pawn, _) = leg_probe_fixture(Vec3::ZERO, 0.0);
    let mut movement = PlayerMovementComponent::from_descriptor(&player_descriptor());
    movement.velocity = Vec3::X * 3.0;
    registry.set_component(pawn, movement).unwrap();
    // The registry treats the first movement pawn as local for old maps. Mark a
    // separate pawn so this fixture exercises the listen-host remote-pawn path.
    let local_pawn = registry.spawn(Transform::default());
    registry
        .set_component(
            local_pawn,
            PlayerMovementComponent::from_descriptor(&player_descriptor()),
        )
        .unwrap();
    registry.mark_local_player_pawn(local_pawn).unwrap();
    let mut remote_player_aims = HashMap::new();
    remote_player_aims.insert(pawn, (-0.4, 1.2));

    super::update_pose_inputs(
        &mut registry,
        (0.0, 0.0),
        &remote_player_aims,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
    );

    let inputs = registry
        .get_component::<MeshComponent>(pawn)
        .unwrap()
        .pose_inputs
        .unwrap();
    assert!((inputs.aim_pitch + 0.4).abs() <= 1.0e-6);
    assert!((inputs.aim_yaw - 1.2).abs() <= 1.0e-6);
    assert!((inputs.heading_yaw + std::f32::consts::FRAC_PI_2).abs() <= 1.0e-6);
}

/// One leg model: hip → knee → ankle, composed ankle resting at model (0,-0.7,0),
/// with one looping idle clip (no joint tracks, so the pose falls back to the
/// rest hierarchy the ground probe reads). Leg `0` drives foot probe `0`.
///
/// The knee carries a small forward (+Z) offset that the ankle segment cancels,
/// so the composed ankle stays at exactly (0,-0.7,0) — the ground probe reads
/// only the ankle, so its contact/normal are unchanged — while giving the
/// two-bone IK a well-defined bend plane (a perfectly straight leg has none and
/// cannot be re-solved). Segment sum 2·√(0.35²+0.12²) ≈ 0.740.
fn leg_model() -> crate::scripting_systems::hit_zones::ModelHitZones {
    use postretro_model::pose_modifier::{JointMask, LegChain};
    use postretro_model::skeleton::{AnimationClip, Joint, RestLocal, Skeleton};

    let joint = |parent, offset: Vec3| Joint {
        parent,
        inverse_bind: glam::Mat4::IDENTITY.to_cols_array_2d(),
        rest_local: RestLocal {
            translation: offset,
            rotation: glam::Quat::IDENTITY,
            scale: Vec3::ONE,
        },
    };
    let skeleton = Skeleton {
        joints: vec![
            joint(None, Vec3::ZERO),
            joint(Some(0), Vec3::new(0.0, -0.35, 0.12)),
            joint(Some(1), Vec3::new(0.0, -0.35, -0.12)),
        ],
    };
    let clips = vec![AnimationClip {
        name: "idle".to_string(),
        duration: 1.0,
        joints: Vec::new(),
        travel_speed: None,
    }];
    let mut chain_mask = JointMask::new();
    for j in [0usize, 1, 2] {
        assert!(chain_mask.insert(j));
    }
    crate::scripting_systems::hit_zones::ModelHitZones {
        skeleton: std::sync::Arc::new(skeleton),
        clips: std::sync::Arc::new(clips),
        joint_zones: vec![None, None, None],
        sockets: std::collections::HashMap::new(),
        derived_bound: None,
        legs: vec![LegChain {
            chain_mask,
            foot_joint: 2,
        }],
        pose_stack: std::sync::Arc::new(
            postretro_model::pose_modifier::PoseModifierStack::default(),
        ),
    }
}

/// Spawn one leg-tagged animated entity at `position`/`yaw`, run the production
/// tick-end pose order (aim/heading, then the ground probe) against a flat floor,
/// and return the resulting `PoseInputs`.
fn probe_leg_entity(position: Vec3, yaw: f32) -> postretro_entities::PoseInputs {
    probe_leg_entity_in(&floor_world(), position, yaw)
}

/// Same production tick-end pose order as [`probe_leg_entity`], but against a
/// caller-supplied collision world so a slope fixture can drive the probe.
fn probe_leg_entity_in(
    world: &CollisionWorld,
    position: Vec3,
    yaw: f32,
) -> postretro_entities::PoseInputs {
    let (mut registry, entity, store) = leg_probe_fixture(position, yaw);
    let mover_colliders: Vec<MoverCollider> = Vec::new();
    let mover_states = MoverTickStateTable::default();

    super::update_presentation_pose_inputs(
        &mut registry,
        world,
        &mover_colliders,
        &mover_states,
        &store,
        0.0,
        super::PresentationPoseInputs {
            camera_aim: (0.0, 0.0),
            remote_player_aims: &HashMap::new(),
            remote_aim_pitches: &HashMap::new(),
            remote_heading_yaws: &HashMap::new(),
            remote_network_ids: &HashMap::new(),
        },
    );

    registry
        .get_component::<MeshComponent>(entity)
        .unwrap()
        .pose_inputs
        .expect("leg entity receives pose inputs")
}

fn leg_probe_fixture(position: Vec3, yaw: f32) -> (EntityRegistry, EntityId, HitZoneStore) {
    let mut registry = EntityRegistry::new();
    let mut states = std::collections::HashMap::new();
    states.insert(
        "idle".to_string(),
        AnimationState {
            clip: "idle".into(),
            looping: true,
            crossfade_ms: 0.0,
            interrupt: InterruptPolicy::Smooth,
            travel_speed: None,
            clip_index: Some(0),
        },
    );
    let entity = registry.spawn(Transform {
        position,
        rotation: glam::Quat::from_rotation_y(yaw),
        scale: Vec3::ONE,
    });
    registry
        .set_component(
            entity,
            MeshComponent {
                model: "legwalker".into(),
                animation: Some(MeshAnimation::new(states, "idle".into())),
                origin_offset: Vec3::ZERO,
                shadow_bias_scale: 1.0,
                shadow_only: false,
                attachments: Vec::new(),
                pose_inputs: None,
            },
        )
        .expect("leg entity mesh should attach");

    let mut store = HitZoneStore::new();
    store.insert_for_test(
        postretro_model::ModelHandle::from("legwalker".to_string()),
        leg_model(),
    );
    (registry, entity, store)
}

#[test]
fn foot_ground_probes_are_deterministic_and_carry_forward_aim() {
    let position = Vec3::new(0.0, 1.0, 0.0);
    let yaw = 0.4;

    let first = probe_leg_entity(position, yaw);
    let second = probe_leg_entity(position, yaw);

    // Determinism: repeated headless runs of the same tick state are identical.
    assert_eq!(
        first.feet, second.feet,
        "probe feet must be bit-identical run-to-run"
    );
    assert_eq!(first.foot_count, second.foot_count);

    // foot_count equals the entity's leg-set length.
    assert_eq!(first.foot_count, 1);

    // Flat ground under the foot: contact, upward model-space normal, height.
    let foot = first.feet[0];
    assert!(foot.hit, "foot over flat floor finds ground");
    assert!(
        (foot.normal - Vec3::Y).length() < 1.0e-4,
        "upward model-space normal, got {:?}",
        foot.normal
    );
    // Entity origin sits 1.0 above the floor, so the model-space ground height
    // under the foot is -1.0.
    assert!(
        (foot.contact_height + 1.0).abs() < 1.0e-3,
        "contact_height = {}",
        foot.contact_height
    );

    // Aim/heading fields authored by update_pose_inputs survive the probe write.
    assert!(
        (first.heading_yaw - yaw).abs() < 1.0e-5,
        "heading preserved through the probe RMW, got {}",
        first.heading_yaw
    );
    assert!((first.aim_yaw - yaw).abs() < 1.0e-5);
    assert!(first.aim_pitch.abs() < 1.0e-6);

    // A foot with no ground within the planting reach reports a miss, yet
    // foot_count still equals the leg-set length and the aim fields stand.
    let high = probe_leg_entity(Vec3::new(0.0, 10.0, 0.0), yaw);
    assert_eq!(high.foot_count, 1);
    assert!(
        !high.feet[0].hit,
        "no ground within the planting reach is a miss"
    );
    assert!((high.heading_yaw - yaw).abs() < 1.0e-5);
}

// Regression: connected clients skip simulate_tick, so remote legged meshes
// previously reached rendering with no pose inputs or ground probes.
#[test]
fn connected_client_presentation_probes_freshly_displayed_remote_transform() {
    let (mut registry, entity, store) = leg_probe_fixture(Vec3::new(0.0, 10.0, 0.0), 0.0);
    registry
        .set_presentation_transform(
            entity,
            Transform {
                position: Vec3::new(0.0, 1.0, 0.0),
                rotation: glam::Quat::from_rotation_y(0.6),
                scale: Vec3::ONE,
            },
        )
        .expect("remote presentation transform should apply");

    super::update_presentation_pose_inputs(
        &mut registry,
        &floor_world(),
        &[],
        &MoverTickStateTable::default(),
        &store,
        0.0,
        super::PresentationPoseInputs {
            camera_aim: (0.0, 0.0),
            remote_player_aims: &HashMap::new(),
            remote_aim_pitches: &HashMap::new(),
            remote_heading_yaws: &HashMap::new(),
            remote_network_ids: &HashMap::new(),
        },
    );

    let inputs = registry
        .get_component::<MeshComponent>(entity)
        .unwrap()
        .pose_inputs
        .expect("connected-client presentation produces pose inputs");
    assert_eq!(inputs.foot_count, 1);
    assert!(inputs.feet[0].hit, "displayed remote foot probes the floor");
    assert!((inputs.feet[0].contact_height + 1.0).abs() < 1.0e-3);
    assert!((inputs.heading_yaw - 0.6).abs() < 1.0e-5);
}

// Regression: publishing the model's leg count before validating animation,
// sample, and transform availability preserved stale contacts in the renderer.
#[test]
fn unavailable_probe_inputs_clear_stale_feet_and_publish_zero_count() {
    let world = floor_world();
    let (mut registry, entity, store) = leg_probe_fixture(Vec3::new(0.0, 1.0, 0.0), 0.0);
    let mover_states = MoverTickStateTable::default();
    let run = |registry: &mut EntityRegistry| {
        super::update_presentation_pose_inputs(
            registry,
            &world,
            &[],
            &mover_states,
            &store,
            0.0,
            super::PresentationPoseInputs {
                camera_aim: (0.0, 0.0),
                remote_player_aims: &HashMap::new(),
                remote_aim_pitches: &HashMap::new(),
                remote_heading_yaws: &HashMap::new(),
                remote_network_ids: &HashMap::new(),
            },
        );
    };
    let assert_cleared = |registry: &EntityRegistry, reason: &str| {
        let inputs = registry
            .get_component::<MeshComponent>(entity)
            .unwrap()
            .pose_inputs
            .unwrap();
        assert_eq!(inputs.foot_count, 0, "{reason}");
        assert_eq!(inputs.feet, [FootProbe::default(); MAX_FEET], "{reason}");
    };

    run(&mut registry);
    let original_mesh = registry
        .get_component::<MeshComponent>(entity)
        .unwrap()
        .clone();
    assert_eq!(original_mesh.pose_inputs.unwrap().foot_count, 1);
    assert!(original_mesh.pose_inputs.unwrap().feet[0].hit);

    let mut missing_model = original_mesh.clone();
    missing_model.model = "missing-model".to_string();
    registry.set_component(entity, missing_model).unwrap();
    run(&mut registry);
    assert_cleared(&registry, "missing model clears stale feet");

    let mut hidden_stale = original_mesh.clone();
    hidden_stale.model = "missing-model".to_string();
    let mut hidden_inputs = hidden_stale.pose_inputs.unwrap();
    hidden_inputs.foot_count = 0;
    hidden_stale.pose_inputs = Some(hidden_inputs);
    registry.set_component(entity, hidden_stale).unwrap();
    run(&mut registry);
    assert_cleared(
        &registry,
        "non-default foot slots clear even when the stale live count is already zero",
    );

    let mut no_animation = original_mesh.clone();
    no_animation.animation = None;
    registry.set_component(entity, no_animation).unwrap();
    run(&mut registry);
    let stateless_inputs = registry
        .get_component::<MeshComponent>(entity)
        .unwrap()
        .pose_inputs
        .expect("stateless legged mesh receives rest-pose probes");
    assert_eq!(stateless_inputs.foot_count, 1);
    assert!(
        stateless_inputs.feet[0].hit,
        "stateless legged mesh probes from its rest-pose foot",
    );

    let mut unresolved = original_mesh.clone();
    unresolved.pose_inputs = original_mesh.pose_inputs;
    unresolved
        .animation
        .as_mut()
        .unwrap()
        .states
        .get_mut("idle")
        .unwrap()
        .clip_index = None;
    registry.set_component(entity, unresolved).unwrap();
    run(&mut registry);
    assert_cleared(
        &registry,
        "unresolved animated state is unavailable instead of sampling clip 0",
    );
    let zones = store.get_by_name("legwalker").unwrap();
    let unresolved_animation = registry
        .get_component::<MeshComponent>(entity)
        .unwrap()
        .animation
        .as_ref()
        .unwrap();
    assert!(
        crate::scripting_systems::hit_zones::sample_world_pose_for_probe(
            zones,
            Some(unresolved_animation),
            0.0,
            entity.to_raw(),
        )
        .is_none()
    );
    registry.set_component(entity, original_mesh).unwrap();
    let mut tilted = *registry.get_component::<Transform>(entity).unwrap();
    tilted.rotation = glam::Quat::from_rotation_x(0.2);
    registry.set_component(entity, tilted).unwrap();
    run(&mut registry);
    assert_cleared(&registry, "unsupported transform clears stale feet");
}

#[test]
fn stateless_probe_sampler_holds_rest_pose_when_clip_zero_moves() {
    use postretro_model::skeleton::{AnimationClip, Interp, JointTracks, Track};

    let mut zones = leg_model();
    zones.clips = Arc::new(vec![AnimationClip {
        name: "moving-clip-zero".to_string(),
        duration: 1.0,
        joints: vec![JointTracks {
            translation: Track::new(
                vec![0.0, 1.0],
                vec![Vec3::new(8.0, 0.0, 0.0), Vec3::new(8.0, 0.0, 0.0)],
                Interp::Linear,
            )
            .expect("finite clip-zero track builds"),
            ..JointTracks::default()
        }],
        travel_speed: None,
    }]);

    let pose =
        crate::scripting_systems::hit_zones::sample_world_pose_for_probe(&zones, None, 0.5, 7)
            .expect("stateless rest pose is available");
    let foot = pose[2].w_axis.truncate();
    assert!(
        (foot - Vec3::new(0.0, -0.7, 0.0)).length() < 1.0e-5,
        "stateless probe must use rest pose, not clip-zero translation: {foot:?}",
    );
}

#[test]
fn foot_probe_transform_accepts_small_positive_uniform_and_nonuniform_scales() {
    for scale in [Vec3::splat(1.0e-6), Vec3::new(1.0e-6, 2.0e-6, 3.0e-6)] {
        let transform = Transform {
            position: Vec3::ZERO,
            rotation: glam::Quat::IDENTITY,
            scale,
        };
        let model_to_world = model_matrix(&transform, Vec3::ZERO).unwrap();
        assert!(
            super::foot_probe_inverse(&transform, &model_to_world).is_some(),
            "positive finite scale {scale:?} has a finite inverse"
        );
    }
}

#[test]
fn foot_ik_plants_and_orients_on_slope_within_walkable_limit() {
    use postretro_model::anim::{Loop, sample_clip_looped_modified};
    use postretro_model::pose_modifier::{ModifierEntry, PoseModifier, PoseModifierStack};

    // A ~16.7° ground tilt about world Z: surface y = 0.3 * x. Walkable
    // (normal.y = 1/sqrt(1.09) = 0.958 >= COS_WALKABLE = 0.643).
    let slope = 0.3_f32;
    let world = sloped_floor_world(slope);

    // Entity over the uphill face at x = 0.5 (surface there sits at y = 0.15),
    // dropped so the model-space rest ankle (model-y = -0.70) hovers just above
    // the slope: foot_world.y = 0.88 - 0.70 = 0.18, a 0.03 gap to the surface —
    // inside the plant blend band so the foot genuinely plants (not swing).
    let position = Vec3::new(0.5, 0.88, 0.0);
    let inputs = probe_leg_entity_in(&world, position, 0.0);

    // (2) The production tick probe reports the slope contact, not the flat
    // floor: one live foot, a hit, the tilted (non-Y) upward normal, and the
    // model-space slope height under the foot.
    assert_eq!(inputs.foot_count, 1);
    let foot = inputs.feet[0];
    assert!(foot.hit, "foot over the walkable slope finds ground");
    // Model-space slope surface under the foot: y(0.5) - position.y = 0.15 - 0.88.
    let expected_contact = slope * position.x - position.y;
    assert!(
        (foot.contact_height - expected_contact).abs() < 1.0e-3,
        "slope contact height = {}, expected {expected_contact}",
        foot.contact_height
    );
    // Tilted ground normal, clearly not straight up, yet still walkable.
    let expected_normal = Vec3::new(-slope, 1.0, 0.0).normalize();
    assert!(
        (foot.normal - expected_normal).length() < 1.0e-3,
        "tilted slope normal, got {:?}",
        foot.normal
    );
    assert!(
        (foot.normal - Vec3::Y).length() > 0.1,
        "normal must be a real tilt, not flat-ground Vec3::Y: {:?}",
        foot.normal
    );

    // (3) Apply the FootIk modifier directly through the wgpu-free sampler with
    // those driven probes. Build the stack from the model's own leg set, exactly
    // as the loader does (one FootIk entry carrying the whole leg list).
    let model = leg_model();
    let stack = PoseModifierStack::new(vec![ModifierEntry {
        mask: postretro_model::pose_modifier::JointMask::new(),
        modifier: PoseModifier::FootIk {
            legs: model.legs.clone(),
        },
    }]);

    // Rest/clip pose (no joint tracks) for the same skeleton, unmodified, gives
    // the flat-ground ankle baseline the plant must move away from.
    let mut rest_palette = Vec::new();
    sample_clip_looped_modified(
        &model.clips[0],
        &model.skeleton,
        0.0,
        Loop::Wrap,
        &PoseModifierStack::default(),
        None,
        &mut rest_palette,
    );
    // Identity inverse_bind: a palette entry's translation column is that joint's
    // model-space position.
    let ankle_rest_y = rest_palette[2].matrix[3][1];
    assert!(
        (ankle_rest_y - -0.70).abs() < 1.0e-4,
        "rest ankle model-y = {ankle_rest_y}, expected -0.70"
    );

    let mut planted = Vec::new();
    sample_clip_looped_modified(
        &model.clips[0],
        &model.skeleton,
        0.0,
        Loop::Wrap,
        &stack,
        Some(&inputs),
        &mut planted,
    );

    let ankle_planted_y = planted[2].matrix[3][1];
    // Planted onto the slope surface: the ankle drops from the flat-ground rest
    // (-0.70) toward the probed slope contact height, clearly below rest.
    assert!(
        ankle_planted_y < ankle_rest_y - 1.0e-2,
        "ankle did not plant down onto the slope: rest {ankle_rest_y}, planted {ankle_planted_y}"
    );
    // ...and it lands at (approximately) the probed slope surface, never below it.
    assert!(
        ankle_planted_y >= foot.contact_height - 1.0e-3,
        "ankle drove through the slope surface: planted {ankle_planted_y}, contact {}",
        foot.contact_height
    );
    assert!(
        (ankle_planted_y - foot.contact_height).abs() < 2.0e-2,
        "ankle did not reach the slope surface: planted {ankle_planted_y}, contact {}",
        foot.contact_height
    );

    // Foot orients toward the ground normal: the sole (foot model +Y) tips off
    // straight-up toward the tilted slope normal.
    let foot_rot = glam::Quat::from_mat4(&glam::Mat4::from_cols_array_2d(&planted[2].matrix));
    let sole = (foot_rot * Vec3::Y).normalize();
    assert!(
        sole.dot(foot.normal) > sole.dot(Vec3::Y),
        "foot sole tilted toward the slope normal: sole {sole:?}, normal {:?}",
        foot.normal
    );

    // The solve rotates only the leg joints and never translates the root: joint
    // 0 stays at the model origin, so the plant can never push the pelvis through
    // the surface.
    let root = planted[0].matrix[3];
    assert!(
        root[0].abs() < 1.0e-6 && root[1].abs() < 1.0e-6 && root[2].abs() < 1.0e-6,
        "root joint translated during the solve: {:?}",
        [root[0], root[1], root[2]]
    );
}

fn fixed_command_stream() -> Vec<RecordedCommand> {
    (0..TICK_COUNT)
        .map(|tick| {
            let phase = tick % 120;
            let fire_pressed = matches!(tick, 5 | 180 | 360 | 540);
            RecordedCommand {
                wish_dir: if phase < 45 {
                    Vec2::new(0.25, 1.0)
                } else if phase < 80 {
                    Vec2::new(-0.5, 0.2)
                } else {
                    Vec2::ZERO
                },
                jump_pressed: matches!(tick, 30 | 210 | 390),
                dash_pressed: matches!(tick, 90 | 270 | 450),
                running: phase < 70,
                crouch_intent: (300..360).contains(&tick),
                facing_yaw: if tick < 300 { 0.0 } else { 0.35 },
                fire_pressed,
                fire_active: fire_pressed || matches!(tick, 6 | 181 | 361 | 541),
            }
        })
        .collect()
}

fn run_stream(commands: &[RecordedCommand], spawn_order: SpawnOrder) -> SimRun {
    let mut harness = SimHarness::new(spawn_order, SimFixture::Determinism);
    let mut events = Vec::with_capacity(commands.len());
    let mut ir_slot_timeline = Vec::with_capacity(commands.len());
    for command in commands {
        events.push(harness.tick(*command));
        ir_slot_timeline.push(harness.trigger_slot());
    }
    let predicate_crossing_sequence = events
        .iter()
        .map(|events| events.predicate_crossing_fires.clone())
        .collect();
    SimRun {
        pawns: harness.role_outcomes(),
        selected_player_health: harness.selected_player_health(),
        enemy_state: harness.enemy_state(),
        trigger_residual_counts: events
            .iter()
            .map(|events| events.trigger_residuals.len())
            .collect(),
        trigger_slot: harness.trigger_slot(),
        ir_slot_timeline,
        predicate_crossing_sequence,
        trigger_arm_target_armed: harness.trigger_arm_target_armed(),
        role_health_ledger: harness.role_health_ledger(),
        trap_pool_source_selected: harness.trap_pool_source_selected,
        events,
    }
}

// O1/O2/O31/O61: `SimHarness` installs `levelLoad = [note(presA), wait(5),
// note(presB)]`; presA runs at install. `frame()` advances the scheduler after
// install and before its tick, so presB lands at the FIRST post-install frame's
// drain. A counter advanced only by window events would never advance here and
// the instance would be skipped forever.
#[test]
fn sim_harness_first_post_install_tick_advances_level_load_wait() {
    let mut harness = SimHarness::new(
        SpawnOrder::AlphaThenBeta,
        SimFixture::LevelLoadWait { duration_ms: 5.0 },
    );
    let neutral = RecordedCommand {
        wish_dir: Vec2::ZERO,
        jump_pressed: false,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
        fire_pressed: false,
        fire_active: false,
    };

    // presA ran at install; the wait enrolled its tail at frame counter 0.
    assert_eq!(harness.note_log(), vec!["presA".to_string()]);

    // The first post-install frame advances the counter before its tick. The
    // install-time enrollment is therefore old enough to advance and land.
    harness.frame(&[neutral]);
    assert_eq!(
        harness.note_log(),
        vec!["presA".to_string(), "presB".to_string()],
        "the first post-install tick advances and lands the levelLoad wait"
    );
}

fn assert_trigger_positive_anchors(run: &SimRun) {
    assert_eq!(
        run.trigger_slot,
        Some(SlotValue::Number(4.0)),
        "two activators must each execute both IR increments at the fixed-tick write point"
    );
    assert_eq!(
        run.ir_slot_timeline.first(),
        Some(&Some(SlotValue::Number(4.0))),
        "the first slot-timeline sample must capture the same-frame IR accumulation"
    );
    assert_eq!(
        run.predicate_crossing_sequence.first(),
        Some(&vec![("determinismReady".to_string(), true)]),
        "the IR predicate must fire once when the IR-written slots first satisfy it"
    );
    assert_eq!(
        run.predicate_crossing_sequence
            .iter()
            .skip(1)
            .flatten()
            .filter(|(reaction, _)| reaction == "determinismReady")
            .count(),
        0,
        "the predicate remains true after tick one, so it must not re-fire without a false re-arm"
    );
    assert_eq!(
        run.predicate_crossing_sequence
            .iter()
            .flatten()
            .filter(|(reaction, _)| reaction == "accumulatorEdge")
            .map(|(_, rising)| *rising)
            .collect::<Vec<_>>(),
        vec![true, false],
        "the production post-tick accumulator seam must drive both crossing directions"
    );
    assert!(
        run.trigger_arm_target_armed,
        "the baseline trigger must arm its target"
    );
    assert!(
        run.trap_pool_source_selected,
        "the fixed seed must select the live trap-pool source for this tick sequence",
    );
    assert!(
        run.events
            .iter()
            .any(|events| !events.trigger_fires.is_empty()),
        "the baseline must include at least one named trigger fire"
    );
}

fn assert_fixed_stream_weapon_positive_anchors(run: &SimRun) {
    let pellet_fans = run
        .events
        .iter()
        .filter_map(|events| {
            (!events.weapon_impact_points.is_empty()).then_some(&events.weapon_impact_points)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        pellet_fans.len(),
        4,
        "the fixed command stream must fire all four deterministic shotgun shells"
    );
    assert!(
        pellet_fans.iter().all(|fan| fan.len() == 8),
        "the backstop makes every multi-pellet shell expose all eight cast impacts"
    );
    assert_ne!(
        pellet_fans[0], pellet_fans[1],
        "consecutive shells, fired from the two inventory slots after the switch, must use distinct fans"
    );
    assert_ne!(
        pellet_fans[1], pellet_fans[2],
        "consecutive shells from the same inventory slot must use distinct fans"
    );
}

fn assert_runs_match(actual: &SimRun, expected: &SimRun) {
    assert_eq!(
        actual.events, expected.events,
        "stage-grouped event names must match exactly"
    );
    assert_eq!(
        actual.enemy_state, expected.enemy_state,
        "AI state must resolve from the same selected local pawn label"
    );
    // Exact equality is safe here: health deltas are integer damage values (10.0)
    // applied via integer-path arithmetic with no per-frame interpolation, so
    // deterministic runs must produce bit-identical results.
    assert_eq!(
        actual.selected_player_health, expected.selected_player_health,
        "selected player health must match exactly"
    );
    assert_eq!(
        actual.trigger_residual_counts, expected.trigger_residual_counts,
        "trigger fire/residual sequence must match exactly"
    );
    assert_eq!(
        actual.trigger_slot, expected.trigger_slot,
        "final IR-written trigger slot must match exactly"
    );
    assert_eq!(
        actual.ir_slot_timeline, expected.ir_slot_timeline,
        "IR-written slot timelines must match exactly"
    );
    assert_eq!(
        actual.predicate_crossing_sequence, expected.predicate_crossing_sequence,
        "IR predicate crossing-fire sequences must match exactly"
    );
    assert_eq!(
        actual.trigger_arm_target_armed, expected.trigger_arm_target_armed,
        "trigger fixed-tick registry mutation must match exactly"
    );
    assert_eq!(
        actual.trap_pool_source_selected, expected.trap_pool_source_selected,
        "the fixed-seed trap-pool selection must match exactly"
    );
    assert_eq!(
        actual.pawns.len(),
        expected.pawns.len(),
        "same role count expected"
    );
    for ((actual_role, actual), (expected_role, expected)) in
        actual.pawns.iter().zip(expected.pawns.iter())
    {
        assert_eq!(actual_role, expected_role, "roles must compare by label");
        assert_vec3_within(
            actual.position,
            expected.position,
            POSITION_EPSILON,
            "position",
        );
        assert_vec3_within(
            actual.velocity,
            expected.velocity,
            VELOCITY_EPSILON,
            "velocity",
        );
    }
}

fn assert_vec3_within(actual: Vec3, expected: Vec3, epsilon: f32, label: &str) {
    let delta = (actual - expected).abs();
    assert!(
        delta.x <= epsilon && delta.y <= epsilon && delta.z <= epsilon,
        "{label} differed by ({:.6}, {:.6}, {:.6}); actual=({:.6}, {:.6}, {:.6}) expected=({:.6}, {:.6}, {:.6})",
        delta.x,
        delta.y,
        delta.z,
        actual.x,
        actual.y,
        actual.z,
        expected.x,
        expected.y,
        expected.z,
    );
}

fn command_strategy() -> impl Strategy<Value = RecordedCommand> {
    let axis = prop_oneof![
        Just(-1.0_f32),
        Just(-0.35),
        Just(0.0),
        Just(0.35),
        Just(1.0)
    ];
    let yaw = prop_oneof![
        Just(-0.7_f32),
        Just(-0.25),
        Just(0.0),
        Just(0.25),
        Just(0.7)
    ];
    (
        axis.clone(),
        axis,
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        yaw,
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(
                right,
                forward,
                jump_pressed,
                dash_pressed,
                running,
                crouch_intent,
                facing_yaw,
                fire_pressed,
                fire_held,
            )| RecordedCommand {
                wish_dir: Vec2::new(right, forward),
                jump_pressed,
                dash_pressed,
                running,
                crouch_intent,
                facing_yaw,
                fire_pressed,
                fire_active: fire_pressed || fire_held,
            },
        )
}

#[test]
fn simulate_tick_determinism_harness_matches_run_to_run_and_spawn_order() {
    let commands = fixed_command_stream();
    assert_eq!(commands.len(), TICK_COUNT);

    let baseline = run_stream(&commands, SpawnOrder::AlphaThenBeta);
    let rerun = run_stream(&commands, SpawnOrder::AlphaThenBeta);
    let reversed_spawn = run_stream(&commands, SpawnOrder::BetaThenAlpha);

    assert_trigger_positive_anchors(&baseline);
    assert_fixed_stream_weapon_positive_anchors(&baseline);
    assert_runs_match(&rerun, &baseline);
    assert_runs_match(&reversed_spawn, &baseline);
}

#[test]
fn trigger_events_keep_two_activator_order_across_spawn_reversal() {
    let commands = [RecordedCommand {
        wish_dir: Vec2::ZERO,
        jump_pressed: false,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
        fire_pressed: false,
        fire_active: false,
    }];
    let alpha_then_beta = run_stream(&commands, SpawnOrder::AlphaThenBeta);
    let beta_then_alpha = run_stream(&commands, SpawnOrder::BetaThenAlpha);
    let expected = vec![
        RecordedTriggerFire {
            trigger: EntityLabel::TriggerSource,
            player: PlayerLabel::Local(Role::Alpha),
            event_name: "determinismTrigger".to_string(),
            edge: TriggerEventEdge::Enter,
        },
        RecordedTriggerFire {
            trigger: EntityLabel::TriggerSource,
            player: PlayerLabel::Remote(1),
            event_name: "determinismTrigger".to_string(),
            edge: TriggerEventEdge::Enter,
        },
    ];

    assert_eq!(
        alpha_then_beta.events[0].trigger_fires, expected,
        "both local and remote activators must reach the trigger stage"
    );
    assert_eq!(
        beta_then_alpha.events[0].trigger_fires, alpha_then_beta.events[0].trigger_fires,
        "trigger event ordering must not depend on pawn spawn order"
    );
}

#[test]
fn trigger_events_keep_multi_pawn_damage_ledger_across_spawn_reversal() {
    // AC 14: determinism of the damage EFFECT, not just fire order. The
    // determinism trigger runs `damage(on.activators, 25)`, so both pressers
    // take the hit on their tick-one enter. The resulting per-pawn health ledger
    // must be identical run-to-run and across spawn-order reversal.
    let commands = [RecordedCommand {
        wish_dir: Vec2::ZERO,
        jump_pressed: false,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
        fire_pressed: false,
        fire_active: false,
    }];
    let baseline = run_stream(&commands, SpawnOrder::AlphaThenBeta);
    let rerun = run_stream(&commands, SpawnOrder::AlphaThenBeta);
    let reversed = run_stream(&commands, SpawnOrder::BetaThenAlpha);

    // The presser landed on both activators: every pawn shows the 25-HP zap.
    assert_eq!(
        baseline.role_health_ledger.len(),
        2,
        "both pawns are ledgered"
    );
    for (role, health) in &baseline.role_health_ledger {
        assert!(
            *health <= 75.0,
            "activator {role:?} must show the presser's 25-HP damage; health was {health}"
        );
    }
    assert_eq!(
        rerun.role_health_ledger, baseline.role_health_ledger,
        "identical runs must produce an identical per-pawn damage ledger"
    );
    assert_eq!(
        reversed.role_health_ledger, baseline.role_health_ledger,
        "the damage ledger must not depend on pawn spawn order"
    );
}

#[test]
fn run_movement_tick_applies_local_command_only_to_marked_pawn() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (beta, alpha) = {
        let mut registry = registry.borrow_mut();
        let beta = spawn_player(&mut registry, Role::Beta.start_position());
        let alpha = spawn_player(&mut registry, Role::Alpha.start_position());
        registry.mark_local_player_pawn(alpha).unwrap();
        for id in [alpha, beta] {
            let mut movement = registry
                .get_component::<PlayerMovementComponent>(id)
                .unwrap()
                .clone();
            movement.set_grounded(true);
            registry.set_component(id, movement).unwrap();
        }
        (beta, alpha)
    };
    let beta_start = registry
        .borrow()
        .get_component::<Transform>(beta)
        .unwrap()
        .position;
    let input = MovementInput {
        wish_dir: Vec2::ZERO,
        jump_pressed: true,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
        use_pressed: false,
        drop_pressed: false,
    };

    let events = super::run_movement_tick(&registry, &floor_world(), GRAVITY, &input, DT);

    assert_eq!(
        events,
        vec!["jumped"],
        "only the marked local pawn may emit movement outcomes"
    );
    let registry = registry.borrow();
    assert!(
        registry
            .get_component::<PlayerMovementComponent>(alpha)
            .unwrap()
            .velocity
            .y
            > 0.0,
        "marked local pawn should consume the jump command"
    );
    assert_eq!(
        registry.get_component::<Transform>(beta).unwrap().position,
        beta_start,
        "unmarked additional pawn must not move from local input"
    );
    assert_eq!(
        registry
            .get_component::<PlayerMovementComponent>(beta)
            .unwrap()
            .velocity,
        Vec3::ZERO,
        "unmarked additional pawn velocity must remain untouched"
    );
}

#[test]
fn run_movement_tick_no_marker_fallback_drives_first_movement_pawn_only() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (first, second) = {
        let mut registry = registry.borrow_mut();
        let first = spawn_player(&mut registry, Role::Alpha.start_position());
        let second = spawn_player(&mut registry, Role::Beta.start_position());
        for id in [first, second] {
            let mut movement = registry
                .get_component::<PlayerMovementComponent>(id)
                .unwrap()
                .clone();
            movement.set_grounded(true);
            registry.set_component(id, movement).unwrap();
        }
        (first, second)
    };
    let second_start = registry
        .borrow()
        .get_component::<Transform>(second)
        .unwrap()
        .position;
    let input = MovementInput {
        wish_dir: Vec2::ZERO,
        jump_pressed: true,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
        use_pressed: false,
        drop_pressed: false,
    };

    let events = super::run_movement_tick(&registry, &floor_world(), GRAVITY, &input, DT);

    assert_eq!(
        events,
        vec!["jumped"],
        "no-marker fallback applies the local command to one deterministic pawn"
    );
    let registry = registry.borrow();
    assert!(
        registry
            .get_component::<PlayerMovementComponent>(first)
            .unwrap()
            .velocity
            .y
            > 0.0,
        "first fallback pawn should consume the jump command"
    );
    assert_eq!(
        registry
            .get_component::<Transform>(second)
            .unwrap()
            .position,
        second_start,
        "second pawn must not move from the single local command"
    );
}

#[test]
fn run_movement_tick_invalid_marker_fallback_drives_first_movement_pawn_only() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (first, second) = {
        let mut registry = registry.borrow_mut();
        let invalid_marker = registry.spawn(Transform::default());
        registry.mark_local_player_pawn(invalid_marker).unwrap();
        let first = spawn_player(&mut registry, Role::Alpha.start_position());
        let second = spawn_player(&mut registry, Role::Beta.start_position());
        for id in [first, second] {
            let mut movement = registry
                .get_component::<PlayerMovementComponent>(id)
                .unwrap()
                .clone();
            movement.set_grounded(true);
            registry.set_component(id, movement).unwrap();
        }
        (first, second)
    };
    let second_start = registry
        .borrow()
        .get_component::<Transform>(second)
        .unwrap()
        .position;
    let input = MovementInput {
        wish_dir: Vec2::ZERO,
        jump_pressed: true,
        dash_pressed: false,
        running: false,
        crouch_intent: false,
        facing_yaw: 0.0,
        use_pressed: false,
        drop_pressed: false,
    };

    let events = super::run_movement_tick(&registry, &floor_world(), GRAVITY, &input, DT);

    assert_eq!(
        events,
        vec!["jumped"],
        "invalid marker fallback applies the local command to one deterministic pawn"
    );
    let registry = registry.borrow();
    assert!(
        registry
            .get_component::<PlayerMovementComponent>(first)
            .unwrap()
            .velocity
            .y
            > 0.0,
        "first fallback pawn should consume the jump command"
    );
    assert_eq!(
        registry
            .get_component::<Transform>(second)
            .unwrap()
            .position,
        second_start,
        "second pawn must not move from an invalid local marker fallback"
    );
}

#[test]
fn simulate_tick_uses_sim_command_fire_button_with_callback_aim() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (weapon, target) = {
        let mut registry = registry.borrow_mut();
        (
            spawn_local_active_weapon(&mut registry),
            spawn_target(&mut registry, Vec3::new(0.0, 2.0, -10.0)),
        )
    };
    let world = CollisionWorld::new();
    let hit_zones = HitZoneStore::new();
    let mut progress = ProgressTracker::new();
    let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
    let mover_colliders = Vec::new();
    let mut mover_states = MoverTickStateTable::default();
    let command = SimCommand {
        movement: MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            use_pressed: false,
            drop_pressed: false,
        },
        fire_button: FireButtonState {
            pressed: false,
            active: false,
        },
        reload: false,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    };

    let events = simulate_tick(
        registry.clone(),
        &world,
        &hit_zones,
        None,
        GRAVITY,
        Some(weapon),
        0.0,
        &mut progress,
        &mut ai_runtime,
        &mover_colliders,
        &mut mover_states,
        &[],
        &command,
        |_| super::PostMovementCommand {
            aim_origin: Vec3::new(0.0, 2.0, -20.0),
            aim_direction: Vec3::Z,
        },
        DT,
        None,
        |_| {},
    );

    assert!(
        events.weapon.is_empty(),
        "valid callback aim must not fire when SimCommand.fire_button is inactive"
    );
    assert_eq!(
        registry
            .borrow()
            .get_component::<HealthComponent>(target)
            .expect("target keeps health")
            .current,
        20.0,
        "inactive fire button must leave the valid target undamaged"
    );
}

#[test]
fn simulate_tick_normalizes_callback_aim_direction_before_weapon_fire() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (weapon, target) = {
        let mut registry = registry.borrow_mut();
        (
            spawn_local_active_weapon(&mut registry),
            spawn_target(&mut registry, Vec3::new(0.0, 2.0, -45.0)),
        )
    };
    let world = CollisionWorld::new();
    let hit_zones = HitZoneStore::new();
    let mut progress = ProgressTracker::new();
    let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
    let mover_colliders = Vec::new();
    let mut mover_states = MoverTickStateTable::default();
    let command = SimCommand {
        movement: MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            use_pressed: false,
            drop_pressed: false,
        },
        fire_button: FireButtonState {
            pressed: true,
            active: true,
        },
        reload: false,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    };

    let events = simulate_tick(
        registry.clone(),
        &world,
        &hit_zones,
        None,
        GRAVITY,
        Some(weapon),
        0.0,
        &mut progress,
        &mut ai_runtime,
        &mover_colliders,
        &mut mover_states,
        &[],
        &command,
        |_| super::PostMovementCommand {
            aim_origin: Vec3::new(0.0, 2.0, 0.0),
            aim_direction: Vec3::new(0.0, 0.0, -2.0),
        },
        DT,
        None,
        |_| {},
    );

    assert_eq!(
        events.weapon,
        vec!["activate"],
        "valid non-unit aim still fires, but range is measured after normalization"
    );
    assert_eq!(
        registry
            .borrow()
            .get_component::<HealthComponent>(target)
            .expect("target keeps health")
            .current,
        20.0,
        "non-unit aim must not extend hitscan range in metres"
    );
}

#[test]
fn simulate_tick_noops_weapon_fire_for_invalid_callback_aim_direction() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (weapon, target) = {
        let mut registry = registry.borrow_mut();
        let weapon = spawn_local_active_weapon(&mut registry);
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .expect("weapon keeps component")
            .clone();
        component.cooldown_remaining_ms = 100.0;
        registry.set_component(weapon, component).unwrap();
        let target = spawn_target(&mut registry, Vec3::new(0.0, 2.0, -10.0));
        (weapon, target)
    };
    let world = CollisionWorld::new();
    let hit_zones = HitZoneStore::new();
    let mut progress = ProgressTracker::new();
    let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
    let mover_colliders = Vec::new();
    let mut mover_states = MoverTickStateTable::default();
    let command = SimCommand {
        movement: MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            use_pressed: false,
            drop_pressed: false,
        },
        fire_button: FireButtonState {
            pressed: true,
            active: true,
        },
        reload: false,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    };

    let events = simulate_tick(
        registry.clone(),
        &world,
        &hit_zones,
        None,
        GRAVITY,
        Some(weapon),
        0.0,
        &mut progress,
        &mut ai_runtime,
        &mover_colliders,
        &mut mover_states,
        &[],
        &command,
        |_| super::PostMovementCommand {
            aim_origin: Vec3::new(0.0, 2.0, 0.0),
            aim_direction: Vec3::ZERO,
        },
        DT,
        None,
        |_| {},
    );

    assert!(
        events.weapon.is_empty(),
        "zero aim should suppress shot events"
    );
    let registry = registry.borrow();
    let weapon_component = registry
        .get_component::<WeaponComponent>(weapon)
        .expect("weapon keeps component");
    assert!(
        (weapon_component.cooldown_remaining_ms - (100.0 - DT * 1000.0)).abs() < 1.0e-4,
        "invalid aim must still advance weapon cooldown"
    );
    assert!(
        weapon_component.shoot_press_consumed,
        "invalid aim must still advance semi-auto press state"
    );
    assert_eq!(
        registry
            .get_component::<HealthComponent>(target)
            .expect("target keeps health")
            .current,
        20.0,
        "invalid aim must not damage a target"
    );
}

#[test]
fn simulate_tick_noops_weapon_fire_for_non_finite_callback_aim_origin() {
    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let (weapon, target) = {
        let mut registry = registry.borrow_mut();
        (
            spawn_local_active_weapon(&mut registry),
            spawn_target(&mut registry, Vec3::new(0.0, 2.0, -10.0)),
        )
    };
    let world = CollisionWorld::new();
    let hit_zones = HitZoneStore::new();
    let mut progress = ProgressTracker::new();
    let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
    let mover_colliders = Vec::new();
    let mut mover_states = MoverTickStateTable::default();
    let command = SimCommand {
        movement: MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            use_pressed: false,
            drop_pressed: false,
        },
        fire_button: FireButtonState {
            pressed: true,
            active: true,
        },
        reload: false,
        firing_slot: 0,
        select_slot: None,
        use_pressed: false,
        drop_pressed: false,
    };

    let events = simulate_tick(
        registry.clone(),
        &world,
        &hit_zones,
        None,
        GRAVITY,
        Some(weapon),
        0.0,
        &mut progress,
        &mut ai_runtime,
        &mover_colliders,
        &mut mover_states,
        &[],
        &command,
        |_| super::PostMovementCommand {
            aim_origin: Vec3::new(f32::NAN, 2.0, 0.0),
            aim_direction: Vec3::NEG_Z,
        },
        DT,
        None,
        |_| {},
    );

    assert!(
        events.weapon.is_empty(),
        "non-finite aim origin should suppress shot events"
    );
    let registry = registry.borrow();
    assert!(
        registry
            .get_component::<WeaponComponent>(weapon)
            .expect("weapon keeps component")
            .shoot_press_consumed,
        "non-finite origin must still advance semi-auto press state"
    );
    assert_eq!(
        registry
            .get_component::<HealthComponent>(target)
            .expect("target keeps health")
            .current,
        20.0,
        "non-finite origin must not damage a target"
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        ..ProptestConfig::default()
    })]

    #[test]
    fn simulate_tick_is_deterministic_for_random_command_stream(
        commands in prop::collection::vec(command_strategy(), TICK_COUNT)
    ) {
        let baseline = run_stream(&commands, SpawnOrder::AlphaThenBeta);
        let rerun = run_stream(&commands, SpawnOrder::AlphaThenBeta);
        let reversed_spawn = run_stream(&commands, SpawnOrder::BetaThenAlpha);

        assert_trigger_positive_anchors(&baseline);
        assert_runs_match(&rerun, &baseline);
        assert_runs_match(&reversed_spawn, &baseline);
    }
}

// E18 Task 7: tick/frame-timing-dependent Ordering rows, using `SimHarness`
// through `frame()`. A child module (not a sibling file at the crate level) so
// it can reach the private `SimHarness`/`SimFixture` construction surface the
// same way this file's own tests do — see the `pub(crate) mod determinism_tests`
// doc comment in `sim/mod.rs`.
mod e18_task7_tick_ordering;
