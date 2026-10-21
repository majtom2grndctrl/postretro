// Test-support scaffolding for the integrated prediction/reconciliation harness.
// Builds the shared world/descriptor fixtures and the `LoopbackHarness` that drives
// the real production seams end to end (client predict + send -> conditioner ->
// host sanitize/queue/resolve/tick/snapshot -> conditioner -> client reconcile/smooth)
// over the dev-only in-memory `PacketConditioner`.
// See: context/lib/networking.md · context/lib/testing_guide.md §4
//
// This is test infrastructure only (`#[cfg(test)]`), not production runtime state.
// It wires the genuine production seams — it never re-implements movement, the gap
// policy, or reconciliation, and it never instantiates the `sim::predict_reconcile`
// prototype type. The Transform-only bystanders it spawns live only in this test
// path and never carry `local_player`.

#![cfg(test)]

use glam::{Vec2, Vec3};

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use postretro_collision::{
    ConvexLeaf, EnvironmentView, EnvironmentVolume, EnvironmentVolumes, LeafPlane,
};
use postretro_combat_model::{EnvironmentTuning, FluidMovementTable, TuningPayload};
use postretro_net::harness::{LinkConfig, PacketConditioner, VirtualMillis};
use postretro_net::replication::{EntitySnapshot, ServerReplication};
use postretro_net::transport::NetServer;
use postretro_net::wire::{
    self, ClientMessage, InputCommand, NetworkId, RawSnapshotMessage, ServerControlMessage,
    WireFireButtonState, WireMovementInput,
};

use super::client::{ClientReplication, MoverCorrection};
use super::command_queue::{
    HostCommandQueues, MovementOwners, ResolvedPawnCommand,
    host_resolve_remote_commands_in_registry,
};
use super::prediction::{ClientPrediction, CorrectionClass};
use super::reconcile::reconcile_local_pawn_with_mover_history;
use super::replication::{ReplicableSet, host_register_loaded_movers, produce_owned_snapshots};
use crate::collision::CollisionWorld;
use crate::collision::moving::{CombinedCollisionWorld, MoverCollider};
use crate::kinematic_mover::{
    MoverTickStateTable, run_kinematic_mover_tick, run_mover_blocking_pass, run_mover_pin_pretest,
};
use crate::movement::{MovementEvents, MovementInput};
use crate::netcode::{
    ClientPredictionTickContext, NetworkIdAllocator, apply_installed_movement_tuning_to_armed_pawn,
    client_environment_view, client_fluid_table, client_predict_tick_with_sender,
    host_handle_client_message, replace_client_tuning, stamped_client_input,
    tuning_control_if_changed, tuning_payload_for_pawn,
};
use crate::scripting::builtins::net_descriptor::materialize_net_local_movement_component;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::sim::SimCommand;
use crate::trigger_system::{AuthoritativePlayer, PlayerId, TriggerSystem};
use crate::weapon::FireButtonState;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::provenance::{
    DescriptorComponentKind, DescriptorProvenance, DescriptorSpawnPath,
};
use postretro_entities::{
    EntityId, EntityRegistry, EntityTypeDescriptor, KinematicMoverComponent, KinematicMoverMode,
    MoverCommand, Transform, TriggerActivation, TriggerFireMode, TriggerVolumeComponent,
};
use postretro_foundation::{
    AirParams, BoolOrIr, CapsuleParams, CrouchParams, DashParams, FallParams, FluidRegistry,
    ForgivenessParams, GroundParams, GroundRef, HealthDescriptor, NumberOrIr,
    PlayerMovementComponent, PlayerMovementDescriptor, SlideParams, SpeedParams, SwimParams,
    VaultParams,
};

pub(crate) const DT: f32 = 1.0 / 60.0;
/// "No corrections": every recorded correction is at
/// most this many metres under [`light_link`].
pub(crate) const NO_CORRECTION_M: f32 = 1.0e-4;
pub(crate) const GRAVITY: f32 = -20.0;
pub(crate) const TICK_MS: VirtualMillis = 16; // ~16.667 ms; integer ms keeps the clock exact-ish
pub(crate) const CLIENT_ID: u64 = 1;
pub(crate) const START: Vec3 = Vec3::new(0.0, 1.21, 0.0);
pub(crate) const MOVING_PLATFORM_ID: u32 = 77;
pub(crate) const MOVING_PLATFORM_SPEED_MPS: f32 = 0.6;
pub(crate) const ROTATING_PLATFORM_INITIAL_RATE_RAD_S: f32 = 1.2;
pub(crate) const ROTATING_PLATFORM_ACCEL_RAD_S2: f32 = 2.4;
pub(crate) const ROTATING_PLATFORM_RIDER_START: Vec3 = Vec3::new(1.0, 1.21, 0.0);

/// Host playout (jitter buffer) warmup, in host ticks: the host buffers inbound
/// commands for this many ticks before it begins resolving them, so the buffer has
/// depth ≥ the link's worst-case one-way latency + jitter (45 + 60 ms ≈ 7 ticks).
/// Sized at the 7-tick minimum that keeps the buffer non-empty under worst-case
/// jitter — larger only inflates the client's steady prediction lead (and thus the
/// smoothed correction magnitude) without changing convergence. See the rationale in
/// `host_tick`.
pub(crate) const PLAYOUT_WARMUP_TICKS: u32 = 7;

/// Settle margin, in host ticks, between the host's resolved cursor reaching the last
/// sent command tick and freezing the drain *target* tick. Covers the gap policy's
/// hold window (INPUT_HOLD_TICKS) plus enough ticks for neutral-input deceleration to
/// fall well below the gate tolerance, so the frozen-target snapshot reflects an
/// effectively-at-rest pose. Bounded + deterministic (vs. waiting for exact stillness,
/// whose geometric decay tail is unboundedly slow). See `drain_step`.
pub(crate) const DRAIN_SETTLE_TICKS: u32 = 30;

/// The mandated automated harness profile, applied in BOTH directions:
/// 45 ms base + up to 60 ms jitter (a 45..105 ms one-way range, ≈150 ms mean RTT),
/// 5% loss, fixed seed. Matches the Phase 2 latency harness profile exactly.
pub(crate) fn mandated_link() -> LinkConfig {
    LinkConfig {
        delay: 45,
        jitter: 60,
        loss_probability: 0.05,
        seed: 0x1502,
    }
}

/// A near-perfect link for scenario tests that exercise the gap policy, stale and
/// duplicate seams, and exact prediction deterministically without the full
/// latency profile's loss obscuring the assertion. A small fixed delay keeps the
/// predict→ack loop realistic (the client predicts ahead of the host) without
/// jitter or loss.
pub(crate) fn light_link() -> LinkConfig {
    LinkConfig {
        delay: 32,
        jitter: 0,
        loss_probability: 0.0,
        seed: 0x6010,
    }
}

/// A large flat floor so the pawn stays grounded for the whole run — keeps the
/// scenario about prediction/reconciliation, not terrain interaction.
pub(crate) fn floor_world() -> CollisionWorld {
    let points = vec![
        Vec3::new(-500.0, 0.0, -500.0),
        Vec3::new(500.0, 0.0, -500.0),
        Vec3::new(500.0, 0.0, 500.0),
        Vec3::new(-500.0, 0.0, 500.0),
    ];
    let triangles = vec![[0, 2, 1], [0, 3, 2]];
    CollisionWorld::from_triangles_for_test(points, triangles)
}

/// Flat floor joined continuously to a downhill facet at `z = -1`. Forward
/// movement crosses the crease, so prediction can hold a newer contact normal
/// than an authoritative baseline restored on the flat facet.
pub(crate) fn faceted_slope_world(slope: f32) -> CollisionWorld {
    let far_y = slope * (-500.0 + 1.0);
    let points = vec![
        Vec3::new(-500.0, far_y, -500.0),
        Vec3::new(500.0, far_y, -500.0),
        Vec3::new(500.0, 0.0, -1.0),
        Vec3::new(-500.0, 0.0, -1.0),
        Vec3::new(-500.0, 0.0, -1.0),
        Vec3::new(500.0, 0.0, -1.0),
        Vec3::new(500.0, 0.0, 500.0),
        Vec3::new(-500.0, 0.0, 500.0),
    ];
    CollisionWorld::from_triangles_for_test(
        points,
        vec![[0, 2, 1], [0, 3, 2], [4, 6, 5], [4, 7, 6]],
    )
}

pub(crate) fn faceted_floor_height(slope: f32, z: f32) -> f32 {
    if z < -1.0 { slope * (z + 1.0) } else { 0.0 }
}

pub(crate) fn downhill_facet_normal(slope: f32) -> Vec3 {
    Vec3::new(0.0, 1.0, -slope).normalize()
}

/// An axis-aligned box environment volume that sets only gravity, as the level
/// loader would hand both peers.
pub(crate) fn gravity_box_volume(min: Vec3, max: Vec3, gravity: f32) -> EnvironmentVolume {
    let plane = |normal: Vec3, distance: f32| LeafPlane { normal, distance };
    EnvironmentVolume {
        source_entity: 1,
        priority: 0,
        gravity: Some(gravity),
        push: None,
        force: 1.0,
        fluid: None,
        brushes: vec![ConvexLeaf {
            planes: vec![
                plane(Vec3::X, max.x),
                plane(Vec3::NEG_X, -min.x),
                plane(Vec3::Y, max.y),
                plane(Vec3::NEG_Y, -min.y),
                plane(Vec3::Z, max.z),
                plane(Vec3::NEG_Z, -min.z),
            ],
            aabb_min: min,
            aabb_max: max,
        }],
    }
}

