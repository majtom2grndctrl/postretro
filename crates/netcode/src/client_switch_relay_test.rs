// Switch declarations and outcomes on Input across demotion, re-promotion and level unload.
// See: context/lib/networking.md §Combat authority · §Slot lifecycle · context/lib/testing_guide.md
//
// A renet relay pair with the production client endpoint and the host's Input
// dispatch, switch lane and switch apply. Switches carry no framing of their own
// beyond Input's: a declaration rides Input's participation frame, so the
// host's Input drain drops one framed under a retired epoch; and demotion or level
// unload clears the client's chain while declaration ids never reset, so a stale
// outcome meets an empty chain or a newer front.

#![cfg(test)]

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use postretro_entities::components::inventory::Inventory;
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{EntityId, EntityRegistry, Transform};
use postretro_foundation::WeaponDescriptor;
use postretro_net::harness::{RenetDatagram, parse_renet_datagram, renet_packet_channel};
use postretro_net::replication::ServerReplication;
use postretro_net::slots::SlotEvent;
use postretro_net::transport::{Channel, NetServer};
use postretro_net::wire::{self, DivergenceReason, ServerControlMessage};

use super::command_queue::{HostCommandQueues, MovementOwners, ResolutionSource, WeaponOwners};
use super::endpoint::{ClientSessionStatus, ClientTimeSync, NetEndpoint};
use super::interpolation::InterpolationDelayState;
use super::predict_reconcile_harness_test_fixtures::{CLIENT_ID, input_at};
use super::predict_reconcile_relay_link_test_fixtures::{
    RELAY_FINGERPRINT, RELAY_LEVEL, decode_client_input, relay_pair,
};
use super::prediction::ClientPrediction;
use super::replication::ReplicableSet;
use super::state_slots::{ClientStateApply, HostStateReplication};
use super::{
    CurrentSwitchResolution, NetworkIdAllocator, OpenAuthorizedShots, PendingHitDeclarations,
    SlotPawns, client::ClientReplication, client_drive_time_sync, client_resolve_switch_outcomes,
    host_apply_switch_deliveries, host_handle_accept, host_handle_client_messages,
    host_handle_lifecycle, host_resolve_remote_commands,
};

const FRAME: Duration = Duration::from_micros(16_667);

/// More host ticks than a retained switch may wait before it applies whatever
/// is still pending (`SWITCH_WAIT_TICKS`), so a declaration that reached the
/// lane is answered within it.
const PAST_SWITCH_WAIT: u32 = 300;

fn rifle() -> WeaponComponent {
    let descriptor = serde_json::from_value::<WeaponDescriptor>(serde_json::json!({
        "damage": 10, "range": 96, "resolution": "hitscan",
        "primary": { "trigger": "press", "recoveryMs": 130, "steps": [{ "kind": "shot" }] },
    }))
    .expect("rifle fixture parses")
    .validate()
    .expect("rifle fixture validates");
    WeaponComponent::from_descriptor(&descriptor)
}

/// A host and a connected client over an in-memory relay. One frame runs the
/// host's poll, Input dispatch and fixed tick, then the client's Control drain,
/// Input drain and fixed tick, as production orders them.
struct SwitchRelay {
    server: NetServer,
    host_registry: EntityRegistry,
    allocator: NetworkIdAllocator,
    replicable: ReplicableSet,
    replication: ServerReplication,
    state_slots: HostStateReplication,
    slot_pawns: SlotPawns,
    command_queues: HostCommandQueues,
    owners: MovementOwners,
    weapon_owners: WeaponOwners,
    open_shots: OpenAuthorizedShots,
    pending_hits: PendingHitDeclarations,
    weaponless: HashSet<EntityId>,
    last_sent_tuning: HashMap<u64, postretro_combat_model::TuningPayload>,
    /// Client ticks of the commands the host resolved for real, in order.
    resolved_real: Vec<u32>,

