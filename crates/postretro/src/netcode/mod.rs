// Engine-side netcode glue for M15 Phase 3: role selection, the `NetworkId <-> EntityId`
// maps, the connection-slot lifecycle, the game-logic-owned host delta serialize and
// client apply/interpolation steps, and client-side prediction and reconciliation
// (the sole engine code that mutates the registry for replication).
// See: context/lib/networking.md

mod client;
mod command_queue;
mod interpolation;
mod lifecycle;
mod movement_state;
mod prediction;
mod reconcile;
mod replication;
mod wire_convert;

// M15 Phase 3 Task 6: the integrated in-memory prediction/reconciliation harness and
// its shared test scaffolding. Both are test-only — they drive the real Task 1-5 seams
// end to end over the dev `PacketConditioner` but introduce no production runtime state.
#[cfg(test)]
mod predict_reconcile_harness;
#[cfg(test)]
mod predict_reconcile_harness_test_fixtures;
// M15 Phase 3 regression: the connected-client boot-spawn gate (the boot → baseline
// arm sequence the harness above otherwise skips). Pins "client owns no local pawn
// until baseline".
#[cfg(test)]
mod boot_spawn_gate_test;

pub(crate) use client::ClientReplication;
pub(crate) use command_queue::{HostCommandQueues, MovementOwners, host_resolve_movement_inputs};
// `ResolvedCommand` / `ResolutionSource` are produced by the command queue and consumed
// via the submodule path only; not re-exported here.
pub(crate) use interpolation::{DemoMover, InterpolationDelayState};
pub(crate) use lifecycle::{SlotPawnSource, SlotPawns, on_slot_accepted, on_slot_closed};
pub(crate) use prediction::ClientPrediction;
// Correction-classification API + thresholds and the reconcile entry point.
// Re-exported for test consumers (the integrated latency harness asserts classification
// directly against the pinned AC thresholds); production code uses the direct submodule path.
#[allow(unused_imports)]
pub(crate) use prediction::{
    CorrectionClass, DASH_CORRECTION_MAX_M, ORDINARY_CORRECTION_MAX_M, TELEPORT_CORRECTION_MIN_M,
    classify_correction,
};
#[allow(unused_imports)]
pub(crate) use reconcile::reconcile_local_pawn;
pub(crate) use replication::{ReplicableSet, produce_owned_snapshots};
pub(crate) use wire_convert::sim_command_to_input;

// The conversion/merge helpers (`wire_convert`, `movement_state`) live in their focused
// submodules and are imported by callers via the direct submodule path.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use glam::{Quat, Vec3};

use postretro_net::replication::ServerReplication;
use postretro_net::timesync::{
    self, ClockEstimator, MonotonicClock, TimeSyncRequest, TimeSyncSender,
};
use postretro_net::transport::{NetClient, NetServer};
use postretro_net::wire::{
    self, ComponentPayload, NetworkId, RawSnapshotMessage, SnapshotMessage, ValidationError,
    WireError, WireMovementState, WirePlayerMovementState, WireTransform,
};

use crate::collision::CollisionWorld;
use crate::scripting::components::player_movement::PlayerMovementComponent;
use crate::scripting::registry::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform,
};
use crate::sim::SimCommand;

/// Default listen port for `--host` when no port is supplied.
pub(crate) const DEFAULT_HOST_PORT: u16 = 27015;

/// Max clients a listen server accepts. Phase 1 co-op bar is "ugly-but-connected"
/// loopback; a small ceiling keeps the netcode transport allocation modest.
const MAX_CLIENTS: usize = 8;

/// Network role selected at startup from CLI args.
///
/// Default is single-player (net inert — no endpoint is constructed). `--host
/// [port]` opens a listen server; `--connect <ip:port>` opens a client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NetRole {
    /// No networking. The endpoint is `None`; serialize/apply never run.
    SinglePlayer,
    /// Listen server bound to `port`.
    Host { port: u16 },
    /// Client connecting to `addr`.
    Connect { addr: SocketAddr },
}

/// Whether `role` must suppress the level-install boot player spawn
/// (`spawn_from_player_starts`) — M15 Phase 3, Task 3/6 contract. A CONNECTED
/// CLIENT owns ZERO `PlayerMovement` pawns until the host's `local_player`
/// baseline arms exactly one; spawning a boot pawn would create a second,
/// never-replicated, never-despawned pawn (camera glued to a frozen pawn pre-arm,
/// then an entity jump + spurious boot-pos → host-pos reconcile teleport at arm).
/// Single-player and the listen host KEEP their boot spawn (they need their own /
/// authoritative pawns). The install path keys this off the live endpoint
/// (`App::is_connected_client`); this is the equivalent role-level statement used
/// where only the parsed role is in hand (and by the regression test).
#[cfg(test)]
pub(crate) fn role_suppresses_boot_player_spawn(role: &NetRole) -> bool {
    matches!(role, NetRole::Connect { .. })
}

/// Parsed net configuration. Today this is just the role; kept as a struct so
/// future net CLI knobs (tick rate override, snapshot rate) extend it without
/// rippling the `main.rs` call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetConfig {
    pub(crate) role: NetRole,
}

/// Error parsing the net CLI flags. Carries an operator-facing message; `main.rs`
/// logs it and falls back to single-player rather than aborting boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NetArgError(pub(crate) String);

impl std::fmt::Display for NetArgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Parse the net role from the full `argv` slice (including `argv[0]`).
///
/// Recognized flags, scanned independently of the positional PRL-map path (which
/// the existing `resolve_map_path` handling owns — this parser never consumes it):
/// - `--host [port]` — listen server; bare `--host` uses [`DEFAULT_HOST_PORT`].
/// - `--connect <ip:port>` — client; `<ip:port>` is required.
///
/// Absent both flags, the role is [`NetRole::SinglePlayer`]. `--host` and
/// `--connect` are mutually exclusive — supplying both is an error.
pub(crate) fn parse_net_config(args: &[String]) -> Result<NetConfig, NetArgError> {
    let mut role: Option<NetRole> = None;

    let mut iter = args.iter().skip(1).peekable();
    while let Some(arg) = iter.next() {
        // `--host` with an optional inline (`--host=PORT`) or following port.
        let host_inline = arg.strip_prefix("--host=");
        if arg == "--host" || host_inline.is_some() {
            if role.is_some() {
                return Err(NetArgError(
                    "--host and --connect are mutually exclusive".into(),
                ));
            }
            let port = if let Some(value) = host_inline {
                parse_port(value)?
            } else if let Some(value) = iter.next_if(|v| !v.is_empty() && !v.starts_with("--")) {
                parse_port(value)?
            } else {
                DEFAULT_HOST_PORT
            };
            role = Some(NetRole::Host { port });
            continue;
        }

        // `--connect <ip:port>` with optional inline (`--connect=ip:port`).
        let connect_inline = arg.strip_prefix("--connect=");
        if arg == "--connect" || connect_inline.is_some() {
            if role.is_some() {
                return Err(NetArgError(
                    "--host and --connect are mutually exclusive".into(),
                ));
            }
            let value = if let Some(value) = connect_inline {
                value.to_string()
            } else {
                iter.next_if(|v| !v.is_empty() && !v.starts_with("--"))
                    .cloned()
                    .ok_or_else(|| NetArgError("--connect requires an <ip:port> address".into()))?
            };
            let addr: SocketAddr = value
                .parse()
                .map_err(|_| NetArgError(format!("invalid --connect address: {value}")))?;
            role = Some(NetRole::Connect { addr });
            continue;
        }
    }

    Ok(NetConfig {
        role: role.unwrap_or(NetRole::SinglePlayer),
    })
}

fn parse_port(value: &str) -> Result<u16, NetArgError> {
    value
        .parse::<u16>()
        .map_err(|_| NetArgError(format!("invalid --host port: {value}")))
}

