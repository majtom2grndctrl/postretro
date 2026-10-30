// Dev-only latency-sim harness: relay conditioner with scripted drops and renet datagram
// inspection, plus relay-pair fixtures (`reveal_both`). Gated on `dev-tools` or `test`.
// See: context/lib/networking.md
//
// This is the in-process latency simulator for the transport's
// connection-level relay accessors. It sits between an already-connected
// `NetServer`/`NetClient` pair, pulling the connection-level packet buffers
// each side wants to send (`packets_to_send`), holding them on a *virtual*
// clock, and delivering them to the peer (`process_packet` /
// `process_packet_from`) once their scheduled arrival time has passed —
// applying one-way delay, bounded jitter, and loss.
//
// It is deliberately NOT turmoil: turmoil conditions tokio sockets, but this
// path bypasses the renet_netcode UDP transport entirely and conditions opaque
// packet buffers in-process, so no socket interception (and no async runtime)
// is involved. The clock is driven by the caller (`advance`) — the conditioner
// never reads wall-clock time, so its delay/jitter/loss decisions are fully
// deterministic under a fixed seed (testing_guide "deterministic time").

#![cfg(any(test, feature = "dev-tools"))]

/// Virtual time unit for the conditioner clock, in milliseconds. The harness
/// never reads a real clock; the caller advances this monotone counter and the
/// conditioner schedules deliveries against it. A `u64` of milliseconds covers
/// any test horizon without overflow.
pub type VirtualMillis = u64;

/// Deterministic SplitMix64 PRNG. Inlined (≈10 lines) rather than pulling in a
/// `rand` dependency: the net crate keeps a minimal dependency tree, and a
/// single seeded stream is all the conditioner needs for reproducible
/// jitter/loss decisions. Same seed ⇒ same sequence, every run, every platform.
#[derive(Debug, Clone)]
pub(crate) struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub(crate) fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Next raw 64-bit value (the canonical SplitMix64 finalizer).
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform `f64` in `[0, 1)` — used for the loss coin-flip. Takes the top
    /// 53 bits so the mantissa is filled exactly without bias.
    fn next_unit_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A uniform integer in `[0, bound]` inclusive — used for the jitter draw.
    /// `bound == 0` yields `0` (no jitter) without touching the stream.
    fn next_in_inclusive(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            return 0;
        }
        self.next_u64() % (bound + 1)
    }
}

/// Conditioning parameters for one direction of a link. Times are
/// milliseconds: virtual ones on [`PacketConditioner`]'s own clock, wall-clock
/// ones in the dev lag proxy (`crate::lag_proxy`), which ignores `seed`.
#[derive(Debug, Clone, Copy)]
pub struct LinkConfig {
    /// Base one-way delay applied to every delivered packet.
    pub delay: VirtualMillis,
    /// Maximum additional delay drawn uniformly from `[0, jitter]` per packet.
    /// `0` disables jitter (every packet gets exactly `delay`).
    pub jitter: VirtualMillis,
    /// Probability in `[0, 1]` that a packet is dropped entirely on enqueue.
    /// `0.0` never drops; `1.0` always drops.
    pub loss_probability: f64,
    /// Seed for the deterministic PRNG driving jitter and loss.
    pub seed: u64,
}

impl LinkConfig {
    /// A perfect link: no delay, no jitter, no loss. The PRNG is still seeded so
    /// the type is well-formed, but with both jitter and loss off it is never
    /// consulted.
    #[must_use]
    pub fn perfect() -> Self {
        Self {
            delay: 0,
            jitter: 0,
            loss_probability: 0.0,
            seed: 0,
        }
    }

    /// The per-packet decision of the link model, shared by
    /// [`PacketConditioner`] and the dev lag proxy (`crate::lag_proxy`): `None`
    /// drops the packet, else its one-way delay (`delay` plus a uniform
    /// `[0, jitter]` draw). The loss flip consumes the stream only when loss is
    /// on, and the jitter draw only when the packet survives and jitter is on,
    /// so the stream position is a deterministic function of the call sequence.
    pub(crate) fn sample_delay(&self, rng: &mut SplitMix64) -> Option<VirtualMillis> {
        if self.loss_probability > 0.0 && rng.next_unit_f64() < self.loss_probability {
            return None;
        }
        Some(self.delay + rng.next_in_inclusive(self.jitter))
    }
}

/// A test-scripted loss on top of the seeded link model: the datagram is
/// dropped even when the loss coin passes it. A test that needs "the
/// datagram carrying command t" lost targets it by its position among one
/// channel's datagrams; `NthOnChannel` reads the renet channel byte through
/// [`renet_packet_channel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptedDrop {
    /// The `index`th datagram (0-based) whose renet header names `channel`
    /// ([`renet_packet_channel`]), counting every such datagram whatever its
    /// fate. An ack-only datagram carries no channel and never matches.
    NthOnChannel {
        channel: crate::transport::Channel,
        index: u64,
    },
    /// Every datagram enqueued while the virtual clock is in `[start, end)`.
    Window {
        start: VirtualMillis,
        end: VirtualMillis,
    },
}

impl ScriptedDrop {
    fn matches(self, channel_index: Option<(u8, u64)>, now: VirtualMillis) -> bool {
        match self {
            Self::NthOnChannel {
                channel,
                index: target,
            } => channel_index == Some((u8::from(channel), target)),
            Self::Window { start, end } => (start..end).contains(&now),
        }
    }
}

/// The renet channel id a connection-level datagram carries, or `None` for an
/// ack-only or unparseable one.
///
/// Couples to the pinned renet `=2.0.0` packet layout (`Packet::to_bytes`):
/// byte 0 is the packet type (0 SmallReliable, 1 SmallUnreliable,
/// 2 ReliableSlice, 3 UnreliableSlice, 4 Ack), then a QUIC-varint sequence,
/// then the channel id byte. A renet upgrade must re-check it; the
/// conditioner's scripted-drop self-test reads real renet datagrams through it.
#[must_use]
pub fn renet_packet_channel(packet: &[u8]) -> Option<u8> {
    let (&kind, rest) = packet.split_first()?;
    if kind > 3 {
        return None;
    }
    // A QUIC varint's top two bits give its length: 1, 2, 4 or 8 bytes.
    let varint_len = 1usize << (rest.first()? >> 6);
    rest.get(varint_len).copied()
}

