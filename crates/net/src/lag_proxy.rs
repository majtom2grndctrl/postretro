// Dev lag proxy: an in-process UDP relay, on its own threads, that conditions
// a live endpoint's datagrams with the harness link model on a wall clock.
// See: context/lib/networking.md §Testing the conditioned link

#![cfg(feature = "dev-tools")]

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::harness::{LinkConfig, SplitMix64};

/// Receive buffer size. renetcode never emits more than
/// `NETCODE_MAX_PACKET_BYTES` (1400), so netcode traffic always fits. A larger
/// stray is truncated on Unix (and then fails netcode's own checks) or reported
/// as a receive error on Windows (`WSAEMSGSIZE`); either way the reader drops
/// it and keeps reading.
const MAX_DATAGRAM_BYTES: usize = 2048;
/// Packets held per direction. A full queue drops new packets, so a large delay
/// on a busy host bounds memory instead of growing without limit.
const MAX_QUEUED_PACKETS: usize = 4096;
/// A full queue warns again only once it has drained below this, so sustained
/// saturation warns once per episode, not once per packet.
const QUEUE_LOW_WATER: usize = MAX_QUEUED_PACKETS / 2;
/// Distinct downstream peers the proxy relays for. Beyond it a new sender's
/// datagrams are dropped, so a flood of source addresses cannot open sockets
/// and threads without bound.
const MAX_PEERS: usize = 64;
/// A peer silent this long has gone: its upstream socket is closed. Well past
/// netcode's 15 s connection timeout, so the server has already let it go.
const PEER_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a blocked reader waits before checking for shutdown and idle
/// peers. It bounds how long dropping the proxy takes; it never delays a
/// datagram, whose arrival wakes the reader at once.
const READ_TIMEOUT: Duration = Duration::from_millis(20);
/// Pause after a second consecutive receive error, so a socket stuck failing
/// cannot spin its reader.
const ERROR_BACKOFF: Duration = Duration::from_millis(5);
/// Fixed rather than time-seeded: nothing here needs unpredictability, and a
/// fixed stream keeps single-link tests reproducible.
const RNG_SEED: u64 = 0x1A6_C0DE;

/// Impairment applied to each direction through the proxy, with the harness
/// [`LinkConfig`] semantics: every packet waits `delay_ms` plus a uniform
/// `[0, jitter_ms]` draw, after a `loss_probability` drop flip. The default is
/// [`LagProfile::OFF`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LagProfile {
    /// Clamped to [`LagProfile::MAX_MS`] when sampled.
    pub delay_ms: u32,
    /// Clamped to [`LagProfile::MAX_MS`] when sampled.
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

    /// Ceiling on `delay_ms` and on `jitter_ms`, each. Past ~7.5 s one-way,
    /// netcode's 15 s timeout drops the link anyway; the ceiling keeps every
    /// held datagram due well inside the peer idle timeout, and lets a queue
    /// filled at the worst setting drain within seconds once lowered.
    pub const MAX_MS: u32 = 5_000;

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
            delay: u64::from(self.delay_ms.min(Self::MAX_MS)),
            jitter: u64::from(self.jitter_ms.min(Self::MAX_MS)),
            loss_probability: loss,
            // The proxy owns one stream across profile changes; the seed is unused.
            seed: 0,
        }
    }
}

/// One item waiting for its arrival time.
struct Entry<T> {
    deliver_at: Instant,
    /// Enqueue order, so equal arrival times leave in the order they came.
    seq: u64,
    item: T,
}

impl<T> PartialEq for Entry<T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<T> Eq for Entry<T> {}

impl<T> PartialOrd for Entry<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Entry<T> {
    // Reversed: `BinaryHeap` is a max-heap and the earliest arrival must pop first.
    fn cmp(&self, other: &Self) -> Ordering {
        (other.deliver_at, other.seq).cmp(&(self.deliver_at, self.seq))
    }
}

/// One direction's in-flight datagrams, released by arrival time. Pure: the
/// caller supplies every `Instant`.
struct DelayQueue<T> {
    heap: BinaryHeap<Entry<T>>,
    next_seq: u64,
    /// Set when a push is refused; cleared below [`QUEUE_LOW_WATER`].
    overflowed: bool,
}

impl<T> DelayQueue<T> {
    fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            next_seq: 0,
            overflowed: false,
        }
    }

    /// Schedule `item` for `deliver_at`. Returns `false` when the queue is full
    /// and the item was dropped.
    fn push(&mut self, deliver_at: Instant, item: T) -> bool {
        if self.heap.len() >= MAX_QUEUED_PACKETS {
            if !self.overflowed {
                self.overflowed = true;
                log::warn!("[Net] lag proxy queue full; dropping packets");
            }
            return false;
        }
        self.heap.push(Entry {
            deliver_at,
            seq: self.next_seq,
            item,
        });
        self.next_seq += 1;
        true
    }

    /// Pop the next item due at or before `now`, earliest first.
    fn pop_due(&mut self, now: Instant) -> Option<T> {
        if self.heap.peek()?.deliver_at > now {
            return None;
        }
        let entry = self.heap.pop()?;
        if self.heap.len() < QUEUE_LOW_WATER {
            self.overflowed = false;
        }
        Some(entry.item)
    }

    fn next_due(&self) -> Option<Instant> {
        self.heap.peek().map(|entry| entry.deliver_at)
    }

    fn len(&self) -> usize {
        self.heap.len()
    }
}