/// The active network endpoint held by `App`. `None` for single-player; a
/// `Host`/`Client` variant once the role's transport is constructed.
///
/// Construction can fail (socket bind, transport init); failures are logged at
/// the call site and degrade to single-player (the field stays `None`) so a
/// netcode setup error never blocks boot.
pub(crate) enum NetEndpoint {
    /// Listen server plus the host-side `EntityId -> NetworkId` allocator. The
    /// `NetServer` is boxed: it is by far the largest endpoint payload (the renet
    /// connection layer + netcode transport), so an unboxed variant would inflate
    /// every `NetEndpoint` to its size (clippy::large_enum_variant). Boxing keeps
    /// the enum compact; the endpoint is a per-process singleton, so the extra
    /// indirection is paid once.
    Host {
        server: Box<NetServer>,
        allocator: NetworkIdAllocator,
        /// Monotonic server tick stamp written into each snapshot.
        tick: u32,
        /// Phase 2 per-client replication tracker (acked baselines, deltas,
        /// tombstones, refresh queue), keyed by `NetworkId`. Registry-blind: fed
        /// owned wire-mirror snapshots, never the registry.
        replication: Box<ServerReplication>,
        /// The Phase 2 replicable set: entities explicitly registered as
        /// authoritative networked gameplay objects (slot pawns, demo mover).
        replicable: ReplicableSet,
        /// Task 4 connection-lifecycle state: the slot -> remote-pawn `EntityId`
        /// map. An accepted client gets one slot-owned inert pawn here; a closed
        /// slot despawns it. Owned alongside `allocator`/`replicable` because the
        /// accept/close cleanup mutates all three together.
        slot_pawns: SlotPawns,
        /// M15 Phase 3 host authoritative command queues, keyed by client id. Inbound
        /// `ClientMessage::Input` is sanitized + queued here; the movement stage
        /// resolves one command per pawn per fixed tick via the deterministic gap
        /// policy.
        command_queues: HostCommandQueues,
        /// M15 Phase 3 movement-authority owner map: `EntityId -> owning client id`.
        /// Stamps `owner_client_id` + the resolved cursor onto each owned pawn's
        /// snapshot so the net crate can derive per-recipient `local_player`.
        owners: MovementOwners,
        /// The listen host's OWN player pawn, registered for OUTBOUND replication
        /// only (M15 Phase 3, issue 3b). The host pawn is driven LOCALLY by
        /// `simulate_tick`/`local_movement_pawn` exactly as in single-player — it is
        /// never command-queued, predicted, or reconciled. This field only tracks
        /// the registered pawn `EntityId` so a level reload can unregister the stale
        /// pawn before registering the freshly-spawned one (the registry bumps the
        /// generation on despawn, so the reloaded pawn is a distinct entity). `None`
        /// until the first level install registers the host pawn, and on maps with no
        /// player_spawn (a headless/observer host).
        ///
        /// Its snapshot carries `owner_client_id = None` (no remote owner), so the
        /// per-recipient `local_player` flag is false for EVERY client — clients treat
        /// it as a normal remote pawn (interpolated, drawn as a capsule). No second
        /// local-player marker exists; the host pawn stays the host's own
        /// `local_player_pawn` registry-side and is replicated outbound, that is all.
        host_pawn: Option<EntityId>,
        /// Task 6 Phase 2 net-demo fixture. When the demo path is active
        /// (`POSTRETRO_NET_DEMO_MOVER=1`), the host spawns one deterministic
        /// AI-less mover ([`DemoMover`]) and stores its `EntityId` here; each tick
        /// it is driven along its parametric loop and replicated like any other
        /// authoritative object. `None` when the demo path is off (production /
        /// ordinary host) or before the first tick spawns it. Not a gameplay
        /// archetype — it carries no script/FGD surface.
        demo_mover: DemoMoverState,
    },
    /// Client plus the Phase 2 client replication state (the `NetworkId -> EntityId`
    /// map, per-entity baseline table, pending-repair set, sequence tracking). The
    /// `NetClient` is boxed for the same reason the `Host` server is.
    Client {
        client: Box<NetClient>,
        replication: ClientReplication,
        /// Task 5 time-sync substrate: the 5 Hz probe sender, the clock/jitter
        /// estimator (consumed by Task 6 interpolation), and the production
        /// monotonic clock the estimator reads through.
        time_sync: Box<ClientTimeSync>,
        /// Remote-entity interpolation delay feedback. Time-sync jitter sets the
        /// baseline delay; recent buffer starvation temporarily raises it.
        interpolation_delay: InterpolationDelayState,
        /// M15 Phase 3 client-side movement prediction for the local pawn: the
        /// command + predicted-state ring, the armed `NetworkId -> EntityId`
        /// baseline, and the forward-prediction tick. Long-lived prediction state
        /// lives here (and in `prediction.rs`), never on `App` (source-layout gate).
        prediction: ClientPrediction,
    },
}

/// The production monotonic clock: the engine's `Instant` frame clock exposed as
/// a [`MonotonicClock`] so the estimator reads elapsed microseconds since this
/// origin, never wall-clock. A standalone field on [`ClientTimeSync`] so reading
/// it never aliases the `sender`/`estimator` borrows.
pub(crate) struct EngineClock {
    origin: std::time::Instant,
}

impl MonotonicClock for EngineClock {
    fn now_micros(&self) -> u64 {
        // Saturate at u64::MAX rather than panic on the (practically unreachable)
        // overflow of microseconds since process start.
        self.origin.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
    }
}

/// Client-side time-sync state: the 5 Hz probe sender, the clock/jitter
/// estimator (consumed by Task 6 interpolation), and the production monotonic
/// clock both read through.
pub(crate) struct ClientTimeSync {
    clock: EngineClock,
    sender: TimeSyncSender,
    estimator: ClockEstimator,
}

impl ClientTimeSync {
    fn new() -> Self {
        Self {
            clock: EngineClock {
                origin: std::time::Instant::now(),
            },
            sender: TimeSyncSender::new(),
            // The engine sim runs at 60 Hz; the estimator converts microseconds to
            // ticks at the same rate so its offset is in sim ticks.
            estimator: ClockEstimator::new(timesync::DEFAULT_MICROS_PER_TICK),
        }
    }

    /// Emit a 5 Hz probe if the cadence is due, recording the issued `sample_id`
    /// with the estimator in the same step. Sending and recording are fused here so
    /// a caller cannot queue a probe whose echo the estimator's provenance guard
    /// would then reject as never-issued — which would silently freeze the clock
    /// estimate. Returns the request to encode and send, or `None` when not due.
    fn maybe_send_probe(&mut self, client_tick: u32) -> Option<TimeSyncRequest> {
        let req = self.sender.maybe_send(&self.clock, client_tick)?;
        self.estimator.record_sent(req.sample_id);
        Some(req)
    }

    /// The smoothed server-tick estimate for the current local time, for the
    /// interpolation sampling path. `None` until the first echo has been folded in.
    pub(crate) fn estimated_server_tick(&self) -> Option<f64> {
        self.estimator
            .is_initialized()
            .then(|| self.estimator.estimated_server_tick(&self.clock))
    }

    /// The smoothed jitter estimate in microseconds, for interpolation delay
    /// sizing. `None` until the first echo has been folded in.
    pub(crate) fn jitter_micros(&self) -> Option<f64> {
        self.estimator
            .is_initialized()
            .then(|| self.estimator.jitter_micros())
    }
}

impl NetEndpoint {
    /// Construct the endpoint for `role`, or `Ok(None)` for single-player.
    ///
    /// The netcode clock origin is `SystemTime::now()` since the unix epoch
    /// (`NetServer::new`/`NetClient::new` contract). Returns the transport error
    /// for the caller to log and fall back to single-player.
    pub(crate) fn from_role(role: &NetRole) -> Result<Option<NetEndpoint>, String> {
        match role {
            NetRole::SinglePlayer => Ok(None),
            NetRole::Host { port } => {
                let bind_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, *port));
                let socket = UdpSocket::bind(bind_addr)
                    .map_err(|e| format!("host bind {bind_addr} failed: {e}"))?;
                let public_addr = socket
                    .local_addr()
                    .map_err(|e| format!("host local_addr failed: {e}"))?;
                let server = NetServer::new(socket, public_addr, MAX_CLIENTS, now())
                    .map_err(|e| format!("host transport init failed: {e}"))?;
                Ok(Some(NetEndpoint::Host {
                    server: Box::new(server),
                    allocator: NetworkIdAllocator::new(),
                    tick: 0,
                    replication: Box::new(ServerReplication::new()),
                    replicable: ReplicableSet::new(),
                    slot_pawns: SlotPawns::new(),
                    command_queues: HostCommandQueues::new(),
                    owners: MovementOwners::new(),
                    host_pawn: None,
                    demo_mover: DemoMoverState::from_env(),
                }))
            }
            NetRole::Connect { addr } => {
                // Bind an ephemeral local socket on the same address family.
                let bind_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0));
                let socket =
                    UdpSocket::bind(bind_addr).map_err(|e| format!("client bind failed: {e}"))?;
                // Client id is arbitrary under unsecure auth; use the wall clock
                // so two clients on one host do not collide.
                let client_id = now().as_nanos() as u64;
                let client = NetClient::new(socket, *addr, client_id, now())
                    .map_err(|e| format!("client transport init failed: {e}"))?;
                Ok(Some(NetEndpoint::Client {
                    client: Box::new(client),
                    replication: ClientReplication::new(),
                    time_sync: Box::new(ClientTimeSync::new()),
                    interpolation_delay: InterpolationDelayState::new(),
                    prediction: ClientPrediction::new(),
                }))
            }
        }
    }
}

fn now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the unix epoch")
}

/// Host-side monotonic `EntityId -> NetworkId` allocator. Ids are never recycled:
/// each newly-seen `EntityId` gets the next counter value, and the mapping is
/// stable for the entity's lifetime so the client's `NetworkId -> EntityId` map
/// stays coherent across snapshots.
pub(crate) struct NetworkIdAllocator {
    next: u32,
    map: HashMap<EntityId, NetworkId>,
}

impl NetworkIdAllocator {
    pub(crate) fn new() -> Self {
        Self {
            next: 0,
            map: HashMap::new(),
        }
    }

    /// Stamp `id` with its stable `NetworkId`, allocating a fresh one on first
    /// sight. Monotonic counter; never recycled.
    pub(crate) fn stamp(&mut self, id: EntityId) -> NetworkId {
        if let Some(net_id) = self.map.get(&id) {
            return *net_id;
        }
        let net_id = NetworkId(self.next);
        self.next += 1;
        self.map.insert(id, net_id);
        net_id
    }
}

/// Engine-aligned `u16` wire discriminant for a `ComponentKind`, via an
/// exhaustive match (no enum-layout reliance, no `_` arm). A renamed/removed
/// variant is a compile error here, which is the drift guard's whole point:
/// keep this numerically equal to `ComponentPayload::kind()` in `postretro-net`.
pub(crate) fn component_kind_discriminant(kind: ComponentKind) -> u16 {
    match kind {
        ComponentKind::Transform => 0,
        ComponentKind::Light => 1,
        ComponentKind::BillboardEmitter => 2,
        ComponentKind::ParticleState => 3,
        ComponentKind::SpriteVisual => 4,
        ComponentKind::FogVolume => 5,
        ComponentKind::PlayerMovement => 6,
        ComponentKind::Weapon => 7,
        ComponentKind::DescriptorProvenance => 8,
        ComponentKind::Mesh => 9,
        ComponentKind::Health => 10,
        ComponentKind::Agent => 11,
        ComponentKind::Brain => 12,
    }
}

/// Convert an engine `Transform` to its wire mirror. Phase 2 replicates scale
/// alongside position + rotation. glam `Quat` is `xyzw`, mirrored to the wire's
/// fixed `[x, y, z, w]` order.
pub(crate) fn transform_to_wire(transform: &Transform) -> WireTransform {
    let p = transform.position;
    let q = transform.rotation;
    let s = transform.scale;
    WireTransform {
        position: [p.x, p.y, p.z],
        rotation: [q.x, q.y, q.z, q.w],
        scale: [s.x, s.y, s.z],
    }
}

