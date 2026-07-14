// Determinism coverage for the headless fixed-tick seam.
// See: context/lib/entity_model.md §5

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use glam::{Vec2, Vec3};
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
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::trigger_bindings::{
    BoundTriggerCommandKind, TriggerBindingTable, TriggerResidualHandle,
};
use crate::trigger_system::{PlayerId, TriggerEvent, TriggerEventEdge, TriggerSystem};
use crate::weapon::FireButtonState;
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::components::brain::{AiStateMap, AiTuning, BrainComponent, LogicalState};
use postretro_entities::components::health::{HealthComponent, Hitbox};
use postretro_entities::components::mesh::{
    AnimationState, InterruptPolicy, MeshAnimation, MeshComponent, RATE_CHANGE_EPSILON, RATE_MAX,
    RATE_MIN,
};
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{
    CrossingCondition, CrossingDescriptor, DataRegistry, EntityId, EntityRegistry, MoverCommand,
    NamedReaction, PrimitiveDescriptor, ReactionDescriptor, ReplicationScope, ScriptCtx,
    SlotOwnership, SlotRecord, SlotSchema, SlotTable, SlotType, SlotValue, Transform,
    TriggerActivation, TriggerFireMode, TriggerVolumeComponent,
};
use postretro_foundation::{
    AirParams, CapsuleParams, FallParams, FireMode, ForgivenessParams, GroundParams, IrNode,
    IrValue, PlayerMovementComponent, PlayerMovementDescriptor, ResolutionMode, SpeedParams,
    WeaponDescriptor,
};
use postretro_scripting_core::reaction_dispatch::ProgressTracker;
use postretro_scripting_core::state_crossings::CrossingDetector;

const TICK_COUNT: usize = 600;
const DT: f32 = 1.0 / 60.0;
const GRAVITY: f32 = -20.0;
const POSITION_EPSILON: f32 = 0.001;
const VELOCITY_EPSILON: f32 = 0.001;

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
            },
            fire_button: FireButtonState {
                pressed: self.fire_pressed,
                active: self.fire_active,
            },
            reload: false,
            use_pressed: false,
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
    ai: Vec<&'static str>,
    weapon: Vec<&'static str>,
    death: Vec<String>,
    authorized_shots: Vec<RecordedShot>,
    reload_deliveries: Vec<RecordedReload>,
    trigger_residuals: Vec<TriggerResidualHandle>,
    trigger_fires: Vec<RecordedTriggerFire>,
    trigger_command_fires: Vec<RecordedCommandFire>,
    predicate_crossing_fires: Vec<String>,
}

#[derive(Debug)]
struct SimRun {
    pawns: Vec<(Role, PawnOutcome)>,
    selected_player_health: f32,
    enemy_state: LogicalState,
    events: Vec<RecordedTick>,
    trigger_residual_counts: Vec<usize>,
    trigger_slot: Option<SlotValue>,
    ir_slot_timeline: Vec<Option<SlotValue>>,
    predicate_crossing_sequence: Vec<Vec<String>>,
    trigger_arm_target_armed: bool,
}

struct SimHarness {
    registry: Rc<RefCell<EntityRegistry>>,
    world: CollisionWorld,
    hit_zones: HitZoneStore,
    active_wieldable: EntityId,
    progress: ProgressTracker,
    ai_warned: HashSet<String>,
    mover_colliders: Vec<MoverCollider>,
    mover_states: MoverTickStateTable,
    trigger_system: TriggerSystem,
    trigger_bridge: TriggerVolumeBridge,
    trigger_bindings: TriggerBindingTable,
    trigger_script_ctx: ScriptCtx,
    trigger_slots: Rc<RefCell<SlotTable>>,
    crossing_detector: CrossingDetector,
    role_ids: Vec<(Role, EntityId)>,
    labels: HashMap<EntityId, EntityLabel>,
    selected_player: EntityId,
    remote_player: EntityId,
    enemy: EntityId,
    trigger_arm_target: EntityId,
}

