// Dev lag proxy: an in-process UDP relay that conditions a live endpoint's
// datagrams with the harness link model on a wall clock.
// See: context/lib/networking.md §Testing the conditioned link

#![cfg(feature = "dev-tools")]

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::harness::{LinkConfig, SplitMix64};

/// Largest datagram the proxy relays intact. renetcode never emits more than
/// `NETCODE_MAX_PACKET_BYTES` (1400); the headroom keeps an oversized stray
/// from being silently truncated into something that parses.
const MAX_DATAGRAM_BYTES: usize = 2048;
/// Packets held per direction. A full queue drops new packets, so a large delay
/// on a busy host bounds memory instead of growing without limit.
const MAX_QUEUED_PACKETS: usize = 4096;
/// Distinct downstream peers the proxy relays for. Beyond it a new sender's
/// datagrams are dropped, so a flood of source addresses cannot open sockets
/// without bound.
const MAX_PEERS: usize = 64;
/// Datagrams read from one socket per pump, so a flood cannot hold the frame.
const MAX_RECV_PER_PUMP: usize = 1024;
/// A peer silent this long has gone: its upstream socket is closed. Well past
/// netcode's 15 s connection timeout, so the server has already let it go.
const PEER_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const RNG_SEED: u64 = 0x1A6_C0DE;

/// Impairment applied to each direction through the proxy, with the harness
/// [`LinkConfig`] semantics: every packet waits `delay_ms` plus a uniform
/// `[0, jitter_ms]` draw, after a `loss_probability` drop flip. The default is
/// [`LagProfile::OFF`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LagProfile {
    pub delay_ms: u32,
    pub jitter_ms: u32,
    /// Clamped to `[0, 1]` when sampled; NaN reads as `0`.
    pub loss_probability: f64,
}

impl LagProfile {
    /// Forward unimpaired.
    pub const OFF: Self = Self {
        delay_ms: 0,
        jitter_ms: 0,
        loss_probability: 0.0,
    };

    /// Whether the profile forwards unimpaired. Loss at or below zero (or NaN)
    /// reads as off, matching how it is sampled.
    #[must_use]
    pub fn is_off(&self) -> bool {
        let link = self.link_config();
        link.delay == 0 && link.jitter == 0 && link.loss_probability == 0.0
    }

    fn link_config(&self) -> LinkConfig {
        let loss = if self.loss_probability.is_nan() {
            0.0
        } else {
            self.loss_probability.clamp(0.0, 1.0)
        };
        LinkConfig {
            delay: u64::from(self.delay_ms),
            jitter: u64::from(self.jitter_ms),
            loss_probability: loss,
            // The proxy owns one stream across profile changes; the seed is unused.
            seed: 0,
        }
    }
}

/// One datagram waiting for its arrival time. `peer` is the downstream address
/// the datagram belongs to: its source when heading to the target, its
/// destination when heading back.
#[derive(Debug)]
struct Scheduled {
    deliver_at: Instant,
    /// Enqueue order, so equal arrival times leave in the order they came.
    seq: u64,
    peer: SocketAddr,
    bytes: Vec<u8>,
}

impl PartialEq for Scheduled {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Scheduled {}

impl PartialOrd for Scheduled {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Scheduled {
    // Reversed: `BinaryHeap` is a max-heap and the earliest arrival must pop first.
    fn cmp(&self, other: &Self) -> Ordering {
        (other.deliver_at, other.seq).cmp(&(self.deliver_at, self.seq))
    }
}

/// One direction's in-flight datagrams, released by arrival time.
#[derive(Debug, Default)]
struct DelayQueue {
    heap: BinaryHeap<Scheduled>,
    next_seq: u64,
    overflowed: bool,
}

impl DelayQueue {
    /// Schedule `bytes` for `now + delay`. Returns `false` when the queue is full
    /// and the datagram was dropped.
    fn push(&mut self, now: Instant, delay: Duration, peer: SocketAddr, bytes: Vec<u8>) -> bool {
        if self.heap.len() >= MAX_QUEUED_PACKETS {
            if !self.overflowed {
                self.overflowed = true;
                log::warn!("[Net] lag proxy queue full; dropping packets");
            }
            return false;
        }
        self.overflowed = false;
        self.heap.push(Scheduled {
            deliver_at: now + delay,
            seq: self.next_seq,
            peer,
            bytes,
        });
        self.next_seq += 1;
        true
    }

