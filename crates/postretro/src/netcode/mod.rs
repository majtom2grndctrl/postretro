// Engine-side netcode glue: role selection, `NetworkId <-> EntityId` maps,
// connection-slot lifecycle, game-logic-owned host serialize/client apply,
// interpolation, prediction, and reconciliation. This is the sole engine code
// that mutates the registry for replication.
// See: context/lib/networking.md

use std::collections::VecDeque;

mod client;
mod command_queue;
mod descriptor_class;
// The ordered frame stages of the replicated-state → presentation path (snapshot apply,
// then state-crossing detection). Owns the order via a witness value so `App` and the
// headless co-op harness cannot each invent their own sequencing.
pub(crate) mod frame_order;
mod interpolation;
mod lifecycle;
mod movement_state;
mod prediction;
mod reconcile;
mod remote_materialize;
mod replication;
// M15 Phase 3.5: the replicated-slot schema/fingerprint/lowering, the engine
// production (`HostStateReplication`) and client apply (`ClientStateApply`) glue. The
// schema is the only place the engine maps `StateSlotId <-> dotted name`; the net
// trackers stay registry-blind.
mod state_slots;
mod tuning_payload;
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
// M15 Phase 3.5 Task 6: the conditioned-loss state-slot harness. Drives the real
// host production / client apply glue through the dev `PacketConditioner` under loss,
// proving shared + owner-private slots converge and a dropped baseline repairs via
// `StateBaselineRefresh` without reconnect.
#[cfg(test)]
mod state_slot_loss_harness_test;
// Test-only co-op harness covers trigger-state replication through client presentation.
// It shares production ordering; trigger events remain local and never cross the wire.
#[cfg(test)]
mod trigger_state_channel_harness_test;
// E10 (Networked Enemy Authority Baseline) Task 7: the integration harness proving the
// whole host→client enemy path end to end (host registration → wire → conditioned link
// → client remote-presentation materialization → interpolation → despawn cleanup →
// late join). Drives the genuine Task 1-6 seams; carries the manual loopback recipe.
#[cfg(test)]
mod enemy_replication_harness_test;

pub(crate) use client::{ClientPresentationInputs, ClientReplication, MoverCorrection};
pub(crate) use command_queue::{
    HostCommandQueues, MovementOwners, ResolvedPawnCommand, WeaponOwners,
    active_wieldable_for_pawn, host_resolve_remote_commands,
};
// `ResolvedCommand` / `ResolutionSource` are produced by the command queue and consumed
// via the submodule path only; not re-exported here.
pub(crate) use interpolation::{DemoMover, InterpolationDelayState, MAX_DELAY_MICROS};
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
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use reconcile::reconcile_local_pawn;
#[cfg(test)]
pub(crate) use replication::produce_owned_snapshots;
pub(crate) use replication::{
    ReplicableSet, host_register_loaded_movers, host_register_map_enemies,
};
pub(crate) use tuning_payload::{TuningPayload, WieldableTuningPayload};
pub(crate) use wire_convert::sim_command_to_input;

// The conversion/merge helpers (`wire_convert`, `movement_state`) live in their focused
// submodules and are imported by callers via the direct submodule path.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use glam::{Quat, Vec3};
use parry3d::math::{Point, Vector};

use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::inventory::{Inventory, WIELDABLE_SLOT_CAPACITY};
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, EntityTypeDescriptor, SlotTable,
    Transform,
};
use postretro_foundation::{NavAgentParams, PlayerMovementComponent};
use postretro_net::replication::ServerReplication;
use postretro_net::timesync::{
    self, ClockEstimator, MonotonicClock, TimeSyncRequest, TimeSyncSender,
};
use postretro_net::transport::{NetClient, NetServer, ServerPoll};
use postretro_net::wire::{
    self, ComponentPayload, DivergenceReason, EntityRecord, NetworkId, RawSnapshotMessage,
    ServerControlMessage, SnapshotMessage, ValidationError, WireError, WireKinematicMoverState,
    WireMovementState, WirePlayerMovementState, WireTransform,
};

use crate::collision::{self, CollisionWorld};
use crate::movement::MovementCollisionSource;
use crate::scripting_systems;
use crate::sim::SimCommand;
use crate::weapon::{self, ActivationOutcome, WeaponImpact};
use tuning_payload::decode_tuning_payload;

/// Synchronize dirty pawn inventory changes into third-person presentation
/// attachments immediately before snapshot production. `WeaponOwners` is only the
/// dirty attachment queue; each drain resolves the active instance from live pawn
/// inventory before mirroring its descriptor identity onto the pawn mesh.
pub(crate) fn synchronize_weapon_owner_attachments(
    registry: &mut EntityRegistry,
    weapon_owners: &mut WeaponOwners,
    descriptors: &[EntityTypeDescriptor],
    hit_zone_store: &crate::scripting_systems::hit_zones::HitZoneStore,
) -> Vec<EntityId> {
    weapon_owners
        .take_attachment_changes(registry)
        .into_iter()
        .filter_map(|(pawn, weapon)| {
            let active_weapon_archetype = weapon.and_then(|weapon| {
                registry
                    .get_component::<DescriptorProvenance>(weapon)
                    .ok()
                    .map(|provenance| provenance.canonical_name.clone())
            });
            remote_materialize::update_active_weapon_attachment(
                registry,
                pawn,
                descriptors,
                active_weapon_archetype.as_deref(),
                hit_zone_store,
            )
            .then_some(pawn)
        })
        .collect()
}

/// Synchronize one local pawn's third-person attachment from its active inventory
/// weapon. Single-player has no dirty attachment queue, and the listen host installs
/// its own pawn before the regular pre-snapshot synchronization can run.
pub(crate) fn synchronize_weapon_attachment_for_pawn(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    descriptors: &[EntityTypeDescriptor],
    hit_zone_store: &crate::scripting_systems::hit_zones::HitZoneStore,
) -> bool {
    let active_weapon_archetype = active_wieldable_for_pawn(registry, pawn).and_then(|weapon| {
        registry
            .get_component::<DescriptorProvenance>(weapon)
            .ok()
            .map(|provenance| provenance.canonical_name.clone())
    });
    remote_materialize::update_active_weapon_attachment(
        registry,
        pawn,
        descriptors,
        active_weapon_archetype.as_deref(),
        hit_zone_store,
    )
}

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
        /// Boxed to keep the endpoint variants compact. It remains owned by the
        /// host endpoint with the slot lifecycle it serves.
        allocator: Box<NetworkIdAllocator>,
        /// Monotonic fixed-simulation tick stamp written into each snapshot.
        /// Advanced once per completed host fixed tick, not once per network send.
        tick: u32,
        /// Last fixed tick that emitted a snapshot batch. Redraws may run the
        /// send path again without advancing `tick`; they must not emit or sample
        /// owner-private projection twice for that tick.
        last_emitted_snapshot_tick: Option<u32>,
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
        /// E16 host dirty attachment queue. Fire, owner-private projections, snapshot
        /// archetypes, and hit-declaration ingest resolve a pawn's active weapon from
        /// its `Inventory`; this queue only schedules mesh attachment refreshes.
        weapon_owners: WeaponOwners,
        /// E16 host-authorized shots that are still open for a future client HIT
        /// declaration. Keyed by deterministic `ShotId`; Task 6 validates ownership
        /// and retires entries from this store.
        open_shots: OpenAuthorizedShots,
        /// E16 client HIT declarations received before the matching fixed-sim FIRE
        /// authorization has opened its shot. Flushed after host weapon simulation
        /// records authorized shots, so same-frame Input(FIRE)+HitDeclaration can
        /// settle in order without losing owner-private verdict scoping.
        pending_hit_declarations: PendingHitDeclarations,
        /// De-dup latch for weaponless remote pawns that try to fire. Missing weapons
        /// are a normal descriptor state, so this logs once per pawn rather than as an
        /// error every tick.
        weaponless_fire_logged: std::collections::HashSet<EntityId>,
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
        /// E10 Task 4: the set of map-placed or runtime-spawned AI enemy `EntityId`s the host
        /// has registered for outbound replication this level. The single owner of that id set so a level
        /// reload has one place to clean up: `host_register_map_enemies` unregisters every
        /// stale id here before registering the freshly-spawned level's enemies (the
        /// registry bumps generations on despawn, so a reloaded enemy is a distinct
        /// entity). Empty until the first level install registers enemies, and on a map
        /// with no AI enemies.
        map_enemies: std::collections::HashSet<EntityId>,
        /// PRL-loaded kinematic movers registered for outbound replication. Clients
        /// bind these by `mover_id` to their already-loaded local mover entities
        /// rather than spawning from the baseline.
        loaded_movers: std::collections::HashSet<EntityId>,
        /// Task 6 Phase 2 net-demo fixture. When the demo path is active
        /// (`POSTRETRO_NET_DEMO_MOVER=1`), the host spawns one deterministic
        /// AI-less mover ([`DemoMover`]) and stores its `EntityId` here; each tick
        /// it is driven along its parametric loop and replicated like any other
        /// authoritative object. `None` when the demo path is off (production /
        /// ordinary host) or before the first tick spawns it. Not a gameplay
        /// archetype — it carries no script/FGD surface.
        demo_mover: DemoMoverState,
        /// M15 Phase 3.5 replicated-state production: the deterministic replicated-slot
        /// schema (built once from the live `SlotTable`) and the registry-blind
        /// `ServerStateReplication` tracker. The frame send path ingests this frame's
        /// projected values and produces per-client state records spliced into the
        /// entity snapshot envelope. Boxed to keep the variant compact.
        state_slots: Box<state_slots::HostStateReplication>,
        /// Last authoritative tuning sent for each participating slot. This is a
        /// change detector only: demotion removes the entry and promotion sends a
        /// fresh payload.
        last_sent_tuning: HashMap<u64, TuningPayload>,
        missing_identity_warned: bool,
    },
    /// Client plus the Phase 2 client replication state (the `NetworkId -> EntityId`
    /// map, per-entity baseline table, pending-repair set, sequence tracking). The
    /// `NetClient` and replication tracker are boxed to keep this variant compact.
    Client {
        client: Box<NetClient>,
        replication: Box<ClientReplication>,
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
        /// M15 Phase 3.5 replicated-state apply: the deterministic schema (identical to
        /// the server's, built once from the live `SlotTable`) and the per-slot held
        /// baseline. The snapshot receive path validates the whole state batch against
        /// the schema and applies it all-or-nothing through the store-write path before
        /// the UI read snapshot is built.
        state_slots: Box<state_slots::ClientStateApply>,
        /// Host-resolved values the client predicts with. The generation lets the
        /// movement materializer rebuild after a staged host retune without relying
        /// on Control/Snapshot ordering.
        tuning: Option<TuningPayload>,
        tuning_generation: u64,
        applied_movement_tuning_generation: u64,
    },
}

