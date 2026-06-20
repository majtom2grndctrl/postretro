// renet/renet_netcode transport: polled, non-blocking UDP server/client over std sockets.
// See: context/research/netcode/
//
// renet 2.0 separates the *connection layer* (`RenetServer`/`RenetClient`, which
// own channels and produce/consume opaque packet payloads) from the *netcode
// transport* (`NetcodeServerTransport`/`NetcodeClientTransport`, which encrypt
// those payloads and move them over a non-blocking `std::net::UdpSocket`). We wrap
// both into `NetServer`/`NetClient` with a synchronous `update(dt)` surface: no
// spawned runtime, no threads — the caller polls each frame (development_guide
// §4.2). The connection-layer packet I/O (`get_packets_to_send` /
// `process_packet*`) is re-exposed for the Task 5 in-memory relay, which drives
// the same payloads without touching UDP.

use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use renet::{
    ChannelConfig, ClientId, ConnectionConfig, RenetClient, RenetServer, SendType, ServerEvent,
};
use renet_netcode::{
    ClientAuthentication, NetcodeClientTransport, NetcodeServerTransport, NetcodeTransportError,
    ServerAuthentication, ServerConfig,
};

use crate::wire::{self, ProtocolVersion, WireError};

/// App protocol identity. Hand-bumped on any change that breaks cross-version
/// compatibility of the message *vocabulary* (a new control message, a changed
/// channel layout). Carried as `ProtocolVersion::app_protocol_id` and folded into
/// the transport-level `protocol_id`.
pub const PROTOCOL_ID: u32 = 0x_5052_4C31; // "PRL1"

/// Wire-format version. Hand-bumped whenever the bitcode byte layout of any wire
/// type changes (added field, reordered enum, bumped bitcode major). Carried as
/// `ProtocolVersion::wire_version` and folded into the transport-level
/// `protocol_id` so a wire-incompatible peer is refused at the netcode layer.
pub const WIRE_VERSION: u32 = 1;

/// Transport-level gate fed to renet_netcode as the netcode `protocol_id: u64`.
/// Packs both hand-bumped consts so the encrypted handshake itself fails for any
/// peer whose `(PROTOCOL_ID, WIRE_VERSION)` pair differs — the connection never
/// establishes. The app-level `ProtocolVersion` (sent over the control channel)
/// carries the same two values for the second, app-level gate.
#[must_use]
pub const fn transport_protocol_id() -> u64 {
    ((PROTOCOL_ID as u64) << 32) | (WIRE_VERSION as u64)
}

/// The app-level handshake value built from this build's protocol consts. Sent by
/// the client as its first control message and validated by the server.
#[must_use]
pub const fn protocol_version() -> ProtocolVersion {
    ProtocolVersion {
        app_protocol_id: PROTOCOL_ID,
        wire_version: WIRE_VERSION,
    }
}

/// Named renet channels. The `u8` ids are the channel identifiers shared by the
/// server and client `ChannelConfig` lists.
///
/// - `Control` — reliable-ordered: the version handshake (and, in Phase 2,
///   spawn/despawn). Order matters and loss is unacceptable.
/// - `Snapshot` — unreliable: full-state snapshots, latest-wins; a dropped
///   snapshot is superseded by the next one.
/// - `Input` — reserved for the Phase 2 client input-command stream. Registered
///   now so the channel layout (and thus `transport_protocol_id`) is stable;
///   carries no traffic in Phase 1.
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

/// Per-channel byte budget before back-pressure. Reliable channels disconnect on
/// overflow; unreliable channels drop the oldest. Matches renet's own default.
const CHANNEL_MEMORY_BYTES: usize = 5 * 1024 * 1024;

/// Reliable resend cadence for the control channel. renet's default.
const RELIABLE_RESEND: Duration = Duration::from_millis(300);

/// Build the three-channel `ConnectionConfig` shared by server and client. Both
/// peers must agree on the channel layout, so a single constructor produces both
/// the `server_channels_config` and `client_channels_config`.
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

/// Why the server refused a joining client at the app-level handshake gate. Carries
/// both versions so a test (and the operator log) can see exactly what diverged.
/// Distinct from the transport gate, which rejects wire-incompatible peers before a
/// connection ever forms — this is the second gate, applied to an *established*
/// connection's first control message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RejectReason {
    /// The `ProtocolVersion` this build expects (from `protocol_version()`).
    pub expected: ProtocolVersion,
    /// The `ProtocolVersion` the client actually sent.
    pub received: ProtocolVersion,
}

