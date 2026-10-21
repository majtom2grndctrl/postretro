// Relay link layer for the prediction/reconciliation harness: a renet relay pair
// over two `PacketConditioner`s, stepped in production frame order on a virtual clock.
// See: context/lib/networking.md §Testing the conditioned link · context/lib/testing_guide.md §4
//
// Test infrastructure only. Unlike `LoopbackHarness`, nothing here stands in for the
// transport: input rides `NetClient::send_input` framed by participation, acks and
// time sync ride the reliable-ordered Input channel at the shipped resend, snapshots
// ride the Snapshot channel, and the host queue starts at its production buildup
// latch with no harness warmup.

#![cfg(test)]

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use postretro_collision::{EnvironmentView, EnvironmentVolumes};
use postretro_combat_model::{EnvironmentTuning, FluidMovementTable, TuningPayload};
use postretro_entities::{
    EntityId, EntityRegistry, EntityTypeDescriptor, FactionRegistry, FactionSentimentState,
    SlotTable, Transform,
};
use postretro_foundation::{MovementStateKind, PlayerMovementComponent, VaultParams};
use postretro_net::harness::{LinkConfig, PacketConditioner, ScriptedDrop, VirtualMillis};
use postretro_net::replication::ServerReplication;
use postretro_net::slots::SlotEvent;
use postretro_net::transport::{NetClient, NetServer};
use postretro_net::wire::ServerControlMessage;

use super::client::{ClientReplication, NoPresentationBindings};
use super::command_queue::{
    HostCommandQueues, MovementOwners, WeaponOwners, host_resolve_remote_commands_in_registry,
};
use super::endpoint::{ClientSessionStatus, ClientTimeSync, LocalCorrectionSample, NetEndpoint};
use super::interpolation::InterpolationDelayState;
use super::predict_reconcile_harness_test_fixtures::{
    CLIENT_ID, DT, GRAVITY, HostTickTrace, START, component, entity_descriptors, floor_world,
    net_slot_provenance,
};
use super::prediction::ClientPrediction;
use super::replication::ReplicableSet;
use super::state_slots::{ClientStateApply, HostStateReplication, ReplicatedSlotIdentity};
use super::{
    ClientPredictionTickContext, NetworkIdAllocator, OpenAuthorizedShots, PendingHitDeclarations,
    SERVER_TICK_MICROS, client_decay_local_correction, client_drive_time_sync, client_predict_tick,
    client_receive_and_apply, complete_host_fixed_tick, host_handle_client_messages,
    host_replicate, host_send_tuning_if_changed, tuning_payload_for_pawn,
};
use crate::collision::CollisionWorld;
use crate::collision::moving::{CombinedCollisionWorld, MoverCollider};
use crate::kinematic_mover::{MoverTickStateTable, run_kinematic_mover_tick};
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::sim::SimCommand;

/// The owner's dev lag proxy preset (`networking.md` §Testing the conditioned
/// link), for one direction: 45 ms + up to 60 ms jitter + 5% loss.
pub(crate) fn dev_profile(seed: u64) -> LinkConfig {
    LinkConfig {
        delay: 45,
        jitter: 60,
        loss_probability: 0.05,
        seed,
    }
}

/// An arbitrary origin for the client's monotonic clock, unrelated to the host
/// tick, so time sync must recover the offset rather than read it.
const CLIENT_CLOCK_ORIGIN_US: u64 = 7_000_000;

/// Virtual time at the start of frame `frame` of a 60 Hz run, in microseconds.
pub(crate) fn frame_start_us(frame: u64) -> u64 {
    frame * 1_000_000 / 60
}

/// One own-pawn reconcile the client ran, with the frame it ran in and the
/// pawn's movement state after that frame's apply.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClientCorrection {
    pub(crate) frame: u64,
    pub(crate) sample: LocalCorrectionSample,
    pub(crate) movement_state: Option<MovementStateKind>,
}

/// A host and a predicting client joined over a renet relay pair. One frame
/// ([`Self::frame`]) runs the host's frame then the client's at the same
/// virtual time, each polling its endpoint first; what a frame sends leaves at
/// the next frame's poll, as the production transport flushes at poll.
pub(crate) struct RelayHarness {
    pub(crate) world: CollisionWorld,
    pub(crate) mover_colliders: Vec<MoverCollider>,
    pub(crate) descriptors: Vec<EntityTypeDescriptor>,
    pub(crate) host_level_gravity: f32,
    host_environment_volumes: EnvironmentVolumes,
    host_fluid_table: FluidMovementTable,
    client_environment_volumes: EnvironmentVolumes,