    endpoint: NetEndpoint,
    client_registry: EntityRegistry,
    client_tick: u32,
    /// Whether the client sends an input command each frame.
    send_commands: bool,
    /// Every resolution the client's Input drain produced, one per outcome.
    resolutions: Vec<CurrentSwitchResolution>,
    /// Frames whose Control drain held the slot, with whether the client was
    /// participating again by the end of that drain.
    holdings: Vec<bool>,

    /// Client datagrams waiting for the host's next poll.
    up: Vec<Vec<u8>>,
    /// Host datagrams waiting for the client's next poll.
    down: Vec<Vec<u8>>,
    /// Drop every client datagram, as a dead uplink does.
    cut_up: bool,
    /// Hold every host datagram in flight, delivering the backlog in one
    /// client poll when lifted, as a delay spike does.
    hold_down: bool,
    /// Drop host datagrams on the Input channel only.
    cut_down_input: bool,
    /// Every switch declaration message the client flushed, cut or not, as
    /// `(renet message id, transport bytes)`.
    declarations_flushed: Vec<(u64, Vec<u8>)>,
    /// Every switch declaration message the host's poll took while the slot
    /// participated, so that frame's Input drain read it.
    declarations_drained: Vec<(u64, Vec<u8>)>,
    /// The declaration id of every switch outcome message the host flushed on
    /// Input, cut or not, resends included.
    outcomes_flushed: Vec<u32>,
    /// The host lane's retained switches right after the latest host frame's
    /// Input dispatch, before its tick can apply and remove one.
    retained_after_dispatch: usize,
}

/// The switch declaration messages a client datagram carries on Input, as
/// `(renet message id, transport bytes)`.
fn switch_declarations(packet: &[u8]) -> Vec<(u64, Vec<u8>)> {
    let Some(RenetDatagram::Reliable { channel, messages }) = parse_renet_datagram(packet) else {
        return Vec::new();
    };
    if channel != u8::from(Channel::Input) {
        return Vec::new();
    }
    messages
        .into_iter()
        .filter(|(_, bytes)| {
            matches!(
                decode_client_input(bytes),
                Some(wire::ClientMessage::SwitchDeclaration(_))
            )
        })
        .collect()
}

/// The declaration id of each switch outcome message a host datagram carries
/// on Input. Host Input carries no participation frame.
fn switch_outcome_ids(packet: &[u8]) -> Vec<u32> {
    let Some(RenetDatagram::Reliable { channel, messages }) = parse_renet_datagram(packet) else {
        return Vec::new();
    };
    if channel != u8::from(Channel::Input) {
        return Vec::new();
    }
    messages
        .iter()
        .filter_map(
            |(_, bytes)| match wire::decode::<wire::ServerMessage>(bytes).ok()? {
                wire::ServerMessage::SwitchAccepted(outcome) => Some(outcome.declaration_id),
                wire::ServerMessage::SwitchRefused(outcome) => Some(outcome.declaration_id),
                _ => None,
            },
        )
        .collect()
}

impl SwitchRelay {
    /// Join the client and run until its pawn is owned and its commands flow.
    fn joined() -> Self {
        let (mut server, mut client) = relay_pair();
        server.add_relay_connection(CLIENT_ID, None);
        client.set_connected();
        let endpoint = NetEndpoint::Client {
            client: Box::new(client),
            replication: Box::new(ClientReplication::new()),
            time_sync: Box::new(ClientTimeSync::new()),
            interpolation_delay: InterpolationDelayState::new(),
            prediction: ClientPrediction::new(),
            state_slots: Box::new(ClientStateApply::new()),
            tuning: None,
            tuning_generation: 0,
            applied_movement_tuning_generation: 0,
            next_switch_declaration_id: 0,
            pending_switch_declarations: std::collections::VecDeque::new(),
            session_status: ClientSessionStatus::default(),
        };
        let mut relay = Self {
            server,
            host_registry: EntityRegistry::new(),
            allocator: NetworkIdAllocator::new(),
            replicable: ReplicableSet::new(),
            replication: ServerReplication::new(),
            state_slots: HostStateReplication::new(),
            slot_pawns: SlotPawns::new(),
            command_queues: HostCommandQueues::new(),
            owners: MovementOwners::new(),
            weapon_owners: WeaponOwners::new(),
            open_shots: OpenAuthorizedShots::new(),
            pending_hits: PendingHitDeclarations::new(),
            weaponless: HashSet::new(),
            last_sent_tuning: HashMap::new(),
            resolved_real: Vec::new(),
            endpoint,
            client_registry: EntityRegistry::new(),
            client_tick: 0,
            send_commands: true,
            resolutions: Vec::new(),
            holdings: Vec::new(),
            up: Vec::new(),
            down: Vec::new(),
            cut_up: false,
            hold_down: false,
            cut_down_input: false,
            declarations_flushed: Vec::new(),
            declarations_drained: Vec::new(),
            outcomes_flushed: Vec::new(),
            retained_after_dispatch: 0,
        };
        relay.run_until_participating();
        relay.frames(10);
        relay
    }