/// One level volume record as every peer loads it from the level file: its
/// fluid by NAME. `volume.fluid` is ignored; each peer resolves `fluid` against
/// its own active fluid table ([`resolve_environment_records`]).
#[derive(Debug, Clone)]
pub(crate) struct NamedEnvironmentVolume {
    pub(crate) volume: EnvironmentVolume,
    pub(crate) fluid: Option<String>,
}

/// Resolve level records against a fluid table, as the binary's level install
/// does: a fluid's index is its position in `fluid_names`, and a name the table
/// lacks loads dry while the volume's gravity and push still apply.
pub(crate) fn resolve_environment_records(
    records: &[NamedEnvironmentVolume],
    fluid_names: &[String],
) -> EnvironmentVolumes {
    EnvironmentVolumes::build(
        records
            .iter()
            .map(|record| EnvironmentVolume {
                fluid: record
                    .fluid
                    .as_deref()
                    .and_then(|name| fluid_names.iter().position(|declared| declared == name))
                    .and_then(|index| u16::try_from(index).ok()),
                ..record.volume.clone()
            })
            .collect(),
    )
}

/// The shared player descriptor (dash-capable) both ends materialize their pawn
/// from. Both peers load identical local descriptors, so movement tuning comes from
/// the shared table; the host's level gravity alone reaches the client through
/// Control-channel `TuningPayload` delivery. Snapshots carry the mutable tick state.
pub(crate) fn player_descriptor() -> PlayerMovementDescriptor {
    PlayerMovementDescriptor {
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
        dash: Some(DashParams {
            boost_speed: NumberOrIr::Literal(18.0),
            momentum_retention: NumberOrIr::Literal(0.65),
            steer_control: NumberOrIr::Literal(0.2),
            dash_drag: NumberOrIr::Literal(18.0),
            cooldown_ms: NumberOrIr::Literal(250.0),
            air_dashes: 0,
            preserve_vertical: BoolOrIr::Literal(false),
        }),
        forgiveness: Some(ForgivenessParams {
            coyote_ms: 0.0,
            jump_buffer_ms: 0.0,
        }),
        crouch: Some(CrouchParams {
            half_height: 0.4,
            eye_height: 0.2,
            transition_rate: 15.0,
        }),
        slide: Some(SlideParams {
            min_speed: 8.0,
            slide_drag: 2.0,
            slope_assist: 1.0,
            steer_rate: 180.0,
            entry_boost: 1.5,
            min_duration_ms: 250.0,
        }),
        swim: None,
        vault: None,
        view_feel: None,
    }
}

pub(crate) fn component() -> PlayerMovementComponent {
    PlayerMovementComponent::from_descriptor(&player_descriptor())
}

/// The descriptor class both peers share for the local pawn (mirrors the production
/// default the host stamps and the client falls back to).
pub(crate) const ENTITY_CLASS: &str = "player";

/// The shared descriptor table both peers load (same content on both ends). The host
/// materializes its authoritative pawn from this table's `movement` block; the client
/// materializes its LOCAL pawn's component from the SAME table via the real
/// `materialize_net_local_movement_component` path — so the two components are
/// bit-identical and the wire mutable subset merges onto a matching base. The single
/// `"player"` descriptor wraps the harness `player_descriptor()` movement block.
pub(crate) fn entity_descriptors() -> Vec<EntityTypeDescriptor> {
    vec![EntityTypeDescriptor {
        faction: None,
        tolerance: None,
        canonical_name: Some(ENTITY_CLASS.to_string()),
        inventory: None,
        light: None,
        emitter: None,
        movement: Some(player_descriptor()),
        weapon: None,
        touchable: None,
        mesh: None,
        health: None,
        behavior: None,
    }]
}

/// The `DescriptorProvenance` a real net-slot pawn carries, so the host snapshot
/// production (`produce_owned_snapshots` -> `descriptor_entity_class`) stamps
/// `entity_class = Some("pawn:player")` on the owned snapshot — exactly as
/// `spawn_net_slot_pawn` does in production.
fn net_slot_provenance() -> DescriptorProvenance {
    DescriptorProvenance {
        canonical_name: ENTITY_CLASS.to_string(),
        owned_components: [DescriptorComponentKind::Movement].into_iter().collect(),
        map_overrides: Default::default(),
        spawn_path: DescriptorSpawnPath::NetworkSlot,
    }
}

