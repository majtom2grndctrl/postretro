// renet/renet_netcode transport: polled, non-blocking UDP server/client over std sockets.
// See: context/lib/networking.md
//
// renet 2.0 separates the *connection layer* (`RenetServer`/`RenetClient`, which
// own channels and produce/consume opaque packet payloads) from the *netcode
// transport* (`NetcodeServerTransport`/`NetcodeClientTransport`, which encrypt
// those payloads and move them over a non-blocking `std::net::UdpSocket`). We wrap
// both into `NetServer`/`NetClient` with a synchronous `update(dt)` surface: no
// spawned runtime, no threads — the caller polls each frame (development_guide
// §4.2). The connection-layer packet I/O (`get_packets_to_send` /
// `process_packet*`) is re-exposed for the in-memory harness (`harness.rs`),
// which drives the same payloads without touching UDP.

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
use crate::wire::{self, ProtocolVersion, WireError};

/// Classify a renet [`DisconnectReason`] into the engine-visible [`CloseCause`].
/// A clean client-initiated disconnect is `Disconnect`; every other reason
/// (transport timeout, channel/serialization error, server-initiated) folds into
/// `Timeout` — from the cleanup path's view they are all "the link went away and
/// we must reclaim the slot". `DisconnectedByServer` is the server reclaiming the
/// slot itself (e.g. an app-handshake reject), also a non-clean close.
fn close_cause_from(reason: DisconnectReason) -> CloseCause {
    match reason {
        DisconnectReason::DisconnectedByClient => CloseCause::Disconnect,
        _ => CloseCause::Timeout,
    }
}

/// App protocol identity. Hand-bumped on any change that breaks cross-version
/// compatibility of the message *vocabulary* (a new control message, a changed
/// channel layout). Carried as `ProtocolVersion::app_protocol_id` and folded into
/// the transport-level `protocol_id`.
pub const PROTOCOL_ID: u32 = 0x_5052_4C33; // "PRL3" — Phase 3.5 adds the state-slot message family

/// Wire-format version. Hand-bumped whenever the bitcode byte layout of any wire
/// type changes (added field, reordered enum, bumped bitcode major). Carried as
/// `ProtocolVersion::wire_version` and folded into the transport-level
/// `protocol_id` so a wire-incompatible peer is refused at the netcode layer.
pub const WIRE_VERSION: u32 = 5;

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
/// - `Control` — reliable-ordered: the version handshake and spawn/despawn.
///   Order matters and loss is unacceptable.
/// - `Snapshot` — unreliable: snapshot replication records: full baselines,
///   deltas, despawn tombstones, and state records. Ack/refresh repairs missed baselines.
/// - `Input` — reliable-ordered: carries client→server acks, baseline-refresh
///   requests, and time-sync probes, plus server→client time-sync echoes.
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
    /// Per-client connection slot state and accepted->closed lifecycle transitions
    /// (`slots.rs`). Replaces the Phase 1 pending/accepted-only handshake map: it
    /// also models the terminal `Closed` state so post-close traffic is refused and
    /// the engine glue can run its remote-pawn cleanup on the close transition.
    slots: SlotTable,
}