    fn client(&mut self) -> &mut postretro_net::transport::NetClient {
        let NetEndpoint::Client { client, .. } = &mut self.endpoint else {
            unreachable!("the relay endpoint is a client");
        };
        client
    }

    fn client_participating(&mut self) -> bool {
        self.client().is_participating()
    }

    fn run_until_participating(&mut self) {
        for _ in 0..120 {
            self.frame();
            if self.client_participating() && self.host_pawn().is_some() {
                return;
            }
        }
        panic!("the client never participated");
    }

    fn frames(&mut self, count: u32) {
        for _ in 0..count {
            self.frame();
        }
    }

    fn frame(&mut self) {
        self.host_frame();
        self.client_frame(None);
    }

    /// One frame whose client tick declares a switch to `slot` before its command.
    fn declare(&mut self, slot: u8) {
        self.host_frame();
        self.client_frame(Some(slot));
    }

    fn host_pawn(&self) -> Option<EntityId> {
        self.slot_pawns.pawn_for(CLIENT_ID)
    }

    fn host_active_slot(&self) -> Option<usize> {
        let pawn = self.host_pawn()?;
        self.host_registry
            .get_component::<Inventory>(pawn)
            .ok()
            .map(|inventory| inventory.active_slot)
    }

    fn pending_declarations(&self) -> bool {
        self.endpoint.has_pending_switch_declarations()
    }

    /// Hold the slot on the host's next poll, as a parity or level change does.
    fn host_demotes(&mut self) {
        self.server.set_level_parity(None);
    }

    fn host_repromotes(&mut self) {
        self.server
            .set_level_parity(Some((RELAY_LEVEL.to_string(), RELAY_FINGERPRINT)));
    }

    fn host_frame(&mut self) {
        self.server.update_connections(FRAME);
        let mut declarations = Vec::new();
        for packet in std::mem::take(&mut self.up) {
            declarations.extend(switch_declarations(&packet));
            self.server.process_packet_from(&packet, CLIENT_ID);
        }
        let poll = self.server.poll_handshakes();
        for event in &poll.lifecycle {
            match event {
                SlotEvent::Participating { client_id } => {
                    if self.server.is_current_participation_entry(event) {
                        self.spawn_armed_pawn(*client_id);
                    }
                }
                SlotEvent::Demoted { .. } | SlotEvent::Closed { .. } => {
                    let _ = host_handle_lifecycle(
                        &mut self.host_registry,
                        &mut self.allocator,
                        &mut self.replicable,
                        &mut self.replication,
                        &mut self.state_slots,
                        &mut self.slot_pawns,
                        &mut self.command_queues,
                        &mut self.owners,
                        &mut self.weapon_owners,
                        &mut self.open_shots,
                        &mut self.pending_hits,
                        &mut self.weaponless,
                        &mut self.last_sent_tuning,
                        None,
                        std::slice::from_ref(event),
                    );
                }
            }
        }
        if self.server.is_participating(CLIENT_ID) {
            self.declarations_drained.extend(declarations);
        }
        for client_id in self.server.participating_clients() {
            host_handle_client_messages(
                &mut self.server,
                &mut self.replication,
                &mut self.state_slots,
                &mut self.command_queues,
                &self.owners,
                &mut self.pending_hits,
                &mut self.open_shots,
                client_id,
                0,
                0,
            );
        }
        self.retained_after_dispatch = self.command_queues.retained_switch_count(CLIENT_ID);
        for resolved in host_resolve_remote_commands(&self.owners, &mut self.command_queues) {
            if resolved.source == ResolutionSource::Real {
                self.resolved_real.push(resolved.client_tick);
            }
        }
        host_apply_switch_deliveries(
            &mut self.host_registry,
            &mut self.server,
            &self.owners,
            &mut self.command_queues,
            &mut self.weapon_owners,
            false,
        );
        for packet in self.server.packets_to_send(CLIENT_ID) {
            self.outcomes_flushed.extend(switch_outcome_ids(&packet));
            let input = renet_packet_channel(&packet) == Some(u8::from(Channel::Input));
            if !(self.cut_down_input && input) {
                self.down.push(packet);
            }
        }
    }

