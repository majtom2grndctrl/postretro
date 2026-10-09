// Polled, registry-blind renet transport and E15 two-stage control gate.
// See: context/lib/networking.md

use std::collections::{BTreeMap, HashMap};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::Duration;

use renet::{
    ChannelConfig, ClientId, ConnectionConfig, DisconnectReason, RenetClient, RenetServer,
    SendType, ServerEvent,
};
use renet_netcode::{
    ClientAuthentication, NetcodeClientTransport, NetcodeDisconnectReason, NetcodeServerTransport,
    NetcodeTransportError, ServerAuthentication, ServerConfig,
};

use crate::slots::{CloseCause, SlotEvent, SlotState, SlotTable};
use crate::wire::{
    self, ClientControlMessage, ClientSwitchDeclaration, ConnectClaim, JoinSeedValue,
    NETCODE_USER_DATA_BYTES, ParityDeclaration, ParticipationFrame, ServerControlFrame,
    ServerControlMessage, ServerPresentationMessage,
};

pub use crate::handshake::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Control = 0,
    Snapshot = 1,
    Input = 2,
    Presentation = 3,
}

impl From<Channel> for u8 {
    fn from(channel: Channel) -> Self {
        channel as u8
    }
}

const CHANNEL_MEMORY_BYTES: usize = 5 * 1024 * 1024;
const RELIABLE_RESEND: Duration = Duration::from_millis(300);

#[must_use]
pub fn connection_config() -> ConnectionConfig {
    let channels = vec![
        ChannelConfig {
            channel_id: Channel::Control.into(),
            max_memory_usage_bytes: CHANNEL_MEMORY_BYTES,
            send_type: SendType::ReliableOrdered {
                resend_time: RELIABLE_RESEND,
            },
        },
        ChannelConfig {
            channel_id: Channel::Snapshot.into(),
            max_memory_usage_bytes: CHANNEL_MEMORY_BYTES,
            send_type: SendType::Unreliable,
        },
        ChannelConfig {
            channel_id: Channel::Input.into(),
            max_memory_usage_bytes: CHANNEL_MEMORY_BYTES,
            send_type: SendType::ReliableOrdered {
                resend_time: RELIABLE_RESEND,
            },
        },
        ChannelConfig {
            channel_id: Channel::Presentation.into(),
            max_memory_usage_bytes: CHANNEL_MEMORY_BYTES,
            send_type: SendType::Unreliable,
        },
    ];
    ConnectionConfig {
        available_bytes_per_tick: 60_000,
        server_channels_config: channels.clone(),
        client_channels_config: channels,
    }
}

fn close_cause_from(reason: DisconnectReason) -> CloseCause {
    match reason {
        DisconnectReason::DisconnectedByClient => CloseCause::Disconnect,
        _ => CloseCause::Timeout,
    }
}

/// A verdict about a Control message. Lifecycle effects are reported separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeOutcome {
    Admitted {
        client_id: ClientId,
    },
    Rejected {
        client_id: ClientId,
        cause: ClosingCause,
    },
    /// A slot is held below participating: any holding cause, content parity
    /// or revealed.
    ParityHeld {
        client_id: ClientId,
        cause: HoldingCause,
    },
}

#[derive(Debug, Default)]
#[must_use = "a poll carries gate verdicts and slot lifecycle transitions"]
pub struct ServerPoll {
    /// Client slots closed since the preceding poll. This edge includes admitted
    /// and pending connections, unlike lifecycle events which only report a
    /// former participating slot.
    pub disconnects: Vec<ClientId>,
    pub handshakes: Vec<HandshakeOutcome>,
    pub lifecycle: Vec<SlotEvent>,
    /// Registry-blind reliable controls from currently participating clients.
    /// Engine code resolves the client id into a pawn and validates its state.
    pub switch_declarations: Vec<(ClientId, ClientSwitchDeclaration)>,
    /// Registry-blind join seeds. The engine owns buffering, validation, and
    /// per-seat application because the transport has no slot declarations.
    pub join_seeds: Vec<(ClientId, BTreeMap<String, JoinSeedValue>)>,
}

type ControlMessages = (
    Vec<HandshakeOutcome>,
    Vec<(ClientId, ClientSwitchDeclaration)>,
    Vec<(ClientId, BTreeMap<String, JoinSeedValue>)>,
);

/// Synchronous server transport. It knows only opaque declarations and slot ids.
pub struct NetServer {
    server: RenetServer,
    transport: NetcodeServerTransport,
    slots: SlotTable,
    parity_declarations: HashMap<ClientId, ParityDeclaration>,
    /// The level identity each slot last declared revealed. Absent means not
    /// revealed. Only the client's own declaration or `close_slot` changes it:
    /// parity changes and host level changes leave it in place, so a demoted
    /// slot re-promotes without a fresh reveal once the other terms hold.
    revealed_declarations: HashMap<ClientId, String>,
    connect_claims: HashMap<ClientId, ConnectClaim>,
    pending_lifecycle: Vec<SlotEvent>,
    /// Live slots that closed since the previous poll. Reported to the engine
    /// so it can retire slot-bound game state.
    pending_slot_disconnects: Vec<ClientId>,
    /// Slots awaiting transport teardown after a reliable closing cause was
    /// sent. They are not reported as new disconnects a second time.
    pending_disconnects: Vec<ClientId>,
    holding_diagnostics: HashMap<ClientId, HoldingCause>,
    next_participation_epoch: u64,
    participation_epochs: HashMap<ClientId, u64>,
    mod_identity: Option<(String, String)>,
    mod_digest: Option<[u8; 32]>,
    level_parity: Option<(String, [u8; 32])>,
    /// The level identity the host itself has revealed (finished settling).
    /// Cleared by the engine at unload and suspend, so a reinstall of the same
    /// level holds every slot until the host's new reveal.
    revealed_level: Option<String>,
    relevel_catalog_id: Option<String>,
    // Retained only for the Phase-2 compatibility setter below.
    legacy_kinematic_static_fingerprint: Option<[u8; 32]>,
}

impl NetServer {
    pub fn new(
        socket: UdpSocket,
        public_addr: SocketAddr,
        max_clients: usize,
        current_time: Duration,
        kinematic_static_fingerprint: Option<[u8; 32]>,
    ) -> Result<Self, NetcodeTransportError> {
        let server = RenetServer::new(connection_config());
        let transport = NetcodeServerTransport::new(
            ServerConfig {
                current_time,
                max_clients,
                protocol_id: transport_protocol_id(),
                public_addresses: vec![public_addr],
                authentication: ServerAuthentication::Unsecure,
            },
            socket,
        )?;
        Ok(Self {
            server,
            transport,
            slots: SlotTable::new(),
            parity_declarations: HashMap::new(),
            revealed_declarations: HashMap::new(),
            connect_claims: HashMap::new(),
            pending_lifecycle: Vec::new(),
            pending_slot_disconnects: Vec::new(),
            pending_disconnects: Vec::new(),
            holding_diagnostics: HashMap::new(),
            next_participation_epoch: 1,
            participation_epochs: HashMap::new(),
            mod_identity: None,
            mod_digest: None,
            level_parity: None,
            revealed_level: None,
            relevel_catalog_id: None,
            legacy_kinematic_static_fingerprint: kinematic_static_fingerprint,
        })
    }

    pub fn set_mod_identity(&mut self, id: String, version: String) {
        self.mod_identity = Some((id, version));
    }

    /// Whether admission can be evaluated. Kept as a transport query so the
    /// engine can surface a missing-manifest diagnostic without learning slot
    /// internals.
    pub fn has_mod_identity(&self) -> bool {
        self.mod_identity.is_some()
    }

    pub fn has_connected_clients(&self) -> bool {
        !self.server.clients_id().is_empty()
    }

    pub fn set_mod_digest(&mut self, digest: Option<[u8; 32]>) {
        self.mod_digest = digest;
        let _ = self.reevaluate_parity(None);
    }

    pub fn set_level_parity(&mut self, level: Option<(String, [u8; 32])>) {
        self.level_parity = level;
        let _ = self.reevaluate_parity(None);
    }

    /// Record the level identity the host has revealed, or `None` at unload
    /// and suspend. A slot participates only once the host has revealed its
    /// installed level, so a promotion never lands while the host is settling.
    pub fn set_revealed_level(&mut self, level: Option<String>) {
        self.revealed_level = level;
        let _ = self.reevaluate_parity(None);
    }

    /// The level identity the host has revealed, if any.
    pub fn revealed_level(&self) -> Option<&str> {
        self.revealed_level.as_deref()
    }

    /// Install the catalog id clients should follow. This is intentionally
    /// separate from level parity: the net crate cannot distinguish a catalog
    /// id from a raw-path fallback inside that opaque identity string.
    pub fn set_relevel_catalog_id(&mut self, catalog_id: Option<String>) {
        self.relevel_catalog_id = catalog_id;
        let Some(catalog_id) = self.relevel_catalog_id.clone() else {
            return;
        };
        for client_id in self.relevel_recipients() {
            self.send_relevel(client_id, &catalog_id);
        }
    }

    /// Phase-2 compatibility shim. Its old close-on-change behavior is preserved;
    /// new lifecycle code must use mod/level parity setters instead.
    #[deprecated(note = "use set_mod_digest and set_level_parity")]
    pub fn set_kinematic_static_fingerprint(&mut self, fingerprint: [u8; 32]) {
        if self.legacy_kinematic_static_fingerprint == Some(fingerprint) {
            return;
        }
        if self.legacy_kinematic_static_fingerprint.is_some() {
            for client_id in self.server.clients_id() {
                if let Some(event) = self.close_slot(client_id, CloseCause::Timeout) {
                    self.pending_lifecycle.push(event);
                }
                self.server.disconnect(client_id);
            }
        }
        self.legacy_kinematic_static_fingerprint = Some(fingerprint);
    }

    #[must_use]
    pub fn local_addr(&self) -> Vec<SocketAddr> {
        self.transport.addresses()
    }

    pub fn update(&mut self, dt: Duration) -> Result<ServerPoll, NetcodeTransportError> {
        self.server.update(dt);
        self.transport.update(dt, &mut self.server)?;
        self.collect_server_events();
        self.apply_pending_disconnects();
        self.discard_ineligible_input();
        let (handshakes, switch_declarations, join_seeds) = self.process_control_messages();
        self.discard_ineligible_input();
        let lifecycle = std::mem::take(&mut self.pending_lifecycle);
        let disconnects = std::mem::take(&mut self.pending_slot_disconnects);
        self.transport.send_packets(&mut self.server);
        Ok(ServerPoll {
            disconnects,
            handshakes,
            lifecycle,
            switch_declarations,
            join_seeds,
        })
    }

    fn apply_pending_disconnects(&mut self) {
        let mut waiting = Vec::new();
        for client_id in std::mem::take(&mut self.pending_disconnects) {
            if !self.server.is_connected(client_id) {
                continue;
            }
            if self
                .server
                .channel_available_memory(client_id, Channel::Control)
                == CHANNEL_MEMORY_BYTES
            {
                self.server.disconnect(client_id);
            } else {
                waiting.push(client_id);
            }
        }
        self.pending_disconnects = waiting;
    }

    fn discard_ineligible_input(&mut self) {
        for client_id in self.server.clients_id() {
            if self.is_participating(client_id) {
                continue;
            }
            while self
                .server
                .receive_message(client_id, Channel::Input)
                .is_some()
            {}
        }
    }

