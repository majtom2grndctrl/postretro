// Engine-side netcode glue (M15 Phase 1). The ONLY engine code that touches the
// `EntityRegistry` on behalf of replication: `postretro-net` emits typed
// snapshots and never mutates the registry (entity_model.md §6; netcode
// contracts in context/lib/networking.md). This module
// owns role selection, the optional endpoint held by `App`, the
// `NetworkId <-> EntityId` maps, and the two game-logic-owned steps (host
// `serialize`, client `apply`) plus the `WireTransform <-> Transform` and
// `ComponentKind -> u16` conversions.
//
// Phase 1 is "ugly-but-connected": the client is a pure viewer of
// host-authoritative state. No prediction, no client->server gameplay input, no
// despawn reconciliation (a `NetworkId` missing from a later snapshot is left
// untouched — Phase 2 owns remove-missing).

mod replication;

pub(crate) use replication::{produce_owned_snapshots, ReplicableSet};

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use glam::{Quat, Vec3};

use postretro_net::replication::ServerReplication;
use postretro_net::timesync::{self, ClockEstimator, MonotonicClock, TimeSyncSender};
use postretro_net::transport::{NetClient, NetServer};
use postretro_net::wire::{
    self, ComponentPayload, EntityRecord, NetworkId, RawSnapshotMessage, Snapshot, ValidationError,
    WireError, WireMovementState, WirePlayerMovementState, WireTransform,
};

