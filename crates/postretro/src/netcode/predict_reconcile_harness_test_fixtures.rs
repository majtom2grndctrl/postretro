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
use parry3d::math::{Isometry, Point};
use parry3d::shape::TriMesh;

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use postretro_net::harness::{LinkConfig, PacketConditioner, VirtualMillis};
use postretro_net::replication::{EntitySnapshot, ServerReplication};
use postretro_net::transport::NetServer;
use postretro_net::wire::{
    self, ClientMessage, InputCommand, NetworkId, RawSnapshotMessage, WireFireButtonState,
    WireMovementInput,
};

use super::client::{ClientReplication, MoverCorrection};
use super::command_queue::{HostCommandQueues, MovementOwners, host_resolve_remote_commands};
use super::prediction::ClientPrediction;
use super::reconcile::reconcile_local_pawn_with_mover_history;
use super::replication::{ReplicableSet, host_register_loaded_movers, produce_owned_snapshots};
use super::wire_convert::sim_command_to_input;
use crate::collision::CollisionWorld;
use crate::collision::moving::{CombinedCollisionWorld, MoverCollider};
use crate::kinematic_mover::{MoverTickStateTable, run_kinematic_mover_tick};
use crate::movement::MovementInput;
use crate::netcode::{NetworkIdAllocator, host_handle_client_message};
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
    AirParams, BoolOrIr, CapsuleParams, DashParams, FallParams, ForgivenessParams, GroundParams,
    GroundRef, HealthDescriptor, NumberOrIr, PlayerMovementComponent, PlayerMovementDescriptor,
    SpeedParams,
};

pub(crate) const DT: f32 = 1.0 / 60.0;
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