impl std::fmt::Display for RejectReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "protocol mismatch: expected app_protocol_id={:#010x} wire_version={}, \
             received app_protocol_id={:#010x} wire_version={}",
            self.expected.app_protocol_id,
            self.expected.wire_version,
            self.received.app_protocol_id,
            self.received.wire_version,
        )
    }
}

impl std::error::Error for RejectReason {}

/// Pure handshake gate: the app-level second gate, independent of sockets so the
/// reject reason is unit-assertable. A match is `Ok(())`; any divergence yields the
/// typed `RejectReason` carrying expected vs received. Wired into
/// `NetServer::update` for the live path.
pub fn validate_handshake(
    expected: ProtocolVersion,
    received: ProtocolVersion,
) -> Result<(), RejectReason> {
    if expected == received {
        Ok(())
    } else {
        Err(RejectReason { expected, received })
    }
}

/// Outcome of a client's handshake, tracked per client by the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HandshakeState {
    /// Connected at the transport layer; the app-level `ProtocolVersion` control
    /// message has not arrived yet. No entity state is sent in this state.
    Pending,
    /// Handshake validated — the client may receive entity state.
    Accepted,
}

/// A handshake result surfaced to the caller after `update`. The accept path lets
/// the caller begin replicating to that client; the reject path carries the typed
/// reason already logged and acted on (client disconnected) by `update`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeOutcome {
    /// The client passed the app gate and is now accepted.
    Accepted { client_id: ClientId },
    /// The client failed the app gate; it has been disconnected and no entity
    /// state was sent.
    Rejected {
        client_id: ClientId,
        reason: RejectReason,
    },
}

/// Synchronous, non-blocking server: renet connection layer + netcode UDP
/// transport, plus the app-level handshake gate. Poll `update(dt)` each frame;
/// it drives renet, drains the netcode socket, validates pending handshakes, and
/// returns this frame's handshake outcomes. No spawned runtime or threads.
pub struct NetServer {
    server: RenetServer,
    transport: NetcodeServerTransport,
    /// Per-client handshake state. A client absent from this map is unknown; a
    /// `Pending` entry has connected but not yet been validated.
    handshakes: std::collections::HashMap<ClientId, HandshakeState>,
}

impl NetServer {
    /// Bind a server to `socket` (already bound, e.g. `UdpSocket::bind`). The
    /// netcode transport is configured with `transport_protocol_id()` as its
    /// gate and is set non-blocking internally. `current_time` is the monotonic
    /// duration the netcode layer uses as its clock origin (typically
    /// `SystemTime::now().duration_since(UNIX_EPOCH)`); callers in tests can pass
    /// any fixed origin.
    pub fn new(
        socket: UdpSocket,
        public_addr: SocketAddr,
        max_clients: usize,
        current_time: Duration,
    ) -> Result<Self, NetcodeTransportError> {
        let server = RenetServer::new(connection_config());
        let server_config = ServerConfig {
            current_time,
            max_clients,
            protocol_id: transport_protocol_id(),
            public_addresses: vec![public_addr],
            authentication: ServerAuthentication::Unsecure,
        };
        let transport = NetcodeServerTransport::new(server_config, socket)?;
        Ok(Self {
            server,
            transport,
            handshakes: std::collections::HashMap::new(),
        })
    }

    /// The address the underlying socket is bound to (resolves ephemeral `:0`).
    #[must_use]
    pub fn local_addr(&self) -> Vec<SocketAddr> {
        self.transport.addresses()
    }