use crate::scripting::registry::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform,
};

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
    },
    /// Client plus the `NetworkId -> EntityId` map for applied snapshots. The
    /// `NetClient` is boxed for the same reason the `Host` server is.
    Client {
        client: Box<NetClient>,
        map: HashMap<NetworkId, EntityId>,
        /// Task 5 time-sync substrate: the 5 Hz probe sender, the clock/jitter
        /// estimator (consumed by Task 6 interpolation), and the production
        /// monotonic clock the estimator reads through.
        time_sync: Box<ClientTimeSync>,
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

    /// The smoothed server-tick estimate for the current local time, for Task 6
    /// interpolation consumers. `None` until the first echo has been folded in.
    #[allow(dead_code)] // consumed by Task 6 interpolation; surfaced now.
    pub(crate) fn estimated_server_tick(&self) -> Option<f64> {
        self.estimator
            .is_initialized()
            .then(|| self.estimator.estimated_server_tick(&self.clock))
    }

    /// The smoothed jitter estimate in microseconds, for Task 6 interpolation
    /// delay sizing. `None` until the first echo has been folded in.
    #[allow(dead_code)] // consumed by Task 6 interpolation; surfaced now.
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
                    map: HashMap::new(),
                    time_sync: Box::new(ClientTimeSync::new()),
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
        // The movement wire subset is not applied by Phase 1 glue (Task 3 merges
        // it into a local `PlayerMovementComponent`); still validate its floats so
        // a non-finite payload is dropped at the boundary rather than carried.
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

/// Convert a wire `ComponentPayload` to the engine `ComponentValue`, or `None`
/// when Phase 1 glue does not apply this payload. The exhaustive match (no `_`
/// arm) means a new payload variant is a compile error here until its handling is
/// decided.
///
/// `PlayerMovementState` returns `None` in Phase 1: the wire carries only the
/// mutable tick subset, and reconstructing a full `PlayerMovementComponent` from
/// it (merging onto a local descriptor-derived component) is Task 3's job, not a
/// Phase 1 spawn-from-wire concern.
fn payload_to_component_value(payload: &ComponentPayload) -> Option<ComponentValue> {
    match payload {
        ComponentPayload::Transform(wire) => {
            Some(ComponentValue::Transform(wire_to_transform(wire)))
        }
        ComponentPayload::PlayerMovementState(_) => None,
    }
}


/// Client apply step (game-logic-owned). For each `(NetworkId, ComponentPayload)`:
/// on first sight of the `NetworkId`, `spawn` with the converted `Transform` and
/// record the mapping; otherwise `set_component_value` on the mapped `EntityId`.
///
/// Phase 1 never despawns: a `NetworkId` absent from a later snapshot is left
/// untouched (remove-missing reconciliation is Phase 2). A stale mapped
/// `EntityId` (registry rejected the write) is re-spawned and the map updated.
pub(crate) fn apply(
    registry: &mut EntityRegistry,
    snapshot: &Snapshot,
    map: &mut HashMap<NetworkId, EntityId>,
) {
    for (net_id, payload) in &snapshot.entries {
        // Untrusted-wire guard: a non-finite pose (NaN/±Inf in position or
        // rotation) is dropped at the boundary before it can reach the registry
        // — never spawned, never set. This covers both first-sight spawn and
        // subsequent set paths because the skip precedes the branch below.
        if !payload_is_finite(payload) {
            log::warn!("[Net] dropping snapshot entry for {net_id:?}: non-finite transform");
            continue;
        }
        // Phase 1 glue applies only payloads with an engine `ComponentValue`
        // mapping; others (movement tick subset) are skipped until Task 3.
        let Some(value) = payload_to_component_value(payload) else {
            continue;
        };
        match map.get(net_id).copied() {
            Some(existing) => {
                if registry
                    .set_component_value(existing, value.clone())
                    .is_err()
                {
                    // The mapped entity is gone (should not happen in Phase 1,
                    // which never despawns) — re-spawn from this payload so the
                    // remote entity stays visible rather than silently vanishing.
                    let id = spawn_from_value(registry, value);
                    map.insert(*net_id, id);
                }
            }
            None => {
                let id = spawn_from_value(registry, value);
                map.insert(*net_id, id);
            }
        }
    }
}

/// Decode Phase 2 wire bytes into an in-process [`Snapshot`]. Decodes the raw
/// envelope (corrupt bytes -> `Err`), validates it into the typed apply model
/// (invalid kinds/version -> `Err`), then flattens the Phase 1-shaped records
/// (`FullBaseline` only in Phase 1) back into `(NetworkId, ComponentPayload)`
/// entries. `Delta`/`Despawn` records are ignored here — applying them is Task 3.
pub(crate) fn decode_snapshot(bytes: &[u8]) -> Result<Snapshot, SnapshotDecodeError> {
    let raw: RawSnapshotMessage = wire::decode(bytes).map_err(SnapshotDecodeError::Decode)?;
    let typed = raw.validate().map_err(SnapshotDecodeError::Validate)?;
    let mut entries = Vec::new();
    for record in typed.records {
        // Phase 1 apply only consumes full-baseline records; lifecycle deltas and
        // despawns are Task 3's reconciliation, not this Phase 1 bridge.
        if let EntityRecord::FullBaseline {
            network_id,
            components,
            ..
        } = record
        {
            for payload in components {
                entries.push((NetworkId(network_id), payload));
            }
        }
    }
    Ok(Snapshot {
        tick: typed.server_tick,
        entries,
    })
}

/// Microseconds per server sim tick (60 Hz), used to derive the telemetry-only
/// `server_echo_time_us` carried in a time-sync echo. Equal to the estimator's
/// [`timesync::DEFAULT_MICROS_PER_TICK`]; kept here so `main.rs` builds the
/// telemetry stamp without importing the net const directly.
pub(crate) const SERVER_TICK_MICROS: u64 = timesync::DEFAULT_MICROS_PER_TICK;

/// Snapshot send cadence: one snapshot per client every third 60 Hz sim tick
/// (20 Hz). The host ingests the registry every sim tick (so dirty detection sees
/// every change) but only encodes + sends on this cadence.
pub(crate) const SNAPSHOT_TICK_INTERVAL: u32 = 3;

/// Drive one host sim tick of Phase 2 per-client delta replication. Game-logic
/// owned: borrows the registry immutably, copies the replicable set into owned
/// wire-mirror snapshots, releases the borrow, then feeds the net tracker and (on
/// the 20 Hz cadence) encodes + sends a per-client delta snapshot to every accepted
/// client.
///
/// `tick` is the monotonic server tick stamp; it is advanced by the caller. A
/// snapshot is encoded only when `tick % SNAPSHOT_TICK_INTERVAL == 0`, but the
/// tracker ingests every tick so an entity that changes and reverts within the
/// interval is still detected on the boundary it is sampled.
pub(crate) fn host_replicate(
    registry: &EntityRegistry,
    server: &mut NetServer,
    allocator: &mut NetworkIdAllocator,
    replication: &mut ServerReplication,
    replicable: &ReplicableSet,
    tick: u32,
) {
    // Owned post-tick snapshot rule: copy replicable state into owned mirrors keyed
    // by NetworkId while borrowing the registry, then release before the net call.
    let owned = produce_owned_snapshots(registry, replicable, allocator);
    replication.ingest_tick(owned);

    // Snapshots emit at 20 Hz (every third 60 Hz tick); ingest ran every tick above.
    if tick % SNAPSHOT_TICK_INTERVAL != 0 {
        return;
    }

    let accepted = server.accepted_clients();
    if accepted.is_empty() {
        return;
    }
    // One sequence shared across all clients in this 20 Hz batch.
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
            // Phase 1/2 round-trips input but does not yet apply it to gameplay.
            wire::ClientMessage::Input(_) => {}
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
    // 1. Emit a probe if the 5 Hz cadence is due.
    if let Some(req) = time_sync.sender.maybe_send(&time_sync.clock, client_tick) {
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

/// Collect the world-space positions of every replicated remote entity for the
/// client-side debug wireframe (M15 Phase 1 visibility aid). Returns the
/// `Transform.position` of each `EntityId` in the client's `NetworkId ->
/// EntityId` map; empty for single-player and the host (no client map).
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
    let NetEndpoint::Client { map, .. } = endpoint else {
        return Vec::new();
    };
    map.values()
        .filter_map(|&id| {
            registry
                .get_component::<Transform>(id)
                .ok()
                .map(|t| t.position)
        })
        .collect()
}

/// Spawn a fresh entity seeded from a replicated `ComponentValue`. A spawn always
/// installs a `Transform`; if the payload is a `Transform`, spawn directly from
/// it, otherwise spawn at the default pose and set the component.
fn spawn_from_value(registry: &mut EntityRegistry, value: ComponentValue) -> EntityId {
    match value {
        ComponentValue::Transform(transform) => registry.spawn(transform),
        other => {
            let id = registry.spawn(Transform::default());
            // The id was just returned by spawn, so the only failure mode is an
            // unsupported component kind, which cannot occur in Phase 1.
            let _ = registry.set_component_value(id, other);
            id
        }
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

        let payload = ComponentPayload::Transform(wire);
        let value =
            payload_to_component_value(&payload).expect("Transform payload converts to a value");
        let ComponentValue::Transform(rebuilt) = value else {
            panic!("Transform payload must rebuild a Transform component");
        };

        assert!((rebuilt.position - original.position).length() < EPSILON);
        // angle_between is 0 when rotations match.
        assert!(rebuilt.rotation.angle_between(original.rotation) < 1e-4);
        // Phase 2 replicates scale; it must round-trip through the wire mirror.
        assert!((rebuilt.scale - original.scale).length() < EPSILON);
    }

    // --- Apply: spawns on first sight, mutates the same EntityId on second. ---
    #[test]
    fn apply_spawns_on_first_sight_and_mutates_on_second() {
        let mut registry = EntityRegistry::new();
        let mut map: HashMap<NetworkId, EntityId> = HashMap::new();

        let first_pos = Vec3::new(1.0, 2.0, 3.0);
        let snapshot1 = Snapshot {
            tick: 1,
            entries: vec![(
                NetworkId(42),
                ComponentPayload::Transform(transform_to_wire(&Transform {
                    position: first_pos,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                })),
            )],
        };
        apply(&mut registry, &snapshot1, &mut map);

        assert_eq!(map.len(), 1, "first sight records exactly one mapping");
        let spawned = *map.get(&NetworkId(42)).expect("NetworkId(42) mapped");
        let after_first = registry
            .get_component::<Transform>(spawned)
            .expect("spawned entity carries a Transform");
        assert!((after_first.position - first_pos).length() < EPSILON);

        // Second snapshot: same NetworkId, moved position. Must mutate, not respawn.
        let moved_pos = Vec3::new(10.0, 20.0, 30.0);
        let snapshot2 = Snapshot {
            tick: 2,
            entries: vec![(
                NetworkId(42),
                ComponentPayload::Transform(transform_to_wire(&Transform {
                    position: moved_pos,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                })),
            )],
        };
        apply(&mut registry, &snapshot2, &mut map);

        assert_eq!(map.len(), 1, "second snapshot does not add a new mapping");
        let same = *map.get(&NetworkId(42)).expect("mapping unchanged");
        assert_eq!(
            same, spawned,
            "same NetworkId must map to the same EntityId"
        );
        let after_second = registry
            .get_component::<Transform>(same)
            .expect("entity still live");
        assert!(
            (after_second.position - moved_pos).length() < EPSILON,
            "second snapshot moves the existing entity"
        );
    }

    #[test]
    fn apply_phase1_never_despawns_missing_network_ids() {
        let mut registry = EntityRegistry::new();
        let mut map: HashMap<NetworkId, EntityId> = HashMap::new();

        let snapshot1 = Snapshot {
            tick: 1,
            entries: vec![
                (
                    NetworkId(1),
                    ComponentPayload::Transform(transform_to_wire(&Transform::default())),
                ),
                (
                    NetworkId(2),
                    ComponentPayload::Transform(transform_to_wire(&Transform::default())),
                ),
            ],
        };
        apply(&mut registry, &snapshot1, &mut map);
        let id1 = *map.get(&NetworkId(1)).unwrap();
        let id2 = *map.get(&NetworkId(2)).unwrap();

        // Second snapshot omits NetworkId(2). Phase 1 leaves it untouched.
        let snapshot2 = Snapshot {
            tick: 2,
            entries: vec![(
                NetworkId(1),
                ComponentPayload::Transform(transform_to_wire(&Transform::default())),
            )],
        };
        apply(&mut registry, &snapshot2, &mut map);

        assert!(registry.exists(id1), "present id stays live");
        assert!(
            registry.exists(id2),
            "omitted NetworkId is NOT despawned in Phase 1"
        );
        assert!(map.contains_key(&NetworkId(2)), "mapping is retained");
    }

    // --- Apply: a non-finite wire Transform (NaN/Inf) is dropped at the
    // boundary — never spawned, never set — while finite entries in the same
    // snapshot still apply. Guards downstream interpolation/camera/culling math
    // from a hostile or buggy host. ---
    #[test]
    fn apply_skips_non_finite_transform_entry() {
        let mut registry = EntityRegistry::new();
        let mut map: HashMap<NetworkId, EntityId> = HashMap::new();

        // Entry A: position carries NaN — must be skipped. Build the wire mirror
        // directly so the non-finite value survives to the apply boundary.
        let poisoned = WireTransform {
            position: [f32::NAN, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        };
        // Entry B: a finite transform in the same snapshot — must still apply.
        let finite_pos = Vec3::new(4.0, 5.0, 6.0);
        let snapshot = Snapshot {
            tick: 1,
            entries: vec![
                (NetworkId(1), ComponentPayload::Transform(poisoned)),
                (
                    NetworkId(2),
                    ComponentPayload::Transform(transform_to_wire(&Transform {
                        position: finite_pos,
                        rotation: Quat::IDENTITY,
                        scale: Vec3::ONE,
                    })),
                ),
            ],
        };
        apply(&mut registry, &snapshot, &mut map);

        // The poisoned entry never spawned and never recorded a mapping.
        assert!(
            !map.contains_key(&NetworkId(1)),
            "non-finite entry must not spawn or map"
        );
        // The finite entry in the same snapshot applied normally.
        assert_eq!(map.len(), 1, "only the finite entry maps");
        let spawned = *map.get(&NetworkId(2)).expect("finite entry mapped");
        let applied = registry
            .get_component::<Transform>(spawned)
            .expect("finite entry carries a Transform");
        assert!((applied.position - finite_pos).length() < EPSILON);

        // A second snapshot resending the poison for an already-mapped id must
        // not overwrite the good state with a non-finite pose (skip applies on
        // the set path too).
        let snapshot2 = Snapshot {
            tick: 2,
            entries: vec![(
                NetworkId(2),
                ComponentPayload::Transform(WireTransform {
                    position: [f32::INFINITY, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0],
                }),
            )],
        };
        apply(&mut registry, &snapshot2, &mut map);
        let unchanged = registry
            .get_component::<Transform>(spawned)
            .expect("entity still live");
        assert!(
            (unchanged.position - finite_pos).length() < EPSILON,
            "non-finite set must not overwrite a good pose"
        );
    }

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
