// Client clock-sync exchange and the bounded server-tick / jitter estimator.
// See: context/lib/networking.md
//
// Two wire messages and one pure estimator. The client periodically sends a
// `TimeSyncRequest` stamped with its local monotonic send tick + microseconds;
// the server echoes it as a `TimeSyncEcho`, sampling the current `server_tick`
// at echo time. The client records its local receive microseconds and feeds the
// completed round trip to `ClockEstimator`, which maintains a smoothed estimate
// of the server tick (offset), RTT and link jitter for the interpolation code
// (Task 6) to consume. Only near-minimum-RTT round trips fold into the estimate
// (see `ClockEstimator`'s min-RTT gate): probes ride a reliable-ordered channel
// whose head-of-line stalls inflate one direction only.
//
// Registry-blind by construction: this module sees only `u32`/`u64`/`f64`
// scalars and the harness virtual clock — no `EntityId`, no `glam`, no engine
// types. RTT is always computed from the *client's own* monotonic send/receive
// microseconds; the server's echoed microseconds are telemetry only and are
// never compared against client microseconds (the two clocks have unrelated
// origins). The estimator reads time only through an injected `MonotonicClock`,
// so tests and the harness drive it on a virtual clock and it never touches
// wall-clock.

use bitcode::{Decode, Encode};

/// Client -> server time-sync probe, carried on the reliable-ordered
/// `Channel::Input` (appended to the `ClientMessage` envelope). Stamps the
/// client's local monotonic send tick and microseconds so the round trip can be
/// measured entirely against the client's own clock when the echo returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct TimeSyncRequest {
    /// Monotonic per-client probe id. The estimator drops a stale echo by
    /// comparing this against the newest sample it has accepted.
    pub sample_id: u32,
    /// The client's local sim tick at send time (carried back in the echo for
    /// diagnostics; the offset math uses the receive-midpoint microseconds).
    pub client_send_tick: u32,
    /// The client's local monotonic microseconds at send time. Used only against
    /// the client's own receive microseconds to compute RTT — never compared to
    /// the server's clock.
    pub client_send_time_us: u64,
}

/// Server -> client echo of a `TimeSyncRequest`, carried on `Channel::Input`.
/// Mirrors the request fields back so the client can match the echo to its
/// in-flight sample, and adds the server's current tick sampled at echo time.
///
/// `server_echo_time_us` is **telemetry only**: it is the server's local
/// monotonic microseconds at echo, kept for same-process diagnostics/tests. The
/// offset estimate never reads it — server and client microseconds have
/// unrelated origins, so comparing them directly would be meaningless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
pub struct TimeSyncEcho {
    pub sample_id: u32,
    pub client_send_tick: u32,
    pub client_send_time_us: u64,
    /// The server's current sim tick, sampled at the instant the echo is built.
    pub server_tick: u32,
    /// Server-local monotonic microseconds at echo. Telemetry only — asserted
    /// only in same-process tests, never used in the offset math.
    pub server_echo_time_us: u64,
}

impl TimeSyncRequest {
    /// Build the server's echo of this request, stamping the server's current
    /// tick and (telemetry-only) local microseconds.
    #[must_use]
    pub fn echo(&self, server_tick: u32, server_echo_time_us: u64) -> TimeSyncEcho {
        TimeSyncEcho {
            sample_id: self.sample_id,
            client_send_tick: self.client_send_tick,
            client_send_time_us: self.client_send_time_us,
            server_tick,
            server_echo_time_us,
        }
    }
}

/// Injectable monotonic clock the estimator reads time through. Production wraps
/// the engine's `Instant`-based frame clock; tests and the in-memory harness
/// implement it over a virtual `u64` microsecond counter the caller advances.
///
/// Contract: `now_micros` is monotonic non-decreasing within one client's
/// lifetime. The estimator never assumes any particular origin — only that the
/// difference between two reads is a real elapsed-microseconds duration.
pub trait MonotonicClock {
    /// Current monotonic time in microseconds.
    fn now_micros(&self) -> u64;
}

/// Microseconds per sim tick at the engine's 60 Hz fixed timestep (16_667 us).
/// The estimator is handed this at construction so the net crate stays free of
/// the engine's tick-rate constant; the engine passes its own `TICK_DURATION`.
pub const DEFAULT_MICROS_PER_TICK: u64 = 16_667;

/// Sample cadence: probes are sent at 5 Hz (one every 200 ms). Exposed so the
/// engine send loop and the tests share the one cadence constant.
pub const SAMPLE_PERIOD_US: u64 = 200_000;

/// Exponential smoothing weight for the offset and RTT estimates. A new sample
/// contributes 10%; the running estimate retains 90%. Low weight trades
/// responsiveness for stability against per-packet jitter.
pub const OFFSET_SMOOTHING: f64 = 0.1;

/// Exponential smoothing weight for the jitter estimate. Slightly higher (0.2)
/// than the offset weight so the jitter band reacts faster to a changing link
/// while the offset itself stays stable.
pub const JITTER_SMOOTHING: f64 = 0.2;