    /// Participation entry: the slot's pawn, holding a rifle in each of three
    /// slots with slot 0 active, owned by the client.
    fn spawn_armed_pawn(&mut self, client_id: u64) {
        host_handle_accept(
            &mut self.host_registry,
            &mut self.allocator,
            &mut self.replicable,
            &mut self.slot_pawns,
            client_id,
        );
        let pawn = self.host_pawn().expect("entry spawns the slot's pawn");
        let mut inventory = Inventory::default();
        for slot in 0..3 {
            let weapon = self.host_registry.spawn(Transform::default());
            self.host_registry.set_component(weapon, rifle()).unwrap();
            inventory.wieldables[slot] = Some(weapon);
        }
        self.host_registry.set_component(pawn, inventory).unwrap();
        self.owners.set(pawn, client_id);
    }

    fn client_frame(&mut self, declare: Option<u8>) {
        let packets = if self.hold_down {
            Vec::new()
        } else {
            std::mem::take(&mut self.down)
        };
        let client = self.client();
        client.update_connections(FRAME);
        for packet in packets {
            client.process_packet(&packet);
        }
        let controls = client.drain_control();
        for control in controls {
            if let ServerControlMessage::Divergence(DivergenceReason::Holding(_)) = control {
                self.endpoint.demote_client_state(&mut self.client_registry);
                let participating = self.client_participating();
                self.holdings.push(participating);
            }
        }
        {
            let NetEndpoint::Client {
                client,
                time_sync,
                pending_switch_declarations,
                ..
            } = &mut self.endpoint
            else {
                unreachable!("the relay endpoint is a client");
            };
            let _ = client_drive_time_sync(client, time_sync, self.client_tick);
            self.resolutions.extend(client_resolve_switch_outcomes(
                time_sync,
                pending_switch_declarations,
            ));
        }
        let tick = self.client_tick;
        if let Some(slot) = declare {
            self.endpoint.send_client_switch_declaration(
                slot,
                tick,
                0,
                None,
                wire::SwitchDeclarationKind::Switch,
            );
        }
        if self.send_commands {
            self.client()
                .send_input(wire::encode(&wire::ClientMessage::Input(input_at(
                    tick, 0.0,
                ))));
        }
        self.client_tick = tick.wrapping_add(1);
        let packets = self.client().packets_to_send();
        for packet in &packets {
            self.declarations_flushed
                .extend(switch_declarations(packet));
        }
        if !self.cut_up {
            self.up.extend(packets);
        }
    }
}

fn is_accepted(resolution: &CurrentSwitchResolution) -> bool {
    matches!(resolution, CurrentSwitchResolution::Accepted { .. })
}