/// Inverse of [`transform_to_wire`]. Rotation is rebuilt from the `[x, y, z, w]`
/// wire order via `Quat::from_xyzw`; scale is now carried on the wire.
pub(crate) fn wire_to_transform(wire: &WireTransform) -> Transform {
    Transform {
        position: Vec3::new(wire.position[0], wire.position[1], wire.position[2]),
        rotation: Quat::from_xyzw(
            wire.rotation[0],
            wire.rotation[1],
            wire.rotation[2],
            wire.rotation[3],
        ),
        scale: Vec3::new(wire.scale[0], wire.scale[1], wire.scale[2]),
    }
}

/// Every position/rotation/scale component of a wire `Transform` is finite (no
/// NaN, no ±Inf). A snapshot arrives from an untrusted peer; a non-finite pose
/// round-trips byte-faithfully through the codec and would poison downstream
/// interpolation and camera/culling math if stored. The apply path drops any
/// entry that fails this check.
fn wire_transform_is_finite(t: &WireTransform) -> bool {
    t.position.iter().all(|c| c.is_finite())
        && t.rotation.iter().all(|c| c.is_finite())
        && t.scale.iter().all(|c| c.is_finite())
}

/// A wire `ComponentPayload` is safe to apply: all f32 fields are finite. The
/// exhaustive match (no `_` arm) means a new payload variant is a compile error
/// here until its finite-check is written.
fn payload_is_finite(payload: &ComponentPayload) -> bool {
    match payload {
        ComponentPayload::Transform(wire) => wire_transform_is_finite(wire),
        // The movement payload is received and validated here but not yet applied
        // to any local `PlayerMovementComponent` — the authoritative mover is
        // Transform-only. Validate its floats now so a non-finite payload is
        // dropped at the ingest boundary rather than propagated.
        ComponentPayload::PlayerMovementState(m) => player_movement_is_finite(m),
    }
}

/// Every f32 field of a wire movement payload is finite. Mirrors the untrusted-
/// wire guard `wire_transform_is_finite` applies to poses.
fn player_movement_is_finite(m: &WirePlayerMovementState) -> bool {
    let state_finite = match m.movement_state {
        WireMovementState::Normal => true,
        WireMovementState::Dash { elapsed_ms, boost } => {
            elapsed_ms.is_finite() && boost.iter().all(|c| c.is_finite())
        }
        WireMovementState::Crouching { eye_current } => eye_current.is_finite(),
    };
    m.velocity.iter().all(|c| c.is_finite())
        && m.dash_cooldown_ms.is_finite()
        && m.coyote_timer_ms.is_finite()
        && m.jump_buffer_timer_ms.is_finite()
        && m.capsule_half_height.is_finite()
        && m.capsule_eye_height.is_finite()
        && state_finite
}

/// Decode Phase 2 wire bytes into the typed [`SnapshotMessage`] apply model. Decodes
/// the raw envelope (corrupt bytes -> `Err`), then validates it into the typed model
/// (invalid kinds/version -> `Err`). The full record set — `FullBaseline`, `Delta`,
/// and `Despawn` — is preserved for the client apply state machine; nothing is
/// flattened or dropped here.
pub(crate) fn decode_snapshot(bytes: &[u8]) -> Result<SnapshotMessage, SnapshotDecodeError> {
    let raw: RawSnapshotMessage = wire::decode(bytes).map_err(SnapshotDecodeError::Decode)?;
    raw.validate().map_err(SnapshotDecodeError::Validate)
}

/// Drive the client's receive + apply + ack path for one frame (game-logic-owned).
/// Drains every snapshot received this frame, decodes + validates each (a corrupt or
/// invalid packet is logged and dropped, never a panic), applies it through the
/// [`ClientReplication`] state machine, and sends the resulting ack + any
/// baseline-refresh requests back on `Channel::Input`. Then advances the
/// pending-repair 5 Hz cadence by `frame_dt` and sends any due resends.
///
/// The mutable registry borrow is threaded in by the caller (`main.rs`), so this
/// module never reaches into `App`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn client_receive_and_apply(
    registry: &mut EntityRegistry,
    client: &mut NetClient,
    replication: &mut ClientReplication,
    prediction: &mut ClientPrediction,
    descriptors: &[crate::scripting::data_descriptors::EntityTypeDescriptor],
    collision: &CollisionWorld,
    gravity: f32,
    tick_dt: f32,
    frame_dt: Duration,
) {
    for bytes in client.drain_snapshots() {
        let snapshot = match decode_snapshot(&bytes) {
            Ok(snapshot) => snapshot,
            Err(err) => {
                log::warn!("[Net] dropping undecodable snapshot: {err}");
                continue;
            }
        };
        let outcome = replication.apply_snapshot(registry, &snapshot);
        // M15 Phase 3 Task 3 + Task 7: a `local_player` baseline arms client prediction
        // with the marked local pawn AND materializes its descriptor-backed
        // `PlayerMovementComponent` from the wire `entity_class` (default `"player"`).
        // `apply_snapshot` spawned the pawn Transform-only; the descriptor-immutable
        // movement tuning never crosses the wire, so the client materializes it locally
        // from the same descriptor table both peers share — then the wire mutable subset
        // has a component to merge onto and prediction/reconciliation light up. Materialize
        // BEFORE reconcile (which merges onto the existing component) and arm BEFORE
        // reconcile so the just-armed pawn reconciles on its arming snapshot too. The
        // materialization itself lives behind a focused helper next to the host's
        // net-slot spawn internals; this is the thin call. Re-arming the same pawn
        // preserves unacked history; the helper is idempotent (a re-arm keeps live state).
        if let Some(armed) = &outcome.armed_local_pawn {
            prediction.arm(armed.network_id, armed.entity_id);
            let entity_class = armed.entity_class.as_deref().unwrap_or("player");
            crate::scripting::builtins::data_archetype::materialize_net_local_movement_component(
                entity_class,
                descriptors,
                registry,
                armed.entity_id,
            );
        }
        // M15 Phase 3 Task 5: reconcile the local predicted pawn against the
        // authoritative record this snapshot delivered — merge the movement subset,
        // restore the transform, prune through the host ack, replay the unacked tail,
        // snap the reconciled gameplay state, and seed the decaying presentation
        // offset (or snap on a teleport). The registry-touching orchestration lives in
        // `reconcile`; long-lived prediction/smoothing state lives in `prediction`.
        if let Some(local) = &outcome.local_reconcile {
            reconcile::reconcile_local_pawn(
                registry,
                prediction,
                local.entity_id,
                local.transform,
                local.movement.as_ref(),
                local.acked_tick,
                collision,
                gravity,
                tick_dt,
            );
        }
        for buffer in client::encode_client_messages(&outcome) {
            client.send_input(buffer);
        }
    }

    // Resend pending baseline-refresh requests on the 5 Hz cadence. A request is one
    // BaselineRefresh ClientMessage on the reliable Input channel; the matching full
    // baseline clears the pending entry so the resend stops.
    let due = replication.tick_pending_repairs(frame_dt.as_secs_f32() * 1000.0);
    for req in due {
        let buffer = wire::encode(&wire::ClientMessage::BaselineRefresh(req));
        client.send_input(buffer);
    }
}

/// Drive one connected-client predicted fixed tick (M15 Phase 3 Task 3). Sends
/// exactly one `ClientMessage::Input` for `command` (stamped with the next
/// monotonic `client_tick`) on the reliable `Channel::Input`, then — once
/// prediction is armed — advances the local pawn through the movement-only replay
/// helper and writes the predicted `Transform` + `PlayerMovementComponent` back to
/// the registry. Returns `true` if it drove the local pawn this tick, `false` if it
/// only sent input (prediction not yet armed, or the armed pawn is missing).
///
/// This is the connected-client substitute for the local movement stage of
/// `sim::simulate_tick`: it advances ONLY the local pawn's movement (no AI, weapons,
/// death sweep, or reactions — those stay host-authoritative and arrive via
/// snapshots). The caller skips `simulate_tick` for local gameplay movement when
/// this returns. Before the `local_player` baseline arms prediction, the client
/// still sends input but drives no provisional pawn (`false`).
///
/// Game-logic-owned: the mutable registry borrow is threaded in by the caller so
/// this module never reaches into `App`.
pub(crate) fn client_predict_tick(
    registry: &mut EntityRegistry,
    client: &mut NetClient,
    prediction: &mut ClientPrediction,
    command: &SimCommand,
    collision: &CollisionWorld,
    gravity: f32,
    tick_dt: f32,
) -> bool {
    // 1. Send exactly one Input command for this predicted tick, stamped with the
    //    next monotonic client_tick. Sent even before the baseline arms prediction
    //    so the host's command stream starts immediately on connect.
    let client_tick = prediction.next_client_tick();
    let input = sim_command_to_input(command, client_tick);
    client.send_input(wire::encode(&wire::ClientMessage::Input(input)));

    // 2. Before the local baseline arms prediction, drive no provisional pawn.
    let Some(armed) = prediction.armed() else {
        return false;
    };

    // 3. Read the armed pawn's current applied state (seeded from the authoritative
    //    baseline / last reconcile). A missing pawn means the mapping went stale
    //    between arming and now; skip this tick rather than predict from nothing.
    let prev = match (
        registry.get_component::<Transform>(armed.entity_id),
        registry.get_component::<PlayerMovementComponent>(armed.entity_id),
    ) {
        (Ok(transform), Ok(movement)) => (*transform, movement.clone()),
        _ => return false,
    };

    // 4. Advance the local pawn one predicted tick through the movement-only helper
    //    and record it in the history ring.
    let Some((transform, movement)) =
        prediction.predict_tick(input, prev, collision, gravity, tick_dt)
    else {
        return false;
    };

    // 5. Stamp previous = current for the local pawn BEFORE writing the new predicted
    //    pose — the per-tick transform-history bookkeeping the render path needs. The
    //    connected client skips `simulate_tick` (it would rerun AI/weapons/death), so
    //    the registry-wide stage-0 `snapshot_transforms` never runs here; this is its
    //    single-pawn equivalent. Without it `previous_transforms[localpawn]` freezes at
    //    the last reconcile/spawn and the render-stage `interpolated_transform` (local
    //    pawn mesh + any prev/current-derived velocity) lerps live-current against an
    //    ever-staler previous, producing the velocity-proportional first-person jitter.
    registry.snapshot_transform(armed.entity_id);

    // 6. Write the predicted state back to the registry so camera follow, collision,
    //    and the next predicted tick read it. Task 5 reconciles this against the
    //    authoritative snapshot.
    let _ = registry.set_component(armed.entity_id, transform);
    let _ = registry.set_component(armed.entity_id, movement);
    true
}