/// Size of the min-RTT gate's window of recent raw RTT samples: 16 probes, about
/// 3.2 s at the 5 Hz cadence. Every non-stale echo enters it; the window minimum
/// is the gate's reference. Because it rolls, a sustained latency increase leaves
/// the window within this many samples and is then adopted.
pub const RTT_WINDOW_SAMPLES: usize = 16;

/// Fixed floor of the min-RTT gate's tolerance, in microseconds (20 ms). A
/// sample folds only if its RTT is at most `window_min + tolerance`. The floor
/// keeps a near-zero-RTT (LAN) link from gating out ordinary scheduling noise.
pub const RTT_GATE_FLOOR_US: f64 = 20_000.0;

/// Proportional part of the min-RTT gate's tolerance: `tolerance =
/// max(RTT_GATE_FLOOR_US, RTT_GATE_FRACTION × window_min)`. At 0.5 a 150 ms link
/// admits samples up to 225 ms, while a probe held behind an outage on the
/// reliable-ordered channel (hundreds of ms added to one direction) lands far
/// outside.
pub const RTT_GATE_FRACTION: f64 = 0.5;

/// A completed round trip, ready to fold into the estimate. Built by
/// [`ClockEstimator::ingest_echo`] from the echo plus the client-local receive
/// microseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Sample {
    /// RTT in microseconds, from the client's own send/receive times.
    rtt_us: f64,
    /// The server tick the echo carried.
    server_tick: f64,
    /// The client-local microsecond midpoint of the round trip — the moment the
    /// echoed `server_tick` is estimated to have been valid on the client clock.
    recv_midpoint_us: f64,
}

/// Pure clock/jitter estimator. Holds the smoothed mapping from client-local
/// monotonic microseconds to server ticks (an `offset_ticks`) plus smoothed RTT
/// and jitter, advancing only on the newest accepted sample.
///
/// Min-RTT gate: probes and echoes ride the reliable-ordered `Channel::Input`, so
/// a probe queued behind a lost packet waits out a resend in *one* direction. Its
/// RTT inflates and the midpoint assumption breaks, biasing the offset by half
/// the stall. Only near-minimum-RTT samples carry an unbiased midpoint, so every
/// non-stale echo enters a [`RTT_WINDOW_SAMPLES`] window but folds into
/// offset/RTT/jitter only when its RTT is at most the window minimum plus
/// `max(RTT_GATE_FLOOR_US, RTT_GATE_FRACTION × min)`. Gated samples touch nothing
/// else. Snapshots ride an unreliable channel and see no such stalls, so the
/// filtered jitter is the right input for the remote interpolation delay. The
/// first sample still seeds. A later sample that sets the window minimum more
/// than one gate tolerance below the smoothed RTT reseeds offset and RTT at
/// weight 1 and zeroes jitter, so a stalled first probe does not linger.
///
/// Under a fast RTT ramp the gate holds the estimate at the old minimum until
/// the ramp flattens and that minimum rolls out of the window; the interpolation
/// delay's measured snapshot-lag law covers remotes meanwhile.
///
/// Stale-echo policy: each accepted sample must carry a `sample_id` strictly
/// greater than the last accepted one. A reordered or duplicated echo (older or
/// equal id) is ignored, so only the newest in-flight probe advances state.
///
/// Provenance policy: an echo is also rejected if its `sample_id` exceeds the
/// highest id actually issued by the local [`TimeSyncSender`]. Call
/// [`ClockEstimator::record_sent`] each time a probe is emitted; `ingest_echo`
/// rejects any echo whose id was never sent, closing the wedge-by-forged-id
/// attack vector.
pub struct ClockEstimator {
    micros_per_tick: f64,
    /// `false` until the first sample is folded in; the first sample seeds the
    /// estimates directly rather than smoothing toward a meaningless zero.
    initialized: bool,
    /// Highest `sample_id` accepted so far. A sample id `<=` this is stale.
    last_sample_id: Option<u32>,
    /// Highest `sample_id` issued by the local sender. An echo with a
    /// `sample_id` greater than this was never sent and is rejected.
    last_issued_id: Option<u32>,
    /// Smoothed server-tick offset: `estimated_server_tick(t) =
    /// client_ticks(t) + offset_ticks`, where `client_ticks(t) = t_us /
    /// micros_per_tick`.
    offset_ticks: f64,
    /// Smoothed RTT in microseconds.
    rtt_us: f64,
    /// Smoothed jitter in microseconds: EWMA of the absolute deviation of each
    /// sample's RTT from the running RTT (mean-absolute-deviation style).
    jitter_us: f64,
    /// Recent raw RTTs of every non-stale echo, the min-RTT gate's reference.
    rtt_window: RttWindow,
}

/// Fixed-capacity ring of the most recent raw RTT samples, in microseconds. No
/// allocation: [`RTT_WINDOW_SAMPLES`] slots, overwritten oldest-first.
#[derive(Debug, Clone, Copy)]
struct RttWindow {
    samples: [f64; RTT_WINDOW_SAMPLES],
    /// Filled slots, saturating at the capacity.
    len: usize,
    /// Slot the next sample overwrites.
    next: usize,
}