    // --- Host ---
    pub(crate) server: NetServer,
    pub(crate) host_registry: EntityRegistry,
    pub(crate) host_pawn: EntityId,
    host_mover_states: MoverTickStateTable,
    pub(crate) command_queues: HostCommandQueues,
    pub(crate) owners: MovementOwners,
    weapon_owners: WeaponOwners,
    replicable: ReplicableSet,
    allocator: NetworkIdAllocator,
    server_replication: ServerReplication,
    server_state: HostStateReplication,
    pending_hit_declarations: PendingHitDeclarations,
    open_shots: OpenAuthorizedShots,
    host_slot_table: SlotTable,
    host_faction_sentiment: FactionSentimentState,
    host_last_sent_tuning: HashMap<u64, TuningPayload>,
    /// Completed host fixed ticks: the time-sync echo's server tick.
    pub(crate) host_tick: u32,
    last_emitted_snapshot_tick: Option<u32>,
    /// One record per host fixed tick: how the client's command resolved and
    /// the pawn's state after the tick.
    pub(crate) host_trace: Vec<HostTickTrace>,

    // --- Client ---
    /// The production client endpoint, its transport the relay `NetClient`
    /// and its time sync on the virtual clock.
    pub(crate) endpoint: NetEndpoint,
    pub(crate) client_registry: EntityRegistry,
    client_mover_states: MoverTickStateTable,
    client_slot_table: SlotTable,
    client_factions: FactionRegistry,
    client_faction_sentiment: RefCell<FactionSentimentState>,
    hit_zone_store: HitZoneStore,
    /// The client pawn, once its first baseline armed prediction.
    pub(crate) client_pawn: Option<EntityId>,
    /// Every own-pawn reconcile, in order.
    pub(crate) corrections: Vec<ClientCorrection>,
    /// The `client_tick` of the client's latest predicted command.
    pub(crate) last_client_tick: Option<u32>,

    // --- Link and clock ---
    pub(crate) to_server: PacketConditioner,
    pub(crate) to_client: PacketConditioner,
    client_clock_us: Arc<AtomicU64>,
    /// Frames run so far.
    pub(crate) frame: u64,
    frame_dt: Duration,
}

impl RelayHarness {
    /// A host whose pawn stands at [`START`] on [`floor_world`], and a client
    /// connected to it over `to_server` / `to_client` that has not yet joined:
    /// the first frames run its admission, parity and reveal on Control.
    pub(crate) fn new(to_server: LinkConfig, to_client: LinkConfig) -> Self {
        let descriptors = entity_descriptors();

        let mut host_registry = EntityRegistry::new();
        let host_pawn = host_registry.spawn(Transform {
            position: START,
            ..Transform::default()
        });
        host_registry.set_component(host_pawn, component()).unwrap();
        host_registry
            .set_component(host_pawn, net_slot_provenance())
            .unwrap();
        let mut owners = MovementOwners::new();
        owners.set(host_pawn, CLIENT_ID);
        let mut replicable = ReplicableSet::new();
        replicable.register(host_pawn);
        let mut allocator = NetworkIdAllocator::new();
        allocator.stamp(host_pawn);

        let (mut server, mut client) = relay_pair();
        server.add_relay_connection(CLIENT_ID, None);
        client.set_connected();

        let client_clock_us = Arc::new(AtomicU64::new(CLIENT_CLOCK_ORIGIN_US));
        let endpoint = NetEndpoint::Client {
            client: Box::new(client),
            replication: Box::new(ClientReplication::new()),
            time_sync: Box::new(ClientTimeSync::with_virtual_clock(Arc::clone(
                &client_clock_us,
            ))),
            interpolation_delay: InterpolationDelayState::new(),
            prediction: ClientPrediction::new(),
            state_slots: Box::new(ClientStateApply::new()),
            tuning: None,
            tuning_generation: 0,
            applied_movement_tuning_generation: 0,
            next_switch_declaration_id: 0,
            pending_switch_declarations: VecDeque::new(),
            session_status: ClientSessionStatus::default(),
        };

        Self {
            world: floor_world(),
            mover_colliders: Vec::new(),
            descriptors,
            host_level_gravity: GRAVITY,
            host_environment_volumes: EnvironmentVolumes::empty(),
            host_fluid_table: FluidMovementTable::default(),
            client_environment_volumes: EnvironmentVolumes::empty(),
            server,
            host_registry,
            host_pawn,
            host_mover_states: MoverTickStateTable::default(),
            command_queues: HostCommandQueues::new(),
            owners,
            weapon_owners: WeaponOwners::new(),
            replicable,
            allocator,
            server_replication: ServerReplication::new(),
            server_state: HostStateReplication::new(),
            pending_hit_declarations: PendingHitDeclarations::new(),
            open_shots: OpenAuthorizedShots::new(),
            host_slot_table: SlotTable::new(),
            host_faction_sentiment: FactionSentimentState::default(),
            host_last_sent_tuning: HashMap::new(),
            host_tick: 0,
            last_emitted_snapshot_tick: None,
            host_trace: Vec::new(),
            endpoint,
            client_registry: EntityRegistry::new(),
            client_mover_states: MoverTickStateTable::default(),
            client_slot_table: SlotTable::new(),
            client_factions: FactionRegistry::default(),
            client_faction_sentiment: RefCell::new(FactionSentimentState::default()),
            hit_zone_store: HitZoneStore::new(),
            client_pawn: None,
            corrections: Vec::new(),
            last_client_tick: None,
            to_server: PacketConditioner::new(to_server),
            to_client: PacketConditioner::new(to_client),
            client_clock_us,
            frame: 0,
            frame_dt: Duration::ZERO,
        }
    }