/// A forward-walking sim command at the given facing, dash optional. The harness
/// stamps the wire `client_tick` from `ClientPrediction::next_client_tick`.
pub(crate) fn forward_command(dash_pressed: bool) -> SimCommand {
    SimCommand {
        input_tick: 0,
        secondary_button: crate::weapon::FireButtonState {
            pressed: false,
            active: false,
        },
        activation: postretro_foundation::ActivationInput::default(),
        movement: MovementInput {
            wish_dir: Vec2::new(0.0, 1.0),
            jump_pressed: false,
            dash_pressed,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
            facing_pitch: 0.0,
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
    }
}

pub(crate) fn idle_command() -> SimCommand {
    SimCommand {
        input_tick: 0,
        secondary_button: crate::weapon::FireButtonState {
            pressed: false,
            active: false,
        },
        activation: postretro_foundation::ActivationInput::default(),
        movement: MovementInput {
            wish_dir: Vec2::ZERO,
            jump_pressed: false,
            dash_pressed: false,
            running: false,
            crouch_intent: false,
            facing_yaw: 0.0,
            facing_pitch: 0.0,
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
    }
}

/// One remote Use rising edge. Keep the full command and movement mirror in
/// lockstep so the client-prediction and client-to-host input boundaries exercise
/// the same authored edge.
pub(crate) fn use_command() -> SimCommand {
    let mut command = idle_command();
    command.movement.use_pressed = true;
    command.use_pressed = true;
    command
}

/// An owned `InputCommand` directly at a chosen `client_tick` — used by the
/// scenario tests that inject duplicate / stale / malformed input at the
/// `host_handle_client_message` drain seam without going through the conditioner.
pub(crate) fn input_at(client_tick: u32, wish_forward: f32) -> InputCommand {
    InputCommand {
        teleport_yaw_stamp: 0.0,
        secondary_button: postretro_net::wire::WireFireButtonState {
            pressed: false,
            active: false,
        },
        activation: postretro_net::wire::WireActivationInput::default(),
        client_tick,
        movement: WireMovementInput {
            wish_dir: [0.0, wish_forward],
            jump_pressed: false,
            dash_pressed: false,
            running: true,
            crouch_intent: false,
            facing_yaw: 0.0,
            use_pressed: false,
            drop_pressed: false,
            aim_pitch: 0.0,
            firing_slot: 0,
        },
        fire_button: WireFireButtonState {
            pressed: false,
            active: false,
        },
        reload: false,
    }
}

/// A bystander entity (no `PlayerMovement`, no `local_player`) carrying a
/// zero-HP `HealthComponent`. If the full `simulate_tick` death sweep ran during
/// prediction/reconciliation it would despawn this entity; its survival is the
/// observable seam proving the movement-only path never reached the registry-wide
/// systems (testing_guide: assert behaviour at the seam).
pub(crate) fn spawn_dead_bystander(registry: &mut EntityRegistry) -> EntityId {
    let id = registry.spawn(Transform {
        position: Vec3::new(50.0, 1.0, 50.0),
        ..Transform::default()
    });
    let mut health = HealthComponent::from_descriptor(&HealthDescriptor {
        max: 100.0,
        hitbox: None,
        zone_multipliers: std::collections::HashMap::new(),
    });
    health.current = 0.0; // would be swept by run_death_sweep if it ran
    registry.set_component(id, health).unwrap();
    id
}

/// One host fixed tick as the client's pawn ended it: the command resolution
/// that drove it (after the teleport-frame rotation) and the authoritative pose,
/// avatar yaw and teleport record.
#[derive(Debug, Clone)]
pub(crate) struct HostTickTrace {
    pub(crate) server_tick: u32,
    pub(crate) resolved: Option<ResolvedPawnCommand>,
    pub(crate) position: Vec3,
    pub(crate) yaw: f32,
    pub(crate) teleport: postretro_foundation::TeleportState,
}

impl HostTickTrace {
    fn capture(
        registry: &EntityRegistry,
        pawn: EntityId,
        server_tick: u32,
        resolved: Option<ResolvedPawnCommand>,
    ) -> Self {
        let transform = registry.get_component::<Transform>(pawn).unwrap();
        // Yaw of `Quat::from_rotation_y(yaw)`: its forward is (-sin, 0, -cos).
        let forward = transform.rotation * Vec3::NEG_Z;
        Self {
            server_tick,
            resolved,
            position: transform.position,
            yaw: (-forward.x).atan2(-forward.z),
            teleport: registry
                .get_component::<PlayerMovementComponent>(pawn)
                .unwrap()
                .teleport
                .clone(),
        }
    }
}

/// The two-sided, deterministic loopback harness driving the genuine Task 1-5
/// seams through the in-memory packet conditioner. One conditioner per direction,
/// both seeded with the same mandated profile. A virtual ms clock the harness
/// advances drives delivery; no wall-clock time is ever read.
pub(crate) struct LoopbackHarness {
    pub(crate) world: CollisionWorld,
    /// The client's own static world, when a scenario needs one: installed
    /// teleporter bindings name each peer's own entities, so a teleporter
    /// scenario gives the client its own copy of the level. `None` shares
    /// [`Self::world`], which every other scenario relies on.
    pub(crate) client_world: Option<CollisionWorld>,
    /// The level's volume records, fluids by name, identical on both peers as
    /// the digest-bound level file is. Empty by default.
    pub(crate) environment_records: Vec<NamedEnvironmentVolume>,
    /// Each peer's installed environment volumes, built separately from the
    /// records against that peer's active fluid table.
    pub(crate) host_environment_volumes: EnvironmentVolumes,
    pub(crate) client_environment_volumes: EnvironmentVolumes,
    /// The host's manifest fluids (its `DataRegistry.fluids`) and the movement
    /// table its volumes resolved against and its tuning sends.
    pub(crate) host_fluids: FluidRegistry,
    pub(crate) host_fluid_table: FluidMovementTable,
    /// The client's own manifest fluids. Only the local presentation lookup
    /// reads them; volume resolution and movement never do.
    pub(crate) client_fluids: FluidRegistry,
    /// The fluid names the client's volumes last resolved against: the installed
    /// host table's, empty before a payload installs.
    pub(crate) client_volume_fluid_names: Vec<String>,
    /// The host's live level gravity (its `script_ctx.gravity`): the host pawn
    /// integrates it, and every tuning send reads it. A test may change it
    /// between steps, as a `setGravity` reaction would.
    pub(crate) host_level_gravity: f32,
    /// The client's own level gravity. Prediction and replay never read it; the
    /// client takes the host's value from its installed tuning payload.
    pub(crate) client_level_gravity: f32,

    // --- Host side (authoritative) ---
    pub(crate) host_registry: EntityRegistry,
    pub(crate) host_pawn: EntityId,
    /// A `NetServer` is constructed only to satisfy `host_handle_client_message`'s
    /// signature — the `Input` arm never touches it (no traffic flows over its
    /// socket). It keeps the harness routing inbound input through the genuine
    /// per-message dispatcher seam the task names, not a private shortcut.
    pub(crate) server: NetServer,
    pub(crate) command_queues: HostCommandQueues,
    pub(crate) owners: MovementOwners,
    pub(crate) replicable: ReplicableSet,
    pub(crate) allocator: NetworkIdAllocator,
    pub(crate) host_loaded_movers: HashSet<EntityId>,
    pub(crate) server_replication: ServerReplication,
    /// M15 Phase 3.5 host state tracker — required by `host_handle_client_message`'s
    /// signature. The movement harness drives no replicated state slots, so it stays
    /// empty; it exists so the genuine per-message dispatcher seam compiles and runs.
    pub(crate) server_state: super::state_slots::HostStateReplication,
    pub(crate) server_tick: u32,
    pub(crate) snapshot_sequence: u32,
    /// The production per-slot tuning change detector.
    pub(crate) host_last_sent_tuning: HashMap<u64, TuningPayload>,
    /// Host ticks elapsed since start, gating the playout warmup (the host begins
    /// resolving client commands only after [`PLAYOUT_WARMUP_TICKS`]).
    pub(crate) host_ticks_elapsed: u32,

    // --- Client side (predicting) ---
    pub(crate) client_registry: EntityRegistry,
    pub(crate) client_replication: ClientReplication,
    pub(crate) prediction: ClientPrediction,
    /// The client's installed host tuning and its generation, replaced through the
    /// production `replace_client_tuning` install path.
    pub(crate) client_tuning: Option<Box<TuningPayload>>,
    pub(crate) client_tuning_generation: u64,
    /// Tuning payloads the client installed, in arrival order.
    pub(crate) client_tuning_installs: u32,
    /// Every baseline refresh request the client sent, immediate and resent, in
    /// send order.
    pub(crate) sent_refresh_requests: Vec<wire::BaselineRefreshRequest>,
    /// The shared descriptor table the client materializes its LOCAL pawn from on
    /// arming — the SAME table both peers load in production. Drives the real
    /// `materialize_net_local_movement_component` path (no harness stub).
    pub(crate) descriptors: Vec<EntityTypeDescriptor>,
    /// The client pawn's mapped `EntityId`, set once the first baseline arms it.
    pub(crate) client_pawn: Option<EntityId>,
    pub(crate) host_pawn_network_id: NetworkId,
    pub(crate) host_mover: Option<EntityId>,
    pub(crate) client_mover: Option<EntityId>,
    pub(crate) mover_network_id: Option<NetworkId>,
    pub(crate) mover_colliders: Vec<MoverCollider>,
    pub(crate) host_mover_states: MoverTickStateTable,
    pub(crate) host_trigger_system: TriggerSystem,
    pub(crate) host_trigger_bridge: TriggerVolumeBridge,
    pub(crate) host_use_trigger: Option<EntityId>,
    pub(crate) client_mover_states: MoverTickStateTable,
    pub(crate) latest_mover_corrections: Vec<MoverCorrection>,
    pub(crate) latest_local_corrections: Vec<f32>,
    /// The class of each local reconcile in the latest `client_receive`, in
    /// order, beside [`Self::latest_local_corrections`].
    pub(crate) latest_local_correction_classes: Vec<CorrectionClass>,
    /// The network ids carried as full baselines by the snapshots the latest
    /// `client_receive` applied — first records and re-baselines alike.
    pub(crate) latest_full_baselines: Vec<u32>,
    /// One record per host fixed tick after the playout warmup: the resolved
    /// command (if any) and the client pawn's authoritative state after the tick.
    pub(crate) host_trace: Vec<HostTickTrace>,
    /// One `(acked_tick, last_sent_client_tick)` pair per local reconcile in the
    /// latest `client_receive`: the replay window is the client ticks after the
    /// ack up to the last one sent.
    pub(crate) latest_reconcile_windows: Vec<(Option<u32>, Option<u32>)>,
    /// The latest client step's forward-predicted movement events, as
    /// production `client_predict_tick` hands them to the frame drain; default
    /// when nothing was predicted. Reconcile replay never writes here.
    pub(crate) latest_predicted_movement_events: MovementEvents,

    // --- Bystander death-sweep guards (one per registry) ---
    pub(crate) host_bystander: EntityId,
    pub(crate) client_bystander: EntityId,

    // --- Link conditioners (one per direction) + virtual clock ---
    pub(crate) to_server: PacketConditioner,
    pub(crate) to_client: PacketConditioner,
    /// The reliable-ordered Control lane: the link's one-way delay with no jitter
    /// or loss, standing in for reliable delivery.
    pub(crate) to_client_control: PacketConditioner,
    pub(crate) virtual_ms: VirtualMillis,

    /// The highest `client_tick` the client has actually sent. Drain completeness
    /// is measured against this (the host cursor must reach it and the client must
    /// process a snapshot acking it).
    pub(crate) last_sent_client_tick: Option<u32>,
    /// The highest server tick the client has acked back (read off the acks the
    /// client produced). `ClientReplication` keeps this internally; the harness
    /// mirrors it from the ack stream to drive the drain condition without adding a
    /// production accessor.
    pub(crate) client_acked_server_tick: u32,
    /// The frozen drain target: the host `server_tick` `DRAIN_SETTLE_TICKS` after the
    /// resolved cursor reached `last_sent_client_tick`. `None` until that settle
    /// completes. The client must ack at-or-past this fixed tick for the drain to be
    /// complete — it is not the (still-advancing) live `server_tick`.
    pub(crate) drain_target_tick: Option<u32>,
    /// Host ticks elapsed since the resolved cursor first reached `last_sent` during
    /// the drain, counting up to [`DRAIN_SETTLE_TICKS`] before the target freezes.
    pub(crate) ticks_since_cursor_caught: u32,
}

impl LoopbackHarness {
    /// Build the harness for a given link profile. The host owns one descriptor-
    /// backed pawn registered as the client's movement authority; the server
    /// replication tracker has the client registered (the accepted-client gate the
    /// production path enforces). Both registries also hold a zero-HP bystander.
    pub(crate) fn new(link: LinkConfig) -> Self {
        let world = floor_world();

        // Host pawn.
        let mut host_registry = EntityRegistry::new();
        let host_pawn = host_registry.spawn(Transform {
            position: START,
            ..Transform::default()
        });
        host_registry.set_component(host_pawn, component()).unwrap();
        // Stamp net-slot provenance so the REAL host snapshot production stamps
        // `entity_class = Some("pawn:player")` on the owned snapshot (the production path
        // reads `DescriptorProvenance` with `spawn_path == NetworkSlot`).
        host_registry
            .set_component(host_pawn, net_slot_provenance())
            .unwrap();
        let host_bystander = spawn_dead_bystander(&mut host_registry);

        // A `NetServer` bound to an ephemeral socket. No packets ever flow over it;
        // it only satisfies the `host_handle_client_message` signature.
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind ephemeral udp socket");
        let addr: SocketAddr = socket.local_addr().expect("resolve bound addr");
        let server = NetServer::new(socket, addr, 8, Duration::from_secs(1), Some([0x5a; 32]))
            .expect("construct harness NetServer");

        let command_queues = HostCommandQueues::new();
        let mut owners = MovementOwners::new();
        owners.set(host_pawn, CLIENT_ID);
        let mut replicable = ReplicableSet::new();
        replicable.register(host_pawn);

        let mut allocator = NetworkIdAllocator::new();
        let host_pawn_network_id = allocator.stamp(host_pawn);

        let mut server_replication = ServerReplication::new();
        server_replication.register_client(CLIENT_ID);

        let mut server_state = super::state_slots::HostStateReplication::new();
        server_state.register_client(CLIENT_ID);

        // Client side: starts empty — the first applied baseline spawns + arms the
        // local pawn through the real apply path. The client bystander is local.
        let mut client_registry = EntityRegistry::new();
        let client_bystander = spawn_dead_bystander(&mut client_registry);

        let control_link = LinkConfig {
            jitter: 0,
            loss_probability: 0.0,
            ..link
        };

        Self {
            world,
            client_world: None,
            environment_records: Vec::new(),
            host_environment_volumes: EnvironmentVolumes::empty(),
            client_environment_volumes: EnvironmentVolumes::empty(),
            host_fluids: FluidRegistry::default(),
            host_fluid_table: FluidMovementTable::default(),
            client_fluids: FluidRegistry::default(),
            client_volume_fluid_names: Vec::new(),
            host_level_gravity: GRAVITY,
            client_level_gravity: GRAVITY,
            host_registry,
            host_pawn,
            server,
            command_queues,
            owners,
            replicable,
            allocator,
            host_loaded_movers: HashSet::new(),
            server_replication,
            server_state,
            server_tick: 0,
            snapshot_sequence: 0,
            host_last_sent_tuning: HashMap::new(),
            host_ticks_elapsed: 0,
            client_registry,
            client_replication: ClientReplication::new(),
            prediction: ClientPrediction::new(),
            client_tuning: None,
            sent_refresh_requests: Vec::new(),
            client_tuning_generation: 0,
            client_tuning_installs: 0,
            descriptors: entity_descriptors(),
            client_pawn: None,
            host_pawn_network_id,
            host_mover: None,
            client_mover: None,
            mover_network_id: None,
            mover_colliders: Vec::new(),
            host_mover_states: MoverTickStateTable::default(),
            host_trigger_system: TriggerSystem::default(),
            host_trigger_bridge: TriggerVolumeBridge::new(),
            host_use_trigger: None,
            client_mover_states: MoverTickStateTable::default(),
            latest_mover_corrections: Vec::new(),
            latest_local_corrections: Vec::new(),
            latest_local_correction_classes: Vec::new(),
            latest_full_baselines: Vec::new(),
            host_trace: Vec::new(),
            latest_reconcile_windows: Vec::new(),
            latest_predicted_movement_events: MovementEvents::default(),
            host_bystander,
            client_bystander,
            to_server: PacketConditioner::new(link),
            to_client: PacketConditioner::new(link),
            to_client_control: PacketConditioner::new(control_link),
            virtual_ms: 0,
            last_sent_client_tick: None,
            client_acked_server_tick: 0,
            drain_target_tick: None,
            ticks_since_cursor_caught: 0,
        }
    }

    /// Install the same fluidless environment volume records on both peers,
    /// each building its own query set as each peer does from its own copy of
    /// the level.
    pub(crate) fn with_environment_volumes(
        link: LinkConfig,
        volumes: Vec<EnvironmentVolume>,
    ) -> Self {
        assert!(
            volumes.iter().all(|volume| volume.fluid.is_none()),
            "fluid volumes name their fluid: use `with_fluid_tables`"
        );
        let records = volumes
            .into_iter()
            .map(|volume| NamedEnvironmentVolume {
                volume,
                fluid: None,
            })
            .collect();
        Self::with_fluid_tables(
            link,
            records,
            FluidRegistry::default(),
            FluidRegistry::default(),
        )
    }

    /// Seed the host and the client with independent fluid declarations, as two
    /// peers on the same mod id whose manifests differ, over the same level
    /// records. The host's volumes resolve against its registry; the client's
    /// resolve against nothing (dry) until the host's table arrives through the
    /// tuning-payload install, and never against `client_fluids`.
    pub(crate) fn with_fluid_tables(
        link: LinkConfig,
        records: Vec<NamedEnvironmentVolume>,
        host_fluids: FluidRegistry,
        client_fluids: FluidRegistry,
    ) -> Self {
        let mut h = Self::new(link);
        h.environment_records = records;
        h.client_fluids = client_fluids;
        h.client_environment_volumes =
            resolve_environment_records(&h.environment_records, &h.client_volume_fluid_names);
        h.host_reload_fluids(host_fluids);
        h.host_environment_volumes =
            resolve_environment_records(&h.environment_records, h.host_fluid_table.names());
        h
    }

    /// Give both peers' shared player descriptor a swim block, as a mod whose
    /// player can swim, and rematerialize the host pawn from it. Call before the
    /// client arms: the client materializes its pawn from the host's tuning.
    pub(crate) fn with_player_swim(mut self, swim: SwimParams) -> Self {
        for descriptor in &mut self.descriptors {
            if let Some(movement) = descriptor.movement.as_mut() {
                movement.swim = Some(swim.clone());
            }
        }
        let movement = self.descriptors[0]
            .movement
            .as_ref()
            .expect("the shared player descriptor has movement");
        self.host_registry
            .set_component(
                self.host_pawn,
                PlayerMovementComponent::from_descriptor(movement),
            )
            .unwrap();
        self
    }

    /// Give both peers' shared player descriptor a vault block and
    /// rematerialize the host pawn from it, as [`Self::with_player_swim`] does.
    pub(crate) fn with_player_vault(mut self, vault: VaultParams) -> Self {
        for descriptor in &mut self.descriptors {
            if let Some(movement) = descriptor.movement.as_mut() {
                movement.vault = Some(vault.clone());
            }
        }
        let movement = self.descriptors[0]
            .movement
            .as_ref()
            .expect("the shared player descriptor has movement");
        self.host_registry
            .set_component(
                self.host_pawn,
                PlayerMovementComponent::from_descriptor(movement),
            )
            .unwrap();
        self
    }

    /// HOST: commit a manifest whose fluids are `fluids`, as a host reload does:
    /// the registry and its movement table are replaced, and the installed
    /// volumes re-resolve when the table's names changed. The next host frame's
    /// whole-struct tuning dedupe resends the new table.
    pub(crate) fn host_reload_fluids(&mut self, fluids: FluidRegistry) {
        let table = FluidMovementTable::from_registry(&fluids);
        if table.names() != self.host_fluid_table.names() {
            self.host_environment_volumes =
                resolve_environment_records(&self.environment_records, table.names());
        }
        self.host_fluids = fluids;
        self.host_fluid_table = table;
    }

    /// The host's tick view: its volumes under its live level gravity and the
    /// viscosities of the table they resolved against.
    pub(crate) fn host_environment_view(&self) -> EnvironmentView<'_> {
        EnvironmentView::new(
            self.host_level_gravity,
            &self.host_environment_volumes,
            self.host_fluid_table.viscosities(),
        )
    }

    /// The client's prediction and replay view, through the production
    /// `client_environment_view`. `None` before the host's tuning installs.
    pub(crate) fn client_environment_view(&self) -> Option<EnvironmentView<'_>> {
        client_environment_view(
            self.client_tuning.as_deref(),
            &self.client_environment_volumes,
        )
    }

    /// CLIENT: after a tuning install, re-resolve the client's volumes when the
    /// installed host table's names differ from the ones they last resolved
    /// against, as the binary does after every install.
    fn client_sync_fluid_table(&mut self) {
        let names = client_fluid_table(self.client_tuning.as_deref())
            .map(FluidMovementTable::names)
            .unwrap_or_default();
        if names == self.client_volume_fluid_names.as_slice() {
            return;
        }
        self.client_volume_fluid_names = names.to_vec();
        self.client_environment_volumes =
            resolve_environment_records(&self.environment_records, &self.client_volume_fluid_names);
    }

    /// Seed the host and the client with independent level gravity, as two peers
    /// whose `initialGravity` (or a pre-join `setGravity`) differ.
    pub(crate) fn with_level_gravity(
        link: LinkConfig,
        host_level_gravity: f32,
        client_level_gravity: f32,
    ) -> Self {
        let mut h = Self::new(link);
        h.host_level_gravity = host_level_gravity;
        h.client_level_gravity = client_level_gravity;
        h
    }

    /// The level gravity the client's installed host tuning carries, if any.
    pub(crate) fn client_installed_level_gravity(&self) -> Option<f32> {
        self.client_tuning
            .as_deref()
            .map(|payload| payload.environment.level_gravity)
    }

    pub(crate) fn with_moving_platform(link: LinkConfig) -> Self {
        Self::with_platform_initial_phase(link, false)
    }

    /// Shared authored motion for the steady prediction/drift gate. Host-only
    /// trigger transitions use the stopped `with_moving_platform` fixture instead.
    pub(crate) fn with_started_moving_platform(link: LinkConfig) -> Self {
        Self::with_platform_initial_phase(link, true)
    }

    fn with_platform_initial_phase(link: LinkConfig, started: bool) -> Self {
        let mut h = Self::new(link);
        h.world = CollisionWorld::new();

        let host_mover = spawn_platform_mover(&mut h.host_registry, started);
        let client_mover = spawn_platform_mover(&mut h.client_registry, started);
        h.host_registry
            .set_tags(host_mover, vec!["harness-platform".into()])
            .expect("tag host mover");
        spawn_harness_trigger(
            &mut h.host_registry,
            &mut h.host_trigger_bridge,
            TriggerActivation::Touch,
            MoverCommand::Start,
        );
        h.host_use_trigger = Some(spawn_harness_trigger(
            &mut h.host_registry,
            &mut h.host_trigger_bridge,
            TriggerActivation::Use,
            MoverCommand::GoToPathNode("finish".into()),
        ));
        h.mover_colliders
            .push(platform_collider(MOVING_PLATFORM_ID));
        host_register_loaded_movers(
            &h.host_registry,
            &mut h.allocator,
            &mut h.replicable,
            &mut h.host_loaded_movers,
        );
        h.mover_network_id = Some(h.allocator.stamp(host_mover));
        h.host_mover = Some(host_mover);
        h.client_mover = Some(client_mover);
        h
    }

    /// A started mover carrying a ledge: a riser facing +Z at `riser_z`, its top
    /// `top_y` above the static floor, sliding along X at the platform speed
    /// across the pawn's forward path. Static floor beneath for the approach.
    pub(crate) fn with_moving_ledge(link: LinkConfig, riser_z: f32, top_y: f32) -> Self {
        let mut h = Self::new(link);
        let origin = Vec3::new(0.0, 0.0, riser_z);
        let waypoints = vec![origin, origin + Vec3::new(6.0, 0.0, 0.0)];
        let host_mover = spawn_path_mover(&mut h.host_registry, waypoints.clone());
        let client_mover = spawn_path_mover(&mut h.client_registry, waypoints);
        h.mover_colliders
            .push(ledge_collider(MOVING_PLATFORM_ID, top_y));
        host_register_loaded_movers(
            &h.host_registry,
            &mut h.allocator,
            &mut h.replicable,
            &mut h.host_loaded_movers,
        );
        h.mover_network_id = Some(h.allocator.stamp(host_mover));
        h.host_mover = Some(host_mover);
        h.client_mover = Some(client_mover);
        h
    }

    /// An unstarted default (`displace`) `once` lift with `collider`, travelling
    /// `waypoints` at `speed_mps`, in `world` on both peers. The pawn spawns at
    /// [`START`], over the lift's origin.
    pub(crate) fn with_lift(
        link: LinkConfig,
        world: CollisionWorld,
        collider: MoverCollider,
        waypoints: Vec<Vec3>,
        speed_mps: f32,
    ) -> Self {
        let mut h = Self::new(link);
        h.world = world;
        let spawn = |registry: &mut EntityRegistry| {
            spawn_mover(
                registry,
                collider.mover_id(),
                waypoints.clone(),
                speed_mps,
                KinematicMoverMode::Once,
                false,
            )
        };
        let host_mover = spawn(&mut h.host_registry);
        let client_mover = spawn(&mut h.client_registry);
        h.mover_colliders.push(collider);
        host_register_loaded_movers(
            &h.host_registry,
            &mut h.allocator,
            &mut h.replicable,
            &mut h.host_loaded_movers,
        );
        h.mover_network_id = Some(h.allocator.stamp(host_mover));
        h.host_mover = Some(host_mover);
        h.client_mover = Some(client_mover);
        h
    }

    /// A pure rotator with a rider planted one metre from its pivot. The static
    /// axis/acceleration remain local on both peers; only the phase replicates.
    pub(crate) fn with_rotating_platform(link: LinkConfig) -> Self {
        let mut h = Self::new(link);
        h.world = CollisionWorld::new();
        h.host_registry
            .set_component(
                h.host_pawn,
                Transform {
                    position: ROTATING_PLATFORM_RIDER_START,
                    ..Transform::default()
                },
            )
            .expect("place rider on rotating platform");

        let host_mover = spawn_rotating_platform_mover(&mut h.host_registry);
        let client_mover = spawn_rotating_platform_mover(&mut h.client_registry);
        h.mover_colliders
            .push(platform_collider(MOVING_PLATFORM_ID));
        host_register_loaded_movers(
            &h.host_registry,
            &mut h.allocator,
            &mut h.replicable,
            &mut h.host_loaded_movers,
        );
        h.mover_network_id = Some(h.allocator.stamp(host_mover));
        h.host_mover = Some(host_mover);
        h.client_mover = Some(client_mover);
        h
    }

    /// CLIENT STEP: predict the local pawn one tick from `command` (the real
    /// `ClientPrediction::predict_tick` seam, writing back to the client registry),
    /// stamp the outbound `InputCommand`, encode the real `ClientMessage::Input`,
    /// and enqueue it on the to-server conditioner. Mirrors `client_predict_tick`'s
    /// send-then-predict order without needing a live `NetClient`.
    pub(crate) fn client_predict_and_send(&mut self, command: &SimCommand) {
        self.client_predict_and_send_with_delivery(command, true);
    }

    /// [`Self::client_predict_and_send`], optionally losing the outbound packet
    /// before the conditioner: the client predicts and logs the command exactly
    /// as it would, but the host never receives it.
    pub(crate) fn client_predict_and_send_with_delivery(
        &mut self,
        command: &SimCommand,
        deliver: bool,
    ) {
        self.client_registry.snapshot_transforms();
        run_kinematic_mover_tick(&mut self.client_registry, &mut self.client_mover_states, DT);

        // The production predicted tick: stamp + send, then (once armed and tuned)
        // predict with the registry's bound teleporter armed state and write back.
        let static_world = self.client_world.as_ref().unwrap_or(&self.world);
        let combined_collision = CombinedCollisionWorld::new(
            static_world,
            &self.mover_colliders,
            &self.client_mover_states,
        );
        let mut sent = Vec::new();
        let result = client_predict_tick_with_sender(
            &mut self.client_registry,
            &mut |bytes| sent.push(bytes),
            &mut self.prediction,
            ClientPredictionTickContext {
                command,
                collision: &combined_collision,
                environment_volumes: &self.client_environment_volumes,
                host_tuning: self.client_tuning.as_deref(),
                tick_dt: DT,
            },
        );
        self.latest_predicted_movement_events = result.movement_events;

        // Enqueue the real wire message through the conditioner.
        if deliver {
            for bytes in sent {
                self.to_server.enqueue(bytes);
            }
        }
        self.last_sent_client_tick = Some(result.client_tick);
        // Sending fresh input invalidates any frozen drain target: a later drain must
        // re-freeze against the NEW last-sent tick. (A test may interleave drain steps
        // mid-run to simulate an input gap, then resume sending.)
        self.drain_target_tick = None;
        self.ticks_since_cursor_caught = 0;
    }

    /// A zero-tick-frame send: one stamped command outside the predicted tick
    /// cadence, built by the same production helper as
    /// `client_send_input_command_in_registry`. Nothing is predicted for it.
    pub(crate) fn client_send_zero_tick(&mut self, command: &SimCommand) -> u32 {
        let input = stamped_client_input(&mut self.prediction, command, &self.client_registry);
        let client_tick = input.client_tick;
        self.to_server
            .enqueue(wire::encode(&ClientMessage::Input(input)));
        self.last_sent_client_tick = Some(client_tick);
        self.drain_target_tick = None;
        self.ticks_since_cursor_caught = 0;
        client_tick
    }

    /// HOST STEP: deliver any due client->server packets, route each through the
    /// real `host_handle_client_message` ingest seam (decode -> sanitize -> queue),
    /// then advance the authoritative host one fixed tick (resolve per-pawn movement
    /// via the gap policy, run the multi-pawn movement seam), and produce + enqueue a
    /// snapshot for the client through the to-client conditioner. One host frame
    /// running one tick; [`Self::host_frame`] runs several before one send.
    pub(crate) fn host_tick(&mut self) {
        self.host_frame(1, |_, _| {});
    }

    /// One host frame in production order: the poll (input ingest, and the
    /// participation-entry tuning send on first sight), `ticks` fixed ticks, then
    /// the send (changed tuning, then the snapshot). `between_ticks(h, i)` runs
    /// after tick `i`, as a reaction during that tick would: a `setGravity` there
    /// takes effect from the next tick, whose view is built at its start.
    pub(crate) fn host_frame(&mut self, ticks: u32, mut between_ticks: impl FnMut(&mut Self, u32)) {
        self.host_poll();
        for tick in 0..ticks {
            self.host_simulate_tick();
            between_ticks(self, tick);
        }
        self.host_send();
    }

    fn host_poll(&mut self) {
        // 1. Deliver + ingest due input packets through the real per-message
        //    dispatcher. The Input arm sanitizes + queues; a malformed/stale/duplicate
        //    command never touches another client or the registry. `server` /
        //    `server_replication` are required by the signature but untouched by the
        //    Input arm.
        let server_now_us = self.virtual_ms * 1000;
        for packet in self.to_server.take_ready() {
            if let Ok(msg) = wire::decode::<ClientMessage>(&packet) {
                host_handle_client_message(
                    &mut self.server,
                    &mut self.server_replication,
                    &mut self.server_state,
                    &mut self.command_queues,
                    CLIENT_ID,
                    self.server_tick,
                    server_now_us,
                    msg,
                );
            }
        }

        // Participation entry: the materialization send, at the live values.
        if !self.host_last_sent_tuning.contains_key(&CLIENT_ID) {
            self.host_send_tuning_if_changed();
        }
    }

    fn host_simulate_tick(&mut self) {
        // 2. Authoritative movement tick: resolve one command per owned pawn through
        //    the deterministic gap policy, then advance the named pawns.
        //
        //    Playout (jitter) buffer: the host does not begin resolving a client's
        //    command stream until a short warmup has buffered enough depth to cover the
        //    link's one-way latency + jitter (45..105 ms ≈ up to 7 ticks). Resolving
        //    immediately would race the cursor ahead of the still-in-flight commands —
        //    every jittered arrival would land at/below the cursor and be dropped as
        //    stale, starving the host onto neutral while the client predicts full
        //    motion (an unbounded, teleport-sized divergence). The warmup is this
        //    harness's stand-in for the production time-sync playout delay; once past
        //    it, the buffer always holds the next expected tick, so the host resolves
        //    real commands in order and its pawn follows the client's actual path,
        //    lagging by the (prediction-compensated) playout delay.
        self.host_ticks_elapsed = self.host_ticks_elapsed.wrapping_add(1);
        self.host_registry.snapshot_transforms();
        // Production mover order (`simulate_tick_with_presentation_aim`): the pin
        // pretest, the driver, then movement and triggers, then the blocking
        // pass. The harness has no consumer for mover events or crush impacts.
        let mut mover_events = Vec::new();
        {
            let (mover_poses, blocking_state) = self.host_mover_states.split_for_blocking();
            run_mover_pin_pretest(
                &mut self.host_registry,
                &self.world,
                &self.mover_colliders,
                &mover_poses,
                blocking_state,
                DT,
                &mut mover_events,
            );
        }
        run_kinematic_mover_tick(&mut self.host_registry, &mut self.host_mover_states, DT);
        let mut trigger_use_edges = HashMap::new();
        if self.host_ticks_elapsed > PLAYOUT_WARMUP_TICKS {
            let resolved_commands = host_resolve_remote_commands_in_registry(
                &self.owners,
                &mut self.command_queues,
                &self.host_registry,
            );
            let pawn_inputs = resolved_commands
                .iter()
                .map(|resolved| (resolved.pawn, resolved.command.movement.clone()))
                .collect::<Vec<_>>();
            trigger_use_edges.extend(resolved_commands.iter().filter_map(|resolved| {
                resolved
                    .command
                    .use_pressed
                    .then_some((PlayerId::Remote(resolved.client_id), true))
            }));
            let combined_collision = CombinedCollisionWorld::new(
                &self.world,
                &self.mover_colliders,
                &self.host_mover_states,
            );
            crate::sim::run_host_movement_tick(
                &mut self.host_registry,
                &combined_collision,
                EnvironmentView::new(
                    self.host_level_gravity,
                    &self.host_environment_volumes,
                    self.host_fluid_table.viscosities(),
                ),
                &pawn_inputs,
                DT,
            );
            self.host_trace.push(HostTickTrace::capture(
                &self.host_registry,
                self.host_pawn,
                self.server_tick,
                resolved_commands.first().cloned(),
            ));
        }
        self.host_trigger_system.run_authoritative_tick(
            &mut self.host_registry,
            &self.host_trigger_bridge,
            &[AuthoritativePlayer {
                id: PlayerId::Remote(CLIENT_ID),
                pawn: self.host_pawn,
            }],
            &trigger_use_edges,
            DT,
        );
        let (mover_poses, blocking_state) = self.host_mover_states.split_for_blocking();
        run_mover_blocking_pass(
            &mut self.host_registry,
            &self.world,
            &self.mover_colliders,
            &mover_poses,
            blocking_state,
            DT,
            &mut mover_events,
            &mut |_| {},
        );
        self.server_tick = self.server_tick.wrapping_add(1);
    }

    fn host_send(&mut self) {
        // Changed tuning is queued immediately before the snapshot, as
        // `net_serialize_and_send` does, so it reaches the client no later than
        // the first snapshot simulated after the change.
        self.host_send_tuning_if_changed();

        // During the playout warmup the host has resolved no command yet (cursor is
        // None), so a snapshot now would carry `last_processed_client_tick = None`.
        // Once the client has started predicting, a None-ack local record is an
        // authoritative RESET (reconcile.rs) — it would snap the client back to the
        // baseline pose and discard its predicted history every warmup tick, a large
        // spurious correction. So the host withholds snapshots until the warmup ends
        // and it can stamp a real resolved cursor; the client simply stays unarmed
        // (it still sends input) until its first real baseline arrives — the genuine
        // "client waits for its baseline before predicting" contract.
        if self.host_ticks_elapsed <= PLAYOUT_WARMUP_TICKS {
            return;
        }

        // 3. Produce + encode the authoritative snapshot through the REAL server
        //    replication tracker (full per-client encode with movement-authority
        //    metadata), then enqueue it on the to-client conditioner.
        let owned: Vec<EntitySnapshot> = produce_owned_snapshots(
            &self.host_registry,
            &self.replicable,
            &mut self.allocator,
            &self.owners,
            &self.command_queues,
        );
        self.server_replication.ingest_tick(owned);
        let sequence = self.server_replication.begin_batch();
        self.snapshot_sequence = sequence;
        if let Some(raw) =
            self.server_replication
                .encode_in_batch(CLIENT_ID, self.server_tick, sequence)
        {
            self.to_client.enqueue(wire::encode(&raw));
        }
    }

    /// CLIENT STEP: deliver any due server->client snapshot packets and apply each
    /// through the real `ClientReplication::apply_snapshot` + `reconcile_local_pawn`
    /// path (arm-before-reconcile, exactly as `client_receive_and_apply`). Returns
    /// the acks the client would send back to the host (so the harness can feed them
    /// to `ServerReplication::apply_ack`, advancing the per-client baseline so the
    /// next encode emits deltas — the genuine steady state).
    pub(crate) fn client_receive(&mut self) -> Vec<wire::AckMessage> {
        self.latest_mover_corrections.clear();
        self.latest_local_corrections.clear();
        self.latest_local_correction_classes.clear();
        self.latest_full_baselines.clear();
        self.latest_reconcile_windows.clear();
        // Control drains before snapshots, as `App::net_poll_and_apply` does.
        self.client_drain_control();
        let mut acks = Vec::new();
        for packet in self.to_client.take_ready() {
            let Ok(raw) = wire::decode::<RawSnapshotMessage>(&packet) else {
                continue;
            };
            let Ok(snapshot) = raw.validate() else {
                continue;
            };
            self.latest_full_baselines
                .extend(snapshot.records.iter().filter_map(|record| match record {
                    wire::EntityRecord::FullBaseline { network_id, .. } => Some(*network_id),
                    _ => None,
                }));
            let outcome = self
                .client_replication
                .apply_snapshot_with_mover_target_tick(
                    &mut self.client_registry,
                    &snapshot,
                    self.server_tick,
                    DT,
                );
            self.latest_mover_corrections
                .extend(outcome.mover_corrections.iter().copied());

            // Arm BEFORE reconcile (load-bearing ordering). This is the REAL production
            // client path (`client_receive_and_apply`): arm prediction, then materialize
            // the local pawn's descriptor-backed `PlayerMovementComponent` from the wire
            // `entity_class` (default `"player"`) via the production helper. The wire
            // never carries descriptor-immutable tuning, so the client builds the
            // component from the SAME shared descriptor table the host spawned its pawn
            // from — then the wire mutable subset has a matching base to merge onto.
            if let Some(armed) = &outcome.armed_local_pawn {
                self.prediction.arm(armed.network_id, armed.entity_id);
                self.client_pawn = Some(armed.entity_id);
                let entity_class = armed.entity_class.as_deref().unwrap_or("player");
                let entity_class =
                    super::descriptor_class::decode_replicated_descriptor_class(entity_class)
                        .canonical_name();
                materialize_net_local_movement_component(
                    entity_class,
                    &self.descriptors,
                    &mut self.client_registry,
                    armed.entity_id,
                );
            }
            if let Some(reconcile) = &outcome.local_reconcile
                && let Some(environment) = client_environment_view(
                    self.client_tuning.as_deref(),
                    &self.client_environment_volumes,
                )
            {
                let before = self
                    .client_registry
                    .get_component::<Transform>(reconcile.entity_id)
                    .ok()
                    .map(|transform| transform.position);
                let combined_collision = CombinedCollisionWorld::new(
                    self.client_world.as_ref().unwrap_or(&self.world),
                    &self.mover_colliders,
                    &self.client_mover_states,
                );
                let class = reconcile_local_pawn_with_mover_history(
                    &mut self.client_registry,
                    &mut self.prediction,
                    reconcile.entity_id,
                    reconcile.transform,
                    reconcile.movement.as_ref(),
                    reconcile.acked_tick,
                    reconcile.server_tick,
                    Some(self.client_replication.mover_history_mut()),
                    &combined_collision,
                    environment,
                    DT,
                );
                self.latest_reconcile_windows
                    .push((reconcile.acked_tick, self.last_sent_client_tick));
                self.latest_local_correction_classes.extend(class);
                if class.is_some()
                    && let (Some(before), Ok(after)) = (
                        before,
                        self.client_registry
                            .get_component::<Transform>(reconcile.entity_id),
                    )
                {
                    self.latest_local_corrections
                        .push((before - after.position).length());
                }
            }
            // Every refresh request apply raised rides the uplink to the host.
            for request in &outcome.refresh_requests {
                self.sent_refresh_requests.push(*request);
                self.to_server
                    .enqueue(wire::encode(&ClientMessage::BaselineRefresh(*request)));
            }
            if let Some(ack) = outcome.ack {
                self.client_acked_server_tick =
                    self.client_acked_server_tick.max(ack.acked_server_tick);
                acks.push(ack);
            }
        }
        // Production's once-per-frame unmerged-first-record refresh, then the
        // pending-repair resend cadence.
        let unmerged_wanted = self.prediction.first_record_pending()
            && client_environment_view(
                self.client_tuning.as_deref(),
                &self.client_environment_volumes,
            )
            .is_some()
            && self.prediction.armed().is_some_and(|pawn| {
                self.client_registry
                    .get_component::<PlayerMovementComponent>(pawn.entity_id)
                    .is_ok()
            });
        if let Some(request) = self
            .client_replication
            .refresh_unmerged_local_pawn(unmerged_wanted)
        {
            self.sent_refresh_requests.push(request);
            self.to_server
                .enqueue(wire::encode(&ClientMessage::BaselineRefresh(request)));
        }
        for request in self.client_replication.tick_pending_repairs(DT * 1000.0) {
            self.sent_refresh_requests.push(request);
            self.to_server
                .enqueue(wire::encode(&ClientMessage::BaselineRefresh(request)));
        }
        acks
    }

    /// HOST: build this slot's tuning payload from the host pawn and the host's live
    /// level gravity and fluid table, and enqueue it on the Control lane when it differs from the
    /// last one sent (`tuning_control_if_changed`, shared with production).
    pub(crate) fn host_send_tuning_if_changed(&mut self) {
        let payload = tuning_payload_for_pawn(
            &self.host_registry,
            self.host_pawn,
            &self.descriptors,
            None,
            &EnvironmentTuning {
                level_gravity: self.host_level_gravity,
                fluids: self.host_fluid_table.clone(),
            },
        );
        if let Some(message) =
            tuning_control_if_changed(&mut self.host_last_sent_tuning, CLIENT_ID, payload)
        {
            self.to_client_control.enqueue(message);
        }
    }

    /// CLIENT: install every due tuning payload through the production
    /// `replace_client_tuning` path.
    pub(crate) fn client_drain_control(&mut self) {
        for packet in self.to_client_control.take_ready() {
            let Ok(ServerControlMessage::Tuning(bytes)) = wire::decode(&packet) else {
                continue;
            };
            replace_client_tuning(
                &mut self.client_tuning,
                &mut self.client_tuning_generation,
                &bytes,
            )
            .expect("host tuning payload installs");
            self.client_tuning_installs += 1;
            self.client_sync_fluid_table();
        }
    }

    /// CLIENT: rebuild the armed pawn's movement component from the installed
    /// host tuning through the production rebuild
    /// (`apply_installed_movement_tuning_to_armed_pawn`), as a pawn-tuning
    /// change does on install — a weapon pickup, a host reload. Called between
    /// steps it lands after this frame's reconcile and before the next
    /// prediction, as a Control payload arriving in a frame with no snapshot
    /// does, so nothing restores the pawn before it predicts.
    pub(crate) fn client_rebuild_movement_from_installed_tuning(&mut self) {
        let armed = self
            .client_replication
            .armed_local_pawn()
            .expect("the client is armed");
        let mut applied_generation = 0;
        apply_installed_movement_tuning_to_armed_pawn(
            &armed,
            self.client_tuning.as_deref(),
            1,
            &mut applied_generation,
            &self.descriptors,
            &mut self.client_registry,
        );
    }

    /// Feed client acks back to the server replication tracker (the reverse control
    /// path). Advances each client's acked-baseline set so subsequent encodes emit
    /// deltas rather than full baselines — the production steady state.
    pub(crate) fn apply_acks(&mut self, acks: &[wire::AckMessage]) {
        for ack in acks {
            self.server_replication.apply_ack(
                CLIENT_ID,
                ack.latest_snapshot_sequence,
                &ack.entity_baselines,
                &ack.despawn_tombstones,
            );
        }
    }

    /// Advance both link conditioners by one tick step and the virtual master clock.
    pub(crate) fn advance_clock(&mut self) {
        self.to_server.advance(TICK_MS);
        self.to_client.advance(TICK_MS);
        self.to_client_control.advance(TICK_MS);
        self.virtual_ms += TICK_MS;
    }

    /// Step with `command` until the client has armed its local pawn (received and
    /// applied its first `local_player` baseline through the real apply path), or a
    /// bounded number of steps elapse. Returns the number of steps taken. Scenario
    /// tests use this instead of a hard-coded loop count so they are robust to the
    /// host playout warmup (the client cannot arm until the host emits its first
    /// post-warmup snapshot and it round-trips the link).
    pub(crate) fn step_until_armed(&mut self, command: &SimCommand) -> u32 {
        let mut steps = 0;
        while self.client_pawn.is_none() && steps < 200 {
            self.step(command);
            steps += 1;
        }
        steps
    }

    /// One full simulated step: client predicts + sends, clock advances, host
    /// ingests + ticks + snapshots, client receives + reconciles, acks flow back.
    pub(crate) fn step(&mut self, command: &SimCommand) {
        self.client_predict_and_send(command);
        self.advance_clock();
        self.host_tick();
        let acks = self.client_receive();
        self.apply_acks(&acks);
    }

    /// A catch-up step: the client predicts and sends two commands, then one host
    /// frame runs two fixed ticks before a single send. `between_ticks` is passed
    /// to [`Self::host_frame`].
    pub(crate) fn step_catch_up(
        &mut self,
        commands: [&SimCommand; 2],
        between_ticks: impl FnMut(&mut Self, u32),
    ) {
        for command in commands {
            self.client_predict_and_send(command);
            self.advance_clock();
        }
        self.host_frame(2, between_ticks);
        let acks = self.client_receive();
        self.apply_acks(&acks);
    }

    /// A drain step that sends no new input. It has two regimes, picked automatically
    /// so the drain is robust under packet loss:
    ///
    /// 1. **Active drain (host keeps ticking + snapshotting):** until the client has
    ///    acked the frozen drain *target* tick, the host keeps fully ticking — it
    ///    resolves any in-flight input (advancing the cursor to `last_sent`, then
    ///    holding/neutral as the gap policy dictates), advances the pawn, and emits a
    ///    snapshot every tick. The target tick is frozen the first time the cursor
    ///    reaches `last_sent`; the host's authoritative *pose* is then final (no new
    ///    input moves it), so every later snapshot carries the same pose at an
    ///    advancing `server_tick`. Continuing to emit means a snapshot lost to the 5%
    ///    link is simply superseded by the next one — the client is guaranteed to
    ///    eventually receive a snapshot at-or-past the target and ack it.
    /// 2. **Settle (host quiescent):** once the client has acked the target, the host
    ///    stops emitting so both conditioner queues can genuinely empty; the loop only
    ///    advances the clock, drains+ingests any straggler input (stale, dropped at
    ///    intake), and lets the client receive the last in-flight snapshot. This is
    ///    what lets `is_drained` (which requires zero packets in flight) become true.
    pub(crate) fn drain_step(&mut self) {
        let acked_target = matches!(
            self.drain_target_tick,
            Some(target) if self.client_acked_server_tick >= target
        );

        if !acked_target {
            // Regime 1: host keeps ticking + snapshotting until the client acks the
            // frozen target. host_tick resolves queued input and advances the cursor.
            self.advance_clock();
            self.host_tick();
            // Freeze the target a fixed SETTLE margin of ticks AFTER the cursor reaches
            // last_sent. The gap policy holds the last command for INPUT_HOLD_TICKS, then
            // goes neutral and the pawn decelerates asymptotically — freezing on
            // cursor-catchup alone would pin the target to a still-moving pose (a
            // spurious residual the client never quite reaches), while waiting for exact
            // position stillness is unboundedly slow (the geometric decay tail). A fixed
            // settle margin (> hold window + enough ticks for deceleration to fall below
            // the gate tolerance) is deterministic and bounded.
            if self.drain_target_tick.is_none() {
                let cursor_caught = matches!(
                    (
                        self.command_queues.resolved_cursor(CLIENT_ID),
                        self.last_sent_client_tick,
                    ),
                    (Some(cursor), Some(last)) if cursor >= last
                );
                if cursor_caught {
                    self.ticks_since_cursor_caught += 1;
                    if self.ticks_since_cursor_caught >= DRAIN_SETTLE_TICKS {
                        self.drain_target_tick = Some(self.server_tick);
                    }
                }
            }
        } else {
            // Regime 2: host quiescent — emit no new snapshot, but still drain + ingest
            // straggler input through the real seam so the to-server queue empties (a
            // stale command at/below the cursor is dropped at intake).
            self.advance_clock();
            let server_now_us = self.virtual_ms * 1000;
            for packet in self.to_server.take_ready() {
                if let Ok(msg) = wire::decode::<ClientMessage>(&packet) {
                    host_handle_client_message(
                        &mut self.server,
                        &mut self.server_replication,
                        &mut self.server_state,
                        &mut self.command_queues,
                        CLIENT_ID,
                        self.server_tick,
                        server_now_us,
                        msg,
                    );
                }
            }
        }

        let acks = self.client_receive();
        self.apply_acks(&acks);
    }

    /// The explicit drain condition: no packets in flight in EITHER
    /// direction, AND the host input queue is drained (its resolved cursor has reached
    /// the final sent command tick), AND the client has processed a snapshot acking
    /// that cursor (its acked server tick reached the frozen drain *target* — the
    /// server tick of the snapshot that first reflected the caught-up cursor).
    pub(crate) fn is_drained(&self) -> bool {
        let no_in_flight = self.to_server.in_flight() == 0
            && self.to_client.in_flight() == 0
            && self.to_client_control.in_flight() == 0;

        let cursor_caught_up = match (
            self.command_queues.resolved_cursor(CLIENT_ID),
            self.last_sent_client_tick,
        ) {
            (Some(cursor), Some(last)) => cursor >= last,
            // Nothing sent yet, or nothing resolved: not a meaningful drained state
            // for a run that sent commands.
            _ => false,
        };

        let client_acked_target = match self.drain_target_tick {
            Some(target) => self.client_acked_server_tick >= target,
            None => false,
        };

        no_in_flight && cursor_caught_up && client_acked_target
    }

    /// The current host-authoritative pawn position.
    pub(crate) fn host_position(&self) -> Vec3 {
        self.host_registry
            .get_component::<Transform>(self.host_pawn)
            .unwrap()
            .position
    }

    /// The current client registry (gameplay-authoritative, post-reconcile) pawn
    /// position. `None` until the local pawn has been armed.
    pub(crate) fn client_position(&self) -> Option<Vec3> {
        let pawn = self.client_pawn?;
        Some(
            self.client_registry
                .get_component::<Transform>(pawn)
                .unwrap()
                .position,
        )
    }

    /// Final horizontal+vertical position error between the client's reconciled pawn
    /// and the host authority. `f32::INFINITY` if the client never armed (a hard
    /// failure for a run that should converge).
    pub(crate) fn position_error(&self) -> f32 {
        match self.client_position() {
            Some(client) => (client - self.host_position()).length(),
            None => f32::INFINITY,
        }
    }

    /// Whether both zero-HP bystanders are still alive — the seam guard that the
    /// movement-only prediction/reconcile path never invoked the full
    /// `simulate_tick` death sweep on either registry.
    pub(crate) fn bystanders_alive(&self) -> bool {
        self.host_registry.exists(self.host_bystander)
            && self.client_registry.exists(self.client_bystander)
    }

    /// The local pawn's gameplay-authoritative (registry CURRENT) first-person eye:
    /// the pawn's current `Transform.position` plus the constant capsule eye height —
    /// exactly what `follow_camera_to_local_pawn` reads each fixed tick to drive
    /// `camera.position` (which is then pushed into `frame_timing`). NO presentation
    /// offset is folded here: the production camera-follow seam is passed `Vec3::ZERO`
    /// and the offset is added once at the render stage. `None` until the local pawn is
    /// armed and carries both components. The presented-eye smoothness test feeds this
    /// stream into a `frame_timing`-equivalent interpolator + the offset to reconstruct
    /// the exact first-person eye the player sees.
    pub(crate) fn local_pawn_eye(&self) -> Option<Vec3> {
        let pawn = self.client_pawn?;
        let position = self
            .client_registry
            .get_component::<Transform>(pawn)
            .ok()?
            .position;
        let eye_height = self
            .client_registry
            .get_component::<PlayerMovementComponent>(pawn)
            .ok()?
            .capsule
            .eye_height;
        Some(position + Vec3::new(0.0, eye_height, 0.0))
    }

    /// The local pawn's render-eye via the registry's *interpolated* transform at
    /// sub-tick `alpha` (the surface the pawn MESH and portal-visibility apex read in
    /// `main.rs`), plus the constant capsule eye height. This is the surface the
    /// structural fix repairs: `interpolated_transform` lerps `previous_transforms`
    /// against current, and a stale local previous (the bug) makes it lerp against an
    /// ever-staler frozen pose → velocity-proportional jitter. The existing harness
    /// never samples it (`position_error` reads only the CURRENT transform).
    pub(crate) fn local_pawn_interpolated_eye(&self, alpha: f32) -> Option<Vec3> {
        let pawn = self.client_pawn?;
        let interpolated = self
            .client_registry
            .interpolated_transform(pawn, alpha)
            .ok()?;
        let eye_height = self
            .client_registry
            .get_component::<PlayerMovementComponent>(pawn)
            .ok()?
            .capsule
            .eye_height;
        Some(interpolated.position + Vec3::new(0.0, eye_height, 0.0))
    }

    pub(crate) fn host_mover_position(&self) -> Option<Vec3> {
        let id = self.host_mover?;
        Some(
            self.host_registry
                .get_component::<Transform>(id)
                .ok()?
                .position,
        )
    }

    pub(crate) fn client_mover_position(&self) -> Option<Vec3> {
        let id = self.client_mover?;
        Some(
            self.client_registry
                .get_component::<Transform>(id)
                .ok()?
                .position,
        )
    }

    pub(crate) fn mover_position_error(&self) -> f32 {
        match (self.host_mover_position(), self.client_mover_position()) {
            (Some(host), Some(client)) => (host - client).length(),
            _ => f32::INFINITY,
        }
    }

    pub(crate) fn host_mover_velocity(&self) -> Option<Vec3> {
        let id = self.host_mover?;
        Some(
            self.host_registry
                .get_component::<KinematicMoverComponent>(id)
                .ok()?
                .current_linear_velocity,
        )
    }

    pub(crate) fn host_ground(&self) -> GroundRef {
        self.host_registry
            .get_component::<PlayerMovementComponent>(self.host_pawn)
            .map(|movement| movement.ground)
            .unwrap_or(GroundRef::Airborne)
    }

    pub(crate) fn client_ground(&self) -> GroundRef {
        let Some(pawn) = self.client_pawn else {
            return GroundRef::Airborne;
        };
        self.client_registry
            .get_component::<PlayerMovementComponent>(pawn)
            .map(|movement| movement.ground)
            .unwrap_or(GroundRef::Airborne)
    }

    pub(crate) fn host_velocity(&self) -> Vec3 {
        self.host_registry
            .get_component::<PlayerMovementComponent>(self.host_pawn)
            .map(|movement| movement.velocity)
            .unwrap_or(Vec3::ZERO)
    }

    pub(crate) fn client_velocity(&self) -> Vec3 {
        let Some(pawn) = self.client_pawn else {
            return Vec3::ZERO;
        };
        self.client_registry
            .get_component::<PlayerMovementComponent>(pawn)
            .map(|movement| movement.velocity)
            .unwrap_or(Vec3::ZERO)
    }

    pub(crate) fn client_mover_history_samples(&self) -> usize {
        self.client_replication
            .mover_history_sample_count(MOVING_PLATFORM_ID)
    }

    pub(crate) fn client_loaded_mover_count(&self) -> usize {
        self.client_registry
            .iter_with_kind(postretro_entities::ComponentKind::KinematicMover)
            .count()
    }
}