    /// Pop the next datagram due at or before `now`, earliest first.
    fn pop_due(&mut self, now: Instant) -> Option<Scheduled> {
        if self.heap.peek()?.deliver_at > now {
            return None;
        }
        self.heap.pop()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.heap.len()
    }
}

/// The socket a downstream peer's traffic leaves through toward the target, so
/// the target sees one distinct address per peer.
struct Upstream {
    socket: UdpSocket,
    last_heard: Instant,
}

/// A polled UDP relay between downstream peers and one target address.
///
/// Every datagram a peer sends to [`LagProxy::local_addr`] leaves toward the
/// target through that peer's own upstream socket; every datagram the target
/// sends back to an upstream socket is relayed to its peer from the downstream
/// socket, so the peer sees replies from the address it dialed. Both directions
/// pass through the same [`LagProfile`].
///
/// - **Client mode** ([`LagProxy::client`]): the downstream socket is a fresh
///   loopback port the client transport dials; the target is the real host.
/// - **Host mode** ([`LagProxy::host`]): the downstream socket is the host's
///   public socket; the target is the server transport's internal loopback port.
///
/// Nothing runs off-thread: [`LagProxy::pump`] moves every datagram that has
/// arrived and releases every datagram that is due.
pub struct LagProxy {
    downstream: UdpSocket,
    local_addr: SocketAddr,
    target: SocketAddr,
    upstream_bind: SocketAddr,
    peers: HashMap<SocketAddr, Upstream>,
    to_target: DelayQueue,
    to_peer: DelayQueue,
    profile: LagProfile,
    rng: SplitMix64,
    peer_limit_logged: bool,
    buffer: Box<[u8; MAX_DATAGRAM_BYTES]>,
}

impl LagProxy {
    /// A client-mode proxy for a client dialing `host_addr`: binds a loopback
    /// port for the client transport to dial in its place.
    pub fn client(host_addr: SocketAddr) -> io::Result<Self> {
        let downstream = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        let upstream_bind = match host_addr {
            SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
            SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
        };
        Self::new(downstream, host_addr, upstream_bind)
    }

    /// A host-mode proxy that owns the host's `public_socket` and relays to the
    /// server transport bound at loopback `server_addr`.
    pub fn host(public_socket: UdpSocket, server_addr: SocketAddr) -> io::Result<Self> {
        Self::new(public_socket, server_addr, (Ipv4Addr::LOCALHOST, 0).into())
    }

    fn new(
        downstream: UdpSocket,
        target: SocketAddr,
        upstream_bind: SocketAddr,
    ) -> io::Result<Self> {
        downstream.set_nonblocking(true)?;
        Ok(Self {
            local_addr: downstream.local_addr()?,
            downstream,
            target,
            upstream_bind,
            peers: HashMap::new(),
            to_target: DelayQueue::default(),
            to_peer: DelayQueue::default(),
            profile: LagProfile::OFF,
            rng: SplitMix64::new(RNG_SEED),
            peer_limit_logged: false,
            buffer: Box::new([0; MAX_DATAGRAM_BYTES]),
        })
    }

    /// The downstream address: what a client-mode transport dials, or the
    /// host's public address.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    #[must_use]
    pub fn profile(&self) -> LagProfile {
        self.profile
    }

    /// Change the impairment. Datagrams already in flight keep the arrival time
    /// they were given; only datagrams received afterwards use `profile`.
    pub fn set_profile(&mut self, profile: LagProfile) {
        self.profile = profile;
    }

    /// Relay every datagram that has arrived, then send every datagram now due.
    pub fn pump(&mut self) {
        self.pump_at(Instant::now());
    }

    /// [`LagProxy::pump`] at an explicit monotonic `now`, so tests can drive
    /// arrival times without sleeping.
    pub(crate) fn pump_at(&mut self, now: Instant) {
        self.receive_downstream(now);
        self.receive_upstream(now);
        self.release_due(now);
        self.peers.retain(|_, upstream| {
            now.saturating_duration_since(upstream.last_heard) < PEER_IDLE_TIMEOUT
        });
    }