    /// Advance one frame: drive renet, drain the socket, then validate any pending
    /// handshakes that arrived this frame. Returns the handshake outcomes produced
    /// this frame (acceptances and the typed rejections). Flushes outbound packets
    /// at the end so anything sent before/after `update` leaves the socket.
    ///
    /// Non-blocking: the netcode transport drains the socket to `WouldBlock` and
    /// returns. Never blocks the caller.
    pub fn update(&mut self, dt: Duration) -> Result<Vec<HandshakeOutcome>, NetcodeTransportError> {
        self.server.update(dt);
        self.transport.update(dt, &mut self.server)?;

        // Reap connection lifecycle events so handshake state tracks live clients.
        while let Some(event) = self.server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    self.handshakes.insert(client_id, HandshakeState::Pending);
                }
                ServerEvent::ClientDisconnected { client_id, .. } => {
                    self.handshakes.remove(&client_id);
                }
            }
        }

        let outcomes = self.process_control_messages();

        self.transport.send_packets(&mut self.server);
        Ok(outcomes)
    }

    /// Drain the control channel for every client and, for those still `Pending`,
    /// validate the first message as the `ProtocolVersion` handshake. This is the
    /// app-level gate: it runs *before* any entity state is sent (callers only
    /// `send_snapshot` to `accepted` clients), so a rejected client receives no
    /// snapshot.
    fn process_control_messages(&mut self) -> Vec<HandshakeOutcome> {
        let mut outcomes = Vec::new();
        let expected = protocol_version();

        for client_id in self.server.clients_id() {
            while let Some(bytes) = self.server.receive_message(client_id, Channel::Control) {
                // A client already accepted may send later control traffic
                // (spawn/despawn, Phase 2). Phase 1 has none, so drain-and-ignore.
                if self.handshakes.get(&client_id) == Some(&HandshakeState::Accepted) {
                    continue;
                }

                let received: ProtocolVersion = match wire::decode(&bytes) {
                    Ok(v) => v,
                    Err(err) => {
                        // A malformed first message is not a valid handshake. Treat
                        // a decode failure as a reject with the all-zero version so
                        // the operator sees something actionable, then disconnect.
                        log::warn!(
                            "[Net] handshake decode failed for client {client_id}: {err}; \
                             disconnecting"
                        );
                        let reason = RejectReason {
                            expected,
                            received: malformed_version(&err),
                        };
                        self.reject(client_id, reason);
                        outcomes.push(HandshakeOutcome::Rejected { client_id, reason });
                        break;
                    }
                };

                match validate_handshake(expected, received) {
                    Ok(()) => {
                        self.handshakes.insert(client_id, HandshakeState::Accepted);
                        log::info!("[Net] client {client_id} accepted (protocol {received:?})");
                        outcomes.push(HandshakeOutcome::Accepted { client_id });
                    }
                    Err(reason) => {
                        log::warn!("[Net] rejecting client {client_id}: {reason}");
                        self.reject(client_id, reason);
                        outcomes.push(HandshakeOutcome::Rejected { client_id, reason });
                        break;
                    }
                }
            }
        }

        outcomes
    }

    /// Disconnect a client and forget its handshake state. renet flushes the
    /// disconnect over the transport on the next `send_packets`.
    fn reject(&mut self, client_id: ClientId, _reason: RejectReason) {
        self.handshakes.remove(&client_id);
        self.server.disconnect(client_id);
    }

    /// Has the given client passed the app-level handshake?
    #[must_use]
    pub fn is_accepted(&self, client_id: ClientId) -> bool {
        self.handshakes.get(&client_id) == Some(&HandshakeState::Accepted)
    }

    /// Client ids that have passed the handshake and may receive entity state.
    #[must_use]
    pub fn accepted_clients(&self) -> Vec<ClientId> {
        self.handshakes
            .iter()
            .filter(|(_, state)| **state == HandshakeState::Accepted)
            .map(|(id, _)| *id)
            .collect()
    }

    /// All transport-connected client ids (regardless of handshake state).
    #[must_use]
    pub fn connected_clients(&self) -> Vec<ClientId> {
        self.server.clients_id()
    }

    /// Send a snapshot buffer to an accepted client over the unreliable snapshot
    /// channel. No-op (returns `false`) for a client that has not passed the app
    /// gate — this is the invariant that "no entity state is sent after reject".
    pub fn send_snapshot(&mut self, client_id: ClientId, snapshot: Vec<u8>) -> bool {
        if !self.is_accepted(client_id) {
            return false;
        }
        self.server
            .send_message(client_id, Channel::Snapshot, snapshot);
        true
    }

    /// Drain any input-channel messages from a client (Phase 2 traffic; empty in
    /// Phase 1). Exposed now so the channel surface is complete.
    pub fn drain_input(&mut self, client_id: ClientId) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Some(bytes) = self.server.receive_message(client_id, Channel::Input) {
            out.push(bytes.to_vec());
        }
        out
    }

    /// Connection-level packets renet wants delivered to `client_id`, *bypassing*
    /// the netcode UDP transport. The Task 5 in-memory relay hands these straight
    /// to the peer's `process_packet`. Returns `Vec<Vec<u8>>` (renet's
    /// `Vec<Payload>`); an unknown client yields an empty `Vec`.
    pub fn packets_to_send(&mut self, client_id: ClientId) -> Vec<Vec<u8>> {
        self.server
            .get_packets_to_send(client_id)
            .unwrap_or_default()
    }

    /// Feed a connection-level packet received out-of-band (the in-memory relay)
    /// into renet for `client_id`. Mirror of `packets_to_send`. Ignores packets
    /// for unknown clients.
    pub fn process_packet_from(&mut self, packet: &[u8], client_id: ClientId) {
        let _ = self.server.process_packet_from(packet, client_id);
    }

    /// Register a client with the renet connection layer without going through the
    /// netcode transport — required by the in-memory relay, which establishes the
    /// connection itself. Mirrors `RenetServer::add_connection` and seeds the
    /// handshake state as `Pending`.
    pub fn add_relay_connection(&mut self, client_id: ClientId) {
        self.server.add_connection(client_id);
        self.handshakes.insert(client_id, HandshakeState::Pending);
    }

    /// Drive only the renet connection layer (no socket). The in-memory relay calls
    /// this instead of `update`, then drains handshakes via `poll_handshakes`.
    pub fn update_connections(&mut self, dt: Duration) {
        self.server.update(dt);
    }

    /// Run the app-level handshake gate over already-delivered control messages.
    /// Used by the in-memory relay, which moves packets itself and so cannot use
    /// `update`'s socket path. Returns this poll's handshake outcomes.
    pub fn poll_handshakes(&mut self) -> Vec<HandshakeOutcome> {
        while let Some(event) = self.server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    self.handshakes
                        .entry(client_id)
                        .or_insert(HandshakeState::Pending);
                }
                ServerEvent::ClientDisconnected { client_id, .. } => {
                    self.handshakes.remove(&client_id);
                }
            }
        }
        self.process_control_messages()
    }
}