/// The local-pawn presentation offset (M15 Phase 3 Task 5): the decaying correction
/// added to the local pawn's gameplay-authoritative registry transform to produce the
/// continuous first-person *presentation* pose. `Vec3::ZERO` for single-player, the
/// host, or a client whose prediction is unarmed / fully converged. THE single accessor
/// every local first-person render seam in `main.rs` reads (camera follow, view-feel
/// eye, `RenderCamera`, portal visibility apex) so they all consume one continuous pose
/// while gameplay reads the snapped registry transform.
pub(crate) fn client_local_presentation_offset(endpoint: Option<&NetEndpoint>) -> Vec3 {
    match endpoint {
        Some(NetEndpoint::Client { prediction, .. }) => prediction.presentation_offset(),
        _ => Vec3::ZERO,
    }
}

/// Decay the local-pawn presentation offset after the current presented fixed-tick
/// camera pose has been pushed. The render stage interpolates those presented poses
/// directly, so the offset is baked into the frame-timing endpoints exactly once. A
/// no-op for single-player, the host, or a client with no correction in flight.
pub(crate) fn client_decay_local_correction(endpoint: Option<&mut NetEndpoint>) {
    if let Some(NetEndpoint::Client { prediction, .. }) = endpoint {
        prediction.decay_presentation_offset();
    }
}

/// Sample every remote entity's interpolation buffer and write the resulting poses
/// through the registry's remote-presentation helper (Task 6). Game-logic-owned:
/// called once per frame, **after** `client_receive_and_apply` (which fills the
/// buffers) and **before** the render collectors read entities, so the renderer stays
/// read-only over the registry.
///
/// The render target is `estimated_server_tick - interpolation_delay`. Jitter sets
/// the baseline delay; recent held-newest starvation temporarily adds headroom.
/// Before the time-sync estimator has folded its first echo (`estimated_server_tick`
/// is `None`), there is no trustworthy clock to render against, so the buffers are
/// left unsampled and remote entities stay at their last-applied snapshot pose.
///
/// The mutable registry borrow is threaded in by the caller (`main.rs`), so this
/// module never reaches into `App`.
///
/// `frame_dt_secs` is the frame's wall-clock delta (the same per-frame delta the frame
/// loop computes); it drives the framerate-independent starvation feedback so the
/// adaptive delay's time-constant does not scale with frame rate.
pub(crate) fn client_sample_interpolation(
    registry: &mut EntityRegistry,
    replication: &mut ClientReplication,
    time_sync: &ClientTimeSync,
    interpolation_delay: &mut InterpolationDelayState,
    frame_dt_secs: f64,
) {
    // No estimate yet: render at the last-applied pose until the clock initializes.
    let Some(estimated_tick) = time_sync.estimated_server_tick() else {
        return;
    };
    // Jitter is available whenever the estimate is; default to 0 defensively.
    let jitter = time_sync.jitter_micros().unwrap_or(0.0);
    let render_server_tick =
        interpolation_delay.render_server_tick(estimated_tick, jitter, SERVER_TICK_MICROS);
    let stats = replication.sample_into_registry(registry, render_server_tick);
    if stats.presented > 0 {
        interpolation_delay.observe_sampled_frame(stats.starvation_feedback > 0, frame_dt_secs);
    }
}

/// Microseconds per server sim tick (60 Hz), used to derive the telemetry-only
/// `server_echo_time_us` carried in a time-sync echo. Equal to the estimator's
/// [`timesync::DEFAULT_MICROS_PER_TICK`]; kept here so `main.rs` builds the
/// telemetry stamp without importing the net const directly.
pub(crate) const SERVER_TICK_MICROS: u64 = timesync::DEFAULT_MICROS_PER_TICK;

/// Snapshot send cadence: one snapshot per client every second 60 Hz sim tick
/// (30 Hz). The host ingests the registry every sim tick (so dirty detection sees
/// every change) but only encodes + sends on this cadence.
///
/// M15 Phase 3 calibration (playtest bug "Symptom 2", 2026-06-22): raised from every
/// third tick (20 Hz) to every second tick (30 Hz). The faster cadence shrinks the
/// snapshot-spacing contribution to remote-view latency (~50 ms half-period → ~33 ms,
/// so ~16 ms mean) and keeps two snapshots bracketing the now-tighter 50 ms
/// interpolation floor (`MIN_DELAY_MICROS`) so remote motion stays smooth. The +50%
/// snapshot bandwidth is acceptable for co-op's small player count.
pub(crate) const SNAPSHOT_TICK_INTERVAL: u32 = 2;

/// Host-only Phase 2 net-demo fixture state. Activation is a startup decision read
/// once from the environment; the spawned `EntityId` is filled in lazily on the first
/// host tick that has a registry to spawn into.
///
/// Gated to the demo/harness path only — `enabled` is false on an ordinary host, so a
/// production listen server never spawns the demo mover. This is deliberately an env
/// gate rather than a CLI flag or FGD entity: the mover is a throwaway demo fixture,
/// not an authored gameplay object, so it must not grow a permanent CLI/script/FGD
/// surface (entity_model.md §4 — no authored archetype).
pub(crate) struct DemoMoverState {
    enabled: bool,
    entity: Option<EntityId>,
}

impl DemoMoverState {
    /// Read the demo-mover activation from the environment. `POSTRETRO_NET_DEMO_MOVER=1`
    /// turns it on; anything else (unset, empty, other value) leaves it off.
    fn from_env() -> Self {
        let enabled = std::env::var("POSTRETRO_NET_DEMO_MOVER")
            .map(|v| v == "1")
            .unwrap_or(false);
        Self {
            enabled,
            entity: None,
        }
    }
}

/// Drive the host-only demo mover (Task 6, demo path only). On the first call with the
/// demo path active, spawns one deterministic AI-less mover, registers it in the
/// replicable set, and stamps its `NetworkId`; every call thereafter writes its
/// deterministic pose for `server_tick`. A no-op when the demo path is off.
///
/// Game-logic-owned: the spawn and the pose write flow through `EntityRegistry::spawn`
/// / `set_component`. The mover is a `Transform`-only entity (no movement payload), so
/// on the client it replicates as the dumb mover whose interpolation-buffer starvation
/// path holds the last pose.
pub(crate) fn host_drive_demo_mover(
    registry: &mut EntityRegistry,
    demo_mover: &mut DemoMoverState,
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    server_tick: u32,
) {
    if !demo_mover.enabled {
        return;
    }
    let pose = DemoMover::pose_at(server_tick);
    match demo_mover.entity {
        Some(id) if registry.exists(id) => {
            // Steady state: write the deterministic pose for this tick.
            let _ = registry.set_component_value(id, ComponentValue::Transform(pose));
        }
        _ => {
            // First tick (or the entity vanished): spawn, register, stamp.
            let id = registry.spawn(pose);
            allocator.stamp(id);
            replicable.register(id);
            demo_mover.entity = Some(id);
            log::info!("[Net] demo mover spawned {id:?} (Phase 2 net-demo fixture)");
        }
    }
}

/// Drive one host sim tick of Phase 2 per-client delta replication. Game-logic
/// owned: borrows the registry immutably, copies the replicable set into owned
/// wire-mirror snapshots, releases the borrow, then feeds the net tracker and (on
/// the 30 Hz cadence) encodes + sends a per-client delta snapshot to every accepted
/// client.
///
/// `tick` is the monotonic server tick stamp; it is advanced by the caller. A
/// snapshot is encoded only when `tick % SNAPSHOT_TICK_INTERVAL == 0`, but the
/// tracker ingests every tick so an entity that changes and reverts within the
/// interval is still detected on the boundary it is sampled.
#[allow(clippy::too_many_arguments)]
pub(crate) fn host_replicate(
    registry: &EntityRegistry,
    server: &mut NetServer,
    allocator: &mut NetworkIdAllocator,
    replication: &mut ServerReplication,
    replicable: &ReplicableSet,
    owners: &MovementOwners,
    command_queues: &HostCommandQueues,
    tick: u32,
) {
    // Owned post-tick snapshot rule: copy replicable state into owned mirrors keyed
    // by NetworkId while borrowing the registry, then release before the net call.
    // Owned movement pawns also carry their owner id + resolved cursor (Phase 3).
    let owned = produce_owned_snapshots(registry, replicable, allocator, owners, command_queues);
    replication.ingest_tick(owned);

    // Snapshots emit at 30 Hz (every second 60 Hz tick); ingest ran every tick above.
    if tick % SNAPSHOT_TICK_INTERVAL != 0 {
        return;
    }

    let accepted = server.accepted_clients();
    if accepted.is_empty() {
        return;
    }
    // One sequence shared across all clients in this 30 Hz batch.
    let sequence = replication.begin_batch();
    for client_id in accepted {
        // Register lazily: an accepted client gets a fresh per-client state on first
        // sight (all-FullBaseline first snapshot). Idempotent.
        replication.register_client(client_id);
        if let Some(raw) = replication.encode_in_batch(client_id, tick, sequence) {
            let bytes = wire::encode(&raw);
            let _ = server.send_snapshot(client_id, bytes);
        }
    }
}