#[derive(Debug, Default)]
pub(crate) struct ClientApplyFrameOutcome {
    pub(crate) materialized_remote_entity_presentation: bool,
    pub(crate) armed_local_pawn: Option<ClientArmedLocalPawn>,
    pub(crate) owner_private_weapon_cooldown_fresh: bool,
    /// Final authoritative mover correction per mover received this frame.
    /// App consumes these after snapshot apply to refresh the live carry table.
    pub(crate) mover_corrections: Vec<client::MoverCorrection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClientArmedLocalPawn {
    pub(crate) entity_id: EntityId,
    pub(crate) entity_class: Option<String>,
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

/// The world-independent result of advancing a live network endpoint.
///
/// World-less frames intentionally retain host lifecycle events for the engine:
/// a level transition can demote a slot while no world exists. Snapshot apply,
/// prediction, state-crossing detection, and command draining are deliberately
/// absent here; those operations have no meaningful target without a world.
#[must_use = "world-less host lifecycle events must be handled by the engine"]
pub(crate) enum WorldLessPoll {
    /// Host-side gate verdicts and lifecycle transitions produced by this poll.
    Host(ServerPoll),
    /// A client transport advance plus every typed server Control message that
    /// arrived during it. The App routes these after the endpoint borrow ends.
    Client(Vec<ServerControlMessage>),
    /// The transport failed after logging its diagnostic. Keep the frame alive,
    /// matching the Running-path error handling.
    Failed,
}

/// Route every server Control variant through the one client-side drain. Control
/// is reliable and ordered, so splitting relevel, diagnostic, and tuning drains
/// would let one consumer steal another consumer's message.
pub(crate) fn client_drain_control(app: &mut crate::App, controls: Vec<ServerControlMessage>) {
    for control in controls {
        match control {
            ServerControlMessage::Relevel(catalog_id) => {
                app.follow_relevel_catalog(catalog_id);
            }
            ServerControlMessage::Divergence(DivergenceReason::Closing(cause)) => {
                // Admission failure is terminal, so this is the client-visible
                // diagnostic for a player who cannot join the host.
                log::error!(
                    "[Net] incompatible host: {}",
                    DivergenceReason::Closing(cause)
                );
            }
            ServerControlMessage::Divergence(DivergenceReason::Holding(cause)) => {
                log::warn!("[Net] host is holding this client for content parity: {cause:?}");
                if let Some(session) = app.session.as_mut()
                    && let Some(endpoint) = session.net_endpoint.as_mut()
                {
                    let mut registry = session.scripting.script_ctx.registry.borrow_mut();
                    endpoint.demote_client_state(&mut registry);
                }
            }
            ServerControlMessage::Tuning(bytes) => {
                let script_ctx = app
                    .session
                    .as_ref()
                    .map(|session| session.scripting.script_ctx.clone());
                let descriptors = script_ctx
                    .as_ref()
                    .map(|script_ctx| script_ctx.data_registry.borrow().entities.clone())
                    .unwrap_or_default();
                if let Some(session) = app.session.as_mut()
                    && let Some(endpoint) = session.net_endpoint.as_mut()
                {
                    let mut registry = session.scripting.script_ctx.registry.borrow_mut();
                    endpoint.install_tuning_payload(&bytes, &mut registry, &descriptors);
                }
            }
        }
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
                let server = NetServer::new(socket, public_addr, MAX_CLIENTS, now(), None)
                    .map_err(|e| format!("host transport init failed: {e}"))?;
                Ok(Some(NetEndpoint::Host {
                    server: Box::new(server),
                    allocator: Box::new(NetworkIdAllocator::new()),
                    tick: 0,
                    last_emitted_snapshot_tick: None,
                    replication: Box::new(ServerReplication::new()),
                    replicable: ReplicableSet::new(),
                    slot_pawns: SlotPawns::new(),
                    command_queues: HostCommandQueues::new(),
                    owners: MovementOwners::new(),
                    weapon_owners: WeaponOwners::new(),
                    open_shots: OpenAuthorizedShots::new(),
                    pending_hit_declarations: PendingHitDeclarations::new(),
                    weaponless_fire_logged: std::collections::HashSet::new(),
                    host_pawn: None,
                    map_enemies: std::collections::HashSet::new(),
                    loaded_movers: std::collections::HashSet::new(),
                    demo_mover: DemoMoverState::from_env(),
                    state_slots: Box::new(state_slots::HostStateReplication::new()),
                    last_sent_tuning: HashMap::new(),
                    missing_identity_warned: false,
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
                let client = NetClient::new(socket, *addr, client_id, now(), None)
                    .map_err(|e| format!("client transport init failed: {e}"))?;
                Ok(Some(NetEndpoint::Client {
                    client: Box::new(client),
                    replication: Box::new(ClientReplication::new()),
                    time_sync: Box::new(ClientTimeSync::new()),
                    interpolation_delay: InterpolationDelayState::new(),
                    prediction: ClientPrediction::new(),
                    state_slots: Box::new(state_slots::ClientStateApply::new()),
                    tuning: None,
                    tuning_generation: 0,
                    applied_movement_tuning_generation: 0,
                }))
            }
        }
    }

    pub(crate) fn set_mod_identity(&mut self, id: String, version: String) {
        match self {
            Self::Host { server, .. } => server.set_mod_identity(id, version),
            Self::Client { client, .. } => client.set_mod_identity(id, version),
        }
    }

    /// A debug no-start-script session otherwise looks exactly like a stalled
    /// pending handshake. Emit the operator-facing warning once when a peer has
    /// actually arrived, rather than noisily at boot before anyone connects.
    pub(crate) fn warn_once_if_mod_identity_missing(&mut self) {
        let Self::Host {
            server,
            missing_identity_warned,
            ..
        } = self
        else {
            return;
        };
        if !*missing_identity_warned && server.has_connected_clients() && !server.has_mod_identity()
        {
            log::warn!("[Net] no mod identity installed");
            *missing_identity_warned = true;
        }
    }

    pub(crate) fn set_mod_digest(&mut self, digest: [u8; 32]) {
        match self {
            Self::Host { server, .. } => server.set_mod_digest(Some(digest)),
            Self::Client { client, .. } => client.set_mod_digest(Some(digest)),
        }
    }

    pub(crate) fn set_level_parity(&mut self, level: Option<(String, [u8; 32])>) {
        match self {
            Self::Host { server, .. } => server.set_level_parity(level),
            Self::Client { client, .. } => client.set_level_parity(level),
        }
    }

    pub(crate) fn set_relevel_catalog_id(&mut self, catalog_id: Option<String>) {
        if let Self::Host { server, .. } = self {
            server.set_relevel_catalog_id(catalog_id);
        }
    }

    /// Advance transport work that remains valid when no level world is
    /// installed: socket I/O, keepalive, and handshake/gate processing.
    ///
    /// The registry argument intentionally reserves the game-logic-owned
    /// control-drain seam. The currently available transport advance never
    /// mutates it: world-less polling must not apply snapshots, simulate,
    /// predict, detect state crossings, or drain commands. Task 4's typed
    /// Control router and Task 7's demotion handling use this same borrow.
    pub(crate) fn poll_world_less(
        &mut self,
        dt: Duration,
        registry: &mut EntityRegistry,
    ) -> WorldLessPoll {
        let _ = registry;
        match self {
            NetEndpoint::Host { server, .. } => match server.update(dt) {
                Ok(poll) => WorldLessPoll::Host(poll),
                Err(err) => {
                    log::error!("[Net] host update failed: {err}");
                    WorldLessPoll::Failed
                }
            },
            NetEndpoint::Client { client, .. } => {
                if let Err(err) = client.update(dt) {
                    log::error!("[Net] client update failed: {err}");
                    WorldLessPoll::Failed
                } else {
                    // A world-less frame can never apply snapshots. Drain current-
                    // epoch bytes too: otherwise a snapshot queued between unload
                    // and replacement install can mutate the new world later.
                    discard_world_less_snapshots(client);
                    WorldLessPoll::Client(client.drain_control())
                }
            }
        }
    }

    /// Reset connected-client state derived from the old level, preserving the
    /// transport connection. Previously-known `NetworkId`s immediately request fresh
    /// full baselines so unchanged acked remotes are not lost after the local registry
    /// is cleared. Replicated state-slot schema and baselines rebuild from the next
    /// installed level.
    pub(crate) fn reset_level_scoped_client_state(&mut self) {
        let NetEndpoint::Client {
            client,
            replication,
            interpolation_delay,
            prediction,
            state_slots,
            ..
        } = self
        else {
            return;
        };

        let refresh_requests = replication.reset_for_level_unload();
        for req in refresh_requests {
            client.send_input(wire::encode(&wire::ClientMessage::BaselineRefresh(req)));
        }
        prediction.reset_for_level_unload();
        interpolation_delay.reset_for_level_unload();
        state_slots.reset_schema();
    }

    /// Clear state whose entity ids belong to the old host level. This is separate
    /// from per-slot demotion cleanup: a level unload invalidates even host-owned
    /// and unowned replicated objects.
    pub(crate) fn reset_level_scoped_host_state(&mut self) {
        let Self::Host {
            allocator,
            replication,
            replicable,
            slot_pawns,
            command_queues,
            owners,
            weapon_owners,
            open_shots,
            pending_hit_declarations,
            weaponless_fire_logged,
            host_pawn,
            map_enemies,
            loaded_movers,
            demo_mover,
            state_slots,
            last_sent_tuning,
            ..
        } = self
        else {
            return;
        };
        allocator.reset_for_level_unload();
        replication.reset_for_level_unload();
        *replicable = ReplicableSet::new();
        *slot_pawns = SlotPawns::new();
        *command_queues = HostCommandQueues::new();
        *owners = MovementOwners::new();
        *weapon_owners = WeaponOwners::new();
        *open_shots = OpenAuthorizedShots::new();
        *pending_hit_declarations = PendingHitDeclarations::new();
        weaponless_fire_logged.clear();
        *host_pawn = None;
        map_enemies.clear();
        loaded_movers.clear();
        *demo_mover = DemoMoverState::from_env();
        state_slots.reset_schema();
        last_sent_tuning.clear();
    }

    pub(crate) fn reset_state_slot_schema(&mut self) {
        match self {
            Self::Host {
                server,
                state_slots,
                ..
            } => state_slots.reset_schema_for_clients(server.participating_clients()),
            Self::Client { state_slots, .. } => state_slots.reset_schema(),
        }
    }

    /// Client demotion is not a normal unload reset: no repair requests are useful
    /// while the host intentionally holds the slot. Despawn mapped entities first so
    /// this is also correct for a following relevel unload.
    pub(crate) fn demote_client_state(&mut self, registry: &mut EntityRegistry) {
        let Self::Client {
            replication,
            interpolation_delay,
            prediction,
            tuning,
            applied_movement_tuning_generation,
            ..
        } = self
        else {
            return;
        };
        replication.despawn_all_mapped(registry);
        replication.reset_for_demotion();
        prediction.reset_for_level_unload();
        interpolation_delay.reset_for_level_unload();
        *tuning = None;
        *applied_movement_tuning_generation = 0;
    }

    fn install_tuning_payload(
        &mut self,
        bytes: &[u8],
        registry: &mut EntityRegistry,
        descriptors: &[EntityTypeDescriptor],
    ) {
        let Self::Client {
            replication,
            tuning,
            tuning_generation,
            applied_movement_tuning_generation,
            ..
        } = self
        else {
            return;
        };
        let result = replace_client_tuning(tuning, tuning_generation, bytes);
        if let Some(armed) = replication.armed_local_pawn() {
            apply_installed_movement_tuning_to_armed_pawn(
                &armed,
                tuning.as_ref(),
                *tuning_generation,
                applied_movement_tuning_generation,
                descriptors,
                registry,
            );
        } else if tuning
            .as_ref()
            .and_then(|payload| payload.movement.as_ref())
            .is_none()
        {
            *applied_movement_tuning_generation = *tuning_generation;
        }
        if let Err(error) = result {
            log::error!("[Net] tuning payload epoch/decode failure: {error}");
        }
    }

    pub(crate) fn client_tuning(&self) -> Option<(&TuningPayload, u64)> {
        match self {
            Self::Client {
                tuning: Some(tuning),
                tuning_generation,
                ..
            } => Some((tuning, *tuning_generation)),
            _ => None,
        }
    }
}

fn apply_installed_movement_tuning_to_armed_pawn(
    armed: &client::ArmedLocalPawn,
    tuning: Option<&TuningPayload>,
    tuning_generation: u64,
    applied_generation: &mut u64,
    descriptors: &[EntityTypeDescriptor],
    registry: &mut EntityRegistry,
) {
    remote_materialize::materialize_armed_local_pawn(
        armed,
        descriptors,
        registry,
        tuning.and_then(|payload| payload.movement.as_ref()),
        tuning,
        true,
    );
    *applied_generation = tuning_generation;
}

fn discard_world_less_snapshots(client: &mut NetClient) {
    drop(client.drain_snapshots());
}

fn replace_client_tuning(
    tuning: &mut Option<TuningPayload>,
    tuning_generation: &mut u64,
    bytes: &[u8],
) -> Result<(), tuning_payload::TuningPayloadError> {
    // Invalidate first. A bad replacement must never leave the last accepted
    // descriptor-derived prediction state live. This reaches only the queued
    // tuning value: live Inventory instances are intentionally untouched, so an
    // in-flight equip keeps its already-latched duration and state.
    *tuning = None;
    *tuning_generation = tuning_generation.wrapping_add(1);
    *tuning = Some(decode_tuning_payload(bytes)?);
    Ok(())
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
    reverse: HashMap<NetworkId, EntityId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ShotId(u64);

impl ShotId {
    pub(crate) fn from_parts(pawn: NetworkId, client_tick: u32) -> Self {
        Self((u64::from(pawn.0) << 32) | u64::from(client_tick))
    }

    pub(crate) fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub(crate) fn raw(self) -> u64 {
        self.0
    }

    fn client_tick(self) -> u32 {
        self.0 as u32
    }
}

const HIT_RANGE_TOLERANCE: f32 = 1.25;
const MAX_OPEN_SHOT_AGE_TICKS: u32 = 180;
const MAX_PENDING_HIT_DECLARATIONS_PER_CLIENT: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AuthorizedShot {
    pub(crate) shot_id: ShotId,
    pub(crate) pawn: EntityId,
    pub(crate) weapon: EntityId,
    pub(crate) fire_tick: u32,
    pub(crate) damage: f32,
    pub(crate) range: f32,
    pub(crate) pellet_count: usize,
    pub(crate) credit_source: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OpenAuthorizedShot {
    pub(crate) shot: AuthorizedShot,
    pub(crate) owner_client_id: u64,
}

#[derive(Debug, Default)]
pub(crate) struct OpenAuthorizedShots {
    shots: HashMap<ShotId, OpenAuthorizedShot>,
}

impl OpenAuthorizedShots {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn record(&mut self, shot: AuthorizedShot, owner_client_id: u64) {
        self.shots.insert(
            shot.shot_id,
            OpenAuthorizedShot {
                shot,
                owner_client_id,
            },
        );
    }

    pub(crate) fn get(&self, shot_id: ShotId) -> Option<OpenAuthorizedShot> {
        self.shots.get(&shot_id).cloned()
    }

    pub(crate) fn retire(&mut self, shot_id: ShotId) -> Option<OpenAuthorizedShot> {
        self.shots.remove(&shot_id)
    }

    pub(crate) fn remove_client(&mut self, client_id: u64) {
        self.shots
            .retain(|_, shot| shot.owner_client_id != client_id);
    }

    pub(crate) fn remove_pawn(&mut self, pawn: EntityId) {
        self.shots.retain(|_, shot| shot.shot.pawn != pawn);
    }

    pub(crate) fn prune_stale(&mut self, current_tick: u32) {
        self.shots.retain(|_, shot| {
            current_tick.wrapping_sub(shot.shot.fire_tick) <= MAX_OPEN_SHOT_AGE_TICKS
        });
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.shots.len()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PendingHitDeclaration {
    pub(crate) client_id: u64,
    pub(crate) declaration: wire::HitDeclaration,
}

#[derive(Debug, Default)]
pub(crate) struct PendingHitDeclarations {
    declarations: VecDeque<PendingHitDeclaration>,
}

impl PendingHitDeclarations {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn push(&mut self, client_id: u64, declaration: wire::HitDeclaration) {
        let retained_for_client = self
            .declarations
            .iter()
            .filter(|pending| pending.client_id == client_id)
            .count();
        if retained_for_client >= MAX_PENDING_HIT_DECLARATIONS_PER_CLIENT
            && let Some(index) = self
                .declarations
                .iter()
                .position(|pending| pending.client_id == client_id)
        {
            self.declarations.remove(index);
        }
        self.declarations.push_back(PendingHitDeclaration {
            client_id,
            declaration,
        });
    }

    pub(crate) fn remove_client(&mut self, client_id: u64) {
        self.declarations
            .retain(|pending| pending.client_id != client_id);
    }

    pub(crate) fn remove_pawn_shots(&mut self, allocator: &NetworkIdAllocator, pawn: EntityId) {
        let Some(pawn_net) = allocator.network_id_for_entity(pawn) else {
            return;
        };
        self.declarations.retain(|pending| {
            let shot_id = ShotId::from_raw(pending.declaration.shot_id);
            (shot_id.raw() >> 32) as u32 != pawn_net.0
        });
    }

    fn drain_ready(
        &mut self,
        command_queues: &HostCommandQueues,
        open_shots: &OpenAuthorizedShots,
    ) -> Vec<PendingHitDeclaration> {
        let mut ready = Vec::new();
        let mut waiting = VecDeque::new();
        while let Some(pending) = self.declarations.pop_front() {
            let shot_id = ShotId::from_raw(pending.declaration.shot_id);
            let shot_open = open_shots.get(shot_id).is_some();
            let resolved_past_shot = command_queues
                .resolved_cursor(pending.client_id)
                .is_some_and(|cursor| prediction::client_tick_le(shot_id.client_tick(), cursor));
            if shot_open || resolved_past_shot {
                ready.push(pending);
            } else {
                waiting.push_back(pending);
            }
        }
        self.declarations = waiting;
        ready
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.declarations.len()
    }
}

impl NetworkIdAllocator {
    pub(crate) fn new() -> Self {
        Self {
            next: 0,
            map: HashMap::new(),
            reverse: HashMap::new(),
        }
    }

    fn reset_for_level_unload(&mut self) {
        self.map.clear();
        self.reverse.clear();
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
        self.reverse.insert(net_id, id);
        net_id
    }

    /// Drop the dead `EntityId -> NetworkId` mapping for an entity that no longer
    /// replicates (e.g. unregistered on level reload), so the map does not accrue
    /// one dead entry per ever-spawned replicable for the host's lifetime. Does
    /// not touch `next`: NetworkIds stay monotonic and are never recycled — only
    /// the stale mapping entry is pruned.
    pub(crate) fn forget(&mut self, id: EntityId) {
        if let Some(net_id) = self.map.remove(&id) {
            self.reverse.remove(&net_id);
        }
    }

    /// Resolve a host-side `EntityId` to its stable `NetworkId`, if it has been
    /// stamped.
    pub(crate) fn network_id_for_entity(&self, id: EntityId) -> Option<NetworkId> {
        self.map.get(&id).copied()
    }

    /// Resolve a declared wire `NetworkId` back to the current host entity, if the
    /// mapping is still live.
    pub(crate) fn entity_for_network_id(&self, net_id: NetworkId) -> Option<EntityId> {
        self.reverse.get(&net_id).copied()
    }

    /// Test-only: is an `EntityId -> NetworkId` mapping currently retained?
    #[cfg(test)]
    pub(crate) fn maps_entity(&self, id: EntityId) -> bool {
        self.map.contains_key(&id)
    }

    /// Test-only: is a `NetworkId -> EntityId` mapping currently retained?
    #[cfg(test)]
    pub(crate) fn maps_network_id(&self, net_id: NetworkId) -> bool {
        self.reverse.contains_key(&net_id)
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
        ComponentKind::KinematicMover => 13,
        ComponentKind::TriggerVolume => 14,
        ComponentKind::AmmoReserve => 15,
        ComponentKind::Spawner => 16,
        ComponentKind::EntityState => 17,
        ComponentKind::DeferredEffect => 18,
        ComponentKind::Inventory => 19,
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
        ComponentPayload::MeshAnimationState(_) => true,
        ComponentPayload::KinematicMoverState(m) => kinematic_mover_is_finite(m),
    }
}

fn kinematic_mover_is_finite(m: &WireKinematicMoverState) -> bool {
    m.segment_elapsed_ms.is_finite()
        && m.wait_remaining_ms.is_finite()
        && m.velocity.iter().all(|c| c.is_finite())
        && m.spin_angle_rad.is_finite()
        && m.spin_angle_before_tick_rad.is_finite()
        && m.spin_rate_rad_s.is_finite()
        && m.spin_target_rate_rad_s.is_finite()
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
        && m.aim_pitch.is_finite()
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
///
/// Returns `true` when this receive pass materialized at least one remote
/// presentation mesh, allowing the caller to resolve late-spawned animation clip
/// indices against the already-loaded model tables.
#[allow(clippy::too_many_arguments)]
pub(crate) fn client_receive_and_apply(
    registry: &mut EntityRegistry,
    slot_table: &mut SlotTable,
    client: &mut NetClient,
    replication: &mut ClientReplication,
    state_slots: &mut state_slots::ClientStateApply,
    prediction: &mut ClientPrediction,
    descriptors: &[EntityTypeDescriptor],
    hit_zone_store: &crate::scripting_systems::hit_zones::HitZoneStore,
    agent_params: Option<NavAgentParams>,
    collision: &impl MovementCollisionSource,
    gravity: f32,
    tick_dt: f32,
    frame_dt: Duration,
    mover_target_tick: Option<u32>,
    host_movement_tuning: Option<&postretro_foundation::PlayerMovementDescriptor>,
    host_tuning: Option<&TuningPayload>,
    rebuild_movement_tuning: bool,
) -> ClientApplyFrameOutcome {
    let mut frame_outcome = ClientApplyFrameOutcome::default();
    for bytes in client.drain_snapshots() {
        let snapshot = match decode_snapshot(&bytes) {
            Ok(snapshot) => snapshot,
            Err(err) => {
                log::warn!("[Net] dropping undecodable snapshot: {err}");
                continue;
            }
        };
        if descriptors.is_empty() && snapshot_requires_descriptor_table(&snapshot) {
            log::trace!(
                "[Net] deferring descriptor-backed snapshot {} until level descriptors load",
                snapshot.sequence
            );
            continue;
        }
        let target_tick = mover_target_tick
            .unwrap_or(snapshot.server_tick)
            .max(snapshot.server_tick);
        let mut outcome = replication.apply_snapshot_with_mover_target_tick(
            registry,
            &snapshot,
            target_tick,
            tick_dt,
        );
        for correction in &outcome.mover_corrections {
            if let Some(existing) = frame_outcome
                .mover_corrections
                .iter_mut()
                .find(|existing| existing.mover_id == correction.mover_id)
            {
                *existing = *correction;
            } else {
                frame_outcome.mover_corrections.push(*correction);
            }
        }

        // M15 Phase 3.5: apply this snapshot's replicated-state records. Validated as a
        // whole batch against the local schema, then committed all-or-nothing through
        // the engine store-write path (type/range/enum/finite checks run). Runs only on
        // a snapshot the entity apply accepted (a stale/duplicate sequence yields no
        // `ack`, so the state batch is skipped too — it carries the same superseded
        // frame). Applied BEFORE the UI read snapshot is built (the receive path runs in
        // the Game-logic stage, well before render), so the UI sees the authoritative
        // value next frame. The state acks ride back in the SAME `AckMessage` as the
        // entity acks (one ack per snapshot); refresh requests go out as their own
        // `ClientMessage::StateBaselineRefresh`.
        if outcome.ack.is_some() {
            let state_outcome = state_slots.apply_snapshot_state(
                slot_table,
                snapshot.sequence,
                &snapshot.state_schema_fingerprint,
                &snapshot.state_records,
            );
            frame_outcome.owner_private_weapon_cooldown_fresh |= state_outcome
                .fresh_slots
                .iter()
                .any(|name| name == "player.weaponCooldownMs");
            if let Some(ack) = outcome.ack.as_mut() {
                ack.slot_baselines = state_outcome.slot_baselines;
            }
            for req in state_outcome.refresh_requests {
                client.send_input(wire::encode(&wire::ClientMessage::StateBaselineRefresh(
                    req,
                )));
            }
        }
        // M15 Phase 3 Task 3 + Task 7: a `local_player` baseline arms client prediction
        // with the marked local pawn AND materializes its descriptor-backed presentation.
        // Arm and materialize BEFORE reconcile so the just-armed pawn reconciles on its
        // arming snapshot too; the materialization call-site glue lives in
        // `remote_materialize` (the focused seam also materializes remote enemies).
        if let Some(armed) = &outcome.armed_local_pawn {
            prediction.arm(armed.network_id, armed.entity_id);
            frame_outcome.materialized_remote_entity_presentation |=
                remote_materialize::materialize_armed_local_pawn(
                    armed,
                    descriptors,
                    registry,
                    host_movement_tuning,
                    host_tuning,
                    rebuild_movement_tuning,
                );
            frame_outcome.armed_local_pawn = Some(ClientArmedLocalPawn {
                entity_id: armed.entity_id,
                entity_class: armed.entity_class.clone(),
            });
        }
        // The local pawn's world body is shadow-only, but it still needs the same
        // third-person weapon silhouette as peers see. Its viewmodel remains separate
        // from this presentation-only attachment path.
        for update in &outcome.local_weapon_attachments {
            frame_outcome.materialized_remote_entity_presentation |=
                remote_materialize::update_active_weapon_attachment(
                    registry,
                    update.entity_id,
                    descriptors,
                    update.active_weapon_archetype.as_deref(),
                    hit_zone_store,
                );
        }
        // Each non-local baseline that just spawned a descriptor-class-bearing entity gets
        // its presentation materialized here, where the shared descriptor table is in scope
        // (the net-facing apply is descriptor-blind). A descriptor's `movement` block is the
        // durable player-type signal; names are author-controlled. Both paths attach ONLY the
        // descriptor mesh — no Brain/Agent/Health/Weapon/PlayerMovement — and are idempotent
        // plus unknown-class-tolerant, so a failed presentation still interpolates transform.
        for remote in &outcome.remote_entities {
            let descriptor = descriptors.iter().find(|descriptor| {
                descriptor.canonical_name.as_deref() == Some(remote.entity_class.as_str())
            });
            let materialized = if matches!(descriptor, Some(descriptor) if descriptor.movement.is_some())
            {
                let player_locomotion = descriptor.and_then(|descriptor| {
                    let movement = descriptor.movement.as_ref()?;
                    let mesh = descriptor.mesh.as_ref()?;
                    let idle_state = mesh.default_state.clone()?;
                    let walk_state = ["walk_forward", "walk"]
                        .into_iter()
                        .find(|state| mesh.animations.contains_key(*state))?
                        .to_string();
                    let run_state = mesh
                        .animations
                        .contains_key("run")
                        .then(|| "run".to_string());
                    let derived_travel_speed = |state_name: &str| {
                        let state = mesh.animations.get(state_name)?;
                        hit_zone_store
                            .get(&postretro_model::ModelHandle::from(mesh.model.clone()))
                            .and_then(|model| {
                                model
                                    .clips
                                    .iter()
                                    .find(|clip| clip.name == state.clip)
                                    .and_then(|clip| clip.travel_speed)
                            })
                    };
                    Some(client::RemotePlayerLocomotionReference {
                        idle_state,
                        walk_derived_travel_speed: derived_travel_speed(&walk_state),
                        run_derived_travel_speed: run_state
                            .as_deref()
                            .and_then(derived_travel_speed),
                        walk_state,
                        run_state,
                        walk_speed: movement.ground.speed.walk,
                        run_speed: movement.ground.speed.run,
                    })
                });
                replication.cache_remote_player_locomotion(remote.network_id, player_locomotion);
                remote_materialize::materialize_armed_remote_player(
                    remote,
                    descriptors,
                    registry,
                    agent_params,
                )
            } else {
                // Remote AI has no local AgentComponent, so derive its walk rate from
                // exactly the motion this client presents. The shared descriptor supplies
                // the immutable reference speed and the locomotion state; neither belongs
                // on the snapshot wire format.
                //
                // "The walking state" is picked by the same rule the host tick's
                // animation substitution uses: the graph's locomotion state, which chases
                // without an action of its own. The brain's graph is the descriptor's
                // authored `behavior` block, so both ends agree on what this enemy carries.
                let walk_reference = descriptor.and_then(|descriptor| {
                    let mesh = descriptor.mesh.as_ref()?;
                    let graph = descriptor.behavior.as_ref()?;
                    let locomotion = scripting_systems::ai::locomotion_animation(graph)?;
                    let state = mesh.animations.get(locomotion)?;
                    let derived_travel_speed = hit_zone_store
                        .get(&postretro_model::ModelHandle::from(mesh.model.clone()))
                        .and_then(|model| {
                            model
                                .clips
                                .iter()
                                .find(|clip| clip.name == state.clip)
                                .and_then(|clip| clip.travel_speed)
                        });
                    Some((
                        graph.move_speed,
                        locomotion.to_string(),
                        derived_travel_speed,
                    ))
                });
                replication.cache_remote_enemy_walk_playback(remote.network_id, walk_reference);
                remote_materialize::materialize_armed_remote_enemy(
                    remote,
                    descriptors,
                    registry,
                    agent_params,
                )
            };
            if materialized {
                if let Some(state) = remote.initial_animation_state.as_deref() {
                    client::apply_mesh_animation_state(registry, remote.entity_id, state, true);
                }
            }
            let attachment_changed = remote.weapon_attachment_changed
                && remote_materialize::update_active_weapon_attachment(
                    registry,
                    remote.entity_id,
                    descriptors,
                    remote.active_weapon_archetype.as_deref(),
                    hit_zone_store,
                );
            frame_outcome.materialized_remote_entity_presentation |=
                materialized || attachment_changed;
        }
        // M15 Phase 3 Task 5: reconcile the local predicted pawn against the
        // authoritative record this snapshot delivered — merge the movement subset,
        // restore the transform, prune through the host ack, replay the unacked tail,
        // snap the reconciled gameplay state, and seed the decaying presentation
        // offset (or snap on a teleport). The registry-touching orchestration lives in
        // `reconcile`; long-lived prediction/smoothing state lives in `prediction`.
        if let Some(local) = &outcome.local_reconcile {
            reconcile::reconcile_local_pawn_with_mover_history(
                registry,
                prediction,
                local.entity_id,
                local.transform,
                local.movement.as_ref(),
                local.acked_tick,
                local.server_tick,
                Some(replication.mover_history()),
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
    frame_outcome
}

fn snapshot_requires_descriptor_table(snapshot: &SnapshotMessage) -> bool {
    snapshot.records.iter().any(|record| match record {
        EntityRecord::FullBaseline { entity_class, .. }
        | EntityRecord::Delta { entity_class, .. } => entity_class.is_some(),
        EntityRecord::Despawn { .. } => false,
    })
}

/// Drive one connected-client predicted fixed tick (M15 Phase 3 Task 3). Sends
/// exactly one `ClientMessage::Input` for `command` (stamped with the next
/// monotonic `client_tick`) on the reliable `Channel::Input`, then — once
/// prediction is armed — advances the local pawn through the movement-only replay
/// helper and writes the predicted `Transform` + `PlayerMovementComponent` back to
/// the registry. Returns the sent `client_tick` even when prediction is not yet
/// armed, so the caller can bind current-frame client fire to the command it sent.
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
pub(crate) struct ClientPredictionTickContext<'a, C: MovementCollisionSource> {
    pub(crate) command: &'a SimCommand,
    pub(crate) aim_pitch: f32,
    pub(crate) collision: &'a C,
    pub(crate) gravity: f32,
    pub(crate) tick_dt: f32,
}

pub(crate) fn client_predict_tick<C: MovementCollisionSource>(
    registry: &mut EntityRegistry,
    client: &mut NetClient,
    prediction: &mut ClientPrediction,
    context: ClientPredictionTickContext<'_, C>,
) -> u32 {
    let ClientPredictionTickContext {
        command,
        aim_pitch,
        collision,
        gravity,
        tick_dt,
    } = context;
    // 1. Send exactly one Input command for this predicted tick, stamped with the
    //    next monotonic client_tick. Sent even before the baseline arms prediction
    //    so the host's command stream starts immediately on connect.
    let client_tick = prediction.next_client_tick();
    let input = sim_command_to_input(command, client_tick, aim_pitch);
    client.send_input(wire::encode(&wire::ClientMessage::Input(input)));

    // 2. Before the local baseline arms prediction, drive no provisional pawn.
    let Some(armed) = prediction.armed() else {
        return client_tick;
    };

    // 3. Read the armed pawn's current applied state (seeded from the authoritative
    //    baseline / last reconcile). A missing pawn means the mapping went stale
    //    between arming and now; skip this tick rather than predict from nothing.
    let prev = match (
        registry.get_component::<Transform>(armed.entity_id),
        registry.get_component::<PlayerMovementComponent>(armed.entity_id),
    ) {
        (Ok(transform), Ok(movement)) => (*transform, movement.clone()),
        _ => return client_tick,
    };

    // 4. Advance the local pawn one predicted tick through the movement-only helper
    //    and record it in the history ring.
    let Some((transform, movement)) =
        prediction.predict_tick(input, prev, collision, gravity, tick_dt)
    else {
        return client_tick;
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
    client_tick
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

pub(crate) fn client_peek_next_command_tick(endpoint: Option<&NetEndpoint>) -> Option<u32> {
    match endpoint {
        Some(NetEndpoint::Client { prediction, .. }) => Some(prediction.peek_next_client_tick()),
        _ => None,
    }
}

pub(crate) fn client_send_input_command(
    endpoint: Option<&mut NetEndpoint>,
    command: &SimCommand,
    aim_pitch: f32,
) -> Option<u32> {
    let Some(NetEndpoint::Client {
        client, prediction, ..
    }) = endpoint
    else {
        return None;
    };
    let client_tick = prediction.next_client_tick();
    let input = sim_command_to_input(command, client_tick, aim_pitch);
    client.send_input(wire::encode(&wire::ClientMessage::Input(input)));
    Some(client_tick)
}

pub(crate) fn client_local_pawn_network_id(endpoint: Option<&NetEndpoint>) -> Option<NetworkId> {
    match endpoint {
        Some(NetEndpoint::Client { replication, .. }) => replication.local_pawn_network_id(),
        _ => None,
    }
}

pub(crate) fn shot_id_raw(pawn: NetworkId, client_tick: u32) -> u64 {
    ShotId::from_parts(pawn, client_tick).raw()
}

pub(crate) fn client_send_hit_declaration(
    endpoint: Option<&mut NetEndpoint>,
    shot_id: u64,
    hits: &[weapon::LocalHitRecord],
) -> Option<usize> {
    let Some(NetEndpoint::Client {
        client,
        replication,
        ..
    }) = endpoint
    else {
        return None;
    };

    let records = local_hits_to_wire_records(hits, |entity_id| {
        replication.network_id_for_entity(entity_id)
    });
    let record_count = records.len();
    client.send_input(wire::encode(&wire::ClientMessage::HitDeclaration(
        wire::HitDeclaration { shot_id, records },
    )));
    Some(record_count)
}

fn local_hits_to_wire_records(
    hits: &[weapon::LocalHitRecord],
    mut resolve: impl FnMut(EntityId) -> Option<NetworkId>,
) -> Vec<wire::HitRecord> {
    hits.iter()
        .filter_map(|hit| {
            let target = resolve(hit.target)?;
            Some(wire::HitRecord {
                target: target.0,
                point: hit.point.to_array(),
                zone: hit.zone.clone(),
            })
        })
        .collect()
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
/// through the registry's remote-presentation helper. Game-logic-owned:
/// called once per frame, **after** `client_receive_and_apply` (which fills the
/// buffers) and **before** the render collectors read entities, so the renderer stays
/// read-only over the registry.
///
/// The render target is `estimated_server_tick - interpolation_delay`. Jitter sets
/// the baseline delay; recent held-newest starvation temporarily adds headroom.
/// Before the time-sync estimator has folded its first echo (`estimated_server_tick`
/// is `None`), there is no trustworthy clock to render against, so the buffers are
/// left unsampled and remote entities stay at their last-applied snapshot pose. Their
/// walk rate still derives from that held pose's zero XZ speed, while avatar aim uses
/// its newest finite held pitch. No server tick is invented before initialization.
///
/// The mutable registry borrow is threaded in by the caller (`main.rs`), so this
/// module never reaches into `App`.
///
/// `frame_dt_secs` is the frame's wall-clock delta (the same per-frame delta the frame
/// loop computes); it drives the framerate-independent starvation feedback so the
/// adaptive delay's time-constant does not scale with frame rate.
/// `frame_anim_time` is the slow-mo/freeze-gated animation clock used by mesh sampling;
/// remote walk-rate rebases must use this exact clock for clip-time continuity.
pub(crate) fn client_sample_interpolation(
    registry: &mut EntityRegistry,
    replication: &mut ClientReplication,
    time_sync: &ClientTimeSync,
    interpolation_delay: &mut InterpolationDelayState,
    frame_dt_secs: f64,
    frame_anim_time: f64,
) -> ClientPresentationInputs {
    // No estimate yet: retain the last-applied pose until the clock initializes,
    // while deriving walk playback from that held (zero-speed) presentation.
    let Some(estimated_tick) = time_sync.estimated_server_tick() else {
        replication.apply_held_remote_enemy_walk_playback_rates(registry, frame_anim_time);
        replication.apply_held_remote_player_presentation(registry, frame_anim_time);
        return replication.presented_player_inputs().clone();
    };
    // Jitter is available whenever the estimate is; default to 0 defensively.
    let jitter = time_sync.jitter_micros().unwrap_or(0.0);
    let render_server_tick =
        interpolation_delay.render_server_tick(estimated_tick, jitter, SERVER_TICK_MICROS);
    let stats = replication.sample_into_registry(registry, render_server_tick, frame_anim_time);
    if stats.presented > 0 {
        interpolation_delay.observe_sampled_frame(stats.starvation_feedback > 0, frame_dt_secs);
    }
    replication.presented_player_inputs().clone()
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

/// Advance the authoritative host tick and retain whether this redraw crossed a
/// snapshot-cadence edge. Catch-up frames call this once per completed fixed tick,
/// while the post-loop serializer consumes the accumulated `snapshot_due` bit once.
pub(crate) fn complete_host_fixed_tick(tick: &mut u32, snapshot_due: &mut bool) {
    *tick = tick.wrapping_add(1);
    *snapshot_due |= *tick % SNAPSHOT_TICK_INTERVAL == 0;
}

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
/// `tick` is the monotonic fixed-simulation tick stamp. `snapshot_due` records that
/// at least one completed tick reached the cadence during this redraw. A snapshot
/// is still encoded at most once for `tick`, even if a caller reaches this path
/// more than once before another fixed tick completes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn host_replicate(
    registry: &EntityRegistry,
    slot_table: &SlotTable,
    server: &mut NetServer,
    allocator: &mut NetworkIdAllocator,
    replication: &mut ServerReplication,
    state_slots: &mut state_slots::HostStateReplication,
    replicable: &ReplicableSet,
    owners: &MovementOwners,
    weapon_owners: &WeaponOwners,
    command_queues: &HostCommandQueues,
    host_aim: Option<(EntityId, f32)>,
    tick: u32,
    snapshot_due: bool,
    last_emitted_snapshot_tick: &mut Option<u32>,
) -> Vec<EntityId> {
    // Owned post-tick snapshot rule: copy replicable state into owned mirrors keyed
    // by NetworkId while borrowing the registry, then release before the net call.
    // Owned movement pawns also carry their owner id + resolved cursor (Phase 3).
    let owned = replication::produce_owned_snapshots_with_host_aim(
        registry,
        replicable,
        allocator,
        owners,
        command_queues,
        host_aim,
    );
    replication.ingest_tick(owned);

    // Catch-up may finish past a cadence edge (for example 1 -> 2 -> 3). The
    // completed-tick seam retains that edge for this post-loop send.
    if !snapshot_due {
        return Vec::new();
    }

    let participating = server.participating_clients();
    if participating.is_empty() {
        return Vec::new();
    }
    if *last_emitted_snapshot_tick == Some(tick) {
        return Vec::new();
    }
    *last_emitted_snapshot_tick = Some(tick);

    // The replicated-state schema fingerprint is stamped into every snapshot carrying
    // state records so the client gates on a match. Built once from the live slot table.
    let state_fingerprint = state_slots.fingerprint(slot_table);
    // Ingest this frame's authoritative source values ONCE before the per-client loop:
    // the scan is frame-wide (every replicated slot, every owned pawn), so running it
    // per client would repeat it O(clients) times. Each client's `produce_for_client`
    // below only reads the now-ingested per-client view.
    let sampled_weapons = state_slots.ingest_frame_and_collect_sampled_weapons(
        slot_table,
        registry,
        owners,
        weapon_owners,
    );
    // One sequence shared across all clients in this 30 Hz batch — and shared with the
    // state tracker's `produce_for_client` so one ack describes one server frame.
    let sequence = replication.begin_batch();
    for client_id in participating {
        // Registration also occurs on participation entry. Keep this idempotent
        // send-side guard so a participating client always has baseline state.
        replication.register_client(client_id);
        state_slots.register_client(client_id);
        if let Some(mut raw) = replication.encode_in_batch(client_id, tick, sequence) {
            // Splice this client's replicated-state records into the SAME snapshot
            // envelope the entity tracker produced (no new channel, no sibling message).
            // The entity tracker leaves `state_records` empty + an all-zero fingerprint;
            // overwrite both with the real fingerprint and the per-client records.
            raw.state_schema_fingerprint = state_fingerprint;
            if let Some(records) = state_slots.produce_for_client(client_id, sequence) {
                raw.state_records = records;
            }
            let bytes = wire::encode(&raw);
            let _ = server.send_snapshot(client_id, bytes);
        }
    }
    sampled_weapons
}

/// Spawn and register the slot-owned pawn on entry to participation. The engine
/// drives this from ordered `SlotEvent::Participating` edges, including
/// re-promotion. It uses the same allocator, replicable set, and slot map as exit
/// cleanup. Idempotent per slot (see [`on_slot_accepted`]).
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

/// Production participation seam for a movement session: spawn the descriptor-backed
/// remote `PlayerMovement` pawn for a participating client. Deterministically assigns the
/// slot a `player_spawn` placement (auditable, stable across reconnect), records the
/// owner mapping, then materializes the pawn through [`on_slot_accepted`]'s descriptor
/// path. Falls back to nothing (logged) if there are no spawn points or the descriptor
/// spawn fails — the caller keeps the slot for a later retry.
///
/// `spawn_points` are the level's `player_spawn` placements; `descriptors` the
/// registered entity descriptors; `agent_params` the navmesh capsule (or `None`).
/// Game-logic-owned: the spawn flows through `EntityRegistry::spawn`; the caller
/// threads in the mutable registry borrow. Returns the materialized pawn so the
/// caller can resolve presentation bindings against the level-installed tables.
#[allow(clippy::too_many_arguments)]
pub(crate) fn host_handle_accept_descriptor(
    registry: &mut EntityRegistry,
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    slot_pawns: &mut SlotPawns,
    command_queues: &mut HostCommandQueues,
    owners: &mut MovementOwners,
    weapon_owners: &mut WeaponOwners,
    open_shots: &mut OpenAuthorizedShots,
    pending_hit_declarations: &mut PendingHitDeclarations,
    weaponless_fire_logged: &mut std::collections::HashSet<EntityId>,
    client_id: u64,
    spawn_points: &[crate::scripting::map_entity::MapEntity],
    descriptors: &[EntityTypeDescriptor],
    agent_params: Option<NavAgentParams>,
) -> Option<EntityId> {
    cleanup_stale_slot_replacement(
        registry,
        allocator,
        replicable,
        slot_pawns,
        command_queues,
        owners,
        weapon_owners,
        open_shots,
        pending_hit_declarations,
        weaponless_fire_logged,
        client_id,
    );

    // Deterministic, auditable slot -> placement assignment recorded BEFORE the spawn.
    let Some(idx) = slot_pawns.assign_placement(client_id, spawn_points.len()) else {
        log::warn!(
            "[Net] slot {client_id} accepted but the map has no player_spawn placements; no pawn spawned"
        );
        return None;
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
        let entity_class = placement
            .key_values
            .get("entity_class")
            .map(String::as_str)
            .unwrap_or("player");
        remote_materialize::apply_remote_player_viewer_role(
            entity_class,
            descriptors,
            registry,
            pawn,
        );
        // Record the owner mapping (pawn -> client_id) so snapshot production can stamp
        // `owner_client_id` and the resolved cursor. The client's command queue is
        // created lazily on its first ingested command.
        owners.set(pawn, client_id);
        weapon_owners.mark_attachment_dirty(pawn);
        let _ = command_queues;
        return Some(pawn);
    }
    None
}

/// Register the listen host's OWN player pawn for OUTBOUND replication (M15 Phase 3,
/// issue 3b): without this, the host pawn never enters the `ReplicableSet`, so
/// `produce_owned_snapshots` never emits it and clients see no host capsule.
///
/// This is replication/presentation bookkeeping only. The host pawn keeps being driven LOCALLY by
/// `simulate_tick`/`local_movement_pawn` — it is deliberately NOT recorded in
/// `MovementOwners`, NOT command-queued, and NOT predicted/reconciled. Because it has
/// no `owner_client_id`, its per-recipient `local_player` flag is false for every
/// client (clients interpolate it as a normal remote pawn).
///
/// Idempotent and reload-safe: registering the same pawn twice is a no-op (the set and
/// the allocator are both stable per `EntityId`). On a level reload the freshly-spawned
/// pawn is a distinct `EntityId`, so the previously-tracked host pawn (if any) is
/// unregistered and marked for attachment removal first. The new pawn is marked for
/// inventory-derived attachment resolution here so ownership and presentation change
/// atomically.
///
/// Game-logic-owned: it reads the registry through the borrow the caller threads in and
/// only touches host bookkeeping; it never reaches into `App`.
pub(crate) fn host_register_own_pawn(
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    host_pawn: &mut Option<EntityId>,
    weapon_owners: &mut WeaponOwners,
    pawn: EntityId,
) {
    // A level reload spawns a fresh host pawn (distinct EntityId). Drop the stale
    // registration before registering the new one so the replicable set never names a
    // despawned id. Re-registering the SAME pawn skips the churn (idempotent install).
    if let Some(previous) = *host_pawn {
        if previous != pawn {
            replicable.unregister(previous);
            allocator.forget(previous);
            weapon_owners.remove_pawn(previous);
        }
    }
    // Stamp the stable session-monotonic NetworkId and register for replication,
    // mirroring `on_slot_accepted` — but with NO owner mapping, so the host pawn is
    // replicated as an unowned (never-local) remote pawn to every client.
    let net_id = allocator.stamp(pawn);
    replicable.register(pawn);
    *host_pawn = Some(pawn);
    weapon_owners.mark_attachment_dirty(pawn);
    log::info!("[Net] host registered own pawn {pawn:?} as {net_id:?} (outbound replication only)");
}

/// Remove the listen host's prior local pawn from replication and weapon ownership.
/// Level install calls this when the replacement map has no player spawn, so stale
/// ownership cannot survive merely because there is no new pawn to register.
pub(crate) fn host_unregister_own_pawn(
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    host_pawn: &mut Option<EntityId>,
    weapon_owners: &mut WeaponOwners,
) -> Option<EntityId> {
    let previous = host_pawn.take()?;
    replicable.unregister(previous);
    allocator.forget(previous);
    weapon_owners.remove_pawn(previous);
    Some(previous)
}

/// Apply participation exits to the host's remote-pawn state. Close and demotion
/// share this cleanup: despawn the slot pawn, drop it from replication, and clear
/// ownership, command, state-slot, tuning, and combat bookkeeping.
///
/// Game-logic-owned: the registry mutation flows through `EntityRegistry::despawn`.
/// The mutable registry borrow is threaded in by the caller so this module never
/// reaches into `App`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn host_handle_lifecycle(
    registry: &mut EntityRegistry,
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    replication: &mut ServerReplication,
    state_slots: &mut state_slots::HostStateReplication,
    slot_pawns: &mut SlotPawns,
    command_queues: &mut HostCommandQueues,
    owners: &mut MovementOwners,
    weapon_owners: &mut WeaponOwners,
    open_shots: &mut OpenAuthorizedShots,
    pending_hit_declarations: &mut PendingHitDeclarations,
    weaponless_fire_logged: &mut std::collections::HashSet<EntityId>,
    last_sent_tuning: &mut HashMap<u64, TuningPayload>,
    lifecycle: &[postretro_net::slots::SlotEvent],
) {
    use postretro_net::slots::SlotEvent;
    for event in lifecycle {
        match event {
            SlotEvent::Closed { client_id, .. } | SlotEvent::Demoted { client_id, .. } => {
                let previous_pawn = slot_pawns.pawn_for(*client_id);
                if let Some(pawn) = previous_pawn {
                    pending_hit_declarations.remove_pawn_shots(allocator, pawn);
                }
                let despawned = on_slot_closed(
                    registry,
                    slot_pawns,
                    allocator,
                    replicable,
                    replication,
                    *client_id,
                );
                // M15 Phase 3: drop the closed client's command queue and the pawn's
                // owner mapping so its stale authority metadata never rides a later
                // snapshot. The slot's placement assignment is intentionally retained
                // (a reconnecting client lands on its prior spawn — auditable source).
                command_queues.remove_client(*client_id);
                // M15 Phase 3.5: drop the closed client's replicated-state baselines and
                // its owner-private slot values so none leak past the connection.
                state_slots.remove_client(*client_id);
                last_sent_tuning.remove(client_id);
                if let Some(pawn) = despawned {
                    cleanup_remote_pawn_owned_state(
                        registry,
                        allocator,
                        replicable,
                        owners,
                        weapon_owners,
                        open_shots,
                        pending_hit_declarations,
                        weaponless_fire_logged,
                        *client_id,
                        pawn,
                        &[],
                    );
                } else if let Some(pawn) = previous_pawn {
                    cleanup_remote_pawn_owned_state(
                        registry,
                        allocator,
                        replicable,
                        owners,
                        weapon_owners,
                        open_shots,
                        pending_hit_declarations,
                        weaponless_fire_logged,
                        *client_id,
                        pawn,
                        &[],
                    );
                }
            }
            // Entry is handled by the App's participation seam, which registers
            // replication and spawns the pawn. Kept exhaustive so a new slot event
            // is a compile error here as well.
            SlotEvent::Participating { .. } => {}
        }
    }
}

/// Resolve the host-authoritative prediction values for one spawned slot pawn.
/// The wieldable rows come from its live inventory, not the authored loadout: a
/// descriptor refresh retunes existing instances, while loadout changes wait for
/// the next pawn install. The net crate carries the resulting JSON bytes without
/// learning this vocabulary.
pub(crate) fn tuning_payload_for_pawn(
    registry: &EntityRegistry,
    pawn: EntityId,
    descriptors: &[EntityTypeDescriptor],
) -> TuningPayload {
    let class = registry
        .get_component::<DescriptorProvenance>(pawn)
        .ok()
        .map(|provenance| provenance.canonical_name.as_str())
        .unwrap_or("player");
    let descriptor = descriptors
        .iter()
        .find(|descriptor| descriptor.canonical_name.as_deref() == Some(class));
    let movement = descriptor.and_then(|descriptor| descriptor.movement.clone());
    let wieldables: [Option<WieldableTuningPayload>; WIELDABLE_SLOT_CAPACITY] = registry
        .get_component::<Inventory>(pawn)
        .ok()
        .map(|inventory| {
            std::array::from_fn(|slot| {
                let weapon_id = inventory.wieldables[slot]?;
                let canonical_name = registry
                    .get_component::<DescriptorProvenance>(weapon_id)
                    .ok()?
                    .canonical_name
                    .clone();
                let weapon = registry.get_component::<WeaponComponent>(weapon_id).ok()?;
                Some(WieldableTuningPayload {
                    canonical_name,
                    range: weapon.range,
                    cooldown_ms: weapon.cooldown_ms,
                    fire_mode: weapon.fire_mode,
                    resolution: weapon.resolution,
                    lower_ms: weapon.lower_ms,
                    raise_ms: weapon.raise_ms,
                })
            })
        })
        .unwrap_or_else(|| std::array::from_fn(|_| None));
    TuningPayload::new(movement, wieldables)
}

pub(crate) fn host_send_tuning_if_changed(
    server: &mut NetServer,
    last_sent_tuning: &mut HashMap<u64, TuningPayload>,
    client_id: u64,
    payload: TuningPayload,
) {
    if last_sent_tuning.get(&client_id) == Some(&payload) {
        return;
    }
    server.send_control(
        client_id,
        wire::encode(&ServerControlMessage::Tuning(
            tuning_payload::encode_tuning_payload(&payload),
        )),
    );
    last_sent_tuning.insert(client_id, payload);
}

#[allow(clippy::too_many_arguments)]
fn cleanup_stale_slot_replacement(
    registry: &mut EntityRegistry,
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    slot_pawns: &mut SlotPawns,
    command_queues: &mut HostCommandQueues,
    owners: &mut MovementOwners,
    weapon_owners: &mut WeaponOwners,
    open_shots: &mut OpenAuthorizedShots,
    pending_hit_declarations: &mut PendingHitDeclarations,
    weaponless_fire_logged: &mut std::collections::HashSet<EntityId>,
    client_id: u64,
) {
    let Some(pawn) = slot_pawns.pawn_for(client_id) else {
        return;
    };
    if registry.exists(pawn) {
        return;
    }

    let Some((_removed_pawn, wieldables)) = slot_pawns.remove_client(client_id) else {
        return;
    };
    command_queues.remove_client(client_id);
    cleanup_remote_pawn_owned_state(
        registry,
        allocator,
        replicable,
        owners,
        weapon_owners,
        open_shots,
        pending_hit_declarations,
        weaponless_fire_logged,
        client_id,
        pawn,
        &wieldables,
    );
}

#[allow(clippy::too_many_arguments)]
fn cleanup_remote_pawn_owned_state(
    registry: &mut EntityRegistry,
    allocator: &mut NetworkIdAllocator,
    replicable: &mut ReplicableSet,
    owners: &mut MovementOwners,
    weapon_owners: &mut WeaponOwners,
    open_shots: &mut OpenAuthorizedShots,
    pending_hit_declarations: &mut PendingHitDeclarations,
    weaponless_fire_logged: &mut std::collections::HashSet<EntityId>,
    client_id: u64,
    pawn: EntityId,
    wieldables: &[EntityId],
) {
    // Pawn and sibling weapon are one runtime ownership unit. Despawning both
    // prevents a non-cancellable reload from surviving without the reserve-owning
    // pawn that must finish its transfer.
    pending_hit_declarations.remove_client(client_id);
    open_shots.remove_client(client_id);
    open_shots.remove_pawn(pawn);
    weaponless_fire_logged.remove(&pawn);
    owners.remove_pawn(pawn);
    weapon_owners.remove_pawn(pawn);
    replicable.unregister(pawn);
    allocator.forget(pawn);
    for &wieldable in wieldables {
        replicable.unregister(wieldable);
        allocator.forget(wieldable);
        let _ = registry.despawn(wieldable);
    }
    let _ = registry.despawn(pawn);
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn host_handle_client_messages(
    server: &mut NetServer,
    replication: &mut ServerReplication,
    state_slots: &mut state_slots::HostStateReplication,
    command_queues: &mut HostCommandQueues,
    pending_hit_declarations: &mut PendingHitDeclarations,
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
        host_handle_client_message_inner(
            server,
            replication,
            state_slots,
            command_queues,
            Some(&mut *pending_hit_declarations),
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
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub(crate) fn host_handle_client_message(
    server: &mut NetServer,
    replication: &mut ServerReplication,
    state_slots: &mut state_slots::HostStateReplication,
    command_queues: &mut HostCommandQueues,
    client_id: u64,
    server_tick: u32,
    server_now_us: u64,
    msg: wire::ClientMessage,
) {
    host_handle_client_message_inner(
        server,
        replication,
        state_slots,
        command_queues,
        None,
        client_id,
        server_tick,
        server_now_us,
        msg,
    );
}

struct HostHitIngestContext<'a> {
    registry: &'a mut EntityRegistry,
    collision_world: &'a CollisionWorld,
    allocator: &'a NetworkIdAllocator,
    owners: &'a MovementOwners,
    open_shots: &'a mut OpenAuthorizedShots,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct HitDeclarationResult {
    fire_accepted: bool,
    hit_accepted: bool,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn host_flush_pending_hit_declarations(
    server: &mut NetServer,
    registry: &mut EntityRegistry,
    collision_world: &CollisionWorld,
    allocator: &NetworkIdAllocator,
    owners: &MovementOwners,
    command_queues: &HostCommandQueues,
    open_shots: &mut OpenAuthorizedShots,
    pending_hit_declarations: &mut PendingHitDeclarations,
    current_tick: u32,
    mut on_impact: impl FnMut(&mut EntityRegistry),
) -> bool {
    open_shots.prune_stale(current_tick);
    let mut accepted_any_hit = false;
    for pending in pending_hit_declarations.drain_ready(command_queues, open_shots) {
        let result = ingest_hit_declaration(
            HostHitIngestContext {
                registry: &mut *registry,
                collision_world,
                allocator,
                owners,
                open_shots: &mut *open_shots,
            },
            pending.client_id,
            &pending.declaration,
            &mut on_impact,
        );
        send_shot_verdict(
            server,
            pending.client_id,
            pending.declaration.shot_id,
            result.fire_accepted,
            result.hit_accepted,
        );
        accepted_any_hit |= result.hit_accepted;
    }
    accepted_any_hit
}

#[allow(clippy::too_many_arguments)]
fn host_handle_client_message_inner(
    server: &mut NetServer,
    replication: &mut ServerReplication,
    state_slots: &mut state_slots::HostStateReplication,
    command_queues: &mut HostCommandQueues,
    pending_hit_declarations: Option<&mut PendingHitDeclarations>,
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
            // The same ack advances replicated-state baselines (M15 Phase 3.5), keyed
            // by `StateSlotId` rather than `NetworkId`. One ack, one server frame.
            state_slots.apply_ack(client_id, ack.latest_snapshot_sequence, &ack.slot_baselines);
        }
        wire::ClientMessage::BaselineRefresh(req) => {
            replication.request_refresh(client_id, req.network_id, req.missing_baseline_ref);
        }
        // Echo the time-sync probe with the server tick sampled now. The echo
        // rides Channel::Input back; the client measures RTT from its own
        // send/receive times and folds the server tick into its estimate.
        wire::ClientMessage::TimeSync(req) => {
            let echo = req.echo(server_tick, server_now_us);
            server.send_input(
                client_id,
                wire::encode(&wire::ServerMessage::TimeSync(echo)),
            );
        }
        // M15 Phase 3 Task 4: sanitize + queue the input command for this client.
        // `ingest` rejects non-finite commands, drops stale/duplicate ones, and never
        // mutates any other client's queue. The movement stage resolves them per tick.
        wire::ClientMessage::Input(input) => {
            command_queues.ingest(client_id, &input);
        }
        wire::ClientMessage::HitDeclaration(declaration) => {
            if let Some(pending) = pending_hit_declarations {
                pending.push(client_id, declaration);
            }
        }
        // M15 Phase 3.5: a client missing a replicated state-slot baseline. The state
        // tracker schedules a full baseline for that slot in the client's next snapshot.
        // Keyed by `StateSlotId` (distinct from the entity `BaselineRefresh`).
        wire::ClientMessage::StateBaselineRefresh(req) => {
            state_slots.request_refresh(client_id, req.slot_id, req.missing_baseline_ref);
        }
    }
}

fn send_shot_verdict(
    server: &mut NetServer,
    client_id: u64,
    shot_id: u64,
    accept: bool,
    hit_accepted: bool,
) {
    server.send_input(
        client_id,
        wire::encode(&wire::ServerMessage::ShotVerdicts(
            wire::ShotVerdictsMessage {
                verdicts: vec![wire::ShotVerdict {
                    shot_id,
                    accept,
                    hit_accepted,
                }],
            },
        )),
    );
}

fn ingest_hit_declaration(
    context: HostHitIngestContext<'_>,
    client_id: u64,
    declaration: &wire::HitDeclaration,
    mut on_impact: impl FnMut(&mut EntityRegistry),
) -> HitDeclarationResult {
    let shot_id = ShotId::from_raw(declaration.shot_id);
    let Some(open) = context.open_shots.get(shot_id) else {
        return HitDeclarationResult::default();
    };
    if open.owner_client_id != client_id
        || context.owners.owner_of(open.shot.pawn) != Some(client_id)
    {
        return HitDeclarationResult::default();
    }

    // Once the declaration binds to this client's still-open authorized FIRE,
    // consume it even if every record below fails validation. That keeps a client
    // from retrying the same authorized shot until a declaration happens to land.
    // The peek above already confirmed the entry; nothing mutates the store between,
    // so retire for its removal side effect and reuse the peeked shot.
    context.open_shots.retire(shot_id);
    let pellet_count = open.shot.pellet_count;
    if pellet_count == 0 {
        return HitDeclarationResult {
            fire_accepted: true,
            hit_accepted: false,
        };
    }

    let mut hit_accepted = false;
    for record in declaration.records.iter().take(pellet_count) {
        let accepted = apply_valid_hit_record(
            context.registry,
            context.collision_world,
            context.allocator,
            open.shot.pawn,
            open.shot.weapon,
            open.shot.damage,
            open.shot.range,
            open.shot.credit_source.clone(),
            record,
        );
        if accepted {
            // One remote pellet is one impact fire. Its policy effects must
            // settle before validation and damage for the next record observe
            // the target again.
            on_impact(context.registry);
            hit_accepted = true;
        }
    }

    HitDeclarationResult {
        fire_accepted: true,
        hit_accepted,
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_valid_hit_record(
    registry: &mut EntityRegistry,
    collision_world: &CollisionWorld,
    allocator: &NetworkIdAllocator,
    attacker: EntityId,
    weapon_id: EntityId,
    damage: f32,
    range: f32,
    credit_source: String,
    record: &wire::HitRecord,
) -> bool {
    let Some(target) = allocator.entity_for_network_id(NetworkId(record.target)) else {
        return false;
    };
    if registry.get_component::<HealthComponent>(target).is_err() {
        return false;
    }
    let point = Vec3::from_array(record.point);
    if !point.is_finite() {
        return false;
    }
    // Reconstruct the attacker eye once and reuse it for both the static-world LOS
    // check and the range check below.
    let Some(eye) = attacker_eye(registry, attacker) else {
        return false;
    };
    if !has_static_world_los(collision_world, eye, point) {
        return false;
    }
    let distance = eye.distance(point);
    let max_range = range * HIT_RANGE_TOLERANCE;
    if !max_range.is_finite() || distance > max_range {
        return false;
    }

    let impact = WeaponImpact {
        point,
        normal: Vec3::ZERO,
        target: Some(target),
        zone: record.zone.clone(),
        outcome: ActivationOutcome::Hit(weapon::DamagePayload { amount: damage }),
    };
    crate::sim::apply_authorized_weapon_impact_damage(
        registry,
        weapon_id,
        Some(attacker),
        &impact,
        credit_source,
        damage,
    );
    true
}

fn has_static_world_los(collision_world: &CollisionWorld, eye: Vec3, point: Vec3) -> bool {
    let to_point = point - eye;
    let distance = to_point.length();
    if !distance.is_finite() || distance <= 1.0e-5 {
        return false;
    }
    let dir = to_point / distance;
    let hit = collision::cast_ray(
        collision_world,
        Point::new(eye.x, eye.y, eye.z),
        Vector::new(dir.x, dir.y, dir.z),
        distance,
    );
    !matches!(hit, Some(hit) if hit.time_of_impact < distance - 1.0e-4)
}

fn attacker_eye(registry: &EntityRegistry, attacker: EntityId) -> Option<Vec3> {
    let transform = registry.get_component::<Transform>(attacker).ok()?;
    let movement = registry
        .get_component::<PlayerMovementComponent>(attacker)
        .ok()?;
    Some(transform.position + Vec3::new(0.0, movement.capsule.eye_height, 0.0))
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
) -> Vec<wire::ShotVerdict> {
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
        return Vec::new();
    }
    let recv_us = time_sync.clock.now_micros();
    let mut verdicts = Vec::new();
    for bytes in echoes {
        match wire::decode::<wire::ServerMessage>(&bytes) {
            Ok(wire::ServerMessage::TimeSync(echo)) => {
                time_sync.estimator.ingest_echo(&echo, recv_us);
            }
            Ok(wire::ServerMessage::ShotVerdicts(message)) => {
                verdicts.extend(message.verdicts);
            }
            Err(err) => {
                log::warn!("[Net] dropping undecodable server input message: {err}");
            }
        }
    }
    verdicts
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

/// Collect world-space positions for meshless replicated-entity debug wireframes.
/// On the CLIENT, checks each non-local `NetworkId -> EntityId` mapping. On the HOST,
/// checks the authoritative `ReplicableSet`. Entities with a `MeshComponent` already
/// have their presentation, so capsules remain only a fallback for meshless or
/// unresolved replicated entities. Empty for single-player.
///
/// Read-only: borrows the registry immutably and never touches wgpu — the
/// caller hands these positions to the renderer, which owns the capsule draw
/// (Renderer-owns-GPU). The returned position is the capsule center, matching
/// the pawn `Transform.position` convention (the collision capsule is symmetric
/// about it; see `movement/substrate.rs`).
///
/// The overlay can cover AI enemies, pawns, movers, and other networked gameplay
/// objects. It uses standing-player dimensions only for its meshless fallback, so
/// an unresolved non-player entity gets an approximate marker rather than no aid.
///
/// `dev-tools`-gated: the sole consumer is the host/client debug-capsule draw behind
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
                (!registry
                    .has_component_kind(id, ComponentKind::Mesh)
                    .unwrap_or(false))
                .then(|| {
                    registry
                        .get_component::<Transform>(id)
                        .ok()
                        .map(|t| t.position)
                })
                .flatten()
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
                (!registry
                    .has_component_kind(id, ComponentKind::Mesh)
                    .unwrap_or(false))
                .then(|| {
                    registry
                        .get_component::<Transform>(id)
                        .ok()
                        .map(|t| t.position)
                })
                .flatten()
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parry3d::math::{Isometry, Point};
    use parry3d::shape::TriMesh;
    use postretro_entities::components::mesh::MeshAttachment;
    use postretro_entities::components::weapon::{ReloadFeedback, WeaponComponent};
    use postretro_entities::provenance::{DescriptorComponentKind, DescriptorSpawnPath};
    use postretro_foundation::{FireMode, ResolutionMode, WeaponDescriptor};

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

    fn sample_wire_transform() -> WireTransform {
        WireTransform {
            position: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        }
    }

    fn install_test_tuning(tuning: &mut Option<TuningPayload>, generation: &mut u64) -> Vec<u8> {
        let mut wieldables = std::array::from_fn(|_| None);
        wieldables[0] = Some(WieldableTuningPayload {
            canonical_name: "reference_pistol".to_string(),
            range: 12.0,
            cooldown_ms: 90.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            lower_ms: 25,
            raise_ms: 35,
        });
        let encoded = tuning_payload::encode_tuning_payload(&TuningPayload::new(
            host_player_descriptor().movement,
            wieldables,
        ));
        replace_client_tuning(tuning, generation, &encoded).expect("valid tuning installs");
        encoded
    }

    #[test]
    fn tuning_payload_reads_each_live_inventory_slot_not_the_authored_loadout() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon_id = registry.spawn(Transform::default());
        let mut weapon = test_weapon(10.0, 96.0);
        weapon.cooldown_ms = 180.0;
        weapon.fire_mode = FireMode::Auto;
        weapon.lower_ms = 45;
        weapon.raise_ms = 70;
        registry.set_component(weapon_id, weapon).unwrap();
        registry
            .set_component(
                weapon_id,
                DescriptorProvenance {
                    canonical_name: "live_ion_rifle".to_string(),
                    owned_components: Default::default(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::DefaultWeapon,
                },
            )
            .unwrap();
        let mut inventory = Inventory::default();
        inventory.wieldables[3] = Some(weapon_id);
        inventory.active_slot = 3;
        registry.set_component(pawn, inventory).unwrap();

        let payload = tuning_payload_for_pawn(&registry, pawn, &[]);

        assert!(payload.movement.is_none());
        assert!(payload.wieldables[0].is_none());
        let slot = payload.wieldables[3].as_ref().unwrap();
        assert_eq!(slot.canonical_name, "live_ion_rifle");
        assert_eq!(slot.range, 96.0);
        assert_eq!(slot.cooldown_ms, 180.0);
        assert_eq!(slot.fire_mode, FireMode::Auto);
        assert_eq!(slot.lower_ms, 45);
        assert_eq!(slot.raise_ms, 70);
    }

    // Regression: a malformed replacement retained the previously accepted
    // payload and generation, leaving descriptor-derived prediction live.
    #[test]
    fn malformed_tuning_replacement_invalidates_previous_install() {
        let mut tuning = None;
        let mut generation = 0;
        install_test_tuning(&mut tuning, &mut generation);
        let accepted_generation = generation;

        let result = replace_client_tuning(&mut tuning, &mut generation, b"not json");

        assert!(matches!(
            result,
            Err(tuning_payload::TuningPayloadError::Malformed { .. })
        ));
        assert!(tuning.is_none());
        assert_ne!(generation, accepted_generation);
    }

    // Regression: an unknown-epoch replacement retained the previously
    // accepted payload and descriptor-derived prediction state.
    #[test]
    fn unknown_epoch_tuning_replacement_invalidates_previous_install() {
        let mut tuning = None;
        let mut generation = 0;
        let encoded = install_test_tuning(&mut tuning, &mut generation);
        let accepted_generation = generation;
        let unknown_epoch = String::from_utf8(encoded).unwrap().replacen(
            &format!("\"epoch\":{}", tuning_payload::TUNING_PAYLOAD_EPOCH),
            &format!("\"epoch\":{}", tuning_payload::TUNING_PAYLOAD_EPOCH + 1),
            1,
        );

        let result = replace_client_tuning(&mut tuning, &mut generation, unknown_epoch.as_bytes());

        assert!(matches!(
            result,
            Err(tuning_payload::TuningPayloadError::EpochMismatch { .. })
        ));
        assert!(tuning.is_none());
        assert_ne!(generation, accepted_generation);
    }

    // Regression: Control tuning arriving after the local-player baseline was
    // stored but never rebuilt movement until another arming snapshot arrived.
    #[test]
    fn late_and_idle_retune_rebuild_armed_movement_and_keep_local_view_feel() {
        let mut local_descriptor = host_player_descriptor();
        local_descriptor.movement.as_mut().unwrap().view_feel = Some(ViewFeelParams {
            bob: None,
            tilt: None,
            sway: None,
        });
        let descriptors = [local_descriptor.clone()];
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let armed = client::ArmedLocalPawn {
            network_id: NetworkId(9),
            entity_id: pawn,
            entity_class: Some("player".to_string()),
        };
        let mut applied_generation = 0;

        let mut host_movement = local_descriptor.movement.unwrap();
        host_movement.ground.speed.run = 29.0;
        host_movement.view_feel = None;
        let first = TuningPayload::new(Some(host_movement.clone()), std::array::from_fn(|_| None));
        apply_installed_movement_tuning_to_armed_pawn(
            &armed,
            Some(&first),
            1,
            &mut applied_generation,
            &descriptors,
            &mut registry,
        );

        let installed = registry
            .get_component::<PlayerMovementComponent>(pawn)
            .expect("late tuning materializes movement immediately");
        assert_eq!(installed.ground_params.speed.run, 29.0);
        assert_eq!(
            installed.view_feel,
            descriptors[0].movement.as_ref().unwrap().view_feel,
            "host tuning cannot overwrite local presentation feel"
        );
        assert_eq!(applied_generation, 1);

        host_movement.ground.speed.run = 37.0;
        let retuned = TuningPayload::new(Some(host_movement), std::array::from_fn(|_| None));
        apply_installed_movement_tuning_to_armed_pawn(
            &armed,
            Some(&retuned),
            2,
            &mut applied_generation,
            &descriptors,
            &mut registry,
        );
        assert_eq!(
            registry
                .get_component::<PlayerMovementComponent>(pawn)
                .unwrap()
                .ground_params
                .speed
                .run,
            37.0,
            "idle staged retune rebuilds without waiting for another snapshot"
        );
        assert_eq!(applied_generation, 2);
    }

    fn snapshot_with_record(record: EntityRecord) -> SnapshotMessage {
        SnapshotMessage {
            sequence: 1,
            server_tick: 1,
            records: vec![record],
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
        }
    }

    fn test_weapon(damage: f32, range: f32) -> WeaponComponent {
        WeaponComponent::from_descriptor(&WeaponDescriptor {
            damage,
            range,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            credit_source: Some("weapon.test.net".to_string()),
            third_person_model: None,
            viewmodel: None,
            resource: None,
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
        })
    }

    fn set_active_inventory(registry: &mut EntityRegistry, pawn: EntityId, weapon: EntityId) {
        let mut inventory = postretro_entities::components::inventory::Inventory::default();
        inventory.wieldables[0] = Some(weapon);
        registry.set_component(pawn, inventory).unwrap();
    }

    #[test]
    fn world_less_poll_advances_both_roles_without_touching_registry_state() {
        use std::net::{Ipv4Addr, SocketAddr};

        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform {
            position: Vec3::new(3.0, 2.0, 1.0),
            ..Transform::default()
        });

        let mut host = NetEndpoint::from_role(&NetRole::Host { port: 0 })
            .expect("host endpoint constructs")
            .expect("host role yields an endpoint");
        assert!(matches!(
            host.poll_world_less(Duration::from_millis(16), &mut registry),
            WorldLessPoll::Host(_)
        ));

        let mut client = NetEndpoint::from_role(&NetRole::Connect {
            addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 1)),
        })
        .expect("client endpoint constructs")
        .expect("connect role yields an endpoint");
        assert!(matches!(
            client.poll_world_less(Duration::from_millis(16), &mut registry),
            WorldLessPoll::Client(_)
        ));

        assert_eq!(
            registry
                .get_component::<Transform>(entity)
                .expect("world-less transport must not mutate the registry")
                .position,
            Vec3::new(3.0, 2.0, 1.0),
            "world-less transport does not apply snapshots or simulate"
        );
    }

    // Regression: world-less polling retained a current-epoch snapshot and
    // applied its old-world bytes after the replacement level installed.
    #[test]
    fn world_less_snapshot_drain_discards_current_epoch_bytes() {
        const CLIENT_ID: u64 = 71;
        let server_socket = std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .expect("bind relay server");
        let server_addr = server_socket.local_addr().expect("relay server address");
        let mut server =
            NetServer::new(server_socket, server_addr, 2, Duration::from_secs(1), None)
                .expect("construct relay server");
        let client_socket = std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .expect("bind relay client");
        let mut client = NetClient::new(
            client_socket,
            server_addr,
            CLIENT_ID,
            Duration::from_secs(1),
            None,
        )
        .expect("construct relay client");
        server.add_relay_connection(CLIENT_ID);
        client.set_connected();
        server.set_mod_identity("postretro.test".to_string(), "1".to_string());
        server.set_mod_digest(Some([7; 32]));
        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        client.set_mod_identity("postretro.test".to_string(), "1".to_string());
        client.set_mod_digest(Some([7; 32]));
        client.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        client.update_connections(Duration::from_millis(16));
        for packet in client.packets_to_send() {
            server.process_packet_from(&packet, CLIENT_ID);
        }
        let poll = server.poll_handshakes();
        assert!(matches!(
            poll.lifecycle.as_slice(),
            [postretro_net::slots::SlotEvent::Participating { .. }]
        ));
        for packet in server.packets_to_send(CLIENT_ID) {
            client.process_packet(&packet);
        }
        assert!(client.drain_control().is_empty());

        assert!(server.send_snapshot(CLIENT_ID, vec![1, 2, 3]));
        for packet in server.packets_to_send(CLIENT_ID) {
            client.process_packet(&packet);
        }
        discard_world_less_snapshots(&mut client);
        assert!(
            client.drain_snapshots().is_empty(),
            "no old-world snapshot survives the world-less frame"
        );
    }

    // Regression: a 1 -> 2 -> 3 catch-up redraw skipped the qualifying tick-2
    // snapshot because cadence was checked only once against the final tick.
    #[test]
    fn host_catch_up_preserves_snapshot_cadence_and_zero_tick_ack_semantics() {
        const CLIENT_ID: u64 = 72;
        let server_socket = std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .expect("bind relay server");
        let server_addr = server_socket.local_addr().expect("relay server address");
        let mut server =
            NetServer::new(server_socket, server_addr, 2, Duration::from_secs(1), None)
                .expect("construct relay server");
        let client_socket = std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .expect("bind relay client");
        let mut client = NetClient::new(
            client_socket,
            server_addr,
            CLIENT_ID,
            Duration::from_secs(1),
            None,
        )
        .expect("construct relay client");
        server.add_relay_connection(CLIENT_ID);
        client.set_connected();
        server.set_mod_identity("postretro.test".to_string(), "1".to_string());
        server.set_mod_digest(Some([7; 32]));
        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        client.set_mod_identity("postretro.test".to_string(), "1".to_string());
        client.set_mod_digest(Some([7; 32]));
        client.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        client.update_connections(Duration::from_millis(16));
        for packet in client.packets_to_send() {
            server.process_packet_from(&packet, CLIENT_ID);
        }
        let poll = server.poll_handshakes();
        assert!(matches!(
            poll.lifecycle.as_slice(),
            [postretro_net::slots::SlotEvent::Participating { .. }]
        ));
        for packet in server.packets_to_send(CLIENT_ID) {
            client.process_packet(&packet);
        }
        assert!(client.drain_control().is_empty());

        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon_id = registry.spawn(Transform::default());
        let mut weapon = test_weapon(10.0, 100.0);
        let start_tick = weapon.begin_reload_feedback_tick();
        weapon.publish_reload_feedback(ReloadFeedback::Started, start_tick);
        let completed_tick = weapon.begin_reload_feedback_tick();
        weapon.publish_reload_feedback(ReloadFeedback::Completed, completed_tick);
        registry.set_component(weapon_id, weapon).unwrap();

        let mut allocator = NetworkIdAllocator::new();
        allocator.stamp(pawn);
        let mut replicable = ReplicableSet::new();
        replicable.register(pawn);
        let mut owners = MovementOwners::new();
        owners.set(pawn, CLIENT_ID);
        let weapon_owners = WeaponOwners::new();
        set_active_inventory(&mut registry, pawn, weapon_id);
        let command_queues = HostCommandQueues::new();
        let slot_table = SlotTable::new();
        let mut replication = ServerReplication::new();
        let mut state_slots = state_slots::HostStateReplication::new();
        let mut last_emitted_snapshot_tick = None;
        let mut timing = crate::frame_timing::FrameTiming::new(
            crate::frame_timing::InterpolableState::new(Vec3::ZERO),
        );
        let catch_up = timing.accumulate(crate::frame_timing::TICK_DURATION * 3);
        assert_eq!(catch_up.ticks, 3);
        let mut tick = 0;
        let mut snapshot_due = false;
        for _ in 0..catch_up.ticks {
            complete_host_fixed_tick(&mut tick, &mut snapshot_due);
        }
        assert_eq!(tick, 3);
        assert!(
            snapshot_due,
            "the completed-tick wiring must retain the tick-2 cadence edge"
        );

        let sampled = host_replicate(
            &registry,
            &slot_table,
            &mut server,
            &mut allocator,
            &mut replication,
            &mut state_slots,
            &replicable,
            &owners,
            &weapon_owners,
            &command_queues,
            None,
            tick,
            snapshot_due,
            &mut last_emitted_snapshot_tick,
        );
        assert_eq!(sampled, vec![weapon_id]);
        crate::sim::clear_owner_reload_feedback_for_weapons(&mut registry, &sampled);
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .owner_reload_status(),
            (1.0, true)
        );

        let zero_tick_redraw = timing.accumulate(Duration::ZERO);
        assert_eq!(zero_tick_redraw.ticks, 0);
        let mut snapshot_due = false;
        for _ in 0..zero_tick_redraw.ticks {
            complete_host_fixed_tick(&mut tick, &mut snapshot_due);
        }
        let sampled = host_replicate(
            &registry,
            &slot_table,
            &mut server,
            &mut allocator,
            &mut replication,
            &mut state_slots,
            &replicable,
            &owners,
            &weapon_owners,
            &command_queues,
            None,
            tick,
            snapshot_due,
            &mut last_emitted_snapshot_tick,
        );
        assert!(sampled.is_empty());
        crate::sim::clear_owner_reload_feedback_for_weapons(&mut registry, &sampled);
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .owner_reload_status(),
            (1.0, true),
            "the second redraw must not acknowledge the completed endpoint"
        );
        for packet in server.packets_to_send(CLIENT_ID) {
            client.process_packet(&packet);
        }
        assert_eq!(
            client.drain_snapshots().len(),
            1,
            "one catch-up redraw emits one current snapshot batch"
        );

        let next_tick = timing.accumulate(crate::frame_timing::TICK_DURATION);
        assert_eq!(next_tick.ticks, 1);
        let mut snapshot_due = false;
        for _ in 0..next_tick.ticks {
            complete_host_fixed_tick(&mut tick, &mut snapshot_due);
        }
        assert_eq!(tick, 4);
        assert!(snapshot_due);
        let sampled = host_replicate(
            &registry,
            &slot_table,
            &mut server,
            &mut allocator,
            &mut replication,
            &mut state_slots,
            &replicable,
            &owners,
            &weapon_owners,
            &command_queues,
            None,
            tick,
            snapshot_due,
            &mut last_emitted_snapshot_tick,
        );
        assert_eq!(sampled, vec![weapon_id]);
        crate::sim::clear_owner_reload_feedback_for_weapons(&mut registry, &sampled);
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_id)
                .unwrap()
                .owner_reload_status(),
            (0.0, false),
            "the next qualifying fixed tick advances the next endpoint"
        );
        for packet in server.packets_to_send(CLIENT_ID) {
            client.process_packet(&packet);
        }
        assert_eq!(client.drain_snapshots().len(), 1);
    }

    // Regression: client level unload reset entity replication but retained the
    // old state-slot schema and held baselines into the replacement level.
    #[test]
    fn client_level_unload_resets_state_slot_apply_state() {
        let mut endpoint = NetEndpoint::from_role(&NetRole::Connect {
            addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 1)),
        })
        .expect("client endpoint constructs")
        .expect("connect role yields an endpoint");
        let mut slot_table = SlotTable::new();
        let schema = state_slots::ReplicatedSlotSchema::build(&slot_table);
        let slot_id = schema
            .id_for("player.health")
            .expect("built-in health slot is replicated");
        let fingerprint = *schema.fingerprint();
        let record = postretro_net::state_slots::RawStateSlotRecord {
            slot_id: slot_id.0,
            kind: postretro_net::state_slots::STATE_RECORD_KIND_FULL_BASELINE,
            has_baseline_ref: false,
            baseline_ref: 0,
            baseline_id: 7,
            value: postretro_net::state_slots::WireSlotValue::Number(75.0),
        };

        let NetEndpoint::Client { state_slots, .. } = &mut endpoint else {
            panic!("connect role must construct a client endpoint");
        };
        let applied = state_slots.apply_snapshot_state(&mut slot_table, 1, &fingerprint, &[record]);
        assert_eq!(applied.slot_baselines, vec![(slot_id.0, 7)]);
        assert!(!state_slots.is_reset());

        endpoint.reset_level_scoped_client_state();

        let NetEndpoint::Client { state_slots, .. } = &endpoint else {
            panic!("endpoint role cannot change during reset");
        };
        assert!(
            state_slots.is_reset(),
            "old client schema and held baselines cannot survive level unload"
        );
    }

    // Regression: host level reset reconstructed the allocator and reused old
    // NetworkIds while the transport connection survived.
    #[test]
    fn host_level_reset_preserves_session_monotonic_network_ids() {
        let mut registry = EntityRegistry::new();
        let old_entity = registry.spawn(Transform::default());
        let new_entity = registry.spawn(Transform::default());
        let mut endpoint = NetEndpoint::from_role(&NetRole::Host { port: 0 })
            .expect("host endpoint constructs")
            .expect("host role yields an endpoint");

        let old_network_id = match &mut endpoint {
            NetEndpoint::Host { allocator, .. } => allocator.stamp(old_entity),
            NetEndpoint::Client { .. } => unreachable!("constructed host"),
        };
        endpoint.reset_level_scoped_host_state();
        let new_network_id = match &mut endpoint {
            NetEndpoint::Host { allocator, .. } => {
                assert!(!allocator.maps_entity(old_entity));
                allocator.stamp(new_entity)
            }
            NetEndpoint::Client { .. } => unreachable!("constructed host"),
        };

        assert!(
            new_network_id.0 > old_network_id.0,
            "NetworkIds remain session-monotonic across level lifetime"
        );
    }

    #[test]
    fn host_weapon_owner_sync_reads_weapon_provenance_and_clears_unloaded_prop() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let mut mesh = MeshComponent::stateless("models/player/model.gltf".to_string());
        mesh.attachments.push(MeshAttachment::unresolved(
            remote_materialize::ACTIVE_WEAPON_SOCKET.to_string(),
            "models/old_weapon/model.gltf".to_string(),
        ));
        registry.set_component(pawn, mesh).unwrap();

        let weapon = registry.spawn(Transform::default());
        registry
            .set_component(
                weapon,
                DescriptorProvenance {
                    canonical_name: "reference_pistol".to_string(),
                    owned_components: [DescriptorComponentKind::Weapon].into_iter().collect(),
                    map_overrides: Default::default(),
                    spawn_path: DescriptorSpawnPath::DefaultWeapon,
                },
            )
            .unwrap();
        let descriptors = vec![EntityTypeDescriptor {
            canonical_name: Some("reference_pistol".to_string()),
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: Some(WeaponDescriptor {
                damage: 1.0,
                range: 1.0,
                cooldown_ms: 1.0,
                fire_mode: FireMode::Semi,
                resolution: ResolutionMode::Hitscan,
                credit_source: None,
                third_person_model: Some("models/pistol/model.gltf".to_string()),
                viewmodel: None,
                resource: None,
                lower_ms: 0,
                raise_ms: 0,
                block_during_reload: None,
            }),
            mesh: None,
            health: None,
            behavior: None,
        }];
        let mut weapon_owners = WeaponOwners::new();
        set_active_inventory(&mut registry, pawn, weapon);
        weapon_owners.mark_attachment_dirty(pawn);

        let changed = synchronize_weapon_owner_attachments(
            &mut registry,
            &mut weapon_owners,
            &descriptors,
            &crate::scripting_systems::hit_zones::HitZoneStore::new(),
        );
        assert_eq!(changed, vec![pawn]);
        assert_eq!(
            active_wieldable_for_pawn(&registry, pawn),
            Some(weapon),
            "draining presentation changes preserves inventory provenance"
        );
        assert!(
            registry
                .get_component::<MeshComponent>(pawn)
                .unwrap()
                .attachments
                .is_empty(),
            "an unavailable third-person descriptor model clears the old hand prop"
        );
        assert!(
            synchronize_weapon_owner_attachments(
                &mut registry,
                &mut weapon_owners,
                &descriptors,
                &crate::scripting_systems::hit_zones::HitZoneStore::new(),
            )
            .is_empty(),
            "unchanged ownership performs no per-frame mesh synchronization"
        );
    }

    fn authorized_test_shot(
        shot_id: ShotId,
        pawn: EntityId,
        weapon: EntityId,
        fire_tick: u32,
        damage: f32,
        range: f32,
    ) -> AuthorizedShot {
        AuthorizedShot {
            shot_id,
            pawn,
            weapon,
            fire_tick,
            damage,
            range,
            pellet_count: 1,
            credit_source: "weapon.test.net".to_string(),
        }
    }

    #[test]
    fn shot_id_packs_pawn_network_id_and_client_tick() {
        let raw = shot_id_raw(NetworkId(0xABCD_EF01), 0x1234_5678);
        assert_eq!(raw, 0xABCD_EF01_1234_5678);
    }

    #[test]
    fn local_hit_wire_conversion_drops_unnamed_targets_and_keeps_empty_valid() {
        let named = EntityId::from_raw(1);
        let unnamed = EntityId::from_raw(2);
        let records = local_hits_to_wire_records(
            &[
                weapon::LocalHitRecord {
                    target: named,
                    point: Vec3::new(1.0, 2.0, 3.0),
                    zone: Some("head".to_string()),
                },
                weapon::LocalHitRecord {
                    target: unnamed,
                    point: Vec3::new(4.0, 5.0, 6.0),
                    zone: None,
                },
            ],
            |entity_id| (entity_id == named).then_some(NetworkId(77)),
        );

        assert_eq!(
            records,
            vec![wire::HitRecord {
                target: 77,
                point: [1.0, 2.0, 3.0],
                zone: Some("head".to_string()),
            }]
        );

        let empty = local_hits_to_wire_records(&[], |_| Some(NetworkId(1)));
        assert!(empty.is_empty(), "empty declarations remain valid misses");
    }

    fn movement_component_with_eye_height(eye_height: f32) -> PlayerMovementComponent {
        let mut descriptor = host_player_descriptor()
            .movement
            .expect("test player descriptor has movement");
        descriptor.capsule.eye_height = eye_height;
        PlayerMovementComponent::from_descriptor(&descriptor)
    }

    fn wall_at_x(x: f32) -> CollisionWorld {
        let points = vec![
            Point::new(x, -1.0, -1.0),
            Point::new(x, 1.0, -1.0),
            Point::new(x, 1.0, 1.0),
            Point::new(x, -1.0, 1.0),
        ];
        let triangles = vec![[0u32, 1, 2], [0, 2, 3]];
        CollisionWorld {
            mesh: TriMesh::new(points, triangles),
            isometry: Isometry::identity(),
        }
    }

    struct HitIngestFixture {
        registry: EntityRegistry,
        allocator: NetworkIdAllocator,
        owners: MovementOwners,
        open_shots: OpenAuthorizedShots,
        collision_world: CollisionWorld,
        pawn: EntityId,
        weapon: EntityId,
        target: EntityId,
        target_net: NetworkId,
        shot_id: ShotId,
    }

    impl HitIngestFixture {
        fn new(collision_world: CollisionWorld) -> Self {
            let mut registry = EntityRegistry::new();
            let pawn = registry.spawn(Transform {
                position: Vec3::ZERO,
                ..Transform::default()
            });
            registry
                .set_component(pawn, movement_component_with_eye_height(0.5))
                .unwrap();
            let weapon = registry.spawn(Transform::default());
            registry
                .set_component(weapon, test_weapon(10.0, 10.0))
                .unwrap();
            let target = registry.spawn(Transform {
                position: Vec3::new(4.0, 0.0, 0.0),
                ..Transform::default()
            });
            registry
                .set_component(
                    target,
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

            let mut allocator = NetworkIdAllocator::new();
            let pawn_net = allocator.stamp(pawn);
            let target_net = allocator.stamp(target);
            let shot_id = ShotId::from_parts(pawn_net, 11);
            let mut owners = MovementOwners::new();
            owners.set(pawn, 7);
            set_active_inventory(&mut registry, pawn, weapon);
            let mut open_shots = OpenAuthorizedShots::new();
            open_shots.record(
                authorized_test_shot(shot_id, pawn, weapon, 99, 10.0, 10.0),
                7,
            );

            Self {
                registry,
                allocator,
                owners,
                open_shots,
                collision_world,
                pawn,
                weapon,
                target,
                target_net,
                shot_id,
            }
        }

        fn declaration(&self, records: Vec<wire::HitRecord>) -> wire::HitDeclaration {
            wire::HitDeclaration {
                shot_id: self.shot_id.raw(),
                records,
            }
        }

        fn record(&self, point: Vec3, zone: Option<&str>) -> wire::HitRecord {
            wire::HitRecord {
                target: self.target_net.0,
                point: point.to_array(),
                zone: zone.map(str::to_string),
            }
        }

        fn ingest_result(
            &mut self,
            client_id: u64,
            declaration: &wire::HitDeclaration,
        ) -> HitDeclarationResult {
            ingest_hit_declaration(
                HostHitIngestContext {
                    registry: &mut self.registry,
                    collision_world: &self.collision_world,
                    allocator: &self.allocator,
                    owners: &self.owners,
                    open_shots: &mut self.open_shots,
                },
                client_id,
                declaration,
                |_| {},
            )
        }

        fn ingest(&mut self, client_id: u64, declaration: &wire::HitDeclaration) -> bool {
            self.ingest_result(client_id, declaration).hit_accepted
        }

        fn target_health(&self) -> HealthComponent {
            self.registry
                .get_component::<HealthComponent>(self.target)
                .unwrap()
                .clone()
        }
    }

    #[test]
    fn open_authorized_shots_store_records_and_retires_by_shot_id() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let shot_id = ShotId::from_parts(NetworkId(0xABCD_EF01), 0x1234_5678);
        assert_eq!(shot_id.raw(), 0xABCD_EF01_1234_5678);

        let mut shots = OpenAuthorizedShots::new();
        shots.record(
            authorized_test_shot(shot_id, pawn, weapon, 99, 10.0, 10.0),
            7,
        );

        let open = shots.get(shot_id).expect("shot should be open");
        assert_eq!(open.owner_client_id, 7);
        assert_eq!(open.shot.pawn, pawn);
        assert_eq!(open.shot.weapon, weapon);
        assert_eq!(open.shot.damage, 10.0);
        assert_eq!(open.shot.range, 10.0);
        assert_eq!(open.shot.pellet_count, 1);
        assert_eq!(open.shot.fire_tick, 99);
        assert_eq!(shots.len(), 1);
        assert_eq!(shots.retire(shot_id), Some(open));
        assert!(shots.get(shot_id).is_none());
        assert_eq!(shots.len(), 0);
    }

    #[test]
    fn open_authorized_shots_prune_stale_and_remove_client_or_pawn_state() {
        let mut registry = EntityRegistry::new();
        let pawn_a = registry.spawn(Transform::default());
        let pawn_b = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        let stale = ShotId::from_parts(NetworkId(10), 19);
        let newest_retained = ShotId::from_parts(NetworkId(10), 20);
        let other_client = ShotId::from_parts(NetworkId(11), 21);
        let mut shots = OpenAuthorizedShots::new();
        shots.record(
            authorized_test_shot(stale, pawn_a, weapon, 19, 10.0, 10.0),
            7,
        );
        shots.record(
            authorized_test_shot(newest_retained, pawn_a, weapon, 20, 10.0, 10.0),
            7,
        );
        shots.record(
            authorized_test_shot(other_client, pawn_b, weapon, 200, 10.0, 10.0),
            8,
        );

        shots.prune_stale(200);
        assert!(shots.get(stale).is_none());
        assert!(shots.get(newest_retained).is_some());
        assert!(shots.get(other_client).is_some());

        shots.remove_pawn(pawn_a);
        assert!(shots.get(newest_retained).is_none());
        assert!(shots.get(other_client).is_some());

        shots.remove_client(8);
        assert_eq!(shots.len(), 0);
    }

    #[test]
    fn pending_hit_declarations_are_bounded_per_client_and_cleanable() {
        let mut pending = PendingHitDeclarations::new();
        for tick in 0..=MAX_PENDING_HIT_DECLARATIONS_PER_CLIENT {
            pending.push(
                7,
                wire::HitDeclaration {
                    shot_id: ShotId::from_parts(NetworkId(4), tick as u32).raw(),
                    records: Vec::new(),
                },
            );
        }
        pending.push(
            8,
            wire::HitDeclaration {
                shot_id: ShotId::from_parts(NetworkId(5), 1).raw(),
                records: Vec::new(),
            },
        );

        assert_eq!(pending.len(), MAX_PENDING_HIT_DECLARATIONS_PER_CLIENT + 1);
        assert_eq!(
            pending
                .declarations
                .iter()
                .filter(|declaration| declaration.client_id == 7)
                .count(),
            MAX_PENDING_HIT_DECLARATIONS_PER_CLIENT
        );
        assert!(
            pending
                .declarations
                .iter()
                .all(|declaration| declaration.declaration.shot_id
                    != ShotId::from_parts(NetworkId(4), 0).raw()),
            "the oldest declaration for the overflowing client is dropped"
        );

        let mut allocator = NetworkIdAllocator::new();
        let pawn = EntityId::from_raw(99);
        allocator.stamp(pawn);
        pending.push(
            9,
            wire::HitDeclaration {
                shot_id: ShotId::from_parts(NetworkId(0), 777).raw(),
                records: Vec::new(),
            },
        );
        pending.remove_pawn_shots(&allocator, pawn);
        assert!(
            pending
                .declarations
                .iter()
                .all(|declaration| declaration.declaration.shot_id
                    != ShotId::from_parts(NetworkId(0), 777).raw())
        );
        pending.remove_client(7);
        assert!(
            pending
                .declarations
                .iter()
                .all(|declaration| declaration.client_id != 7)
        );
    }

    #[test]
    fn pending_hit_declaration_succeeds_after_same_frame_fire_authorization_opens_shot() {
        let mut fixture = HitIngestFixture::new(CollisionWorld::new());
        let declaration = fixture.declaration(vec![fixture.record(Vec3::new(4.0, 0.5, 0.0), None)]);
        fixture.open_shots.retire(fixture.shot_id);
        let mut pending = PendingHitDeclarations::new();
        pending.push(7, declaration.clone());

        assert!(
            pending
                .drain_ready(&HostCommandQueues::new(), &fixture.open_shots)
                .is_empty(),
            "before FIRE authorization the declaration remains queued"
        );
        fixture.open_shots.record(
            authorized_test_shot(
                fixture.shot_id,
                fixture.pawn,
                fixture.weapon,
                100,
                10.0,
                10.0,
            ),
            7,
        );
        let ready = pending.drain_ready(&HostCommandQueues::new(), &fixture.open_shots);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].client_id, 7);

        let result = fixture.ingest_result(ready[0].client_id, &ready[0].declaration);
        assert!(result.fire_accepted);
        assert!(result.hit_accepted);
        assert_eq!(fixture.target_health().current, 90.0);
    }

    #[test]
    fn hit_declaration_without_open_shot_rejects_without_damage() {
        let mut fixture = HitIngestFixture::new(CollisionWorld::new());
        fixture.open_shots.retire(fixture.shot_id);
        let declaration = fixture.declaration(vec![fixture.record(Vec3::new(4.0, 0.5, 0.0), None)]);

        assert_eq!(
            fixture.ingest_result(7, &declaration),
            HitDeclarationResult::default()
        );
        assert_eq!(fixture.target_health().current, 100.0);
    }

    #[test]
    fn hit_declaration_consumes_authorized_shot_once_and_credits_attacker() {
        let mut fixture = HitIngestFixture::new(CollisionWorld::new());
        let declaration = fixture.declaration(vec![fixture.record(Vec3::new(4.0, 0.5, 0.0), None)]);

        let result = fixture.ingest_result(7, &declaration);
        assert!(result.fire_accepted);
        assert!(result.hit_accepted);
        assert_eq!(fixture.target_health().current, 90.0);
        assert_eq!(
            fixture.ingest_result(7, &declaration),
            HitDeclarationResult::default()
        );
        let health = fixture.target_health();
        assert_eq!(health.current, 90.0);
        let entry = health.contributor_ledger.entries().first().unwrap();
        assert_eq!(entry.last_attacker, Some(fixture.pawn));
        assert_eq!(entry.last_weapon, Some(fixture.weapon));
    }

    // Regression: remote HIT validation rejected persistent zero-HP targets,
    // so gib policies saw local impacts but never authoritative remote ones.
    #[test]
    fn hit_declaration_on_zero_health_target_reaches_common_impact_dispatch() {
        let mut fixture = HitIngestFixture::new(CollisionWorld::new());
        let mut health = fixture.target_health();
        health.current = 0.0;
        fixture
            .registry
            .set_component(fixture.target, health)
            .unwrap();
        let declaration = fixture.declaration(vec![fixture.record(Vec3::new(4.0, 0.5, 0.0), None)]);

        let result = fixture.ingest_result(7, &declaration);

        assert!(result.fire_accepted);
        assert!(result.hit_accepted);
        assert!(fixture.target_health().current.abs() <= f32::EPSILON);
        let dispatches = fixture.registry.take_impact_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].target, fixture.target);
        assert!(dispatches[0].health_before.abs() <= f32::EPSILON);
        assert!((dispatches[0].health_after + 10.0).abs() <= f32::EPSILON);
    }