// Permit: an outcome in the current participation resolves its declaration.
#[test]
fn switch_outcome_in_the_current_participation_resolves_its_declaration() {
    let mut relay = SwitchRelay::joined();
    relay.declare(1);
    assert!(relay.pending_declarations());
    relay.frames(10);
    assert_eq!(relay.resolutions.len(), 1, "one outcome arrived");
    assert!(
        is_accepted(&relay.resolutions[0]),
        "{:?}",
        relay.resolutions
    );
    assert!(!relay.pending_declarations());
    assert_eq!(relay.host_active_slot(), Some(1));
}

// Refuse: the outcome for a declaration made before a demotion is in flight when
// Holding lands, and both reach the client in one frame. The Control drain runs
// first and clears the chain, so the outcome resolves nothing. Permit: a
// declaration in the new participation resolves on its own outcome (P9).
#[test]
fn switch_outcome_landing_with_the_holding_that_retires_it_resolves_nothing() {
    let mut relay = SwitchRelay::joined();
    relay.hold_down = true;
    relay.declare(1);
    relay.frames(5);
    assert_eq!(relay.host_active_slot(), Some(1), "the host answered");
    relay.host_demotes();
    relay.frames(2);
    assert!(
        relay.resolutions.is_empty(),
        "nothing reached the client yet"
    );
    relay.hold_down = false;
    relay.frame();
    assert_eq!(
        relay.holdings.len(),
        1,
        "Holding landed in the frame the outcome did"
    );
    assert_eq!(
        relay.resolutions,
        [CurrentSwitchResolution::None],
        "the outcome arrived and resolved nothing"
    );
    assert!(!relay.pending_declarations());

    relay.host_repromotes();
    relay.run_until_participating();
    relay.declare(2);
    relay.frames(10);
    assert_eq!(relay.resolutions.len(), 2);
    assert!(
        is_accepted(&relay.resolutions[1]),
        "{:?}",
        relay.resolutions
    );
    assert!(!relay.pending_declarations());
    assert_eq!(relay.host_active_slot(), Some(2));
}

// Refuse: declaration n's outcome is held in flight across a demotion, a
// re-promotion and declaration n+1. Outcome n then meets n+1 at the chain's
// front and resolves nothing; n+1 resolves on its own outcome (P10).
#[test]
fn stale_switch_outcome_meets_a_newer_front_and_leaves_it_to_its_own_outcome() {
    let mut relay = SwitchRelay::joined();
    relay.cut_down_input = true;
    relay.declare(1);
    relay.frames(5);
    assert_eq!(relay.host_active_slot(), Some(1), "the host answered n");
    relay.host_demotes();
    relay.frames(5);
    assert_eq!(relay.holdings.len(), 1, "Control still reached the client");
    assert!(!relay.pending_declarations(), "Holding cleared the chain");
    relay.host_repromotes();
    relay.run_until_participating();
    relay.declare(2);
    relay.frames(5);
    assert!(relay.pending_declarations(), "n+1 waits on its outcome");
    assert!(
        relay.resolutions.is_empty(),
        "no outcome reached the client"
    );

    relay.cut_down_input = false;
    relay.frames(5);
    assert_eq!(
        relay.resolutions.len(),
        2,
        "both outcomes arrived: {:?}",
        relay.resolutions
    );
    assert_eq!(
        relay.resolutions[0],
        CurrentSwitchResolution::None,
        "outcome n leaves n+1 alone"
    );
    assert!(
        is_accepted(&relay.resolutions[1]),
        "{:?}",
        relay.resolutions
    );
    assert!(!relay.pending_declarations());
    assert_eq!(relay.host_active_slot(), Some(2));
}

// Refuse: a level unload clears the chain, so an outcome from before it
// resolves nothing. Permit: the next declaration resolves on its own outcome.
#[test]
fn switch_outcome_from_before_a_level_unload_resolves_nothing() {
    let mut relay = SwitchRelay::joined();
    relay.cut_down_input = true;
    relay.declare(1);
    relay.frames(5);
    assert_eq!(relay.host_active_slot(), Some(1), "the host answered");
    relay.endpoint.reset_level_scoped_client_state();
    assert!(!relay.pending_declarations(), "unload cleared the chain");
    relay.cut_down_input = false;
    relay.frames(3);
    assert_eq!(relay.resolutions, [CurrentSwitchResolution::None]);

    relay.declare(2);
    relay.frames(10);
    assert_eq!(relay.resolutions.len(), 2);
    assert!(
        is_accepted(&relay.resolutions[1]),
        "{:?}",
        relay.resolutions
    );
    assert!(!relay.pending_declarations());
}