impl RttWindow {
    const fn new() -> Self {
        Self {
            samples: [0.0; RTT_WINDOW_SAMPLES],
            len: 0,
            next: 0,
        }
    }

    fn push(&mut self, rtt_us: f64) {
        self.samples[self.next] = rtt_us;
        self.next = (self.next + 1) % RTT_WINDOW_SAMPLES;
        self.len = (self.len + 1).min(RTT_WINDOW_SAMPLES);
    }

    /// Smallest RTT in the window; `None` while empty.
    fn min(&self) -> Option<f64> {
        self.samples[..self.len].iter().copied().reduce(f64::min)
    }

    /// Does `rtt_us` sit within the gate tolerance of the window minimum? An
    /// empty window admits everything.
    fn admits(&self, rtt_us: f64) -> bool {
        self.min()
            .is_none_or(|min| rtt_us <= min + gate_tolerance_us(min))
    }
}

/// The min-RTT gate's tolerance above a window minimum of `min_us`.
fn gate_tolerance_us(min_us: f64) -> f64 {
    (RTT_GATE_FRACTION * min_us).max(RTT_GATE_FLOOR_US)
}

impl ClockEstimator {
    /// Build an estimator that converts microseconds to ticks at
    /// `micros_per_tick` (the engine passes [`DEFAULT_MICROS_PER_TICK`]).
    ///
    /// # Panics
    /// Debug-asserts `micros_per_tick > 0`; a zero tick length would divide by
    /// zero in the microseconds->ticks conversion.
    #[must_use]
    pub fn new(micros_per_tick: u64) -> Self {
        debug_assert!(micros_per_tick > 0, "micros_per_tick must be positive");
        Self {
            micros_per_tick: micros_per_tick as f64,
            initialized: false,
            last_sample_id: None,
            last_issued_id: None,
            offset_ticks: 0.0,
            rtt_us: 0.0,
            jitter_us: 0.0,
            rtt_window: RttWindow::new(),
        }
    }

    /// Has the estimator folded in at least one sample? Before this the
    /// `estimated_server_tick` is a pure passthrough of local ticks (offset 0).
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Record that a probe with `sample_id` was just emitted. Must be called once
    /// per [`TimeSyncSender::maybe_send`] result (passing the returned
    /// `sample_id`). `ingest_echo` rejects any echo whose `sample_id` exceeds
    /// the highest id recorded here.
    pub fn record_sent(&mut self, sample_id: u32) {
        self.last_issued_id = Some(match self.last_issued_id {
            None => sample_id,
            Some(prev) => prev.max(sample_id),
        });
    }

    /// Fold one echo into the estimate. `recv_time_us` is the client's local
    /// monotonic microseconds when the echo arrived — RTT is computed purely
    /// from it and the echoed `client_send_time_us`, never from the server's
    /// clock. Returns `true` if the echo was accepted, `false` if it was a
    /// stale/duplicate or unsent echo. An accepted echo always enters the min-RTT
    /// window but folds into offset/RTT/jitter only if the gate admits it (see
    /// [`ClockEstimator`]); a gated-out echo still returns `true`.
    ///
    /// Provenance check: rejects an echo whose `sample_id` was never issued by
    /// the local sender (greater than the highest id passed to
    /// [`ClockEstimator::record_sent`]). A forged high id cannot wedge the
    /// estimator; a subsequent legitimate echo still advances state.
    ///
    /// A non-monotonic receive time (echo `client_send_time_us` greater than
    /// `recv_time_us`, e.g. a corrupted or replayed packet) yields a clamped
    /// zero RTT rather than an underflow — never a panic.
    pub fn ingest_echo(&mut self, echo: &TimeSyncEcho, recv_time_us: u64) -> bool {
        // Provenance guard: reject an echo we could not have sent.
        match self.last_issued_id {
            None => return false, // no probe sent yet
            Some(max_issued) if echo.sample_id > max_issued => return false,
            _ => {}
        }

        // Stale-echo guard: only a strictly-newer sample advances state. A
        // reordered or duplicated echo is dropped here.
        if let Some(last) = self.last_sample_id
            && echo.sample_id <= last
        {
            return false;
        }
        self.last_sample_id = Some(echo.sample_id);

        // RTT from client-local times only. saturating_sub guards a corrupt
        // echo whose send time is after the recorded receive time.
        let rtt_us = recv_time_us.saturating_sub(echo.client_send_time_us) as f64;
        // The echoed server_tick is estimated valid at the round-trip midpoint
        // on the client's own clock: send + RTT/2.
        let recv_midpoint_us = echo.client_send_time_us as f64 + rtt_us / 2.0;

        let sample = Sample {
            rtt_us,
            server_tick: f64::from(echo.server_tick),
            recv_midpoint_us,
        };
        // Min-RTT gate: the sample joins the window first, so a new minimum
        // always folds. The first sample seeds regardless (it is the minimum).
        self.rtt_window.push(rtt_us);
        if !self.initialized || self.reseeds_from(rtt_us) {
            self.seed(sample);
        } else if self.rtt_window.admits(rtt_us) {
            self.fold(sample);
        }
        true
    }