    /// Give the shared player descriptor a vault block and rematerialize the
    /// host pawn from it, before the client joins.
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

    /// The client pawn's predicted movement state, once armed.
    pub(crate) fn client_movement_state(&self) -> Option<MovementStateKind> {
        let pawn = self.client_pawn?;
        self.client_registry
            .get_component::<PlayerMovementComponent>(pawn)
            .ok()
            .map(|movement| movement.movement_state.kind())
    }

    /// One 60 Hz frame: the host's frame, then the client's, each running one
    /// fixed tick. The client predicts and sends `command`.
    pub(crate) fn frame(&mut self, command: &SimCommand) {
        self.advance_clock();
        self.host_frame();
        self.client_frame(command);
    }

    /// Run frames of `command` until the client predicts with an armed pawn and
    /// the host has resolved one of its commands for real, or `max_frames`
    /// pass. Returns whether it got there.
    pub(crate) fn run_until_predicting(&mut self, command: &SimCommand, max_frames: u64) -> bool {
        for _ in 0..max_frames {
            self.frame(command);
            let resolved_real = self.host_trace.iter().any(|trace| {
                trace.resolved.as_ref().is_some_and(|resolved| {
                    resolved.source == super::command_queue::ResolutionSource::Real
                })
            });
            if resolved_real && self.client_pawn.is_some() && self.client_tuning().is_some() {
                return true;
            }
        }
        false
    }

    /// The client's time sync: its clock and estimator.
    pub(crate) fn time_sync(&self) -> &ClientTimeSync {
        match &self.endpoint {
            NetEndpoint::Client { time_sync, .. } => time_sync,
            NetEndpoint::Host { .. } => unreachable!("the relay endpoint is a client"),
        }
    }

    /// The client's monotonic clock now, in microseconds.
    pub(crate) fn client_clock_us(&self) -> u64 {
        self.client_clock_us.load(Ordering::Relaxed)
    }

    /// Both peers still hold the slot participating: nothing disconnected it.
    pub(crate) fn both_participating(&self) -> bool {
        let NetEndpoint::Client { client, .. } = &self.endpoint else {
            unreachable!("the relay endpoint is a client");
        };
        client.is_participating() && self.server.is_participating(CLIENT_ID)
    }

    /// Cut both directions for `duration_ms` from now: every datagram either
    /// peer flushes in that window is lost, retransmissions included. Returns
    /// the window in conditioner milliseconds.
    pub(crate) fn script_outage(
        &mut self,
        duration_ms: VirtualMillis,
    ) -> (VirtualMillis, VirtualMillis) {
        let start = self.to_server.now();
        debug_assert_eq!(start, self.to_client.now(), "both links share one clock");
        let end = start + duration_ms;
        for link in [&mut self.to_server, &mut self.to_client] {
            link.script_drop(ScriptedDrop::Window { start, end });
        }
        (start, end)
    }

    /// The client's installed host tuning.
    pub(crate) fn client_tuning(&self) -> Option<&TuningPayload> {
        match &self.endpoint {
            NetEndpoint::Client { tuning, .. } => tuning.as_deref(),
            NetEndpoint::Host { .. } => None,
        }
    }