/// What one renet datagram carries, read back for test inspection: which
/// messages a flush packed, which slice of a large message it is, or a renet
/// ack. Message bytes are the transport's own (a participation or Control
/// frame on most channels); [`participation_frame_payload`] and
/// [`server_control_payload`] unwrap them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenetDatagram {
    /// Small reliable messages for one channel, each with its renet message id.
    Reliable {
        channel: u8,
        messages: Vec<(u64, Vec<u8>)>,
    },
    /// Small unreliable messages for one channel.
    Unreliable { channel: u8, messages: Vec<Vec<u8>> },
    /// One slice of a message larger than renet's 1200-byte slice size.
    Slice {
        channel: u8,
        reliable: bool,
        message_id: u64,
        slice_index: u64,
        num_slices: u64,
    },
    /// renet's own ack packet: no channel and no message.
    Ack,
}

/// Parse one connection-level datagram. Couples to the pinned renet `=2.0.0`
/// layout as [`renet_packet_channel`] does (`Packet::to_bytes`: type byte,
/// QUIC-varint sequence, channel byte, then a big-endian `u16` message count
/// with varint-length messages, or a slice header). `None` for anything that
/// does not parse.
#[must_use]
pub fn parse_renet_datagram(packet: &[u8]) -> Option<RenetDatagram> {
    let mut reader = DatagramReader(packet);
    let kind = reader.u8()?;
    if kind == 4 {
        return Some(RenetDatagram::Ack);
    }
    let _sequence = reader.varint()?;
    let channel = reader.u8()?;
    let datagram = match kind {
        0 => {
            let count = reader.u16()?;
            let mut messages = Vec::with_capacity(usize::from(count));
            for _ in 0..count {
                let message_id = reader.varint()?;
                messages.push((message_id, reader.varint_bytes()?.to_vec()));
            }
            RenetDatagram::Reliable { channel, messages }
        }
        1 => {
            let count = reader.u16()?;
            let mut messages = Vec::with_capacity(usize::from(count));
            for _ in 0..count {
                messages.push(reader.varint_bytes()?.to_vec());
            }
            RenetDatagram::Unreliable { channel, messages }
        }
        2 | 3 => {
            let message_id = reader.varint()?;
            let slice_index = reader.varint()?;
            let num_slices = reader.varint()?;
            let _payload = reader.varint_bytes()?;
            RenetDatagram::Slice {
                channel,
                reliable: kind == 2,
                message_id,
                slice_index,
                num_slices,
            }
        }
        _ => return None,
    };
    reader.0.is_empty().then_some(datagram)
}

/// The engine payload inside a Snapshot, client Input or Presentation message,
/// with the transport's participation frame removed.
#[must_use]
pub fn participation_frame_payload(message: &[u8]) -> Option<Vec<u8>> {
    crate::wire::decode::<crate::wire::ParticipationFrame>(message)
        .ok()
        .map(|frame| frame.payload)
}

/// The engine payload inside a server Control message, or `None` for the
/// transport-only participation-start marker.
#[must_use]
pub fn server_control_payload(message: &[u8]) -> Option<Vec<u8>> {
    crate::wire::decode::<crate::wire::ServerControlFrame>(message)
        .ok()
        .and_then(|frame| frame.payload)
}

/// A cursor over one datagram's bytes in renet's (octets') byte order.
struct DatagramReader<'a>(&'a [u8]);

impl<'a> DatagramReader<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        if self.0.len() < len {
            return None;
        }
        let (head, rest) = self.0.split_at(len);
        self.0 = rest;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|bytes| bytes[0])
    }

    fn u16(&mut self) -> Option<u16> {
        self.take(2)
            .map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    /// A QUIC varint: the top two bits of the first byte give its length.
    fn varint(&mut self) -> Option<u64> {
        let len = 1usize << (self.0.first()? >> 6);
        let bytes = self.take(len)?;
        Some(
            bytes
                .iter()
                .enumerate()
                .fold(0u64, |value, (index, &byte)| {
                    let byte = if index == 0 { byte & 0x3f } else { byte };
                    (value << 8) | u64::from(byte)
                }),
        )
    }

    fn varint_bytes(&mut self) -> Option<&'a [u8]> {
        let len = usize::try_from(self.varint()?).ok()?;
        self.take(len)
    }
}

/// What became of one datagram the conditioner enqueued, read back by its
/// position among one channel's datagrams ([`PacketConditioner::channel_datagram`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatagramFate {
    /// Virtual time it was enqueued: the sender's flush.
    pub enqueued_at: VirtualMillis,
    /// Virtual time it arrives, or `None` when the link or a scripted drop lost it.
    pub arrives_at: Option<VirtualMillis>,
}

/// One packet buffer scheduled for delivery at a virtual arrival time.
#[derive(Debug, Clone)]
struct ScheduledPacket {
    deliver_at: VirtualMillis,
    packet: Vec<u8>,
}

/// In-memory packet conditioner for one direction of the relay. Holds enqueued
/// packet buffers on a virtual clock and releases them once their scheduled
/// arrival time has been reached, applying configured delay, jitter, and loss.
///
/// Lifecycle per relayed step:
/// 1. `enqueue` each buffer from `packets_to_send` (jitter/loss decided here),
/// 2. `advance` the virtual clock by the elapsed virtual time,
/// 3. `take_ready` to collect the buffers due at the new clock, then hand each
///    to the peer's `process_packet` / `process_packet_from`.
///
/// Determinism: with a fixed `seed`, the loss coin-flips and jitter draws form a
/// fixed sequence, so a given series of enqueues produces the same drops and the
/// same arrival times every run.
#[derive(Debug)]
pub struct PacketConditioner {
    config: LinkConfig,
    rng: SplitMix64,
    /// Current virtual time. Monotone; advanced only by `advance`.
    now: VirtualMillis,
    /// Packets in flight, not yet due. Not kept sorted; `take_ready` scans.
    queue: Vec<ScheduledPacket>,
    /// Count of packets dropped by the loss model or a scripted drop, for test
    /// assertions/telemetry.
    dropped: u64,
    /// The scripted share of `dropped`.
    scripted_dropped: u64,
    /// Scripted drops in force, applied after the seeded draw.
    scripted: Vec<ScriptedDrop>,
    /// Per renet channel id, the fate of every datagram enqueued carrying it,
    /// in enqueue order: index `n` is the [`ScriptedDrop::NthOnChannel`] index.
    channel_datagrams: std::collections::HashMap<u8, Vec<DatagramFate>>,
}

