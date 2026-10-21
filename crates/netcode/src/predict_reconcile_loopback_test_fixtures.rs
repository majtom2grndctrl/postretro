// `LoopbackHarness`: a host and a predicting client over the relay link layer
// (`predict_reconcile_relay_link_test_fixtures`), stepped one tick at a time through
// the genuine production seams (client predict + framed Input send -> conditioner ->
// host Input drain/sanitize/queue/resolve/tick/snapshot -> conditioner -> client
// reconcile/smooth). Acks, refresh requests and time sync ride the reliable-ordered
// Input channel; tuning rides Control.
// See: context/lib/networking.md §Testing the conditioned link · context/lib/testing_guide.md §4
//
// Test infrastructure only (`#[cfg(test)]`). Its scenario worlds and movers are in
// `predict_reconcile_harness_world_test_fixtures`; its data builders in
// `predict_reconcile_harness_test_fixtures`.

#![cfg(test)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use glam::Vec3;
use postretro_collision::{EnvironmentView, EnvironmentVolumes};
use postretro_combat_model::{EnvironmentTuning, FluidMovementTable, TuningPayload};
use postretro_entities::{
    EntityId, EntityRegistry, EntityTypeDescriptor, KinematicMoverComponent, Transform,
};
use postretro_foundation::{FluidRegistry, GroundRef, PlayerMovementComponent};
use postretro_net::harness::{LinkConfig, PacketConditioner, VirtualMillis};
use postretro_net::replication::{EntitySnapshot, ServerReplication};
use postretro_net::slots::SlotEvent;
use postretro_net::transport::{Channel, NetClient, NetServer};
use postretro_net::wire::{self, ClientMessage, NetworkId, ServerControlMessage};

use super::client::{ClientReplication, MoverCorrection, encode_client_messages};
use super::command_queue::{
    HostCommandQueues, MovementOwners, ResolutionSource, host_resolve_remote_commands_in_registry,
};
use super::endpoint::ClientTimeSync;
use super::predict_reconcile_harness_test_fixtures::{
    CLIENT_ID, DRAIN_SETTLE_TICKS, DT, GRAVITY, HostTickTrace, MOVING_PLATFORM_ID,
    NamedEnvironmentVolume, START, component, entity_descriptors, floor_world, net_slot_provenance,
    resolve_environment_records, spawn_dead_bystander,
};
use super::predict_reconcile_relay_link_test_fixtures::{
    CLIENT_CLOCK_ORIGIN_US, DatagramDrop, carries_tuning, flush, frame_start_us, relay_pair,
};
use super::prediction::{ClientPrediction, CorrectionClass};
use super::reconcile::reconcile_local_pawn_with_mover_history;
use super::replication::{ReplicableSet, produce_owned_snapshots};
use super::{
    OpenAuthorizedShots, PendingHitDeclarations, SERVER_TICK_MICROS, client_drive_time_sync,
    client_predict_tick, decode_snapshot, host_handle_client_messages,
    host_send_tuning_if_changed as send_tuning_if_changed,
};
use crate::collision::CollisionWorld;
use crate::collision::moving::{CombinedCollisionWorld, MoverCollider};
use crate::kinematic_mover::{
    MoverTickStateTable, run_kinematic_mover_tick, run_mover_blocking_pass, run_mover_pin_pretest,
};
use crate::movement::MovementEvents;
use crate::netcode::{
    ClientPredictionTickContext, NetworkIdAllocator, apply_installed_movement_tuning_to_armed_pawn,
    client_environment_view, client_fluid_table, replace_client_tuning, stamped_client_input,
    tuning_payload_for_pawn,
};
use crate::scripting::builtins::net_descriptor::materialize_net_local_movement_component;
use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;
use crate::sim::SimCommand;
use crate::trigger_system::{AuthoritativePlayer, PlayerId, TriggerSystem};

