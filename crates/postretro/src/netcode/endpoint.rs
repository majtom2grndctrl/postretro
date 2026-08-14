// Network endpoint ownership and lifecycle resets.
// See: context/lib/networking.md

use super::*;
use postretro_foundation::Seat;

/// The active network endpoint held by `App`. `None` for single-player; a
/// `Host`/`Client` variant once the role's transport is constructed.
///
/// Construction can fail (socket bind, transport init); failures are logged at
/// the call site and degrade to single-player (the field stays `None`) so a
/// netcode setup error never blocks boot.
// The host endpoint necessarily retains several live replication maps. Boxing
// those individually would add indirection to its hot lifecycle paths without
// reducing the singleton's meaningful footprint.
#[allow(clippy::large_enum_variant)]
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
        /// World-item entities registered for replication while they carry the
        /// host-local `TouchableComponent`. The sweep removes stale entries on
        /// acquisition and stamps a fresh `NetworkId` when a drop restores touchability.
        world_items: std::collections::HashSet<EntityId>,
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
        tuning: Option<Box<TuningPayload>>,
        tuning_generation: u64,
        applied_movement_tuning_generation: u64,
        next_switch_declaration_id: u32,
        pending_switch_declarations: VecDeque<PendingSwitchDeclaration>,
        /// Latest status-only roster received from the host. This survives level
        /// changes with the endpoint and feeds the client-local UI projection.
        session_status: ClientSessionStatus,
    },
}

/// Client-owned view of the latest status-only session roster.
///
/// The wire shape deliberately contains no player claims or display names. The
/// client retains it as one value so `session_id`, own seat, open-seat count, and
/// seat connectivity cannot drift across separate presentation fields.
#[derive(Debug, Default)]
pub(crate) struct ClientSessionStatus {
    roster: Option<SessionRosterMessage>,
}

impl ClientSessionStatus {
    /// Replace the retained publication. Returns whether its observable status
    /// changed, allowing presentation diagnostics to avoid duplicate lines.
    pub(crate) fn retain(&mut self, roster: SessionRosterMessage) -> bool {
        let changed = self.roster.as_ref() != Some(&roster);
        self.roster = Some(roster);
        changed
    }

    #[must_use]
    pub(crate) fn open_seats(&self) -> Option<u32> {
        self.roster.as_ref().map(|roster| roster.open_seats)
    }

    #[must_use]
    pub(crate) fn local_seat(&self) -> Option<Seat> {
        self.roster
            .as_ref()
            .and_then(|roster| roster.your_seat)
            .map(Seat)
    }
}

const SESSION_OPEN_SEATS_SLOT: &str = "session.openSeats";