    // Regression: multi-pellet declarations used to batch every damage record
    // before running impact policy, so later pellets observed stale policy state.
    #[test]
    fn hit_declaration_runs_impact_consumer_after_each_accepted_pellet() {
        let mut fixture = HitIngestFixture::new(CollisionWorld::new());
        fixture.open_shots.retire(fixture.shot_id);
        let mut shot = authorized_test_shot(
            fixture.shot_id,
            fixture.pawn,
            fixture.weapon,
            99,
            10.0,
            10.0,
        );
        shot.pellet_count = 2;
        fixture.open_shots.record(shot, 7);
        let declaration = fixture.declaration(vec![
            fixture.record(Vec3::new(4.0, 0.5, 0.0), None),
            fixture.record(Vec3::new(4.0, 0.5, 0.0), None),
        ]);
        let target = fixture.target;
        let mut health_seen_by_consumer = Vec::new();

        let result = ingest_hit_declaration(
            HostHitIngestContext {
                registry: &mut fixture.registry,
                collision_world: &fixture.collision_world,
                allocator: &fixture.allocator,
                owners: &fixture.owners,
                open_shots: &mut fixture.open_shots,
            },
            7,
            &declaration,
            |registry| {
                health_seen_by_consumer.push(
                    registry
                        .get_component::<HealthComponent>(target)
                        .expect("target stays live")
                        .current,
                );
            },
        );

        assert!(result.hit_accepted);
        assert_eq!(health_seen_by_consumer, [90.0, 80.0]);
    }