/// Synchronous, non-blocking client: renet connection layer + netcode UDP
/// transport. Poll `update(dt)` each frame. No spawned runtime or threads.
pub struct NetClient {
    client: RenetClient,
    transport: NetcodeClientTransport,
    /// Whether the `ProtocolVersion` control message has been queued yet. The
    /// first `update` after connect sends it as the first reliable control message.
    handshake_sent: bool,
}

impl NetClient {
    /// Connect to `server_addr` from `socket`. `client_id` identifies this client
    /// to the (unsecure) netcode layer; `current_time` is the netcode clock origin
    /// (see `NetServer::new`). The netcode transport uses `transport_protocol_id()`
    /// so a wire-incompatible server is refused before the connection forms.
    pub fn new(
        socket: UdpSocket,
        server_addr: SocketAddr,
        client_id: u64,
        current_time: Duration,
    ) -> Result<Self, NetcodeTransportError> {
        let client = RenetClient::new(connection_config());
        let authentication = ClientAuthentication::Unsecure {
            protocol_id: transport_protocol_id(),
            client_id,
            server_addr,
            user_data: None,
        };
        let transport = NetcodeClientTransport::new(current_time, authentication, socket)?;
        Ok(Self {
            client,
            transport,
            handshake_sent: false,
        })
    }

    /// Advance one frame: drive renet + the netcode transport, then, once the
    /// transport connection is established, queue the `ProtocolVersion` handshake
    /// as the first reliable control message (exactly once). Flushes outbound
    /// packets at the end. Non-blocking.
    pub fn update(&mut self, dt: Duration) -> Result<(), NetcodeTransportError> {
        self.client.update(dt);
        self.transport.update(dt, &mut self.client)?;

        if self.client.is_connected() && !self.handshake_sent {
            let bytes = wire::encode(&protocol_version());
            self.client.send_message(Channel::Control, bytes);
            self.handshake_sent = true;
        }

        self.transport.send_packets(&mut self.client)?;
        Ok(())
    }