/// What one `update`/`poll_handshakes` poll produced this frame: the app-handshake
/// verdicts (accept/reject, for logging and per-client replication registration) and
/// the slot lifecycle close events (clean disconnect or timeout) the engine glue
/// turns into remote-pawn cleanup. Accept transitions ride `handshakes` as
/// `HandshakeOutcome::Accepted`; close transitions ride `lifecycle`.
#[derive(Debug, Default)]
#[must_use = "a poll reports client accept/reject and slot close transitions; handle or explicitly ignore"]
pub struct ServerPoll {
    /// App-handshake verdicts produced this poll.
    pub handshakes: Vec<HandshakeOutcome>,
    /// Slot close transitions produced this poll (accepted slots that went closed).
    pub lifecycle: Vec<SlotEvent>,
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
            slots: SlotTable::new(),
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
    pub fn update(&mut self, dt: Duration) -> Result<ServerPoll, NetcodeTransportError> {
        self.server.update(dt);
        self.transport.update(dt, &mut self.server)?;

        // Reap connection lifecycle events: a connect opens a Pending slot; a
        // disconnect closes the slot, classifying clean-disconnect vs timeout, and
        // surfaces a close event when an *accepted* slot closes (so the glue cleans
        // up its slot-owned pawn). A closed slot is terminal — post-close traffic is
        // refused below.
        let mut lifecycle = Vec::new();
        while let Some(event) = self.server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    self.slots.on_connect(client_id);
                }
                ServerEvent::ClientDisconnected { client_id, reason } => {
                    if let Some(close) = self.slots.on_close(client_id, close_cause_from(reason)) {
                        lifecycle.push(close);
                    }
                }
            }
        }

        let handshakes = self.process_control_messages();

        self.transport.send_packets(&mut self.server);
        Ok(ServerPoll {
            handshakes,
            lifecycle,
        })
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
            // A closed slot is terminal: refuse its control traffic. renet may still
            // surface a buffered message from a peer mid-disconnect; the slot model
            // says that peer is gone, so drop it without a handshake decision.
            if self.slots.is_closed(client_id) {
                continue;
            }
            while let Some(bytes) = self.server.receive_message(client_id, Channel::Control) {
                // A client already accepted may send later control traffic.
                // No post-handshake control messages are defined today; drain
                // and ignore so the channel buffer does not accumulate.
                if self.slots.is_accepted(client_id) {
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
                        // The slot transitions Pending->Accepted exactly once; the
                        // emitted `SlotEvent::Accepted` is not surfaced here (the
                        // `HandshakeOutcome::Accepted` below carries the same accept
                        // signal the caller already consumes). The slot table is now
                        // the source of truth for `is_accepted`/`accepted_clients`.
                        // The caller must spawn the slot pawn from its
                        // `HandshakeOutcome::Accepted` handling — the accept signal
                        // rides `handshakes`, not `lifecycle`.
                        let _ = self.slots.on_accept(client_id);
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

    /// Disconnect a client at the app-handshake reject and close its slot. The
    /// slot moves to `Closed { Timeout }` (a server-initiated reject is a non-clean
    /// close from the slot model's view); renet flushes the disconnect over the
    /// transport on the next `send_packets`. A pending (never-accepted) reject emits
    /// no lifecycle event — there was no slot-owned pawn to clean up.
    fn reject(&mut self, client_id: ClientId, _reason: RejectReason) {
        let _ = self.slots.on_close(client_id, CloseCause::Timeout);
        self.server.disconnect(client_id);
    }

    /// Has the given client passed the app-level handshake and not since closed?
    #[must_use]
    pub fn is_accepted(&self, client_id: ClientId) -> bool {
        self.slots.is_accepted(client_id)
    }

    /// Has the given client's slot closed (clean disconnect or timeout)?
    #[must_use]
    pub fn is_closed(&self, client_id: ClientId) -> bool {
        self.slots.is_closed(client_id)
    }

    /// The current lifecycle state of a slot, or `None` if the client is unknown.
    #[must_use]
    pub fn slot_state(&self, client_id: ClientId) -> Option<SlotState> {
        self.slots.state(client_id)
    }

    /// Client ids that have passed the handshake and may receive entity state.
    /// Closed slots are excluded.
    #[must_use]
    pub fn accepted_clients(&self) -> Vec<ClientId> {
        self.slots.accepted_clients()
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

    /// Drain any input-channel messages from a client (Phase 2 acks, refresh
    /// requests, time-sync probes). A **closed** slot's input is refused: once the
    /// slot is gone its peer must not influence replication state, so any buffered
    /// message renet still surfaces is dropped (the stale-packets-after-close gate).
    pub fn drain_input(&mut self, client_id: ClientId) -> Vec<Vec<u8>> {
        if self.slots.is_closed(client_id) {
            // Drain-and-drop so renet's buffer does not grow, but return nothing.
            while self
                .server
                .receive_message(client_id, Channel::Input)
                .is_some()
            {}
            return Vec::new();
        }
        let mut out = Vec::new();
        while let Some(bytes) = self.server.receive_message(client_id, Channel::Input) {
            out.push(bytes.to_vec());
        }
        out
    }

    /// Send a buffer to a client on the reliable-ordered `Channel::Input`. Used
    /// for the Task 5 time-sync echo, which rides the input channel back to the
    /// client independently of snapshots. Unlike `send_snapshot`, this is not
    /// gated on acceptance: time-sync may flow to a connected client before it has
    /// passed the app handshake (the echo carries no entity state).
    pub fn send_input(&mut self, client_id: ClientId, payload: Vec<u8>) {
        self.server.send_message(client_id, Channel::Input, payload);
    }

    /// Connection-level packets renet wants delivered to `client_id`, *bypassing*
    /// the netcode UDP transport. The in-memory harness (`harness.rs`) hands these
    /// straight to the peer's `process_packet`. Returns `Vec<Vec<u8>>` (renet's
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
    /// connection itself. Mirrors `RenetServer::add_connection` and seeds the slot
    /// as `Pending`.
    pub fn add_relay_connection(&mut self, client_id: ClientId) {
        self.server.add_connection(client_id);
        self.slots.on_connect(client_id);
    }

    /// Force-close a relay slot with `cause`, mirroring a transport disconnect for
    /// the in-memory harness (which moves packets itself and so never sees a renet
    /// disconnect event). Returns the `SlotEvent::Closed` when an accepted slot
    /// closes — the same lifecycle signal `update` surfaces over a real socket — so
    /// harness-driven tests exercise the identical cleanup path. A clean disconnect
    /// passes `CloseCause::Disconnect`; a timeout passes `CloseCause::Timeout`.
    #[must_use]
    pub fn close_relay_connection(
        &mut self,
        client_id: ClientId,
        cause: CloseCause,
    ) -> Option<SlotEvent> {
        // Tell renet to drop the connection too so subsequent relay packets for this
        // client are ignored by the connection layer.
        self.server.remove_connection(client_id);
        self.slots.on_close(client_id, cause)
    }

    /// Drive only the renet connection layer (no socket). The in-memory relay calls
    /// this instead of `update`, then drains handshakes via `poll_handshakes`.
    pub fn update_connections(&mut self, dt: Duration) {
        self.server.update(dt);
    }

    /// Run the app-level handshake gate over already-delivered control messages.
    /// Used by the in-memory relay, which moves packets itself and so cannot use
    /// `update`'s socket path. Returns this poll's `ServerPoll` (handshake verdicts +
    /// any slot close transitions). The relay drives close transitions explicitly
    /// via [`NetServer::close_relay_connection`]; close events surfaced here come
    /// from a renet-initiated disconnect (e.g. a handshake reject's `disconnect`).
    pub fn poll_handshakes(&mut self) -> ServerPoll {
        let mut lifecycle = Vec::new();
        while let Some(event) = self.server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    self.slots.on_connect(client_id);
                }
                ServerEvent::ClientDisconnected { client_id, reason } => {
                    if let Some(close) = self.slots.on_close(client_id, close_cause_from(reason)) {
                        lifecycle.push(close);
                    }
                }
            }
        }
        let handshakes = self.process_control_messages();
        ServerPoll {
            handshakes,
            lifecycle,
        }
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

    /// Send a buffer over the reliable-ordered input channel. Carries
    /// `ClientMessage` variants: input commands, acks, baseline-refresh requests,
    /// and time-sync probes (`ClientMessage::TimeSync`).
    pub fn send_input(&mut self, input: Vec<u8>) {
        self.client.send_message(Channel::Input, input);
    }

    /// Drain input-channel buffers received this frame (reliable-ordered). The
    /// server sends time-sync echoes here; decode each buffer as a
    /// `TimeSyncEcho`.
    pub fn drain_input(&mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Some(bytes) = self.client.receive_message(Channel::Input) {
            out.push(bytes.to_vec());
        }
        out
    }

    /// Drain snapshot buffers received this frame (unreliable channel).
    pub fn drain_snapshots(&mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Some(bytes) = self.client.receive_message(Channel::Snapshot) {
            out.push(bytes.to_vec());
        }
        out
    }

    /// Drain control-channel buffers received this frame (reliable-ordered).
    /// No server→client control messages are defined today; exposed so callers
    /// can drain without depending on the channel layout staying empty.
    pub fn drain_control(&mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Some(bytes) = self.client.receive_message(Channel::Control) {
            out.push(bytes.to_vec());
        }
        out
    }

    /// Connection-level packets renet wants delivered to the server, bypassing the
    /// netcode UDP transport. The in-memory harness (`harness.rs`) hands these to
    /// the server's `process_packet_from`.
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
        let err =
            validate_handshake(expected, received).expect_err("divergent version must reject");
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

    // --- Slot lifecycle over the in-memory relay (no UDP). ---
    //
    // These prove the transport-level close gate: once a slot closes, the server
    // surfaces the close transition and refuses that peer's input/snapshot traffic.
    // The relay moves connection-layer packets directly; the netcode handshake never
    // runs, so the app handshake is driven by relaying the client's control message.

    use crate::slots::{CloseCause, SlotEvent, SlotState};

    const RELAY_CLIENT: ClientId = 7;
    const RELAY_DT: Duration = Duration::from_millis(16);

    /// Stand up a relay-connected server/client pair and drive the app handshake to
    /// acceptance by relaying the client's first control message.
    fn relay_accepted_pair() -> (NetServer, NetClient) {
        let origin = Duration::from_secs(1);
        let (server_sock, server_addr) = bound_socket();
        let (client_sock, _client_addr) = bound_socket();
        let mut server =
            NetServer::new(server_sock, server_addr, 8, origin).expect("server transport");
        let mut client = NetClient::new(client_sock, server_addr, RELAY_CLIENT, origin)
            .expect("client transport");

        server.add_relay_connection(RELAY_CLIENT);
        client.set_connected();

        // Pump client->server until the handshake control message is delivered and the
        // app gate accepts. Direct passthrough (no conditioner) so it settles fast.
        for _ in 0..16 {
            client.update_connections(RELAY_DT);
            for packet in client.packets_to_send() {
                server.process_packet_from(&packet, RELAY_CLIENT);
            }
            server.update_connections(RELAY_DT);
            let poll = server.poll_handshakes();
            if poll
                .handshakes
                .iter()
                .any(|o| matches!(o, HandshakeOutcome::Accepted { .. }))
            {
                break;
            }
        }
        assert!(
            server.is_accepted(RELAY_CLIENT),
            "relay client should accept"
        );
        (server, client)
    }

    /// Relay one round of client->server packets so a just-sent input message lands
    /// in the server's connection-layer receive buffer.
    fn relay_client_to_server(server: &mut NetServer, client: &mut NetClient) {
        client.update_connections(RELAY_DT);
        for packet in client.packets_to_send() {
            server.process_packet_from(&packet, RELAY_CLIENT);
        }
        server.update_connections(RELAY_DT);
    }

    #[test]
    fn close_relay_connection_surfaces_close_event_and_refuses_traffic() {
        let (mut server, _client) = relay_accepted_pair();

        // A clean disconnect surfaces the close transition with the Disconnect cause.
        let event = server.close_relay_connection(RELAY_CLIENT, CloseCause::Disconnect);
        assert_eq!(
            event,
            Some(SlotEvent::Closed {
                client_id: RELAY_CLIENT,
                cause: CloseCause::Disconnect,
            })
        );
        assert!(server.is_closed(RELAY_CLIENT));
        assert!(!server.is_accepted(RELAY_CLIENT));
        assert_eq!(
            server.slot_state(RELAY_CLIENT),
            Some(SlotState::Closed {
                cause: CloseCause::Disconnect
            })
        );

        // Post-close: a snapshot send is refused and accepted_clients excludes it.
        assert!(
            !server.send_snapshot(RELAY_CLIENT, vec![1, 2, 3]),
            "a closed slot receives no entity state"
        );
        assert!(server.accepted_clients().is_empty());
    }

    #[test]
    fn input_from_a_closed_slot_is_ignored() {
        let (mut server, mut client) = relay_accepted_pair();

        // Client sends an input message; relay it so it lands in the server's buffer.
        client.send_input(vec![9, 9, 9]);
        relay_client_to_server(&mut server, &mut client);

        // Close the slot BEFORE the server drains. The buffered (now stale) input must
        // be dropped, not returned.
        let _ = server.close_relay_connection(RELAY_CLIENT, CloseCause::Timeout);
        assert!(
            server.drain_input(RELAY_CLIENT).is_empty(),
            "stale input from a closed slot is ignored"
        );

        // A second close (e.g. a redundant disconnect event) is a no-op transition.
        assert_eq!(
            server.close_relay_connection(RELAY_CLIENT, CloseCause::Disconnect),
            None,
            "close is terminal and idempotent"
        );
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
    fn run_handshake(client_version: ProtocolVersion) -> (NetServer, Vec<HandshakeOutcome>, bool) {
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
            let poll = server.update(POLL_DT).expect("server update");
            all_outcomes.extend(poll.handshakes);

            // Stop once we have a verdict (accept or reject seen).
            if !all_outcomes.is_empty() {
                // Give the disconnect/ack a couple more frames to flush, then stop.
                for _ in 0..3 {
                    std::thread::sleep(POLL_SLEEP);
                    client.client.update(POLL_DT);
                    let _ = client.transport.update(POLL_DT, &mut client.client);
                    let _ = client.transport.send_packets(&mut client.client);
                    let extra = server.update(POLL_DT).expect("server update");
                    all_outcomes.extend(extra.handshakes);
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
        assert!(
            connected,
            "transport connection should establish over loopback"
        );
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
            !outcomes
                .iter()
                .any(|o| matches!(o, HandshakeOutcome::Accepted { .. })),
            "diverged client must never be accepted"
        );
    }
}