fn spawn_platform_mover(registry: &mut EntityRegistry, started: bool) -> EntityId {
    let id = registry.spawn(Transform::default());
    let mover = KinematicMoverComponent::new(
        MOVING_PLATFORM_ID,
        postretro_entities::KinematicMoverConfig {
            waypoints: vec![Vec3::ZERO, Vec3::new(6.0, 0.0, 0.0)],
            waypoint_names: vec!["start".to_string(), "finish".to_string()],
            speed_mps: MOVING_PLATFORM_SPEED_MPS,
            wait_ms: 0.0,
            mode: KinematicMoverMode::PingPong,
            started,
            spin_axis: Vec3::ZERO,
            initial_spin_rate_rad_s: 0.0,
            spin_accel_rad_s2: 0.0,
            carry_yaw: false,
        },
    );
    registry.set_component(id, mover).unwrap();
    id
}

fn spawn_path_mover(registry: &mut EntityRegistry, waypoints: Vec<Vec3>) -> EntityId {
    let id = registry.spawn(Transform {
        position: waypoints[0],
        ..Transform::default()
    });
    let mover = KinematicMoverComponent::new(
        MOVING_PLATFORM_ID,
        postretro_entities::KinematicMoverConfig {
            waypoint_names: (0..waypoints.len()).map(|i| format!("node{i}")).collect(),
            waypoints,
            speed_mps: MOVING_PLATFORM_SPEED_MPS,
            wait_ms: 0.0,
            mode: KinematicMoverMode::PingPong,
            started: true,
            spin_axis: Vec3::ZERO,
            initial_spin_rate_rad_s: 0.0,
            spin_accel_rad_s2: 0.0,
            carry_yaw: false,
        },
    );
    registry.set_component(id, mover).unwrap();
    id
}