    /// Advance the virtual clock one 60 Hz frame. The conditioners keep whole
    /// milliseconds; the transports and the client clock see the exact frame.
    fn advance_clock(&mut self) {
        let previous_us = frame_start_us(self.frame);
        self.frame += 1;
        let now_us = frame_start_us(self.frame);
        let elapsed_ms: VirtualMillis = now_us / 1000 - previous_us / 1000;
        self.to_server.advance(elapsed_ms);
        self.to_client.advance(elapsed_ms);
        self.client_clock_us
            .store(CLIENT_CLOCK_ORIGIN_US + now_us, Ordering::Relaxed);
        self.frame_dt = Duration::from_micros(now_us - previous_us);
    }

    /// The host frame in `net_poll_and_apply` → fixed tick →
    /// `net_serialize_and_send` order.
    fn host_frame(&mut self) {
        // Poll: the relay half of `NetServer::update`, which flushes everything
        // queued since the previous poll.
        self.server.update_connections(self.frame_dt);
        for packet in self.to_server.take_ready() {
            self.server.process_packet_from(&packet, CLIENT_ID);
        }
        let poll = self.server.poll_handshakes();
        self.to_client
            .enqueue_all(self.server.packets_to_send(CLIENT_ID));
        for event in &poll.lifecycle {
            // The pawn already exists; entry registers the slot for replication
            // and sends its tuning, as production's participation entry does.
            if let SlotEvent::Participating { client_id } = event
                && self.server.is_current_participation_entry(event)
            {
                self.server_replication.register_client(*client_id);
                self.server_state.register_client(*client_id);
                self.host_send_tuning_if_changed();
            }
        }
        let server_tick = self.host_tick;
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

        let mut snapshot_due = false;
        self.host_fixed_tick();
        complete_host_fixed_tick(&mut self.host_tick, &mut snapshot_due);

        // Serialize: changed tuning queues ahead of the snapshot it shapes.
        self.host_send_tuning_if_changed();
        host_replicate(
            &self.host_registry,
            &self.host_slot_table,
            &self.host_faction_sentiment,
            &ReplicatedSlotIdentity::default(),
            &mut self.server,
            &mut self.allocator,
            &mut self.server_replication,
            &mut self.server_state,
            &self.replicable,
            &self.owners,
            &self.weapon_owners,
            &self.command_queues,
            None,
            self.host_tick.wrapping_add(1),
            0.0,
            snapshot_due,
            &mut self.last_emitted_snapshot_tick,
            &Default::default(),
        );
    }

