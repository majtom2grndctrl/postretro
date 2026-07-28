// Polled, registry-blind renet transport and E15 two-stage control gate.

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use renet::{
    ChannelConfig, ClientId, ConnectionConfig, DisconnectReason, RenetClient, RenetServer,
    SendType, ServerEvent,
};
use renet_netcode::{
    ClientAuthentication, NetcodeClientTransport, NetcodeServerTransport, NetcodeTransportError,
    ServerAuthentication, ServerConfig,
};

use crate::slots::{CloseCause, SlotEvent, SlotState, SlotTable};
use crate::wire::{self, ClientControlMessage, ParityDeclaration, ServerControlMessage};

pub use crate::handshake::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Control = 0,
    Snapshot = 1,
    Input = 2,
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
    ParityHeld {
        client_id: ClientId,
        cause: HoldingCause,
    },
}

#[derive(Debug, Default)]
#[must_use = "a poll carries gate verdicts and slot lifecycle transitions"]
pub struct ServerPoll {
    pub handshakes: Vec<HandshakeOutcome>,
    pub lifecycle: Vec<SlotEvent>,
}

/// Synchronous server transport. It knows only opaque declarations and slot ids.
pub struct NetServer {
    server: RenetServer,
    transport: NetcodeServerTransport,
    slots: SlotTable,
    parity_declarations: HashMap<ClientId, ParityDeclaration>,
    pending_lifecycle: Vec<SlotEvent>,
    pending_disconnects: Vec<ClientId>,
    mod_identity: Option<(String, String)>,
    mod_digest: Option<[u8; 32]>,
    level_parity: Option<(String, [u8; 32])>,
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
            pending_lifecycle: Vec::new(),
            pending_disconnects: Vec::new(),
            mod_identity: None,
            mod_digest: None,
            level_parity: None,
            legacy_kinematic_static_fingerprint: kinematic_static_fingerprint,
        })
    }

    pub fn set_mod_identity(&mut self, id: String, version: String) {
        self.mod_identity = Some((id, version));
    }

    pub fn set_mod_digest(&mut self, digest: Option<[u8; 32]>) {
        self.mod_digest = digest;
        // Parity deliberately queues until its first required source exists.
        if self.mod_digest.is_some() {
            let _ = self.reevaluate_parity(None);
        }
    }

    pub fn set_level_parity(&mut self, level: Option<(String, [u8; 32])>) {
        self.level_parity = level;
        let _ = self.reevaluate_parity(None);
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
        self.apply_pending_disconnects();
        self.server.update(dt);
        self.transport.update(dt, &mut self.server)?;
        self.collect_server_events();
        let handshakes = self.process_control_messages();
        let lifecycle = std::mem::take(&mut self.pending_lifecycle);
        self.transport.send_packets(&mut self.server);
        Ok(ServerPoll {
            handshakes,
            lifecycle,
        })
    }

    fn apply_pending_disconnects(&mut self) {
        for client_id in std::mem::take(&mut self.pending_disconnects) {
            self.server.disconnect(client_id);
        }
    }

    fn collect_server_events(&mut self) {
        while let Some(event) = self.server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => self.slots.on_connect(client_id),
                ServerEvent::ClientDisconnected { client_id, reason } => {
                    if let Some(event) = self.close_slot(client_id, close_cause_from(reason)) {
                        self.pending_lifecycle.push(event);
                    }
                }
            }
        }
    }

    fn process_control_messages(&mut self) -> Vec<HandshakeOutcome> {
        let mut outcomes = Vec::new();
        let Some((expected_id, expected_version)) = self.mod_identity.clone() else {
            return outcomes;
        };
        let expected_protocol = protocol_version();

        for client_id in self.server.clients_id() {
            if self.slots.is_closed(client_id) {
                while self
                    .server
                    .receive_message(client_id, Channel::Control)
                    .is_some()
                {}
                continue;
            }
            // Once admitted, the next queued Control message can only be parity;
            // leave it reliably queued until the required installed digest exists.
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
                        if self.mod_digest.is_some() {
                            if let Some(cause) = self.reevaluate_parity(Some(client_id)) {
                                self.send_divergence(client_id, cause.clone());
                                outcomes.push(HandshakeOutcome::ParityHeld { client_id, cause });
                            }
                        } else {
                            break;
                        }
                    }
                    ClientControlMessage::Parity(declaration) => {
                        self.parity_declarations.insert(client_id, declaration);
                        if self.mod_digest.is_none() {
                            break;
                        }
                        let was_participating = self.is_participating(client_id);
                        if let Some(cause) = self.reevaluate_parity(Some(client_id)) {
                            if !was_participating {
                                self.send_divergence(client_id, cause.clone());
                            }
                            outcomes.push(HandshakeOutcome::ParityHeld { client_id, cause });
                        }
                    }
                }
                if self.mod_digest.is_none() {
                    break;
                }
            }
        }
        outcomes
    }

    fn reject(&mut self, client_id: ClientId, cause: ClosingCause) {
        self.send_control(
            client_id,
            wire::encode(&ServerControlMessage::Divergence(
                DivergenceReason::Closing(cause),
            )),
        );
        let _ = self.close_slot(client_id, CloseCause::Timeout);
        if !self.pending_disconnects.contains(&client_id) {
            self.pending_disconnects.push(client_id);
        }
    }

    fn send_divergence(&mut self, client_id: ClientId, cause: HoldingCause) {
        self.send_control(
            client_id,
            wire::encode(&ServerControlMessage::Divergence(
                DivergenceReason::Holding(cause),
            )),
        );
    }

    /// Enforce the single participation predicate after any source install or
    /// parity arrival. Install-driven transitions land in `pending_lifecycle`.
    fn reevaluate_parity(&mut self, only: Option<ClientId>) -> Option<HoldingCause> {
        let ids = only.map_or_else(|| self.server.clients_id(), |id| vec![id]);
        let mut selected_cause = None;
        for client_id in ids {
            let state = self.slots.state(client_id);
            if !matches!(state, Some(SlotState::Admitted | SlotState::Participating)) {
                continue;
            }
            let cause = parity_cause(
                self.mod_digest,
                self.level_parity.as_ref(),
                self.parity_declarations.get(&client_id),
            );
            match cause {
                None => {
                    if let Some(event) = self.slots.participate(client_id) {
                        self.pending_lifecycle.push(event);
                    }
                }
                Some(cause) => {
                    if matches!(state, Some(SlotState::Participating)) {
                        if let Some(event) = self.slots.demote(client_id, cause.clone()) {
                            self.pending_lifecycle.push(event);
                            self.send_divergence(client_id, cause.clone());
                        }
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
        self.parity_declarations.remove(&client_id);
        self.slots.close(client_id, cause)
    }

    #[must_use]
    pub fn is_participating(&self, client_id: ClientId) -> bool {
        self.slots.is_participating(client_id)
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

    /// Snapshots and Input are both participation-gated; held peers are drained
    /// so their reliable channel cannot overflow and disconnect them indirectly.
    pub fn send_snapshot(&mut self, client_id: ClientId, snapshot: Vec<u8>) -> bool {
        if !self.is_participating(client_id) {
            return false;
        }
        self.server
            .send_message(client_id, Channel::Snapshot, snapshot);
        true
    }

    pub fn drain_input(&mut self, client_id: ClientId) -> Vec<Vec<u8>> {
        let participating = self.is_participating(client_id);
        let mut messages = Vec::new();
        while let Some(bytes) = self.server.receive_message(client_id, Channel::Input) {
            if !participating {
                continue;
            }
            // Exhaustively classify recognized input envelopes. Net still leaves
            // their interpretation to the engine and forwards malformed bytes.
            match wire::decode::<crate::wire::ClientMessage>(&bytes) {
                Ok(crate::wire::ClientMessage::Input(_))
                | Ok(crate::wire::ClientMessage::Ack(_))
                | Ok(crate::wire::ClientMessage::BaselineRefresh(_))
                | Ok(crate::wire::ClientMessage::TimeSync(_))
                | Ok(crate::wire::ClientMessage::StateBaselineRefresh(_))
                | Ok(crate::wire::ClientMessage::HitDeclaration(_))
                | Err(_) => messages.push(bytes.to_vec()),
            }
        }
        messages
    }

    /// Control may be sent to an admitted/closed slot to deliver a hold/reject
    /// diagnostic before socket teardown. Payload semantics remain engine-owned.
    pub fn send_control(&mut self, client_id: ClientId, payload: Vec<u8>) {
        self.server
            .send_message(client_id, Channel::Control, payload);
    }

    pub fn send_input(&mut self, client_id: ClientId, payload: Vec<u8>) {
        self.server.send_message(client_id, Channel::Input, payload);
    }

    pub fn packets_to_send(&mut self, client_id: ClientId) -> Vec<Vec<u8>> {
        self.server
            .get_packets_to_send(client_id)
            .unwrap_or_default()
    }

    pub fn process_packet_from(&mut self, packet: &[u8], client_id: ClientId) {
        let _ = self.server.process_packet_from(packet, client_id);
    }

    pub fn add_relay_connection(&mut self, client_id: ClientId) {
        self.server.add_connection(client_id);
        self.slots.on_connect(client_id);
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
        self.apply_pending_disconnects();
        self.collect_server_events();
        let handshakes = self.process_control_messages();
        let lifecycle = std::mem::take(&mut self.pending_lifecycle);
        ServerPoll {
            handshakes,
            lifecycle,
        }
    }
}

/// `None` means the declaration and installed triple match. The order here is
/// the published holding-diagnostic precedence.
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

/// Synchronous client transport. It declares values but never compares them.
pub struct NetClient {
    client: RenetClient,
    transport: NetcodeClientTransport,
    admission_sent: bool,
    parity_sent: bool,
    mod_identity: Option<(String, String)>,
    mod_digest: Option<[u8; 32]>,
    level_parity: Option<(String, [u8; 32])>,
    legacy_kinematic_static_fingerprint: Option<[u8; 32]>,
}

impl NetClient {
    pub fn new(
        socket: UdpSocket,
        server_addr: SocketAddr,
        client_id: u64,
        current_time: Duration,
        kinematic_static_fingerprint: Option<[u8; 32]>,
    ) -> Result<Self, NetcodeTransportError> {
        let client = RenetClient::new(connection_config());
        let transport = NetcodeClientTransport::new(
            current_time,
            ClientAuthentication::Unsecure {
                client_id,
                protocol_id: transport_protocol_id(),
                server_addr,
                user_data: None,
            },
            socket,
        )?;
        Ok(Self {
            client,
            transport,
            admission_sent: false,
            parity_sent: false,
            mod_identity: None,
            mod_digest: None,
            level_parity: None,
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
        }
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
        if !self.admission_sent {
            if let Some((mod_id, mod_version)) = self.mod_identity.clone() {
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
        }
        if !self.parity_sent {
            if let Some(mod_digest) = self.mod_digest {
                self.client.send_message(
                    Channel::Control,
                    wire::encode(&ClientControlMessage::Parity(ParityDeclaration {
                        mod_digest,
                        level: self.level_parity.clone(),
                    })),
                );
                self.parity_sent = true;
            }
        }
    }

    pub fn update(&mut self, dt: Duration) -> Result<(), NetcodeTransportError> {
        self.client.update(dt);
        self.transport.update(dt, &mut self.client)?;
        self.queue_control_messages();
        self.transport.send_packets(&mut self.client)?;
        Ok(())
    }

    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.client.is_connected()
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
        self.client.send_message(Channel::Input, input);
    }

    pub fn drain_input(&mut self) -> Vec<Vec<u8>> {
        drain_client_channel(&mut self.client, Channel::Input)
    }

    pub fn drain_snapshots(&mut self) -> Vec<Vec<u8>> {
        drain_client_channel(&mut self.client, Channel::Snapshot)
    }

    pub fn drain_control(&mut self) -> Vec<Vec<u8>> {
        drain_client_channel(&mut self.client, Channel::Control)
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
    use proptest::prelude::*;

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

    proptest! {
        #[test]
        fn participation_predicate_matches_complete_installed_triple(
            mod_byte in any::<u8>(),
            level_byte in any::<u8>(),
            declared_mod_byte in any::<u8>(),
            declared_level_byte in any::<u8>(),
            installed_level in any::<bool>(),
            declared_level in any::<bool>(),
        ) {
            let installed = installed_level.then(|| ("map".to_string(), [level_byte; 32]));
            let declaration = ParityDeclaration {
                mod_digest: [declared_mod_byte; 32],
                level: declared_level.then(|| ("map".to_string(), [declared_level_byte; 32])),
            };
            let participates = parity_cause(Some([mod_byte; 32]), installed.as_ref(), Some(&declaration)).is_none();
            prop_assert_eq!(participates, installed_level && declared_level && mod_byte == declared_mod_byte && level_byte == declared_level_byte);
        }
    }
}