    fn receive_downstream(&mut self, now: Instant) {
        let link = self.profile.link_config();
        for _ in 0..MAX_RECV_PER_PUMP {
            let Some((len, peer)) = recv(&self.downstream, &mut self.buffer[..]) else {
                break;
            };
            if !self.peers.contains_key(&peer) {
                if self.peers.len() >= MAX_PEERS {
                    if !self.peer_limit_logged {
                        self.peer_limit_logged = true;
                        log::warn!("[Net] lag proxy peer limit reached; dropping {peer}");
                    }
                    continue;
                }
                match bind_upstream(self.upstream_bind) {
                    Ok(socket) => {
                        self.peers.insert(
                            peer,
                            Upstream {
                                socket,
                                last_heard: now,
                            },
                        );
                    }
                    Err(err) => {
                        log::warn!("[Net] lag proxy could not open a socket for {peer}: {err}");
                        continue;
                    }
                }
            }
            if let Some(upstream) = self.peers.get_mut(&peer) {
                upstream.last_heard = now;
            }
            if let Some(delay) = link.sample_delay(&mut self.rng) {
                let bytes = self.buffer[..len].to_vec();
                self.to_target
                    .push(now, Duration::from_millis(delay), peer, bytes);
            }
        }
    }

    fn receive_upstream(&mut self, now: Instant) {
        let link = self.profile.link_config();
        for (&peer, upstream) in &self.peers {
            for _ in 0..MAX_RECV_PER_PUMP {
                let Some((len, from)) = recv(&upstream.socket, &mut self.buffer[..]) else {
                    break;
                };
                // Mirrors the netcode client's own source check: only the
                // target's replies are relayed.
                if from != self.target {
                    continue;
                }
                if let Some(delay) = link.sample_delay(&mut self.rng) {
                    let bytes = self.buffer[..len].to_vec();
                    self.to_peer
                        .push(now, Duration::from_millis(delay), peer, bytes);
                }
            }
        }
    }

    fn release_due(&mut self, now: Instant) {
        while let Some(packet) = self.to_target.pop_due(now) {
            // A peer evicted while its datagrams were in flight has gone; drop them.
            if let Some(upstream) = self.peers.get(&packet.peer) {
                send(&upstream.socket, &packet.bytes, self.target);
            }
        }
        while let Some(packet) = self.to_peer.pop_due(now) {
            send(&self.downstream, &packet.bytes, packet.peer);
        }
    }

    #[cfg(test)]
    pub(crate) fn peer_count(&self) -> usize {
        self.peers.len()
    }