    #[test]
    fn hit_declaration_uses_authorization_time_weapon_facts_after_switch_and_despawn() {
        let mut fixture = HitIngestFixture::new(CollisionWorld::new());
        let original_weapon = fixture.weapon;
        let switched_weapon = fixture.registry.spawn(Transform::default());
        fixture
            .registry
            .set_component(switched_weapon, test_weapon(90.0, 100.0))
            .unwrap();
        set_active_inventory(&mut fixture.registry, fixture.pawn, switched_weapon);
        fixture
            .registry
            .despawn(original_weapon)
            .expect("authorized-time weapon can despawn before HIT arrives");
        let declaration = fixture.declaration(vec![fixture.record(Vec3::new(4.0, 0.5, 0.0), None)]);

        let result = fixture.ingest_result(7, &declaration);
        assert!(result.fire_accepted);
        assert!(result.hit_accepted);
        let health = fixture.target_health();
        assert_eq!(
            health.current, 90.0,
            "HIT applies the 10 damage captured when FIRE was authorized, not the switched weapon's damage"
        );
        let entry = health.contributor_ledger.entries().first().unwrap();
        assert_eq!(entry.last_weapon, Some(original_weapon));
        assert_eq!(entry.source_id, "weapon.test.net");
    }

    #[test]
    fn hit_declaration_with_wrong_owner_rejects_without_retiring_other_client_shot() {
        let mut fixture = HitIngestFixture::new(CollisionWorld::new());
        let declaration = fixture.declaration(vec![fixture.record(Vec3::new(4.0, 0.5, 0.0), None)]);

        assert_eq!(
            fixture.ingest_result(8, &declaration),
            HitDeclarationResult::default()
        );
        assert_eq!(fixture.target_health().current, 100.0);
        assert!(fixture.open_shots.get(fixture.shot_id).is_some());
    }