/// A large flat floor so the pawn stays grounded for the whole run — keeps the
/// scenario about prediction/reconciliation, not terrain interaction.
pub(crate) fn floor_world() -> CollisionWorld {
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

/// The shared player descriptor (dash-capable) both ends materialize their pawn
/// from — descriptor tuning is identical on host and client (it never crosses the
/// wire), so only the mutable tick subset replicates.
pub(crate) fn player_descriptor() -> PlayerMovementDescriptor {
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
        crouch: None,
        slide: None,
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
/// `entity_class = Some("player")` on the owned snapshot — exactly as
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
        movement: MovementInput {
            wish_dir: Vec2::new(0.0, 1.0),
            jump_pressed: false,
            dash_pressed,
            running: true,
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
    }
}

pub(crate) fn idle_command() -> SimCommand {
    SimCommand {
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

/// The two-sided, deterministic loopback harness driving the genuine Task 1-5
/// seams through the in-memory packet conditioner. One conditioner per direction,
/// both seeded with the same mandated profile. A virtual ms clock the harness
/// advances drives delivery; no wall-clock time is ever read.
pub(crate) struct LoopbackHarness {
    pub(crate) world: CollisionWorld,

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
    /// Host ticks elapsed since start, gating the playout warmup (the host begins
    /// resolving client commands only after [`PLAYOUT_WARMUP_TICKS`]).
    pub(crate) host_ticks_elapsed: u32,

    // --- Client side (predicting) ---
    pub(crate) client_registry: EntityRegistry,
    pub(crate) client_replication: ClientReplication,
    pub(crate) prediction: ClientPrediction,
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

    // --- Bystander death-sweep guards (one per registry) ---
    pub(crate) host_bystander: EntityId,
    pub(crate) client_bystander: EntityId,

    // --- Link conditioners (one per direction) + virtual clock ---
    pub(crate) to_server: PacketConditioner,
    pub(crate) to_client: PacketConditioner,
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
        // `entity_class = Some("player")` on the owned snapshot (the production path
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

        Self {
            world,
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
            host_ticks_elapsed: 0,
            client_registry,
            client_replication: ClientReplication::new(),
            prediction: ClientPrediction::new(),
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
            host_bystander,
            client_bystander,
            to_server: PacketConditioner::new(link),
            to_client: PacketConditioner::new(link),
            virtual_ms: 0,
            last_sent_client_tick: None,
            client_acked_server_tick: 0,
            drain_target_tick: None,
            ticks_since_cursor_caught: 0,
        }
    }

    pub(crate) fn with_moving_platform(link: LinkConfig) -> Self {
        let mut h = Self::new(link);
        h.world = CollisionWorld::new();

        let host_mover = spawn_platform_mover(&mut h.host_registry);
        let client_mover = spawn_platform_mover(&mut h.client_registry);
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
        let client_tick = self.prediction.next_client_tick();
        let input = sim_command_to_input(command, client_tick, 0.0);

        self.client_registry.snapshot_transforms();
        run_kinematic_mover_tick(&mut self.client_registry, &mut self.client_mover_states, DT);

        // Drive prediction if armed (writing the predicted pose back to the registry
        // exactly as `client_predict_tick` does).
        if let Some(pawn) = self.client_pawn {
            let prev = (
                *self
                    .client_registry
                    .get_component::<Transform>(pawn)
                    .unwrap(),
                self.client_registry
                    .get_component::<PlayerMovementComponent>(pawn)
                    .unwrap()
                    .clone(),
            );
            let combined_collision = CombinedCollisionWorld::new(
                &self.world,
                &self.mover_colliders,
                &self.client_mover_states,
            );
            if let Some((t, m)) =
                self.prediction
                    .predict_tick(input, prev, &combined_collision, GRAVITY, DT)
            {
                // Mirror production `client_predict_tick`: stamp previous = current for
                // the local pawn BEFORE writing the new predicted pose, so its
                // transform-history stays coherent each predicted tick (the connected
                // client skips `simulate_tick`'s registry-wide stage-0 snapshot). The
                // presented-eye smoothness test depends on this faithful mirror.
                self.client_registry.snapshot_transform(pawn);
                self.client_registry.set_component(pawn, t).unwrap();
                self.client_registry.set_component(pawn, m).unwrap();
            }
        }

        // Encode + enqueue the real wire message through the conditioner.
        let bytes = wire::encode(&ClientMessage::Input(input));
        self.to_server.enqueue(bytes);
        self.last_sent_client_tick = Some(client_tick);
        // Sending fresh input invalidates any frozen drain target: a later drain must
        // re-freeze against the NEW last-sent tick. (A test may interleave drain steps
        // mid-run to simulate an input gap, then resume sending.)
        self.drain_target_tick = None;
        self.ticks_since_cursor_caught = 0;
    }

    /// HOST STEP: deliver any due client->server packets, route each through the
    /// real `host_handle_client_message` ingest seam (decode -> sanitize -> queue),
    /// then advance the authoritative host one fixed tick (resolve per-pawn movement
    /// via the gap policy, run the multi-pawn movement seam), and produce + enqueue a
    /// snapshot for the client through the to-client conditioner.
    pub(crate) fn host_tick(&mut self) {
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
        run_kinematic_mover_tick(&mut self.host_registry, &mut self.host_mover_states, DT);
        let mut trigger_use_edges = HashMap::new();
        if self.host_ticks_elapsed > PLAYOUT_WARMUP_TICKS {
            let resolved_commands =
                host_resolve_remote_commands(&self.owners, &mut self.command_queues);
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
                GRAVITY,
                &pawn_inputs,
                DT,
            );
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
        self.server_tick = self.server_tick.wrapping_add(1);

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
        let mut acks = Vec::new();
        for packet in self.to_client.take_ready() {
            let Ok(raw) = wire::decode::<RawSnapshotMessage>(&packet) else {
                continue;
            };
            let Ok(snapshot) = raw.validate() else {
                continue;
            };
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
            if let Some(reconcile) = outcome.local_reconcile {
                let before = self
                    .client_registry
                    .get_component::<Transform>(reconcile.entity_id)
                    .ok()
                    .map(|transform| transform.position);
                let combined_collision = CombinedCollisionWorld::new(
                    &self.world,
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
                    Some(self.client_replication.mover_history()),
                    &combined_collision,
                    GRAVITY,
                    DT,
                );
                if class.is_some() {
                    if let (Some(before), Ok(after)) = (
                        before,
                        self.client_registry
                            .get_component::<Transform>(reconcile.entity_id),
                    ) {
                        self.latest_local_corrections
                            .push((before - after.position).length());
                    }
                }
            }
            if let Some(ack) = outcome.ack {
                self.client_acked_server_tick =
                    self.client_acked_server_tick.max(ack.acked_server_tick);
                acks.push(ack);
            }
        }
        acks
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

    /// The explicit drain condition (Task 6 §B): no packets in flight in EITHER
    /// direction, AND the host input queue is drained (its resolved cursor has reached
    /// the final sent command tick), AND the client has processed a snapshot acking
    /// that cursor (its acked server tick reached the frozen drain *target* — the
    /// server tick of the snapshot that first reflected the caught-up cursor).
    pub(crate) fn is_drained(&self) -> bool {
        let no_in_flight = self.to_server.in_flight() == 0 && self.to_client.in_flight() == 0;

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

fn spawn_platform_mover(registry: &mut EntityRegistry) -> EntityId {
    let id = registry.spawn(Transform::default());
    let mover = KinematicMoverComponent::new(
        MOVING_PLATFORM_ID,
        postretro_entities::KinematicMoverConfig {
            waypoints: vec![Vec3::ZERO, Vec3::new(6.0, 0.0, 0.0)],
            waypoint_names: vec!["start".to_string(), "finish".to_string()],
            speed_mps: MOVING_PLATFORM_SPEED_MPS,
            wait_ms: 0.0,
            mode: KinematicMoverMode::PingPong,
            started: false,
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