    fn collect_server_events(&mut self) {
        while let Some(event) = self.server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    self.slots.on_connect(client_id);
                    if let Some(user_data) = self.transport.user_data(client_id)
                        && let Some(claim) = wire::decode_connect_claim(&user_data)
                    {
                        self.connect_claims.insert(client_id, claim);
                    }
                }
                ServerEvent::ClientDisconnected { client_id, reason } => {
                    if let Some(event) = self.close_slot(client_id, close_cause_from(reason)) {
                        self.pending_lifecycle.push(event);
                    }
                }
            }
        }
    }

    fn process_control_messages(&mut self) -> ControlMessages {
        let mut outcomes = Vec::new();
        let mut switch_declarations = Vec::new();
        let mut join_seeds = Vec::new();
        let Some((expected_id, expected_version)) = self.mod_identity.clone() else {
            return (outcomes, switch_declarations, join_seeds);
        };
        let expected_protocol = protocol_version();

        for client_id in self.server.clients_id() {
            // Set by any declaration the participation predicate reads.
            let mut declaration_moved = false;
            if self.slots.is_closed(client_id) {
                while self
                    .server
                    .receive_message(client_id, Channel::Control)
                    .is_some()
                {}
                continue;
            }
            // Every predicate verdict needs the installed mod digest, so an
            // admitted slot's Control stays reliably queued until it exists.
            if !matches!(self.slots.state(client_id), Some(SlotState::Pending))
                && self.mod_digest.is_none()
            {
                continue;
            }
            while let Some(bytes) = self.server.receive_message(client_id, Channel::Control) {
                let message: ClientControlMessage = match wire::decode(&bytes) {
                    Ok(message) => message,
                    Err(err) => {
                        let cause = ClosingCause::Protocol {
                            expected: expected_protocol,
                            received: malformed_version(&err),
                        };
                        self.reject(client_id, cause.clone());
                        outcomes.push(HandshakeOutcome::Rejected { client_id, cause });
                        break;
                    }
                };
                match message {
                    ClientControlMessage::Admission {
                        protocol,
                        mod_id,
                        mod_version,
                    } => {
                        if !matches!(self.slots.state(client_id), Some(SlotState::Pending)) {
                            continue;
                        }
                        let cause = match validate_handshake(expected_protocol, protocol) {
                            Ok(()) if mod_id == expected_id => None,
                            Ok(()) => Some(ClosingCause::ModId {
                                expected: expected_id.clone(),
                                received: mod_id,
                                expected_version: expected_version.clone(),
                                received_version: mod_version.clone(),
                            }),
                            Err(cause) => Some(cause),
                        };
                        if let Some(cause) = cause {
                            log::warn!(
                                "[Net] rejecting client {client_id}: {}",
                                DivergenceReason::Closing(cause.clone())
                            );
                            self.reject(client_id, cause.clone());
                            outcomes.push(HandshakeOutcome::Rejected { client_id, cause });
                            break;
                        }
                        // Version is intentionally diagnostic-only: it must never gate.
                        if mod_version != expected_version {
                            log::info!(
                                "[Net] mod version differs for client {client_id}: host={} client={mod_version}",
                                expected_version
                            );
                        }
                        let _ = self.slots.admit(client_id);
                        outcomes.push(HandshakeOutcome::Admitted { client_id });
                        declaration_moved = self.parity_declarations.contains_key(&client_id);
                        if let Some(catalog_id) = self.relevel_catalog_id.clone() {
                            self.send_relevel(client_id, &catalog_id);
                        }
                        if self.mod_digest.is_none() {
                            break;
                        }
                    }
                    ClientControlMessage::Parity(declaration) => {
                        self.parity_declarations.insert(client_id, declaration);
                        declaration_moved = true;
                        if self.mod_digest.is_none() {
                            break;
                        }
                    }
                    ClientControlMessage::SwitchDeclaration(declaration) => {
                        // A declaration has no meaning before a slot owns a live
                        // pawn. Keeping it inside the participation gate also
                        // prevents pre-admission controls from leaking into a later
                        // promotion generation.
                        if self.is_participating(client_id) {
                            switch_declarations.push((client_id, declaration));
                        }
                    }
                    ClientControlMessage::JoinSeed { slots } => {
                        // The engine validates the durable keys against its
                        // committed declarations and applies only eligible
                        // per-owner values. This transport is intentionally
                        // registry-blind, so pass the opaque map through.
                        join_seeds.push((client_id, slots));
                    }
                    ClientControlMessage::Revealed(level) => {
                        // Retained like parity: only the batch's last
                        // declaration reaches the predicate below.
                        match level {
                            Some(level) => {
                                self.revealed_declarations.insert(client_id, level);
                            }
                            None => {
                                self.revealed_declarations.remove(&client_id);
                            }
                        }
                        declaration_moved = true;
                    }
                }
                if self.mod_digest.is_none() {
                    break;
                }
            }
            // Control is reliable-ordered. Evaluate the final retained declarations
            // once after draining this batch so an earlier stale declaration cannot
            // emit a transient diagnostic before a later same-batch replacement,
            // and parity plus a reveal in one batch promote once.
            if declaration_moved
                && self.mod_digest.is_some()
                && matches!(
                    self.slots.state(client_id),
                    Some(SlotState::Admitted | SlotState::Participating)
                )
            {
                let previous = self.holding_diagnostics.get(&client_id).cloned();
                if let Some(cause) = self.reevaluate_parity(Some(client_id))
                    && previous.as_ref() != Some(&cause)
                {
                    outcomes.push(HandshakeOutcome::ParityHeld { client_id, cause });
                }
            }
        }
        (outcomes, switch_declarations, join_seeds)
    }

    fn reject(&mut self, client_id: ClientId, cause: ClosingCause) {
        self.send_control(
            client_id,
            wire::encode(&ServerControlMessage::Divergence(
                DivergenceReason::Closing(cause),
            )),
        );
        if let Some(event) = self.close_slot(client_id, CloseCause::Timeout) {
            self.pending_lifecycle.push(event);
        }
        if !self.pending_disconnects.contains(&client_id) {
            self.pending_disconnects.push(client_id);
        }
    }

    fn send_divergence(&mut self, client_id: ClientId, cause: HoldingCause) -> bool {
        if self.holding_diagnostics.get(&client_id) == Some(&cause) {
            return false;
        }
        self.holding_diagnostics.insert(client_id, cause.clone());
        self.send_control(
            client_id,
            wire::encode(&ServerControlMessage::Divergence(
                DivergenceReason::Holding(cause),
            )),
        );
        true
    }

    fn relevel_recipients(&self) -> Vec<ClientId> {
        self.server
            .clients_id()
            .into_iter()
            .filter(|client_id| {
                matches!(
                    self.slots.state(*client_id),
                    Some(SlotState::Admitted | SlotState::Participating)
                )
            })
            .collect()
    }

    fn send_relevel(&mut self, client_id: ClientId, catalog_id: &str) {
        self.send_control(
            client_id,
            wire::encode(&ServerControlMessage::Relevel(catalog_id.to_owned())),
        );
    }

    /// Enforce the single participation predicate after any source install,
    /// host reveal, or declaration arrival. Install-driven transitions land in
    /// `pending_lifecycle`.
    fn reevaluate_parity(&mut self, only: Option<ClientId>) -> Option<HoldingCause> {
        let ids = only.map_or_else(|| self.server.clients_id(), |id| vec![id]);
        let mut selected_cause = None;
        for client_id in ids {
            let state = self.slots.state(client_id);
            if !matches!(state, Some(SlotState::Admitted | SlotState::Participating)) {
                continue;
            }
            let cause = participation_cause(
                self.mod_digest,
                self.level_parity.as_ref(),
                self.parity_declarations.get(&client_id),
                self.revealed_level.as_deref(),
                self.revealed_declarations
                    .get(&client_id)
                    .map(String::as_str),
            );
            match cause {
                None => {
                    if let Some(event) = self.slots.participate(client_id) {
                        self.holding_diagnostics.remove(&client_id);
                        self.begin_participation(client_id);
                        self.pending_lifecycle.push(event);
                    }
                }
                Some(cause) => {
                    if matches!(state, Some(SlotState::Participating))
                        && let Some(event) = self.slots.demote(client_id, cause.clone())
                    {
                        self.pending_lifecycle.push(event);
                    }
                    if self.parity_declarations.contains_key(&client_id) {
                        let _ = self.send_divergence(client_id, cause.clone());
                    }
                    if only == Some(client_id) {
                        selected_cause = Some(cause);
                    }
                }
            }
        }
        selected_cause
    }

    fn close_slot(&mut self, client_id: ClientId, cause: CloseCause) -> Option<SlotEvent> {
        // `close` records a tombstone for unknown/already-closed ids, so read
        // the prior state first to report only a live transport disconnect.
        let was_live = matches!(self.slots.state(client_id), Some(state) if !matches!(state, SlotState::Closed { .. }));
        self.parity_declarations.remove(&client_id);
        self.revealed_declarations.remove(&client_id);
        self.connect_claims.remove(&client_id);
        self.holding_diagnostics.remove(&client_id);
        self.participation_epochs.remove(&client_id);
        if was_live {
            self.pending_slot_disconnects.push(client_id);
        }
        self.slots.close(client_id, cause)
    }

    fn begin_participation(&mut self, client_id: ClientId) {
        let epoch = self.next_participation_epoch;
        self.next_participation_epoch = self.next_participation_epoch.wrapping_add(1);
        self.participation_epochs.insert(client_id, epoch);
        self.server.send_message(
            client_id,
            Channel::Control,
            wire::encode(&ServerControlFrame {
                participation_epoch: Some(epoch),
                payload: None,
            }),
        );
    }

    #[must_use]
    pub fn is_participating(&self, client_id: ClientId) -> bool {
        self.slots.is_participating(client_id)
    }

    /// Whether a reported entry edge still agrees with the slot's final state
    /// after the poll's complete ordered lifecycle batch.
    #[must_use]
    pub fn is_current_participation_entry(&self, event: &SlotEvent) -> bool {
        matches!(
            event,
            SlotEvent::Participating { client_id } if self.is_participating(*client_id)
        )
    }

    #[deprecated(note = "use is_participating")]
    #[must_use]
    pub fn is_accepted(&self, client_id: ClientId) -> bool {
        self.is_participating(client_id)
    }

    #[must_use]
    pub fn is_closed(&self, client_id: ClientId) -> bool {
        self.slots.is_closed(client_id)
    }

    #[must_use]
    pub fn slot_state(&self, client_id: ClientId) -> Option<SlotState> {
        self.slots.state(client_id)
    }

    #[must_use]
    pub fn participating_clients(&self) -> Vec<ClientId> {
        self.slots.participating_clients()
    }

    #[deprecated(note = "use participating_clients")]
    #[must_use]
    pub fn accepted_clients(&self) -> Vec<ClientId> {
        self.participating_clients()
    }

    #[must_use]
    pub fn connected_clients(&self) -> Vec<ClientId> {
        self.server.clients_id()
    }

    /// The immutable connection claim received during this client's transport
    /// handshake. It disappears with the connection slot.
    #[must_use]
    pub fn connect_claim(&self, client_id: ClientId) -> Option<&ConnectClaim> {
        self.connect_claims.get(&client_id)
    }

    /// Number of transport-scoped claims retained by live slots.
    #[cfg(test)]
    fn connect_claim_count(&self) -> usize {
        self.connect_claims.len()
    }

    /// Snapshots and Input are both participation-gated; held peers are drained
    /// so their reliable channel cannot overflow and disconnect them indirectly.
    pub fn send_snapshot(&mut self, client_id: ClientId, snapshot: Vec<u8>) -> bool {
        if !self.is_participating(client_id) {
            return false;
        }
        let Some(participation_epoch) = self.participation_epochs.get(&client_id).copied() else {
            return false;
        };
        self.server.send_message(
            client_id,
            Channel::Snapshot,
            wire::encode(&ParticipationFrame {
                participation_epoch,
                payload: snapshot,
            }),
        );
        true
    }

    pub fn drain_input(&mut self, client_id: ClientId) -> Vec<Vec<u8>> {
        let participating = self.is_participating(client_id);
        let expected_epoch = self.participation_epochs.get(&client_id).copied();
        let mut messages = Vec::new();
        while let Some(bytes) = self.server.receive_message(client_id, Channel::Input) {
            if !participating {
                continue;
            }
            let frame: ParticipationFrame = match wire::decode(&bytes) {
                Ok(frame) => frame,
                Err(err) => {
                    log::warn!("[Net] dropping unframed client Input from {client_id}: {err}");
                    continue;
                }
            };
            if Some(frame.participation_epoch) != expected_epoch {
                continue;
            }
            // Exhaustively classify recognized input envelopes. Net still leaves
            // their interpretation to the engine and forwards malformed bytes.
            match wire::decode::<crate::wire::ClientMessage>(&frame.payload) {
                Ok(crate::wire::ClientMessage::Input(_))
                | Ok(crate::wire::ClientMessage::Ack(_))
                | Ok(crate::wire::ClientMessage::BaselineRefresh(_))
                | Ok(crate::wire::ClientMessage::TimeSync(_))
                | Ok(crate::wire::ClientMessage::StateBaselineRefresh(_))
                | Ok(crate::wire::ClientMessage::HitDeclaration(_))
                | Err(_) => messages.push(frame.payload),
            }
        }
        messages
    }

    #[cfg(test)]
    fn input_is_empty(&mut self, client_id: ClientId) -> bool {
        self.server
            .receive_message(client_id, Channel::Input)
            .is_none()
    }

    /// Control may be sent to an admitted/closed slot to deliver a hold/reject
    /// diagnostic before socket teardown. Payload semantics remain engine-owned.
    pub fn send_control(&mut self, client_id: ClientId, payload: Vec<u8>) {
        self.server.send_message(
            client_id,
            Channel::Control,
            wire::encode(&ServerControlFrame {
                participation_epoch: self.participation_epochs.get(&client_id).copied(),
                payload: Some(payload),
            }),
        );
    }

    pub fn send_input(&mut self, client_id: ClientId, payload: Vec<u8>) {
        self.server.send_message(client_id, Channel::Input, payload);
    }

    /// Reliable observer facts belong to exactly this participation generation.
    pub fn send_weapon_cues(&mut self, client_id: ClientId, cues: Vec<wire::WeaponCue>) -> bool {
        if !self.is_participating(client_id) {
            return false;
        }
        let Some(participation_epoch) = self.participation_epochs.get(&client_id).copied() else {
            return false;
        };
        let message = wire::WeaponCuesMessage {
            participation_epoch,
            cues,
        };
        if !message.is_valid() {
            return false;
        }
        self.send_input(
            client_id,
            wire::encode(&wire::ServerMessage::WeaponCues(message)),
        );
        true
    }

    /// Send one transient server presentation event to its exact recipient.
    ///
    /// Presentation is intentionally not participation-framed and has no
    /// acknowledgement, resend, or reconciliation path. A departed/demoted
    /// recipient silently loses the cosmetic instead of retaining it for a
    /// later participation generation.
    pub fn send_presentation(
        &mut self,
        client_id: ClientId,
        message: ServerPresentationMessage,
    ) -> bool {
        if !self.is_participating(client_id) {
            return false;
        }
        self.server
            .send_message(client_id, Channel::Presentation, wire::encode(&message));
        true
    }

    pub fn packets_to_send(&mut self, client_id: ClientId) -> Vec<Vec<u8>> {
        self.server
            .get_packets_to_send(client_id)
            .unwrap_or_default()
    }

    pub fn process_packet_from(&mut self, packet: &[u8], client_id: ClientId) {
        let _ = self.server.process_packet_from(packet, client_id);
    }

    /// Add an in-memory relay connection for deterministic transport tests.
    ///
    /// Relay connections bypass renetcode, so their optional user data is
    /// decoded here just as it is on the production `ClientConnected` edge.
    pub fn add_relay_connection(
        &mut self,
        client_id: ClientId,
        user_data: Option<[u8; NETCODE_USER_DATA_BYTES]>,
    ) {
        self.server.add_connection(client_id);
        self.slots.on_connect(client_id);
        if let Some(user_data) = user_data
            && let Some(claim) = wire::decode_connect_claim(&user_data)
        {
            self.connect_claims.insert(client_id, claim);
        }
    }

    #[must_use]
    pub fn close_relay_connection(
        &mut self,
        client_id: ClientId,
        cause: CloseCause,
    ) -> Option<SlotEvent> {
        self.server.remove_connection(client_id);
        self.close_slot(client_id, cause)
    }

    pub fn update_connections(&mut self, dt: Duration) {
        self.server.update(dt);
    }

    pub fn poll_handshakes(&mut self) -> ServerPoll {
        self.collect_server_events();
        self.apply_pending_disconnects();
        self.discard_ineligible_input();
        let (handshakes, switch_declarations, join_seeds) = self.process_control_messages();
        self.discard_ineligible_input();
        let lifecycle = std::mem::take(&mut self.pending_lifecycle);
        let disconnects = std::mem::take(&mut self.pending_slot_disconnects);
        ServerPoll {
            disconnects,
            handshakes,
            lifecycle,
            switch_declarations,
            join_seeds,
        }
    }
}