/// Spawn and register the slot-owned pawn for an accepted client (Task 4). This is
/// the production accept seam: the `NetServer` surfaces an accept only via
/// `ServerPoll.handshakes` (`SlotEvent::Accepted` is discarded inside the transport),
/// so the engine drives the spawn from the `HandshakeOutcome::Accepted` verdict —
/// `host_handle_lifecycle` never sees an accept. Threads the same allocator /
/// replicable set / slot map the close path uses, so accept and close mutate one
/// consistent state. Idempotent per slot (see [`on_slot_accepted`]).
///
/// This glue path has no player descriptor, so the pawn is the `Transform`-only inert
/// fixture (entity_model.md §7b — not a real movement pawn). Called BEFORE the frame's
/// `host_replicate` so the new pawn is in the first snapshot.
///
/// Game-logic-owned: the spawn flows through `EntityRegistry::spawn`; the caller
/// threads in the mutable registry borrow so this module never reaches into `App`.
pub(crate) fn host_handle_accept(
    registry: &mut EntityRegistry,
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    slot_pawns: &mut SlotPawns,
    client_id: u64,
) {
    let _ = on_slot_accepted(
        registry,
        slot_pawns,
        allocator,
        replicable,
        client_id,
        SlotPawnSource::TransformFixture,
    );
}

/// Production accept seam for a Phase 3 movement session: spawn the descriptor-backed
/// remote `PlayerMovement` pawn for an accepted client. Deterministically assigns the
/// slot a `player_spawn` placement (auditable, stable across reconnect), records the
/// owner mapping, then materializes the pawn through [`on_slot_accepted`]'s descriptor
/// path. Falls back to nothing (logged) if there are no spawn points or the descriptor
/// spawn fails — the caller keeps the slot for a later retry.
///
/// `spawn_points` are the level's `player_spawn` placements; `descriptors` the
/// registered entity descriptors; `agent_params` the navmesh capsule (or `None`).
/// Game-logic-owned: the spawn flows through `EntityRegistry::spawn`; the caller
/// threads in the mutable registry borrow.
#[allow(clippy::too_many_arguments)]
pub(crate) fn host_handle_accept_descriptor(
    registry: &mut EntityRegistry,
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    slot_pawns: &mut SlotPawns,
    command_queues: &mut HostCommandQueues,
    owners: &mut MovementOwners,
    client_id: u64,
    spawn_points: &[crate::scripting::map_entity::MapEntity],
    descriptors: &[crate::scripting::data_descriptors::EntityTypeDescriptor],
    agent_params: Option<crate::nav::NavAgentParams>,
) {
    // Deterministic, auditable slot -> placement assignment recorded BEFORE the spawn.
    let Some(idx) = slot_pawns.assign_placement(client_id, spawn_points.len()) else {
        log::warn!(
            "[Net] slot {client_id} accepted but the map has no player_spawn placements; no pawn spawned"
        );
        return;
    };
    let placement = &spawn_points[idx];

    let spawned = on_slot_accepted(
        registry,
        slot_pawns,
        allocator,
        replicable,
        client_id,
        SlotPawnSource::Descriptor {
            placement,
            descriptors,
            agent_params,
        },
    );

    if let Some((pawn, _net_id)) = spawned {
        // Record the owner mapping (pawn -> client_id) so snapshot production can stamp
        // `owner_client_id` and the resolved cursor. The client's command queue is
        // created lazily on its first ingested command.
        owners.set(pawn, client_id);
        let _ = command_queues;
    }
}

/// Register the listen host's OWN player pawn for OUTBOUND replication (M15 Phase 3,
/// issue 3b): without this, the host pawn never enters the `ReplicableSet`, so
/// `produce_owned_snapshots` never emits it and clients see no host capsule.
///
/// This is replication-only. The host pawn keeps being driven LOCALLY by
/// `simulate_tick`/`local_movement_pawn` — it is deliberately NOT recorded in
/// `MovementOwners`, NOT command-queued, and NOT predicted/reconciled. Because it has
/// no `owner_client_id`, its per-recipient `local_player` flag is false for every
/// client (clients interpolate it as a normal remote pawn).
///
/// Idempotent and reload-safe: registering the same pawn twice is a no-op (the set and
/// the allocator are both stable per `EntityId`). On a level reload the freshly-spawned
/// pawn is a distinct `EntityId`, so the previously-tracked host pawn (if any) is
/// unregistered first — never leaving a stale id in the replicable set. Pass the
/// `Host` variant's `host_pawn` tracking slot; the helper updates it.
///
/// Game-logic-owned: it reads the registry through the borrow the caller threads in and
/// only touches the replication bookkeeping; it never reaches into `App`.
pub(crate) fn host_register_own_pawn(
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    host_pawn: &mut Option<EntityId>,
    pawn: EntityId,
) {
    // A level reload spawns a fresh host pawn (distinct EntityId). Drop the stale
    // registration before registering the new one so the replicable set never names a
    // despawned id. Re-registering the SAME pawn skips the churn (idempotent install).
    if let Some(previous) = *host_pawn {
        if previous != pawn {
            replicable.unregister(previous);
        }
    }
    // Stamp the stable session-monotonic NetworkId and register for replication,
    // mirroring `on_slot_accepted` — but with NO owner mapping, so the host pawn is
    // replicated as an unowned (never-local) remote pawn to every client.
    let net_id = allocator.stamp(pawn);
    replicable.register(pawn);
    *host_pawn = Some(pawn);
    log::info!("[Net] host registered own pawn {pawn:?} as {net_id:?} (outbound replication only)");
}

/// Apply this frame's slot lifecycle transitions to the host's remote-pawn state
/// (Task 4). `ServerPoll.lifecycle` carries `SlotEvent::Closed` only — accepts are
/// driven from the handshake verdict via [`host_handle_accept`], never lifecycle.
/// Each close (clean disconnect or timeout — one cleanup path) despawns the slot's
/// pawn, drops it from the replicable set, and drops the slot mapping.
///
/// Game-logic-owned: the registry mutation flows through `EntityRegistry::despawn`.
/// The mutable registry borrow is threaded in by the caller so this module never
/// reaches into `App`.
pub(crate) fn host_handle_lifecycle(
    registry: &mut EntityRegistry,
    replicable: &mut ReplicableSet,
    replication: &mut ServerReplication,
    slot_pawns: &mut SlotPawns,
    command_queues: &mut HostCommandQueues,
    owners: &mut MovementOwners,
    lifecycle: &[postretro_net::slots::SlotEvent],
) {
    use postretro_net::slots::SlotEvent;
    for event in lifecycle {
        match event {
            SlotEvent::Closed { client_id, .. } => {
                let despawned =
                    on_slot_closed(registry, slot_pawns, replicable, replication, *client_id);
                // M15 Phase 3: drop the closed client's command queue and the pawn's
                // owner mapping so its stale authority metadata never rides a later
                // snapshot. The slot's placement assignment is intentionally retained
                // (a reconnecting client lands on its prior spawn — auditable source).
                command_queues.remove_client(*client_id);
                if let Some(pawn) = despawned {
                    owners.remove_pawn(pawn);
                }
            }
            // Accepts never reach lifecycle (the transport discards `SlotEvent::Accepted`
            // at `on_accept`); the spawn is driven from the handshake verdict instead, so
            // this arm is unreachable in production. Kept exhaustive (no `_`) so a new
            // SlotEvent variant is a compile error here.
            SlotEvent::Accepted { .. } => {}
        }
    }
}

/// Drain and apply one accepted client's reliable `Channel::Input` messages on the
/// host: replication acks advance that client's per-entity baseline / retire
/// tombstones, baseline-refresh requests queue a `FullBaseline` for the named
/// entity, and a time-sync probe is echoed back on `Channel::Input` with the
/// current `server_tick`. Corrupt or unknown-variant bytes are logged and dropped
/// — never a panic.
///
/// `server_tick` is the host's current monotonic sim tick (sampled at echo);
/// `server_now_us` is the host's monotonic microseconds, carried in the echo as
/// telemetry only. `InputCommand` messages are decoded but not yet applied
/// (Phase 3 gameplay).
pub(crate) fn host_handle_client_messages(
    server: &mut NetServer,
    replication: &mut ServerReplication,
    command_queues: &mut HostCommandQueues,
    client_id: u64,
    server_tick: u32,
    server_now_us: u64,
) {
    for bytes in server.drain_input(client_id) {
        let msg: wire::ClientMessage = match wire::decode(&bytes) {
            Ok(msg) => msg,
            Err(err) => {
                log::warn!("[Net] dropping undecodable client message from {client_id}: {err}");
                continue;
            }
        };
        host_handle_client_message(
            server,
            replication,
            command_queues,
            client_id,
            server_tick,
            server_now_us,
            msg,
        );
    }
}

/// Apply one decoded `ClientMessage` from `client_id` (M15 Phase 3). Split from the
/// drain loop so the duplicate/old-input hardening is testable by injecting a
/// `ClientMessage::Input` directly at this seam — without a reliable-ordered
/// transport producing duplicates. An invalid `Input` (non-finite) is dropped at
/// intake and mutates no queue or registry state.
pub(crate) fn host_handle_client_message(
    server: &mut NetServer,
    replication: &mut ServerReplication,
    command_queues: &mut HostCommandQueues,
    client_id: u64,
    server_tick: u32,
    server_now_us: u64,
    msg: wire::ClientMessage,
) {
    match msg {
        wire::ClientMessage::Ack(ack) => {
            replication.apply_ack(
                client_id,
                ack.latest_snapshot_sequence,
                &ack.entity_baselines,
                &ack.despawn_tombstones,
            );
        }
        wire::ClientMessage::BaselineRefresh(req) => {
            replication.request_refresh(client_id, req.network_id, req.missing_baseline_ref);
        }
        // Echo the time-sync probe with the server tick sampled now. The echo
        // rides Channel::Input back; the client measures RTT from its own
        // send/receive times and folds the server tick into its estimate.
        wire::ClientMessage::TimeSync(req) => {
            let echo = req.echo(server_tick, server_now_us);
            server.send_input(client_id, wire::encode(&echo));
        }
        // M15 Phase 3 Task 4: sanitize + queue the input command for this client.
        // `ingest` rejects non-finite commands, drops stale/duplicate ones, and never
        // mutates any other client's queue. The movement stage resolves them per tick.
        wire::ClientMessage::Input(input) => {
            command_queues.ingest(client_id, &input);
        }
    }
}