/// The live profile and the PRNG it samples, under one lock.
struct Link {
    profile: LagProfile,
    rng: SplitMix64,
}

/// State the handle shares with every proxy thread.
struct Shared {
    link: Mutex<Link>,
    shutdown: AtomicBool,
    /// Set by a failed send and cleared by the next success, so a failing
    /// streak warns once.
    send_failing: AtomicBool,
    peers: AtomicUsize,
    in_flight: AtomicUsize,
}

impl Shared {
    fn new() -> Self {
        Self {
            link: Mutex::new(Link {
                profile: LagProfile::OFF,
                rng: SplitMix64::new(RNG_SEED),
            }),
            shutdown: AtomicBool::new(false),
            send_failing: AtomicBool::new(false),
            peers: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
        }
    }

    // Poison only means a proxy thread panicked mid-sample; the plain data
    // inside is still whole.
    fn link(&self) -> MutexGuard<'_, Link> {
        self.link.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The link model's decision for one datagram, under the profile live now.
    fn sample_delay(&self) -> Option<Duration> {
        let mut link = self.link();
        let Link { profile, rng } = &mut *link;
        profile
            .link_config()
            .sample_delay(rng)
            .map(Duration::from_millis)
    }

    /// A failed send is dropped like a lost packet; netcode's resends and
    /// timeouts already handle loss.
    fn send(&self, socket: &UdpSocket, bytes: &[u8], to: SocketAddr) {
        match socket.send_to(bytes, to) {
            Ok(_) => {
                if self.send_failing.load(Relaxed) {
                    self.send_failing.store(false, Relaxed);
                }
            }
            Err(err) => {
                if !self.send_failing.swap(true, Relaxed) {
                    log::warn!("[Net] lag proxy send to {to} failed: {err}");
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Direction {
    ToTarget,
    ToPeer,
}

/// A datagram handed to the scheduler, with the socket it leaves through.
struct Delayed {
    direction: Direction,
    deliver_at: Instant,
    socket: Arc<UdpSocket>,
    to: SocketAddr,
    bytes: Vec<u8>,
}

/// What every reader needs to condition a datagram it received.
#[derive(Clone)]
struct Relay {
    shared: Arc<Shared>,
    scheduler: Sender<Delayed>,
}

impl Relay {
    /// Drop `bytes`, send them now, or hand them to the scheduler, by the
    /// profile live at `received_at`.
    fn relay(
        &self,
        direction: Direction,
        socket: &Arc<UdpSocket>,
        to: SocketAddr,
        bytes: &[u8],
        received_at: Instant,
    ) {
        let Some(delay) = self.shared.sample_delay() else {
            return;
        };
        if delay.is_zero() {
            // The reader sends it itself: an unimpaired datagram never waits
            // on another thread.
            self.shared.send(socket, bytes, to);
            return;
        }
        // The scheduler outlives every reader, so this fails only mid-shutdown.
        let _ = self.scheduler.send(Delayed {
            direction,
            deliver_at: received_at + delay,
            socket: Arc::clone(socket),
            to,
            bytes: bytes.to_vec(),
        });
    }
}

/// Holds delayed datagrams and sends each the moment it is due. Sleeps on the
/// channel until the next arrival time or the next delayed datagram; exits
/// once every reader has gone.
fn run_scheduler(shared: &Shared, arrivals: Receiver<Delayed>) {
    let mut to_target: DelayQueue<Delayed> = DelayQueue::new();
    let mut to_peer: DelayQueue<Delayed> = DelayQueue::new();
    loop {
        let now = Instant::now();
        for queue in [&mut to_target, &mut to_peer] {
            while let Some(packet) = queue.pop_due(now) {
                shared.send(&packet.socket, &packet.bytes, packet.to);
            }
        }
        shared
            .in_flight
            .store(to_target.len() + to_peer.len(), Relaxed);
        let next_due = match (to_target.next_due(), to_peer.next_due()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        let arrival = match next_due {
            Some(at) => arrivals.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => arrivals.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match arrival {
            Ok(packet) => {
                let queue = match packet.direction {
                    Direction::ToTarget => &mut to_target,
                    Direction::ToPeer => &mut to_peer,
                };
                queue.push(packet.deliver_at, packet);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Counts consecutive receive errors so a stuck socket logs once and backs off.
#[derive(Default)]
struct ReceiveErrors {
    streak: u32,
}

impl ReceiveErrors {
    /// One receive, blocking up to [`READ_TIMEOUT`]. `None` on timeout or on a
    /// skipped error; the datagram an error reports is gone either way.
    fn receive(&mut self, socket: &UdpSocket, buffer: &mut [u8]) -> Option<(usize, SocketAddr)> {
        match socket.recv_from(buffer) {
            Ok(received) => {
                self.streak = 0;
                Some(received)
            }
            // A Windows `ConnectionReset` is an ICMP report for an earlier send
            // to a closed port. Skipped, as the transports skip it, so
            // netcode's timeout still governs an unreachable host.
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                        | io::ErrorKind::ConnectionReset
                ) =>
            {
                None
            }
            Err(err) => {
                self.streak += 1;
                if self.streak == 1 {
                    log::debug!("[Net] lag proxy receive failed; skipping: {err}");
                } else {
                    thread::sleep(ERROR_BACKOFF);
                }
                None
            }
        }
    }
}

/// A downstream peer's upstream socket and the thread reading its replies.
struct Peer {
    socket: Arc<UdpSocket>,
    last_heard: Instant,
    stop: Arc<AtomicBool>,
    reader: JoinHandle<()>,
}

/// Reads the downstream socket and owns the peer table: opens a peer's
/// upstream socket on its first datagram and closes it once idle.
struct DownstreamReader {
    relay: Relay,
    socket: Arc<UdpSocket>,
    target: SocketAddr,
    upstream_bind: SocketAddr,
    idle_timeout: Duration,
    peers: HashMap<SocketAddr, Peer>,
    /// Readers told to stop, joined once finished so closing a peer never
    /// blocks relaying for the rest.
    retired: Vec<JoinHandle<()>>,
    last_sweep: Instant,
    peer_limit_logged: bool,
    open_failure_logged: bool,
}

impl DownstreamReader {
    fn run(mut self) {
        let mut buffer = vec![0; MAX_DATAGRAM_BYTES];
        let mut errors = ReceiveErrors::default();
        while !self.relay.shared.shutdown.load(Relaxed) {
            if let Some((len, peer)) = errors.receive(&self.socket, &mut buffer) {
                self.relay_from_peer(peer, &buffer[..len]);
            }
            self.sweep();
        }
        for (_, peer) in self.peers.drain() {
            peer.stop.store(true, Relaxed);
            self.retired.push(peer.reader);
        }
        for reader in self.retired.drain(..) {
            join(reader);
        }
    }

    fn relay_from_peer(&mut self, peer: SocketAddr, bytes: &[u8]) {
        let now = Instant::now();
        if !self.peers.contains_key(&peer) && !self.open_peer(peer, now) {
            return;
        }
        let Some(entry) = self.peers.get_mut(&peer) else {
            return;
        };
        entry.last_heard = now;
        self.relay
            .relay(Direction::ToTarget, &entry.socket, self.target, bytes, now);
    }

    fn open_peer(&mut self, peer: SocketAddr, now: Instant) -> bool {
        if self.peers.len() >= MAX_PEERS {
            if !self.peer_limit_logged {
                self.peer_limit_logged = true;
                log::warn!(
                    "[Net] lag proxy peer limit ({MAX_PEERS}) reached; dropping datagrams from {peer}"
                );
            }
            return false;
        }
        match self.spawn_peer(peer, now) {
            Ok(entry) => {
                self.open_failure_logged = false;
                self.peers.insert(peer, entry);
                self.relay.shared.peers.store(self.peers.len(), Relaxed);
                true
            }
            Err(err) => {
                if !self.open_failure_logged {
                    self.open_failure_logged = true;
                    log::warn!("[Net] lag proxy could not open a relay for {peer}: {err}");
                }
                false
            }
        }
    }

    fn spawn_peer(&self, peer: SocketAddr, now: Instant) -> io::Result<Peer> {
        let socket = Arc::new(bind_reader(self.upstream_bind)?);
        let stop = Arc::new(AtomicBool::new(false));
        let reader = UpstreamReader {
            relay: self.relay.clone(),
            socket: Arc::clone(&socket),
            stop: Arc::clone(&stop),
            target: self.target,
            downstream: Arc::clone(&self.socket),
            peer,
        };
        let reader = spawn("net-lag-upstream", move || reader.run())?;
        Ok(Peer {
            socket,
            last_heard: now,
            stop,
            reader,
        })
    }

    /// Close idle peers and join finished readers, at most once per
    /// [`READ_TIMEOUT`].
    fn sweep(&mut self) {
        let now = Instant::now();
        if now.saturating_duration_since(self.last_sweep) < READ_TIMEOUT {
            return;
        }
        self.last_sweep = now;
        let idle_timeout = self.idle_timeout;
        let idle = self
            .peers
            .extract_if(|_, peer| now.saturating_duration_since(peer.last_heard) >= idle_timeout);
        for (_, peer) in idle {
            peer.stop.store(true, Relaxed);
            self.retired.push(peer.reader);
        }
        if self.peers.len() < MAX_PEERS {
            self.peer_limit_logged = false;
        }
        self.relay.shared.peers.store(self.peers.len(), Relaxed);
        let (finished, running) = self
            .retired
            .drain(..)
            .partition::<Vec<_>, _>(JoinHandle::is_finished);
        self.retired = running;
        finished.into_iter().for_each(join);
    }
}

/// Reads one peer's upstream socket and relays the target's replies back to
/// that peer from the downstream socket, so the peer sees replies from the
/// address it dialed.
struct UpstreamReader {
    relay: Relay,
    socket: Arc<UdpSocket>,
    stop: Arc<AtomicBool>,
    target: SocketAddr,
    downstream: Arc<UdpSocket>,
    peer: SocketAddr,
}

impl UpstreamReader {
    fn run(self) {
        let mut buffer = vec![0; MAX_DATAGRAM_BYTES];
        let mut errors = ReceiveErrors::default();
        while !self.stop.load(Relaxed) && !self.relay.shared.shutdown.load(Relaxed) {
            let Some((len, from)) = errors.receive(&self.socket, &mut buffer) else {
                continue;
            };
            // Mirrors the netcode client's own source check: only the target's
            // replies are relayed.
            if from != self.target {
                continue;
            }
            self.relay.relay(
                Direction::ToPeer,
                &self.downstream,
                self.peer,
                &buffer[..len],
                Instant::now(),
            );
        }
    }
}

/// A UDP relay between downstream peers and one target address, running on
/// its own threads so delivery never waits on the engine's frame cadence.
///
/// Every datagram a peer sends to [`LagProxy::local_addr`] leaves toward the
/// target through that peer's own upstream socket; every datagram the target
/// sends back to an upstream socket is relayed to its peer from the downstream
/// socket. Both directions pass through the same [`LagProfile`].
///
/// - **Client mode** ([`LagProxy::client`]): the downstream socket is a fresh
///   loopback port the client transport dials; the target is the real host.
/// - **Host mode** ([`LagProxy::host`]): the downstream socket is the host's
///   public socket; the target is the server transport's internal loopback port.
///
/// One thread reads the downstream socket, one per peer reads its upstream
/// socket, and one scheduler sends delayed datagrams when due. Dropping the
/// proxy stops and joins them all and closes every socket.
pub struct LagProxy {
    local_addr: SocketAddr,
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl LagProxy {
    /// A client-mode proxy for a client dialing `host_addr`: binds a loopback
    /// port for the client transport to dial in its place. IPv4 only, as
    /// `--connect` is.
    pub fn client(host_addr: SocketAddr) -> io::Result<Self> {
        if host_addr.is_ipv6() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the lag proxy relays IPv4 only",
            ));
        }
        let downstream = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        Self::new(
            downstream,
            host_addr,
            (Ipv4Addr::UNSPECIFIED, 0).into(),
            PEER_IDLE_TIMEOUT,
        )
    }

    /// A host-mode proxy that owns the host's `public_socket` and relays to the
    /// server transport bound at loopback `server_addr`.
    pub fn host(public_socket: UdpSocket, server_addr: SocketAddr) -> io::Result<Self> {
        Self::host_with_idle_timeout(public_socket, server_addr, PEER_IDLE_TIMEOUT)
    }

    fn host_with_idle_timeout(
        public_socket: UdpSocket,
        server_addr: SocketAddr,
        idle_timeout: Duration,
    ) -> io::Result<Self> {
        Self::new(
            public_socket,
            server_addr,
            (Ipv4Addr::LOCALHOST, 0).into(),
            idle_timeout,
        )
    }

    fn new(
        downstream: UdpSocket,
        target: SocketAddr,
        upstream_bind: SocketAddr,
        idle_timeout: Duration,
    ) -> io::Result<Self> {
        downstream.set_nonblocking(false)?;
        downstream.set_read_timeout(Some(READ_TIMEOUT))?;
        let shared = Arc::new(Shared::new());
        let (scheduler, arrivals) = mpsc::channel();
        let mut proxy = Self {
            local_addr: downstream.local_addr()?,
            shared: Arc::clone(&shared),
            threads: Vec::with_capacity(2),
        };
        // Pushed as each starts, so a later spawn failure still joins it.
        let scheduler_shared = Arc::clone(&shared);
        proxy.threads.push(spawn("net-lag-scheduler", move || {
            run_scheduler(&scheduler_shared, arrivals);
        })?);
        let reader = DownstreamReader {
            relay: Relay { shared, scheduler },
            socket: Arc::new(downstream),
            target,
            upstream_bind,
            idle_timeout,
            peers: HashMap::new(),
            retired: Vec::new(),
            last_sweep: Instant::now(),
            peer_limit_logged: false,
            open_failure_logged: false,
        };
        proxy
            .threads
            .push(spawn("net-lag-downstream", move || reader.run())?);
        Ok(proxy)
    }

    /// The downstream address: what a client-mode transport dials, or the
    /// host's public address.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    #[must_use]
    pub fn profile(&self) -> LagProfile {
        self.shared.link().profile
    }

    /// Change the impairment. Datagrams already in flight keep the arrival time
    /// they were given; only datagrams received afterwards use `profile`.
    pub fn set_profile(&self, profile: LagProfile) {
        self.shared.link().profile = profile;
    }

    #[cfg(test)]
    fn peer_count(&self) -> usize {
        self.shared.peers.load(Relaxed)
    }

    #[cfg(test)]
    fn in_flight(&self) -> usize {
        self.shared.in_flight.load(Relaxed)
    }
}

impl Drop for LagProxy {
    fn drop(&mut self) {
        // Readers see the flag within one read timeout; the scheduler exits
        // once the last reader has dropped its sender.
        self.shared.shutdown.store(true, Relaxed);
        for thread in self.threads.drain(..) {
            join(thread);
        }
    }
}

fn spawn(name: &str, body: impl FnOnce() + Send + 'static) -> io::Result<JoinHandle<()>> {
    thread::Builder::new().name(name.to_owned()).spawn(body)
}

fn join(thread: JoinHandle<()>) {
    if thread.join().is_err() {
        log::warn!("[Net] a lag proxy thread panicked");
    }
}

fn bind_reader(addr: SocketAddr) -> io::Result<UdpSocket> {
    let socket = UdpSocket::bind(addr)?;
    socket.set_read_timeout(Some(READ_TIMEOUT))?;
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::reveal_both;
    use crate::transport::{NetClient, NetServer};
    use log::Level;
    use postretro_test_log_capture::LogCapture;
    use std::sync::Weak;

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

    /// Poll `done` until it holds or `timeout` passes.
    fn eventually(timeout: Duration, mut done: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if done() {
                return true;
            }
            thread::sleep(MS);
        }
        done()
    }

    fn median(mut samples: Vec<Duration>) -> Duration {
        samples.sort_unstable();
        samples[samples.len() / 2]
    }

    // --- Delay queue and profile, on an injected clock ---

    #[test]
    fn delay_queue_releases_a_datagram_at_its_arrival_time_and_not_before() {
        let t0 = Instant::now();
        let mut queue = DelayQueue::new();
        assert!(queue.push(t0 + 50 * MS, 1));
        assert_eq!(queue.next_due(), Some(t0 + 50 * MS));
        assert!(queue.pop_due(t0 + 49 * MS).is_none());
        assert_eq!(queue.pop_due(t0 + 50 * MS), Some(1));
        assert!(queue.pop_due(t0 + 50 * MS).is_none());
    }

    #[test]
    fn delay_queue_releases_by_arrival_time_then_enqueue_order() {
        let t0 = Instant::now();
        let mut queue = DelayQueue::new();
        queue.push(t0 + 30 * MS, 1);
        queue.push(t0 + 10 * MS, 2);
        queue.push(t0 + 10 * MS, 3);
        let order: Vec<u8> = std::iter::from_fn(|| queue.pop_due(t0 + 30 * MS)).collect();
        assert_eq!(order, vec![2, 3, 1]);
    }

    #[test]
    fn delay_queue_drops_beyond_its_bound() {
        let t0 = Instant::now();
        let mut queue = DelayQueue::new();
        for _ in 0..MAX_QUEUED_PACKETS {
            assert!(queue.push(t0, ()));
        }
        assert!(!queue.push(t0, ()));
        assert_eq!(queue.len(), MAX_QUEUED_PACKETS);
    }

    // Regression: the full-queue warning re-armed on every accepted push, so
    // sustained saturation warned once per poll.
    #[test]
    fn delay_queue_warns_once_per_overflow_episode() {
        const FULL: &str = "lag proxy queue full";
        let capture = LogCapture::start();
        let t0 = Instant::now();
        let mut queue = DelayQueue::new();
        for _ in 0..MAX_QUEUED_PACKETS {
            queue.push(t0, ());
        }
        for _ in 0..10 {
            // Saturated: each released slot refills at once and the next push
            // is refused again.
            assert!(!queue.push(t0, ()));
            assert!(queue.pop_due(t0).is_some());
            assert!(queue.push(t0, ()));
        }
        capture.assert_logged_once(Level::Warn, FULL);

        while queue.len() >= QUEUE_LOW_WATER {
            queue.pop_due(t0);
        }
        while queue.push(t0, ()) {}
        let warnings = capture
            .records()
            .iter()
            .filter(|record| record.level == Level::Warn && record.message.contains(FULL))
            .count();
        assert_eq!(warnings, 2, "a second episode after draining warns again");
    }

    #[test]
    fn profile_off_is_exactly_the_unimpaired_profile() {
        assert!(LagProfile::OFF.is_off());
        assert_eq!(LagProfile::default(), LagProfile::OFF);
        assert!(!delayed(1).is_off());
        assert!(!lossy(0.01).is_off());
        assert!(lossy(f64::NAN).is_off());
    }

    #[test]
    fn profile_delay_and_jitter_are_capped_at_the_ceiling() {
        let link = LagProfile {
            delay_ms: u32::MAX,
            jitter_ms: u32::MAX,
            loss_probability: 2.0,
        }
        .link_config();
        assert_eq!(link.delay, u64::from(LagProfile::MAX_MS));
        assert_eq!(link.jitter, u64::from(LagProfile::MAX_MS));
        assert_eq!(link.loss_probability, 1.0);
    }

    #[test]
    fn client_proxy_refuses_an_ipv6_host_as_connect_does() {
        let host: SocketAddr = "[::1]:27015".parse().unwrap();
        let err = LagProxy::client(host).err().expect("IPv6 refused");
        assert_eq!(err.kind(), io::ErrorKind::Unsupported);
    }

    // --- Proxy over real sockets, on the wall clock, never polled ---

    /// Replies to every datagram with the same bytes, from its own thread.
    struct Echo {
        addr: SocketAddr,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl Echo {
        fn start() -> Self {
            let (socket, addr) = loopback_socket();
            socket.set_read_timeout(Some(5 * MS)).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let thread = {
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    let mut buffer = [0; MAX_DATAGRAM_BYTES];
                    while !stop.load(Relaxed) {
                        if let Ok((len, from)) = socket.recv_from(&mut buffer) {
                            let _ = socket.send_to(&buffer[..len], from);
                        }
                    }
                })
            };
            Self {
                addr,
                stop,
                thread: Some(thread),
            }
        }
    }

    impl Drop for Echo {
        fn drop(&mut self) {
            self.stop.store(true, Relaxed);
            if let Some(thread) = self.thread.take() {
                thread.join().expect("echo thread");
            }
        }
    }

    /// Median wall time for `rounds` sequential datagrams to come back.
    fn median_echo(socket: &UdpSocket, to: SocketAddr, rounds: u8) -> Duration {
        let samples = (0..rounds)
            .map(|round| {
                let start = Instant::now();
                socket.send_to(&[round], to).unwrap();
                loop {
                    let (bytes, _) =
                        recv_within(socket, Duration::from_secs(1)).expect("the echo returns");
                    if bytes == [round] {
                        return start.elapsed();
                    }
                }
            })
            .collect();
        median(samples)
    }

    #[test]
    fn unimpaired_proxies_add_no_measurable_latency_to_an_echo() {
        // Generous: the frame-polled relay this replaced added up to a frame
        // (~16 ms) per hop off Linux; a threaded relay adds microseconds.
        const SLACK: Duration = Duration::from_millis(5);
        let echo = Echo::start();
        let host = LagProxy::host(loopback_socket().0, echo.addr).expect("host proxy");
        let client = LagProxy::client(host.local_addr()).expect("client proxy");
        let (socket, _) = loopback_socket();

        // Warm up: the first datagram through each proxy opens its peer.
        median_echo(&socket, client.local_addr(), 5);
        median_echo(&socket, echo.addr, 5);
        let direct = median_echo(&socket, echo.addr, 50);
        let proxied = median_echo(&socket, client.local_addr(), 50);
        eprintln!("echo median: direct {direct:?}, through two unimpaired proxies {proxied:?}");
        assert!(
            proxied < direct + SLACK,
            "proxied {proxied:?} vs direct {direct:?}"
        );
    }

    #[test]
    fn a_configured_delay_is_honoured_in_each_direction_without_polling() {
        // Generous upper slack for a loaded test machine; the lower bound is exact.
        const SLACK: Duration = Duration::from_millis(15);
        const DELAY: Duration = Duration::from_millis(50);
        let (host, host_addr) = loopback_socket();
        let (client, _) = loopback_socket();
        let proxy = LagProxy::client(host_addr).expect("client proxy");
        proxy.set_profile(delayed(50));

        let mut up = Vec::new();
        let mut down = Vec::new();
        let mut upstream = None;
        for round in 0..5_u8 {
            let start = Instant::now();
            client.send_to(&[round], proxy.local_addr()).unwrap();
            let (bytes, from) = recv_within(&host, Duration::from_secs(1)).expect("released");
            up.push(start.elapsed());
            assert_eq!(bytes, [round]);
            assert_ne!(
                from,
                proxy.local_addr(),
                "leaves through its upstream socket"
            );
            upstream = Some(from);

            let start = Instant::now();
            host.send_to(&[round], from).unwrap();
            let (bytes, from) = recv_within(&client, Duration::from_secs(1)).expect("released");
            down.push(start.elapsed());
            assert_eq!(bytes, [round]);
            assert_eq!(
                from,
                proxy.local_addr(),
                "the reply comes from the dialed address"
            );
        }
        assert_eq!(proxy.peer_count(), 1);
        assert!(upstream.is_some());
        eprintln!("50 ms delay measured: up {up:?}, down {down:?}");
        for sample in up.iter().chain(&down) {
            assert!(*sample >= DELAY, "released early: {sample:?}");
        }
        assert!(median(up) < DELAY + SLACK);
        assert!(median(down) < DELAY + SLACK);
    }

    #[test]
    fn a_profile_change_applies_only_to_datagrams_received_after_it() {
        let (host, host_addr) = loopback_socket();
        let (client, _) = loopback_socket();
        let proxy = LagProxy::client(host_addr).expect("client proxy");
        proxy.set_profile(delayed(300));

        let sent = Instant::now();
        client.send_to(b"before", proxy.local_addr()).unwrap();
        assert!(eventually(Duration::from_secs(1), || proxy.in_flight() == 1));
        proxy.set_profile(LagProfile::OFF);
        client.send_to(b"after", proxy.local_addr()).unwrap();

        let (first, _) = recv_within(&host, Duration::from_secs(1)).expect("released");
        assert_eq!(first, b"after", "the later datagram overtakes");
        assert!(sent.elapsed() < Duration::from_millis(300));
        let (second, _) = recv_within(&host, Duration::from_secs(1)).expect("released");
        assert_eq!(second, b"before");
        assert!(
            sent.elapsed() >= Duration::from_millis(300),
            "the earlier datagram keeps its delay"
        );
    }

    #[test]
    fn loss_zero_relays_every_datagram_and_loss_one_relays_none() {
        const SENT: usize = 32;
        let (host, host_addr) = loopback_socket();
        let (client, _) = loopback_socket();
        let proxy = LagProxy::client(host_addr).expect("client proxy");
        let relay = |profile: LagProfile, wait: Duration| {
            proxy.set_profile(profile);
            for _ in 0..SENT {
                client.send_to(b"x", proxy.local_addr()).unwrap();
            }
            let mut received = 0;
            while received < SENT && recv_within(&host, wait).is_some() {
                received += 1;
            }
            received
        };
        assert_eq!(relay(lossy(0.0), Duration::from_secs(1)), SENT);
        assert_eq!(relay(lossy(1.0), 100 * MS), 0);
        assert_eq!(proxy.in_flight(), 0, "a lost datagram is never scheduled");
    }

    #[test]
    fn an_oversized_datagram_does_not_stall_the_reader() {
        let (host, host_addr) = loopback_socket();
        let (client, _) = loopback_socket();
        let proxy = LagProxy::client(host_addr).expect("client proxy");
        client
            .send_to(&[0; MAX_DATAGRAM_BYTES + 1000], proxy.local_addr())
            .unwrap();
        client.send_to(b"next", proxy.local_addr()).unwrap();
        let mut relayed = Vec::new();
        while let Some((bytes, _)) = recv_within(&host, Duration::from_secs(1)) {
            let done = bytes == b"next";
            relayed.push(bytes);
            if done {
                break;
            }
        }
        assert_eq!(relayed.last().map(Vec::as_slice), Some(&b"next"[..]));
        assert!(
            relayed
                .iter()
                .all(|bytes| bytes.len() <= MAX_DATAGRAM_BYTES)
        );
    }

    #[test]
    fn each_peer_gets_its_own_upstream_address_until_it_goes_idle() {
        const IDLE: Duration = Duration::from_millis(200);
        let (host, host_addr) = loopback_socket();
        let (first, _) = loopback_socket();
        let (second, _) = loopback_socket();
        let proxy = LagProxy::host_with_idle_timeout(loopback_socket().0, host_addr, IDLE)
            .expect("host proxy");

        first.send_to(b"1", proxy.local_addr()).unwrap();
        second.send_to(b"2", proxy.local_addr()).unwrap();
        let mut sources = HashMap::new();
        while sources.len() < 2 {
            let (bytes, from) = recv_within(&host, Duration::from_secs(1)).expect("relayed");
            sources.insert(bytes, from);
        }
        assert_eq!(proxy.peer_count(), 2);
        assert_ne!(sources[&b"1".to_vec()], sources[&b"2".to_vec()]);

        // A reply to each upstream address returns to its own peer only.
        host.send_to(b"to-2", sources[&b"2".to_vec()]).unwrap();
        let reply = recv_within(&second, Duration::from_secs(1));
        assert_eq!(reply.map(|(bytes, _)| bytes), Some(b"to-2".to_vec()));
        assert!(recv_within(&first, 20 * MS).is_none());

        assert!(
            eventually(Duration::from_secs(2), || proxy.peer_count() == 0),
            "silent peers are closed"
        );
    }

    #[test]
    fn dropping_the_proxy_joins_its_threads_and_closes_its_sockets() {
        let (host, host_addr) = loopback_socket();
        let (client, _) = loopback_socket();
        let proxy = LagProxy::client(host_addr).expect("client proxy");
        // Held in flight, so the scheduler has work pending at drop.
        proxy.set_profile(delayed(LagProfile::MAX_MS));
        client.send_to(b"x", proxy.local_addr()).unwrap();
        assert!(eventually(Duration::from_secs(1), || proxy.in_flight() == 1));
        proxy.set_profile(LagProfile::OFF);
        client.send_to(b"y", proxy.local_addr()).unwrap();
        let (_, upstream) = recv_within(&host, Duration::from_secs(1)).expect("relayed");

        let downstream = proxy.local_addr();
        let shared: Weak<Shared> = Arc::downgrade(&proxy.shared);
        let started = Instant::now();
        drop(proxy);
        eprintln!("proxy drop took {:?}", started.elapsed());
        assert!(
            shared.upgrade().is_none(),
            "every proxy thread has exited and released its state"
        );
        UdpSocket::bind(downstream).expect("the downstream socket is closed");
        UdpSocket::bind(upstream).expect("the upstream socket is closed");
    }

    // --- Real endpoints through the proxy ---

    /// Feeds the endpoints real elapsed time, so netcode's timeouts and the
    /// proxy's wall-clock delays agree.
    struct Pacer(Instant);

    impl Pacer {
        fn new() -> Self {
            Self(Instant::now())
        }

        fn tick(&mut self) -> Duration {
            let now = Instant::now();
            let dt = now - self.0;
            self.0 = now;
            dt
        }
    }

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

    fn proxy_of_client(client: &NetClient) -> &LagProxy {
        client.lag_proxy().expect("proxied client")
    }

    fn proxy_of_server(server: &NetServer) -> &LagProxy {
        server.lag_proxy().expect("proxied server")
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

    fn step(server: &mut NetServer, clients: &mut [&mut NetClient], pacer: &mut Pacer) {
        thread::sleep(MS);
        let dt = pacer.tick();
        for client in clients.iter_mut() {
            client.update(dt).expect("client update");
            // Draining Control activates the participation epoch.
            let _ = client.drain_control();
        }
        let _ = server.update(dt).expect("server update");
    }

    fn step_until(
        server: &mut NetServer,
        clients: &mut [&mut NetClient],
        pacer: &mut Pacer,
        timeout: Duration,
        mut done: impl FnMut(&mut NetServer, &mut [&mut NetClient]) -> bool,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            step(server, clients, pacer);
            if done(server, clients) {
                return true;
            }
        }
        false
    }

    fn participate(
        server: &mut NetServer,
        client: &mut NetClient,
        client_id: u64,
        pacer: &mut Pacer,
    ) {
        let participating = step_until(
            server,
            &mut [client],
            pacer,
            Duration::from_secs(5),
            |server, clients| clients[0].is_participating() && server.is_participating(client_id),
        );
        assert!(
            participating,
            "client {client_id} participates through the proxy"
        );
    }

    /// Wall time for an Input to reach the host and its reply to come back.
    fn echo_round_trip(
        server: &mut NetServer,
        client: &mut NetClient,
        client_id: u64,
        pacer: &mut Pacer,
    ) -> Duration {
        let start = Instant::now();
        client.send_input(b"ping".to_vec());
        let mut echoed = false;
        let returned = step_until(
            server,
            &mut [client],
            pacer,
            Duration::from_secs(5),
            |server, clients| {
                if !echoed && server.drain_input(client_id).contains(&b"ping".to_vec()) {
                    server.send_input(client_id, b"pong".to_vec());
                    echoed = true;
                }
                clients[0].drain_input().contains(&b"pong".to_vec())
            },
        );
        assert!(returned, "the echo returns");
        start.elapsed()
    }

    #[test]
    fn unimpaired_proxies_on_both_ends_connect_and_carry_traffic() {
        let mut pacer = Pacer::new();
        let (mut server, server_addr) = proxied_server();
        let mut client = proxied_client(server_addr, 7);
        assert_eq!(proxy_of_server(&server).profile(), LagProfile::OFF);
        assert_eq!(proxy_of_client(&client).profile(), LagProfile::OFF);
        assert_eq!(
            server.local_addr(),
            vec![server_addr],
            "reports the public address"
        );
        matching(&mut server, &mut client);
        participate(&mut server, &mut client, 7, &mut pacer);
        echo_round_trip(&mut server, &mut client, 7, &mut pacer);

        let proxies: [Weak<Shared>; 2] = [
            Arc::downgrade(&proxy_of_server(&server).shared),
            Arc::downgrade(&proxy_of_client(&client).shared),
        ];
        drop((server, client));
        assert!(
            proxies.iter().all(|shared| shared.upgrade().is_none()),
            "dropping an endpoint joins its proxy's threads"
        );
    }

    #[test]
    fn a_configured_delay_lengthens_the_round_trip_by_both_directions() {
        // Generous bound: the delayed trip cannot beat it, and an unimpaired
        // loopback trip of a few polls stays far under it.
        const BOUND: Duration = Duration::from_millis(300);
        let mut pacer = Pacer::new();
        let (mut server, server_addr) = proxied_server();
        let mut client = proxied_client(server_addr, 7);
        matching(&mut server, &mut client);
        participate(&mut server, &mut client, 7, &mut pacer);

        let unimpaired = echo_round_trip(&mut server, &mut client, 7, &mut pacer);
        assert!(unimpaired < BOUND, "unimpaired round trip {unimpaired:?}");

        // Each proxy delays both directions: 2 x 100 + 2 x 50 = 300 ms.
        proxy_of_client(&client).set_profile(delayed(100));
        proxy_of_server(&server).set_profile(delayed(50));
        let impaired = echo_round_trip(&mut server, &mut client, 7, &mut pacer);
        assert!(impaired >= BOUND, "impaired round trip {impaired:?}");
        assert!(
            client.is_connected(),
            "a mid-session change keeps the connection"
        );
    }

    #[test]
    fn a_step_to_heavy_lag_and_back_to_off_keeps_the_session() {
        let mut pacer = Pacer::new();
        let (mut server, server_addr) = proxied_server();
        let mut client = proxied_client(server_addr, 7);
        matching(&mut server, &mut client);
        participate(&mut server, &mut client, 7, &mut pacer);

        // At least 2 x 500 ms each way, up to 2 x 750 ms.
        proxy_of_client(&client).set_profile(LagProfile {
            delay_ms: 500,
            jitter_ms: 250,
            loss_probability: 0.0,
        });
        let lagged = echo_round_trip(&mut server, &mut client, 7, &mut pacer);
        assert!(lagged >= Duration::from_millis(1000), "lagged {lagged:?}");

        // Keep-alives and acks are still in flight when the delay drops.
        proxy_of_client(&client).set_profile(LagProfile::OFF);
        let recovered = echo_round_trip(&mut server, &mut client, 7, &mut pacer);
        eprintln!("round trip at 500/250: {lagged:?}; right after Off: {recovered:?}");
        assert!(
            recovered < Duration::from_millis(500),
            "recovered {recovered:?}"
        );
        assert!(client.is_connected() && client.is_participating());
        assert!(server.is_participating(7));
    }

    #[test]
    fn total_loss_delivers_nothing_until_the_profile_clears() {
        let mut pacer = Pacer::new();
        let (mut server, server_addr) = proxied_server();
        let mut client = proxied_client(server_addr, 7);
        proxy_of_client(&client).set_profile(lossy(1.0));
        let reached = step_until(
            &mut server,
            &mut [&mut client],
            &mut pacer,
            Duration::from_millis(500),
            |server, _| server.has_connected_clients(),
        );
        assert!(!reached, "no connect request reaches the host");
        assert!(!client.is_connected());

        proxy_of_client(&client).set_profile(lossy(0.0));
        let connected = step_until(
            &mut server,
            &mut [&mut client],
            &mut pacer,
            Duration::from_secs(5),
            |server, clients| clients[0].is_connected() && server.has_connected_clients(),
        );
        assert!(connected, "the next request after the change connects");
    }

    #[test]
    fn two_clients_behind_a_host_proxy_keep_distinct_links() {
        let mut pacer = Pacer::new();
        let (mut server, server_addr) = proxied_server();
        let mut first = plain_client(server_addr, 7);
        let mut second = plain_client(server_addr, 8);
        matching(&mut server, &mut first);
        matching(&mut server, &mut second);
        let connected = step_until(
            &mut server,
            &mut [&mut first, &mut second],
            &mut pacer,
            Duration::from_secs(5),
            |s, c| c.iter().all(|client| client.is_connected()) && s.connected_clients().len() == 2,
        );
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
            &mut pacer,
            Duration::from_secs(5),
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
        let mut pacer = Pacer::new();
        let connected = step_until(
            &mut server,
            &mut [&mut client],
            &mut pacer,
            Duration::from_secs(5),
            |_, clients| clients[0].is_connected(),
        );
        assert!(connected, "the retry connects");
    }
}