/// The single participation predicate: `None` means the slot participates.
/// Content parity is checked first, then the revealed term, so both revealed
/// causes rank below every content cause. Check order is the published
/// holding-diagnostic precedence.
fn participation_cause(
    installed_mod_digest: Option<[u8; 32]>,
    installed_level: Option<&(String, [u8; 32])>,
    declaration: Option<&ParityDeclaration>,
    host_revealed: Option<&str>,
    client_revealed: Option<&str>,
) -> Option<HoldingCause> {
    if let Some(cause) = parity_cause(installed_mod_digest, installed_level, declaration) {
        return Some(cause);
    }
    // Parity holding implies an installed level; stay total regardless.
    let Some((identity, _)) = installed_level else {
        return Some(HoldingCause::HostLevelAbsent);
    };
    if host_revealed != Some(identity.as_str()) {
        return Some(HoldingCause::HostNotRevealed {
            identity: identity.clone(),
        });
    }
    if client_revealed != Some(identity.as_str()) {
        return Some(HoldingCause::ClientNotRevealed {
            identity: identity.clone(),
        });
    }
    None
}

/// `None` means the declaration and installed triple match. The order here is
/// the content half of the published holding-diagnostic precedence.
fn parity_cause(
    installed_mod_digest: Option<[u8; 32]>,
    installed_level: Option<&(String, [u8; 32])>,
    declaration: Option<&ParityDeclaration>,
) -> Option<HoldingCause> {
    let Some(installed_mod_digest) = installed_mod_digest else {
        return Some(HoldingCause::HostLevelAbsent);
    };
    let Some(declaration) = declaration else {
        return Some(HoldingCause::HostLevelAbsent);
    };
    if declaration.mod_digest != installed_mod_digest {
        return Some(HoldingCause::ModDigest {
            expected: installed_mod_digest,
            received: declaration.mod_digest,
        });
    }
    let Some((expected_identity, expected_digest)) = installed_level else {
        return Some(HoldingCause::HostLevelAbsent);
    };
    let Some((received_identity, received_digest)) = &declaration.level else {
        return Some(HoldingCause::LevelAbsent {
            expected_identity: expected_identity.clone(),
        });
    };
    if received_identity != expected_identity {
        return Some(HoldingCause::LevelIdentity {
            expected: expected_identity.clone(),
            received: received_identity.clone(),
        });
    }
    if received_digest != expected_digest {
        return Some(HoldingCause::LevelDigest {
            identity: expected_identity.clone(),
            expected: *expected_digest,
            received: *received_digest,
        });
    }
    None
}

/// Connect attempts before a client that has never reached its host gives up.
/// netcode times each attempt out after 15 s without a reply, so this covers a
/// host started up to about a minute after the client. A main-thread stall past
/// 15 s while still connecting spends an attempt too. Only unanswered attempts
/// retry: a denial (full host) or an expired token ends the connect.
pub const CONNECT_ATTEMPTS: u32 = 4;

/// Synchronous client transport. It declares values but never compares them.
pub struct NetClient {
    client: RenetClient,
    transport: NetcodeClientTransport,
    server_addr: SocketAddr,
    /// Attempt N connects as `base_client_id + (N - 1)`.
    base_client_id: u64,
    user_data: Option<[u8; NETCODE_USER_DATA_BYTES]>,
    /// netcode clock: construction time plus every counted `update` step.
    clock: Duration,
    /// The first `update` carries everything since construction — on a
    /// `--connect` boot, renderer init and mod init on the main thread — so it
    /// does not count against the connect timeout.
    polled: bool,
    connect_attempt: u32,
    ever_connected: bool,
    failure_reported: bool,
    transient_error_logged: bool,
    admission_sent: bool,
    parity_sent: bool,
    join_seed_sent: bool,
    join_seed: BTreeMap<String, JoinSeedValue>,
    /// Whether the host holds the current `revealed_level`. A fresh
    /// connection starts sent: the host reads an absent record as not revealed.
    revealed_sent: bool,
    mod_identity: Option<(String, String)>,
    mod_digest: Option<[u8; 32]>,
    level_parity: Option<(String, [u8; 32])>,
    revealed_level: Option<String>,
    active_participation_epoch: Option<u64>,
    retired_participation_epoch: Option<u64>,
    legacy_kinematic_static_fingerprint: Option<[u8; 32]>,
}

fn client_transport(
    socket: UdpSocket,
    server_addr: SocketAddr,
    client_id: u64,
    current_time: Duration,
    user_data: Option<[u8; NETCODE_USER_DATA_BYTES]>,
) -> Result<NetcodeClientTransport, NetcodeTransportError> {
    NetcodeClientTransport::new(
        current_time,
        ClientAuthentication::Unsecure {
            client_id,
            protocol_id: transport_protocol_id(),
            server_addr,
            user_data,
        },
        socket,
    )
    .map_err(Into::into)
}

impl NetClient {
    pub fn new(
        socket: UdpSocket,
        server_addr: SocketAddr,
        client_id: u64,
        current_time: Duration,
        kinematic_static_fingerprint: Option<[u8; 32]>,
        user_data: Option<[u8; NETCODE_USER_DATA_BYTES]>,
    ) -> Result<Self, NetcodeTransportError> {
        let client = RenetClient::new(connection_config());
        let transport = client_transport(socket, server_addr, client_id, current_time, user_data)?;
        Ok(Self {
            client,
            transport,
            server_addr,
            base_client_id: client_id,
            user_data,
            clock: current_time,
            polled: false,
            connect_attempt: 1,
            ever_connected: false,
            failure_reported: false,
            transient_error_logged: false,
            admission_sent: false,
            parity_sent: false,
            join_seed_sent: false,
            join_seed: BTreeMap::new(),
            revealed_sent: true,
            mod_identity: None,
            mod_digest: None,
            level_parity: None,
            revealed_level: None,
            active_participation_epoch: None,
            retired_participation_epoch: None,
            legacy_kinematic_static_fingerprint: kinematic_static_fingerprint,
        })
    }

    pub fn set_mod_identity(&mut self, id: String, version: String) {
        self.mod_identity = Some((id, version));
    }

    pub fn set_mod_digest(&mut self, digest: Option<[u8; 32]>) {
        if self.mod_digest != digest {
            self.mod_digest = digest;
            self.parity_sent = false;
        }
    }

    pub fn set_level_parity(&mut self, level: Option<(String, [u8; 32])>) {
        if self.level_parity != level {
            self.level_parity = level;
            self.parity_sent = false;
            self.join_seed_sent = false;
        }
    }

    /// Declare the level identity this client has revealed (finished settling),
    /// or `None` at unload and suspend. Independent of parity: a mod-digest or
    /// level change re-declares parity but leaves this declaration as set.
    pub fn set_revealed_level(&mut self, level: Option<String>) {
        if self.revealed_level != level {
            self.revealed_level = level;
            self.revealed_sent = false;
        }
    }

    /// Replace the values to carry with the next content-parity declaration.
    /// An empty map is a meaningful seed: it explicitly starts a player at the
    /// host's declared defaults when no local persistence exists.
    pub fn set_join_seed(&mut self, slots: BTreeMap<String, JoinSeedValue>) {
        self.join_seed = slots;
        self.join_seed_sent = false;
    }

    #[deprecated(note = "use set_mod_digest and set_level_parity")]
    pub fn set_kinematic_static_fingerprint(&mut self, fingerprint: [u8; 32]) {
        if self.legacy_kinematic_static_fingerprint == Some(fingerprint) {
            return;
        }
        if self.admission_sent {
            self.client.disconnect();
        }
        self.legacy_kinematic_static_fingerprint = Some(fingerprint);
    }

    fn queue_control_messages(&mut self) {
        if !self.client.is_connected() {
            return;
        }
        if !self.admission_sent
            && let Some((mod_id, mod_version)) = self.mod_identity.clone()
        {
            self.client.send_message(
                Channel::Control,
                wire::encode(&ClientControlMessage::Admission {
                    protocol: protocol_version(),
                    mod_id,
                    mod_version,
                }),
            );
            self.admission_sent = true;
        }
        if !self.parity_sent
            && let Some(mod_digest) = self.mod_digest
        {
            self.client.send_message(
                Channel::Control,
                wire::encode(&ClientControlMessage::Parity(ParityDeclaration {
                    mod_digest,
                    level: self.level_parity.clone(),
                })),
            );
            self.parity_sent = true;
            if !self.join_seed_sent {
                self.client.send_message(
                    Channel::Control,
                    wire::encode(&ClientControlMessage::JoinSeed {
                        slots: self.join_seed.clone(),
                    }),
                );
                self.join_seed_sent = true;
            }
        }
        if !self.revealed_sent {
            self.client.send_message(
                Channel::Control,
                wire::encode(&ClientControlMessage::Revealed(self.revealed_level.clone())),
            );
            self.revealed_sent = true;
        }
    }

    /// Advance the connection. The connect clock starts at the first poll, not
    /// at construction. A connect attempt the host never answered is retried
    /// with a fresh socket and token, up to [`CONNECT_ATTEMPTS`]. Once the
    /// connection has ended, every call returns the ending error; this
    /// transport logs that ending once, naming the host (renetcode separately
    /// logs each attempt's timeout). A frame error that leaves the connection
    /// open is returned and warned about once per streak of failing frames.
    pub fn update(&mut self, dt: Duration) -> Result<(), NetcodeTransportError> {
        let dt = if self.polled { dt } else { Duration::ZERO };
        self.polled = true;
        self.clock += dt;
        self.client.update(dt);
        let result = self.update_transport(dt).and_then(|()| {
            self.queue_control_messages();
            self.transport.send_packets(&mut self.client)
        });
        if self.client.is_connected() {
            self.ever_connected = true;
        }
        let Err(err) = result else {
            self.transient_error_logged = false;
            return Ok(());
        };
        if self.retry_unanswered_connect() {
            return Ok(());
        }
        if self.transport.disconnect_reason().is_none() {
            if !self.transient_error_logged {
                self.transient_error_logged = true;
                log::warn!(
                    "[Net] transport error talking to host {}: {err}",
                    self.server_addr
                );
            }
        } else if !self.failure_reported {
            self.failure_reported = true;
            if self.ever_connected {
                log::error!("[Net] lost connection to host {}: {err}", self.server_addr);
            } else {
                log::error!(
                    "[Net] could not connect to host {} after {} attempt(s): {err}",
                    self.server_addr,
                    self.connect_attempt
                );
            }
        }
        Err(err)
    }

    /// Receive and advance netcode, skipping ICMP port-unreachable reports.
    /// Windows surfaces one as a `ConnectionReset` from `recv_from` after a send
    /// to a port nothing listens on — a host not up yet, or a mistyped port —
    /// and renet_netcode's client returns it before advancing its clock, so the
    /// connect timeout would stall and the reset would read as a failure.
    /// renet_netcode's server transport already skips these; netcode's timeout
    /// governs here.
    fn update_transport(&mut self, dt: Duration) -> Result<(), NetcodeTransportError> {
        const MAX_SKIPPED_RESETS: usize = 16;
        let mut skipped = 0;
        loop {
            match self.transport.update(dt, &mut self.client) {
                Err(NetcodeTransportError::IO(err))
                    if err.kind() == std::io::ErrorKind::ConnectionReset
                        && skipped < MAX_SKIPPED_RESETS =>
                {
                    skipped += 1;
                }
                result => return result,
            }
        }
    }