    /// The host's movement tick for the client pawn: one command resolved by the
    /// gap policy, then the multi-pawn movement seam.
    fn host_fixed_tick(&mut self) {
        self.host_registry.snapshot_transforms();
        run_kinematic_mover_tick(&mut self.host_registry, &mut self.host_mover_states, DT);
        let resolved = host_resolve_remote_commands_in_registry(
            &self.owners,
            &mut self.command_queues,
            &self.host_registry,
        );
        let pawn_inputs = resolved
            .iter()
            .map(|resolved| (resolved.pawn, resolved.command.movement.clone()))
            .collect::<Vec<_>>();
        let collision = CombinedCollisionWorld::new(
            &self.world,
            &self.mover_colliders,
            &self.host_mover_states,
        );
        crate::sim::run_host_movement_tick(
            &mut self.host_registry,
            &collision,
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
            self.host_tick,
            resolved.first().cloned(),
        ));
    }

    fn host_send_tuning_if_changed(&mut self) {
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
        host_send_tuning_if_changed(
            &mut self.server,
            &mut self.host_last_sent_tuning,
            CLIENT_ID,
            payload,
        );
    }

    /// The client frame in production order: the transport poll, Control, time
    /// sync, snapshot receive and reconcile, then the fixed tick that predicts
    /// and sends `command` and decays the presentation offset once.
    fn client_frame(&mut self, command: &SimCommand) {
        let controls = {
            let NetEndpoint::Client { client, .. } = &mut self.endpoint else {
                unreachable!("the relay endpoint is a client");
            };
            // The relay half of `NetClient::update`.
            client.update_connections(self.frame_dt);
            for packet in self.to_client.take_ready() {
                client.process_packet(&packet);
            }
            self.to_server.enqueue_all(client.packets_to_send());
            client.drain_control()
        };
        for control in controls {
            if let ServerControlMessage::Tuning(bytes) = control {
                self.endpoint.install_tuning_payload(
                    &bytes,
                    &mut self.client_registry,
                    &self.descriptors,
                );
            }
        }

        self.client_receive();

        self.client_registry.snapshot_transforms();
        run_kinematic_mover_tick(&mut self.client_registry, &mut self.client_mover_states, DT);
        {
            let NetEndpoint::Client {
                client,
                prediction,
                tuning,
                ..
            } = &mut self.endpoint
            else {
                unreachable!("the relay endpoint is a client");
            };
            let collision = CombinedCollisionWorld::new(
                &self.world,
                &self.mover_colliders,
                &self.client_mover_states,
            );
            let result = client_predict_tick(
                &mut self.client_registry,
                client,
                prediction,
                ClientPredictionTickContext {
                    command,
                    collision: &collision,
                    environment_volumes: &self.client_environment_volumes,
                    host_tuning: tuning.as_deref(),
                    tick_dt: DT,
                },
            );
            self.last_client_tick = Some(result.client_tick);
        }
        client_decay_local_correction(Some(&mut self.endpoint));
    }

    /// Time sync, then every snapshot through `client_receive_and_apply`, whose
    /// acks and refresh requests go out on Input.
    fn client_receive(&mut self) {
        let NetEndpoint::Client {
            client,
            replication,
            time_sync,
            prediction,
            state_slots,
            tuning,
            tuning_generation,
            applied_movement_tuning_generation,
            ..
        } = &mut self.endpoint
        else {
            unreachable!("the relay endpoint is a client");
        };
        // The client's local tick is its frame counter, as the engine's is.
        let _shot_facts = client_drive_time_sync(client, time_sync, self.frame as u32);
        let mover_target_tick = time_sync
            .estimated_server_tick()
            .map(|tick| tick.floor().clamp(0.0, f64::from(u32::MAX)) as u32);
        let collision = CombinedCollisionWorld::new(
            &self.world,
            &self.mover_colliders,
            &self.client_mover_states,
        );
        let outcome = client_receive_and_apply(
            &mut self.client_registry,
            &mut self.client_slot_table,
            &self.client_factions,
            &self.client_faction_sentiment,
            &ReplicatedSlotIdentity::default(),
            client,
            replication,
            state_slots,
            prediction,
            &self.descriptors,
            &self.hit_zone_store,
            None,
            &collision,
            &self.client_environment_volumes,
            DT,
            self.frame_dt,
            mover_target_tick,
            tuning
                .as_deref()
                .and_then(|payload| payload.movement.as_ref()),
            tuning.as_deref(),
            *applied_movement_tuning_generation != *tuning_generation,
            &mut NoPresentationBindings,
            0.0,
            time_sync.rtt_micros(),
        );
        if let Some(armed) = &outcome.armed_local_pawn {
            self.client_pawn = Some(armed.entity_id);
            if tuning
                .as_deref()
                .and_then(|payload| payload.movement.as_ref())
                .is_some()
            {
                *applied_movement_tuning_generation = *tuning_generation;
            }
        }
        let movement_state = self.client_pawn.and_then(|pawn| {
            self.client_registry
                .get_component::<PlayerMovementComponent>(pawn)
                .ok()
                .map(|movement| movement.movement_state.kind())
        });
        let frame = self.frame;
        self.corrections.extend(
            outcome
                .local_corrections
                .iter()
                .map(|&sample| ClientCorrection {
                    frame,
                    sample,
                    movement_state,
                }),
        );
    }
}

/// A server/client transport pair joined by in-memory relay, both declaring
/// the same mod, level and reveal so the slot will participate.
pub(crate) fn relay_pair() -> (NetServer, NetClient) {
    let origin = Duration::from_secs(1);
    let server_socket =
        UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("relay server binds loopback socket");
    let server_addr: SocketAddr = server_socket
        .local_addr()
        .expect("relay server resolves loopback address");
    let client_socket =
        UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("relay client binds loopback socket");
    let fingerprint = [0x5a; 32];
    let mut server = NetServer::new(server_socket, server_addr, 1, origin, Some(fingerprint))
        .expect("relay server transport constructs");
    let mut client = NetClient::new(
        client_socket,
        server_addr,
        CLIENT_ID,
        origin,
        Some(fingerprint),
        None,
    )
    .expect("relay client transport constructs");
    server.set_mod_identity("test.mod".to_string(), "1.0.0".to_string());
    client.set_mod_identity("test.mod".to_string(), "1.0.0".to_string());
    server.set_mod_digest(Some(fingerprint));
    client.set_mod_digest(Some(fingerprint));
    server.set_level_parity(Some(("test-level".to_string(), fingerprint)));
    client.set_level_parity(Some(("test-level".to_string(), fingerprint)));
    postretro_net::harness::reveal_both(&mut server, &mut client, "test-level");
    (server, client)
}