impl PacketConditioner {
    /// Build a conditioner for `config`, with its clock at virtual time 0.
    #[must_use]
    pub fn new(config: LinkConfig) -> Self {
        Self {
            rng: SplitMix64::new(config.seed),
            config,
            now: 0,
            queue: Vec::new(),
            dropped: 0,
            scripted_dropped: 0,
            scripted: Vec::new(),
            channel_datagrams: std::collections::HashMap::new(),
        }
    }

    /// Add a scripted drop, effective from the next enqueue. A scripted run
    /// draws the same PRNG stream as the unscripted one, so its other
    /// datagrams keep their fates and arrival times.
    pub fn script_drop(&mut self, drop: ScriptedDrop) {
        self.scripted.push(drop);
    }

    /// How many of [`Self::dropped`] were scripted drops.
    #[must_use]
    pub fn scripted_dropped(&self) -> u64 {
        self.scripted_dropped
    }

    /// Datagrams enqueued so far carrying `channel`: the index the next one
    /// will take for [`ScriptedDrop::NthOnChannel`].
    #[must_use]
    pub fn enqueued_on_channel(&self, channel: crate::transport::Channel) -> u64 {
        self.channel_datagrams
            .get(&u8::from(channel))
            .map_or(0, |fates| fates.len() as u64)
    }

    /// The fate of the `index`th datagram (0-based) carrying `channel`, once it
    /// has been enqueued: when it left and when, if ever, it arrives.
    #[must_use]
    pub fn channel_datagram(
        &self,
        channel: crate::transport::Channel,
        index: u64,
    ) -> Option<DatagramFate> {
        let fates = self.channel_datagrams.get(&u8::from(channel))?;
        fates.get(usize::try_from(index).ok()?).copied()
    }

    /// The link decision for the next enqueued datagram: its arrival time, or
    /// `None` when it is dropped. The seeded draw runs first and unchanged, so a
    /// scripted drop never shifts the stream for later datagrams.
    fn schedule(&mut self, packet: &[u8]) -> Option<VirtualMillis> {
        let channel = renet_packet_channel(packet);
        let channel_index = channel.map(|channel| {
            let count = self.channel_datagrams.get(&channel).map_or(0, Vec::len);
            (channel, count as u64)
        });
        let arrives_at = match self.config.sample_delay(&mut self.rng) {
            None => {
                self.dropped += 1;
                None
            }
            Some(_)
                if self
                    .scripted
                    .iter()
                    .any(|drop| drop.matches(channel_index, self.now)) =>
            {
                self.dropped += 1;
                self.scripted_dropped += 1;
                None
            }
            Some(delay) => Some(self.now + delay),
        };
        if let Some(channel) = channel {
            self.channel_datagrams
                .entry(channel)
                .or_default()
                .push(DatagramFate {
                    enqueued_at: self.now,
                    arrives_at,
                });
        }
        arrives_at
    }

    /// The conditioner's current virtual time.
    #[must_use]
    pub fn now(&self) -> VirtualMillis {
        self.now
    }

    /// How many packets have been dropped so far, seeded and scripted alike
    /// (see [`Self::scripted_dropped`] for the scripted share).
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Packets currently in flight (enqueued, not yet delivered or dropped).
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.queue.len()
    }

    /// Enqueue one packet buffer. The loss coin is flipped first, then any
    /// scripted drop applies: a dropped packet is counted and discarded
    /// (returns `false`, never scheduled). A surviving packet is scheduled at
    /// `now + delay + jitter_draw` and returns `true`. PRNG consumption follows
    /// [`LinkConfig::sample_delay`], so the stream position is a deterministic
    /// function of the enqueue sequence.
    pub fn enqueue(&mut self, packet: Vec<u8>) -> bool {
        let Some(deliver_at) = self.schedule(&packet) else {
            return false;
        };
        self.queue.push(ScheduledPacket { deliver_at, packet });
        true
    }

    /// Enqueue a batch of packet buffers (e.g. one `packets_to_send` drain),
    /// returning how many survived the loss model.
    pub fn enqueue_all(&mut self, packets: impl IntoIterator<Item = Vec<u8>>) -> usize {
        packets.into_iter().filter(|p| self.enqueue_one(p)).count()
    }

    /// Internal: enqueue by reference so `enqueue_all` can count survivors
    /// without consuming twice. Clones only the surviving buffer.
    fn enqueue_one(&mut self, packet: &[u8]) -> bool {
        // Mirror `enqueue` but avoid cloning a dropped buffer.
        let Some(deliver_at) = self.schedule(packet) else {
            return false;
        };
        self.queue.push(ScheduledPacket {
            deliver_at,
            packet: packet.to_vec(),
        });
        true
    }

    /// Advance the virtual clock by `dt` milliseconds. Delivery decisions happen
    /// in `take_ready`; this only moves time forward.
    pub fn advance(&mut self, dt: VirtualMillis) {
        self.now += dt;
    }

    /// Remove and return every packet whose scheduled arrival time is at or
    /// before the current virtual clock, in nondecreasing arrival-time order
    /// (ties keep enqueue order — a stable sort). The caller feeds each returned
    /// buffer to the peer's `process_packet` / `process_packet_from`.
    pub fn take_ready(&mut self) -> Vec<Vec<u8>> {
        let now = self.now;
        // Partition: keep not-yet-due in `queue`, collect due ones.
        let mut ready: Vec<ScheduledPacket> = Vec::new();
        let mut still_pending: Vec<ScheduledPacket> = Vec::with_capacity(self.queue.len());
        for sched in self.queue.drain(..) {
            if sched.deliver_at <= now {
                ready.push(sched);
            } else {
                still_pending.push(sched);
            }
        }
        self.queue = still_pending;
        // Stable sort by arrival time so ordered channels see no reordering for
        // equal-delay packets (jitter can legitimately reorder, modelling a real
        // link — renet's reliable channel re-sequences regardless).
        ready.sort_by_key(|s| s.deliver_at);
        ready.into_iter().map(|s| s.packet).collect()
    }
}