// Refuse: a declaration framed under a participation the host retired before it
// arrived never reaches the switch lane and is never answered. Permit: one sent
// after re-promotion reaches the lane and is answered (P6).
#[test]
fn declaration_sent_before_a_demotion_never_reaches_the_lane_after_repromotion() {
    let mut relay = SwitchRelay::joined();
    relay.cut_up = true;
    let declared_on = relay.client_tick;
    relay.declare(1);
    relay.frames(2);
    // The client framed it before the demotion, under the retired epoch; every
    // later copy is renet's resend of these bytes.
    let stale: Vec<(u64, Vec<u8>)> = relay.declarations_flushed.clone();
    assert!(
        !stale.is_empty() && stale.iter().all(|declaration| *declaration == stale[0]),
        "one declaration left before the demotion: {stale:?}"
    );
    let Some(wire::ClientMessage::SwitchDeclaration(stale_declaration)) =
        decode_client_input(&stale[0].1)
    else {
        panic!("the flushed message is a switch declaration");
    };
    let stale_id = stale_declaration.declaration_id;
    relay.host_demotes();
    relay.frames(3);
    assert_eq!(relay.holdings.len(), 1);
    relay.host_repromotes();
    relay.frames(3);
    assert!(
        relay.client_participating(),
        "the new participation is armed"
    );
    relay.cut_up = false;
    for _ in 0..PAST_SWITCH_WAIT {
        relay.frame();
        // Read right after the Input dispatch, so a declaration admitted and
        // applied within one host frame would still show.
        assert_eq!(
            relay.retained_after_dispatch, 0,
            "the retired declaration never enters the lane"
        );
    }
    assert!(
        relay
            .resolved_real
            .iter()
            .any(|&tick| tick > declared_on + 5),
        "the uplink backlog behind the declaration was delivered"
    );
    assert!(
        relay.declarations_drained.contains(&stale[0]),
        "the old-epoch declaration reached the participating host's Input drain"
    );
    assert!(relay.resolutions.is_empty(), "and is never answered");
    assert_eq!(relay.host_active_slot(), Some(0));

    relay.declare(1);
    relay.host_frame();
    assert_eq!(
        relay.command_queues.retained_switch_count(CLIENT_ID),
        1,
        "the new declaration reaches the lane"
    );
    relay.frames(10);
    assert_eq!(relay.resolutions.len(), 1);
    assert!(
        is_accepted(&relay.resolutions[0]),
        "{:?}",
        relay.resolutions
    );
    assert_eq!(relay.host_active_slot(), Some(1));
    assert!(
        relay.outcomes_flushed.contains(&(stale_id + 1)),
        "the scan sees the new declaration's outcome: {:?}",
        relay.outcomes_flushed
    );
    assert!(
        !relay.outcomes_flushed.contains(&stale_id),
        "the host never sends an outcome for the stale declaration {stale_id}: {:?}",
        relay.outcomes_flushed
    );
}

// A declaration the host drops by epoch leaves no chain waiting on it: Holding
// and the next participation marker land in one Control drain and clear the
// chain before the first new declaration, which then resolves on its own
// outcome (P7).
#[test]
fn declaration_the_host_drops_by_epoch_never_leaves_the_chain_waiting() {
    let mut relay = SwitchRelay::joined();
    relay.cut_up = true;
    relay.hold_down = true;
    relay.declare(1);
    relay.host_demotes();
    relay.frames(2);
    relay.host_repromotes();
    relay.frames(2);
    assert!(
        relay.pending_declarations(),
        "the declaration is still pending"
    );
    relay.cut_up = false;
    relay.hold_down = false;
    relay.frame();
    assert_eq!(
        relay.holdings,
        [true],
        "Holding and the new marker landed in one drain"
    );
    assert!(
        !relay.pending_declarations(),
        "the drain cleared the dropped declaration"
    );

    relay.declare(2);
    relay.frames(10);
    assert_eq!(
        relay.resolutions.len(),
        1,
        "only the new declaration is answered"
    );
    assert!(
        is_accepted(&relay.resolutions[0]),
        "{:?}",
        relay.resolutions
    );
    assert!(!relay.pending_declarations());
    assert_eq!(relay.host_active_slot(), Some(2));
}