/// The two-sided, deterministic harness driving the genuine production seams over a
/// renet relay pair on the production channel configuration. One conditioner per
/// direction carries every datagram the transports flush; a virtual 60 Hz frame
/// clock the harness advances drives delivery, renet's resend timers and the
/// client's time-sync clock. No wall-clock time is ever read.
///
/// Each peer polls its transport at the start of its frame, which flushes what it
/// queued since the previous poll: the host in [`Self::host_poll`], the client in
/// [`Self::client_receive`]. A command [`Self::client_predict_and_send`] queues
/// therefore leaves at the next step's client receive, as production's does.
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
    /// The host's relay transport. Its slot admits, matches parity and reveals
    /// over Control in the first frames; its Input drain is the production
    /// participation-framed one.
    pub(crate) server: NetServer,
    pub(crate) command_queues: HostCommandQueues,
    pub(crate) owners: MovementOwners,
    pub(crate) replicable: ReplicableSet,
    pub(crate) allocator: NetworkIdAllocator,
    pub(crate) host_loaded_movers: HashSet<EntityId>,
    pub(crate) server_replication: ServerReplication,
    /// M15 Phase 3.5 host state tracker. The movement harness drives no
    /// replicated state slots, so it holds only the client's registration.
    pub(crate) server_state: super::state_slots::HostStateReplication,
    pending_hit_declarations: PendingHitDeclarations,
    open_shots: OpenAuthorizedShots,
    pub(crate) server_tick: u32,
    pub(crate) snapshot_sequence: u32,
    /// The production per-slot tuning change detector.
    pub(crate) host_last_sent_tuning: HashMap<u64, TuningPayload>,

    // --- Client side (predicting) ---
    /// The client's relay transport.
    pub(crate) client: NetClient,
    /// The client's time sync on the virtual clock: its server-tick estimate is
    /// the mover replay target, as the engine's is.
    pub(crate) time_sync: ClientTimeSync,
    client_clock_us: Arc<AtomicU64>,
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
    /// One record per host fixed tick: the resolved command (if any) and the
    /// client pawn's authoritative state after the tick.
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
    /// Drops host datagrams by content at their flush, on top of the link model.
    pub(crate) down_drop: Option<DatagramDrop>,
    /// While set, every datagram the client flushes is lost until the host has
    /// resolved through this client tick. On the reliable Input channel a
    /// command is never lost outright: it is lost to the host when every copy
    /// arrives after the host gave its tick up, which is this outage.
    uplink_cut_through: Option<u32>,
    /// Frames run so far; the client's local tick for time sync.
    pub(crate) frame: u64,
    host_polled_us: u64,
    client_polled_us: u64,

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
    /// Build the harness for a given link profile, applied in both directions. The
    /// host owns one descriptor-backed pawn registered as the client's movement
    /// authority; the client is connected over the relay but has not joined: the
    /// first frames run its admission, parity and reveal on Control, and the
    /// host registers it for replication on participation entry. Both registries
    /// also hold a zero-HP bystander.
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

        let (mut server, mut client) = relay_pair();
        server.add_relay_connection(CLIENT_ID, None);
        client.set_connected();
        let client_clock_us = Arc::new(AtomicU64::new(CLIENT_CLOCK_ORIGIN_US));
        let time_sync = ClientTimeSync::with_virtual_clock(Arc::clone(&client_clock_us));

        let command_queues = HostCommandQueues::new();
        let mut owners = MovementOwners::new();
        owners.set(host_pawn, CLIENT_ID);
        let mut replicable = ReplicableSet::new();
        replicable.register(host_pawn);

        let mut allocator = NetworkIdAllocator::new();
        let host_pawn_network_id = allocator.stamp(host_pawn);

        // Client side: starts empty — the first applied baseline spawns + arms the
        // local pawn through the real apply path. The client bystander is local.
        let mut client_registry = EntityRegistry::new();
        let client_bystander = spawn_dead_bystander(&mut client_registry);

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
            server_replication: ServerReplication::new(),
            server_state: super::state_slots::HostStateReplication::new(),
            pending_hit_declarations: PendingHitDeclarations::new(),
            open_shots: OpenAuthorizedShots::new(),
            server_tick: 0,
            snapshot_sequence: 0,
            host_last_sent_tuning: HashMap::new(),
            client,
            time_sync,
            client_clock_us,
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
            down_drop: None,
            uplink_cut_through: None,
            frame: 0,
            host_polled_us: 0,
            client_polled_us: 0,
            last_sent_client_tick: None,
            client_acked_server_tick: 0,
            drain_target_tick: None,
            ticks_since_cursor_caught: 0,
        }
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

    /// CLIENT STEP: the production predicted tick (`client_predict_tick`): stamp the
    /// outbound `InputCommand` and queue it on the client's framed Input channel,
    /// then predict the local pawn one tick and write it back to the client registry.
    /// The queued command leaves at the client's next transport poll.
    pub(crate) fn client_predict_and_send(&mut self, command: &SimCommand) {
        self.client_predict_and_send_with_delivery(command, true);
    }

    /// [`Self::client_predict_and_send`], optionally losing the command to the
    /// host: the client predicts and sends it exactly as it would, but the uplink
    /// is cut until the host has resolved past its tick, so the host never runs
    /// it for real. Reliable Input delivers every later command behind it, late.
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
        let result = client_predict_tick(
            &mut self.client_registry,
            &mut self.client,
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
        // Production decays the presentation offset once per predicted fixed tick,
        // after the tick's camera pose bakes it in; without it, corrections would
        // compose across harness steps that never decay.
        self.prediction.decay_presentation_offset();

        if !deliver {
            self.uplink_cut_through = Some(
                self.uplink_cut_through
                    .map_or(result.client_tick, |through| {
                        through.max(result.client_tick)
                    }),
            );
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
        self.client_send_message(&ClientMessage::Input(input));
        self.last_sent_client_tick = Some(client_tick);
        self.drain_target_tick = None;
        self.ticks_since_cursor_caught = 0;
        client_tick
    }

    /// Queue one client message on the client's framed Input channel, as any
    /// client send does: a duplicated or malformed command reaches the host
    /// only through this production path.
    pub(crate) fn client_send_message(&mut self, message: &ClientMessage) {
        self.client.send_input(wire::encode(message));
    }

    /// HOST STEP: the host's transport poll (deliver due client datagrams, flush
    /// what the host queued last frame, run the slot lifecycle, drain each
    /// participating client's Input through the production dispatcher), then one
    /// authoritative fixed tick (resolve per-pawn movement via the gap policy, run
    /// the multi-pawn movement seam), then the send (changed tuning, then the
    /// snapshot). One host frame running one tick; [`Self::host_frame`] runs
    /// several before one send.
    pub(crate) fn host_tick(&mut self) {
        self.host_frame(1, |_, _| {});
    }

    /// One host frame in production order: the poll, `ticks` fixed ticks, then
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

    /// The host's frame-start transport poll, as `net_poll_and_apply` runs it:
    /// renet advances by the time since the last poll, due client datagrams are
    /// processed, everything the host queued since the last poll is flushed,
    /// participation entry registers the slot and sends its tuning, and each
    /// participating client's framed Input is drained and dispatched.
    pub(crate) fn host_poll(&mut self) {
        let now_us = frame_start_us(self.frame);
        self.server
            .update_connections(Duration::from_micros(now_us - self.host_polled_us));
        self.host_polled_us = now_us;
        for packet in self.to_server.take_ready() {
            self.server.process_packet_from(&packet, CLIENT_ID);
        }
        let poll = self.server.poll_handshakes();
        let packets = self.server.packets_to_send(CLIENT_ID);
        flush(
            &mut self.to_client,
            packets,
            self.frame,
            None,
            self.down_drop.as_mut(),
            None,
        );
        for event in &poll.lifecycle {
            match event {
                // The pawn already exists; entry registers the slot for
                // replication and sends its tuning, as production's
                // participation entry does.
                SlotEvent::Participating { client_id } => {
                    if self.server.is_current_participation_entry(event) {
                        self.server_replication.register_client(*client_id);
                        self.server_state.register_client(*client_id);
                        self.host_send_tuning_if_changed();
                    }
                }
                // The slot-scoped half of production's exit cleanup.
                SlotEvent::Demoted { client_id, .. } | SlotEvent::Closed { client_id, .. } => {
                    self.server_replication.remove_client(*client_id);
                    self.server_state.remove_client(*client_id);
                    self.command_queues.remove_client(*client_id);
                    self.host_last_sent_tuning.remove(client_id);
                }
            }
        }
        let server_tick = self.server_tick;
        for client_id in self.server.participating_clients() {
            host_handle_client_messages(
                &mut self.server,
                &mut self.server_replication,
                &mut self.server_state,
                &mut self.command_queues,
                &self.owners,
                &mut self.pending_hit_declarations,
                &mut self.open_shots,
                client_id,
                server_tick,
                u64::from(server_tick) * SERVER_TICK_MICROS,
            );
        }
    }

    fn host_simulate_tick(&mut self) {
        // Authoritative movement tick: resolve one command per owned pawn through
        // the deterministic gap policy (from its production buildup latch), then
        // advance the named pawns.
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
        let resolved_commands = host_resolve_remote_commands_in_registry(
            &self.owners,
            &mut self.command_queues,
            &self.host_registry,
        );
        let pawn_inputs = resolved_commands
            .iter()
            .map(|resolved| (resolved.pawn, resolved.command.movement.clone()))
            .collect::<Vec<_>>();
        let trigger_use_edges: HashMap<_, _> = resolved_commands
            .iter()
            .filter_map(|resolved| {
                resolved
                    .command
                    .use_pressed
                    .then_some((PlayerId::Remote(resolved.client_id), true))
            })
            .collect();
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
        if self.server.is_participating(CLIENT_ID) {
            self.host_send_tuning_if_changed();
        }

        // Produce + encode the authoritative snapshot through the REAL server
        // replication tracker (full per-client encode with movement-authority
        // metadata), then send it on the participation-framed Snapshot channel.
        // Before the slot participates nothing is sent, as `host_replicate` does.
        let owned: Vec<EntitySnapshot> = produce_owned_snapshots(
            &self.host_registry,
            &self.replicable,
            &mut self.allocator,
            &self.owners,
            &self.command_queues,
        );
        self.server_replication.ingest_tick(owned);
        if !self.server.is_participating(CLIENT_ID) {
            return;
        }
        let sequence = self.server_replication.begin_batch();
        self.snapshot_sequence = sequence;
        if let Some(raw) =
            self.server_replication
                .encode_in_batch(CLIENT_ID, self.server_tick, sequence)
        {
            self.server.send_snapshot(CLIENT_ID, wire::encode(&raw));
        }
    }

    /// The client's frame-start transport poll: renet advances by the time since
    /// the last poll, due host datagrams are processed, and everything the client
    /// queued since the last poll is flushed onto the uplink, unless the uplink is
    /// cut for a lost command.
    fn client_poll(&mut self) {
        let now_us = frame_start_us(self.frame);
        self.client
            .update_connections(Duration::from_micros(now_us - self.client_polled_us));
        self.client_polled_us = now_us;
        for packet in self.to_client.take_ready() {
            self.client.process_packet(&packet);
        }
        let packets = self.client.packets_to_send();
        if let Some(through) = self.uplink_cut_through {
            let given_up = self
                .command_queues
                .resolved_cursor(CLIENT_ID)
                .is_some_and(|cursor| cursor >= through);
            if !given_up {
                return;
            }
            self.uplink_cut_through = None;
        }
        flush(&mut self.to_server, packets, self.frame, None, None, None);
    }

    /// CLIENT STEP: the client's frame in production order: the transport poll,
    /// Control, time sync, then every snapshot through the real
    /// `ClientReplication` apply + `reconcile_local_pawn` path (arm-before-reconcile,
    /// exactly as `client_receive_and_apply`), whose acks and refresh requests are
    /// queued on Input. Returns the acks it sent, for inspection.
    pub(crate) fn client_receive(&mut self) -> Vec<wire::AckMessage> {
        self.latest_mover_corrections.clear();
        self.latest_local_corrections.clear();
        self.latest_local_correction_classes.clear();
        self.latest_full_baselines.clear();
        self.latest_reconcile_windows.clear();
        self.client_poll();
        // Control drains before snapshots, as `App::net_poll_and_apply` does.
        self.client_install_controls();
        // The client's local tick is its frame counter, as the engine's is.
        let _shot_facts =
            client_drive_time_sync(&mut self.client, &mut self.time_sync, self.frame as u32);
        let mover_target_tick = self
            .time_sync
            .estimated_server_tick()
            .map(|tick| tick.floor().clamp(0.0, f64::from(u32::MAX)) as u32);
        let mut acks = Vec::new();
        for bytes in self.client.drain_snapshots() {
            let Ok(snapshot) = decode_snapshot(&bytes) else {
                continue;
            };
            self.latest_full_baselines
                .extend(snapshot.records.iter().filter_map(|record| match record {
                    wire::EntityRecord::FullBaseline { network_id, .. } => Some(*network_id),
                    _ => None,
                }));
            let target_tick = mover_target_tick
                .unwrap_or(snapshot.server_tick)
                .max(snapshot.server_tick);
            let outcome = self
                .client_replication
                .apply_snapshot_with_mover_target_tick(
                    &mut self.client_registry,
                    &snapshot,
                    target_tick,
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
                // Installed host tuning with no movement leaves the pawn inert, as
                // production's `materialize_armed_local_pawn` does; every owner
                // record re-arms, so the shared-descriptor shortcut must not revive
                // it. Before any tuning installs, the harness still materializes
                // from the shared descriptors, where production stays inert.
                let inert = self
                    .client_tuning
                    .as_deref()
                    .is_some_and(|tuning| tuning.movement.is_none());
                if !inert {
                    materialize_net_local_movement_component(
                        entity_class,
                        &self.descriptors,
                        &mut self.client_registry,
                        armed.entity_id,
                    );
                }
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
            // The ack, then every refresh request apply raised, ride the uplink's
            // Input channel to the host, as `client_receive_and_apply` sends them.
            for buffer in encode_client_messages(&outcome) {
                self.client.send_input(buffer);
            }
            self.sent_refresh_requests
                .extend(outcome.refresh_requests.iter().copied());
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
        let unmerged = self
            .client_replication
            .refresh_unmerged_local_pawn(unmerged_wanted);
        let due = self.client_replication.tick_pending_repairs(DT * 1000.0);
        for request in unmerged.into_iter().chain(due) {
            self.sent_refresh_requests.push(request);
            self.client_send_message(&ClientMessage::BaselineRefresh(request));
        }
        acks
    }

    /// HOST: build this slot's tuning payload from the host pawn and the host's live
    /// level gravity and fluid table, and send it on Control when it differs from
    /// the last one sent (the production `host_send_tuning_if_changed`).
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
        send_tuning_if_changed(
            &mut self.server,
            &mut self.host_last_sent_tuning,
            CLIENT_ID,
            payload,
        );
    }

    /// CLIENT: a frame whose poll drains Control alone: the client's transport
    /// poll, then every due tuning payload installs. Snapshots that arrived stay
    /// queued for the next [`Self::client_receive`], so this is a frame in which
    /// Control lands and no snapshot is applied.
    pub(crate) fn client_drain_control(&mut self) {
        self.client_poll();
        self.client_install_controls();
    }

    /// Lose every host datagram that carries a tuning payload, its reliable
    /// resends included, until [`Self::land_tuning`]: a tuning send that waits
    /// on the Control lane for its retransmit.
    pub(crate) fn hold_tuning(&mut self) {
        self.down_drop = Some(Box::new(carries_tuning));
    }

    /// Release the held tuning and land its next retransmit between simulated
    /// frames, in client frames that drain Control and apply no snapshot.
    /// Neither peer ticks or predicts meanwhile; only the transports run.
    pub(crate) fn land_tuning(&mut self) {
        self.down_drop = None;
        for _ in 0..60 {
            if self.client_tuning.is_some() {
                return;
            }
            self.advance_clock();
            self.host_poll();
            self.client_drain_control();
        }
    }

    /// Install every drained tuning payload through the production
    /// `replace_client_tuning` path.
    fn client_install_controls(&mut self) {
        for control in self.client.drain_control() {
            let ServerControlMessage::Tuning(bytes) = control else {
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

    /// Advance the virtual clock one 60 Hz frame. The conditioners keep whole
    /// milliseconds; the transports and the client clock see the exact frame.
    pub(crate) fn advance_clock(&mut self) {
        let previous_us = frame_start_us(self.frame);
        self.frame += 1;
        let now_us = frame_start_us(self.frame);
        let elapsed_ms: VirtualMillis = now_us / 1000 - previous_us / 1000;
        self.to_server.advance(elapsed_ms);
        self.to_client.advance(elapsed_ms);
        self.client_clock_us
            .store(CLIENT_CLOCK_ORIGIN_US + now_us, Ordering::Relaxed);
    }

    /// Step with `command` until the client has armed its local pawn (joined, then
    /// received and applied its first `local_player` baseline through the real
    /// apply path), or a bounded number of steps elapse. Returns the number of
    /// steps taken. Scenario tests use this instead of a hard-coded loop count so
    /// they are robust to the join handshake and the link's round trip.
    pub(crate) fn step_until_armed(&mut self, command: &SimCommand) -> u32 {
        let mut steps = 0;
        while self.client_pawn.is_none() && steps < 200 {
            self.step(command);
            steps += 1;
        }
        steps
    }

    /// Step with `command` until the join has settled into steady prediction: the
    /// client has armed, the host has resolved one of its commands for real, and
    /// the client has reconciled against a host ack. Returns the steps taken.
    ///
    /// Until then the stream is starting: the host resolves nothing until its
    /// buildup latch fills, so it has run no movement tick for the pawn the
    /// client arms from, and acks nothing the client predicted. Scenarios that
    /// measure steady-state corrections start here.
    pub(crate) fn step_until_predicting(&mut self, command: &SimCommand) -> u32 {
        let mut steps = 0;
        let mut acked = false;
        while !acked && steps < 200 {
            self.step(command);
            steps += 1;
            let resolved_real = self.host_trace.iter().any(|trace| {
                trace
                    .resolved
                    .as_ref()
                    .is_some_and(|resolved| resolved.source == ResolutionSource::Real)
            });
            acked = resolved_real
                && self.client_pawn.is_some()
                && self
                    .latest_reconcile_windows
                    .iter()
                    .any(|(acked_tick, _)| acked_tick.is_some());
        }
        steps
    }

    /// Run frames that send no input until the slot participates on both peers:
    /// the join's admission, parity and reveal, and nothing on Input yet.
    /// Returns whether it got there within `max_frames`.
    pub(crate) fn join_without_input(&mut self, max_frames: u32) -> bool {
        for _ in 0..max_frames {
            if self.client.is_participating() && self.server.is_participating(CLIENT_ID) {
                return true;
            }
            self.advance_clock();
            self.host_poll();
            self.client_drain_control();
        }
        self.client.is_participating() && self.server.is_participating(CLIENT_ID)
    }

    /// One full simulated step: client predicts + queues its command, clock
    /// advances, host polls + ticks + snapshots, client polls (flushing the
    /// command and last step's acks) + receives + reconciles.
    pub(crate) fn step(&mut self, command: &SimCommand) {
        self.client_predict_and_send(command);
        self.advance_clock();
        self.host_tick();
        self.client_receive();
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
        self.client_receive();
    }

    /// A drain step that sends no new input. It has two regimes, picked automatically
    /// so the drain is robust under packet loss:
    ///
    /// 1. **Active drain (host keeps ticking + snapshotting):** until the client has
    ///    acked the frozen drain *target* tick, the host keeps fully ticking — it
    ///    resolves any in-flight input (advancing the cursor to `last_sent`, then
    ///    holding/neutral as the gap policy dictates), advances the pawn, and emits a
    ///    snapshot every tick. The target tick is frozen a settle margin after the
    ///    cursor reaches `last_sent`; the host's authoritative *pose* is then final,
    ///    so every later snapshot carries the same pose at an advancing `server_tick`.
    ///    A snapshot lost to the link is simply superseded by the next one.
    /// 2. **Settle (host quiescent):** once the client has acked the target, the host
    ///    stops ticking and snapshotting; it still polls, so straggler Input (stale,
    ///    dropped at intake) and time sync drain, and the client receives the last
    ///    snapshots in flight. [`Self::is_drained`] waits for those.
    pub(crate) fn drain_step(&mut self) {
        let acked_target = matches!(
            self.drain_target_tick,
            Some(target) if self.client_acked_server_tick >= target
        );

        self.advance_clock();
        if !acked_target {
            // Regime 1: host keeps ticking + snapshotting until the client acks the
            // frozen target. host_tick resolves queued input and advances the cursor.
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
            // Regime 2: host quiescent — no tick, no snapshot, but its transport
            // still polls and drains Input through the real dispatcher.
            self.host_poll();
        }

        self.client_receive();
    }

    /// The explicit drain condition: the host input queue is drained (its resolved
    /// cursor has reached the final sent command tick), the client has processed a
    /// snapshot acking that cursor (its acked server tick reached the frozen drain
    /// *target*), and no snapshot is still in flight to the client. Input still in
    /// flight carries only resolved commands, acks and time sync, which move nothing.
    pub(crate) fn is_drained(&self) -> bool {
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

        cursor_caught_up && client_acked_target && !self.snapshot_in_flight()
    }

    /// Whether a Snapshot datagram the host flushed has neither arrived nor been
    /// lost yet. Only the last two seconds of datagrams can still be in flight.
    fn snapshot_in_flight(&self) -> bool {
        let now = self.to_client.now();
        let count = self.to_client.enqueued_on_channel(Channel::Snapshot);
        (0..count)
            .rev()
            .map_while(|index| self.to_client.channel_datagram(Channel::Snapshot, index))
            .take_while(|fate| fate.enqueued_at + 2_000 >= now)
            .any(|fate| fate.arrives_at.is_some_and(|arrives_at| arrives_at > now))
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