impl SimHarness {
    fn new(spawn_order: SpawnOrder) -> Self {
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
            (spawn_weapon(&mut registry), enemy)
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
            vec![CrossingDescriptor {
                slot: None,
                // `triggered >= 4 && enabled >= 1`, expressed using the
                // shipped comparison/select vocabulary. Two activators each
                // execute two IR increments on tick one, so this yields one
                // false -> true observer edge per run.
                condition: CrossingCondition::Ir(IrNode::Select {
                    cond: Box::new(IrNode::Ge {
                        a: Box::new(IrNode::Input {
                            name: "determinism.triggered".to_string(),
                        }),
                        b: Box::new(IrNode::Const {
                            value: IrValue::Number(4.0),
                        }),
                    }),
                    a: Box::new(IrNode::Ge {
                        a: Box::new(IrNode::Input {
                            name: "determinism.enabled".to_string(),
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
                fire: vec!["determinismReady".to_string()],
            }],
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
        let mut trigger_bridge = TriggerVolumeBridge::new();
        trigger_bridge.insert_for_test(trigger_source, Vec3::splat(-4.0), Vec3::splat(4.0));

        let mut labels = HashMap::new();
        for (role, id) in &role_ids {
            labels.insert(*id, EntityLabel::Pawn(*role));
        }
        labels.insert(enemy, EntityLabel::Enemy);
        labels.insert(active_wieldable, EntityLabel::Weapon);
        labels.insert(trigger_source, EntityLabel::TriggerSource);
        labels.insert(trigger_arm_target, EntityLabel::TriggerArmTarget);

        Self {
            registry,
            world: floor_world(),
            hit_zones: HitZoneStore::new(),
            active_wieldable,
            progress: ProgressTracker::new(),
            ai_warned: HashSet::new(),
            mover_colliders: Vec::new(),
            mover_states: MoverTickStateTable::default(),
            trigger_system: TriggerSystem::default(),
            trigger_bridge,
            trigger_bindings,
            trigger_script_ctx,
            trigger_slots,
            crossing_detector,
            role_ids,
            labels,
            selected_player,
            remote_player,
            enemy,
            trigger_arm_target,
        }
    }

    fn tick(&mut self, command: RecordedCommand) -> RecordedTick {
        let sim_command = command.to_sim_command();
        let remote_pawn_commands = [RemotePawnCommand {
            pawn: self.remote_player,
            owner_client_id: 1,
            weapon: None,
            shot_id: None,
            fire_tick: 0,
            client_tick: 0,
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
            &mut self.ai_warned,
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
                use_edges: &trigger_use_edges,
            }),
        );
        let predicate_crossing_fires = self.crossing_detector.detect(&self.trigger_slots.borrow());
        self.record(events, predicate_crossing_fires)
    }

    /// Resolve every raw id a tick reports before it reaches the comparison.
    fn record(&self, events: TickEvents, predicate_crossing_fires: Vec<String>) -> RecordedTick {
        RecordedTick {
            movement: events.movement,
            ai: events.ai,
            weapon: events.weapon,
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
            trigger_residuals: events.trigger_residuals,
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

    fn selected_player_health(&self) -> f32 {
        self.registry
            .borrow()
            .get_component::<HealthComponent>(self.selected_player)
            .expect("selected player keeps health")
            .current
    }

    fn enemy_state(&self) -> LogicalState {
        self.registry
            .borrow()
            .get_component::<BrainComponent>(self.enemy)
            .expect("enemy keeps brain")
            .state
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
            tag: tag.map(str::to_string),
            on_complete: None,
            args,
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
    registry
        .set_component(
            id,
            BrainComponent {
                state: LogicalState::Idle,
                attack_cooldown_remaining_ms: 0.0,
                think_stride_counter: 0,
                death_despawn_remaining_ms: None,
                locomotion_moving: false,
                acquired_target: None,
                combat_slot: None,
                combat_slot_hold_ticks: 0,
                tuning: AiTuning {
                    detection_range: 8.0,
                    attack_range: 2.0,
                    leash_range: 12.0,
                    attack_damage: 7.0,
                    attack_cooldown_ms: 1000.0,
                    move_speed: 0.0,
                    death_despawn_ms: 1000.0,
                    states: AiStateMap {
                        idle: "idle".to_string(),
                        alert: "alert".to_string(),
                        attack: "attack".to_string(),
                        death: "death".to_string(),
                    },
                },
            },
        )
        .expect("enemy brain component should attach");
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
                range: 30.0,
                cooldown_ms: 80.0,
                fire_mode: FireMode::Semi,
                resolution: ResolutionMode::Hitscan,
                credit_source: None,
                resource: None,
            }),
        )
        .expect("weapon component should attach");
    id
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
        view_feel: None,
    }
}

fn floor_world() -> CollisionWorld {
    let points = vec![
        Point::new(-500.0, 0.0, -500.0),
        Point::new(500.0, 0.0, -500.0),
        Point::new(500.0, 0.0, 500.0),
        Point::new(-500.0, 0.0, 500.0),
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

fn driven_agent_tuning() -> AiTuning {
    AiTuning {
        detection_range: 40.0,
        attack_range: 2.0,
        leash_range: 48.0,
        attack_damage: 7.0,
        attack_cooldown_ms: 1000.0,
        move_speed: 3.5,
        death_despawn_ms: 1000.0,
        states: AiStateMap {
            idle: "idle".to_string(),
            alert: "locomotion".to_string(),
            attack: "attack".to_string(),
            death: "death".to_string(),
        },
    }
}

fn driven_agent_mesh(current_state: &str) -> MeshComponent {
    let state = |clip: &str, clip_index| AnimationState {
        clip: clip.to_string(),
        looping: true,
        crossfade_ms: 0.0,
        interrupt: InterruptPolicy::Smooth,
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

fn spawn_driven_agent(
    registry: &mut EntityRegistry,
    position: Vec3,
    state: LogicalState,
    animation_state: &str,
) -> EntityId {
    let enemy = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    registry
        .set_component(
            enemy,
            BrainComponent {
                state,
                attack_cooldown_remaining_ms: 0.0,
                think_stride_counter: 0,
                death_despawn_remaining_ms: None,
                locomotion_moving: false,
                acquired_target: None,
                combat_slot: None,
                combat_slot_hold_ticks: 0,
                tuning: driven_agent_tuning(),
            },
        )
        .expect("driven agent brain should attach");
    registry
        .set_component(enemy, AgentComponent::new(0.35, 1.8, 0.4, 3.5))
        .expect("driven agent steering component should attach");
    registry
        .set_component(enemy, driven_agent_mesh(animation_state))
        .expect("driven agent mesh should attach");
    enemy
}

#[allow(clippy::too_many_arguments)]
fn run_driven_agent_sim_tick(
    registry: Rc<RefCell<EntityRegistry>>,
    world: &CollisionWorld,
    nav_graph: &NavGraph,
    anim_time: f64,
    progress: &mut ProgressTracker,
    ai_warned: &mut HashSet<String>,
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
        },
        fire_button: FireButtonState {
            pressed: false,
            active: false,
        },
        reload: false,
        use_pressed: false,
    };
    let _ = simulate_tick(
        registry,
        world,
        &HitZoneStore::new(),
        Some(nav_graph),
        GRAVITY,
        None,
        anim_time,
        progress,
        ai_warned,
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
    );
}

#[test]
fn simulate_tick_scales_walk_rate_from_post_steering_velocity_and_skips_sub_epsilon_writes() {
    let world = floor_world();
    let nav_graph = open_floor_nav_graph();
    let mut progress = ProgressTracker::new();
    let mut ai_warned = HashSet::new();
    let mut mover_states = MoverTickStateTable::default();

    // Attack is not the alert-mapped locomotion state, so the sim-tick path
    // must restore its previously scaled playback rate to one.
    let non_walk_registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let non_walk_enemy = {
        let mut registry = non_walk_registry.borrow_mut();
        spawn_player(&mut registry, Vec3::new(7.0, 1.21, 5.0));
        let enemy = spawn_driven_agent(
            &mut registry,
            Vec3::new(5.0, 1.21, 5.0),
            LogicalState::Attack,
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
        &nav_graph,
        1.0,
        &mut progress,
        &mut ai_warned,
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

    let registry = Rc::new(RefCell::new(EntityRegistry::new()));
    let enemy = {
        let mut registry = registry.borrow_mut();
        spawn_player(&mut registry, Vec3::new(30.0, 1.21, 5.0));
        spawn_driven_agent(
            &mut registry,
            Vec3::new(5.0, 1.21, 5.0),
            LogicalState::Alert,
            "locomotion",
        )
    };

    // The first tick builds the route and drives actual steering. The second
    // tick makes the AI select the walk state from that driven velocity.
    run_driven_agent_sim_tick(
        registry.clone(),
        &world,
        &nav_graph,
        1.0,
        &mut progress,
        &mut ai_warned,
        &mut mover_states,
    );
    run_driven_agent_sim_tick(
        registry.clone(),
        &world,
        &nav_graph,
        2.0,
        &mut progress,
        &mut ai_warned,
        &mut mover_states,
    );

    {
        let registry = registry.borrow();
        let agent = registry
            .get_component::<AgentComponent>(enemy)
            .expect("driven agent keeps steering state");
        let animation = registry
            .get_component::<MeshComponent>(enemy)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        let speed_xz = Vec3::new(agent.velocity.x, 0.0, agent.velocity.z).length();
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
            .clone()
    };
    run_driven_agent_sim_tick(
        registry.clone(),
        &world,
        &nav_graph,
        3.0,
        &mut progress,
        &mut ai_warned,
        &mut mover_states,
    );
    assert_eq!(
        registry
            .borrow()
            .get_component::<MeshComponent>(enemy)
            .expect("driven agent keeps mesh"),
        &before,
        "a sub-epsilon post-steering rate change must leave rebase state untouched",
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
    let mut harness = SimHarness::new(spawn_order);
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
        events,
    }
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
        Some(&vec!["determinismReady".to_string()]),
        "the IR predicate must fire once when the IR-written slots first satisfy it"
    );
    assert!(
        run.predicate_crossing_sequence
            .iter()
            .skip(1)
            .all(Vec::is_empty),
        "the predicate remains true after tick one, so it must not re-fire without a false re-arm"
    );
    assert!(
        run.trigger_arm_target_armed,
        "the baseline trigger must arm its target"
    );
    assert!(
        run.events
            .iter()
            .any(|events| !events.trigger_fires.is_empty()),
        "the baseline must include at least one named trigger fire"
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
            spawn_weapon(&mut registry),
            spawn_target(&mut registry, Vec3::new(0.0, 2.0, -10.0)),
        )
    };
    let world = CollisionWorld::new();
    let hit_zones = HitZoneStore::new();
    let mut progress = ProgressTracker::new();
    let mut ai_warned = HashSet::new();
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
        },
        fire_button: FireButtonState {
            pressed: false,
            active: false,
        },
        reload: false,
        use_pressed: false,
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
        &mut ai_warned,
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
            spawn_weapon(&mut registry),
            spawn_target(&mut registry, Vec3::new(0.0, 2.0, -45.0)),
        )
    };
    let world = CollisionWorld::new();
    let hit_zones = HitZoneStore::new();
    let mut progress = ProgressTracker::new();
    let mut ai_warned = HashSet::new();
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
        },
        fire_button: FireButtonState {
            pressed: true,
            active: true,
        },
        reload: false,
        use_pressed: false,
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
        &mut ai_warned,
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
        let weapon = spawn_weapon(&mut registry);
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
    let mut ai_warned = HashSet::new();
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
        },
        fire_button: FireButtonState {
            pressed: true,
            active: true,
        },
        reload: false,
        use_pressed: false,
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
        &mut ai_warned,
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
            spawn_weapon(&mut registry),
            spawn_target(&mut registry, Vec3::new(0.0, 2.0, -10.0)),
        )
    };
    let world = CollisionWorld::new();
    let hit_zones = HitZoneStore::new();
    let mut progress = ProgressTracker::new();
    let mut ai_warned = HashSet::new();
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
        },
        fire_button: FireButtonState {
            pressed: true,
            active: true,
        },
        reload: false,
        use_pressed: false,
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
        &mut ai_warned,
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