fn apply_client_session_roster(
    session_status: &mut ClientSessionStatus,
    slot_table: &mut SlotTable,
    roster: SessionRosterMessage,
) -> (bool, u32) {
    let changed = session_status.retain(roster);
    let open_seats = session_status
        .open_seats()
        .expect("retaining a roster publishes its open-seat count");
    slot_table
        .get_mut(SESSION_OPEN_SEATS_SLOT)
        .expect("engine state catalog declares session.openSeats")
        .write_value(Some(postretro_entities::SlotValue::Number(
            open_seats as f32,
        )));
    (changed, open_seats)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PendingSwitchDeclaration {
    pub(crate) declaration_id: u32,
    pub(crate) target_slot: u8,
    /// Slot the client actually presented as active when this local switch chain
    /// began. Older host outcomes may move the authoritative rollback point, but
    /// must not turn an unpresented intermediate target into last-weapon history.
    pub(crate) held_origin_slot: usize,
    /// Host-authoritative active slot restored if this declaration is refused.
    /// Ordered predecessor outcomes rebase this without changing local history.
    pub(crate) rollback_slot: usize,
    pub(crate) rollback_last_weapon_slot: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SwitchOutcome {
    Accepted(ServerSwitchAccepted),
    Refused(ServerSwitchRefused),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CurrentSwitchResolution {
    None,
    Accepted {
        last_weapon_slot: Option<usize>,
    },
    Refused {
        target_slot: usize,
        rollback_slot: usize,
        last_weapon_slot: Option<usize>,
    },
}

#[derive(Debug, Default)]
pub(crate) struct ClientApplyFrameOutcome {
    pub(crate) materialized_remote_entity_presentation: bool,
    pub(crate) armed_local_pawn: Option<ClientArmedLocalPawn>,
    /// Host slot identity carried with the latest fresh owner-private cooldown.
    pub(crate) owner_private_weapon_cooldown_slot: Option<usize>,
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
    pub(crate) clock: EngineClock,
    sender: TimeSyncSender,
    pub(crate) estimator: ClockEstimator,
}

impl ClientTimeSync {
    pub(crate) fn new() -> Self {
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
    pub(crate) fn maybe_send_probe(&mut self, client_tick: u32) -> Option<TimeSyncRequest> {
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
            ServerControlMessage::SwitchAccepted(accepted) => {
                let Some(session) = app.session.as_mut() else {
                    continue;
                };
                let Some(endpoint) = session.net_endpoint.as_mut() else {
                    continue;
                };
                let resolution = endpoint.take_switch_outcome(SwitchOutcome::Accepted(accepted));
                apply_client_switch_resolution(session, resolution);
            }
            ServerControlMessage::SwitchRefused(refusal) => {
                let Some(session) = app.session.as_mut() else {
                    continue;
                };
                let Some(endpoint) = session.net_endpoint.as_mut() else {
                    continue;
                };
                let resolution = endpoint.take_switch_outcome(SwitchOutcome::Refused(refusal));
                apply_client_switch_resolution(session, resolution);
            }
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
                    drop(registry);
                    session.gameplay_input_latch.clear();
                }
                app.client_fire_resolutions.clear();
                app.client_predicted_shots.clear();
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
            ServerControlMessage::SessionRoster(roster) => {
                let Some(session) = app.session.as_mut() else {
                    continue;
                };
                let Some(NetEndpoint::Client { session_status, .. }) =
                    session.net_endpoint.as_mut()
                else {
                    continue;
                };
                let (changed, open_seats) = apply_client_session_roster(
                    session_status,
                    &mut session.scripting.script_ctx.slot_table.borrow_mut(),
                    roster,
                );
                if changed {
                    log::info!("[Net] {open_seats} session seats remain open");
                }
            }
        }
    }
}

fn apply_client_switch_resolution(
    session: &mut crate::session::Session,
    resolution: CurrentSwitchResolution,
) {
    let (target_slot, rollback_slot, last_weapon_slot) = match resolution {
        CurrentSwitchResolution::None => return,
        CurrentSwitchResolution::Accepted { last_weapon_slot } => {
            session
                .gameplay_input_latch
                .wieldable_selection_mut()
                .confirm_latest_declaration(last_weapon_slot);
            return;
        }
        CurrentSwitchResolution::Refused {
            target_slot,
            rollback_slot,
            last_weapon_slot,
        } => (target_slot, rollback_slot, last_weapon_slot),
    };
    let mut registry = session.scripting.script_ctx.registry.borrow_mut();
    let Some(pawn) = registry.local_player_movement_pawn() else {
        return;
    };
    let applied =
        crate::sim::refuse_local_wieldable_switch(&mut registry, pawn, target_slot, rollback_slot);
    if !applied {
        return;
    }
    let active_slot = registry
        .get_component::<Inventory>(pawn)
        .ok()
        .map(|inventory| inventory.active_slot);
    drop(registry);
    let selection = session.gameplay_input_latch.wieldable_selection_mut();
    selection.reset_to_active_with_last(active_slot, last_weapon_slot);
}

impl NetEndpoint {
    /// Connected-client state needed by main-thread-only private persistence.
    /// The roster's seat is session-scoped and selects a live value only; the
    /// persistence key remains the local durable player claim.
    #[must_use]
    pub(crate) fn client_per_owner_save_context(&self) -> Option<(bool, bool, Option<Seat>)> {
        let Self::Client {
            client,
            session_status,
            ..
        } = self
        else {
            return None;
        };
        Some((
            client.is_connected(),
            client.is_participating(),
            session_status.local_seat(),
        ))
    }

    /// Send a client-local switch declaration over reliable Control. The client
    /// transport refuses to queue it before participation, so an old level cannot
    /// leak a selection into a newly promoted pawn.
    pub(crate) fn send_client_switch_declaration(
        &mut self,
        slot: u8,
        rollback_slot: usize,
        rollback_last_weapon_slot: Option<usize>,
    ) {
        if let Self::Client {
            client,
            next_switch_declaration_id,
            pending_switch_declarations,
            ..
        } = self
        {
            let declaration_id = *next_switch_declaration_id;
            *next_switch_declaration_id = (*next_switch_declaration_id).wrapping_add(1);
            pending_switch_declarations.push_back(PendingSwitchDeclaration {
                declaration_id,
                target_slot: slot,
                held_origin_slot: rollback_slot,
                rollback_slot,
                rollback_last_weapon_slot,
            });
            client.send_switch_declaration(ClientSwitchDeclaration {
                declaration_id,
                slot,
            });
        }
    }

    fn take_switch_outcome(&mut self, outcome: SwitchOutcome) -> CurrentSwitchResolution {
        let Self::Client {
            pending_switch_declarations,
            ..
        } = self
        else {
            return CurrentSwitchResolution::None;
        };
        resolve_switch_outcome(pending_switch_declarations, outcome)
    }

    /// Construct the endpoint for `role`, or `Ok(None)` for single-player.
    ///
    /// The netcode clock origin is `SystemTime::now()` since the unix epoch
    /// (`NetServer::new`/`NetClient::new` contract). Client user data is carried
    /// unchanged into the immutable netcode authentication token. Returns the
    /// transport error for the caller to log and fall back to single-player.
    pub(crate) fn from_role(
        role: &NetRole,
        user_data: Option<[u8; NETCODE_USER_DATA_BYTES]>,
    ) -> Result<Option<NetEndpoint>, String> {
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
                    world_items: std::collections::HashSet::new(),
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
                let client = NetClient::new(socket, *addr, client_id, now(), None, user_data)
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
                    next_switch_declaration_id: 0,
                    pending_switch_declarations: VecDeque::new(),
                    session_status: ClientSessionStatus::default(),
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
            pending_switch_declarations,
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
        pending_switch_declarations.clear();
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
            world_items,
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
        world_items.clear();
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
            pending_switch_declarations,
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
        pending_switch_declarations.clear();
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
                tuning.as_deref(),
                *tuning_generation,
                applied_movement_tuning_generation,
                descriptors,
                registry,
            );
        } else if tuning
            .as_deref()
            .and_then(|payload| payload.movement.as_ref())
            .is_none()
        {
            *applied_movement_tuning_generation = *tuning_generation;
        }
        if let Err(error) = result {
            log::error!("[Net] tuning payload epoch/decode failure: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_roster_retains_and_projects_open_seats_to_ui_state() {
        let mut session_status = ClientSessionStatus::default();
        let mut slot_table = SlotTable::new();
        let roster = SessionRosterMessage {
            session_id: postretro_net::wire::SessionId([0x52; 16]),
            your_seat: Some(3),
            open_seats: 9,
            entries: vec![postretro_net::wire::RosterEntry {
                seat: 3,
                connected: true,
            }],
        };

        let (changed, open_seats) =
            apply_client_session_roster(&mut session_status, &mut slot_table, roster.clone());

        assert!(changed);
        assert_eq!(open_seats, 9);
        assert_eq!(session_status.open_seats(), Some(9));
        assert_eq!(
            slot_table
                .get(SESSION_OPEN_SEATS_SLOT)
                .and_then(|record| record.value.as_ref()),
            Some(&postretro_entities::SlotValue::Number(9.0)),
            "the UI snapshot source retains the admitted host's open-seat count"
        );
        assert!(
            !apply_client_session_roster(&mut session_status, &mut slot_table, roster).0,
            "an identical reliable roster does not create a second presentation change"
        );
    }

    #[test]
    fn world_less_poll_advances_both_roles_without_touching_registry_state() {
        use std::net::{Ipv4Addr, SocketAddr};

        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform {
            position: Vec3::new(3.0, 2.0, 1.0),
            ..Transform::default()
        });

        let mut host = NetEndpoint::from_role(&NetRole::Host { port: 0 }, None)
            .expect("host endpoint constructs")
            .expect("host role yields an endpoint");
        assert!(matches!(
            host.poll_world_less(Duration::from_millis(16), &mut registry),
            WorldLessPoll::Host(_)
        ));

        let mut client = NetEndpoint::from_role(
            &NetRole::Connect {
                addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 1)),
            },
            None,
        )
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

    #[test]
    fn client_level_unload_resets_state_slot_apply_state() {
        let mut endpoint = NetEndpoint::from_role(
            &NetRole::Connect {
                addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 1)),
            },
            None,
        )
        .expect("client endpoint constructs")
        .expect("connect role yields an endpoint");
        let mut slot_table = SlotTable::new();
        let replication_identity = state_slots::ReplicatedSlotIdentity::default();
        let schema = state_slots::ReplicatedSlotSchema::build(&slot_table, &replication_identity);
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
        let applied = state_slots.apply_snapshot_state(
            &mut slot_table,
            &replication_identity,
            1,
            &fingerprint,
            &[record],
        );
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

    #[test]
    fn host_level_reset_preserves_session_monotonic_network_ids() {
        let mut registry = EntityRegistry::new();
        let old_entity = registry.spawn(Transform::default());
        let new_entity = registry.spawn(Transform::default());
        let mut endpoint = NetEndpoint::from_role(&NetRole::Host { port: 0 }, None)
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
}