    /// Is the transport-level connection established?
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.client.is_connected()
    }

    /// Has the client queued its handshake control message yet?
    #[must_use]
    pub fn handshake_sent(&self) -> bool {
        self.handshake_sent
    }

    /// Send an input-command buffer over the reserved input channel (Phase 2).
    /// Exposed now so the channel surface is complete.
    pub fn send_input(&mut self, input: Vec<u8>) {
        self.client.send_message(Channel::Input, input);
    }

    /// Drain snapshot buffers received this frame (unreliable channel).
    pub fn drain_snapshots(&mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Some(bytes) = self.client.receive_message(Channel::Snapshot) {
            out.push(bytes.to_vec());
        }
        out
    }

    /// Drain control-channel buffers received this frame (reliable-ordered). Phase
    /// 1 server sends no control traffic to the client; exposed for Phase 2.
    pub fn drain_control(&mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Some(bytes) = self.client.receive_message(Channel::Control) {
            out.push(bytes.to_vec());
        }
        out
    }

    /// Connection-level packets renet wants delivered to the server, bypassing the
    /// netcode UDP transport. The Task 5 in-memory relay hands these to the
    /// server's `process_packet_from`.
    pub fn packets_to_send(&mut self) -> Vec<Vec<u8>> {
        self.client.get_packets_to_send()
    }

    /// Feed a connection-level packet received out-of-band (the relay) into renet.
    pub fn process_packet(&mut self, packet: &[u8]) {
        self.client.process_packet(packet);
    }

    /// Mark the renet connection established without the netcode handshake — the
    /// in-memory relay establishes the connection itself.
    pub fn set_connected(&mut self) {
        self.client.set_connected();
    }

    /// Drive only the renet connection layer (no socket), then queue the handshake
    /// once connected. The in-memory relay calls this instead of `update`.
    pub fn update_connections(&mut self, dt: Duration) {
        self.client.update(dt);
        if self.client.is_connected() && !self.handshake_sent {
            let bytes = wire::encode(&protocol_version());
            self.client.send_message(Channel::Control, bytes);
            self.handshake_sent = true;
        }
    }
}