    #[test]
    fn hit_declaration_retires_bound_shot_even_when_records_fail_validation() {
        let mut fixture = HitIngestFixture::new(wall_at_x(1.0));
        let declaration = fixture.declaration(vec![fixture.record(Vec3::new(4.0, 0.5, 0.0), None)]);

        let result = fixture.ingest_result(7, &declaration);
        assert!(result.fire_accepted);
        assert!(!result.hit_accepted);
        assert_eq!(fixture.target_health().current, 100.0);
        assert!(fixture.open_shots.get(fixture.shot_id).is_none());
    }

    #[test]
    fn hit_declaration_static_wall_rejects_but_host_pose_mismatch_does_not() {
        let mut behind_wall = HitIngestFixture::new(wall_at_x(1.0));
        let blocked =
            behind_wall.declaration(vec![behind_wall.record(Vec3::new(4.0, 0.5, 0.0), None)]);
        let result = behind_wall.ingest_result(7, &blocked);
        assert!(result.fire_accepted);
        assert!(!result.hit_accepted);
        assert_eq!(behind_wall.target_health().current, 100.0);

        let mut live_pose_mismatch = HitIngestFixture::new(CollisionWorld::new());
        live_pose_mismatch
            .registry
            .set_component(
                live_pose_mismatch.target,
                Transform {
                    position: Vec3::new(-20.0, 0.0, 0.0),
                    ..Transform::default()
                },
            )
            .unwrap();
        let declared_past_pose = live_pose_mismatch.declaration(vec![
            live_pose_mismatch.record(Vec3::new(4.0, 0.5, 0.0), None),
        ]);
        let result = live_pose_mismatch.ingest_result(7, &declared_past_pose);
        assert!(result.fire_accepted);
        assert!(result.hit_accepted);
        assert_eq!(live_pose_mismatch.target_health().current, 90.0);
    }