    /// Does a sample at `rtt_us` reseed the estimate? It must be the window
    /// minimum and sit more than one gate tolerance below the smoothed RTT. A
    /// stalled first probe seeds an inflated RTT and a midpoint biased by half the
    /// stall, which the 0.1 EWMA would take seconds to drain; the first clean
    /// sample replaces it outright.
    fn reseeds_from(&self, rtt_us: f64) -> bool {
        self.rtt_window.min() == Some(rtt_us) && rtt_us + gate_tolerance_us(rtt_us) < self.rtt_us
    }

    /// The offset that makes the sample's local-clock midpoint map exactly to its
    /// echoed server tick: `server_tick - midpoint_us / micros_per_tick`.
    fn sample_offset(&self, sample: Sample) -> f64 {
        sample.server_tick - sample.recv_midpoint_us / self.micros_per_tick
    }

    /// Adopt `sample` outright (weight 1): the first sample, or a reseed.
    fn seed(&mut self, sample: Sample) {
        self.initialized = true;
        self.offset_ticks = self.sample_offset(sample);
        self.rtt_us = sample.rtt_us;
        self.jitter_us = 0.0;
    }

    /// Smoothing core: EWMA-blend offset/RTT at [`OFFSET_SMOOTHING`] and the
    /// jitter at [`JITTER_SMOOTHING`].
    fn fold(&mut self, sample: Sample) {
        let sample_offset = self.sample_offset(sample);

        // Jitter first, against the *current* RTT estimate (before it absorbs
        // this sample), so the deviation reflects this sample vs. the prior mean.
        let deviation = (sample.rtt_us - self.rtt_us).abs();
        self.jitter_us = lerp(self.jitter_us, deviation, JITTER_SMOOTHING);

        self.offset_ticks = lerp(self.offset_ticks, sample_offset, OFFSET_SMOOTHING);
        self.rtt_us = lerp(self.rtt_us, sample.rtt_us, OFFSET_SMOOTHING);
    }

    /// Estimated server tick at client-local monotonic microseconds `now_us`:
    /// `now_us / micros_per_tick + offset_ticks`. Before any sample is folded,
    /// the offset is 0 (a pure local-tick passthrough).
    #[must_use]
    pub fn estimated_server_tick_at(&self, now_us: u64) -> f64 {
        now_us as f64 / self.micros_per_tick + self.offset_ticks
    }

    /// Estimated server tick at the clock's current time. The production path
    /// reads the engine monotonic clock; tests/harness read the virtual clock.
    #[must_use]
    pub fn estimated_server_tick(&self, clock: &impl MonotonicClock) -> f64 {
        self.estimated_server_tick_at(clock.now_micros())
    }

    /// Smoothed RTT in microseconds.
    #[must_use]
    pub fn rtt_micros(&self) -> f64 {
        self.rtt_us
    }

    /// Smoothed jitter estimate in microseconds — the interpolation buffer
    /// consumer sizes its delay against this.
    #[must_use]
    pub fn jitter_micros(&self) -> f64 {
        self.jitter_us
    }
}

/// Linear interpolation toward `target` by `weight` in `[0, 1]`. Pulled out so
/// the smoothing law is one definition shared by offset, RTT, and jitter.
fn lerp(current: f64, target: f64, weight: f64) -> f64 {
    current + (target - current) * weight
}

/// Drives the client's 5 Hz probe cadence and the monotonic `sample_id`. Pure:
/// it reads the injected clock and emits a `TimeSyncRequest` when a full
/// [`SAMPLE_PERIOD_US`] has elapsed since the last send, so the engine send loop
/// is just "call `maybe_send`, transmit any returned request".
pub struct TimeSyncSender {
    next_sample_id: u32,
    /// Client-local microseconds of the last emitted probe; `None` until the
    /// first send, which fires immediately so sync starts without a 200 ms wait.
    last_send_us: Option<u64>,
}

impl Default for TimeSyncSender {
    fn default() -> Self {
        Self::new()
    }
}