// A switch retained in the host lane when the slot is demoted goes with the
// queue: after re-promotion it is never applied or answered (P8).
#[test]
fn switch_retained_in_the_lane_at_demotion_is_never_applied_after_repromotion() {
    let mut relay = SwitchRelay::joined();
    // The declaration's own command never follows, so the lane holds it.
    relay.send_commands = false;
    relay.declare(1);
    relay.frames(3);
    assert_eq!(
        relay.command_queues.retained_switch_count(CLIENT_ID),
        1,
        "the lane holds the switch"
    );
    relay.host_demotes();
    relay.frame();
    assert_eq!(relay.command_queues.retained_switch_count(CLIENT_ID), 0);
    relay.host_repromotes();
    relay.send_commands = true;
    relay.run_until_participating();
    relay.frames(PAST_SWITCH_WAIT);
    assert!(
        relay.resolutions.is_empty(),
        "never answered: {:?}",
        relay.resolutions
    );
    assert_eq!(relay.host_active_slot(), Some(0), "never applied");
}

// Regression guard: a world-less frame discards client Input, a switch outcome
// included. The level unload that precedes world-less frames already cleared the
// chain, so the discarded outcome leaves no declaration waiting, and the next
// declaration resolves on its own outcome.
#[test]
fn world_less_frame_discarding_a_switch_outcome_leaves_no_stuck_chain() {
    let mut relay = SwitchRelay::joined();
    relay.hold_down = true;
    relay.declare(1);
    relay.frames(5);
    assert_eq!(relay.host_active_slot(), Some(1), "the host answered");
    relay.endpoint.reset_level_scoped_client_state();
    // The world-less frame: its poll delivers the held outcome, then discards it.
    relay.hold_down = false;
    let packets = std::mem::take(&mut relay.down);
    let NetEndpoint::Client {
        client, time_sync, ..
    } = &mut relay.endpoint
    else {
        unreachable!("the relay endpoint is a client");
    };
    client.update_connections(FRAME);
    for packet in packets {
        client.process_packet(&packet);
    }
    super::endpoint::discard_world_less_input(client, time_sync);
    assert!(!relay.pending_declarations(), "no declaration waits");

    relay.frames(3);
    assert!(relay.resolutions.is_empty(), "the outcome was discarded");
    relay.declare(2);
    relay.frames(10);
    assert_eq!(relay.resolutions.len(), 1);
    assert!(
        is_accepted(&relay.resolutions[0]),
        "{:?}",
        relay.resolutions
    );
    assert!(!relay.pending_declarations());
    assert_eq!(relay.host_active_slot(), Some(2));
}

// Grep gate: every switch send site puts its declaration or outcome on Input.
// Control can no longer carry one (its enums have no switch variant, and
// `ServerPoll` no switch intake); this pins the send sites' channel.
#[test]
fn switch_send_sites_put_declarations_and_outcomes_on_input_never_control() {
    for (file, source, input_sends) in [
        ("client_switch.rs", include_str!("client_switch.rs"), 1),
        ("host_switch.rs", include_str!("host_switch.rs"), 2),
    ] {
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("split yields the production half");
        for control in ["send_control(", "Channel::Control", "ControlMessage"] {
            assert!(
                !production.contains(control),
                "{file} names `{control}` beside a switch send"
            );
        }
        assert_eq!(
            production.matches("send_input(").count(),
            input_sends,
            "{file}'s switch sends ride Input"
        );
    }
}