    /// Replace a transport whose connect request or response went unanswered,
    /// returning whether a new attempt started. Never applies once connected:
    /// a dropped session is not silently rejoined.
    fn retry_unanswered_connect(&mut self) -> bool {
        let unanswered = matches!(
            self.transport.disconnect_reason(),
            Some(
                NetcodeDisconnectReason::ConnectionRequestTimedOut
                    | NetcodeDisconnectReason::ConnectionResponseTimedOut
            )
        );
        if !unanswered || self.ever_connected || self.connect_attempt >= CONNECT_ATTEMPTS {
            return false;
        }
        let bind_addr: SocketAddr = match self.server_addr {
            SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
            SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
        };
        // A fresh id: a host that saw the timed-out response may still hold the
        // old one as connected and would deny it.
        let client_id = self
            .base_client_id
            .wrapping_add(u64::from(self.connect_attempt));
        let transport = UdpSocket::bind(bind_addr)
            .map_err(NetcodeTransportError::IO)
            .and_then(|socket| {
                client_transport(
                    socket,
                    self.server_addr,
                    client_id,
                    self.clock,
                    self.user_data,
                )
            });
        match transport {
            Ok(transport) => {
                self.connect_attempt += 1;
                log::warn!(
                    "[Net] no answer from host {}; retrying (attempt {} of {CONNECT_ATTEMPTS})",
                    self.server_addr,
                    self.connect_attempt
                );
                self.transport = transport;
                self.client = RenetClient::new(connection_config());
                true
            }
            Err(err) => {
                log::error!("[Net] could not rebuild the client transport: {err}");
                false
            }
        }
    }

    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.client.is_connected()
    }

    /// Whether the host has activated the current participation generation.
    /// Gameplay-local work such as private persistence must pause while a
    /// parity demotion leaves the transport connection open.
    #[must_use]
    pub fn is_participating(&self) -> bool {
        self.active_participation_epoch.is_some()
    }

    #[must_use]
    pub fn admission_sent(&self) -> bool {
        self.admission_sent
    }

    #[deprecated(note = "use admission_sent")]
    #[must_use]
    pub fn handshake_sent(&self) -> bool {
        self.admission_sent
    }

    pub fn send_input(&mut self, input: Vec<u8>) {
        let Some(participation_epoch) = self.active_participation_epoch else {
            return;
        };
        self.client.send_message(
            Channel::Input,
            wire::encode(&ParticipationFrame {
                participation_epoch,
                payload: input,
            }),
        );
    }

    /// Send one gameplay control only while a participation generation is active.
    /// Admission and parity still own the early Control ordering; game declarations
    /// are valid only after the host has made the client a participant.
    pub fn send_switch_declaration(&mut self, declaration: ClientSwitchDeclaration) {
        if self.active_participation_epoch.is_none() {
            return;
        }
        self.client.send_message(
            Channel::Control,
            wire::encode(&ClientControlMessage::SwitchDeclaration(declaration)),
        );
    }

    pub fn drain_input(&mut self) -> Vec<Vec<u8>> {
        drain_client_channel(&mut self.client, Channel::Input)
            .into_iter()
            .filter(|bytes| match wire::decode::<wire::ServerMessage>(bytes) {
                Ok(wire::ServerMessage::WeaponCues(message)) => {
                    self.client.is_connected()
                        && self.active_participation_epoch == Some(message.participation_epoch)
                        && message.is_valid()
                }
                _ => true,
            })
            .collect()
    }

    pub fn drain_snapshots(&mut self) -> Vec<Vec<u8>> {
        let mut accepted = Vec::new();
        for bytes in drain_client_channel(&mut self.client, Channel::Snapshot) {
            let frame: ParticipationFrame = match wire::decode(&bytes) {
                Ok(frame) => frame,
                Err(err) => {
                    log::warn!("[Net] dropping unframed server Snapshot: {err}");
                    continue;
                }
            };
            if self.accept_snapshot_epoch(frame.participation_epoch) {
                accepted.push(frame.payload);
            }
        }
        accepted
    }

    /// Drain decoded passive presentation events from the dedicated unreliable
    /// channel. Unlike snapshots, these carry no participation epoch. The lane
    /// is still participation-gated: held/disconnected clients exhaust and
    /// discard it so a retired cosmetic cannot enter a later world lifecycle.
    pub fn drain_presentation(&mut self) -> Vec<ServerPresentationMessage> {
        let bytes = drain_client_channel(&mut self.client, Channel::Presentation);
        if !self.client.is_connected() || self.active_participation_epoch.is_none() {
            return Vec::new();
        }
        bytes
            .into_iter()
            .filter_map(|bytes| match wire::decode(&bytes) {
                Ok(message) => Some(message),
                Err(err) => {
                    log::warn!("[Net] dropping malformed server Presentation message: {err}");
                    None
                }
            })
            .collect()
    }

    /// Decode every server Control envelope currently queued by renet. Malformed
    /// payloads are isolated to this message: later reliable controls remain
    /// deliverable and the engine never needs to guess an untagged payload type.
    /// Route returned lifecycle controls before draining snapshots: epoch filtering
    /// keeps a later same-batch promotion's snapshots available.
    pub fn drain_control(&mut self) -> Vec<ServerControlMessage> {
        drain_client_channel(&mut self.client, Channel::Control)
            .into_iter()
            .filter_map(|bytes| {
                let frame: ServerControlFrame = match wire::decode(&bytes) {
                    Ok(frame) => frame,
                    Err(err) => {
                        log::warn!("[Net] dropping unframed server Control message: {err}");
                        return None;
                    }
                };
                let Some(payload) = frame.payload else {
                    if let Some(epoch) = frame.participation_epoch {
                        self.activate_participation(epoch);
                    } else {
                        log::warn!("[Net] dropping participation marker without an epoch");
                    }
                    return None;
                };
                let message: ServerControlMessage = match wire::decode(&payload) {
                    Ok(message) => message,
                    Err(err) => {
                        log::warn!("[Net] dropping malformed server Control message: {err}");
                        return None;
                    }
                };
                if matches!(
                    message,
                    ServerControlMessage::Divergence(DivergenceReason::Holding(_))
                ) && let Some(epoch) = frame.participation_epoch
                {
                    self.retire_participation(epoch);
                }
                Some(message)
            })
            .collect()
    }

    fn accept_snapshot_epoch(&self, epoch: u64) -> bool {
        self.active_participation_epoch == Some(epoch)
    }

    fn retire_participation(&mut self, epoch: u64) {
        if self
            .retired_participation_epoch
            .is_none_or(|retired| epoch_is_newer(epoch, retired))
        {
            self.retired_participation_epoch = Some(epoch);
        }
        if self
            .active_participation_epoch
            .is_some_and(|active| !epoch_is_newer(active, epoch))
        {
            self.active_participation_epoch = None;
            // Presentation is deliberately unframed and fire-and-forget. Flush
            // anything that arrived beside this reliable hold so it cannot be
            // mistaken for work from a later participation generation.
            drop(drain_client_channel(
                &mut self.client,
                Channel::Presentation,
            ));
        }
    }

    fn activate_participation(&mut self, epoch: u64) {
        if self.active_participation_epoch == Some(epoch) {
            return;
        }
        if self.active_participation_epoch.is_some() {
            return;
        }
        if self
            .retired_participation_epoch
            .is_some_and(|retired| !epoch_is_newer(epoch, retired))
        {
            return;
        }
        self.active_participation_epoch = Some(epoch);
    }

    pub fn packets_to_send(&mut self) -> Vec<Vec<u8>> {
        self.client.get_packets_to_send()
    }

    pub fn process_packet(&mut self, packet: &[u8]) {
        self.client.process_packet(packet);
    }

    pub fn set_connected(&mut self) {
        self.client.set_connected();
    }

    pub fn update_connections(&mut self, dt: Duration) {
        self.client.update(dt);
        self.queue_control_messages();
    }
}

fn epoch_is_newer(candidate: u64, reference: u64) -> bool {
    candidate != reference && candidate.wrapping_sub(reference) < (1_u64 << 63)
}