impl TimeSyncSender {
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_sample_id: 0,
            last_send_us: None,
        }
    }

    /// Emit a probe if the 5 Hz cadence is due, stamping it with the client's
    /// current local tick and monotonic microseconds. Returns `None` when no
    /// probe is due this call. The first call always fires.
    pub fn maybe_send(
        &mut self,
        clock: &impl MonotonicClock,
        client_tick: u32,
    ) -> Option<TimeSyncRequest> {
        let now_us = clock.now_micros();
        let due = match self.last_send_us {
            None => true,
            Some(last) => now_us.saturating_sub(last) >= SAMPLE_PERIOD_US,
        };
        if !due {
            return None;
        }
        self.last_send_us = Some(now_us);
        let sample_id = self.next_sample_id;
        self.next_sample_id = self.next_sample_id.wrapping_add(1);
        Some(TimeSyncRequest {
            sample_id,
            client_send_tick: client_tick,
            client_send_time_us: now_us,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{decode, encode};

    /// A virtual monotonic clock for tests/harness: the caller advances it; it
    /// never reads wall-clock (testing_guide "deterministic time").
    #[derive(Debug, Default)]
    struct VirtualClock {
        now_us: std::cell::Cell<u64>,
    }

    impl VirtualClock {
        fn new(start_us: u64) -> Self {
            Self {
                now_us: std::cell::Cell::new(start_us),
            }
        }
        fn advance(&self, dt_us: u64) {
            self.now_us.set(self.now_us.get() + dt_us);
        }
        fn set(&self, t_us: u64) {
            self.now_us.set(t_us);
        }
    }

    impl MonotonicClock for VirtualClock {
        fn now_micros(&self) -> u64 {
            self.now_us.get()
        }
    }

    const EPS: f64 = 1e-6;

    fn echo(sample_id: u32, send_us: u64, server_tick: u32) -> TimeSyncEcho {
        TimeSyncEcho {
            sample_id,
            client_send_tick: (send_us / DEFAULT_MICROS_PER_TICK) as u32,
            client_send_time_us: send_us,
            server_tick,
            server_echo_time_us: 999_999, // telemetry only; never used in math
        }
    }

    // --- Wire round-trip + corrupt decode ---

    #[test]
    fn time_sync_request_round_trips() {
        let req = TimeSyncRequest {
            sample_id: 7,
            client_send_tick: 123,
            client_send_time_us: 5_000_000,
        };
        let bytes = encode(&req);
        let decoded: TimeSyncRequest = decode(&bytes).expect("valid buffer decodes");
        assert_eq!(decoded, req);
    }

    #[test]
    fn time_sync_echo_round_trips() {
        let req = TimeSyncRequest {
            sample_id: 9,
            client_send_tick: 42,
            client_send_time_us: 1_234_567,
        };
        let echo = req.echo(600, 7_777_777);
        let bytes = encode(&echo);
        let decoded: TimeSyncEcho = decode(&bytes).expect("valid buffer decodes");
        assert_eq!(decoded, echo);
        // The echo mirrors the request's client-local fields back unchanged.
        assert_eq!(decoded.sample_id, req.sample_id);
        assert_eq!(decoded.client_send_time_us, req.client_send_time_us);
    }

    #[test]
    fn corrupt_time_sync_decodes_to_err_not_panic() {
        // A hostile/truncated time-sync message must be a typed Err, never a
        // panic — the peer must survive a malformed probe/echo on the wire.
        let garbage = [0xFFu8, 0x00, 0xAB, 0x12, 0x9C, 0x7D, 0x55, 0x01];
        assert!(decode::<TimeSyncRequest>(&garbage).is_err());
        assert!(decode::<TimeSyncEcho>(&garbage).is_err());
        assert!(decode::<TimeSyncRequest>(&[]).is_err());
        assert!(decode::<TimeSyncEcho>(&[]).is_err());
    }

    // --- RTT computed from client-local times only ---

    #[test]
    fn rtt_uses_client_local_send_and_receive_times_only() {
        // The echo carries a wildly different (and even smaller) server-clock
        // microsecond value; RTT must ignore it entirely and use the client's
        // own send/receive delta.
        let mut est = ClockEstimator::new(DEFAULT_MICROS_PER_TICK);
        let send_us = 1_000_000;
        let mut e = echo(0, send_us, 60);
        e.server_echo_time_us = 5; // absurd server clock, must not affect RTT
        let recv_us = send_us + 150_000; // 150 ms round trip on the client clock
        est.record_sent(0);
        assert!(est.ingest_echo(&e, recv_us));
        assert!(
            (est.rtt_micros() - 150_000.0).abs() < EPS,
            "RTT must be recv - send on the client clock, got {}",
            est.rtt_micros()
        );
    }

    #[test]
    fn corrupt_send_after_receive_clamps_rtt_to_zero_without_panic() {
        // A corrupt echo whose client_send_time_us is *after* the recorded
        // receive time must clamp to zero RTT, never underflow/panic.
        let mut est = ClockEstimator::new(DEFAULT_MICROS_PER_TICK);
        let e = echo(0, 2_000_000, 60);
        let recv_us = 1_000_000; // earlier than send — impossible but must be safe
        est.record_sent(0);
        assert!(est.ingest_echo(&e, recv_us));
        assert!((est.rtt_micros() - 0.0).abs() < EPS);
    }

    // --- Stale echoes dropped by sample_id ---

    #[test]
    fn stale_echo_is_ignored_by_sample_id() {
        let mut est = ClockEstimator::new(DEFAULT_MICROS_PER_TICK);
        // Record ids 0-6 as sent so the provenance check passes.
        for id in 0..=6 {
            est.record_sent(id);
        }
        // Newest sample id 5 accepted first.
        assert!(est.ingest_echo(&echo(5, 1_000_000, 600), 1_100_000));
        let offset_after_5 = est.estimated_server_tick_at(0);

        // An older id (3) and a duplicate (5) must both be dropped: state
        // unchanged, ingest returns false.
        assert!(!est.ingest_echo(&echo(3, 9_000_000, 999), 9_500_000));
        assert!(!est.ingest_echo(&echo(5, 9_000_000, 999), 9_500_000));
        assert!(
            (est.estimated_server_tick_at(0) - offset_after_5).abs() < EPS,
            "a stale/duplicate echo must not advance the estimate"
        );

        // A newer id (6) advances again.
        assert!(est.ingest_echo(&echo(6, 2_000_000, 700), 2_100_000));
    }

    // --- Smoothing-weight behavior ---

    #[test]
    fn first_sample_seeds_then_subsequent_samples_smooth_at_weight() {
        let mut est = ClockEstimator::new(DEFAULT_MICROS_PER_TICK);
        est.record_sent(0);
        est.record_sent(1);

        // First sample seeds offset directly. send=0, RTT=0 => midpoint 0 =>
        // offset = server_tick - 0 = 100.
        assert!(est.ingest_echo(&echo(0, 0, 100), 0));
        assert!((est.estimated_server_tick_at(0) - 100.0).abs() < EPS);
        assert!((est.rtt_micros() - 0.0).abs() < EPS);

        // Second sample again at send=0, RTT=0 (midpoint 0) but server_tick=200,
        // so the raw sample offset is 200. With OFFSET_SMOOTHING=0.1 the blended
        // offset is 100 + 0.1*(200-100) = 110.
        assert!(est.ingest_echo(&echo(1, 0, 200), 0));
        assert!(
            (est.estimated_server_tick_at(0) - 110.0).abs() < 1e-4,
            "offset must EWMA toward the new sample at weight 0.1, got {}",
            est.estimated_server_tick_at(0)
        );
    }

    #[test]
    fn jitter_grows_with_rtt_variance_at_its_weight() {
        let mut est = ClockEstimator::new(DEFAULT_MICROS_PER_TICK);
        est.record_sent(0);
        est.record_sent(1);
        // Seed RTT at 100_000 us (jitter seeded to 0).
        assert!(est.ingest_echo(&echo(0, 0, 60), 100_000));
        assert!((est.jitter_micros() - 0.0).abs() < EPS);

        // Next sample RTT = 140_000 (inside the min-RTT gate's 50 ms tolerance at
        // a 100 ms minimum): deviation from current RTT (100_000) is 40_000;
        // jitter = lerp(0, 40_000, 0.2) = 8_000.
        assert!(est.ingest_echo(&echo(1, 0, 60), 140_000));
        assert!(
            (est.jitter_micros() - 8_000.0).abs() < 1e-3,
            "jitter must EWMA the RTT deviation at weight 0.2, got {}",
            est.jitter_micros()
        );
    }

    // --- Min-RTT gate: reliable-channel head-of-line stalls ---

    /// Ticks the test server is ahead of the client clock's tick origin: the true
    /// offset the estimator must recover.
    const TRUE_OFFSET_TICKS: f64 = 3_600.0;
    const MPT: f64 = DEFAULT_MICROS_PER_TICK as f64;

    /// A round trip on a link where the server samples its tick `out_us` after the
    /// send and the echo lands `out_us + back_us` after it. Returns the echo and the
    /// client receive microseconds.
    fn round_trip(sample_id: u32, send_us: u64, out_us: u64, back_us: u64) -> (TimeSyncEcho, u64) {
        let server_tick = (TRUE_OFFSET_TICKS + (send_us + out_us) as f64 / MPT).round() as u32;
        (
            echo(sample_id, send_us, server_tick),
            send_us + out_us + back_us,
        )
    }

    /// Reference model of the pre-gate fold law: every sample EWMA-blends
    /// offset/RTT at `OFFSET_SMOOTHING` and jitter at `JITTER_SMOOTHING`. Returns
    /// `(offset_ticks, jitter_us)`.
    fn ungated_fold(trips: &[(TimeSyncEcho, u64)]) -> (f64, f64) {
        let mut state: Option<(f64, f64, f64)> = None;
        for (e, recv_us) in trips {
            let rtt = recv_us.saturating_sub(e.client_send_time_us) as f64;
            let offset =
                f64::from(e.server_tick) - (e.client_send_time_us as f64 + rtt / 2.0) / MPT;
            state = Some(match state {
                None => (offset, rtt, 0.0),
                Some((o, r, j)) => (
                    lerp(o, offset, OFFSET_SMOOTHING),
                    lerp(r, rtt, OFFSET_SMOOTHING),
                    lerp(j, (rtt - r).abs(), JITTER_SMOOTHING),
                ),
            });
        }
        let (offset, _, jitter) = state.expect("at least one trip");
        (offset, jitter)
    }

    fn feed(est: &mut ClockEstimator, trips: &[(TimeSyncEcho, u64)]) {
        for (e, recv_us) in trips {
            est.record_sent(e.sample_id);
            assert!(est.ingest_echo(e, *recv_us), "non-stale echo is accepted");
        }
    }

    // Playtest defect: under 5% loss a large share of probes waited out Input's
    // former ~300 ms reliable resend in one direction. An outbound-stalled probe
    // reports RTT +300 ms and a server tick 300 ms late, biasing the midpoint
    // offset by +150 ms (~9 ticks) and inflating jitter.
    #[test]
    fn min_rtt_gate_rejects_outbound_stalled_probes() {
        const STALL_US: u64 = 300_000;
        let mut all = Vec::new();
        let mut clean = Vec::new();
        for i in 0..60u32 {
            let send_us = u64::from(i) * SAMPLE_PERIOD_US;
            // Clean link: symmetric 150 ms RTT with ±10 ms of alternating jitter.
            let half = if i % 2 == 0 { 70_000 } else { 80_000 };
            let trip = if i % 3 == 2 {
                // Outbound stall: the probe reaches the server 300 ms late.
                round_trip(i, send_us, half + STALL_US, half)
            } else {
                let trip = round_trip(i, send_us, half, half);
                clean.push(trip);
                trip
            };
            all.push(trip);
        }

        let mut gated = ClockEstimator::new(DEFAULT_MICROS_PER_TICK);
        let mut clean_only = ClockEstimator::new(DEFAULT_MICROS_PER_TICK);
        let mut max_error = 0.0_f64;
        for (n, trip) in all.iter().enumerate() {
            feed(&mut gated, std::slice::from_ref(trip));
            if n >= 10 {
                max_error =
                    max_error.max((gated.estimated_server_tick_at(0) - TRUE_OFFSET_TICKS).abs());
            }
        }
        feed(&mut clean_only, &clean);

        assert!(
            max_error <= 1.0,
            "gated offset must track truth within 1 tick, worst error {max_error:.3}"
        );
        assert!(
            (gated.jitter_micros() - clean_only.jitter_micros()).abs() < 1_000.0,
            "gated jitter {} must match the clean-only jitter {}",
            gated.jitter_micros(),
            clean_only.jitter_micros()
        );
        assert!(
            gated.rtt_micros() < 170_000.0,
            "stalls must not inflate RTT, got {}",
            gated.rtt_micros()
        );

        // The pre-gate law, fed the same sequence, is biased by several ticks and
        // its jitter is inflated far past the clean link's.
        let (old_offset, old_jitter) = ungated_fold(&all);
        assert!(
            old_offset - TRUE_OFFSET_TICKS > 2.0,
            "reference: the ungated fold is biased, got {:.3} ticks",
            old_offset - TRUE_OFFSET_TICKS
        );
        assert!(
            old_jitter > 5.0 * clean_only.jitter_micros(),
            "reference: the ungated jitter {old_jitter} is inflated"
        );
    }

    // A genuine sustained latency increase must not be gated out forever: once the
    // pre-step minimum rolls out of the window, the new RTT is adopted.
    #[test]
    fn min_rtt_gate_adopts_sustained_rtt_step_within_one_window() {
        let mut est = ClockEstimator::new(DEFAULT_MICROS_PER_TICK);
        let mut id = 0u32;
        let mut send_us = 0u64;
        let next = |half: u64, id: &mut u32, send_us: &mut u64| {
            let trip = round_trip(*id, *send_us, half, half);
            *id += 1;
            *send_us += SAMPLE_PERIOD_US;
            trip
        };
        for _ in 0..(2 * RTT_WINDOW_SAMPLES) {
            let trip = next(50_000, &mut id, &mut send_us);
            feed(&mut est, &[trip]);
        }
        assert!((est.rtt_micros() - 100_000.0).abs() < EPS);

        // Step to 250 ms. The first RTT_WINDOW_SAMPLES - 1 step samples still share
        // the window with a 100 ms minimum and are gated out.
        for _ in 0..(RTT_WINDOW_SAMPLES - 1) {
            let trip = next(125_000, &mut id, &mut send_us);
            feed(&mut est, &[trip]);
            assert!(
                (est.rtt_micros() - 100_000.0).abs() < EPS,
                "a step sample is gated while the old minimum remains in the window"
            );
        }
        // Permit side: the window's last pre-step sample rolls out, so the next
        // step sample folds.
        let trip = next(125_000, &mut id, &mut send_us);
        feed(&mut est, &[trip]);
        assert!(
            est.rtt_micros() > 110_000.0,
            "the step is adopted within one window, RTT {}",
            est.rtt_micros()
        );

        for _ in 0..60 {
            let trip = next(125_000, &mut id, &mut send_us);
            feed(&mut est, &[trip]);
        }
        assert!(
            (est.rtt_micros() - 250_000.0).abs() < 1_000.0,
            "RTT converges to the new level, got {}",
            est.rtt_micros()
        );
        assert!(
            (est.estimated_server_tick_at(0) - TRUE_OFFSET_TICKS).abs() <= 1.0,
            "the symmetric step leaves the offset unbiased"
        );
    }

    #[test]
    fn min_rtt_gate_floor_admits_lan_scale_noise() {
        // At a ~1 ms LAN minimum the proportional tolerance is tiny; the 20 ms
        // floor still admits ordinary scheduling noise.
        let mut est = ClockEstimator::new(DEFAULT_MICROS_PER_TICK);
        feed(&mut est, &[round_trip(0, 0, 500, 500)]);
        feed(&mut est, &[round_trip(1, SAMPLE_PERIOD_US, 9_000, 9_000)]);
        assert!(
            est.rtt_micros() > 1_000.0,
            "an 18 ms sample within the floor folds, RTT {}",
            est.rtt_micros()
        );
    }

    // A stalled first probe seeds an inflated RTT and a midpoint biased by half the
    // stall. At EWMA weight 0.1 that took seconds to drain; the first clean sample,
    // a new window minimum far below the smoothed RTT, reseeds outright.
    #[test]
    fn stalled_first_probe_reseeds_on_the_first_clean_sample() {
        const STALL_US: u64 = 300_000;
        let mut est = ClockEstimator::new(DEFAULT_MICROS_PER_TICK);
        feed(&mut est, &[round_trip(0, 0, 75_000 + STALL_US, 75_000)]);
        assert!(
            (est.rtt_micros() - 450_000.0).abs() < EPS,
            "the stall seeded"
        );

        let first_clean = round_trip(1, SAMPLE_PERIOD_US, 75_000, 75_000);
        feed(&mut est, std::slice::from_ref(&first_clean));
        let (clean_offset, _) = ungated_fold(std::slice::from_ref(&first_clean));
        assert!(
            (est.rtt_micros() - 150_000.0).abs() < EPS,
            "RTT reseeds from the clean sample, got {}",
            est.rtt_micros()
        );
        assert!(est.jitter_micros().abs() < EPS, "jitter resets to zero");
        assert!(
            (est.estimated_server_tick_at(0) - clean_offset).abs() < EPS,
            "the offset reseeds from the clean sample"
        );
        assert!((est.estimated_server_tick_at(0) - TRUE_OFFSET_TICKS).abs() <= 1.0);

        // Later clean samples fold as usual: a repeat of the minimum does not
        // reseed and leaves the estimate at the clean link.
        feed(
            &mut est,
            &[round_trip(2, 2 * SAMPLE_PERIOD_US, 75_000, 75_000)],
        );
        assert!((est.rtt_micros() - 150_000.0).abs() < EPS);
        assert!(est.jitter_micros().abs() < EPS);
    }

    // --- Provenance guard: forged sample_id cannot wedge the estimator ---

    #[test]
    fn forged_max_sample_id_is_rejected_and_does_not_freeze_estimator() {
        // A single echo with sample_id = u32::MAX (never sent) must be rejected
        // even when it is strictly greater than the last accepted id (None here).
        // A subsequent legitimate echo (sample_id 0, which was sent) must still
        // advance the estimate — the estimator is not wedged.
        let mut est = ClockEstimator::new(DEFAULT_MICROS_PER_TICK);
        est.record_sent(0); // only id 0 has been issued

        // Forged max id — never sent, must be rejected.
        let forged = echo(u32::MAX, 500_000, 999);
        assert!(
            !est.ingest_echo(&forged, 600_000),
            "echo with sample_id u32::MAX was never sent and must be rejected"
        );
        assert!(
            !est.is_initialized(),
            "a rejected forged echo must not advance the estimate"
        );

        // A legitimate echo (id 0, which was sent) must still be accepted.
        assert!(
            est.ingest_echo(&echo(0, 0, 600), 100_000),
            "a legitimate echo must advance the estimate after a forged echo was rejected"
        );
        assert!(est.is_initialized());
    }

    // --- Sender cadence (5 Hz) ---

    #[test]
    fn sender_fires_immediately_then_at_5hz() {
        let clock = VirtualClock::new(0);
        let mut sender = TimeSyncSender::new();

        // First call fires immediately with sample_id 0.
        let first = sender.maybe_send(&clock, 0).expect("first probe fires");
        assert_eq!(first.sample_id, 0);
        assert_eq!(first.client_send_time_us, 0);

        // Before 200 ms elapses, nothing fires.
        clock.advance(199_000);
        assert!(sender.maybe_send(&clock, 0).is_none());

        // At/after 200 ms, the next probe fires with the next id.
        clock.advance(1_000); // now = 200_000
        let second = sender.maybe_send(&clock, 12).expect("second probe at 5 Hz");
        assert_eq!(second.sample_id, 1);
        assert_eq!(second.client_send_tick, 12);
        assert_eq!(second.client_send_time_us, 200_000);
    }

    // --- estimated_server_tick advances with the clock ---

    #[test]
    fn estimated_server_tick_advances_with_local_clock() {
        let mut est = ClockEstimator::new(DEFAULT_MICROS_PER_TICK);
        est.record_sent(0);
        // Seed: at local time 0 the server is at tick 600.
        assert!(est.ingest_echo(&echo(0, 0, 600), 0));

        let clock = VirtualClock::new(0);
        let t0 = est.estimated_server_tick(&clock);
        assert!((t0 - 600.0).abs() < EPS);

        // One tick of local time elapses (16_667 us): the estimate advances by
        // exactly one server tick (same tick rate on both ends).
        clock.advance(DEFAULT_MICROS_PER_TICK);
        let t1 = est.estimated_server_tick(&clock);
        assert!(
            (t1 - 601.0).abs() < 1e-3,
            "one local tick of elapsed time advances the estimate one server tick"
        );
        clock.set(0);
    }
}