    #[test]
    fn hit_declaration_rejects_records_beyond_tolerated_range() {
        let mut fixture = HitIngestFixture::new(CollisionWorld::new());
        let declaration =
            fixture.declaration(vec![fixture.record(Vec3::new(13.0, 0.5, 0.0), None)]);

        let result = fixture.ingest_result(7, &declaration);
        assert!(result.fire_accepted);
        assert!(!result.hit_accepted);
        assert_eq!(fixture.target_health().current, 100.0);
    }

    #[test]
    fn hit_declaration_clamps_accepted_records_to_default_pellet_count() {
        let mut fixture = HitIngestFixture::new(CollisionWorld::new());
        let second_target = fixture.registry.spawn(Transform {
            position: Vec3::new(5.0, 0.0, 0.0),
            ..Transform::default()
        });
        fixture
            .registry
            .set_component(
                second_target,
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
        let second_net = fixture.allocator.stamp(second_target);
        let declaration = fixture.declaration(vec![
            fixture.record(Vec3::new(4.0, 0.5, 0.0), None),
            wire::HitRecord {
                target: second_net.0,
                point: Vec3::new(5.0, 0.5, 0.0).to_array(),
                zone: None,
            },
        ]);

        let result = fixture.ingest_result(7, &declaration);
        assert!(result.fire_accepted);
        assert!(result.hit_accepted);
        assert_eq!(fixture.target_health().current, 90.0);
        assert_eq!(
            fixture
                .registry
                .get_component::<HealthComponent>(second_target)
                .unwrap()
                .current,
            100.0
        );
    }

    #[test]
    fn hit_declaration_pellet_clamp_counts_declared_invalid_records() {
        let mut fixture = HitIngestFixture::new(CollisionWorld::new());
        let declaration = fixture.declaration(vec![
            wire::HitRecord {
                target: 999_999,
                point: Vec3::new(4.0, 0.5, 0.0).to_array(),
                zone: None,
            },
            fixture.record(Vec3::new(4.0, 0.5, 0.0), None),
        ]);

        let result = fixture.ingest_result(7, &declaration);
        assert!(result.fire_accepted);
        assert!(!result.hit_accepted);
        assert_eq!(
            fixture.target_health().current,
            100.0,
            "the valid second record is beyond the single declared pellet slot"
        );
    }

    #[test]
    fn hit_declaration_zone_scales_damage_and_keeps_pitched_geometry() {
        let mut fixture = HitIngestFixture::new(CollisionWorld::new());
        let mut health = fixture.target_health();
        health.zone_multipliers.insert("head".to_string(), 2.5);
        fixture
            .registry
            .set_component(fixture.target, health)
            .unwrap();
        let declaration =
            fixture.declaration(vec![fixture.record(Vec3::new(4.0, 1.5, 0.0), Some("head"))]);

        assert!(fixture.ingest(7, &declaration));
        let health = fixture.target_health();
        assert_eq!(health.current, 75.0);
        let entry = health.contributor_ledger.entries().first().unwrap();
        assert_eq!(entry.last_hit_damage, 25.0);
        assert_eq!(entry.last_hit_zone.as_deref(), Some("head"));
        assert_eq!(entry.last_attacker, Some(fixture.pawn));
    }

    // Regression: connected-client level unload can leave the transport connected
    // while descriptors are empty. Descriptor-backed snapshots must not apply into
    // that empty level state, but classless Transform traffic remains safe.
    #[test]
    fn descriptor_class_snapshots_require_descriptor_table() {
        let classed = snapshot_with_record(EntityRecord::FullBaseline {
            network_id: 7,
            baseline_id: 1,
            last_processed_client_tick: None,
            local_player: false,
            entity_class: Some("grunt".to_string()),
            active_weapon_archetype: None,
            components: vec![ComponentPayload::Transform(sample_wire_transform())],
        });
        let classless = snapshot_with_record(EntityRecord::FullBaseline {
            network_id: 8,
            baseline_id: 1,
            last_processed_client_tick: None,
            local_player: false,
            entity_class: None,
            active_weapon_archetype: None,
            components: vec![ComponentPayload::Transform(sample_wire_transform())],
        });

        assert!(
            snapshot_requires_descriptor_table(&classed),
            "descriptor-backed baselines wait for the level descriptor table"
        );
        assert!(
            !snapshot_requires_descriptor_table(&classless),
            "classless transform replication can still apply without descriptors"
        );
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
                ComponentKind::Brain => Some(ComponentKind::KinematicMover),
                ComponentKind::KinematicMover => Some(ComponentKind::TriggerVolume),
                ComponentKind::TriggerVolume => Some(ComponentKind::AmmoReserve),
                ComponentKind::AmmoReserve => Some(ComponentKind::Spawner),
                ComponentKind::Spawner => Some(ComponentKind::EntityState),
                ComponentKind::EntityState => Some(ComponentKind::DeferredEffect),
                ComponentKind::DeferredEffect => Some(ComponentKind::Inventory),
                ComponentKind::Inventory => None,
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
        assert_eq!(
            allocator.network_id_for_entity(pawn),
            Some(expected_net_id),
            "host can name the pawn on the wire"
        );
        assert_eq!(
            allocator.entity_for_network_id(expected_net_id),
            Some(pawn),
            "host can resolve a declared target NetworkId"
        );
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

        allocator.forget(pawn);
        assert_eq!(allocator.network_id_for_entity(pawn), None);
        assert_eq!(allocator.entity_for_network_id(expected_net_id), None);
    }

    // Regression: processing all exits before all entries let a same-poll
    // Participating -> Demoted sequence spawn a pawn for the final Admitted state.
    #[test]
    fn ordered_participation_then_demotion_leaves_no_host_pawn() {
        use postretro_net::handshake::HoldingCause;
        use postretro_net::slots::SlotEvent;

        const CLIENT_ID: u64 = 43;
        let events = [
            SlotEvent::Participating {
                client_id: CLIENT_ID,
            },
            SlotEvent::Demoted {
                client_id: CLIENT_ID,
                cause: HoldingCause::HostLevelAbsent,
            },
        ];
        let mut registry = EntityRegistry::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut replication = ServerReplication::new();
        let mut state_slots = state_slots::HostStateReplication::new();
        let mut slot_pawns = SlotPawns::new();
        let mut command_queues = HostCommandQueues::new();
        let mut owners = MovementOwners::new();
        let mut weapon_owners = WeaponOwners::new();
        let mut open_shots = OpenAuthorizedShots::new();
        let mut pending_hit_declarations = PendingHitDeclarations::new();
        let mut weaponless_fire_logged = std::collections::HashSet::new();
        let mut last_sent_tuning = HashMap::new();

        for event in &events {
            host_handle_lifecycle(
                &mut registry,
                &mut allocator,
                &mut replicable,
                &mut replication,
                &mut state_slots,
                &mut slot_pawns,
                &mut command_queues,
                &mut owners,
                &mut weapon_owners,
                &mut open_shots,
                &mut pending_hit_declarations,
                &mut weaponless_fire_logged,
                &mut last_sent_tuning,
                std::slice::from_ref(event),
            );
            if let SlotEvent::Participating { client_id } = event {
                replication.register_client(*client_id);
                state_slots.register_client(*client_id);
                host_handle_accept(
                    &mut registry,
                    &mut allocator,
                    &mut replicable,
                    &mut slot_pawns,
                    *client_id,
                );
            }
        }

        assert!(
            slot_pawns.pawn_for(CLIENT_ID).is_none(),
            "final admitted state owns no pawn"
        );
        assert!(
            replicable.iter().next().is_none(),
            "demotion removes the just-spawned pawn from replication"
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

    #[test]
    fn pre_sync_interpolation_uses_held_remote_walk_rate() {
        use postretro_entities::components::mesh::{
            AnimationState, DEFAULT_CROSSFADE_MS, InterruptPolicy, MeshAnimation, MeshComponent,
            RATE_MIN, resolve_pending_animation_stamps,
        };
        use std::collections::HashMap;

        let mut registry = EntityRegistry::new();
        let mut replication = ClientReplication::new();
        replication.apply_snapshot(
            &mut registry,
            &SnapshotMessage {
                sequence: 0,
                server_tick: 100,
                records: vec![EntityRecord::FullBaseline {
                    network_id: 7,
                    baseline_id: 1,
                    last_processed_client_tick: None,
                    local_player: false,
                    entity_class: None,
                    active_weapon_archetype: None,
                    components: vec![ComponentPayload::Transform(WireTransform {
                        position: [4.0, 0.0, 0.0],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [1.0, 1.0, 1.0],
                    })],
                }],
                state_schema_fingerprint: [0; 32],
                state_records: Vec::new(),
            },
        );
        let id = *replication
            .map()
            .get(&NetworkId(7))
            .expect("remote baseline is mapped");
        let mut states = HashMap::new();
        states.insert(
            "locomotion".to_string(),
            AnimationState {
                clip: "Locomotion".to_string(),
                looping: true,
                crossfade_ms: DEFAULT_CROSSFADE_MS,
                interrupt: InterruptPolicy::Smooth,
                travel_speed: None,
                clip_index: None,
            },
        );
        registry
            .set_component(
                id,
                MeshComponent::animated(
                    "models/remote_enemy/scene.gltf".to_string(),
                    MeshAnimation::new(states, "locomotion".to_string()),
                ),
            )
            .unwrap();
        resolve_pending_animation_stamps(&mut registry, 0.0);
        replication.cache_remote_enemy_walk_playback(
            NetworkId(7),
            Some((60.0, "locomotion".to_string(), None)),
        );

        let time_sync = ClientTimeSync::new();
        let mut delay = InterpolationDelayState::new();
        client_sample_interpolation(
            &mut registry,
            &mut replication,
            &time_sync,
            &mut delay,
            1.0 / 60.0,
            1.0,
        );

        let animation = registry
            .get_component::<MeshComponent>(id)
            .unwrap()
            .animation
            .as_ref()
            .unwrap();
        assert!(
            (animation.rate - RATE_MIN).abs() < EPSILON,
            "without an estimated server tick, the held displayed pose has zero speed"
        );
        assert!(
            (registry.interpolated_transform(id, 1.0).unwrap().position.x - 4.0).abs() < EPSILON,
            "pre-sync sampling must not invent a render tick or move the held baseline pose"
        );
    }

    // Regression: pre-sync presentation returned empty pose inputs, so a remote
    // avatar began at neutral pitch and snapped when the first clock echo arrived.
    #[test]
    fn pre_sync_interpolation_exposes_only_newest_held_remote_player_pitch() {
        let movement_payload = |aim_pitch| {
            ComponentPayload::PlayerMovementState(WirePlayerMovementState {
                velocity: [12.0, 0.0, 4.0],
                ground: postretro_net::wire::WireGroundRef::World,
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
                aim_pitch,
            })
        };
        let record = |baseline_id, transform_x, aim_pitch| EntityRecord::FullBaseline {
            network_id: 7,
            baseline_id,
            last_processed_client_tick: None,
            local_player: false,
            entity_class: Some("player".to_string()),
            active_weapon_archetype: None,
            components: vec![
                ComponentPayload::Transform(WireTransform {
                    position: [transform_x, 0.0, 0.0],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                    scale: [1.0, 1.0, 1.0],
                }),
                movement_payload(aim_pitch),
            ],
        };

        let mut registry = EntityRegistry::new();
        let mut replication = ClientReplication::new();
        replication.apply_snapshot(
            &mut registry,
            &SnapshotMessage {
                sequence: 0,
                server_tick: 100,
                records: vec![record(1, 2.0, -0.35)],
                state_schema_fingerprint: [0; 32],
                state_records: Vec::new(),
            },
        );
        replication.apply_snapshot(
            &mut registry,
            &SnapshotMessage {
                sequence: 1,
                server_tick: 110,
                records: vec![record(2, 4.0, 0.6)],
                state_schema_fingerprint: [0; 32],
                state_records: Vec::new(),
            },
        );
        replication.cache_remote_player_locomotion(
            NetworkId(7),
            Some(client::RemotePlayerLocomotionReference {
                idle_state: "idle".to_string(),
                walk_state: "walk_forward".to_string(),
                run_state: None,
                walk_speed: 60.0,
                run_speed: 60.0,
                walk_derived_travel_speed: None,
                run_derived_travel_speed: None,
            }),
        );

        let inputs = client_sample_interpolation(
            &mut registry,
            &mut replication,
            &ClientTimeSync::new(),
            &mut InterpolationDelayState::new(),
            1.0 / 60.0,
            1.0,
        );

        assert_eq!(inputs.aim_pitches.get(&NetworkId(7)), Some(&0.6));
        assert!(
            inputs.heading_yaws.is_empty(),
            "held presentation has no motion-derived heading"
        );
        let id = replication.entity_for_network_id(NetworkId(7)).unwrap();
        assert!(
            (registry.interpolated_transform(id, 1.0).unwrap().position.x - 4.0).abs() < EPSILON,
            "pre-sync presentation holds the newest applied transform"
        );
    }

    // --- Issue 3b: the listen host's OWN pawn replicates outbound ---------------

    use crate::scripting::builtins::spawn_from_player_starts;
    use crate::scripting::map_entity::MapEntity;
    use postretro_entities::components::mesh::MeshComponent;
    use postretro_entities::{EntityTypeDescriptor, MeshDescriptor};
    use postretro_foundation::{
        AirParams, CapsuleParams, FallParams, GroundParams, PlayerMovementDescriptor, SpeedParams,
        ViewFeelParams,
    };
    use postretro_net::replication::{ServerReplication, typed_records};
    use postretro_net::wire::EntityRecord;

    /// A minimal `"player"` descriptor carrying a movement component, mirroring the
    /// lifecycle-test fixture so `spawn_from_player_starts` materializes a real
    /// `PlayerMovement` pawn and marks it the local player (the host's own pawn).
    fn host_player_descriptor() -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some("player".to_string()),
            inventory: None,
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
            mesh: Some(MeshDescriptor {
                model: "models/exo_red/model.gltf".to_string(),
                shadow_only: true,
                attachments: Default::default(),
                shadow_bias_scale: 1.0,
                animations: Default::default(),
                default_state: None,
                locomotion: None,
            }),
            health: None,
            behavior: None,
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
        let pawn = registry
            .local_player_pawn()
            .expect("the host boot pawn is marked the local player");
        assert!(
            registry
                .get_component::<MeshComponent>(pawn)
                .expect("descriptor player mesh materializes without a snapshot")
                .shadow_only,
            "the single-player/listen-host descriptor path attaches the body shadow mesh"
        );
        pawn
    }

    // Regression: the listen host used full descriptor materialization for joined
    // slot pawns, retaining the local-view `shadowOnly` bit and hiding every peer
    // avatar from the host's forward pass.
    #[test]
    fn listen_host_joined_player_avatar_is_forward_visible() {
        let mut registry = EntityRegistry::new();
        let mut allocator = NetworkIdAllocator::new();
        let mut replicable = ReplicableSet::new();
        let mut slot_pawns = SlotPawns::new();
        let mut queues = HostCommandQueues::new();
        let mut owners = MovementOwners::new();
        let mut weapon_owners = WeaponOwners::new();
        let mut open_shots = OpenAuthorizedShots::default();
        let mut pending_hits = PendingHitDeclarations::default();
        let mut weaponless_logged = std::collections::HashSet::new();
        let descriptors = [host_player_descriptor()];
        let spawn_points = [host_player_spawn_placement()];
        const CLIENT_ID: u64 = 77;

        host_handle_accept_descriptor(
            &mut registry,
            &mut allocator,
            &mut replicable,
            &mut slot_pawns,
            &mut queues,
            &mut owners,
            &mut weapon_owners,
            &mut open_shots,
            &mut pending_hits,
            &mut weaponless_logged,
            CLIENT_ID,
            &spawn_points,
            &descriptors,
            None,
        );

        let pawn = slot_pawns.pawn_for(CLIENT_ID).expect("join spawns pawn");
        assert!(
            !registry
                .get_component::<MeshComponent>(pawn)
                .expect("joined player carries descriptor mesh")
                .shadow_only,
            "shadowOnly is a local-view exception; the listen host sees joined peers forward"
        );
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
        let mut weapon_owners = WeaponOwners::new();

        // Register the host's own pawn for outbound replication (the production seam:
        // `App::host_register_own_pawn_after_install`).
        host_register_own_pawn(
            &mut allocator,
            &mut replicable,
            &mut host_pawn,
            &mut weapon_owners,
            pawn,
        );

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
        // recipient is not the owner — there is no owner) and the boot pawn's descriptor
        // class must be present so the client can materialize its remote avatar.
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
        let (local_player, entity_class) = match host_record {
            EntityRecord::FullBaseline {
                local_player,
                entity_class,
                ..
            }
            | EntityRecord::Delta {
                local_player,
                entity_class,
                ..
            } => (*local_player, entity_class.clone()),
            _ => panic!("host pawn record is a movement baseline/delta"),
        };
        assert!(
            !local_player,
            "the host pawn is NEVER marked local_player for any recipient"
        );
        assert_eq!(
            entity_class.as_deref(),
            Some("player"),
            "the host boot pawn carries the descriptor class needed for remote presentation"
        );

        let mut client_registry = EntityRegistry::new();
        let mut client_replication = ClientReplication::new();
        let client_outcome = client_replication.apply_snapshot(
            &mut client_registry,
            &SnapshotMessage {
                sequence: 1,
                server_tick: 1,
                records,
                state_schema_fingerprint: [0u8; 32],
                state_records: Vec::new(),
            },
        );
        let remote = client_outcome
            .remote_entities
            .first()
            .expect("host pawn baseline asks the client to materialize its remote presentation");
        assert_eq!(remote.entity_class, "player");
        assert!(remote_materialize::materialize_armed_remote_player(
            remote,
            &[host_player_descriptor()],
            &mut client_registry,
            None,
        ));
        let mesh = client_registry
            .get_component::<MeshComponent>(remote.entity_id)
            .expect("client materializes the host pawn descriptor mesh");
        assert!(
            !mesh.shadow_only,
            "the peer-facing host avatar is forward-visible rather than owner shadow-only"
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
        let mut weapon_owners = WeaponOwners::new();

        // First install.
        let first = spawn_host_boot_pawn(&mut registry);
        let first_weapon = registry.spawn(Transform::default());
        set_active_inventory(&mut registry, first, first_weapon);
        host_register_own_pawn(
            &mut allocator,
            &mut replicable,
            &mut host_pawn,
            &mut weapon_owners,
            first,
        );
        assert!(replicable.contains(first));
        assert_eq!(
            active_wieldable_for_pawn(&registry, first),
            Some(first_weapon)
        );

        // Re-registering the SAME pawn (idempotent install) keeps exactly one entry.
        host_register_own_pawn(
            &mut allocator,
            &mut replicable,
            &mut host_pawn,
            &mut weapon_owners,
            first,
        );
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

        host_register_own_pawn(
            &mut allocator,
            &mut replicable,
            &mut host_pawn,
            &mut weapon_owners,
            second,
        );
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

        let second_weapon = registry.spawn(Transform::default());
        set_active_inventory(&mut registry, second, second_weapon);
        host_register_own_pawn(
            &mut allocator,
            &mut replicable,
            &mut host_pawn,
            &mut weapon_owners,
            second,
        );
        assert_eq!(
            active_wieldable_for_pawn(&registry, second),
            Some(second_weapon)
        );
        assert_eq!(
            host_unregister_own_pawn(
                &mut allocator,
                &mut replicable,
                &mut host_pawn,
                &mut weapon_owners,
            ),
            Some(second),
            "a replacement map without a player spawn unregisters the old host pawn"
        );
        assert_eq!(host_pawn, None);
        assert!(!replicable.contains(second));
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

        registry
            .set_component(
                a,
                MeshComponent::stateless("models/avatar.gltf".to_string()),
            )
            .expect("host fixture entity remains live");
        assert_eq!(
            netcode_remote_positions(&host, &registry),
            vec![Vec3::new(-4.0, 0.0, 5.0)],
            "a rendered host avatar does not also receive a fallback capsule"
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

        let remote = {
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
                            active_weapon_archetype: None,
                            components: vec![
                                ComponentPayload::Transform(WireTransform {
                                    position: [3.0, 0.0, 0.0],
                                    rotation: [0.0, 0.0, 0.0, 1.0],
                                    scale: [1.0, 1.0, 1.0],
                                }),
                                ComponentPayload::PlayerMovementState(WirePlayerMovementState {
                                    velocity: [0.0, 0.0, 0.0],
                                    ground: postretro_net::wire::WireGroundRef::World,
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
                                    aim_pitch: 0.0,
                                }),
                            ],
                        },
                        postretro_net::wire::EntityRecord::FullBaseline {
                            network_id: 8,
                            baseline_id: 1,
                            last_processed_client_tick: None,
                            local_player: false,
                            entity_class: None,
                            active_weapon_archetype: None,
                            components: vec![ComponentPayload::Transform(WireTransform {
                                position: [9.0, 0.0, 0.0],
                                rotation: [0.0, 0.0, 0.0, 1.0],
                                scale: [1.0, 1.0, 1.0],
                            })],
                        },
                    ],
                    state_schema_fingerprint: [0u8; 32],
                    state_records: Vec::new(),
                },
            );
            replication
                .map()
                .get(&NetworkId(8))
                .copied()
                .expect("remote baseline mapped")
        };

        assert_eq!(
            netcode_remote_positions(&client, &registry),
            vec![Vec3::new(9.0, 0.0, 0.0)],
            "client remote overlay excludes the local predicted pawn"
        );

        registry
            .set_component(
                remote,
                MeshComponent::stateless("models/avatar.gltf".to_string()),
            )
            .expect("remote remains live after materialization");
        assert!(
            netcode_remote_positions(&client, &registry).is_empty(),
            "a remote with a materialized mesh no longer needs a diagnostic capsule"
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