fn drain_client_channel(client: &mut RenetClient, channel: Channel) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while let Some(bytes) = client.receive_message(channel) {
        out.push(bytes.to_vec());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::reveal_both;
    use proptest::prelude::*;

    const RELAY_CLIENT_ID: ClientId = 41;

    fn loopback_server() -> (NetServer, SocketAddr) {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind server socket");
        let addr = socket.local_addr().expect("server local address");
        let server = NetServer::new(socket, addr, 8, Duration::from_secs(1), None)
            .expect("construct server");
        (server, addr)
    }

    fn loopback_client(server_addr: SocketAddr) -> NetClient {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind client socket");
        NetClient::new(socket, server_addr, 7, Duration::from_secs(1), None, None)
            .expect("construct client")
    }

    fn pump_until_connected(client: &mut NetClient, server: &mut NetServer) -> bool {
        let step = Duration::from_millis(16);
        for _ in 0..200 {
            client.update(step).expect("client update while connecting");
            let _ = server.update(step).expect("server update");
            if client.is_connected() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    /// Regression: a `--connect` boot blocks the main thread between endpoint
    /// construction and the first poll (renderer and mod init), and that whole
    /// gap arrived as the first `dt`, timing the connect out before a single
    /// request was sent. The first poll now starts the connect clock.
    #[test]
    fn a_stall_before_the_first_poll_does_not_time_out_the_connect() {
        let (mut server, server_addr) = loopback_server();
        let mut client = loopback_client(server_addr);

        client
            .update(Duration::from_secs(21))
            .expect("the first poll's stall is not counted");
        assert!(pump_until_connected(&mut client, &mut server));
        assert_eq!(client.connect_attempt, 1, "connected without a retry");
    }

    /// A host that starts after the client: the unanswered attempt is retried
    /// without surfacing an error, and the retry connects.
    #[test]
    fn a_host_that_starts_after_the_first_attempt_times_out_is_reached_by_a_retry() {
        let capture = postretro_test_log_capture::LogCapture::start();
        let reserved = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("reserve a port");
        let server_addr = reserved.local_addr().expect("reserved address");
        drop(reserved);
        let mut client = loopback_client(server_addr);

        client
            .update(Duration::ZERO)
            .expect("first poll sends a request");
        client
            .update(Duration::from_secs(16))
            .expect("the timed-out attempt is retried, not reported");
        assert_eq!(client.connect_attempt, 2);

        let socket = UdpSocket::bind(server_addr).expect("host binds the reserved port");
        let mut server = NetServer::new(socket, server_addr, 8, Duration::from_secs(1), None)
            .expect("construct server");
        assert!(pump_until_connected(&mut client, &mut server));
        capture.assert_not_logged(log::Level::Error, "could not connect");
    }

    #[test]
    fn an_unanswered_client_reports_one_failure_after_the_attempt_budget() {
        let capture = postretro_test_log_capture::LogCapture::start();
        // Bound but never read: requests land and nothing answers.
        let silent_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind silent socket");
        let silent = silent_socket.local_addr().expect("silent address");
        let mut client = loopback_client(silent);

        let results: Vec<bool> = (0..(CONNECT_ATTEMPTS * 4))
            .map(|_| client.update(Duration::from_secs(16)).is_err())
            .collect();
        let first_err = results
            .iter()
            .position(|&err| err)
            .expect("eventually fails");
        assert!(
            results[first_err..].iter().all(|&err| err),
            "failure is terminal"
        );
        assert_eq!(client.connect_attempt, CONNECT_ATTEMPTS);
        assert!(!client.is_connected());
        capture.assert_logged_once(
            log::Level::Error,
            &format!("could not connect to host {silent} after {CONNECT_ATTEMPTS} attempt(s)"),
        );
        drop(silent_socket);
    }

    fn relay_pair() -> (NetServer, NetClient) {
        let server_socket =
            UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("bind server socket");
        let server_addr = server_socket.local_addr().expect("server local address");
        let mut server =
            NetServer::new(server_socket, server_addr, 8, Duration::from_secs(1), None)
                .expect("construct relay server");
        let client_socket =
            UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("bind client socket");
        let mut client = NetClient::new(
            client_socket,
            server_addr,
            RELAY_CLIENT_ID,
            Duration::from_secs(1),
            None,
            None,
        )
        .expect("construct relay client");
        server.add_relay_connection(RELAY_CLIENT_ID, None);
        client.set_connected();
        (server, client)
    }

    fn relay_client_to_server(client: &mut NetClient, server: &mut NetServer) {
        client.update_connections(Duration::from_millis(16));
        for packet in client.packets_to_send() {
            server.process_packet_from(&packet, RELAY_CLIENT_ID);
        }
    }

    fn relay_server_to_client(server: &mut NetServer, client: &mut NetClient) {
        for packet in server.packets_to_send(RELAY_CLIENT_ID) {
            client.process_packet(&packet);
        }
    }

    fn matching_relay_pair() -> (NetServer, NetClient) {
        let (mut server, mut client) = relay_pair();
        server.set_mod_identity("postretro.test".to_string(), "1".to_string());
        server.set_mod_digest(Some([7; 32]));
        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        client.set_mod_identity("postretro.test".to_string(), "1".to_string());
        client.set_mod_digest(Some([7; 32]));
        client.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        reveal_both(&mut server, &mut client, "test-level");
        (server, client)
    }

    /// Matching content on both peers with no reveal on either side: both
    /// peers have installed and published parity but are still settling.
    fn settling_relay_pair() -> (NetServer, NetClient) {
        let (mut server, mut client) = relay_pair();
        server.set_mod_identity("postretro.test".to_string(), "1".to_string());
        server.set_mod_digest(Some([7; 32]));
        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        client.set_mod_identity("postretro.test".to_string(), "1".to_string());
        client.set_mod_digest(Some([7; 32]));
        client.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        (server, client)
    }

    fn participating_entries(lifecycle: &[SlotEvent]) -> usize {
        lifecycle
            .iter()
            .filter(|event| matches!(event, SlotEvent::Participating { .. }))
            .count()
    }

    fn holding_causes(controls: &[ServerControlMessage]) -> Vec<HoldingCause> {
        controls
            .iter()
            .filter_map(|message| match message {
                ServerControlMessage::Divergence(DivergenceReason::Holding(cause)) => {
                    Some(cause.clone())
                }
                _ => None,
            })
            .collect()
    }

    fn host_not_revealed() -> HoldingCause {
        HoldingCause::HostNotRevealed {
            identity: "test-level".to_string(),
        }
    }

    fn client_not_revealed() -> HoldingCause {
        HoldingCause::ClientNotRevealed {
            identity: "test-level".to_string(),
        }
    }

    fn participate_relay_pair() -> (NetServer, NetClient) {
        let (mut server, mut client) = matching_relay_pair();
        relay_client_to_server(&mut client, &mut server);
        let poll = server.poll_handshakes();
        assert_eq!(
            poll.handshakes,
            vec![HandshakeOutcome::Admitted {
                client_id: RELAY_CLIENT_ID
            }]
        );
        assert_eq!(
            poll.lifecycle,
            vec![SlotEvent::Participating {
                client_id: RELAY_CLIENT_ID
            }]
        );
        assert_eq!(
            poll.join_seeds,
            vec![(RELAY_CLIENT_ID, BTreeMap::new())],
            "a client with no retained values still sends an explicit empty join seed"
        );
        assert!(server.is_participating(RELAY_CLIENT_ID));
        (server, client)
    }

    #[test]
    fn presentation_channel_is_the_fourth_dedicated_unreliable_channel() {
        let config = connection_config();
        let channel_ids: Vec<u8> = config
            .server_channels_config
            .iter()
            .map(|channel| channel.channel_id)
            .collect();
        assert_eq!(
            channel_ids,
            vec![
                Channel::Control.into(),
                Channel::Snapshot.into(),
                Channel::Input.into(),
                Channel::Presentation.into(),
            ]
        );
        let presentation = config
            .server_channels_config
            .iter()
            .find(|channel| channel.channel_id == Channel::Presentation.into())
            .expect("presentation channel is configured");
        assert!(matches!(&presentation.send_type, SendType::Unreliable));
    }

    #[test]
    fn presentation_channel_delivers_a_typed_message_only_to_its_recipient() {
        let (mut server, mut client) = participate_relay_pair();
        let message = ServerPresentationMessage {
            payload: crate::wire::ServerPresentationPayload::Spawn {
                template_id: "damage-number".to_string(),
                anchor: [1.0, 2.0, 3.0],
                value: 25.0,
                facts: BTreeMap::from([(
                    "value".to_string(),
                    crate::wire::PresentationFact::Number(25.0),
                )]),
            },
        };

        assert!(server.send_presentation(RELAY_CLIENT_ID, message.clone()));
        relay_server_to_client(&mut server, &mut client);
        assert!(client.drain_control().is_empty());
        assert_eq!(client.drain_presentation(), vec![message]);
    }

    // Regression: unframed pre-demotion presentation survived a Holding drain
    // and was ingested after the same connection re-entered participation.
    #[test]
    fn demotion_discards_late_spawn_and_overlay_before_rejoin() {
        let (mut server, mut client) = participate_relay_pair();
        let stale_spawn = ServerPresentationMessage {
            payload: crate::wire::ServerPresentationPayload::Spawn {
                template_id: "old-number".to_string(),
                anchor: [1.0, 2.0, 3.0],
                value: 25.0,
                facts: BTreeMap::new(),
            },
        };
        let stale_overlay = ServerPresentationMessage {
            payload: crate::wire::ServerPresentationPayload::OverlayFact {
                enemy_id: crate::wire::NetworkId(3),
                health_fraction: 0.5,
                shield_fraction: 0.0,
                has_shield: false,
                alive: true,
            },
        };
        assert!(server.send_presentation(RELAY_CLIENT_ID, stale_spawn));
        assert!(server.send_presentation(RELAY_CLIENT_ID, stale_overlay));

        server.set_level_parity(None);
        assert!(matches!(
            server.poll_handshakes().lifecycle.as_slice(),
            [SlotEvent::Demoted { .. }]
        ));
        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        assert!(matches!(
            server.poll_handshakes().lifecycle.as_slice(),
            [SlotEvent::Participating { .. }]
        ));
        relay_server_to_client(&mut server, &mut client);
        assert!(matches!(
            client.drain_control().as_slice(),
            [ServerControlMessage::Divergence(DivergenceReason::Holding(
                HoldingCause::HostLevelAbsent
            ))]
        ));
        assert!(
            client.drain_presentation().is_empty(),
            "pre-demotion presentation must be retired even after re-promotion",
        );

        let current = ServerPresentationMessage {
            payload: crate::wire::ServerPresentationPayload::Spawn {
                template_id: "current-number".to_string(),
                anchor: [4.0, 5.0, 6.0],
                value: 10.0,
                facts: BTreeMap::new(),
            },
        };
        assert!(server.send_presentation(RELAY_CLIENT_ID, current.clone()));
        relay_server_to_client(&mut server, &mut client);
        assert_eq!(client.drain_presentation(), vec![current]);
    }

    fn observer_cue(ordinal: u8) -> wire::WeaponCue {
        wire::WeaponCue {
            shot_id: wire::WireShotId {
                pawn: 7,
                start_tick: 20,
                lane: 1,
                ordinal,
            },
            kind: wire::WeaponCueKind::Activate,
            sound: Some("alt_fire".into()),
            additional_sound: None,
            alias: Some("alt_activate".into()),
            anchor: wire::WeaponCueAnchor::Entity {
                entity: wire::NetworkId(7),
                origin: [1.0, 2.0, 3.0],
            },
        }
    }

    #[test]
    fn weapon_cues_reliable_delivery_survives_loss_duplicate_and_reordered_packets_once() {
        let (mut server, mut client) = participate_relay_pair();
        relay_server_to_client(&mut server, &mut client);
        let _ = client.drain_control();
        assert!(server.send_weapon_cues(RELAY_CLIENT_ID, vec![observer_cue(0)]));
        let lost = server.packets_to_send(RELAY_CLIENT_ID);
        assert!(!lost.is_empty());
        assert!(server.send_weapon_cues(RELAY_CLIENT_ID, vec![observer_cue(1)]));
        server.update_connections(Duration::from_millis(400));
        let mut packets = server.packets_to_send(RELAY_CLIENT_ID);
        packets.reverse();
        for packet in &packets {
            client.process_packet(packet);
            client.process_packet(packet);
        }
        // Any earlier lost packet that arrives late must also be inert.
        for packet in lost {
            client.process_packet(&packet);
        }
        let messages: Vec<_> = client
            .drain_input()
            .into_iter()
            .filter_map(
                |bytes| match wire::decode::<wire::ServerMessage>(&bytes).unwrap() {
                    wire::ServerMessage::WeaponCues(message) => Some(message),
                    _ => None,
                },
            )
            .collect();
        let ordinals: Vec<_> = messages
            .into_iter()
            .flat_map(|message| message.cues)
            .map(|cue| cue.shot_id.ordinal)
            .collect();
        assert_eq!(ordinals, [0, 1]);
        assert!(client.drain_input().is_empty());
    }

    #[test]
    fn weapon_cues_ignore_retired_participation_and_malformed_payloads() {
        let (mut server, mut client) = participate_relay_pair();
        relay_server_to_client(&mut server, &mut client);
        let _ = client.drain_control();
        let old_epoch = client.active_participation_epoch.unwrap();
        assert!(server.send_weapon_cues(RELAY_CLIENT_ID, vec![observer_cue(0)]));
        let delayed = server.packets_to_send(RELAY_CLIENT_ID);
        server.set_level_parity(None);
        let _ = server.poll_handshakes();
        relay_server_to_client(&mut server, &mut client);
        let _ = client.drain_control();
        assert!(!server.send_weapon_cues(RELAY_CLIENT_ID, vec![observer_cue(1)]));
        server.set_level_parity(Some(("test-level".into(), [9; 32])));
        let _ = server.poll_handshakes();
        relay_server_to_client(&mut server, &mut client);
        let _ = client.drain_control();
        assert_ne!(client.active_participation_epoch, Some(old_epoch));
        for packet in delayed {
            client.process_packet(&packet);
        }
        assert!(client.drain_input().is_empty());
        let mut invalid = observer_cue(0);
        invalid.shot_id.lane = 7;
        let epoch = client.active_participation_epoch.unwrap();
        server.send_input(
            RELAY_CLIENT_ID,
            wire::encode(&wire::ServerMessage::WeaponCues(wire::WeaponCuesMessage {
                participation_epoch: epoch,
                cues: vec![invalid],
            })),
        );
        assert!(server.send_weapon_cues(RELAY_CLIENT_ID, vec![observer_cue(1)]));
        server.update_connections(Duration::from_millis(400));
        relay_server_to_client(&mut server, &mut client);
        let accepted = client.drain_input();
        assert_eq!(accepted.len(), 1);
        let wire::ServerMessage::WeaponCues(message) = wire::decode(&accepted[0]).unwrap() else {
            panic!("current cue");
        };
        assert_eq!(message.cues[0].shot_id.ordinal, 1);
    }

    #[test]
    fn parity_precedence_reports_mod_before_level() {
        let declaration = ParityDeclaration {
            mod_digest: [2; 32],
            level: None,
        };
        assert!(matches!(
            parity_cause(Some([1; 32]), None, Some(&declaration)),
            Some(HoldingCause::ModDigest { .. })
        ));
    }

    // Regression: evaluating admission before the next ordered parity message
    // emitted a transient HostLevelAbsent diagnostic on a normal join.
    #[test]
    fn ordered_admission_then_matching_parity_has_no_transient_hold() {
        let (mut server, mut client) = participate_relay_pair();

        relay_server_to_client(&mut server, &mut client);
        assert!(
            client.drain_control().is_empty(),
            "matching admission and parity must not emit a holding diagnostic"
        );
    }

    #[test]
    fn participating_switch_declaration_reaches_the_server_poll() {
        let (mut server, mut client) = participate_relay_pair();
        // The participation marker normally establishes this before gameplay
        // controls are emitted. Set it directly here so the public client send
        // path, rather than the test-only raw Renet handle, is exercised.
        client.active_participation_epoch = Some(1);
        client.send_switch_declaration(ClientSwitchDeclaration {
            declaration_id: 9,
            slot: 2,
        });

        relay_client_to_server(&mut client, &mut server);
        let poll = server.poll_handshakes();

        assert!(poll.handshakes.is_empty());
        assert!(poll.lifecycle.is_empty());
        assert_eq!(
            poll.switch_declarations,
            vec![(
                RELAY_CLIENT_ID,
                ClientSwitchDeclaration {
                    declaration_id: 9,
                    slot: 2,
                },
            )]
        );
    }

    #[test]
    fn join_seed_reaches_the_server_poll_without_transport_interpretation() {
        let (mut server, mut client) = participate_relay_pair();
        let slots =
            BTreeMap::from([("kplayer0000000001".to_string(), JoinSeedValue::Number(42.0))]);
        client.client.send_message(
            Channel::Control,
            wire::encode(&ClientControlMessage::JoinSeed {
                slots: slots.clone(),
            }),
        );

        relay_client_to_server(&mut client, &mut server);
        let poll = server.poll_handshakes();

        assert_eq!(poll.join_seeds, vec![(RELAY_CLIENT_ID, slots)]);
    }

    // Regression: parity retained while Pending was never reconsidered when the
    // following ordered admission moved the slot to Admitted.
    #[test]
    fn pending_parity_is_reconsidered_after_admission_without_stale_batch_diagnostic() {
        let (mut server, mut client) = relay_pair();
        server.set_mod_identity("postretro.test".to_string(), "1".to_string());
        server.set_mod_digest(Some([7; 32]));
        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        reveal_both(&mut server, &mut client, "test-level");

        let stale = ParityDeclaration {
            mod_digest: [3; 32],
            level: None,
        };
        let matching = ParityDeclaration {
            mod_digest: [7; 32],
            level: Some(("test-level".to_string(), [9; 32])),
        };
        client.client.send_message(
            Channel::Control,
            wire::encode(&ClientControlMessage::Parity(stale)),
        );
        client.client.send_message(
            Channel::Control,
            wire::encode(&ClientControlMessage::Admission {
                protocol: protocol_version(),
                mod_id: "postretro.test".to_string(),
                mod_version: "1".to_string(),
            }),
        );
        client.client.send_message(
            Channel::Control,
            wire::encode(&ClientControlMessage::Parity(matching)),
        );

        relay_client_to_server(&mut client, &mut server);
        let poll = server.poll_handshakes();
        assert_eq!(
            poll.handshakes,
            vec![HandshakeOutcome::Admitted {
                client_id: RELAY_CLIENT_ID
            }]
        );
        assert_eq!(
            poll.lifecycle,
            vec![SlotEvent::Participating {
                client_id: RELAY_CLIENT_ID
            }]
        );
        assert!(server.is_participating(RELAY_CLIENT_ID));

        relay_server_to_client(&mut server, &mut client);
        assert!(
            client.drain_control().is_empty(),
            "only the final ordered declaration may determine the batch verdict"
        );
    }

    // Regression: admitted Input was left queued until promotion, risking reliable
    // channel overflow and replaying stale traffic into the simulation.
    #[test]
    fn held_input_is_discarded_on_every_server_poll() {
        let (mut server, mut client) = relay_pair();
        server.set_mod_identity("postretro.test".to_string(), "1".to_string());
        server.set_mod_digest(Some([7; 32]));
        client.set_mod_identity("postretro.test".to_string(), "1".to_string());
        client.set_mod_digest(Some([7; 32]));

        relay_client_to_server(&mut client, &mut server);
        let poll = server.poll_handshakes();
        assert_eq!(
            server.slot_state(RELAY_CLIENT_ID),
            Some(SlotState::Admitted)
        );
        assert!(matches!(
            poll.handshakes.as_slice(),
            [
                HandshakeOutcome::Admitted { .. },
                HandshakeOutcome::ParityHeld {
                    cause: HoldingCause::HostLevelAbsent,
                    ..
                }
            ]
        ));

        for sample_id in 0..64 {
            client.send_input(wire::encode(&crate::wire::ClientMessage::TimeSync(
                crate::timesync::TimeSyncRequest {
                    sample_id,
                    client_send_tick: sample_id,
                    client_send_time_us: u64::from(sample_id),
                },
            )));
            relay_client_to_server(&mut client, &mut server);
            let _ = server.poll_handshakes();
            assert!(
                server.input_is_empty(RELAY_CLIENT_ID),
                "held traffic from poll {sample_id} must not reach simulation"
            );
        }
    }

    // Regression: clearing the host digest skipped predicate re-evaluation and
    // left a slot participating against an incomplete installed triple.
    #[test]
    fn clearing_mod_digest_demotes_participating_slot() {
        let (mut server, _client) = participate_relay_pair();

        server.set_mod_digest(None);
        let poll = server.poll_handshakes();
        assert_eq!(
            server.slot_state(RELAY_CLIENT_ID),
            Some(SlotState::Admitted)
        );
        assert_eq!(
            poll.lifecycle,
            vec![SlotEvent::Demoted {
                client_id: RELAY_CLIENT_ID,
                cause: HoldingCause::HostLevelAbsent,
            }]
        );
    }

    // Regression: reject closed a participating slot but discarded the close edge,
    // leaking every host table cleaned through SlotEvent.
    #[test]
    fn malformed_control_from_participant_surfaces_one_cleanup_event() {
        let (mut server, mut client) = participate_relay_pair();
        client.client.send_message(Channel::Control, Vec::new());

        relay_client_to_server(&mut client, &mut server);
        let poll = server.poll_handshakes();
        assert!(matches!(
            poll.handshakes.as_slice(),
            [HandshakeOutcome::Rejected { .. }]
        ));
        assert_eq!(
            poll.lifecycle,
            vec![SlotEvent::Closed {
                client_id: RELAY_CLIENT_ID,
                cause: CloseCause::Timeout,
            }]
        );
        assert!(server.poll_handshakes().lifecycle.is_empty());
    }

    // Regression: admission rejection disconnected after one send attempt, so one
    // lost datagram erased the typed cause.
    #[test]
    fn admission_rejection_waits_for_control_ack_before_disconnect() {
        let (mut server, mut client) = relay_pair();
        server.set_mod_identity("postretro.host".to_string(), "1".to_string());
        client.set_mod_identity("postretro.client".to_string(), "1".to_string());

        relay_client_to_server(&mut client, &mut server);
        let poll = server.poll_handshakes();
        let cause = match poll.handshakes.as_slice() {
            [HandshakeOutcome::Rejected { cause, .. }] => cause.clone(),
            other => panic!("expected rejection, got {other:?}"),
        };
        assert!(
            !server.connected_clients().is_empty(),
            "closed slot retains its transport until the reliable cause is acked"
        );

        relay_server_to_client(&mut server, &mut client);
        assert_eq!(
            client.drain_control(),
            vec![ServerControlMessage::Divergence(DivergenceReason::Closing(
                cause
            ))]
        );
        relay_client_to_server(&mut client, &mut server);
        let _ = server.poll_handshakes();
        assert!(
            server.connected_clients().is_empty(),
            "transport closes only after the rejection message is acknowledged"
        );
    }

    // Regression: each duplicate held declaration enqueued another reliable
    // diagnostic, allowing a parity flood to exhaust Control memory.
    #[test]
    fn duplicate_held_parity_flood_emits_one_diagnostic_for_one_cause() {
        let (mut server, mut client) = relay_pair();
        server.set_mod_identity("postretro.test".to_string(), "1".to_string());
        server.set_mod_digest(Some([7; 32]));
        client.set_mod_identity("postretro.test".to_string(), "1".to_string());
        client.set_mod_digest(Some([7; 32]));

        relay_client_to_server(&mut client, &mut server);
        let _ = server.poll_handshakes();
        let duplicate = ParityDeclaration {
            mod_digest: [7; 32],
            level: None,
        };
        for _ in 0..64 {
            client.client.send_message(
                Channel::Control,
                wire::encode(&ClientControlMessage::Parity(duplicate.clone())),
            );
            relay_client_to_server(&mut client, &mut server);
            let _ = server.poll_handshakes();
        }

        relay_server_to_client(&mut server, &mut client);
        let controls = client.drain_control();
        assert_eq!(
            controls
                .iter()
                .filter(|message| matches!(
                    message,
                    ServerControlMessage::Divergence(DivergenceReason::Holding(_))
                ))
                .count(),
            1
        );
    }

    // Regression: the first holding diagnostic suppressed every later mismatch,
    // leaving both the client and gate-outcome consumer with a stale cause.
    #[test]
    fn changed_holding_causes_reach_control_and_message_outcomes() {
        let (mut server, mut client) = relay_pair();
        server.set_mod_identity("postretro.test".to_string(), "1".to_string());
        server.set_mod_digest(Some([7; 32]));
        client.set_mod_identity("postretro.test".to_string(), "1".to_string());
        client.set_mod_digest(Some([7; 32]));

        relay_client_to_server(&mut client, &mut server);
        let initial = server.poll_handshakes();
        assert!(matches!(
            initial.handshakes.as_slice(),
            [
                HandshakeOutcome::Admitted { .. },
                HandshakeOutcome::ParityHeld {
                    cause: HoldingCause::HostLevelAbsent,
                    ..
                }
            ]
        ));
        relay_server_to_client(&mut server, &mut client);
        assert!(matches!(
            client.drain_control().as_slice(),
            [ServerControlMessage::Divergence(DivergenceReason::Holding(
                HoldingCause::HostLevelAbsent
            ))]
        ));

        server.set_level_parity(Some(("host-map".to_string(), [9; 32])));
        relay_server_to_client(&mut server, &mut client);
        assert!(matches!(
            client.drain_control().as_slice(),
            [ServerControlMessage::Divergence(DivergenceReason::Holding(
                HoldingCause::LevelAbsent { .. }
            ))]
        ));

        client.set_level_parity(Some(("client-map".to_string(), [9; 32])));
        relay_client_to_server(&mut client, &mut server);
        let changed = server.poll_handshakes();
        assert!(matches!(
            changed.handshakes.as_slice(),
            [HandshakeOutcome::ParityHeld {
                cause: HoldingCause::LevelIdentity { .. },
                ..
            }]
        ));
        relay_server_to_client(&mut server, &mut client);
        assert!(matches!(
            client.drain_control().as_slice(),
            [ServerControlMessage::Divergence(DivergenceReason::Holding(
                HoldingCause::LevelIdentity { .. }
            ))]
        ));
    }

    // Regression: delayed Snapshot and Input packets from a retired participation
    // generation could mutate the fresh client world and newly-spawned host pawn.
    #[test]
    fn participation_epoch_rejects_stale_cross_boundary_traffic_and_accepts_current() {
        let (mut server, mut client) = participate_relay_pair();
        relay_server_to_client(&mut server, &mut client);
        assert!(client.drain_control().is_empty());

        assert!(server.send_snapshot(RELAY_CLIENT_ID, vec![1]));
        relay_server_to_client(&mut server, &mut client);
        assert_eq!(client.drain_snapshots(), vec![vec![1]]);

        client.send_input(vec![10]);
        let stale_input_packets = client.packets_to_send();
        assert!(server.send_snapshot(RELAY_CLIENT_ID, vec![2]));
        let stale_snapshot_packets = server.packets_to_send(RELAY_CLIENT_ID);

        server.set_level_parity(None);
        assert!(matches!(
            server.poll_handshakes().lifecycle.as_slice(),
            [SlotEvent::Demoted { .. }]
        ));
        relay_server_to_client(&mut server, &mut client);
        assert!(matches!(
            client.drain_control().as_slice(),
            [ServerControlMessage::Divergence(DivergenceReason::Holding(
                HoldingCause::HostLevelAbsent
            ))]
        ));

        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        assert!(matches!(
            server.poll_handshakes().lifecycle.as_slice(),
            [SlotEvent::Participating { .. }]
        ));
        relay_server_to_client(&mut server, &mut client);
        assert!(client.drain_control().is_empty());

        for packet in stale_snapshot_packets {
            client.process_packet(&packet);
        }
        assert!(
            client.drain_snapshots().is_empty(),
            "retired-generation snapshots must not reach engine apply"
        );
        for packet in stale_input_packets {
            server.process_packet_from(&packet, RELAY_CLIENT_ID);
        }
        assert!(
            server.drain_input(RELAY_CLIENT_ID).is_empty(),
            "retired-generation Input must not reach the re-promoted pawn"
        );

        assert!(server.send_snapshot(RELAY_CLIENT_ID, vec![3]));
        relay_server_to_client(&mut server, &mut client);
        assert_eq!(client.drain_snapshots(), vec![vec![3]]);
        client.send_input(vec![11]);
        relay_client_to_server(&mut client, &mut server);
        assert_eq!(server.drain_input(RELAY_CLIENT_ID), vec![vec![11]]);
    }

    // Regression: a host-side promote followed by demotion before one poll was
    // returned left the engine to apply historical participation side effects.
    #[test]
    fn rapid_promotion_then_demotion_reports_both_edges_but_finishes_holding() {
        let (mut server, _client) = participate_relay_pair();
        server.set_level_parity(None);
        assert!(matches!(
            server.poll_handshakes().lifecycle.as_slice(),
            [SlotEvent::Demoted { .. }]
        ));

        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        server.set_level_parity(None);
        let poll = server.poll_handshakes();

        assert!(matches!(
            poll.lifecycle.as_slice(),
            [SlotEvent::Participating { .. }, SlotEvent::Demoted { .. }]
        ));
        assert!(
            !server.is_current_participation_entry(&poll.lifecycle[0]),
            "historical entry must not trigger spawn or tuning side effects"
        );
        assert!(
            !server.is_participating(RELAY_CLIENT_ID),
            "engine participation side effects must follow the final predicate"
        );
    }

    // Regression: an unreliable Snapshot that overtook both its participation
    // marker and a later hold armed a retired epoch and restored demoted state.
    #[test]
    fn snapshot_overtaking_marker_and_hold_cannot_arm_participation_epoch() {
        let (mut server, mut client) = participate_relay_pair();
        let marker_packets = server.packets_to_send(RELAY_CLIENT_ID);
        assert!(
            !marker_packets.is_empty(),
            "promotion must queue a reliable participation marker"
        );

        assert!(server.send_snapshot(RELAY_CLIENT_ID, vec![1]));
        let snapshot_packets = server.packets_to_send(RELAY_CLIENT_ID);
        assert!(
            !snapshot_packets.is_empty(),
            "participating host must queue the snapshot"
        );

        server.set_level_parity(None);
        assert!(matches!(
            server.poll_handshakes().lifecycle.as_slice(),
            [SlotEvent::Demoted { .. }]
        ));
        let holding_packets = server.packets_to_send(RELAY_CLIENT_ID);
        assert!(
            !holding_packets.is_empty(),
            "demotion must queue a reliable holding diagnostic"
        );

        for packet in snapshot_packets {
            client.process_packet(&packet);
        }
        assert!(
            client.drain_snapshots().is_empty(),
            "Snapshot cannot establish participation before reliable Control"
        );

        for packet in marker_packets {
            client.process_packet(&packet);
        }
        for packet in holding_packets {
            client.process_packet(&packet);
        }
        assert!(matches!(
            client.drain_control().as_slice(),
            [ServerControlMessage::Divergence(DivergenceReason::Holding(
                HoldingCause::HostLevelAbsent
            ))]
        ));
        assert!(
            client.drain_snapshots().is_empty(),
            "the overtaking snapshot must remain dropped after epoch retirement"
        );

        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        assert!(matches!(
            server.poll_handshakes().lifecycle.as_slice(),
            [SlotEvent::Participating { .. }]
        ));
        relay_server_to_client(&mut server, &mut client);
        assert!(client.drain_control().is_empty());

        assert!(server.send_snapshot(RELAY_CLIENT_ID, vec![2]));
        relay_server_to_client(&mut server, &mut client);
        assert_eq!(
            client.drain_snapshots(),
            vec![vec![2]],
            "re-promotion marker must arm its new epoch"
        );
    }

    // Regression: a Snapshot queued before a hold could be applied after Control
    // announced demotion, restoring entities and prediction while held.
    #[test]
    fn holding_control_discards_snapshots_already_queued_on_client() {
        let (mut server, mut client) = participate_relay_pair();
        assert!(server.send_snapshot(RELAY_CLIENT_ID, vec![1, 2, 3]));
        server.send_divergence(RELAY_CLIENT_ID, HoldingCause::HostLevelAbsent);
        relay_server_to_client(&mut server, &mut client);

        assert!(matches!(
            client.drain_control().as_slice(),
            [ServerControlMessage::Divergence(DivergenceReason::Holding(
                HoldingCause::HostLevelAbsent
            ))]
        ));
        assert!(
            client.drain_snapshots().is_empty(),
            "the pre-hold snapshot must not survive for engine apply"
        );
    }

    // Regression: Holding N and a later marker N+1 in one reliable Control drain
    // caused the client to discard N+1 snapshots along with retired N traffic.
    #[test]
    fn coalesced_hold_and_repromotion_keep_only_current_epoch_snapshot() {
        let (mut server, mut client) = participate_relay_pair();
        assert!(server.send_snapshot(RELAY_CLIENT_ID, vec![1]));

        server.set_level_parity(None);
        assert!(matches!(
            server.poll_handshakes().lifecycle.as_slice(),
            [SlotEvent::Demoted { .. }]
        ));
        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        assert!(matches!(
            server.poll_handshakes().lifecycle.as_slice(),
            [SlotEvent::Participating { .. }]
        ));
        assert!(server.send_snapshot(RELAY_CLIENT_ID, vec![2]));

        relay_server_to_client(&mut server, &mut client);
        assert!(matches!(
            client.drain_control().as_slice(),
            [ServerControlMessage::Divergence(DivergenceReason::Holding(
                HoldingCause::HostLevelAbsent
            ))]
        ));
        assert_eq!(
            client.drain_snapshots(),
            vec![vec![2]],
            "retired N is fenced while current N+1 remains available to engine apply"
        );
    }

    #[test]
    fn late_admission_receives_the_installed_relevel_catalog_id() {
        let (mut server, mut client) = relay_pair();
        server.set_mod_identity("postretro.test".to_string(), "1".to_string());
        server.set_relevel_catalog_id(Some("e1m1".to_string()));
        client.set_mod_identity("postretro.test".to_string(), "1".to_string());

        relay_client_to_server(&mut client, &mut server);
        let poll = server.poll_handshakes();
        assert!(matches!(
            poll.handshakes.as_slice(),
            [HandshakeOutcome::Admitted {
                client_id: RELAY_CLIENT_ID
            }]
        ));

        relay_server_to_client(&mut server, &mut client);
        assert_eq!(
            client.drain_control(),
            vec![ServerControlMessage::Relevel("e1m1".to_string())],
            "a client admitted after the host installed a catalog level must be told the current map"
        );
    }

    #[test]
    fn installing_a_catalog_level_notifies_an_already_admitted_client() {
        let (mut server, mut client) = relay_pair();
        server.set_mod_identity("postretro.test".to_string(), "1".to_string());
        client.set_mod_identity("postretro.test".to_string(), "1".to_string());

        relay_client_to_server(&mut client, &mut server);
        assert!(matches!(
            server.poll_handshakes().handshakes.as_slice(),
            [HandshakeOutcome::Admitted {
                client_id: RELAY_CLIENT_ID
            }]
        ));
        assert!(client.drain_control().is_empty());

        server.set_relevel_catalog_id(Some("e1m2".to_string()));
        relay_server_to_client(&mut server, &mut client);
        assert_eq!(
            client.drain_control(),
            vec![ServerControlMessage::Relevel("e1m2".to_string())],
            "a catalog install must announce the next map to admitted clients"
        );
    }

    #[test]
    fn close_slot_reports_each_live_connection_once_and_clears_its_claim() {
        let (mut server, _client) = relay_pair();
        let claim = ConnectClaim {
            player_id: crate::wire::PlayerClaimId([0x6c; 16]),
            display_name: "Neon Runner".to_string(),
        };
        server.connect_claims.insert(RELAY_CLIENT_ID, claim);

        assert_eq!(
            server.close_relay_connection(RELAY_CLIENT_ID, CloseCause::Disconnect),
            None,
            "a pending connection has no lifecycle event"
        );
        assert_eq!(server.connect_claim(RELAY_CLIENT_ID), None);
        assert_eq!(server.poll_handshakes().disconnects, vec![RELAY_CLIENT_ID]);

        assert_eq!(
            server.close_slot(RELAY_CLIENT_ID, CloseCause::Disconnect),
            None
        );
        assert!(
            server.poll_handshakes().disconnects.is_empty(),
            "a closed slot cannot emit a second unbind"
        );
    }

    #[test]
    fn close_slot_does_not_report_never_connected_tombstones() {
        let (mut server, _client) = relay_pair();

        assert_eq!(server.close_slot(99, CloseCause::Timeout), None);
        assert!(
            server.poll_handshakes().disconnects.is_empty(),
            "an unknown close records a stale-packet tombstone, not a disconnect"
        );
    }

    #[test]
    fn reject_reports_disconnect_in_its_same_poll() {
        let (mut server, _client) = relay_pair();
        server.reject(
            RELAY_CLIENT_ID,
            ClosingCause::Protocol {
                expected: protocol_version(),
                received: crate::wire::ProtocolVersion {
                    app_protocol_id: 0,
                    wire_version: 0,
                },
            },
        );

        assert_eq!(server.poll_handshakes().disconnects, vec![RELAY_CLIENT_ID]);
    }

    #[test]
    fn netcode_connection_stashes_decoded_connect_claim() {
        let server_socket =
            UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("bind server socket");
        let server_addr = server_socket.local_addr().expect("server local address");
        let mut server =
            NetServer::new(server_socket, server_addr, 8, Duration::from_secs(1), None)
                .expect("construct server");
        let claim = ConnectClaim {
            player_id: crate::wire::PlayerClaimId([0x1b; 16]),
            display_name: "Neon Runner".to_string(),
        };
        let client_socket =
            UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("bind client socket");
        let mut client = NetClient::new(
            client_socket,
            server_addr,
            RELAY_CLIENT_ID,
            Duration::from_secs(1),
            None,
            Some(crate::wire::encode_connect_claim(&claim)),
        )
        .expect("construct client");

        for _ in 0..32 {
            client
                .update(Duration::from_millis(16))
                .expect("advance client transport");
            let _ = server
                .update(Duration::from_millis(16))
                .expect("advance server transport");
            if server.connect_claim(RELAY_CLIENT_ID).is_some() {
                break;
            }
        }

        assert_eq!(server.connect_claim(RELAY_CLIENT_ID), Some(&claim));
    }

    #[test]
    fn relay_connection_stashes_decoded_connect_claim() {
        let server_socket =
            UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("bind server socket");
        let server_addr = server_socket.local_addr().expect("server local address");
        let mut server =
            NetServer::new(server_socket, server_addr, 8, Duration::from_secs(1), None)
                .expect("construct relay server");
        let claim = ConnectClaim {
            player_id: crate::wire::PlayerClaimId([0x3e; 16]),
            display_name: "Relay Runner".to_string(),
        };

        server.add_relay_connection(
            RELAY_CLIENT_ID,
            Some(crate::wire::encode_connect_claim(&claim)),
        );

        assert_eq!(server.connect_claim(RELAY_CLIENT_ID), Some(&claim));
    }

    #[test]
    fn repeated_pending_and_participating_closes_bound_connect_claim_stash() {
        const CYCLES: u64 = 32;
        let (mut server, _client) = relay_pair();
        let _ = server.close_relay_connection(RELAY_CLIENT_ID, CloseCause::Disconnect);
        let _ = server.poll_handshakes();

        for cycle in 0..CYCLES {
            let pending_id = 100 + cycle * 2;
            let participating_id = pending_id + 1;
            let pending_claim = ConnectClaim {
                player_id: crate::wire::PlayerClaimId([cycle as u8; 16]),
                display_name: "Pending Runner".to_owned(),
            };
            server.add_relay_connection(
                pending_id,
                Some(crate::wire::encode_connect_claim(&pending_claim)),
            );
            assert_eq!(server.connect_claim_count(), 1);
            let _ = server.close_relay_connection(pending_id, CloseCause::Disconnect);
            assert_eq!(
                server.connect_claim_count(),
                0,
                "closing a pending transport slot removes its asserted claim"
            );
            let _ = server.poll_handshakes();

            let participating_claim = ConnectClaim {
                player_id: crate::wire::PlayerClaimId([0x80 | cycle as u8; 16]),
                display_name: "Participating Runner".to_owned(),
            };
            server.add_relay_connection(
                participating_id,
                Some(crate::wire::encode_connect_claim(&participating_claim)),
            );
            let _ = server.slots.admit(participating_id);
            let _ = server.slots.participate(participating_id);
            assert_eq!(server.connect_claim_count(), 1);
            let _ = server.close_relay_connection(participating_id, CloseCause::Disconnect);
            assert_eq!(
                server.connect_claim_count(),
                0,
                "closing a participating transport slot removes its asserted claim"
            );
            let _ = server.poll_handshakes();
        }
    }

    #[test]
    fn participation_cause_ranks_revealed_below_every_content_cause() {
        let installed = ("test-level".to_string(), [9; 32]);
        let matching = ParityDeclaration {
            mod_digest: [7; 32],
            level: Some(installed.clone()),
        };
        let divergent = ParityDeclaration {
            mod_digest: [7; 32],
            level: Some(("test-level".to_string(), [8; 32])),
        };

        assert!(matches!(
            participation_cause(
                Some([7; 32]),
                Some(&installed),
                Some(&divergent),
                None,
                None
            ),
            Some(HoldingCause::LevelDigest { .. })
        ));
        assert_eq!(
            participation_cause(Some([7; 32]), Some(&installed), Some(&matching), None, None),
            Some(host_not_revealed()),
            "the host term is checked before the client term"
        );
        assert_eq!(
            participation_cause(
                Some([7; 32]),
                Some(&installed),
                Some(&matching),
                Some("test-level"),
                Some("other-level"),
            ),
            Some(client_not_revealed())
        );
        assert_eq!(
            participation_cause(
                Some([7; 32]),
                Some(&installed),
                Some(&matching),
                Some("test-level"),
                Some("test-level"),
            ),
            None
        );
    }

    // parity publishes at install, so a content-divergent client learns
    // its cause while both peers are still settling, ahead of any revealed cause.
    #[test]
    fn parity_published_at_install_reaches_settling_client() {
        let (mut server, mut client) = settling_relay_pair();
        client.set_level_parity(Some(("client-map".to_string(), [9; 32])));

        relay_client_to_server(&mut client, &mut server);
        let poll = server.poll_handshakes();
        assert!(matches!(
            poll.handshakes.as_slice(),
            [
                HandshakeOutcome::Admitted { .. },
                HandshakeOutcome::ParityHeld {
                    cause: HoldingCause::LevelIdentity { .. },
                    ..
                }
            ]
        ));
        relay_server_to_client(&mut server, &mut client);
        assert!(matches!(
            holding_causes(&client.drain_control()).as_slice(),
            [HoldingCause::LevelIdentity { .. }]
        ));

        // Once the content matches, the hold names the remaining revealed term.
        client.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        relay_client_to_server(&mut client, &mut server);
        let _ = server.poll_handshakes();
        relay_server_to_client(&mut server, &mut client);
        assert_eq!(
            holding_causes(&client.drain_control()),
            vec![host_not_revealed()]
        );
        assert!(!server.is_participating(RELAY_CLIENT_ID));
    }

    // a parity-matched settling client is held with no participation
    // marker; its reveal promotes it on the poll that drains it. Parity and the
    // reveal in one batch promote once.
    #[test]
    fn revealed_declaration_promotes_once_even_batched_with_parity() {
        let (mut server, mut client) = settling_relay_pair();
        server.set_revealed_level(Some("test-level".to_string()));

        relay_client_to_server(&mut client, &mut server);
        let held = server.poll_handshakes();
        assert_eq!(participating_entries(&held.lifecycle), 0);
        assert_eq!(
            server.slot_state(RELAY_CLIENT_ID),
            Some(SlotState::Admitted)
        );
        assert!(!server.send_snapshot(RELAY_CLIENT_ID, vec![1]));
        relay_server_to_client(&mut server, &mut client);
        assert_eq!(
            holding_causes(&client.drain_control()),
            vec![client_not_revealed()]
        );
        assert!(!client.is_participating(), "no epoch marker while held");

        client.set_revealed_level(Some("test-level".to_string()));
        relay_client_to_server(&mut client, &mut server);
        let promoted = server.poll_handshakes();
        assert_eq!(
            promoted.lifecycle,
            vec![SlotEvent::Participating {
                client_id: RELAY_CLIENT_ID
            }]
        );
        relay_server_to_client(&mut server, &mut client);
        assert!(client.drain_control().is_empty());
        assert!(client.is_participating());

        // Batched: admission, parity, join seed and reveal in one host drain.
        let (mut server, mut client) = settling_relay_pair();
        reveal_both(&mut server, &mut client, "test-level");
        relay_client_to_server(&mut client, &mut server);
        let poll = server.poll_handshakes();
        assert_eq!(
            poll.lifecycle,
            vec![SlotEvent::Participating {
                client_id: RELAY_CLIENT_ID
            }],
            "one promotion, one pawn"
        );
        assert!(
            !poll
                .handshakes
                .iter()
                .any(|outcome| matches!(outcome, HandshakeOutcome::ParityHeld { .. })),
            "no transient revealed hold inside the batch"
        );
        relay_server_to_client(&mut server, &mut client);
        assert!(client.drain_control().is_empty());
        assert!(client.is_participating());
    }

    // the host's reveal, timed out or settled alike, promotes
    // a revealed, parity-matched client.
    #[test]
    fn host_reveal_promotes_revealed_matched_client() {
        let (mut server, mut client) = settling_relay_pair();
        client.set_revealed_level(Some("test-level".to_string()));
        relay_client_to_server(&mut client, &mut server);
        let held = server.poll_handshakes();
        assert!(matches!(
            held.handshakes.as_slice(),
            [
                HandshakeOutcome::Admitted { .. },
                HandshakeOutcome::ParityHeld { cause, .. }
            ] if *cause == host_not_revealed()
        ));

        server.set_revealed_level(Some("test-level".to_string()));
        assert_eq!(
            server.poll_handshakes().lifecycle,
            vec![SlotEvent::Participating {
                client_id: RELAY_CLIENT_ID
            }]
        );
    }

    // a reveal declared while parity mismatched is retained and promotes
    // the moment parity matches, with no second declaration.
    #[test]
    fn revealed_before_parity_match_promotes_on_match() {
        let (mut server, mut client) = settling_relay_pair();
        server.set_revealed_level(Some("test-level".to_string()));
        client.set_level_parity(Some(("test-level".to_string(), [8; 32])));
        client.set_revealed_level(Some("test-level".to_string()));
        relay_client_to_server(&mut client, &mut server);
        assert!(matches!(
            server.poll_handshakes().handshakes.as_slice(),
            [
                HandshakeOutcome::Admitted { .. },
                HandshakeOutcome::ParityHeld {
                    cause: HoldingCause::LevelDigest { .. },
                    ..
                }
            ]
        ));

        client.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        assert!(client.revealed_sent, "the reveal is not declared again");
        relay_client_to_server(&mut client, &mut server);
        assert_eq!(
            server.poll_handshakes().lifecycle,
            vec![SlotEvent::Participating {
                client_id: RELAY_CLIENT_ID
            }]
        );
    }

    // a client that never reveals stays admitted and connected, held
    // under the revealed cause, and is sent no entity state.
    #[test]
    fn unrevealed_client_stays_admitted_and_receives_no_entity_state() {
        let (mut server, mut client) = settling_relay_pair();
        server.set_revealed_level(Some("test-level".to_string()));
        relay_client_to_server(&mut client, &mut server);
        let _ = server.poll_handshakes();

        // About 11.5 s of polls, past the engine's ten-second Settling timeout.
        for _ in 0..(12 * 60) {
            relay_client_to_server(&mut client, &mut server);
            assert!(server.poll_handshakes().lifecycle.is_empty());
            assert!(!server.send_snapshot(RELAY_CLIENT_ID, vec![1]));
            server.update_connections(Duration::from_millis(16));
            relay_server_to_client(&mut server, &mut client);
        }

        assert_eq!(server.connected_clients(), vec![RELAY_CLIENT_ID]);
        assert_eq!(
            server.slot_state(RELAY_CLIENT_ID),
            Some(SlotState::Admitted)
        );
        assert_eq!(
            holding_causes(&client.drain_control()),
            vec![client_not_revealed()]
        );
        assert!(client.drain_snapshots().is_empty());
        assert!(!client.is_participating());
    }

    // a duplicate reveal is a no-op, and a reveal naming another level
    // never promotes.
    #[test]
    fn duplicate_or_foreign_reveal_does_not_repromote() {
        let (mut server, mut client) = participate_relay_pair();
        relay_server_to_client(&mut server, &mut client);
        assert!(client.drain_control().is_empty());
        let epoch = client.active_participation_epoch;
        assert!(epoch.is_some());

        for _ in 0..4 {
            client.client.send_message(
                Channel::Control,
                wire::encode(&ClientControlMessage::Revealed(Some(
                    "test-level".to_string(),
                ))),
            );
        }
        relay_client_to_server(&mut client, &mut server);
        let poll = server.poll_handshakes();
        assert!(poll.lifecycle.is_empty());
        assert!(poll.handshakes.is_empty());
        relay_server_to_client(&mut server, &mut client);
        assert!(client.drain_control().is_empty());
        assert_eq!(client.active_participation_epoch, epoch, "no new epoch");

        let (mut server, mut client) = settling_relay_pair();
        server.set_revealed_level(Some("test-level".to_string()));
        client.set_revealed_level(Some("other-level".to_string()));
        relay_client_to_server(&mut client, &mut server);
        let poll = server.poll_handshakes();
        assert_eq!(participating_entries(&poll.lifecycle), 0);
        assert!(matches!(
            poll.handshakes.as_slice(),
            [
                HandshakeOutcome::Admitted { .. },
                HandshakeOutcome::ParityHeld { cause, .. }
            ] if *cause == client_not_revealed()
        ));
    }

    // unload and suspend retract the reveal. A same-level reinstall
    // whose unload and install collapse into one host batch leaves parity
    // unchanged, so only the retained retraction holds it until its own reveal.
    #[test]
    fn retracted_reveal_holds_reinstall_until_new_reveal() {
        let (mut server, mut client) = participate_relay_pair();

        // Unload, flushed but not yet drained by the host.
        client.set_level_parity(None);
        client.set_revealed_level(None);
        relay_client_to_server(&mut client, &mut server);
        // Reinstall the same level, flushed into the same host batch.
        client.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        relay_client_to_server(&mut client, &mut server);

        let poll = server.poll_handshakes();
        assert_eq!(
            poll.lifecycle,
            vec![SlotEvent::Demoted {
                client_id: RELAY_CLIENT_ID,
                cause: client_not_revealed(),
            }]
        );
        assert!(!server.is_participating(RELAY_CLIENT_ID));

        client.set_revealed_level(Some("test-level".to_string()));
        relay_client_to_server(&mut client, &mut server);
        assert_eq!(
            server.poll_handshakes().lifecycle,
            vec![SlotEvent::Participating {
                client_id: RELAY_CLIENT_ID
            }]
        );
    }

    // while the host settles, a revealed, parity-matched
    // client is held and stays connected; the host's reveal promotes it once.
    // A host restart of the same level identity holds again until its new reveal.
    #[test]
    fn host_unrevealed_holds_revealed_client() {
        let (mut server, mut client) = settling_relay_pair();
        client.set_revealed_level(Some("test-level".to_string()));
        for _ in 0..(12 * 60) {
            relay_client_to_server(&mut client, &mut server);
            assert_eq!(
                participating_entries(&server.poll_handshakes().lifecycle),
                0
            );
            server.update_connections(Duration::from_millis(16));
            relay_server_to_client(&mut server, &mut client);
        }
        assert_eq!(server.connected_clients(), vec![RELAY_CLIENT_ID]);
        assert_eq!(
            holding_causes(&client.drain_control()),
            vec![host_not_revealed()]
        );

        server.set_revealed_level(Some("test-level".to_string()));
        let promoted = server.poll_handshakes();
        assert_eq!(participating_entries(&promoted.lifecycle), 1);
        relay_client_to_server(&mut client, &mut server);
        assert!(server.poll_handshakes().lifecycle.is_empty());

        // Same-identity restart: unload clears the host's reveal before the
        // reinstall publishes parity.
        server.set_revealed_level(None);
        server.set_level_parity(None);
        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        let restarted = server.poll_handshakes();
        assert_eq!(participating_entries(&restarted.lifecycle), 0);
        assert!(!server.is_participating(RELAY_CLIENT_ID));
        server.set_revealed_level(Some("test-level".to_string()));
        assert_eq!(
            server.poll_handshakes().lifecycle,
            vec![SlotEvent::Participating {
                client_id: RELAY_CLIENT_ID
            }]
        );
    }

    // suspend clears the host's own reveal through the same
    // retraction as unload; a resumed install of the same level holds every
    // revealed client until the host's new reveal.
    #[test]
    fn host_suspend_clears_own_reveal() {
        let (mut server, _client) = participate_relay_pair();

        // Suspend: parity and reveal both retracted, as `clear_net_level_parity` does.
        server.set_level_parity(None);
        server.set_revealed_level(None);
        assert!(matches!(
            server.poll_handshakes().lifecycle.as_slice(),
            [SlotEvent::Demoted { .. }]
        ));

        // Resume reinstalls the same identity with no unload in between.
        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        assert!(server.poll_handshakes().lifecycle.is_empty());
        assert_eq!(
            server.slot_state(RELAY_CLIENT_ID),
            Some(SlotState::Admitted)
        );

        server.set_revealed_level(Some("test-level".to_string()));
        assert_eq!(
            server.poll_handshakes().lifecycle,
            vec![SlotEvent::Participating {
                client_id: RELAY_CLIENT_ID
            }]
        );
    }

    // a host restart re-promotes a running client at the host's reveal
    // without a new declaration; a client still settling is promoted exactly
    // once, after both reveals, in either order.
    #[test]
    fn host_restart_repromotes_revealed_client_once_in_either_order() {
        // Running client: no client traffic after the restart.
        let (mut server, _client) = participate_relay_pair();
        server.set_revealed_level(None);
        server.set_level_parity(None);
        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        let mut lifecycle = server.poll_handshakes().lifecycle;
        server.set_revealed_level(Some("test-level".to_string()));
        lifecycle.extend(server.poll_handshakes().lifecycle);
        assert_eq!(participating_entries(&lifecycle), 1);
        assert!(server.is_participating(RELAY_CLIENT_ID));

        for host_reveals_first in [true, false] {
            let (mut server, mut client) = settling_relay_pair();
            relay_client_to_server(&mut client, &mut server);
            let mut lifecycle = server.poll_handshakes().lifecycle;
            if host_reveals_first {
                server.set_revealed_level(Some("test-level".to_string()));
                lifecycle.extend(server.poll_handshakes().lifecycle);
                assert_eq!(participating_entries(&lifecycle), 0);
                client.set_revealed_level(Some("test-level".to_string()));
                relay_client_to_server(&mut client, &mut server);
                lifecycle.extend(server.poll_handshakes().lifecycle);
            } else {
                client.set_revealed_level(Some("test-level".to_string()));
                relay_client_to_server(&mut client, &mut server);
                lifecycle.extend(server.poll_handshakes().lifecycle);
                assert_eq!(participating_entries(&lifecycle), 0);
                server.set_revealed_level(Some("test-level".to_string()));
                lifecycle.extend(server.poll_handshakes().lifecycle);
            }
            assert_eq!(
                participating_entries(&lifecycle),
                1,
                "host reveals first: {host_reveals_first}"
            );
            assert!(server.is_participating(RELAY_CLIENT_ID));
        }
    }

    // the revealed record survives a mod-digest demotion, so recovery
    // re-promotes without a new declaration.
    #[test]
    fn mod_digest_recovery_repromotes_without_new_reveal() {
        let (mut server, _client) = participate_relay_pair();
        server.set_mod_digest(Some([8; 32]));
        assert!(matches!(
            server.poll_handshakes().lifecycle.as_slice(),
            [SlotEvent::Demoted {
                cause: HoldingCause::ModDigest { .. },
                ..
            }]
        ));
        server.set_mod_digest(Some([7; 32]));
        assert_eq!(
            server.poll_handshakes().lifecycle,
            vec![SlotEvent::Participating {
                client_id: RELAY_CLIENT_ID
            }]
        );
    }

    // a held client that disconnects never spawns and leaves no
    // revealed record; its rejoin, on a fresh id, is held until it reveals.
    #[test]
    fn disconnect_clears_revealed_record() {
        let (mut server, mut client) = settling_relay_pair();
        client.set_revealed_level(Some("test-level".to_string()));
        relay_client_to_server(&mut client, &mut server);
        let _ = server.poll_handshakes();
        assert!(server.revealed_declarations.contains_key(&RELAY_CLIENT_ID));

        assert_eq!(
            server.close_relay_connection(RELAY_CLIENT_ID, CloseCause::Disconnect),
            None,
            "a held slot has no participating pawn to clean up"
        );
        assert!(!server.revealed_declarations.contains_key(&RELAY_CLIENT_ID));
        server.set_revealed_level(Some("test-level".to_string()));
        assert_eq!(
            participating_entries(&server.poll_handshakes().lifecycle),
            0
        );

        const REJOIN_ID: ClientId = RELAY_CLIENT_ID + 1;
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind rejoin socket");
        let server_addr = server.local_addr()[0];
        let mut rejoin = NetClient::new(
            socket,
            server_addr,
            REJOIN_ID,
            Duration::from_secs(1),
            None,
            None,
        )
        .expect("construct rejoin client");
        server.add_relay_connection(REJOIN_ID, None);
        rejoin.set_connected();
        rejoin.set_mod_identity("postretro.test".to_string(), "1".to_string());
        rejoin.set_mod_digest(Some([7; 32]));
        rejoin.set_level_parity(Some(("test-level".to_string(), [9; 32])));

        let flush = |rejoin: &mut NetClient, server: &mut NetServer| {
            rejoin.update_connections(Duration::from_millis(16));
            for packet in rejoin.packets_to_send() {
                server.process_packet_from(&packet, REJOIN_ID);
            }
        };
        flush(&mut rejoin, &mut server);
        let held = server.poll_handshakes();
        assert_eq!(participating_entries(&held.lifecycle), 0);
        assert_eq!(server.slot_state(REJOIN_ID), Some(SlotState::Admitted));

        rejoin.set_revealed_level(Some("test-level".to_string()));
        flush(&mut rejoin, &mut server);
        assert_eq!(
            server.poll_handshakes().lifecycle,
            vec![SlotEvent::Participating {
                client_id: REJOIN_ID
            }]
        );
    }

    // a demotion caused by the revealed term alone still sends the
    // Holding diagnostic, which retires the client's participation epoch.
    #[test]
    fn revealed_only_demotion_sends_holding_and_retires_epoch() {
        let (mut server, mut client) = participate_relay_pair();
        relay_server_to_client(&mut server, &mut client);
        assert!(client.drain_control().is_empty());
        assert!(client.is_participating());

        client.set_revealed_level(None);
        relay_client_to_server(&mut client, &mut server);
        let poll = server.poll_handshakes();
        assert_eq!(
            poll.lifecycle,
            vec![SlotEvent::Demoted {
                client_id: RELAY_CLIENT_ID,
                cause: client_not_revealed(),
            }]
        );
        assert_eq!(
            poll.handshakes,
            vec![HandshakeOutcome::ParityHeld {
                client_id: RELAY_CLIENT_ID,
                cause: client_not_revealed(),
            }]
        );
        relay_server_to_client(&mut server, &mut client);
        assert_eq!(
            holding_causes(&client.drain_control()),
            vec![client_not_revealed()]
        );
        assert!(!client.is_participating(), "the hold retires the epoch");

        // The host half demotes the same way.
        client.set_revealed_level(Some("test-level".to_string()));
        relay_client_to_server(&mut client, &mut server);
        assert_eq!(
            participating_entries(&server.poll_handshakes().lifecycle),
            1
        );
        relay_server_to_client(&mut server, &mut client);
        let _ = client.drain_control();
        assert!(client.is_participating());

        server.set_revealed_level(None);
        assert_eq!(
            server.poll_handshakes().lifecycle,
            vec![SlotEvent::Demoted {
                client_id: RELAY_CLIENT_ID,
                cause: host_not_revealed(),
            }]
        );
        relay_server_to_client(&mut server, &mut client);
        assert_eq!(
            holding_causes(&client.drain_control()),
            vec![host_not_revealed()]
        );
        assert!(!client.is_participating());
    }

    #[derive(Debug, Clone)]
    enum ParityOperation {
        InstallMod(Option<u8>),
        InstallLevel(Option<(u8, u8)>),
        Declare {
            mod_digest: u8,
            level: Option<(u8, u8)>,
        },
        /// The host records (or clears) its own reveal of `map-{identity}`.
        HostReveal(Option<u8>),
        /// The client declares (or retracts) its reveal of `map-{identity}`.
        ClientReveal(Option<u8>),
    }

    /// A small value domain, so generated declarations and reveals often match
    /// the installed values and both predicate directions are exercised.
    fn small() -> impl Strategy<Value = u8> {
        0_u8..3
    }

    fn parity_operation() -> impl Strategy<Value = ParityOperation> {
        prop_oneof![
            proptest::option::of(small()).prop_map(ParityOperation::InstallMod),
            proptest::option::of((small(), small())).prop_map(ParityOperation::InstallLevel),
            (small(), proptest::option::of((small(), small())))
                .prop_map(|(mod_digest, level)| ParityOperation::Declare { mod_digest, level }),
            proptest::option::of(small()).prop_map(ParityOperation::HostReveal),
            proptest::option::of(small()).prop_map(ParityOperation::ClientReveal),
        ]
    }

    fn level_identity(identity: u8) -> String {
        format!("map-{identity}")
    }

    fn level_pair((identity, digest): (u8, u8)) -> (String, [u8; 32]) {
        (level_identity(identity), [digest; 32])
    }

    #[derive(Default)]
    struct ParityModel {
        installed_mod: Option<u8>,
        installed_level: Option<(u8, u8)>,
        declaration: Option<(u8, Option<(u8, u8)>)>,
        host_revealed: Option<u8>,
        client_revealed: Option<u8>,
    }

    fn apply_parity_operation(
        server: &mut NetServer,
        model: &mut ParityModel,
        operation: ParityOperation,
    ) -> (bool, bool) {
        match operation {
            ParityOperation::InstallMod(value) => {
                model.installed_mod = value;
                server.set_mod_digest(value.map(|byte| [byte; 32]));
            }
            ParityOperation::InstallLevel(value) => {
                model.installed_level = value;
                server.set_level_parity(value.map(level_pair));
            }
            ParityOperation::Declare { mod_digest, level } => {
                model.declaration = Some((mod_digest, level));
                server.parity_declarations.insert(
                    RELAY_CLIENT_ID,
                    ParityDeclaration {
                        mod_digest: [mod_digest; 32],
                        level: level.map(level_pair),
                    },
                );
                let _ = server.reevaluate_parity(Some(RELAY_CLIENT_ID));
            }
            ParityOperation::HostReveal(value) => {
                model.host_revealed = value;
                server.set_revealed_level(value.map(level_identity));
            }
            ParityOperation::ClientReveal(value) => {
                model.client_revealed = value;
                match value {
                    Some(identity) => {
                        server
                            .revealed_declarations
                            .insert(RELAY_CLIENT_ID, level_identity(identity));
                    }
                    None => {
                        server.revealed_declarations.remove(&RELAY_CLIENT_ID);
                    }
                }
                let _ = server.reevaluate_parity(Some(RELAY_CLIENT_ID));
            }
        }
        let expected = matches!(
            (
                model.installed_mod,
                model.installed_level,
                model.declaration
            ),
            (
                Some(installed_mod),
                Some((installed_identity, installed_digest)),
                Some((declared_mod, Some((declared_identity, declared_digest))))
            ) if installed_mod == declared_mod
                && installed_identity == declared_identity
                && installed_digest == declared_digest
                && model.host_revealed == Some(installed_identity)
                && model.client_revealed == Some(installed_identity)
        );
        (server.is_participating(RELAY_CLIENT_ID), expected)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn participation_predicate_holds_after_generated_server_operations(
            operations in proptest::collection::vec(parity_operation(), 0..32),
        ) {
            let (mut server, _client) = relay_pair();
            let _ = server.slots.admit(RELAY_CLIENT_ID);
            let mut model = ParityModel::default();

            for operation in operations {
                let (actual, expected) =
                    apply_parity_operation(&mut server, &mut model, operation);
                prop_assert_eq!(actual, expected);
            }

            // Every generated sequence finishes with explicit demotions and
            // recoveries using the retained declarations, so both directions of
            // every term are exercised rather than left to random selection. The
            // mod-digest recovery re-promotes on the retained reveals alone.
            for operation in [
                ParityOperation::Declare {
                    mod_digest: 17,
                    level: Some((4, 23)),
                },
                ParityOperation::InstallLevel(Some((4, 23))),
                ParityOperation::InstallMod(Some(17)),
                ParityOperation::HostReveal(Some(4)),
                ParityOperation::ClientReveal(Some(4)),
                ParityOperation::InstallMod(Some(18)),
                ParityOperation::InstallMod(Some(17)),
                ParityOperation::ClientReveal(None),
                ParityOperation::ClientReveal(Some(4)),
                ParityOperation::HostReveal(None),
                ParityOperation::HostReveal(Some(4)),
            ] {
                let (actual, expected) =
                    apply_parity_operation(&mut server, &mut model, operation);
                prop_assert_eq!(actual, expected);
            }
            prop_assert!(server.is_participating(RELAY_CLIENT_ID));
        }
    }
}