/// Drive one frame of the client time-sync exchange: emit a 5 Hz probe (stamped
/// with the client's local sim tick and monotonic microseconds) over
/// `Channel::Input`, then fold any echoes received this frame into the clock
/// estimator. `client_tick` is the client's local monotonic sim tick. Corrupt or
/// non-time-sync input bytes are dropped, never a panic.
///
/// The estimator and sender read time through the `ClientTimeSync` monotonic
/// clock (wrapping the engine `Instant`), so this path never touches wall-clock.
pub(crate) fn client_drive_time_sync(
    client: &mut NetClient,
    time_sync: &mut ClientTimeSync,
    client_tick: u32,
) {
    // 1. Emit a probe if the 5 Hz cadence is due. `maybe_send_probe` records the
    //    issued sample id with the estimator so the matching echo passes the
    //    provenance guard (forgetting that would freeze the clock estimate).
    if let Some(req) = time_sync.maybe_send_probe(client_tick) {
        let msg = wire::ClientMessage::TimeSync(req);
        client.send_input(wire::encode(&msg));
    }

    // 2. Fold any echoes that arrived this frame. The receive time is read from
    //    the same monotonic clock, so RTT is purely client-local.
    let echoes = client.drain_input();
    if echoes.is_empty() {
        return;
    }
    let recv_us = time_sync.clock.now_micros();
    for bytes in echoes {
        match wire::decode::<postretro_net::timesync::TimeSyncEcho>(&bytes) {
            Ok(echo) => {
                time_sync.estimator.ingest_echo(&echo, recv_us);
            }
            Err(err) => {
                log::warn!("[Net] dropping undecodable time-sync echo: {err}");
            }
        }
    }
}

/// Failure decoding a wire snapshot into a [`Snapshot`]: a corrupt buffer (bitcode
/// decode) or a structurally-decodable but invalid envelope (bad version/kind).
#[derive(Debug)]
pub(crate) enum SnapshotDecodeError {
    Decode(WireError),
    Validate(ValidationError),
}