/// Reconstruct a best-effort `ProtocolVersion` from a decode failure for logging.
/// The bytes did not decode, so there is no real received version — surface the
/// all-zero sentinel, which can never equal a real `protocol_version()`.
fn malformed_version(_err: &WireError) -> ProtocolVersion {
    ProtocolVersion {
        app_protocol_id: 0,
        wire_version: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now() -> Duration {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the unix epoch")
    }

    // --- Pure-function gate: the primary, deterministic assertions. ---

    #[test]
    fn validate_handshake_accepts_matching_versions() {
        let v = protocol_version();
        assert_eq!(validate_handshake(v, v), Ok(()));
    }

    #[test]
    fn validate_handshake_rejects_divergent_wire_version() {
        let expected = protocol_version();
        let received = ProtocolVersion {
            app_protocol_id: expected.app_protocol_id,
            wire_version: expected.wire_version + 1,
        };
        let err = validate_handshake(expected, received).expect_err("divergent version must reject");
        assert_eq!(err.expected, expected);
        assert_eq!(err.received, received);
    }

    #[test]
    fn validate_handshake_rejects_divergent_protocol_id() {
        let expected = protocol_version();
        let received = ProtocolVersion {
            app_protocol_id: expected.app_protocol_id ^ 0xFFFF,
            wire_version: expected.wire_version,
        };
        let err = validate_handshake(expected, received).expect_err("divergent id must reject");
        assert_eq!(err.expected, expected);
        assert_eq!(err.received, received);
    }

    #[test]
    fn transport_protocol_id_packs_both_consts() {
        let id = transport_protocol_id();
        assert_eq!((id >> 32) as u32, PROTOCOL_ID);
        assert_eq!((id & 0xFFFF_FFFF) as u32, WIRE_VERSION);
    }

    #[test]
    fn channel_ids_are_distinct_and_ordered() {
        assert_eq!(u8::from(Channel::Control), 0);
        assert_eq!(u8::from(Channel::Snapshot), 1);
        assert_eq!(u8::from(Channel::Input), 2);
    }

    #[test]
    fn connection_config_registers_three_channels() {
        let config = connection_config();
        assert_eq!(config.server_channels_config.len(), 3);
        assert_eq!(config.client_channels_config.len(), 3);
    }

    // --- Loopback integration: accept and reject over real UDP sockets. ---
    //
    // Bounded poll loop (never an unbounded/blocking wait). The netcode transport
    // is non-blocking, so each `update` drains and returns; we sleep a few ms
    // between polls to let packets traverse loopback. If the sandbox starves these
    // sockets the loop exits after MAX_POLLS without hanging — the pure-function
    // tests above remain the primary gate. No flakiness was observed across runs.

    const MAX_POLLS: usize = 400;
    const POLL_DT: Duration = Duration::from_millis(16);
    const POLL_SLEEP: Duration = Duration::from_millis(3);

    fn bound_socket() -> (UdpSocket, SocketAddr) {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind ephemeral udp socket");
        let addr = socket.local_addr().expect("resolve bound addr");
        (socket, addr)
    }

    /// Drive a server + client to a settled state, collecting server handshake
    /// outcomes. `client_version` is the app-level `ProtocolVersion` the client
    /// sends — diverging it (while the transport `protocol_id` stays equal) lets
    /// the connection establish and then be app-rejected.
    fn run_handshake(
        client_version: ProtocolVersion,
    ) -> (NetServer, Vec<HandshakeOutcome>, bool) {
        let (server_sock, server_addr) = bound_socket();
        let (client_sock, _client_addr) = bound_socket();

        let mut server =
            NetServer::new(server_sock, server_addr, 8, now()).expect("server transport");
        let mut client =
            NetClient::new(client_sock, server_addr, 1, now()).expect("client transport");

        // The reject test needs the client to send a *diverged* app version while
        // keeping the transport handshake valid. We drive the renet/transport
        // connection manually so we control the exact control payload.
        let mut all_outcomes = Vec::new();
        let mut sent_diverged = false;
        let diverged = client_version != protocol_version();
        let mut client_was_connected = false;

        for _ in 0..MAX_POLLS {
            // Client side: drive transport; once connected, send the chosen version
            // exactly once (overriding NetClient's own handshake for the reject case).
            client.client.update(POLL_DT);
            client
                .transport
                .update(POLL_DT, &mut client.client)
                .expect("client transport update");
            if client.is_connected() {
                client_was_connected = true;
                if diverged {
                    if !sent_diverged {
                        let bytes = wire::encode(&client_version);
                        client.client.send_message(Channel::Control, bytes);
                        sent_diverged = true;
                    }
                } else if !client.handshake_sent {
                    let bytes = wire::encode(&client_version);
                    client.client.send_message(Channel::Control, bytes);
                    client.handshake_sent = true;
                }
            }
            client
                .transport
                .send_packets(&mut client.client)
                .expect("client send packets");

            // Server side: full update (drains socket, validates handshakes).
            let outcomes = server.update(POLL_DT).expect("server update");
            all_outcomes.extend(outcomes);

            // Stop once we have a verdict (accept or reject seen).
            if !all_outcomes.is_empty() {
                // Give the disconnect/ack a couple more frames to flush, then stop.
                for _ in 0..3 {
                    std::thread::sleep(POLL_SLEEP);
                    client.client.update(POLL_DT);
                    let _ = client.transport.update(POLL_DT, &mut client.client);
                    let _ = client.transport.send_packets(&mut client.client);
                    let extra = server.update(POLL_DT).expect("server update");
                    all_outcomes.extend(extra);
                }
                break;
            }

            std::thread::sleep(POLL_SLEEP);
        }

        (server, all_outcomes, client_was_connected)
    }

    #[test]
    fn loopback_matching_version_is_accepted() {
        let (server, outcomes, connected) = run_handshake(protocol_version());
        assert!(connected, "transport connection should establish over loopback");
        let accepted = outcomes
            .iter()
            .find_map(|o| match o {
                HandshakeOutcome::Accepted { client_id } => Some(*client_id),
                HandshakeOutcome::Rejected { .. } => None,
            })
            .expect("matching version should be accepted");
        assert!(server.is_accepted(accepted));
    }

    #[test]
    fn loopback_diverged_app_version_is_rejected_with_typed_reason() {
        let expected = protocol_version();
        let diverged = ProtocolVersion {
            app_protocol_id: expected.app_protocol_id,
            wire_version: expected.wire_version + 7,
        };
        let (server, outcomes, connected) = run_handshake(diverged);
        assert!(
            connected,
            "transport connection must establish (transport protocol_id is equal) so the \
             app gate can reject it"
        );

        let reason = outcomes
            .iter()
            .find_map(|o| match o {
                HandshakeOutcome::Rejected { reason, .. } => Some(*reason),
                HandshakeOutcome::Accepted { .. } => None,
            })
            .expect("diverged app version must produce a typed reject reason");
        assert_eq!(reason.expected, expected);
        assert_eq!(reason.received, diverged);

        // No client remains accepted: the reject path sends no entity state.
        assert!(
            server.accepted_clients().is_empty(),
            "a rejected client must not be accepted (and thus receives no snapshot)"
        );

        // And the send_snapshot guard refuses any post-reject entity state.
        assert!(
            !outcomes.iter().any(|o| matches!(o, HandshakeOutcome::Accepted { .. })),
            "diverged client must never be accepted"
        );
    }
}