fn spawn_mover(
    registry: &mut EntityRegistry,
    mover_id: u32,
    waypoints: Vec<Vec3>,
    speed_mps: f32,
    mode: KinematicMoverMode,
    started: bool,
) -> EntityId {
    let id = registry.spawn(Transform {
        position: waypoints[0],
        ..Transform::default()
    });
    let mover = KinematicMoverComponent::new(
        mover_id,
        postretro_entities::KinematicMoverConfig {
            waypoint_names: (0..waypoints.len()).map(|i| format!("node{i}")).collect(),
            waypoints,
            speed_mps,
            wait_ms: 0.0,
            mode,
            started,
            spin_axis: Vec3::ZERO,
            initial_spin_rate_rad_s: 0.0,
            spin_accel_rad_s2: 0.0,
            carry_yaw: false,
        },
    );
    registry.set_component(id, mover).unwrap();
    id
}

fn spawn_rotating_platform_mover(registry: &mut EntityRegistry) -> EntityId {
    let id = registry.spawn(Transform::default());
    let mover = KinematicMoverComponent::new(
        MOVING_PLATFORM_ID,
        postretro_entities::KinematicMoverConfig {
            waypoints: vec![Vec3::ZERO],
            waypoint_names: vec!["center".to_string()],
            speed_mps: 0.0,
            wait_ms: 0.0,
            mode: KinematicMoverMode::Once,
            started: true,
            spin_axis: Vec3::Y,
            initial_spin_rate_rad_s: ROTATING_PLATFORM_INITIAL_RATE_RAD_S,
            spin_accel_rad_s2: ROTATING_PLATFORM_ACCEL_RAD_S2,
            carry_yaw: false,
        },
    );
    registry.set_component(id, mover).unwrap();
    id
}