    #[cfg(test)]
    pub(crate) fn in_flight(&self) -> usize {
        self.to_target.len() + self.to_peer.len()
    }
}

fn bind_upstream(addr: SocketAddr) -> io::Result<UdpSocket> {
    let socket = UdpSocket::bind(addr)?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}

/// One non-blocking receive. `None` once the socket is drained or on an error
/// the relay cannot act on. A Windows `ConnectionReset` (an ICMP report for an
/// earlier send to a closed port) is skipped, as the transports skip it, so
/// netcode's timeout still governs an unreachable host.
fn recv(socket: &UdpSocket, buffer: &mut [u8]) -> Option<(usize, SocketAddr)> {
    loop {
        match socket.recv_from(buffer) {
            Ok(received) => return Some(received),
            Err(err) if err.kind() == io::ErrorKind::ConnectionReset => continue,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                ) =>
            {
                return None;
            }
            Err(err) => {
                log::debug!("[Net] lag proxy receive failed: {err}");
                return None;
            }
        }
    }
}

/// A failed send is dropped like a lost packet; netcode's resends and timeouts
/// already handle loss.
fn send(socket: &UdpSocket, bytes: &[u8], addr: SocketAddr) {
    if let Err(err) = socket.send_to(bytes, addr) {
        log::debug!("[Net] lag proxy send to {addr} failed: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::reveal_both;
    use crate::transport::{NetClient, NetServer};

    const STEP: Duration = Duration::from_millis(16);
    const MS: Duration = Duration::from_millis(1);

    fn delayed(delay_ms: u32) -> LagProfile {
        LagProfile {
            delay_ms,
            ..LagProfile::OFF
        }
    }

    fn lossy(loss_probability: f64) -> LagProfile {
        LagProfile {
            loss_probability,
            ..LagProfile::OFF
        }
    }

    fn loopback_socket() -> (UdpSocket, SocketAddr) {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind loopback socket");
        let addr = socket.local_addr().expect("loopback address");
        (socket, addr)
    }

    /// The next datagram within `timeout`, with its source.
    fn recv_within(socket: &UdpSocket, timeout: Duration) -> Option<(Vec<u8>, SocketAddr)> {
        socket
            .set_read_timeout(Some(timeout))
            .expect("set read timeout");
        let mut buffer = [0; MAX_DATAGRAM_BYTES];
        let (len, from) = socket.recv_from(&mut buffer).ok()?;
        Some((buffer[..len].to_vec(), from))
    }

    /// Pump at a fixed `now` until `count` datagrams are held. Loopback delivery
    /// is near-instant, but a datagram is only held once the proxy has read it.
    fn pump_until_held(proxy: &mut LagProxy, now: Instant, count: usize) {
        for _ in 0..500 {
            proxy.pump_at(now);
            if proxy.in_flight() >= count {
                return;
            }
            std::thread::sleep(MS);
        }
        panic!("proxy held {} of {count} datagrams", proxy.in_flight());
    }

    // --- Delay queue on an injected clock ---

    #[test]
    fn delay_queue_releases_a_datagram_at_its_arrival_time_and_not_before() {
        let t0 = Instant::now();
        let peer: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let mut queue = DelayQueue::default();
        assert!(queue.push(t0, 50 * MS, peer, vec![1]));
        assert!(queue.pop_due(t0 + 49 * MS).is_none());
        assert_eq!(queue.pop_due(t0 + 50 * MS).map(|p| p.bytes), Some(vec![1]));
        assert!(queue.pop_due(t0 + 50 * MS).is_none());
    }

    #[test]
    fn delay_queue_releases_by_arrival_time_then_enqueue_order() {
        let t0 = Instant::now();
        let peer: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let mut queue = DelayQueue::default();
        queue.push(t0, 30 * MS, peer, vec![1]);
        queue.push(t0, 10 * MS, peer, vec![2]);
        queue.push(t0, 10 * MS, peer, vec![3]);
        let order: Vec<u8> = std::iter::from_fn(|| queue.pop_due(t0 + 30 * MS))
            .map(|p| p.bytes[0])
            .collect();
        assert_eq!(order, vec![2, 3, 1]);
    }

    #[test]
    fn delay_queue_drops_beyond_its_bound() {
        let t0 = Instant::now();
        let peer: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let mut queue = DelayQueue::default();
        for _ in 0..MAX_QUEUED_PACKETS {
            assert!(queue.push(t0, MS, peer, Vec::new()));
        }
        assert!(!queue.push(t0, MS, peer, Vec::new()));
        assert_eq!(queue.len(), MAX_QUEUED_PACKETS);
    }

    #[test]
    fn profile_off_is_exactly_the_unimpaired_profile() {
        assert!(LagProfile::OFF.is_off());
        assert_eq!(LagProfile::default(), LagProfile::OFF);
        assert!(!delayed(1).is_off());
        assert!(!lossy(0.01).is_off());
        assert!(lossy(f64::NAN).is_off());
    }

    // --- Proxy over real sockets, on an injected clock ---

    #[test]
    fn a_configured_delay_holds_each_direction_until_its_arrival_time() {
        let (host, host_addr) = loopback_socket();
        let (client, _) = loopback_socket();
        let mut proxy = LagProxy::client(host_addr).expect("client proxy");
        proxy.set_profile(delayed(100));
        let t0 = Instant::now();

        client.send_to(b"up", proxy.local_addr()).unwrap();
        pump_until_held(&mut proxy, t0, 1);
        proxy.pump_at(t0 + 99 * MS);
        assert!(recv_within(&host, 20 * MS).is_none(), "held before arrival");
        proxy.pump_at(t0 + 100 * MS);
        let (bytes, upstream) = recv_within(&host, Duration::from_secs(1)).expect("released");
        assert_eq!(bytes, b"up");
        assert_ne!(
            upstream,
            proxy.local_addr(),
            "leaves through its upstream socket"
        );

        let t1 = t0 + 200 * MS;
        host.send_to(b"down", upstream).unwrap();
        pump_until_held(&mut proxy, t1, 1);
        proxy.pump_at(t1 + 99 * MS);
        assert!(
            recv_within(&client, 20 * MS).is_none(),
            "held before arrival"
        );
        proxy.pump_at(t1 + 100 * MS);
        let (bytes, from) = recv_within(&client, Duration::from_secs(1)).expect("released");
        assert_eq!(bytes, b"down");
        assert_eq!(
            from,
            proxy.local_addr(),
            "the reply comes from the dialed address"
        );
    }

    #[test]
    fn a_profile_change_applies_only_to_datagrams_received_after_it() {
        let (host, host_addr) = loopback_socket();
        let (client, _) = loopback_socket();
        let mut proxy = LagProxy::client(host_addr).expect("client proxy");
        proxy.set_profile(delayed(200));
        let t0 = Instant::now();

        client.send_to(b"before", proxy.local_addr()).unwrap();
        pump_until_held(&mut proxy, t0, 1);
        proxy.set_profile(LagProfile::OFF);
        client.send_to(b"after", proxy.local_addr()).unwrap();

        let mut after = None;
        for _ in 0..500 {
            proxy.pump_at(t0);
            after = recv_within(&host, MS);
            if after.is_some() {
                break;
            }
        }
        assert_eq!(after.map(|(bytes, _)| bytes), Some(b"after".to_vec()));
        assert_eq!(proxy.in_flight(), 1, "the earlier datagram keeps its delay");
        assert!(recv_within(&host, 20 * MS).is_none());

        proxy.pump_at(t0 + 200 * MS);
        let (bytes, _) = recv_within(&host, Duration::from_secs(1)).expect("released");
        assert_eq!(bytes, b"before");
    }

    #[test]
    fn loss_zero_relays_every_datagram_and_loss_one_relays_none() {
        const SENT: usize = 32;
        let (host, host_addr) = loopback_socket();
        let (client, _) = loopback_socket();
        let mut proxy = LagProxy::client(host_addr).expect("client proxy");
        let now = Instant::now();
        let relay = |proxy: &mut LagProxy, profile: LagProfile| {
            proxy.set_profile(profile);
            for _ in 0..SENT {
                client.send_to(b"x", proxy.local_addr()).unwrap();
            }
            let mut received = 0;
            for _ in 0..200 {
                proxy.pump_at(now);
                while recv_within(&host, MS).is_some() {
                    received += 1;
                }
                if received == SENT {
                    break;
                }
            }
            received
        };
        assert_eq!(relay(&mut proxy, lossy(0.0)), SENT);
        assert_eq!(relay(&mut proxy, lossy(1.0)), 0);
        assert_eq!(proxy.in_flight(), 0, "a lost datagram is never scheduled");
    }

    #[test]
    fn each_peer_gets_its_own_upstream_address_until_it_goes_idle() {
        let (host, host_addr) = loopback_socket();
        let (first, _) = loopback_socket();
        let (second, _) = loopback_socket();
        let mut proxy = LagProxy::host(loopback_socket().0, host_addr).expect("host proxy");
        let t0 = Instant::now();

        first.send_to(b"1", proxy.local_addr()).unwrap();
        second.send_to(b"2", proxy.local_addr()).unwrap();
        let mut sources = HashMap::new();
        for _ in 0..500 {
            proxy.pump_at(t0);
            while let Some((bytes, from)) = recv_within(&host, MS) {
                sources.insert(bytes, from);
            }
            if sources.len() == 2 {
                break;
            }
        }
        assert_eq!(proxy.peer_count(), 2);
        assert_ne!(sources[&b"1".to_vec()], sources[&b"2".to_vec()]);

        // A reply to each upstream address returns to its own peer only.
        host.send_to(b"to-2", sources[&b"2".to_vec()]).unwrap();
        let mut reply = None;
        for _ in 0..500 {
            proxy.pump_at(t0);
            reply = recv_within(&second, MS);
            if reply.is_some() {
                break;
            }
        }
        assert_eq!(reply.map(|(bytes, _)| bytes), Some(b"to-2".to_vec()));
        assert!(recv_within(&first, 20 * MS).is_none());

        proxy.pump_at(t0 + PEER_IDLE_TIMEOUT - MS);
        assert_eq!(proxy.peer_count(), 2, "not yet idle");
        proxy.pump_at(t0 + PEER_IDLE_TIMEOUT);
        assert_eq!(proxy.peer_count(), 0, "silent peers are closed");
    }

    // --- Real endpoints through the proxy ---

    fn proxied_server() -> (NetServer, SocketAddr) {
        let (socket, addr) = loopback_socket();
        let server = NetServer::new_behind_lag_proxy(socket, addr, 8, Duration::from_secs(1), None)
            .expect("proxied server");
        (server, addr)
    }

    fn proxied_client(server_addr: SocketAddr, client_id: u64) -> NetClient {
        let (socket, _) = loopback_socket();
        NetClient::new_behind_lag_proxy(
            socket,
            server_addr,
            client_id,
            Duration::from_secs(1),
            None,
            None,
        )
        .expect("proxied client")
    }

    fn plain_client(server_addr: SocketAddr, client_id: u64) -> NetClient {
        let (socket, _) = loopback_socket();
        NetClient::new(
            socket,
            server_addr,
            client_id,
            Duration::from_secs(1),
            None,
            None,
        )
        .expect("plain client")
    }

    /// Matching content, revealed on both peers, so a connection participates.
    fn matching(server: &mut NetServer, client: &mut NetClient) {
        server.set_mod_identity("postretro.test".to_string(), "1".to_string());
        client.set_mod_identity("postretro.test".to_string(), "1".to_string());
        server.set_mod_digest(Some([7; 32]));
        client.set_mod_digest(Some([7; 32]));
        server.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        client.set_level_parity(Some(("test-level".to_string(), [9; 32])));
        reveal_both(server, client, "test-level");
    }

    fn step(server: &mut NetServer, clients: &mut [&mut NetClient]) {
        for client in clients.iter_mut() {
            client.update(STEP).expect("client update");
            // Draining Control activates the participation epoch.
            let _ = client.drain_control();
        }
        let _ = server.update(STEP).expect("server update");
        std::thread::sleep(MS);
    }

    fn step_until(
        server: &mut NetServer,
        clients: &mut [&mut NetClient],
        steps: usize,
        mut done: impl FnMut(&mut NetServer, &mut [&mut NetClient]) -> bool,
    ) -> bool {
        for _ in 0..steps {
            step(server, clients);
            if done(server, clients) {
                return true;
            }
        }
        false
    }

    fn participate(server: &mut NetServer, client: &mut NetClient, client_id: u64) {
        let participating = step_until(server, &mut [client], 1000, |server, clients| {
            clients[0].is_participating() && server.is_participating(client_id)
        });
        assert!(
            participating,
            "client {client_id} participates through the proxy"
        );
    }

    /// Wall time for an Input to reach the host and its reply to come back.
    fn echo_round_trip(server: &mut NetServer, client: &mut NetClient, client_id: u64) -> Duration {
        let start = Instant::now();
        client.send_input(b"ping".to_vec());
        let mut echoed = false;
        let returned = step_until(server, &mut [client], 2000, |server, clients| {
            if !echoed && server.drain_input(client_id).contains(&b"ping".to_vec()) {
                server.send_input(client_id, b"pong".to_vec());
                echoed = true;
            }
            clients[0].drain_input().contains(&b"pong".to_vec())
        });
        assert!(returned, "the echo returns");
        start.elapsed()
    }

    #[test]
    fn unimpaired_proxies_on_both_ends_connect_and_carry_traffic() {
        let (mut server, server_addr) = proxied_server();
        let mut client = proxied_client(server_addr, 7);
        assert_eq!(server.lag_profile(), Some(LagProfile::OFF));
        assert_eq!(client.lag_profile(), Some(LagProfile::OFF));
        assert_eq!(
            server.local_addr(),
            vec![server_addr],
            "reports the public address"
        );
        matching(&mut server, &mut client);
        participate(&mut server, &mut client, 7);
        echo_round_trip(&mut server, &mut client, 7);
    }

    #[test]
    fn a_configured_delay_lengthens_the_round_trip_by_both_directions() {
        // Generous bound: the delayed trip cannot beat it, and an unimpaired
        // loopback trip of a few polls stays far under it.
        const BOUND: Duration = Duration::from_millis(300);
        let (mut server, server_addr) = proxied_server();
        let mut client = proxied_client(server_addr, 7);
        matching(&mut server, &mut client);
        participate(&mut server, &mut client, 7);

        let unimpaired = echo_round_trip(&mut server, &mut client, 7);
        assert!(unimpaired < BOUND, "unimpaired round trip {unimpaired:?}");

        // Each proxy delays both directions: 2 x 100 + 2 x 50 = 300 ms.
        assert!(client.set_lag_profile(delayed(100)));
        assert!(server.set_lag_profile(delayed(50)));
        let impaired = echo_round_trip(&mut server, &mut client, 7);
        assert!(impaired >= BOUND, "impaired round trip {impaired:?}");
        assert!(
            client.is_connected(),
            "a mid-session change keeps the connection"
        );
    }

    #[test]
    fn total_loss_delivers_nothing_until_the_profile_clears() {
        let (mut server, server_addr) = proxied_server();
        let mut client = proxied_client(server_addr, 7);
        assert!(client.set_lag_profile(lossy(1.0)));
        let reached = step_until(&mut server, &mut [&mut client], 100, |server, _| {
            server.has_connected_clients()
        });
        assert!(!reached, "no connect request reaches the host");
        assert!(!client.is_connected());

        assert!(client.set_lag_profile(lossy(0.0)));
        let connected = step_until(&mut server, &mut [&mut client], 1000, |server, clients| {
            clients[0].is_connected() && server.has_connected_clients()
        });
        assert!(connected, "the next request after the change connects");
    }

    #[test]
    fn two_clients_behind_a_host_proxy_keep_distinct_links() {
        let (mut server, server_addr) = proxied_server();
        let mut first = plain_client(server_addr, 7);
        let mut second = plain_client(server_addr, 8);
        matching(&mut server, &mut first);
        matching(&mut server, &mut second);
        let connected = step_until(&mut server, &mut [&mut first, &mut second], 1000, |s, c| {
            c.iter().all(|client| client.is_connected()) && s.connected_clients().len() == 2
        });
        assert!(connected, "both clients connect through one public port");
        let mut ids = server.connected_clients();
        ids.sort_unstable();
        assert_eq!(ids, vec![7, 8], "the host sees two distinct clients");

        server.send_input(7, b"to-7".to_vec());
        server.send_input(8, b"to-8".to_vec());
        let (mut got_first, mut got_second) = (Vec::new(), Vec::new());
        step_until(
            &mut server,
            &mut [&mut first, &mut second],
            1000,
            |_, clients| {
                got_first.extend(clients[0].drain_input());
                got_second.extend(clients[1].drain_input());
                !got_first.is_empty() && !got_second.is_empty()
            },
        );
        assert_eq!(got_first, vec![b"to-7".to_vec()]);
        assert_eq!(got_second, vec![b"to-8".to_vec()]);
    }

    /// A retry's fresh socket dials the proxy again, which opens a fresh
    /// upstream socket, so a host started late is still reached.
    #[test]
    fn a_retry_after_an_unanswered_attempt_connects_through_the_proxy() {
        let (reserved, server_addr) = loopback_socket();
        drop(reserved);
        let mut client = proxied_client(server_addr, 7);
        client
            .update(Duration::ZERO)
            .expect("first poll sends a request");
        client
            .update(Duration::from_secs(16))
            .expect("the timed-out attempt is retried, not reported");

        let socket = UdpSocket::bind(server_addr).expect("host binds the reserved port");
        let mut server =
            NetServer::new_behind_lag_proxy(socket, server_addr, 8, Duration::from_secs(1), None)
                .expect("proxied server");
        let connected = step_until(&mut server, &mut [&mut client], 1000, |_, clients| {
            clients[0].is_connected()
        });
        assert!(connected, "the retry connects");
    }
}
