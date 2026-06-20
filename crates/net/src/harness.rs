// Dev-only latency-sim harness: an in-memory packet relay conditioner. Gated on
// `dev-tools` (and always built under `test`).
// See: context/research/netcode/
//
// This is the in-process latency simulator for the Task 3 relay accessors. It
// sits between an already-connected `NetServer`/`NetClient` pair, pulling the
// connection-level packet buffers each side wants to send
// (`packets_to_send`), holding them on a *virtual* clock, and delivering them
// to the peer (`process_packet` / `process_packet_from`) once their scheduled
// arrival time has passed — applying one-way delay, bounded jitter, and loss.
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
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
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

/// Conditioning parameters for one direction of the relay. All times are in
/// virtual milliseconds on the conditioner's own clock.
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
    /// Count of packets dropped by the loss model, for test assertions/telemetry.
    dropped: u64,
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
        }
    }

    /// The conditioner's current virtual time.
    #[must_use]
    pub fn now(&self) -> VirtualMillis {
        self.now
    }

    /// How many packets have been dropped by the loss model so far.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Packets currently in flight (enqueued, not yet delivered or dropped).
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.queue.len()
    }

    /// Enqueue one packet buffer. The loss coin is flipped first: a dropped
    /// packet is counted and discarded (returns `false`, never scheduled). A
    /// surviving packet is scheduled at `now + delay + jitter_draw` and returns
    /// `true`.
    ///
    /// The PRNG is advanced for the loss flip on every call, and for the jitter
    /// draw only when the packet survives and `jitter > 0` — so the stream
    /// position is a deterministic function of the enqueue sequence.
    pub fn enqueue(&mut self, packet: Vec<u8>) -> bool {
        if self.config.loss_probability > 0.0 && self.rng.next_unit_f64() < self.config.loss_probability
        {
            self.dropped += 1;
            return false;
        }

        let jitter = self.rng.next_in_inclusive(self.config.jitter);
        let deliver_at = self.now + self.config.delay + jitter;
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
        if self.config.loss_probability > 0.0 && self.rng.next_unit_f64() < self.config.loss_probability
        {
            self.dropped += 1;
            return false;
        }
        let jitter = self.rng.next_in_inclusive(self.config.jitter);
        let deliver_at = self.now + self.config.delay + jitter;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{NetClient, NetServer};
    use crate::wire::{self, ComponentPayload, NetworkId, Snapshot, WireTransform};
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

        let mut server =
            NetServer::new(server_sock, server_addr, 8, origin).expect("server transport");
        let mut client =
            NetClient::new(client_sock, server_addr, CLIENT_ID, origin).expect("client transport");

        server.add_relay_connection(CLIENT_ID);
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
        assert!(cond.take_ready().is_empty(), "delivered packet is not re-delivered");
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
            (0..32).map(|i| cond.enqueue(vec![i as u8])).collect::<Vec<_>>()
        };
        assert_eq!(run(0x1234_5678), run(0x1234_5678));
    }

    // --- The acceptance test: handshake + snapshot through the conditioner. ---

    fn sample_snapshot() -> Snapshot {
        Snapshot {
            tick: 42,
            entries: vec![(
                NetworkId(7),
                ComponentPayload::Transform(WireTransform {
                    position: [1.5, -2.0, 3.25],
                    rotation: [0.0, 0.0, 0.0, 1.0],
                }),
            )],
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
            if cond.in_flight() == 0 && client.handshake_sent() {
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
            server.is_accepted(CLIENT_ID),
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

        // 3. The client must receive the snapshot and decode it byte-faithfully.
        let received = client.drain_snapshots();
        assert_eq!(received.len(), 1, "exactly one snapshot should arrive");
        let decoded: Snapshot =
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
        assert_eq!(delivered.len(), 1, "exactly one buffer survives the conditioner");
        assert_ne!(delivered[0], decoy, "the dropped decoy must not arrive");

        // The surviving buffer decodes to the expected snapshot.
        let decoded: Snapshot =
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

    // A non-conditioned sanity baseline: a perfect link relays the handshake and
    // snapshot with zero delay, proving the pump helpers and relay wiring are
    // sound independent of the delay/loss model.
    #[test]
    fn perfect_link_relays_handshake_and_snapshot() {
        let (mut server, mut client) = relay_pair();
        let mut cond = PacketConditioner::new(LinkConfig::perfect());

        pump_client_to_server(&mut server, &mut client, &mut cond);
        assert!(server.is_accepted(CLIENT_ID));

        let snapshot = sample_snapshot();
        assert!(server.send_snapshot(CLIENT_ID, wire::encode(&snapshot)));
        pump_server_to_client(&mut server, &mut client, &mut cond);

        let received = client.drain_snapshots();
        assert_eq!(received.len(), 1);
        let decoded: Snapshot = wire::decode(&received[0]).expect("decodes");
        assert_eq!(decoded, snapshot);
    }
}