impl std::fmt::Display for SnapshotDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotDecodeError::Decode(e) => write!(f, "{e}"),
            SnapshotDecodeError::Validate(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SnapshotDecodeError {}

/// Standing-player collision-capsule dimensions, used to size the debug
/// wireframe drawn over each replicated remote entity so it matches the real
/// player volume. Sourced from the canonical standing descriptor
/// (`scripting/components/player_movement.rs` and `main.rs`'s built-in pawn:
/// `CapsuleParams { radius: 0.4, half_height: 0.8, .. }`). Duplicated here as
/// named consts rather than threading a deep movement-descriptor dependency
/// into the client render path; if the canonical standing capsule changes,
/// update these to match.
///
/// `dev-tools`-gated: the only consumer is the client debug-capsule draw, which
/// lives behind the same feature (the debug-line renderer is `dev-tools` only).
#[cfg(feature = "dev-tools")]
pub(crate) const REMOTE_CAPSULE_RADIUS: f32 = 0.4;
#[cfg(feature = "dev-tools")]
pub(crate) const REMOTE_CAPSULE_HALF_HEIGHT: f32 = 0.8;

/// Collect the world-space positions of every replicated entity for the
/// debug wireframe (M15 Phase 1 visibility aid). On the CLIENT, returns the
/// `Transform.position` of each `EntityId` in its `NetworkId -> EntityId` map. On
/// the HOST (issue 3a), returns the position of each `EntityId` in the authoritative
/// `ReplicableSet` — the host has no client map, so it draws its own replicated
/// entities (slot/client pawns plus its own pawn). Empty for single-player.
///
/// Read-only: borrows the registry immutably and never touches wgpu — the
/// caller hands these positions to the renderer, which owns the capsule draw
/// (Renderer-owns-GPU). The returned position is the capsule center, matching
/// the pawn `Transform.position` convention (the collision capsule is symmetric
/// about it; see `movement/substrate.rs`).
///
/// Phase 1 wire-binds only `Transform`, so the client cannot distinguish a
/// player pawn from an inert prop — every remote entity gets a capsule. On the
/// sparse dev map this is effectively just the host pawn. Phase 2's
/// replicable-set policy (with the full component set and interest management)
/// will scope this to actual players; see `context/lib/networking.md`.
///
/// `dev-tools`-gated: the sole consumer is the client debug-capsule draw behind
/// that feature (the debug-line renderer is `dev-tools` only).
#[cfg(feature = "dev-tools")]
pub(crate) fn remote_entity_positions(
    endpoint: &NetEndpoint,
    registry: &EntityRegistry,
) -> Vec<Vec3> {
    match endpoint {
        // Client: draw only non-local mapped entities. The local predicted pawn is
        // also in the map, but it is camera/prediction driven and must not get a
        // duplicate "remote" capsule marker.
        NetEndpoint::Client { replication, .. } => replication
            .remote_debug_entity_ids()
            .filter_map(|id| {
                registry
                    .get_component::<Transform>(id)
                    .ok()
                    .map(|t| t.position)
            })
            .collect(),
        // Host: there is no client `NetworkId -> EntityId` map, so source the overlay
        // from the host's OWN authoritative replicated entities — the `ReplicableSet`
        // (the registered slot/client pawns, and the host's own pawn after issue 3b).
        // Read-only and dev-tools-only; no wire change. The set is keyed by `EntityId`,
        // which the registry resolves to a `Transform` directly.
        NetEndpoint::Host { replicable, .. } => replicable
            .iter()
            .filter_map(|id| {
                registry
                    .get_component::<Transform>(id)
                    .ok()
                    .map(|t| t.position)
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Float epsilon for transform round-trips (testing_guide §Floating-point:
    // approximate comparison for computed/converted floats).
    const EPSILON: f32 = 1e-6;

    fn sample_transform() -> Transform {
        Transform {
            position: Vec3::new(1.5, -2.0, 3.25),
            // A non-axis-aligned unit quaternion.
            rotation: Quat::from_xyzw(0.182_574_2, 0.365_148_4, 0.547_722_6, 0.730_296_8)
                .normalize(),
            scale: Vec3::splat(2.0),
        }
    }

    // --- Drift guard: Transform's wire discriminant is pinned to 0, equal to
    // `ComponentKind::Transform as u16`, through the exhaustive mapping. ---
    #[test]
    fn transform_discriminant_pinned_to_zero() {
        assert_eq!(
            component_kind_discriminant(ComponentKind::Transform),
            ComponentKind::Transform as u16
        );
        assert_eq!(component_kind_discriminant(ComponentKind::Transform), 0);
    }

    #[test]
    fn discriminant_mapping_matches_enum_layout() {
        // Drift guard (testing_guide §"Drift guards derive from the source"):
        // every `ComponentKind` variant must satisfy
        // `component_kind_discriminant(variant) == variant as u16`. The variant
        // sequence is produced by an exhaustive `match` with NO `_` arm
        // (`next_kind`, mirroring the production `component_kind_discriminant`
        // match), so a newly-added `ComponentKind` variant is a compile error
        // here — not a silently-passing stale hand-written list. The successor
        // walk then guarantees the assertion runs for every variant.
        fn next_kind(kind: ComponentKind) -> Option<ComponentKind> {
            match kind {
                ComponentKind::Transform => Some(ComponentKind::Light),
                ComponentKind::Light => Some(ComponentKind::BillboardEmitter),
                ComponentKind::BillboardEmitter => Some(ComponentKind::ParticleState),
                ComponentKind::ParticleState => Some(ComponentKind::SpriteVisual),
                ComponentKind::SpriteVisual => Some(ComponentKind::FogVolume),
                ComponentKind::FogVolume => Some(ComponentKind::PlayerMovement),
                ComponentKind::PlayerMovement => Some(ComponentKind::Weapon),
                ComponentKind::Weapon => Some(ComponentKind::DescriptorProvenance),
                ComponentKind::DescriptorProvenance => Some(ComponentKind::Mesh),
                ComponentKind::Mesh => Some(ComponentKind::Health),
                ComponentKind::Health => Some(ComponentKind::Agent),
                ComponentKind::Agent => Some(ComponentKind::Brain),
                ComponentKind::Brain => None,
            }
        }

        // Walk the full chain from the first variant, asserting each.
        let mut current = Some(ComponentKind::Transform);
        let mut visited = 0usize;
        while let Some(kind) = current {
            assert_eq!(
                component_kind_discriminant(kind),
                kind as u16,
                "discriminant must equal enum layout for {kind:?}"
            );
            visited += 1;
            current = next_kind(kind);
        }
        // The successor chain visited every variant exactly once.
        assert_eq!(
            visited,
            ComponentKind::COUNT,
            "the successor walk must cover every ComponentKind variant"
        );
    }

    // --- Round-trip: Transform -> WireTransform -> ComponentValue::Transform
    // preserves position and rotation in [x, y, z, w] order. ---
    #[test]
    fn transform_wire_round_trip_preserves_position_and_rotation() {
        let original = sample_transform();
        let wire = transform_to_wire(&original);

        // Wire stores position in xyz and rotation in [x, y, z, w] order.
        assert!((wire.position[0] - original.position.x).abs() < EPSILON);
        assert!((wire.position[1] - original.position.y).abs() < EPSILON);
        assert!((wire.position[2] - original.position.z).abs() < EPSILON);
        assert!((wire.rotation[0] - original.rotation.x).abs() < EPSILON);
        assert!((wire.rotation[1] - original.rotation.y).abs() < EPSILON);
        assert!((wire.rotation[2] - original.rotation.z).abs() < EPSILON);
        assert!((wire.rotation[3] - original.rotation.w).abs() < EPSILON);

        // Inverse conversion rebuilds the engine Transform from the wire mirror.
        let rebuilt = wire_to_transform(&wire);

        assert!((rebuilt.position - original.position).length() < EPSILON);
        // angle_between is 0 when rotations match.
        assert!(rebuilt.rotation.angle_between(original.rotation) < 1e-4);
        // Phase 2 replicates scale; it must round-trip through the wire mirror.
        assert!((rebuilt.scale - original.scale).length() < EPSILON);
    }

    // Regression: the production host accept seam never spawned the slot-owned pawn.
    // `main.rs`'s `HandshakeOutcome::Accepted` arm only called `register_client`, and
    // `host_handle_lifecycle` reads only `ServerPoll.lifecycle` (which never carries an
    // accept) — so no remote pawn was spawned, no `NetworkId` allocated, nothing entered
    // the replicable set, and nothing replicated in production. The unit lifecycle tests
    // passed only by calling `on_slot_accepted` directly, bypassing this seam. This test
    // drives the accept through `host_handle_accept` — the exact helper the production
    // `HandshakeOutcome::Accepted` arm invokes — and asserts the pawn exists, is
    // replicable, and carries an allocated NetworkId. A future regression that drops the
    // accept-spawn wiring fails here.
    #[test]
    fn host_handle_accept_spawns_registered_replicable_pawn_with_network_id() {
        let mut registry = EntityRegistry::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut slot_pawns = SlotPawns::new();
        const CLIENT_ID: u64 = 42;

        // Drive the accept through the production dispatch helper (NOT on_slot_accepted).
        host_handle_accept(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut slot_pawns,
            CLIENT_ID,
        );

        // A slot-owned pawn now exists for the client and is live in the registry.
        let pawn = slot_pawns
            .pawn_for(CLIENT_ID)
            .expect("accept spawned a slot-owned pawn for the client");
        assert!(
            registry.exists(pawn),
            "the slot pawn is live in the registry"
        );

        // It is registered for replication.
        assert!(
            replicable.contains(pawn),
            "the accepted pawn is in the replicable set"
        );

        // It has an allocated NetworkId and replicates: produce_owned_snapshots emits
        // exactly the one pawn, keyed by its allocated NetworkId.
        let expected_net_id = allocator.stamp(pawn);
        let owned = produce_owned_snapshots(
            &registry,
            &replicable,
            &mut allocator,
            &MovementOwners::new(),
            &HostCommandQueues::new(),
        );
        assert_eq!(owned.len(), 1, "exactly the accepted pawn replicates");
        assert_eq!(
            owned[0].network_id, expected_net_id.0,
            "the replicated pawn carries its allocated NetworkId"
        );
    }

    // Regression: `client_drive_time_sync` once emitted a probe without recording
    // its sample id, so the estimator's provenance guard rejected every echo and
    // the clock never initialized (a silent client-side freeze). `maybe_send_probe`
    // fuses send+record; this drives that production helper and proves the matching
    // echo initializes the estimator.
    #[test]
    fn time_sync_probe_records_issued_id_so_echo_initializes_estimator() {
        let mut time_sync = ClientTimeSync::new();

        // Emit a probe through the production path (the 5 Hz cadence fires on the
        // first call). This must record the issued sample id with the estimator.
        let req = time_sync
            .maybe_send_probe(0)
            .expect("the first probe fires immediately");

        // The server's echo for that exact sample id must pass the provenance guard
        // and fold in, leaving the estimator initialized.
        let echo = req.echo(600, 0);
        assert!(
            time_sync.estimator.ingest_echo(&echo, 0),
            "an echo for an issued sample id must be accepted"
        );
        assert!(
            time_sync.estimated_server_tick().is_some(),
            "the estimator initializes after a recorded probe's echo is folded in"
        );
    }

    // --- Issue 3b: the listen host's OWN pawn replicates outbound ---------------

    use crate::scripting::builtins::spawn_from_player_starts;
    use crate::scripting::data_descriptors::{
        AirParams, CapsuleParams, EntityTypeDescriptor, FallParams, GroundParams,
        PlayerMovementDescriptor, SpeedParams,
    };
    use crate::scripting::map_entity::MapEntity;
    use postretro_net::replication::{ServerReplication, typed_records};
    use postretro_net::wire::EntityRecord;

    /// A minimal `"player"` descriptor carrying a movement component, mirroring the
    /// lifecycle-test fixture so `spawn_from_player_starts` materializes a real
    /// `PlayerMovement` pawn and marks it the local player (the host's own pawn).
    fn host_player_descriptor() -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some("player".to_string()),
            default_weapon: None,
            light: None,
            emitter: None,
            movement: Some(PlayerMovementDescriptor {
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
                forgiveness: None,
                crouch: None,
                view_feel: None,
            }),
            weapon: None,
            mesh: None,
            health: None,
            ai: None,
        }
    }

    fn host_player_spawn_placement() -> MapEntity {
        MapEntity {
            classname: "player_spawn".to_string(),
            origin: glam::Vec3::new(5.0, 1.0, -2.0),
            angles: glam::Vec3::ZERO,
            key_values: std::collections::HashMap::new(),
            tags: vec![],
        }
    }

    /// Spawn the host's own boot pawn exactly as `install_level_payload` does, returning
    /// the marked `local_player_pawn` `EntityId`.
    fn spawn_host_boot_pawn(registry: &mut EntityRegistry) -> EntityId {
        let descriptors = [host_player_descriptor()];
        let placement = [host_player_spawn_placement()];
        spawn_from_player_starts(&placement, &descriptors, registry, None);
        registry
            .local_player_pawn()
            .expect("the host boot pawn is marked the local player")
    }

    // Issue 3b: after host setup the host's own pawn is registered in the ReplicableSet
    // with a NetworkId, and `produce_owned_snapshots` emits a VALID NON-local movement
    // record for it — owner None (so `local_player` false for any recipient), no ack
    // (`last_processed_client_tick` None), carrying Transform + PlayerMovementState +
    // entity_class. It is NEVER local for any recipient and is NOT double-registered on
    // a second install.
    #[test]
    fn host_own_pawn_replicates_as_non_local_movement_record() {
        let mut registry = EntityRegistry::new();
        let pawn = spawn_host_boot_pawn(&mut registry);

        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut host_pawn: Option<EntityId> = None;

        // Register the host's own pawn for outbound replication (the production seam:
        // `App::host_register_own_pawn_after_install`).
        host_register_own_pawn(&mut allocator, &mut replicable, &mut host_pawn, pawn);

        // It is in the replicable set, tracked, and carries an allocated NetworkId.
        assert!(
            replicable.contains(pawn),
            "the host's own pawn is registered for replication"
        );
        assert_eq!(host_pawn, Some(pawn), "the host pawn is tracked for reload");
        let net_id = allocator.stamp(pawn);

        // The host pawn has NO owner mapping (it is driven locally, never command-queued)
        // and NO resolved cursor — produce it with empty owners / queues.
        let owners = MovementOwners::new();
        let queues = HostCommandQueues::new();
        let owned =
            produce_owned_snapshots(&registry, &replicable, &mut allocator, &owners, &queues);
        assert_eq!(owned.len(), 1, "exactly the host's own pawn replicates");
        let snap = &owned[0];
        assert_eq!(snap.network_id, net_id.0, "stamped with its NetworkId");

        // Owner None -> local_player false for EVERY recipient; no ack cursor.
        assert_eq!(
            snap.owner_client_id, None,
            "the host pawn has no remote owner (never local_player on any recipient)"
        );
        assert_eq!(
            snap.last_processed_client_tick, None,
            "the host pawn is not command-driven, so it carries no ack cursor"
        );

        // Carries Transform + PlayerMovementState (the movement subset the producer
        // attaches for a live PlayerMovementComponent).
        assert_eq!(
            snap.components.len(),
            2,
            "host pawn replicates Transform + PlayerMovementState"
        );
        assert!(
            snap.components
                .iter()
                .any(|c| matches!(c, ComponentPayload::Transform(_))),
            "carries a Transform payload"
        );
        assert!(
            snap.components
                .iter()
                .any(|c| matches!(c, ComponentPayload::PlayerMovementState(_))),
            "carries a PlayerMovementState payload"
        );

        // The record is a VALID non-local movement record on the wire: ingest into the
        // tracker and encode for a sample recipient. `local_player` must be false (the
        // recipient is not the owner — there is no owner). Validation accepts a movement
        // record with None ack + local_player false + entity_class present-or-absent.
        const RECIPIENT: u64 = 7;
        let mut replication = ServerReplication::new();
        replication.register_client(RECIPIENT);
        let owned2 =
            produce_owned_snapshots(&registry, &replicable, &mut allocator, &owners, &queues);
        replication.ingest_tick(owned2);
        let encoded = replication
            .encode_for_client(RECIPIENT, 1)
            .expect("registered recipient encodes");
        // `typed_records` runs the wire validation; a malformed record would error here.
        let records = typed_records(&encoded);
        let host_record = records
            .iter()
            .find(|r| match r {
                EntityRecord::FullBaseline { network_id, .. }
                | EntityRecord::Delta { network_id, .. } => *network_id == net_id.0,
                _ => false,
            })
            .expect("the host pawn reaches the recipient as a valid record");
        let local_player = match host_record {
            EntityRecord::FullBaseline { local_player, .. }
            | EntityRecord::Delta { local_player, .. } => *local_player,
            _ => panic!("host pawn record is a movement baseline/delta"),
        };
        assert!(
            !local_player,
            "the host pawn is NEVER marked local_player for any recipient"
        );
    }

    // Issue 3b: a second install (level reload) re-registers the freshly-spawned host
    // pawn and unregisters the stale one — it never double-registers, and the replicable
    // set never names a despawned id.
    #[test]
    fn host_own_pawn_not_double_registered_on_reload() {
        let mut registry = EntityRegistry::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut host_pawn: Option<EntityId> = None;

        // First install.
        let first = spawn_host_boot_pawn(&mut registry);
        host_register_own_pawn(&mut allocator, &mut replicable, &mut host_pawn, first);
        assert!(replicable.contains(first));

        // Re-registering the SAME pawn (idempotent install) keeps exactly one entry.
        host_register_own_pawn(&mut allocator, &mut replicable, &mut host_pawn, first);
        let count_after_idempotent = replicable.iter().count();
        assert_eq!(
            count_after_idempotent, 1,
            "re-registering the same host pawn does not double-register"
        );

        // A level reload: despawn the old pawn and spawn a fresh one (distinct EntityId).
        registry.despawn(first).expect("old host pawn despawns");
        registry.clear_for_level_unload();
        let second = spawn_host_boot_pawn(&mut registry);
        assert_ne!(first, second, "the reloaded host pawn is a distinct entity");

        host_register_own_pawn(&mut allocator, &mut replicable, &mut host_pawn, second);
        assert_eq!(host_pawn, Some(second), "tracks the fresh host pawn");
        assert!(
            !replicable.contains(first),
            "the stale host pawn is unregistered on reload"
        );
        assert!(
            replicable.contains(second),
            "the fresh host pawn is registered"
        );
        assert_eq!(
            replicable.iter().count(),
            1,
            "exactly one host pawn is registered after reload"
        );
    }

    // Issue 3a: `remote_entity_positions` for a Host endpoint sources the overlay from
    // the host's authoritative ReplicableSet (non-empty when pawns are registered);
    // the Client endpoint still sources its own NetworkId -> EntityId map.
    #[cfg(feature = "dev-tools")]
    #[test]
    fn remote_entity_positions_host_sources_replicable_set() {
        // Build a real Host endpoint (binds an ephemeral UDP socket), then move its
        // replicable set out, populate it, and move it back — registering two
        // authoritative pawns at known positions.
        let mut registry = EntityRegistry::new();
        let a = registry.spawn(Transform {
            position: Vec3::new(1.0, 2.0, 3.0),
            ..Transform::default()
        });
        let b = registry.spawn(Transform {
            position: Vec3::new(-4.0, 0.0, 5.0),
            ..Transform::default()
        });

        let mut host = NetEndpoint::from_role(&NetRole::Host { port: 0 })
            .expect("host endpoint constructs")
            .expect("host role yields an endpoint");
        let NetEndpoint::Host { replicable, .. } = &mut host else {
            panic!("from_role(Host) must yield a Host endpoint");
        };
        replicable.register(a);
        replicable.register(b);

        let mut positions = netcode_remote_positions(&host, &registry);
        positions.sort_by(|p, q| p.x.partial_cmp(&q.x).unwrap());
        assert_eq!(
            positions,
            vec![Vec3::new(-4.0, 0.0, 5.0), Vec3::new(1.0, 2.0, 3.0)],
            "the host overlay sources the registered authoritative pawns' positions"
        );

        // A Client endpoint with an empty NetworkId -> EntityId map sources its own
        // (empty) map, NOT the host set — confirming the per-endpoint branch.
        let client = NetEndpoint::from_role(&NetRole::Connect {
            addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 1)),
        })
        .expect("client endpoint constructs")
        .expect("connect role yields an endpoint");
        assert!(
            netcode_remote_positions(&client, &registry).is_empty(),
            "a client with no mapped entities draws nothing (client-map path, not host)"
        );
    }

    // Regression: the client dev-tools overlay used to draw every mapped entity,
    // including the local predicted pawn. That duplicated the player capsule at the
    // prediction/reconcile seam and made the overlay look like a production jitter.
    #[cfg(feature = "dev-tools")]
    #[test]
    fn remote_entity_positions_client_excludes_local_predicted_pawn() {
        let mut registry = EntityRegistry::new();
        let mut client = NetEndpoint::from_role(&NetRole::Connect {
            addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 1)),
        })
        .expect("client endpoint constructs")
        .expect("connect role yields an endpoint");

        let NetEndpoint::Client { replication, .. } = &mut client else {
            panic!("from_role(Connect) must yield a Client endpoint");
        };
        replication.apply_snapshot(
            &mut registry,
            &SnapshotMessage {
                sequence: 0,
                server_tick: 10,
                records: vec![
                    postretro_net::wire::EntityRecord::FullBaseline {
                        network_id: 7,
                        baseline_id: 1,
                        last_processed_client_tick: None,
                        local_player: true,
                        entity_class: Some("player".to_string()),
                        components: vec![
                            ComponentPayload::Transform(WireTransform {
                                position: [3.0, 0.0, 0.0],
                                rotation: [0.0, 0.0, 0.0, 1.0],
                                scale: [1.0, 1.0, 1.0],
                            }),
                            ComponentPayload::PlayerMovementState(WirePlayerMovementState {
                                velocity: [0.0, 0.0, 0.0],
                                is_grounded: true,
                                air_jumps_remaining: 1,
                                air_dashes_remaining: 1,
                                dash_cooldown_ms: 0.0,
                                air_ticks: 0,
                                movement_state: WireMovementState::Normal,
                                coyote_timer_ms: 0.0,
                                jump_buffer_timer_ms: 0.0,
                                jump_spent: false,
                                capsule_half_height: 0.8,
                                capsule_eye_height: 1.5,
                            }),
                        ],
                    },
                    postretro_net::wire::EntityRecord::FullBaseline {
                        network_id: 8,
                        baseline_id: 1,
                        last_processed_client_tick: None,
                        local_player: false,
                        entity_class: None,
                        components: vec![ComponentPayload::Transform(WireTransform {
                            position: [9.0, 0.0, 0.0],
                            rotation: [0.0, 0.0, 0.0, 1.0],
                            scale: [1.0, 1.0, 1.0],
                        })],
                    },
                ],
            },
        );

        assert_eq!(
            netcode_remote_positions(&client, &registry),
            vec![Vec3::new(9.0, 0.0, 0.0)],
            "client remote overlay excludes the local predicted pawn"
        );
    }

    /// Local alias so the dev-tools-gated test reads the same symbol the caller uses.
    #[cfg(feature = "dev-tools")]
    fn netcode_remote_positions(endpoint: &NetEndpoint, registry: &EntityRegistry) -> Vec<Vec3> {
        remote_entity_positions(endpoint, registry)
    }

    // The client apply state machine (spawn, mutate-in-place, despawn, non-finite
    // drop, baseline repair, sequence tracking, ack production) is tested in the
    // `client` submodule, which owns that path. This module's tests cover the wire
    // conversions, the discriminant drift guard, and CLI parsing.

    // --- Argv parsing: default / --host / --connect, coexisting with the map path. ---
    fn argv(parts: &[&str]) -> Vec<String> {
        std::iter::once("postretro")
            .chain(parts.iter().copied())
            .map(String::from)
            .collect()
    }

    #[test]
    fn parse_default_is_single_player() {
        let config = parse_net_config(&argv(&[])).unwrap();
        assert_eq!(config.role, NetRole::SinglePlayer);
    }

    #[test]
    fn parse_host_without_port_uses_default() {
        let config = parse_net_config(&argv(&["--host"])).unwrap();
        assert_eq!(
            config.role,
            NetRole::Host {
                port: DEFAULT_HOST_PORT
            }
        );
    }

    #[test]
    fn parse_host_with_port() {
        let config = parse_net_config(&argv(&["--host", "30000"])).unwrap();
        assert_eq!(config.role, NetRole::Host { port: 30000 });
        let inline = parse_net_config(&argv(&["--host=40000"])).unwrap();
        assert_eq!(inline.role, NetRole::Host { port: 40000 });
    }

    #[test]
    fn parse_connect_with_addr() {
        let config = parse_net_config(&argv(&["--connect", "127.0.0.1:27015"])).unwrap();
        assert_eq!(
            config.role,
            NetRole::Connect {
                addr: "127.0.0.1:27015".parse().unwrap()
            }
        );
    }

    #[test]
    fn parse_connect_missing_addr_is_error() {
        assert!(parse_net_config(&argv(&["--connect"])).is_err());
        assert!(parse_net_config(&argv(&["--connect", "not-an-addr"])).is_err());
    }

    #[test]
    fn parse_host_and_connect_are_mutually_exclusive() {
        assert!(parse_net_config(&argv(&["--host", "--connect", "127.0.0.1:1"])).is_err());
    }

    #[test]
    fn net_flags_do_not_clobber_positional_map_path() {
        // The positional PRL-map path coexists with the net flags. `parse_net_config`
        // ignores the positional path entirely, and `resolve_map_path` (the existing
        // handler) must still recover it alongside `--host`/`--connect`.
        let args = argv(&["content/dev/maps/campaign-test.prl", "--host", "30000"]);
        let config = parse_net_config(&args).unwrap();
        assert_eq!(config.role, NetRole::Host { port: 30000 });
        assert_eq!(
            crate::resolve_map_path(&args).as_deref(),
            Some("content/dev/maps/campaign-test.prl"),
            "the positional map path survives the net flags"
        );

        // And with --connect: the positional map path leads (the conventional
        // `cargo run -p postretro -- <map>` ordering), then the net flag.
        let args = argv(&["maps/e1m1.prl", "--connect", "127.0.0.1:27015"]);
        let config = parse_net_config(&args).unwrap();
        assert_eq!(
            config.role,
            NetRole::Connect {
                addr: "127.0.0.1:27015".parse().unwrap()
            }
        );
        assert_eq!(
            crate::resolve_map_path(&args).as_deref(),
            Some("maps/e1m1.prl")
        );
    }
}