/// Host-only trigger fixture for the network harness. Both volumes contain the
/// remote pawn's spawn capsule: the touch trigger proves trigger evaluation is a
/// host tick responsibility, while the use trigger exercises the client-input
/// `use_pressed -> PlayerId::Remote` seam.
fn spawn_harness_trigger(
    registry: &mut EntityRegistry,
    bridge: &mut TriggerVolumeBridge,
    activation: TriggerActivation,
    command: MoverCommand,
) -> EntityId {
    let id = registry.spawn(Transform::default());
    registry
        .set_component(
            id,
            TriggerVolumeComponent::new(
                activation,
                "harness-platform".into(),
                String::new(),
                String::new(),
                command,
                TriggerFireMode::Once,
                0.0,
                true,
            ),
        )
        .expect("attach harness trigger");
    bridge.insert_for_test(id, Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 2.5, 1.0));
    id
}

/// A mover-local ledge: a riser facing +Z at local z = 0 up to `top_y`, and its
/// top running to local z = -6, 40 m wide.
fn ledge_collider(mover_id: u32, top_y: f32) -> MoverCollider {
    MoverCollider::from_local_triangles(
        mover_id,
        &[
            Vec3::new(-20.0, 0.0, 0.0),
            Vec3::new(20.0, 0.0, 0.0),
            Vec3::new(20.0, top_y, 0.0),
            Vec3::new(-20.0, top_y, 0.0),
            Vec3::new(-20.0, top_y, -6.0),
            Vec3::new(20.0, top_y, -6.0),
            Vec3::new(20.0, top_y, 0.0),
            Vec3::new(-20.0, top_y, 0.0),
        ],
        &[[0, 1, 2], [0, 2, 3], [4, 6, 5], [4, 7, 6]],
    )
    .unwrap()
}

fn platform_collider(mover_id: u32) -> MoverCollider {
    MoverCollider::from_local_triangles(
        mover_id,
        &[
            Vec3::new(-1.75, 0.0, -1.75),
            Vec3::new(1.75, 0.0, -1.75),
            Vec3::new(1.75, 0.0, 1.75),
            Vec3::new(-1.75, 0.0, 1.75),
        ],
        &[[0, 1, 2], [0, 2, 3]],
    )
    .unwrap()
}