/// Test fixture: both peers reveal `identity`. Parity alone never promotes;
/// a slot also needs the host's own reveal and the client's revealed
/// declaration (`networking.md` §Slot lifecycle), so every parity-matched relay
/// pair that expects to participate calls this beside its parity setup. The
/// client's declaration rides its next Control flush.
pub fn reveal_both(
    server: &mut crate::transport::NetServer,
    client: &mut crate::transport::NetClient,
    identity: &str,
) {
    server.set_revealed_level(Some(identity.to_owned()));
    client.set_revealed_level(Some(identity.to_owned()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{Channel, NetClient, NetServer};
    use crate::wire::{
        self, COMPONENT_KIND_TRANSFORM, RECORD_KIND_FULL_BASELINE, RawComponentPayload,
        RawEntityRecord, RawSnapshotMessage, SNAPSHOT_VERSION, WireTransform,
    };
    use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
    use std::time::Duration;

    const CLIENT_ID: u64 = 1;
    const RELAY_DT: Duration = Duration::from_millis(16);
    const RELAY_DT_MS: VirtualMillis = 16;

    fn bound_socket() -> (UdpSocket, SocketAddr) {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind ephemeral udp socket");
        let addr = socket.local_addr().expect("resolve bound addr");
        (socket, addr)
    }

    /// Stand up a `NetServer`/`NetClient` pair wired through the in-memory relay
    /// (no UDP traffic). Sockets are bound only to satisfy the transport
    /// constructors; `add_relay_connection` + `set_connected` establish the
    /// renet connection directly, so the netcode handshake never runs.
    fn relay_pair() -> (NetServer, NetClient) {
        // A fixed virtual origin: the netcode clock is never advanced over the
        // relay path, so any constant works.
        let origin = Duration::from_secs(1);
        let (server_sock, server_addr) = bound_socket();
        let (client_sock, _client_addr) = bound_socket();

        let static_fingerprint = [0x5a; 32];
        let mut server = NetServer::new(
            server_sock,
            server_addr,
            8,
            origin,
            Some(static_fingerprint),
        )
        .expect("server transport");
        let mut client = NetClient::new(
            client_sock,
            server_addr,
            CLIENT_ID,
            origin,
            Some(static_fingerprint),
            None,
        )
        .expect("client transport");

        // E15 admission is immutable and parity is a separate live predicate.
        // This relay fixture intentionally installs a complete matching triple
        // and reveals it on both peers, so the slot participates.
        server.set_mod_identity("test.mod".to_string(), "1.0.0".to_string());
        server.set_mod_digest(Some(static_fingerprint));
        server.set_level_parity(Some(("test-level".to_string(), static_fingerprint)));
        client.set_mod_identity("test.mod".to_string(), "1.0.0".to_string());
        client.set_mod_digest(Some(static_fingerprint));
        client.set_level_parity(Some(("test-level".to_string(), static_fingerprint)));
        reveal_both(&mut server, &mut client, "test-level");

        server.add_relay_connection(CLIENT_ID, None);
        client.set_connected();
        (server, client)
    }

    // --- PRNG determinism ---

    #[test]
    fn splitmix64_is_deterministic_for_a_fixed_seed() {
        let mut a = SplitMix64::new(0xDEAD_BEEF);
        let mut b = SplitMix64::new(0xDEAD_BEEF);
        for _ in 0..64 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn splitmix64_unit_f64_stays_in_unit_interval() {
        let mut rng = SplitMix64::new(7);
        for _ in 0..10_000 {
            let u = rng.next_unit_f64();
            assert!((0.0..1.0).contains(&u), "unit draw out of range: {u}");
        }
    }

    // --- Conditioner timing ---

    #[test]
    fn conditioner_holds_packet_until_delay_elapses() {
        let mut cond = PacketConditioner::new(LinkConfig {
            delay: 50,
            jitter: 0,
            loss_probability: 0.0,
            seed: 1,
        });
        assert!(cond.enqueue(b"hello".to_vec()));

        // Before the delay elapses, nothing is ready.
        cond.advance(16);
        assert!(cond.take_ready().is_empty());
        cond.advance(16);
        assert!(cond.take_ready().is_empty());

        // Once virtual time passes 50ms, the packet is delivered exactly once.
        cond.advance(32); // now = 64 >= 50
        let ready = cond.take_ready();
        assert_eq!(ready, vec![b"hello".to_vec()]);
        assert!(
            cond.take_ready().is_empty(),
            "delivered packet is not re-delivered"
        );
    }

    #[test]
    fn conditioner_loss_is_deterministic_under_seed() {
        // Same seed + same enqueue sequence ⇒ identical drop pattern.
        let run = |seed: u64| {
            let mut cond = PacketConditioner::new(LinkConfig {
                delay: 0,
                jitter: 0,
                loss_probability: 0.5,
                seed,
            });
            (0..32)
                .map(|i| cond.enqueue(vec![i as u8]))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(0x1234_5678), run(0x1234_5678));
    }

    // --- The acceptance test: handshake + snapshot through the conditioner. ---

    fn sample_snapshot() -> RawSnapshotMessage {
        RawSnapshotMessage {
            server_script_time: 0.0,
            level_tick: 0,
            version: SNAPSHOT_VERSION,
            sequence: 1,
            server_tick: 42,
            records: vec![RawEntityRecord {
                record_kind: RECORD_KIND_FULL_BASELINE,
                network_id: 7,
                baseline_id_or_ref: 1,
                new_baseline_id_or_tombstone_id: 0,
                reason: 0,
                has_last_processed_client_tick: false,
                last_processed_client_tick: 0,
                local_player: false,
                has_entity_class: false,
                entity_class: String::new(),
                has_active_weapon_archetype: false,
                active_weapon_archetype: String::new(),
                projectile_presentation: None,
                presentation_binding: None,
                components: vec![RawComponentPayload {
                    teleporter: None,
                    component_kind: COMPONENT_KIND_TRANSFORM,
                    transform: Some(WireTransform {
                        position: [1.5, -2.0, 3.25],
                        rotation: [0.0, 0.0, 0.0, 1.0],
                        scale: [1.0, 1.0, 1.0],
                    }),
                    player_movement: None,
                    mesh_animation_state: None,
                    kinematic_mover: None,
                    light: None,
                    emitter: None,
                    fog: None,
                    ghost_link: None,
                }],
            }],
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
            faction_sentiment_record: None,
            ghost_audio: None,
        }
    }

    /// Relay every queued client→server packet through `cond` at non-zero delay,
    /// advancing the virtual clock each step until the queue drains. The server
    /// then polls handshakes so its app gate runs over the delivered control
    /// messages.
    fn pump_client_to_server(
        server: &mut NetServer,
        client: &mut NetClient,
        cond: &mut PacketConditioner,
    ) {
        // Drive the client connection layer so it queues its handshake control
        // message, then relay the resulting packets through the conditioner.
        for _ in 0..16 {
            client.update_connections(RELAY_DT);
            cond.enqueue_all(client.packets_to_send());
            cond.advance(RELAY_DT_MS);
            for packet in cond.take_ready() {
                server.process_packet_from(&packet, CLIENT_ID);
            }
            server.update_connections(RELAY_DT);
            let _ = server.poll_handshakes();
            if cond.in_flight() == 0 && client.admission_sent() {
                // One more flush pass to drain anything just queued.
                cond.enqueue_all(client.packets_to_send());
                cond.advance(RELAY_DT_MS);
                for packet in cond.take_ready() {
                    server.process_packet_from(&packet, CLIENT_ID);
                }
                let _ = server.poll_handshakes();
                break;
            }
        }
    }

    /// Relay every queued server→client packet through `cond`, advancing the
    /// clock until the queue drains, so the client receives delivered buffers.
    fn pump_server_to_client(
        server: &mut NetServer,
        client: &mut NetClient,
        cond: &mut PacketConditioner,
    ) {
        for _ in 0..16 {
            server.update_connections(RELAY_DT);
            cond.enqueue_all(server.packets_to_send(CLIENT_ID));
            cond.advance(RELAY_DT_MS);
            for packet in cond.take_ready() {
                client.process_packet(&packet);
            }
            client.update_connections(RELAY_DT);
            if cond.in_flight() == 0 {
                break;
            }
        }
    }

    // The headline acceptance test. A handshake + one snapshot is relayed
    // through the conditioner at non-zero delay; the snapshot must decode to the
    // expected value after conditioned delivery.
    #[test]
    fn snapshot_survives_conditioned_relay_and_decodes() {
        let (mut server, mut client) = relay_pair();
        let mut cond = PacketConditioner::new(LinkConfig {
            delay: 40,
            jitter: 8,
            loss_probability: 0.0, // no loss on the live channels — loss is exercised at buffer level below
            seed: 0xA5A5_1234,
        });

        // 1. Relay the handshake (client → server) through the conditioner.
        pump_client_to_server(&mut server, &mut client, &mut cond);
        assert!(
            server.is_participating(CLIENT_ID),
            "handshake must complete over the conditioned relay before snapshots flow"
        );

        // 2. Server sends one snapshot; relay it (server → client) through the
        //    conditioner at the same non-zero delay.
        let snapshot = sample_snapshot();
        assert!(
            server.send_snapshot(CLIENT_ID, wire::encode(&snapshot)),
            "accepted client must accept the snapshot send"
        );
        pump_server_to_client(&mut server, &mut client, &mut cond);

        // 3. Runtime drains reliable Control before Snapshot each frame. The
        // transport-only participation marker arms this snapshot's epoch and
        // must not escape as an engine Control message.
        assert!(
            client.drain_control().is_empty(),
            "participation marker must remain transport-only"
        );

        // 4. The client must receive the snapshot and decode it byte-faithfully.
        let received = client.drain_snapshots();
        assert_eq!(received.len(), 1, "exactly one snapshot should arrive");
        let decoded: RawSnapshotMessage =
            wire::decode(&received[0]).expect("conditioned delivery must not corrupt the payload");
        assert_eq!(
            decoded, snapshot,
            "snapshot decodes to the expected value after conditioned delay/jitter"
        );
    }

    // Loss assertion at the conditioner buffer level (see report): renet's
    // reliable control channel resends a dropped buffer, so "one specific packet
    // drops and stays dropped" cannot be pinned through the live channel. We
    // instead prove the conditioner's invariant directly — a loss-dropped decoy
    // buffer never arrives, while the real snapshot buffer survives and decodes.
    #[test]
    fn conditioner_drops_decoy_but_delivers_snapshot_buffer() {
        // Seed chosen so the loss model drops the *first* enqueued buffer (the
        // decoy) and passes the *second* (the snapshot). Verified by the
        // deterministic-drop assertion below before relying on it.
        let mut cond = PacketConditioner::new(LinkConfig {
            delay: 30,
            jitter: 0,
            loss_probability: 0.5,
            seed: SEED_DROP_FIRST_PASS_SECOND,
        });

        let decoy = b"DECOY-not-a-snapshot".to_vec();
        let snapshot = sample_snapshot();
        let snapshot_bytes = wire::encode(&snapshot);

        // Enqueue decoy first, snapshot second.
        let decoy_survived = cond.enqueue(decoy.clone());
        let snapshot_survived = cond.enqueue(snapshot_bytes.clone());
        assert!(!decoy_survived, "decoy must be dropped by the loss model");
        assert!(snapshot_survived, "snapshot must survive the loss model");
        assert_eq!(cond.dropped(), 1);
        assert_eq!(cond.in_flight(), 1, "only the snapshot is in flight");

        // Advance past the delay; only the snapshot buffer is delivered.
        cond.advance(16);
        assert!(cond.take_ready().is_empty(), "not yet due");
        cond.advance(16); // now = 32 >= 30
        let delivered = cond.take_ready();
        assert_eq!(
            delivered.len(),
            1,
            "exactly one buffer survives the conditioner"
        );
        assert_ne!(delivered[0], decoy, "the dropped decoy must not arrive");

        // The surviving buffer decodes to the expected snapshot.
        let decoded: RawSnapshotMessage =
            wire::decode(&delivered[0]).expect("surviving buffer must be the snapshot");
        assert_eq!(decoded, snapshot);
    }

    /// Seed pinned so the first loss coin-flip drops and the second passes, with
    /// `loss_probability = 0.5`. Asserted by `seed_drops_first_passes_second` so
    /// the decoy/snapshot test above is not relying on an unverified constant.
    const SEED_DROP_FIRST_PASS_SECOND: u64 = 3;

    #[test]
    fn seed_drops_first_passes_second() {
        // Pin the exact drop pattern the decoy test depends on: at p=0.5 and this
        // seed, flip #1 < 0.5 (drop) and flip #2 >= 0.5 (pass).
        let mut rng = SplitMix64::new(SEED_DROP_FIRST_PASS_SECOND);
        let first = rng.next_unit_f64();
        let second = rng.next_unit_f64();
        assert!(first < 0.5, "first flip must drop (got {first})");
        assert!(second >= 0.5, "second flip must pass (got {second})");
    }

    // --- Scripted drop ---

    /// Real renet datagrams, one server flush per relay step. Each flush carries
    /// one new Input message beside the join's Control traffic and renet acks,
    /// so the stream mixes channels as a live link does.
    fn captured_server_flushes(steps: u8) -> Vec<Vec<Vec<u8>>> {
        let (mut server, mut client) = relay_pair();
        let mut flushes = Vec::new();
        for step in 0..steps {
            server.send_input(CLIENT_ID, vec![step; 24]);
            server.update_connections(RELAY_DT);
            let packets = server.packets_to_send(CLIENT_ID);
            for packet in &packets {
                client.process_packet(packet);
            }
            client.update_connections(RELAY_DT);
            for packet in client.packets_to_send() {
                server.process_packet_from(&packet, CLIENT_ID);
            }
            flushes.push(packets);
        }
        flushes
    }

    /// Relay `flushes` one per step, then drain, returning each arrival with
    /// the virtual time it was taken.
    fn relay_flushes(
        cond: &mut PacketConditioner,
        flushes: &[Vec<Vec<u8>>],
    ) -> Vec<(VirtualMillis, Vec<u8>)> {
        let mut arrivals = Vec::new();
        let drain_steps = std::iter::repeat_n(&[][..], 16);
        for flush in flushes.iter().map(Vec::as_slice).chain(drain_steps) {
            cond.enqueue_all(flush.iter().cloned());
            cond.advance(RELAY_DT_MS);
            let now = cond.now();
            arrivals.extend(cond.take_ready().into_iter().map(|p| (now, p)));
        }
        arrivals
    }

    #[test]
    fn scripted_drop_loses_one_input_datagram_and_keeps_other_arrivals() {
        const TARGET: u64 = 6;
        let flushes = captured_server_flushes(24);
        let input = u8::from(Channel::Input);
        let all: Vec<&Vec<u8>> = flushes.iter().flatten().collect();
        assert!(
            all.iter().any(|p| renet_packet_channel(p) != Some(input)),
            "the captured stream must mix Input with other datagrams"
        );
        let target = all
            .iter()
            .filter(|p| renet_packet_channel(p) == Some(input))
            .nth(TARGET as usize)
            .copied()
            .expect("the captured stream carries enough Input datagrams");

        let link = LinkConfig {
            delay: 45,
            jitter: 60,
            loss_probability: 0.05,
            seed: 0x5C41,
        };
        let mut unscripted = PacketConditioner::new(link);
        let baseline = relay_flushes(&mut unscripted, &flushes);
        let mut scripted = PacketConditioner::new(link);
        scripted.script_drop(ScriptedDrop::NthOnChannel {
            channel: Channel::Input,
            index: TARGET,
        });
        let observed = relay_flushes(&mut scripted, &flushes);

        assert!(
            baseline.iter().any(|(_, p)| p == target),
            "precondition: the seeded link delivers the targeted Input datagram"
        );
        assert_eq!(scripted.scripted_dropped(), 1);
        assert_eq!(scripted.dropped(), unscripted.dropped() + 1);
        let fate = |cond: &PacketConditioner, index| {
            cond.channel_datagram(Channel::Input, index)
                .expect("the Input datagram was enqueued")
        };
        assert_eq!(fate(&scripted, TARGET).arrives_at, None);
        assert_eq!(
            fate(&scripted, TARGET).enqueued_at,
            fate(&unscripted, TARGET).enqueued_at
        );
        assert!(fate(&unscripted, TARGET).arrives_at.is_some());
        assert_eq!(
            fate(&scripted, TARGET + 1),
            fate(&unscripted, TARGET + 1),
            "the next Input datagram keeps its seeded arrival"
        );
        let expected: Vec<_> = baseline.into_iter().filter(|(_, p)| p != target).collect();
        assert_eq!(
            observed, expected,
            "every other datagram keeps its fate and arrival time"
        );
    }

    #[test]
    fn scripted_window_drops_every_datagram_enqueued_inside_it() {
        let mut cond = PacketConditioner::new(LinkConfig {
            delay: 10,
            ..LinkConfig::perfect()
        });
        cond.script_drop(ScriptedDrop::Window { start: 32, end: 64 });
        let mut arrivals = Vec::new();
        // Enqueued at virtual 0, 16, 32, 48, 64 and 80 ms.
        for step in 0..6u8 {
            cond.enqueue(vec![step]);
            cond.advance(16);
            arrivals.extend(cond.take_ready());
        }
        cond.advance(16);
        arrivals.extend(cond.take_ready());
        assert_eq!(arrivals, vec![vec![0], vec![1], vec![4], vec![5]]);
        assert_eq!(cond.scripted_dropped(), 2);
    }

    // The datagram reader the relay fixtures tap flushes with, against real renet
    // datagrams: packed Input messages with their ids, a Control payload past the
    // slice size, a snapshot's participation frame, and renet's acks.
    #[test]
    fn datagram_reader_reads_real_renet_flushes() {
        let (mut server, mut client) = relay_pair();
        let mut cond = PacketConditioner::new(LinkConfig::perfect());
        pump_client_to_server(&mut server, &mut client, &mut cond);
        assert!(server.is_participating(CLIENT_ID));

        // Incompressible bytes: bitcode packs a run, which would fit one datagram.
        let mut rng = SplitMix64::new(0xC7);
        let large_control: Vec<u8> = (0..3_000).map(|_| rng.next_u64() as u8).collect();
        server.send_control(CLIENT_ID, large_control);
        server.send_input(CLIENT_ID, vec![1; 24]);
        server.send_input(CLIENT_ID, vec![2; 24]);
        let snapshot = sample_snapshot();
        assert!(server.send_snapshot(CLIENT_ID, wire::encode(&snapshot)));
        server.update_connections(RELAY_DT);
        let packets = server.packets_to_send(CLIENT_ID);
        let datagrams: Vec<RenetDatagram> = packets
            .iter()
            .map(|packet| parse_renet_datagram(packet).expect("a renet datagram parses"))
            .collect();

        let input = u8::from(Channel::Input);
        let input_messages: Vec<Vec<u8>> = datagrams
            .iter()
            .filter_map(|datagram| match datagram {
                RenetDatagram::Reliable { channel, messages } if *channel == input => {
                    Some(messages.iter().map(|(_, bytes)| bytes.clone()))
                }
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(input_messages, vec![vec![1; 24], vec![2; 24]]);

        let control = u8::from(Channel::Control);
        let slices: Vec<(u64, u64, u64)> = datagrams
            .iter()
            .filter_map(|datagram| match datagram {
                RenetDatagram::Slice {
                    channel,
                    reliable: true,
                    message_id,
                    slice_index,
                    num_slices,
                } if *channel == control => Some((*message_id, *slice_index, *num_slices)),
                _ => None,
            })
            .collect();
        assert_eq!(slices.len(), 3, "3000 B of Control payload is three slices");
        assert!(
            slices
                .iter()
                .enumerate()
                .all(|(index, &(id, slice, count))| id == slices[0].0
                    && slice == index as u64
                    && count == 3),
            "{slices:?}"
        );

        let snapshot_channel = u8::from(Channel::Snapshot);
        let snapshots: Vec<RawSnapshotMessage> = datagrams
            .iter()
            .filter_map(|datagram| match datagram {
                RenetDatagram::Unreliable { channel, messages } if *channel == snapshot_channel => {
                    Some(messages.iter().map(|message| {
                        let payload = participation_frame_payload(message)
                            .expect("a snapshot rides a participation frame");
                        wire::decode::<RawSnapshotMessage>(&payload).expect("snapshot decodes")
                    }))
                }
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(snapshots, vec![snapshot]);

        // The client acks what it received in a renet ack packet, and a small
        // Control message unwraps to the payload the host sent.
        let small_control = vec![0x5c; 8];
        server.send_control(CLIENT_ID, small_control.clone());
        server.update_connections(RELAY_DT);
        let control_payloads: Vec<Vec<u8>> = server
            .packets_to_send(CLIENT_ID)
            .iter()
            .filter_map(|packet| match parse_renet_datagram(packet)? {
                RenetDatagram::Reliable { channel, messages } if channel == control => Some(
                    messages
                        .into_iter()
                        .filter_map(|(_, bytes)| server_control_payload(&bytes)),
                ),
                _ => None,
            })
            .flatten()
            .collect();
        assert!(
            control_payloads.contains(&small_control),
            "{control_payloads:?}"
        );
        for packet in &packets {
            client.process_packet(packet);
        }
        client.update_connections(RELAY_DT);
        let client_packets = client.packets_to_send();
        assert!(
            client_packets
                .iter()
                .any(|packet| parse_renet_datagram(packet) == Some(RenetDatagram::Ack)),
            "the client's flush carries a renet ack"
        );
    }

    // A non-conditioned sanity baseline: a perfect link relays the handshake and
    // snapshot with zero delay, proving the pump helpers and relay wiring are
    // sound independent of the delay/loss model.
    #[test]
    fn perfect_link_relays_handshake_and_snapshot() {
        let (mut server, mut client) = relay_pair();
        let mut cond = PacketConditioner::new(LinkConfig::perfect());

        pump_client_to_server(&mut server, &mut client, &mut cond);
        assert!(server.is_participating(CLIENT_ID));

        let snapshot = sample_snapshot();
        assert!(server.send_snapshot(CLIENT_ID, wire::encode(&snapshot)));
        pump_server_to_client(&mut server, &mut client, &mut cond);

        assert!(
            client.drain_control().is_empty(),
            "participation marker must remain transport-only"
        );
        let received = client.drain_snapshots();
        assert_eq!(received.len(), 1);
        let decoded: RawSnapshotMessage = wire::decode(&received[0]).expect("decodes");
        assert_eq!(decoded, snapshot);
    }

    // --- Time-sync convergence under the Phase 1 latency harness ---
    //
    // The headline Task 5 acceptance test. A client/server time-sync exchange is
    // relayed through the conditioner at the mandated profile (150 ms RTT mean,
    // 5% loss, heavy jitter) on the virtual clock. The client clock estimate must
    // track the true server tick within 2 sim ticks once it has had enough
    // samples and time. Fully deterministic: seeded PRNG + caller-advanced virtual
    // clock, no wall-clock read anywhere.

    use crate::timesync::{
        ClockEstimator, DEFAULT_MICROS_PER_TICK, MonotonicClock, SAMPLE_PERIOD_US, TimeSyncSender,
    };

    /// The mandated automated harness profile, applied in BOTH directions: a
    /// 45..105 ms one-way range (75 ms mean) under the conditioner's additive
    /// jitter, 5% loss, fixed seed. Mean RTT ≈ 150 ms.
    fn timesync_link() -> LinkConfig {
        LinkConfig {
            delay: 45,
            jitter: 60,
            loss_probability: 0.05,
            seed: 0x1502,
        }
    }

    /// One sim tick in milliseconds (60 Hz). The virtual master clock advances in
    /// these steps so the server tick is exactly `virtual_ms * 60 / 1000` past a
    /// base, and the client microsecond clock advances 1000× as microseconds.
    const TICK_MS: VirtualMillis = 16; // ~16.667 ms; integer ms keeps the clock exact-ish

    /// A virtual monotonic microsecond clock for the estimator. The relay loop
    /// sets it from the master virtual-ms clock (plus a fixed client origin offset
    /// to prove the offset math handles an unrelated client clock origin).
    struct RelayClock {
        now_us: std::cell::Cell<u64>,
    }
    impl MonotonicClock for RelayClock {
        fn now_micros(&self) -> u64 {
            self.now_us.get()
        }
    }

    #[test]
    fn time_sync_estimate_tracks_server_tick_within_two_ticks_under_harness() {
        // Two conditioners, one per direction, both on the same mandated profile.
        let mut to_server = PacketConditioner::new(timesync_link());
        let mut to_client = PacketConditioner::new(timesync_link());

        // Master virtual clock (ms). The server's true tick is derived from it so
        // the estimator has a ground truth to converge to.
        let mut virtual_ms: VirtualMillis = 0;

        // The server's tick clock: an arbitrary non-zero base proves the estimator
        // recovers the offset, not just the rate. true_tick(ms) = base + ms*60/1000.
        const SERVER_TICK_BASE: u64 = 3_600; // 60 s worth of ticks of head-start
        let true_server_tick = |ms: u64| -> u64 { SERVER_TICK_BASE + (ms * 60) / 1000 };

        // The client monotonic microsecond clock has its own unrelated origin: a
        // fixed offset added on top of virtual-ms-as-microseconds. The estimator
        // must still recover the server tick from client-local times alone.
        const CLIENT_CLOCK_ORIGIN_US: u64 = 9_000_000_000; // arbitrary, unrelated
        let client_now_us = |ms: u64| -> u64 { CLIENT_CLOCK_ORIGIN_US + ms * 1000 };

        let clock = RelayClock {
            now_us: std::cell::Cell::new(client_now_us(0)),
        };
        let mut sender = TimeSyncSender::new();
        let mut estimator = ClockEstimator::new(DEFAULT_MICROS_PER_TICK);

        // In-flight echoes carry their scheduled client-arrival microsecond so the
        // estimator records the receive time on the client's own clock. The relay
        // delivers `ServerMessage::TimeSync` bytes; we tag each with the master-ms
        // arrival so we can compute the client receive microsecond at ingest.
        // (The conditioner works in ms; we translate to client microseconds.)

        let mut successful_samples: u32 = 0;
        // Run until BOTH gates pass: 5 s of simulated time AND ≥20 samples (the
        // spec's "whichever comes later"). Cap the loop so a regression cannot hang.
        const FIVE_SECONDS_MS: VirtualMillis = 5_000;
        const MIN_SAMPLES: u32 = 20;
        const MAX_MS: VirtualMillis = 60_000; // generous ceiling; never reached normally

        // Track the worst-case error once the gates are satisfied so the assertion
        // reflects steady-state tracking, not the cold-start transient.
        let mut max_error_after_gates = 0.0_f64;

        while virtual_ms < MAX_MS {
            // 1. Client: maybe emit a 5 Hz probe at the current client time.
            clock.now_us.set(client_now_us(virtual_ms));
            let client_tick = (virtual_ms * 60 / 1000) as u32;
            if let Some(req) = sender.maybe_send(&clock, client_tick) {
                // Register the issued sample_id so the estimator's provenance
                // guard accepts the eventual echo.
                estimator.record_sent(req.sample_id);
                // Encode as the real wire envelope so the codec is exercised.
                let msg = wire::ClientMessage::TimeSync(req);
                to_server.enqueue(wire::encode(&msg));
            }

            // 2. Advance both link clocks by one tick step.
            to_server.advance(TICK_MS);
            to_client.advance(TICK_MS);

            // 3. Server: deliver any due requests, echo each with the server tick
            //    sampled at the master clock now, enqueue the echo back.
            for packet in to_server.take_ready() {
                // Decode the client envelope; a corrupt packet is ignored, never a
                // panic (the conditioner does not corrupt, but the path is real).
                if let Ok(wire::ClientMessage::TimeSync(req)) =
                    wire::decode::<wire::ClientMessage>(&packet)
                {
                    let server_tick = true_server_tick(virtual_ms) as u32;
                    let echo = req.echo(server_tick, virtual_ms * 1000);
                    to_client.enqueue(wire::encode(&wire::ServerMessage::TimeSync(echo)));
                }
            }

            // 4. Client: deliver any due echoes, record the receive time on the
            //    client's own clock, fold into the estimator.
            for packet in to_client.take_ready() {
                if let Ok(wire::ServerMessage::TimeSync(echo)) =
                    wire::decode::<wire::ServerMessage>(&packet)
                {
                    let recv_us = client_now_us(virtual_ms);
                    if estimator.ingest_echo(&echo, recv_us) {
                        successful_samples += 1;
                    }
                }
            }

            // 5. Once both gates are met, accumulate the steady-state error.
            let gates_met = virtual_ms >= FIVE_SECONDS_MS && successful_samples >= MIN_SAMPLES;
            if gates_met && estimator.is_initialized() {
                let estimated = estimator.estimated_server_tick_at(client_now_us(virtual_ms));
                let truth = true_server_tick(virtual_ms) as f64;
                let error = (estimated - truth).abs();
                if error > max_error_after_gates {
                    max_error_after_gates = error;
                }
            }

            virtual_ms += TICK_MS;

            // Stop once both gates are satisfied AND we have measured a span of
            // steady-state error (another full second past the gate) so the bound
            // is checked on settled, not transient, state.
            if gates_met && virtual_ms >= FIVE_SECONDS_MS + 1_000 {
                break;
            }
        }

        // Both gates must actually have been reached.
        assert!(
            successful_samples >= MIN_SAMPLES,
            "expected at least {MIN_SAMPLES} successful samples through 5% loss, got {successful_samples}"
        );
        assert!(virtual_ms >= FIVE_SECONDS_MS, "must simulate at least 5 s");

        // The acceptance bound: estimated server tick within 2 sim ticks of truth.
        // Measured worst-case steady-state error is ~0.96 ticks at this seed.
        assert!(
            max_error_after_gates <= 2.0,
            "estimated server tick must track truth within 2 ticks after the gates; \
             worst-case steady-state error was {max_error_after_gates:.3} ticks \
             ({successful_samples} samples, sample period {SAMPLE_PERIOD_US} us)"
        );
    }
}
