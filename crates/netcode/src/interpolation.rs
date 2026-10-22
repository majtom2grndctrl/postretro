// Client-side remote-entity interpolation: per-`NetworkId` sample buffers, the
// interpolation-delay sizing law, the interpolate/extrapolate/hold state machine,
// and the host-only deterministic demo mover (Phase 2 net-demo fixture).
// See: context/lib/networking.md · context/lib/entity_model.md §5
//
// M15 Phase 2 Task 6. The client renders each remote (server-authoritative) entity
// in the *past* — at `estimated_server_tick - interpolation_delay` — so the two
// snapshots bracketing that render time have almost always arrived, and the visible
// pose is a clean lerp/slerp between them rather than a teleport on each packet.
//
// `estimated_server_tick` is the server's *current* tick, but a snapshot stamped
// tick T reaches the client about one-way latency after T. The delay is the larger
// of two laws, each rounded up to whole ticks:
//   - Time-sync law: a transit term (smoothed RTT / 2) plus the buffer term
//     `clamp(50 ms + 2 × jitter + starvation_margin, 50 ms, 250 ms)`.
//   - Measured-lag law: `lag_peak + one snapshot interval`, where
//     `lag_peak` is a decaying peak of how far the estimated server tick has run
//     past each newly accepted newest snapshot's tick. Snapshots ride the
//     unreliable channel, so this sees the true arrival spread that time sync's
//     min-RTT gate filters out, and any offset error cancels because the render
//     target reads the same estimate.
// On a LAN (RTT ≈ 0, lag ≈ 0) the delay reduces to the 50 ms buffer floor. The
// render target slews toward `estimated_server_tick - delay` at 0.75–1.25× the
// frame's wall-clock time, so a whole-tick delay step or a forward estimate jump
// neither freezes nor jumps remotes. A target already at or past the newest
// snapshot may hold instead of playing on into starvation.
//
// Time model: every sample is stamped with the server tick it was valid at (the
// snapshot's `server_tick`), never a wall-clock instant. The render target is also a
// server tick, derived from `ClientTimeSync` (Task 5) minus the delay. So this whole
// module is wall-clock-free and unit-testable on injected ticks.
//
// State machine when sampling at a target server tick `t`:
//   - Two samples bracket `t` (`a.tick <= t <= b.tick`): LERP/SLERP between them.
//   - `t` older than the oldest sample: HOLD the oldest pose (we have not buffered
//     far enough back; clamping to the oldest is the least-wrong visible pose).
//   - `t` newer than the newest sample (starvation): EXTRAPOLATE forward from the
//     newest pose using the velocity derived from its newest transform segment for at
//     most `MAX_EXTRAPOLATION` of server time, then HOLD the newest pose. A buffer
//     with no adjacent transform samples holds immediately.

use std::collections::HashMap;
use std::collections::VecDeque;

use glam::{Quat, Vec3};

use crate::sim::LocomotionStance;

use postretro_net::wire::NetworkId;

use postretro_entities::Transform;

/// Lower bound of the interpolation delay: 50 ms expressed in microseconds.
///
/// M15 Phase 3 calibration (playtest bug "Symptom 2", 2026-06-22): the remote view
/// lagged ~0.5 s — the sum of this floor (was 100 ms, up to 250 ms under jitter), the
/// 20 Hz snapshot cadence, and time-sync smoothing. Co-op runs on LAN / low-latency
/// links with a small player count, so the conservative 100 ms floor (sized for an
/// open-internet competitive link) was overkill. Halving it to 50 ms ≈ 3 ticks at
/// 60 Hz keeps two snapshots bracketing the render target on a clean link at the
/// raised 30 Hz cadence (`SNAPSHOT_TICK_INTERVAL = 2`, ~33 ms apart), so motion stays
/// smooth without starving the buffer; the jitter term below still raises the delay
/// automatically (toward `MAX_DELAY_MICROS`) on a genuinely jittery link. Combined
/// with the faster cadence this targets ~80-120 ms steady-state remote-view latency.
const MIN_DELAY_MICROS: u64 = 50_000;
/// Upper bound of the interpolation delay: 250 ms. Past this the added input-to-photon
/// latency is worse than the occasional extrapolation a tighter delay would cause.
/// Unchanged by the Symptom 2 calibration: it is the safety ceiling for a bad link,
/// reached only when measured jitter is high, not the steady-state co-op latency.
pub(crate) const MAX_DELAY_MICROS: u64 = 250_000;
/// Jitter multiplier in the delay law: the delay absorbs twice the measured jitter on
/// top of the 50 ms floor before clamping (a 2σ-style margin against the smoothed
/// mean-absolute deviation `ClientTimeSync::jitter_micros` reports).
const JITTER_MULTIPLIER: f64 = 2.0;
/// Rate at which the starvation margin rises while remote buffers are starving,
/// in microseconds of margin per wall-clock second.
///
/// Framerate-independent: the rise is `rate × dt_secs` per observed frame rather than
/// a fixed per-frame step, so the controller's wall-clock time-constant no longer
/// scales with frame rate (a 144 fps client adapted ~2.4× faster than a 60 fps one
/// before this). Calibrated to preserve the previous 60 Hz behavior exactly: the old
/// law added 25 ms per starved frame, so at 60 fps that is `25_000 × 60 = 1_500_000`
/// µs/sec. One 60 Hz frame (`dt = 1/60`) still adds exactly 25 ms.
const STARVATION_MARGIN_RISE_MICROS_PER_SEC: f64 = 25_000.0 * 60.0;
/// The starvation feedback may add at most 100 ms before the normal max-delay clamp.
/// Jitter can still raise the final delay to `MAX_DELAY_MICROS`.
const MAX_STARVATION_MARGIN_MICROS: f64 = 100_000.0;
/// Per-second multiplicative decay factor for the starvation margin on clean frames.
///
/// Framerate-independent: the decay is applied as `factor.powf(dt_secs)`, so the
/// fraction retained over a wall-clock second is constant regardless of frame rate.
/// Calibrated to preserve the previous 60 Hz behavior exactly: the old law multiplied
/// by `0.85` per clean frame, so over one second (60 frames at 60 Hz) the margin was
/// scaled by `0.85^60`. Defining the per-second factor as `0.85^60` reproduces the old
/// `0.85` per-frame factor at `dt = 1/60` (`(0.85^60)^(1/60) = 0.85`).
///
/// `LazyLock` because `f64::powi` is not a `const fn`; the value is computed once on
/// first use and never changes.
static STARVATION_MARGIN_DECAY_PER_SEC: std::sync::LazyLock<f64> =
    std::sync::LazyLock::new(|| 0.85_f64.powi(60));
/// Ceiling applied to a single frame's `dt_secs` before driving the starvation
/// feedback or the render target's playback. A hitch (long GC pause, alt-tab,
/// breakpoint) would otherwise let one frame add a pathological amount of margin,
/// decay it fully to zero, or jump remotes; clamping to 0.25 s bounds the per-frame
/// influence to a quarter-second of adaptation.
const MAX_FEEDBACK_DT_SECS: f64 = 0.25;
/// Half-life of the snapshot-lag peak's decay toward the latest measured lag, in
/// microseconds of estimated server time (2 s). A new maximum lifts the peak
/// instantly; a lower lag pulls it down by half the gap every 2 s. Measured on the
/// server-tick estimate rather than per call or per frame, so it is independent of
/// frame rate and of how many snapshots happen to land per frame.
const LAG_PEAK_HALF_LIFE_MICROS: f64 = 2_000_000.0;
/// Ceiling on a single measured snapshot lag, in microseconds (1 s), so a corrupt
/// or transiently wild server-tick estimate cannot drive the delay unbounded.
const MAX_SNAPSHOT_LAG_MICROS: f64 = 1_000_000.0;
/// Once the decaying lag peak is within this many ticks of the latest lag, it snaps
/// to it, so a settled link's delay is not held one rounded-up tick high by an
/// exponential tail that never quite closes.
const LAG_PEAK_EPSILON_TICKS: f64 = 0.05;
/// Below this, the starvation margin is treated as settled and snapped to zero.
const STARVATION_MARGIN_EPSILON_MICROS: f64 = 500.0;
/// Playback-rate bounds for the slewed render target, as multiples of the frame's
/// wall-clock time in server ticks. A delay step or an estimate jump moves the
/// target toward its new value at these rates instead of freezing remotes (a
/// raise) or jumping a whole tick in one frame (a lowered ceiling or a reseed), so
/// remote motion stays even. ±25% stays below a noticeable speed change. The lower
/// bound drops to zero once the target reaches the newest snapshot (see
/// [`InterpolationDelayState::render_server_tick`]).
const RENDER_TARGET_PLAYBACK_RATE: std::ops::RangeInclusive<f64> = 0.75..=1.25;
/// A render target this many ticks behind its candidate snaps to it (0.5 s): a
/// gap this wide would take seconds to slew out. One ahead by this much holds,
/// since the target never rewinds.
const RENDER_TARGET_SNAP_TICKS: f64 = 30.0;
/// Velocity below this squared magnitude is treated as stationary for starvation
/// feedback. The pose still holds; it just does not raise global delay.
const STARVATION_VELOCITY_EPSILON_SQ: f32 = 1e-8;

/// Maximum forward extrapolation past the newest sample, in microseconds (100 ms).
/// Beyond this the predicted pose has drifted too far from any real sample to trust,
/// so the state machine holds the last pose instead of extrapolating further.
const MAX_EXTRAPOLATION_MICROS: f64 = 100_000.0;

/// How many samples to retain per entity. At the 30 Hz snapshot cadence (~2 sim ticks
/// apart) and a 250 ms max delay, ~8 samples span the delay window; 16 leaves ample
/// headroom for reordered/duplicated arrivals without unbounded growth.
const MAX_SAMPLES_PER_ENTITY: usize = 16;

/// Base interpolation delay in **whole sim ticks**, sized from measured link jitter,
/// with no transit term (RTT 0, the LAN case).
///
/// The continuous law is `clamp(50 ms + 2 × jitter, 50 ms, 250 ms)`; the result is
/// then **rounded up** to a whole number of sim ticks, because the render target is a
/// server tick and a fractional-tick delay would bias the bracketing search. Rounding
/// *up* keeps the delay at least the requested duration (never less buffered headroom
/// than asked for).
///
/// `jitter_micros` is the smoothed jitter from `ClientTimeSync::jitter_micros`;
/// `micros_per_tick` is the engine's `DEFAULT_MICROS_PER_TICK`. A negative or
/// non-finite jitter (impossible from the estimator, but defended) is treated as zero.
#[cfg(test)]
#[must_use]
pub(crate) fn interpolation_delay_ticks(jitter_micros: f64, micros_per_tick: u64) -> u32 {
    adaptive_interpolation_delay_ticks(jitter_micros, 0.0, 0.0, micros_per_tick)
}

/// Client-side feedback state for the remote interpolation delay.
///
/// Time-sync jitter is the feed-forward signal. Buffer starvation is the feedback
/// signal: if remotes had to hold their newest pose, the current delay was too tight
/// for recent traffic, so subsequent frames temporarily add headroom.
///
/// Production delay is the larger of, each rounded up to whole sim ticks:
/// - `rtt / 2 + clamp(50 ms + 2 × jitter + starvation_margin, 50 ms, 250 ms)`.
///   `rtt / 2` is the estimated one-way transit latency: the render target is
///   measured back from the server's *current* tick, and a snapshot reaches the
///   client that long after it was stamped. The clamp bounds only the buffer term;
///   a non-finite or negative RTT counts as zero.
/// - `lag_peak + SNAPSHOT_TICK_INTERVAL`, once a snapshot lag has been
///   observed ([`Self::observe_newest_snapshot`]). `lag_peak` rises instantly to a
///   new maximum and decays toward the latest lag with a
///   [`LAG_PEAK_HALF_LIFE_MICROS`] half-life; each lag is capped at
///   [`MAX_SNAPSHOT_LAG_MICROS`].
///
/// The render target is slewed: it moves toward `estimated − delay` at a playback
/// rate within [`RENDER_TARGET_PLAYBACK_RATE`] of the frame's wall-clock time,
/// holds instead once it reaches the newest snapshot, never rewinds, and snaps
/// only on the first frame or past [`RENDER_TARGET_SNAP_TICKS`].
#[derive(Debug, Clone, Copy, Default)]
pub struct InterpolationDelayState {
    starvation_margin_micros: f64,
    /// Previous frame's render target.
    last_render_tick: Option<f64>,
    lag_peak: Option<SnapshotLagPeak>,
    /// Newest snapshot tick already folded, so each one folds once. Survives a
    /// level unload: the replication cursor keeps the pre-unload tick, and folding
    /// it against a post-load estimate would read the load time as lag. The render
    /// target also reads it to tell when remotes have run out of snapshots.
    folded_snapshot_tick: Option<u32>,
}

/// Decaying peak of measured snapshot arrival lag, in server ticks.
#[derive(Debug, Clone, Copy)]
struct SnapshotLagPeak {
    peak_ticks: f64,
    /// Estimated server tick at the latest fold: the decay clock.
    observed_at_tick: f64,
}

impl InterpolationDelayState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Drop level-scoped delay feedback after remote interpolation buffers are reset.
    /// The folded snapshot tick stays, so only a snapshot accepted after the reset
    /// folds.
    pub(crate) fn reset_for_level_unload(&mut self) {
        *self = Self {
            folded_snapshot_tick: self.folded_snapshot_tick,
            ..Self::default()
        };
    }

    /// Delay in whole sim ticks: the larger of the time-sync law (measured jitter,
    /// recent starvation, and the one-way transit implied by the smoothed
    /// `rtt_micros`) and the measured snapshot-lag law.
    pub(crate) fn delay_ticks(
        &self,
        jitter_micros: f64,
        rtt_micros: f64,
        micros_per_tick: u64,
    ) -> u32 {
        let time_sync_law = adaptive_interpolation_delay_ticks(
            jitter_micros,
            self.starvation_margin_micros,
            rtt_micros,
            micros_per_tick,
        );
        let Some(lag) = self.lag_peak else {
            return time_sync_law;
        };
        // The lag is read on the first sampled frame after arrival, so it already
        // over-reads by up to a frame; no margin beyond the snapshot interval.
        let lag_law = (lag.peak_ticks + f64::from(crate::host::SNAPSHOT_TICK_INTERVAL)).ceil();
        // The peak is capped and non-negative, so this cast never saturates.
        time_sync_law.max(lag_law as u32)
    }

    /// Fold the arrival lag of the newest accepted snapshot: how far the server-tick
    /// estimate `estimated_tick` has run past `newest_snapshot_tick`. Call once per
    /// frame with the newest accepted snapshot's tick; a tick already folded is a
    /// no-op, so per-frame calls fold each new newest snapshot exactly once.
    ///
    /// The lag is clamped to `[0, MAX_SNAPSHOT_LAG_MICROS]`. A new maximum lifts the
    /// peak instantly; otherwise the peak decays toward this lag by the estimated
    /// server time elapsed since the previous fold.
    pub(crate) fn observe_newest_snapshot(
        &mut self,
        estimated_tick: f64,
        newest_snapshot_tick: u32,
        micros_per_tick: u64,
    ) {
        if !estimated_tick.is_finite() || self.folded_snapshot_tick == Some(newest_snapshot_tick) {
            return;
        }
        let micros_per_tick = micros_per_tick as f64;
        let max_lag_ticks = MAX_SNAPSHOT_LAG_MICROS / micros_per_tick;
        let lag_ticks =
            (estimated_tick - f64::from(newest_snapshot_tick)).clamp(0.0, max_lag_ticks);
        let peak_ticks = match self.lag_peak {
            None => lag_ticks,
            Some(previous) => {
                let elapsed_micros =
                    (estimated_tick - previous.observed_at_tick).max(0.0) * micros_per_tick;
                let retained = 0.5_f64.powf(elapsed_micros / LAG_PEAK_HALF_LIFE_MICROS);
                let excess = (previous.peak_ticks - lag_ticks) * retained;
                if excess < LAG_PEAK_EPSILON_TICKS {
                    lag_ticks
                } else {
                    lag_ticks + excess
                }
            }
        };
        self.lag_peak = Some(SnapshotLagPeak {
            peak_ticks,
            observed_at_tick: estimated_tick,
        });
        self.folded_snapshot_tick = Some(newest_snapshot_tick);
    }

    /// Render target tick for the remote interpolation buffers. The candidate is
    /// `estimated_tick − delay`; the target slews toward it at a playback rate
    /// clamped to [`RENDER_TARGET_PLAYBACK_RATE`] of `frame_dt_secs` in ticks, so a
    /// whole-tick delay step or a forward estimate jump (a time-sync reseed)
    /// neither freezes remotes nor jumps them. Once the previous target is at or
    /// past the newest folded snapshot the lower bound is zero: the target may
    /// hold rather than keep slewing while the delay rises, so a lasting latency
    /// rise starves remotes about as long as the rise. At a steady delay it keeps
    /// tracking the candidate. It never moves backward: an estimate that
    /// steps back slows it like a delay raise, and one more than
    /// [`RENDER_TARGET_SNAP_TICKS`] back holds it. The first frame, and a target
    /// that far behind the candidate, snap.
    pub(crate) fn render_server_tick(
        &mut self,
        estimated_tick: f64,
        frame_dt_secs: f64,
        jitter_micros: f64,
        rtt_micros: f64,
        micros_per_tick: u64,
    ) -> f64 {
        if !estimated_tick.is_finite() {
            // Hold the last target. With none yet the value passes through: the
            // caller samples only after the estimator seeds a finite tick.
            return self.last_render_tick.unwrap_or(estimated_tick);
        }
        let delay_ticks = self.delay_ticks(jitter_micros, rtt_micros, micros_per_tick);
        let candidate = estimated_tick - f64::from(delay_ticks);
        let render_tick = match self.last_render_tick {
            None => candidate,
            Some(last) => {
                let gap = candidate - last;
                if gap > RENDER_TARGET_SNAP_TICKS {
                    candidate
                } else if gap < -RENDER_TARGET_SNAP_TICKS {
                    last
                } else {
                    let nominal = playback_ticks(frame_dt_secs, micros_per_tick);
                    let starved = self
                        .folded_snapshot_tick
                        .is_some_and(|newest| last >= f64::from(newest));
                    let min_rate = if starved {
                        0.0
                    } else {
                        *RENDER_TARGET_PLAYBACK_RATE.start()
                    };
                    last + gap.clamp(
                        min_rate * nominal,
                        RENDER_TARGET_PLAYBACK_RATE.end() * nominal,
                    )
                }
            }
        };
        self.last_render_tick = Some(render_tick);
        render_tick
    }

    /// Fold one sampled frame into the starvation feedback. Call only when at least
    /// one remote entity was actually sampled; empty frames carry no link signal.
    ///
    /// `dt_secs` is the frame's wall-clock delta. The rise and decay are driven off it
    /// so the controller's time-constant is framerate-independent (it reproduces the
    /// old per-frame behavior exactly at 60 fps). A non-finite or non-positive `dt` is
    /// a no-op (no signal to integrate); a too-large `dt` (a hitch) is clamped to
    /// `MAX_FEEDBACK_DT_SECS` so one long frame cannot produce a pathological jump or a
    /// full decay-to-zero.
    pub(crate) fn observe_sampled_frame(&mut self, had_starvation: bool, dt_secs: f64) {
        if !dt_secs.is_finite() || dt_secs <= 0.0 {
            return;
        }
        let dt = dt_secs.min(MAX_FEEDBACK_DT_SECS);
        if had_starvation {
            self.starvation_margin_micros = (self.starvation_margin_micros
                + STARVATION_MARGIN_RISE_MICROS_PER_SEC * dt)
                .min(MAX_STARVATION_MARGIN_MICROS);
        } else {
            self.starvation_margin_micros *= STARVATION_MARGIN_DECAY_PER_SEC.powf(dt);
            if self.starvation_margin_micros < STARVATION_MARGIN_EPSILON_MICROS {
                self.starvation_margin_micros = 0.0;
            }
        }
    }

    #[cfg(test)]
    fn starvation_margin_micros(&self) -> f64 {
        self.starvation_margin_micros
    }

    #[cfg(test)]
    fn lag_peak_ticks(&self) -> Option<f64> {
        self.lag_peak.map(|lag| lag.peak_ticks)
    }
}

/// One frame's wall-clock delta in server ticks: the render target's nominal
/// playback. A non-finite or negative delta plays nothing; a hitch is clamped to
/// [`MAX_FEEDBACK_DT_SECS`] and the gap it leaves slews out over later frames.
/// Always finite and non-negative, so the playback clamp's bounds stay ordered.
fn playback_ticks(frame_dt_secs: f64, micros_per_tick: u64) -> f64 {
    let dt = if frame_dt_secs.is_finite() && frame_dt_secs > 0.0 {
        frame_dt_secs.min(MAX_FEEDBACK_DT_SECS)
    } else {
        0.0
    };
    dt * 1_000_000.0 / micros_per_tick.max(1) as f64
}

fn adaptive_interpolation_delay_ticks(
    jitter_micros: f64,
    starvation_margin_micros: f64,
    rtt_micros: f64,
    micros_per_tick: u64,
) -> u32 {
    debug_assert!(micros_per_tick > 0, "micros_per_tick must be positive");
    let jitter = if jitter_micros.is_finite() && jitter_micros > 0.0 {
        jitter_micros
    } else {
        0.0
    };
    let starvation_margin =
        if starvation_margin_micros.is_finite() && starvation_margin_micros > 0.0 {
            starvation_margin_micros.min(MAX_STARVATION_MARGIN_MICROS)
        } else {
            0.0
        };
    // One-way transit: half the smoothed RTT. Unclamped — it is measured latency,
    // not buffering headroom.
    let transit = if rtt_micros.is_finite() && rtt_micros > 0.0 {
        rtt_micros / 2.0
    } else {
        0.0
    };
    // Clamp the buffer term, add transit, then round UP to whole ticks.
    let raw_micros = MIN_DELAY_MICROS as f64 + JITTER_MULTIPLIER * jitter + starvation_margin;
    let clamped = raw_micros.clamp(MIN_DELAY_MICROS as f64, MAX_DELAY_MICROS as f64);
    let ticks = ((clamped + transit) / micros_per_tick as f64).ceil();
    // Finite and non-negative; an absurd RTT saturates the cast rather than wrapping.
    ticks as u32
}

/// One buffered remote pose, stamped by the server tick it was valid at.
/// `intrinsic_velocity` is the replicated player-movement velocity, which excludes
/// mover carry and is used only for remote-player presentation. World-transform
/// extrapolation derives velocity from adjacent transform samples. `aim_pitch` is the
/// presentation pitch sampled with the transform from the same snapshot. `stance`
/// is the replicated movement state's posture and footing, present exactly when
/// `intrinsic_velocity` is: both come from the same movement payload.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TransformSample {
    pub(crate) teleport: Option<TeleportSample>,
    pub(crate) server_tick: u32,
    pub(crate) transform: Transform,
    pub(crate) intrinsic_velocity: Option<Vec3>,
    pub(crate) stance: Option<LocomotionStance>,
    pub(crate) aim_pitch: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TeleportSample {
    pub(crate) generation: u8,
    pub(crate) last_teleporter: u16,
}

/// Accepted authoritative sample where a remote pawn's generation changed.
/// Its position is independent of the delayed pose currently being rendered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ObservedTeleport {
    pub entity_id: postretro_entities::EntityId,
    pub generation: u8,
    pub last_teleporter: u16,
    pub arrival: Vec3,
}

fn crosses_teleport(a: &TransformSample, b: &TransformSample) -> bool {
    a.teleport
        .zip(b.teleport)
        .is_some_and(|(a, b)| a.generation != b.generation)
}

fn cut_pose(
    sample: &TransformSample,
    stance: LocomotionStance,
    holds_newest: bool,
    state_tick: f64,
) -> PresentedPose {
    PresentedPose {
        state_tick,
        transform: sample.transform,
        source: PoseSource::TeleportCut,
        holds_newest,
        speed_xz: 0.0,
        horizontal_velocity: Vec3::ZERO,
        intrinsic_velocity: sample.intrinsic_velocity,
        stance,
        aim_pitch: sample.aim_pitch,
    }
}

/// The pose the state machine resolved for a render target, plus which branch
/// produced it. The branch tag is observable so tests assert the interpolate /
/// extrapolate / hold transitions directly (testing_guide §Behavior over
/// implementation: the branch *is* the contract here, not an internal detail).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PresentedPose {
    /// The host tick, in the buffer's sample units, whose replicated state this
    /// pose shows, for state read beside it (ghost-link history). The target tick
    /// itself, except that a blend shows its leading sample, so a state and the
    /// first motion toward it present together, and a pose held on the near side
    /// of a teleport cut shows no state of the cut's own tick, so an arm sent with
    /// a swap presents on the cut frame.
    pub(crate) state_tick: f64,
    pub(crate) transform: Transform,
    pub(crate) source: PoseSource,
    /// The target is at or past the newest sample and this pose holds it: a
    /// `HeldNewest` hold, or a cut onto the newest sample when it arrived through
    /// a teleport. Adaptive delay reads both as a starved buffer.
    pub(crate) holds_newest: bool,
    /// Horizontal speed of the motion represented by `transform`. This is the
    /// displayed motion, rather than a host-side simulation velocity: interpolated
    /// Transform-only enemies derive it from their bracketing samples, and held
    /// poses report zero because the client is visibly stationary.
    pub(crate) speed_xz: f32,
    /// Horizontal world-space motion represented by this presentation sample.
    /// Avatar pose uses its direction for lower-body heading; held poses report
    /// zero so callers fall back to the displayed transform yaw.
    pub(crate) horizontal_velocity: Vec3,
    /// Interpolated replicated player-movement velocity. This stays distinct from
    /// [`Self::horizontal_velocity`]: a rider's transform can orbit with a mover
    /// while their player-controlled velocity remains stationary.
    pub(crate) intrinsic_velocity: Option<Vec3>,
    /// Replicated posture and footing at or before the presented tick. Discrete,
    /// so it steps rather than blends: between two samples it is the earlier
    /// one's until the target reaches the later. Its vertical speed steps with it,
    /// so a takeoff or apex lands on the tick the host saw it. Standing and
    /// grounded when no movement state is known.
    pub(crate) stance: LocomotionStance,
    /// Interpolated remote camera pitch for avatar pose presentation.
    pub(crate) aim_pitch: f32,
}

/// Which branch of the sample state machine produced a [`PresentedPose`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PoseSource {
    /// Hold one side of a generation discontinuity without deriving motion.
    TeleportCut,
    /// Target bracketed by two samples: lerp/slerp between them.
    Interpolated,
    /// Target older than the oldest sample: held the oldest pose.
    HeldOldest,
    /// Target newer than the newest sample, within the extrapolation window:
    /// extrapolated forward from the newest pose by its velocity.
    Extrapolated,
    /// Target newer than the newest sample, past the extrapolation window or with
    /// no velocity to extrapolate: held the newest pose.
    HeldNewest,
}

/// Per-entity ordered sample buffer (newest at the back), keyed externally by
/// `NetworkId`. Samples are inserted in server-tick order; an out-of-order or
/// duplicate-tick arrival is positioned by tick so the bracketing search is always
/// against a monotonically-stamped sequence.
#[derive(Debug, Default)]
struct EntityBuffer {
    /// Samples ordered by ascending `server_tick`. Bounded to
    /// [`MAX_SAMPLES_PER_ENTITY`]; the oldest is evicted on overflow.
    samples: VecDeque<TransformSample>,
}

impl EntityBuffer {
    /// Insert a sample, keeping the deque ordered by ascending `server_tick`. A
    /// duplicate tick replaces the existing sample (latest-wins for the same tick);
    /// the oldest sample is evicted once the buffer is full.
    fn insert(&mut self, sample: TransformSample) {
        // Common case: the new sample is the newest (snapshots usually arrive in
        // order). Fast-path the back insert; otherwise find the ordered position.
        match self.samples.back() {
            Some(back) if sample.server_tick > back.server_tick => {
                self.samples.push_back(sample);
            }
            Some(back) if sample.server_tick == back.server_tick => {
                // Same tick as the newest: replace it (latest-wins).
                *self.samples.back_mut().expect("back exists") = sample;
            }
            Some(_) => {
                // Out-of-order arrival: insert by tick. Replace on an exact tick match.
                let pos = self
                    .samples
                    .iter()
                    .position(|s| s.server_tick >= sample.server_tick)
                    .unwrap_or(self.samples.len());
                if self
                    .samples
                    .get(pos)
                    .is_some_and(|s| s.server_tick == sample.server_tick)
                {
                    self.samples[pos] = sample;
                } else {
                    self.samples.insert(pos, sample);
                }
            }
            None => self.samples.push_back(sample),
        }

        // Bound the buffer: evict the oldest until within capacity.
        while self.samples.len() > MAX_SAMPLES_PER_ENTITY {
            self.samples.pop_front();
        }
    }

    /// Resolve the pose at server tick `target_tick` via the interpolate /
    /// extrapolate / hold state machine. `None` only when the buffer is empty (no
    /// sample ever received for this entity).
    fn sample_at(&self, target_tick: f64) -> Option<PresentedPose> {
        let oldest = self.samples.front()?;
        let newest = self.samples.back().expect("non-empty buffer has a back");

        // Target older than everything buffered: hold the oldest pose. We simply do
        // not have history that far back.
        if target_tick <= f64::from(oldest.server_tick) {
            return Some(PresentedPose {
                state_tick: target_tick,
                transform: oldest.transform,
                source: PoseSource::HeldOldest,
                holds_newest: false,
                speed_xz: 0.0,
                horizontal_velocity: Vec3::ZERO,
                intrinsic_velocity: self.intrinsic_velocity_at_or_before(oldest.server_tick),
                stance: self.stance_at_or_before(oldest.server_tick),
                aim_pitch: oldest.aim_pitch,
            });
        }

        // Target newer than everything buffered: starvation. Extrapolate forward from
        // the newest pose by its velocity for at most MAX_EXTRAPOLATION, else hold.
        if target_tick >= f64::from(newest.server_tick) {
            return Some(self.extrapolate_or_hold(newest, target_tick));
        }

        // Bracketed: find the adjacent pair (a, b) with a.tick <= target <= b.tick.
        // Samples are tick-ordered, so the first sample with tick >= target is `b`
        // and its predecessor is `a`.
        let mut samples = self.samples.iter();
        let mut a = samples.next().expect("non-empty buffer has a first sample");
        for b in samples {
            if f64::from(a.server_tick) <= target_tick && target_tick <= f64::from(b.server_tick) {
                // Discrete fields take the earlier sample until the target
                // reaches the later one.
                let at = if target_tick < f64::from(b.server_tick) {
                    a
                } else {
                    b
                };
                if crosses_teleport(a, b) {
                    let state_tick = if target_tick < f64::from(b.server_tick) {
                        target_tick.min(f64::from(b.server_tick) - 1.0)
                    } else {
                        target_tick
                    };
                    return Some(cut_pose(
                        at,
                        self.stance_at_or_before(at.server_tick),
                        false,
                        state_tick,
                    ));
                }
                let span = f64::from(b.server_tick) - f64::from(a.server_tick);
                // Equal-tick neighbors (span 0) cannot happen — insert dedupes ticks —
                // but guard the divide rather than risk a NaN alpha.
                let alpha = if span > 0.0 {
                    ((target_tick - f64::from(a.server_tick)) / span) as f32
                } else {
                    0.0
                };
                return Some(PresentedPose {
                    state_tick: f64::from(b.server_tick),
                    transform: lerp_transform(&a.transform, &b.transform, alpha),
                    source: PoseSource::Interpolated,
                    holds_newest: false,
                    speed_xz: xz_speed_between(a, b),
                    horizontal_velocity: xz_velocity_between(a, b),
                    intrinsic_velocity: self.interpolated_intrinsic_velocity(a, b, alpha),
                    stance: self.stance_at_or_before(at.server_tick),
                    aim_pitch: a.aim_pitch + (b.aim_pitch - a.aim_pitch) * alpha,
                });
            }
            a = b;
        }

        // Unreachable: target is strictly between oldest and newest, so some adjacent
        // pair must bracket it. Fall back to the newest pose rather than panic.
        Some(PresentedPose {
            state_tick: target_tick,
            transform: newest.transform,
            source: PoseSource::HeldNewest,
            holds_newest: true,
            speed_xz: 0.0,
            horizontal_velocity: Vec3::ZERO,
            intrinsic_velocity: self.intrinsic_velocity_at_or_before(newest.server_tick),
            stance: self.stance_at_or_before(newest.server_tick),
            aim_pitch: newest.aim_pitch,
        })
    }

    /// Starvation branch: the render target is at/after the newest sample. If an
    /// adjacent transform sample supplies world velocity and the overshoot is within
    /// the bounded window, extrapolate position forward (rotation/scale held — no
    /// angular wire velocity to extrapolate); otherwise hold the newest pose.
    fn extrapolate_or_hold(&self, newest: &TransformSample, target_tick: f64) -> PresentedPose {
        if self
            .samples
            .iter()
            .rev()
            .nth(1)
            .is_some_and(|previous| crosses_teleport(previous, newest))
        {
            return cut_pose(
                newest,
                self.stance_at_or_before(newest.server_tick),
                true,
                target_tick,
            );
        }
        let overshoot_ticks = target_tick - f64::from(newest.server_tick);
        let overshoot_micros = overshoot_ticks * crate::netcode::SERVER_TICK_MICROS as f64;

        match self
            .samples
            .iter()
            .rev()
            .nth(1)
            .and_then(|previous| transform_velocity_between(previous, newest))
        {
            Some(velocity) if overshoot_micros <= MAX_EXTRAPOLATION_MICROS => {
                // Position advances by velocity × elapsed seconds; orientation and
                // scale hold (the wire carries no angular velocity).
                let dt_secs = (overshoot_ticks * crate::netcode::SERVER_TICK_MICROS as f64
                    / 1_000_000.0) as f32;
                let predicted = Transform {
                    position: newest.transform.position + velocity * dt_secs,
                    rotation: newest.transform.rotation,
                    scale: newest.transform.scale,
                };
                PresentedPose {
                    state_tick: target_tick,
                    transform: predicted,
                    source: PoseSource::Extrapolated,
                    holds_newest: false,
                    speed_xz: xz_speed(velocity),
                    horizontal_velocity: Vec3::new(velocity.x, 0.0, velocity.z),
                    intrinsic_velocity: self.intrinsic_velocity_at_or_before(newest.server_tick),
                    stance: self.stance_at_or_before(newest.server_tick),
                    aim_pitch: newest.aim_pitch,
                }
            }
            // No adjacent transform sample or past the extrapolation window: hold the
            // last known pose.
            _ => PresentedPose {
                state_tick: target_tick,
                transform: newest.transform,
                source: PoseSource::HeldNewest,
                holds_newest: true,
                speed_xz: 0.0,
                horizontal_velocity: Vec3::ZERO,
                intrinsic_velocity: self.intrinsic_velocity_at_or_before(newest.server_tick),
                stance: self.stance_at_or_before(newest.server_tick),
                aim_pitch: newest.aim_pitch,
            },
        }
    }

    /// Whether a held-newest pose means the buffer likely missed fresher samples.
    /// Stationary omitted-by-delta entities are expected to hold their last pose and
    /// must not raise the shared delay for every remote.
    fn held_newest_needs_feedback(&self, latest_accepted_server_tick: u32) -> bool {
        let Some(newest) = self.samples.back() else {
            return false;
        };
        if newest.server_tick < latest_accepted_server_tick {
            return false;
        }
        if let Some(velocity) = newest.intrinsic_velocity {
            return velocity.length_squared() > STARVATION_VELOCITY_EPSILON_SQ;
        }
        let Some(previous) = self.samples.iter().rev().nth(1) else {
            return false;
        };
        transform_changed(&previous.transform, &newest.transform)
    }

    /// Most recent replicated intrinsic velocity at or before `server_tick`.
    /// Movement-state deltas are sparse, so a Transform-only delta inherits the
    /// last authoritative movement velocity rather than becoming transform-only.
    fn intrinsic_velocity_at_or_before(&self, server_tick: u32) -> Option<Vec3> {
        self.movement_field_at_or_before(server_tick, |sample| sample.intrinsic_velocity)
    }

    /// Most recent replicated stance at or before `server_tick`, inherited the
    /// same way as [`Self::intrinsic_velocity_at_or_before`].
    fn stance_at_or_before(&self, server_tick: u32) -> LocomotionStance {
        self.movement_field_at_or_before(server_tick, |sample| sample.stance)
            .unwrap_or_default()
    }

    /// The newest movement-payload field at or before `server_tick`, never
    /// reaching back across a teleport generation.
    fn movement_field_at_or_before<T>(
        &self,
        server_tick: u32,
        field: impl Fn(&TransformSample) -> Option<T>,
    ) -> Option<T> {
        let mut samples = self
            .samples
            .iter()
            .rev()
            .filter(|sample| sample.server_tick <= server_tick);
        let newest = samples.next()?;
        let generation = newest.teleport.map(|teleport| teleport.generation);
        std::iter::once(newest)
            .chain(samples)
            .take_while(|sample| sample.teleport.map(|teleport| teleport.generation) == generation)
            .find_map(field)
    }

    fn transform_at_or_before(&self, server_tick: u32) -> Option<Transform> {
        self.samples
            .iter()
            .rev()
            .find(|sample| sample.server_tick <= server_tick)
            .map(|sample| sample.transform)
    }

    /// Resolve the intrinsic player velocity alongside the transform interpolation.
    /// Transform-only entities retain `None`; remote players linearly interpolate the
    /// two authoritative movement states, falling back to the available prior state
    /// while an out-of-order sparse delta is waiting for its predecessor.
    fn interpolated_intrinsic_velocity(
        &self,
        a: &TransformSample,
        b: &TransformSample,
        alpha: f32,
    ) -> Option<Vec3> {
        match (
            self.intrinsic_velocity_at_or_before(a.server_tick),
            self.intrinsic_velocity_at_or_before(b.server_tick),
        ) {
            (Some(a_velocity), Some(b_velocity)) => Some(a_velocity.lerp(b_velocity, alpha)),
            (Some(velocity), None) | (None, Some(velocity)) => Some(velocity),
            (None, None) => None,
        }
    }
}

/// Magnitude of a world-space vector projected onto the gameplay ground plane.
fn xz_speed(vector: Vec3) -> f32 {
    Vec3::new(vector.x, 0.0, vector.z).length()
}

/// Average horizontal speed across a pair of server-tick-stamped samples. Equal
/// tick samples are merged on insert, but retain the zero fallback as a defensive
/// guard against malformed in-memory test data.
fn xz_speed_between(a: &TransformSample, b: &TransformSample) -> f32 {
    xz_velocity_between(a, b).length()
}

/// Average world-space velocity across two samples, projected to the gameplay
/// ground plane. Kept alongside [`xz_speed_between`] so avatar lower-body heading
/// and locomotion rate derive from exactly the same presented segment.
fn xz_velocity_between(a: &TransformSample, b: &TransformSample) -> Vec3 {
    transform_velocity_between(a, b).map_or(Vec3::ZERO, |velocity| {
        Vec3::new(velocity.x, 0.0, velocity.z)
    })
}

/// Average world-space velocity across adjacent server-tick-stamped transforms.
/// Equal-tick samples are merged on insert, but `None` keeps starvation conservative
/// if malformed in-memory data bypasses that invariant.
fn transform_velocity_between(a: &TransformSample, b: &TransformSample) -> Option<Vec3> {
    if crosses_teleport(a, b) {
        return None;
    }
    let tick_span = b.server_tick.saturating_sub(a.server_tick);
    if tick_span == 0 {
        return None;
    }
    let span_secs = tick_span as f32 * crate::netcode::SERVER_TICK_MICROS as f32 / 1_000_000.0;
    Some((b.transform.position - a.transform.position) / span_secs)
}

/// Per-remote-entity interpolation buffers, keyed by `NetworkId`. Receives
/// server-tick-stamped `Transform` samples on each apply and resolves a presented
/// pose for a render target tick. Buffers are independent per entity — a sample for
/// one `NetworkId` never affects another's bracketing search.
#[derive(Debug, Default)]
pub struct RemoteInterpolationBuffer {
    buffers: HashMap<NetworkId, EntityBuffer>,
    pending_teleports: observed_teleports::PendingObservedTeleports,
}

impl RemoteInterpolationBuffer {
    /// Construct an empty buffer set. The production owner (`ClientReplication`)
    /// builds it through `Default`; this named constructor is the test entry point.
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Record a server-tick-stamped sample for `network_id`. Out-of-order and
    /// duplicate-tick arrivals are positioned/merged by tick (see [`EntityBuffer`]).
    pub(crate) fn record(&mut self, network_id: NetworkId, mut sample: TransformSample) {
        let buffer = self.buffers.entry(network_id).or_default();
        if sample.teleport.is_none() {
            sample.teleport = buffer
                .samples
                .iter()
                .rev()
                .find(|previous| previous.server_tick <= sample.server_tick)
                .and_then(|previous| previous.teleport);
        }
        if buffer.samples.back().is_some_and(|previous| {
            sample.server_tick >= previous.server_tick && crosses_teleport(previous, &sample)
        }) {
            self.pending_teleports.push(network_id, sample);
        }
        buffer.insert(sample);
    }

    /// One cue per accepted sample whose generation changed since the last
    /// drain, per pawn in acceptance order; see [`observed_teleports`].
    pub(crate) fn take_observed_teleports(
        &mut self,
        resolve: impl FnMut(NetworkId) -> Option<postretro_entities::EntityId>,
    ) -> Vec<ObservedTeleport> {
        self.pending_teleports.take(resolve)
    }

    /// Record a movement-only snapshot delta against the latest known transform.
    /// Player movement can change without a Transform delta (for example when a
    /// rider stops moving relative to a still-moving base), but avatar presentation
    /// must still observe the new intrinsic velocity and stance at that server tick.
    pub(crate) fn record_intrinsic_velocity(
        &mut self,
        network_id: NetworkId,
        server_tick: u32,
        intrinsic_velocity: Vec3,
        stance: LocomotionStance,
        aim_pitch: f32,
        teleport: Option<TeleportSample>,
    ) {
        let Some(buffer) = self.buffers.get_mut(&network_id) else {
            return;
        };
        let Some(transform) = buffer.transform_at_or_before(server_tick) else {
            return;
        };
        let sample = TransformSample {
            teleport,
            server_tick,
            transform,
            intrinsic_velocity: Some(intrinsic_velocity),
            stance: Some(stance),
            aim_pitch,
        };
        self.record(network_id, sample);
    }

    /// Drop an entity's buffer (it despawned). Idempotent.
    pub fn forget(&mut self, network_id: NetworkId) {
        self.buffers.remove(&network_id);
        self.pending_teleports.forget(network_id);
    }

    /// Resolve the presented pose for `network_id` at render server tick
    /// `target_tick`. `None` when the entity has no buffer or no samples yet (the
    /// caller leaves the entity at its last-applied pose until the first sample).
    pub(crate) fn presented_pose(
        &self,
        network_id: NetworkId,
        target_tick: f64,
    ) -> Option<PresentedPose> {
        self.buffers.get(&network_id)?.sample_at(target_tick)
    }

    /// Newest recorded authoritative transform for `network_id`, if any sample
    /// exists. The host-side delayed-presentation path (`netcode::host_presentation`)
    /// reads this to restore a client pawn's authoritative pose before each fixed
    /// tick, since the per-frame presentation write leaves the registry holding the
    /// delayed pose (which the authoritative movement/serialize reads must not see).
    pub(crate) fn newest_transform(&self, network_id: NetworkId) -> Option<Transform> {
        self.buffers
            .get(&network_id)?
            .samples
            .back()
            .map(|sample| sample.transform)
    }

    /// The newest buffered stance, for a pre-time-sync held presentation.
    pub(crate) fn newest_stance(&self, network_id: NetworkId) -> LocomotionStance {
        self.buffers
            .get(&network_id)
            .and_then(|buffer| {
                let newest = buffer.samples.back()?;
                Some(buffer.stance_at_or_before(newest.server_tick))
            })
            .unwrap_or_default()
    }

    /// Newest finite pitch for a pre-time-sync held presentation. No render tick
    /// is required because the visible transform is still the newest applied pose.
    pub(crate) fn newest_aim_pitch(&self, network_id: NetworkId) -> Option<f32> {
        self.buffers
            .get(&network_id)?
            .samples
            .back()
            .map(|sample| sample.aim_pitch)
            .filter(|pitch| pitch.is_finite())
    }

    /// True when this entity's held-newest pose should feed adaptive delay.
    pub(crate) fn held_newest_needs_feedback(
        &self,
        network_id: NetworkId,
        latest_accepted_server_tick: u32,
    ) -> bool {
        self.buffers
            .get(&network_id)
            .is_some_and(|buffer| buffer.held_newest_needs_feedback(latest_accepted_server_tick))
    }

    /// Number of buffered entities (tests / diagnostics).
    #[cfg(test)]
    pub(crate) fn entity_count(&self) -> usize {
        self.buffers.len()
    }

    /// Number of samples buffered for an entity (tests / diagnostics).
    #[cfg(test)]
    pub(crate) fn sample_count(&self, network_id: NetworkId) -> usize {
        self.buffers.get(&network_id).map_or(0, |b| b.samples.len())
    }
}

#[path = "interpolation_observed_teleports.rs"]
mod observed_teleports;

#[cfg(test)]
#[path = "interpolation_teleport_tests.rs"]
mod teleport_tests;

/// Lerp position and scale, slerp rotation, between two transforms. `alpha` in
/// `[0, 1]`; glam's `slerp` takes the shortest arc.
fn lerp_transform(a: &Transform, b: &Transform, alpha: f32) -> Transform {
    Transform {
        position: a.position.lerp(b.position, alpha),
        rotation: a.rotation.slerp(b.rotation, alpha),
        scale: a.scale.lerp(b.scale, alpha),
    }
}

fn transform_changed(a: &Transform, b: &Transform) -> bool {
    a.position != b.position || a.rotation != b.rotation || a.scale != b.scale
}

/// Host-only Phase 2 net-demo fixture: a deterministic, AI-less mover that follows a
/// parametric loop keyed on the server tick. It is *not* an authored gameplay
/// archetype and carries no script/FGD surface — it exists only so the Phase 2 net
/// demo/harness has a server-authoritative entity whose motion is smooth enough to
/// see the client-side interpolation working. Spawned and driven entirely host-side
/// and registered in the `ReplicableSet` like any other replicated object.
///
/// The path is a horizontal circle (radius `RADIUS`) traversed once every
/// `PERIOD_TICKS` server ticks, with the mover facing along its tangent. Pure
/// function of the tick, so the demo motion is identical every run.
pub(crate) struct DemoMover;

impl DemoMover {
    /// Loop radius in world units.
    const RADIUS: f32 = 4.0;
    /// Server ticks per full loop (~6 s at 60 Hz).
    const PERIOD_TICKS: f32 = 360.0;
    /// World-space center the loop orbits.
    const CENTER: Vec3 = Vec3::new(0.0, 1.0, 0.0);

    /// The deterministic pose for the demo mover at `server_tick`. Position orbits
    /// `CENTER` on the XZ plane; rotation faces the motion tangent (yaw only).
    #[must_use]
    pub(crate) fn pose_at(server_tick: u32) -> Transform {
        let theta = std::f32::consts::TAU * (server_tick as f32 / Self::PERIOD_TICKS).fract();
        let position =
            Self::CENTER + Vec3::new(Self::RADIUS * theta.cos(), 0.0, Self::RADIUS * theta.sin());
        // Tangent of the circle is perpendicular to the radius; yaw faces it.
        let yaw = theta + std::f32::consts::FRAC_PI_2;
        Transform {
            position,
            rotation: Quat::from_rotation_y(yaw),
            scale: Vec3::ONE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_net::timesync::DEFAULT_MICROS_PER_TICK;

    const POS_EPS: f32 = 1e-4;
    /// One 60 Hz frame, in seconds. The starvation-feedback tests drive
    /// `observe_sampled_frame` at this fixed delta so their tick assertions match the
    /// pre-framerate-independence behavior exactly (one frame at 60 fps == one old
    /// per-frame step / decay).
    const DT_60HZ: f64 = 1.0 / 60.0;
    /// Tolerance for server-tick float comparisons.
    const TICK_EPS: f64 = 1e-9;

    /// Wall-clock seconds for a frame spanning `ticks` server ticks.
    fn frame_dt(ticks: f64) -> f64 {
        ticks * MICROS_PER_TICK / 1_000_000.0
    }

    fn assert_lag_peak(state: &InterpolationDelayState, expected: f64) {
        let peak = state.lag_peak_ticks().expect("a lag has been folded");
        assert!(
            (peak - expected).abs() < TICK_EPS,
            "lag peak {peak} vs {expected}"
        );
    }

    fn sample(tick: u32, x: f32) -> TransformSample {
        TransformSample {
            teleport: None,
            server_tick: tick,
            transform: Transform {
                position: Vec3::new(x, 0.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            intrinsic_velocity: None,
            stance: None,
            aim_pitch: 0.0,
        }
    }

    // --- Delay law: clamp at both ends + round UP to whole ticks. ---

    #[test]
    fn delay_clamps_to_min_at_zero_jitter() {
        // Zero jitter -> 50 ms floor -> ceil(50_000 / 16_667) = 3 ticks.
        let ticks = interpolation_delay_ticks(0.0, DEFAULT_MICROS_PER_TICK);
        let expected = (MIN_DELAY_MICROS as f64 / DEFAULT_MICROS_PER_TICK as f64).ceil() as u32;
        assert_eq!(ticks, expected);
        assert_eq!(ticks, 3, "50 ms floor is 3 whole ticks at 60 Hz");
    }

    #[test]
    fn delay_clamps_to_max_at_high_jitter() {
        // Jitter large enough that 50 ms + 2×jitter exceeds the 250 ms ceiling:
        // the result clamps to 250 ms -> ceil(250_000 / 16_667) = 15 ticks.
        let huge_jitter = 1_000_000.0; // 1 s of jitter
        let ticks = interpolation_delay_ticks(huge_jitter, DEFAULT_MICROS_PER_TICK);
        let expected = (MAX_DELAY_MICROS as f64 / DEFAULT_MICROS_PER_TICK as f64).ceil() as u32;
        assert_eq!(ticks, expected);
        assert_eq!(ticks, 15, "250 ms ceiling is 15 whole ticks at 60 Hz");
    }

    #[test]
    fn delay_rounds_up_to_whole_ticks() {
        // Pick a jitter that lands the raw delay strictly between two tick
        // boundaries, then assert the result is the CEIL, not the floor/round.
        // 50 ms + 2×jitter; choose jitter = 5_000 us -> raw = 60_000 us.
        // 60_000 / 16_667 = 3.6 -> ceil = 4.
        let ticks = interpolation_delay_ticks(5_000.0, DEFAULT_MICROS_PER_TICK);
        assert_eq!(ticks, 4, "fractional tick delay rounds up");

        // A raw delay just past a tick boundary rounds up to the next whole tick.
        // jitter = 20_000 -> raw = 90_000 us; 90_000 / 16_667 = 5.4 -> 6.
        let ticks2 = interpolation_delay_ticks(20_000.0, DEFAULT_MICROS_PER_TICK);
        assert_eq!(ticks2, 6, "fractional tick delay rounds up to 6");
    }

    #[test]
    fn delay_treats_non_finite_jitter_as_zero() {
        // A NaN/negative jitter (impossible from the estimator, but defended) maps
        // to the 50 ms floor, never a garbage or panicking delay.
        let nan = interpolation_delay_ticks(f64::NAN, DEFAULT_MICROS_PER_TICK);
        let neg = interpolation_delay_ticks(-50_000.0, DEFAULT_MICROS_PER_TICK);
        assert_eq!(nan, 3);
        assert_eq!(neg, 3);
    }

    #[test]
    fn adaptive_delay_rises_after_observed_starvation() {
        let mut state = InterpolationDelayState::new();
        assert_eq!(
            state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK),
            3,
            "clean link starts at the 50 ms floor"
        );

        state.observe_sampled_frame(true, DT_60HZ);
        assert_eq!(
            state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK),
            5,
            "one starved frame adds 25 ms of temporary headroom"
        );

        for _ in 0..8 {
            state.observe_sampled_frame(true, DT_60HZ);
        }
        assert!(
            state.starvation_margin_micros() <= MAX_STARVATION_MARGIN_MICROS,
            "starvation feedback stays capped"
        );
        assert_eq!(
            state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK),
            9,
            "capped starvation feedback raises the clean-link delay to 150 ms"
        );
    }

    #[test]
    fn adaptive_delay_decays_after_clean_sampled_frames() {
        let mut state = InterpolationDelayState::new();
        for _ in 0..4 {
            state.observe_sampled_frame(true, DT_60HZ);
        }
        let raised = state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK);
        assert!(raised > 3, "starvation feedback raised delay");

        for _ in 0..64 {
            state.observe_sampled_frame(false, DT_60HZ);
        }

        assert_eq!(
            state.starvation_margin_micros(),
            0.0,
            "clean frames settle the starvation margin exactly to zero"
        );
        assert_eq!(
            state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK),
            3,
            "settled state returns to the clean-link 50 ms floor"
        );
    }

    #[test]
    fn adaptive_delay_still_clamps_to_global_max() {
        let mut state = InterpolationDelayState::new();
        for _ in 0..8 {
            state.observe_sampled_frame(true, DT_60HZ);
        }

        let ticks = state.delay_ticks(1_000_000.0, 0.0, DEFAULT_MICROS_PER_TICK);
        let expected = (MAX_DELAY_MICROS as f64 / DEFAULT_MICROS_PER_TICK as f64).ceil() as u32;
        assert_eq!(
            ticks, expected,
            "jitter plus starvation feedback cannot exceed the global ceiling"
        );
    }

    // The starvation feedback's time-constant must be wall-clock based, not per-frame:
    // a fixed wall-clock duration of starvation (then of clean frames) must produce the
    // same margin regardless of frame rate. Before this, the per-frame step/decay made a
    // 144 fps client adapt ~2.4× faster than a 60 fps one.
    #[test]
    fn starvation_feedback_is_framerate_independent() {
        const MARGIN_EPS: f64 = 1.0; // microseconds

        // Drive the same wall-clock duration of starvation at two frame rates: 60 fps
        // (dt=1/60 for N frames) vs 144 fps (dt=1/144 for the proportional 2.4N frames).
        let n_60 = 50;
        let n_144 = n_60 * 144 / 60; // same total wall-clock time (50/60 s == 120/144 s)

        let mut state_60 = InterpolationDelayState::new();
        for _ in 0..n_60 {
            state_60.observe_sampled_frame(true, 1.0 / 60.0);
        }
        let mut state_144 = InterpolationDelayState::new();
        for _ in 0..n_144 {
            state_144.observe_sampled_frame(true, 1.0 / 144.0);
        }
        assert!(
            (state_60.starvation_margin_micros() - state_144.starvation_margin_micros()).abs()
                < MARGIN_EPS,
            "rise differs by frame rate: 60fps={} 144fps={}",
            state_60.starvation_margin_micros(),
            state_144.starvation_margin_micros()
        );
        assert_eq!(
            state_60.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK),
            state_144.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK),
            "rise produces the same delay ticks at 60 and 144 fps"
        );

        // Now decay the same wall-clock duration of clean frames at both rates.
        let clean_60 = 30;
        let clean_144 = clean_60 * 144 / 60;
        for _ in 0..clean_60 {
            state_60.observe_sampled_frame(false, 1.0 / 60.0);
        }
        for _ in 0..clean_144 {
            state_144.observe_sampled_frame(false, 1.0 / 144.0);
        }
        assert!(
            (state_60.starvation_margin_micros() - state_144.starvation_margin_micros()).abs()
                < MARGIN_EPS,
            "decay differs by frame rate: 60fps={} 144fps={}",
            state_60.starvation_margin_micros(),
            state_144.starvation_margin_micros()
        );

        // A single coarse frame must equal two half-length frames covering the same time.
        let mut one_big = InterpolationDelayState::new();
        one_big.observe_sampled_frame(true, 1.0 / 30.0);
        let mut two_small = InterpolationDelayState::new();
        two_small.observe_sampled_frame(true, 1.0 / 60.0);
        two_small.observe_sampled_frame(true, 1.0 / 60.0);
        assert!(
            (one_big.starvation_margin_micros() - two_small.starvation_margin_micros()).abs()
                < MARGIN_EPS,
            "one dt=1/30 frame != two dt=1/60 frames: {} vs {}",
            one_big.starvation_margin_micros(),
            two_small.starvation_margin_micros()
        );
    }

    #[test]
    fn observe_sampled_frame_defends_pathological_dt() {
        // Non-finite and non-positive dt are no-ops: no rise, no decay, no panic.
        for bad_dt in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -1.0] {
            let mut rising = InterpolationDelayState::new();
            rising.observe_sampled_frame(true, bad_dt);
            assert_eq!(
                rising.starvation_margin_micros(),
                0.0,
                "bad dt {bad_dt} must not raise the margin"
            );

            // And on a margin already raised, a bad dt must not decay it.
            let mut settled = InterpolationDelayState::new();
            settled.observe_sampled_frame(true, DT_60HZ);
            let before = settled.starvation_margin_micros();
            settled.observe_sampled_frame(false, bad_dt);
            assert_eq!(
                settled.starvation_margin_micros(),
                before,
                "bad dt {bad_dt} must not decay the margin"
            );
        }

        // A huge dt (a hitch) is clamped: one starved frame cannot exceed the cap, and
        // one clean frame cannot decay a raised margin to zero in a single step.
        let mut hitch_rise = InterpolationDelayState::new();
        hitch_rise.observe_sampled_frame(true, 100.0);
        assert!(
            hitch_rise.starvation_margin_micros() <= MAX_STARVATION_MARGIN_MICROS,
            "a huge starved frame stays capped at the max margin"
        );

        let mut hitch_decay = InterpolationDelayState::new();
        for _ in 0..8 {
            hitch_decay.observe_sampled_frame(true, DT_60HZ);
        }
        let raised = hitch_decay.starvation_margin_micros();
        assert!(raised > 0.0);
        hitch_decay.observe_sampled_frame(false, 100.0);
        // Clamped to MAX_FEEDBACK_DT_SECS (0.25 s); decay over a quarter second leaves
        // a small but non-pathological remainder rather than annihilating the margin.
        let expected =
            (raised * STARVATION_MARGIN_DECAY_PER_SEC.powf(MAX_FEEDBACK_DT_SECS)).max(0.0);
        let after = hitch_decay.starvation_margin_micros();
        // The decayed value either matches the clamped-dt decay or snaps to zero only if
        // it fell below the epsilon; assert the clamp prevented a single-step wipe when
        // the decay result is above the snap threshold.
        if expected >= STARVATION_MARGIN_EPSILON_MICROS {
            assert!(
                (after - expected).abs() < 1.0,
                "huge clean dt clamped to 0.25 s: expected {expected}, got {after}"
            );
        }
    }

    // Regression: raising adaptive delay after held-newest starvation used to move the
    // remote render target backward when the server estimate advanced by fewer ticks
    // than the delay increase. The slewed target absorbs the rise by playing slower.
    #[test]
    fn adaptive_delay_increase_never_rewinds_render_target() {
        let mut state = InterpolationDelayState::new();

        let first =
            state.render_server_tick(100.0, frame_dt(1.0), 0.0, 0.0, DEFAULT_MICROS_PER_TICK);
        assert!((first - 97.0).abs() < TICK_EPS, "50 ms floor is 3 ticks");

        state.observe_sampled_frame(true, DT_60HZ);
        state.observe_sampled_frame(true, DT_60HZ);

        let next =
            state.render_server_tick(101.0, frame_dt(1.0), 0.0, 0.0, DEFAULT_MICROS_PER_TICK);
        assert!(
            next >= first,
            "render target must not regress after a delay increase: {next} < {first}"
        );
        let min_rate = *RENDER_TARGET_PLAYBACK_RATE.start();
        assert!(
            (next - (first + min_rate)).abs() < TICK_EPS,
            "presentation slows to the minimum playback rate while the rise is absorbed: {next}"
        );

        let caught_up =
            state.render_server_tick(104.0, frame_dt(3.0), 0.0, 0.0, DEFAULT_MICROS_PER_TICK);
        assert!(
            caught_up > next,
            "render target resumes advancing once the estimate catches up"
        );
    }

    // --- Transit term: the delay adds half the smoothed RTT. ---

    #[test]
    fn delay_at_zero_rtt_matches_buffer_only_law() {
        // LAN is unchanged: with RTT 0 the delay is exactly the buffer term, for
        // every jitter level and with starvation feedback raised.
        let mut state = InterpolationDelayState::new();
        for jitter in [0.0, 5_000.0, 20_000.0, 1_000_000.0] {
            assert_eq!(
                state.delay_ticks(jitter, 0.0, DEFAULT_MICROS_PER_TICK),
                interpolation_delay_ticks(jitter, DEFAULT_MICROS_PER_TICK),
                "jitter {jitter}"
            );
        }
        assert_eq!(state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK), 3);
        state.observe_sampled_frame(true, DT_60HZ);
        assert_eq!(
            state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK),
            5,
            "starvation feedback is unchanged at RTT 0"
        );
    }

    #[test]
    fn delay_adds_half_rtt_rounded_up_to_whole_ticks() {
        let state = InterpolationDelayState::new();
        let ticks_for = |micros: f64| (micros / DEFAULT_MICROS_PER_TICK as f64).ceil() as u32;

        // 150 ms RTT -> 75 ms transit. 50 ms + 75 ms = 125 ms = 7.5 ticks -> 8.
        let ticks = state.delay_ticks(0.0, 150_000.0, DEFAULT_MICROS_PER_TICK);
        assert_eq!(ticks, ticks_for(50_000.0 + 75_000.0));
        assert_eq!(ticks, 8);

        // Jitter 20 ms -> buffer 90 ms; + 75 ms = 165 ms = 9.9 ticks -> 10.
        assert_eq!(
            state.delay_ticks(20_000.0, 150_000.0, DEFAULT_MICROS_PER_TICK),
            10
        );

        // The clamp bounds the buffer term only: 250 ms + 75 ms = 325 ms -> 20.
        assert_eq!(
            state.delay_ticks(1_000_000.0, 150_000.0, DEFAULT_MICROS_PER_TICK),
            ticks_for(MAX_DELAY_MICROS as f64 + 75_000.0)
        );
    }

    #[test]
    fn delay_treats_non_finite_or_negative_rtt_as_zero() {
        let state = InterpolationDelayState::new();
        for rtt in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -150_000.0] {
            assert_eq!(
                state.delay_ticks(0.0, rtt, DEFAULT_MICROS_PER_TICK),
                3,
                "RTT {rtt} counts as zero"
            );
        }
    }

    // Playtest defect: the render target is measured back from the server's
    // CURRENT tick, but a snapshot stamped T lands one-way latency after T. With
    // ~75 ms one-way the buffer-only delay (50 ms) sits past the newest sample, so
    // remotes chronically extrapolate and snap back when the late sample lands.
    #[test]
    fn transit_delay_brackets_late_snapshots_in_steady_state() {
        const SNAPSHOT_INTERVAL_TICKS: u32 = 2;
        const RTT_MICROS: f64 = 150_000.0;
        // One-way transit in server ticks (75 ms = 4.5 ticks).
        let transit_ticks = RTT_MICROS / 2.0 / DEFAULT_MICROS_PER_TICK as f64;

        let id = NetworkId(1);
        let mut buf = RemoteInterpolationBuffer::new();
        let mut new_law = InterpolationDelayState::new();
        let mut old_law = InterpolationDelayState::new();
        let mut next_snapshot = 0u32;
        let mut frames = 0;

        // Client frames step the (perfectly synced) server-tick estimate by a
        // quarter tick. A snapshot is buffered once its transit has elapsed.
        for step in 0..800u32 {
            let estimated_tick = f64::from(step) * 0.25;
            while f64::from(next_snapshot) + transit_ticks <= estimated_tick {
                buf.record(id, sample(next_snapshot, next_snapshot as f32));
                next_snapshot += SNAPSHOT_INTERVAL_TICKS;
            }
            let new_target = new_law.render_server_tick(
                estimated_tick,
                frame_dt(0.25),
                0.0,
                RTT_MICROS,
                DEFAULT_MICROS_PER_TICK,
            );
            let old_target = old_law.render_server_tick(
                estimated_tick,
                frame_dt(0.25),
                0.0,
                0.0,
                DEFAULT_MICROS_PER_TICK,
            );
            if estimated_tick < 30.0 {
                continue; // warm-up: the buffer is still filling
            }
            frames += 1;

            let new_pose = buf.presented_pose(id, new_target).expect("buffered");
            assert_eq!(
                new_pose.source,
                PoseSource::Interpolated,
                "transit-aware target {new_target} must be bracketed at estimate {estimated_tick}"
            );
            assert!((new_pose.transform.position.x - new_target as f32).abs() < POS_EPS);

            let old_pose = buf.presented_pose(id, old_target).expect("buffered");
            assert_eq!(
                old_pose.source,
                PoseSource::Extrapolated,
                "reference: the buffer-only target {old_target} outruns the newest sample"
            );
        }
        assert!(frames > 500, "steady state was exercised ({frames} frames)");
    }

    // --- Measured snapshot-lag law. ---

    /// Deterministic splitmix64 stream of uniform `[0, 1)` draws.
    struct Uniform(u64);
    impl Uniform {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            (z ^ (z >> 31)) as f64 / 2f64.powi(64)
        }
    }

    const MICROS_PER_TICK: f64 = DEFAULT_MICROS_PER_TICK as f64;

    /// Harness one-way link: 45 ms + U(0, 60) ms.
    fn harness_one_way_micros(rng: &mut Uniform) -> f64 {
        45_000.0 + 60_000.0 * rng.next()
    }

    /// Smoothed (RTT, jitter) the gated time-sync estimator reports on the harness
    /// link: 5 Hz probes, each direction 45 ms + U(0, 60) ms.
    fn harness_time_sync(rng: &mut Uniform) -> (f64, f64) {
        use postretro_net::timesync::{ClockEstimator, SAMPLE_PERIOD_US, TimeSyncEcho};
        let mut est = ClockEstimator::new(DEFAULT_MICROS_PER_TICK);
        for id in 0..200u32 {
            let send_us = u64::from(id) * SAMPLE_PERIOD_US;
            let rtt = harness_one_way_micros(rng) + harness_one_way_micros(rng);
            est.record_sent(id);
            est.ingest_echo(
                &TimeSyncEcho {
                    sample_id: id,
                    client_send_tick: 0,
                    client_send_time_us: send_us,
                    server_tick: 0,
                    server_echo_time_us: 0,
                },
                send_us + rtt as u64,
            );
        }
        (est.rtt_micros(), est.jitter_micros())
    }

    // Follow-up to the transit term: the min-RTT gate admits only near-minimum
    // round trips, so rtt/2 and jitter under-read the true snapshot arrival
    // spread. Measuring each newest snapshot's lag directly covers the tail.
    #[test]
    fn measured_lag_brackets_jittered_snapshots_on_99_percent_of_frames() {
        let mut rng = Uniform(0x1502);
        let (rtt, jitter) = harness_time_sync(&mut rng);

        // 60 s of 30 Hz snapshots on the harness profile: each arrives after its
        // own one-way delay, and 5% are lost.
        let mut arrivals: Vec<(f64, u32)> = (0..1_800u32)
            .filter_map(|i| {
                let tick = i * crate::host::SNAPSHOT_TICK_INTERVAL;
                let arrival = f64::from(tick) + harness_one_way_micros(&mut rng) / MICROS_PER_TICK;
                (rng.next() >= 0.05).then_some((arrival, tick))
            })
            .collect();
        arrivals.sort_by(|a, b| a.0.total_cmp(&b.0));

        let id = NetworkId(1);
        let mut buf = RemoteInterpolationBuffer::new();
        let mut measured = InterpolationDelayState::new();
        let mut rtt_only = InterpolationDelayState::new();
        let mut next_arrival = 0;
        let mut newest: Option<u32> = None;
        let (mut frames, mut measured_hits, mut rtt_only_hits) = (0u32, 0u32, 0u32);

        // Client frames step the (perfectly synced) server-tick estimate by a
        // quarter tick; the first 3 s warm the buffer and the lag peak.
        for step in 0..14_000u32 {
            let estimated_tick = f64::from(step) * 0.25;
            while next_arrival < arrivals.len() && arrivals[next_arrival].0 <= estimated_tick {
                let tick = arrivals[next_arrival].1;
                buf.record(id, sample(tick, tick as f32));
                newest = Some(newest.map_or(tick, |n| n.max(tick)));
                next_arrival += 1;
            }
            if let Some(newest) = newest {
                measured.observe_newest_snapshot(estimated_tick, newest, DEFAULT_MICROS_PER_TICK);
            }
            let measured_target = measured.render_server_tick(
                estimated_tick,
                frame_dt(0.25),
                jitter,
                rtt,
                DEFAULT_MICROS_PER_TICK,
            );
            let rtt_only_target = rtt_only.render_server_tick(
                estimated_tick,
                frame_dt(0.25),
                jitter,
                rtt,
                DEFAULT_MICROS_PER_TICK,
            );
            if estimated_tick < 180.0 {
                continue;
            }
            frames += 1;
            let bracketed = |target: f64| {
                buf.presented_pose(id, target)
                    .is_some_and(|pose| pose.source == PoseSource::Interpolated)
            };
            measured_hits += u32::from(bracketed(measured_target));
            rtt_only_hits += u32::from(bracketed(rtt_only_target));
        }

        assert!(
            frames > 13_000,
            "steady state was exercised ({frames} frames)"
        );
        assert!(
            f64::from(measured_hits) >= 0.99 * f64::from(frames),
            "measured-lag delay bracketed {measured_hits}/{frames} frames \
             (rtt {rtt:.0} us, jitter {jitter:.0} us)"
        );
        assert!(
            rtt_only_hits < frames,
            "reference: the rtt/2 + gated-jitter law alone misses the arrival tail \
             ({rtt_only_hits}/{frames}, rtt {rtt:.0} us, jitter {jitter:.0} us)"
        );
    }

    #[test]
    fn measured_lag_at_lan_zero_matches_time_sync_law() {
        let mut state = InterpolationDelayState::new();
        state.observe_newest_snapshot(100.0, 100, DEFAULT_MICROS_PER_TICK);
        assert_lag_peak(&state, 0.0);
        for jitter in [0.0, 5_000.0, 20_000.0, 1_000_000.0] {
            assert_eq!(
                state.delay_ticks(jitter, 0.0, DEFAULT_MICROS_PER_TICK),
                interpolation_delay_ticks(jitter, DEFAULT_MICROS_PER_TICK),
                "jitter {jitter}"
            );
        }
        // A snapshot stamped ahead of the estimate (offset error) clamps to zero lag.
        state.observe_newest_snapshot(100.0, 104, DEFAULT_MICROS_PER_TICK);
        assert_eq!(state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK), 3);
    }

    #[test]
    fn measured_lag_peak_rises_instantly_then_decays_without_rewinding() {
        let mut state = InterpolationDelayState::new();
        state.observe_newest_snapshot(100.0, 98, DEFAULT_MICROS_PER_TICK);
        let mut last_target =
            state.render_server_tick(100.0, frame_dt(2.0), 0.0, 0.0, DEFAULT_MICROS_PER_TICK);

        // A lag spike of 10 ticks lifts the delay at once: ceil(10 + 2) = 12.
        state.observe_newest_snapshot(102.0, 92, DEFAULT_MICROS_PER_TICK);
        assert_lag_peak(&state, 10.0);
        assert_eq!(state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK), 12);

        // The lag then drops back to 1 tick on every snapshot. After 120 ticks (one
        // ~2 s half-life) the peak has closed half the gap: about 1 + 9 / 2 = 5.5.
        let mut tick = 102u32;
        while tick < 222 {
            tick += 2;
            state.observe_newest_snapshot(f64::from(tick), tick - 1, DEFAULT_MICROS_PER_TICK);
            let target = state.render_server_tick(
                f64::from(tick),
                frame_dt(2.0),
                0.0,
                0.0,
                DEFAULT_MICROS_PER_TICK,
            );
            assert!(
                target >= last_target,
                "render target rewound: {target} < {last_target}"
            );
            last_target = target;
        }
        let retained = 0.5_f64.powf(120.0 * MICROS_PER_TICK / LAG_PEAK_HALF_LIFE_MICROS);
        let expected = 1.0 + 9.0 * retained;
        assert!((expected - 5.5).abs() < 0.01);
        assert!(
            (state.lag_peak_ticks().unwrap() - expected).abs() < 1e-6,
            "peak after one half-life: {:?} vs {expected}",
            state.lag_peak_ticks()
        );

        // Twenty seconds on, the peak has settled to the 1-tick lag: ceil(1 + 2) = 3.
        for _ in 0..600 {
            tick += 2;
            state.observe_newest_snapshot(f64::from(tick), tick - 1, DEFAULT_MICROS_PER_TICK);
            let target = state.render_server_tick(
                f64::from(tick),
                frame_dt(2.0),
                0.0,
                0.0,
                DEFAULT_MICROS_PER_TICK,
            );
            assert!(
                target >= last_target,
                "render target rewound: {target} < {last_target}"
            );
            last_target = target;
        }
        assert_eq!(state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK), 3);
    }

    #[test]
    fn measured_lag_folds_each_snapshot_once_and_caps_wild_estimates() {
        let mut state = InterpolationDelayState::new();
        state.observe_newest_snapshot(110.0, 100, DEFAULT_MICROS_PER_TICK);
        // The same newest snapshot seen on a later frame is not re-measured.
        state.observe_newest_snapshot(130.0, 100, DEFAULT_MICROS_PER_TICK);
        assert_lag_peak(&state, 10.0);

        // A corrupt estimate far ahead is capped at 1 s of lag.
        state.observe_newest_snapshot(1.0e9, 102, DEFAULT_MICROS_PER_TICK);
        let cap = MAX_SNAPSHOT_LAG_MICROS / MICROS_PER_TICK;
        assert!((state.lag_peak_ticks().unwrap() - cap).abs() < 1e-9);
        assert_eq!(
            state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK),
            (cap + 2.0).ceil() as u32
        );

        // A non-finite estimate is ignored.
        state.observe_newest_snapshot(f64::NAN, 104, DEFAULT_MICROS_PER_TICK);
        assert!((state.lag_peak_ticks().unwrap() - cap).abs() < 1e-9);
    }

    #[test]
    fn measured_lag_resets_on_level_unload() {
        let mut state = InterpolationDelayState::new();
        state.observe_newest_snapshot(120.0, 100, DEFAULT_MICROS_PER_TICK);
        assert!(state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK) > 3);
        state.reset_for_level_unload();
        assert_eq!(state.lag_peak_ticks(), None);
        assert_eq!(state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK), 3);
        // The pre-unload tick, seen again after a long load, is not re-measured.
        state.observe_newest_snapshot(400.0, 100, DEFAULT_MICROS_PER_TICK);
        assert_eq!(state.lag_peak_ticks(), None);
        // The next level's first snapshot is measured fresh.
        state.observe_newest_snapshot(401.0, 400, DEFAULT_MICROS_PER_TICK);
        assert_lag_peak(&state, 1.0);
    }

    // Regression: replication keeps its newest snapshot tick across a level unload
    // while Settling/Frontend frames skip sampling, so the first post-load frame
    // folded the pre-unload tick against the post-load estimate. The load time read
    // as lag (capped at 1 s) and held remotes ~1 s behind for seconds.
    #[test]
    fn level_unload_folds_no_lag_until_a_post_unload_snapshot_arrives() {
        use crate::client::ClientReplication;
        use postretro_entities::EntityRegistry;
        use postretro_net::wire::SnapshotMessage;

        let empty_snapshot = |sequence: u32, server_tick: u32| SnapshotMessage {
            sequence,
            server_tick,
            records: Vec::new(),
            state_schema_fingerprint: [0; 32],
            state_records: Vec::new(),
            faction_sentiment_record: None,
            ghost_audio: None,
            server_script_time: 0.0,
        };
        let mut registry = EntityRegistry::new();
        let mut replication = ClientReplication::new();
        let mut state = InterpolationDelayState::new();

        replication.apply_snapshot(&mut registry, &empty_snapshot(5, 400));
        let newest = replication
            .latest_server_tick()
            .expect("a snapshot accepted");
        state.observe_newest_snapshot(401.0, newest, DEFAULT_MICROS_PER_TICK);
        assert_lag_peak(&state, 1.0);

        let _ = replication.reset_for_level_unload();
        state.reset_for_level_unload();

        // Three seconds of load later, the cursor still names the pre-unload tick.
        assert_eq!(replication.latest_server_tick(), Some(400));
        state.observe_newest_snapshot(580.0, 400, DEFAULT_MICROS_PER_TICK);
        assert_eq!(
            state.lag_peak_ticks(),
            None,
            "no fold of the pre-unload tick"
        );
        assert_eq!(state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK), 3);

        replication.apply_snapshot(&mut registry, &empty_snapshot(6, 580));
        let newest = replication
            .latest_server_tick()
            .expect("a snapshot accepted");
        state.observe_newest_snapshot(581.0, newest, DEFAULT_MICROS_PER_TICK);
        assert_lag_peak(&state, 1.0);
        assert_eq!(state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK), 3);
    }

    // A delay step slews the render target instead of freezing remotes on a raise
    // or jumping a whole tick on each decay crossing.
    #[test]
    fn render_target_slews_through_a_lag_spike_and_its_decay() {
        const FRAME_TICKS: f64 = 0.25;
        let (min_rate, max_rate) = (
            *RENDER_TARGET_PLAYBACK_RATE.start(),
            *RENDER_TARGET_PLAYBACK_RATE.end(),
        );
        let mut state = InterpolationDelayState::new();
        let delay = |state: &InterpolationDelayState| {
            f64::from(state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK))
        };
        // Snapshots every 2 ticks, each newest one folded 1 tick after its stamp.
        let newest_at = |estimated: f64| ((estimated as u32).saturating_sub(1)) & !1;

        let mut last_target = state.render_server_tick(
            100.0,
            frame_dt(FRAME_TICKS),
            0.0,
            0.0,
            DEFAULT_MICROS_PER_TICK,
        );
        assert!(
            (last_target - 97.0).abs() < TICK_EPS,
            "the first frame snaps"
        );
        let mut widest_gap = 0.0_f64;
        // Twenty seconds of frames; at 110 a snapshot lands 10 ticks late.
        for step in 1..=4_800u32 {
            let estimated = 100.0 + f64::from(step) * FRAME_TICKS;
            let newest = if step == 40 {
                100
            } else {
                newest_at(estimated)
            };
            state.observe_newest_snapshot(estimated, newest, DEFAULT_MICROS_PER_TICK);
            let target = state.render_server_tick(
                estimated,
                frame_dt(FRAME_TICKS),
                0.0,
                0.0,
                DEFAULT_MICROS_PER_TICK,
            );
            let advance = target - last_target;
            // A target at or past the newest folded snapshot may hold (the late
            // snapshot on frame 40 folds an older tick than the target).
            let floor_rate = if last_target >= f64::from(newest) {
                0.0
            } else {
                min_rate
            };
            assert!(
                advance >= floor_rate * FRAME_TICKS - TICK_EPS
                    && advance <= max_rate * FRAME_TICKS + TICK_EPS,
                "frame {step}: render advance {advance} outside the playback-rate band"
            );
            widest_gap = widest_gap.max((target - (estimated - delay(&state))).abs());
            last_target = target;
            if step == 40 {
                assert_lag_peak(&state, 10.0);
            }
        }
        assert!(
            widest_gap > 5.0,
            "the spike moved the candidate far enough to exercise slewing: {widest_gap}"
        );
        let estimated = 100.0 + 4_800.0 * FRAME_TICKS;
        assert_eq!(
            state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK),
            3,
            "the lag peak decayed back to the floor"
        );
        assert!(
            (last_target - (estimated - delay(&state))).abs() < TICK_EPS,
            "steady state converges to the candidate: {last_target}"
        );
    }

    // Regression: after a lasting latency rise of D ticks the target kept playing
    // at 0.75× past the newest snapshot, so remotes starved ~4(D − 2) ticks after
    // the first late snapshot landed. Holding at the newest snapshot bounds that
    // near D.
    #[test]
    fn sustained_latency_rise_starves_about_as_long_as_the_rise() {
        const FRAME_TICKS: f64 = 0.25;
        const BASE_LATENCY_TICKS: f64 = 4.5;
        const RISE_TICKS: f64 = 10.0;
        const RISE_AT_TICK: u32 = 200;
        let latency = |tick: u32| {
            if tick < RISE_AT_TICK {
                BASE_LATENCY_TICKS
            } else {
                BASE_LATENCY_TICKS + RISE_TICKS
            }
        };
        let first_late_arrival = f64::from(RISE_AT_TICK) + BASE_LATENCY_TICKS + RISE_TICKS;

        let id = NetworkId(1);
        let mut buf = RemoteInterpolationBuffer::new();
        let mut state = InterpolationDelayState::new();
        let mut next_snapshot = 0u32;
        let mut newest: Option<u32> = None;
        let mut last_target: Option<f64> = None;
        let (mut starved_ticks, mut starved_after_late_arrival) = (0.0_f64, 0.0_f64);

        // Snapshots every interval, each buffered once its latency has elapsed on
        // a perfectly synced estimate; the latency rises for snapshots stamped at
        // or after RISE_AT_TICK and stays raised.
        for step in 0..3_200u32 {
            let estimated = f64::from(step) * FRAME_TICKS;
            while f64::from(next_snapshot) + latency(next_snapshot) <= estimated {
                buf.record(id, sample(next_snapshot, next_snapshot as f32));
                newest = Some(next_snapshot);
                next_snapshot += crate::host::SNAPSHOT_TICK_INTERVAL;
            }
            if let Some(newest) = newest {
                state.observe_newest_snapshot(estimated, newest, DEFAULT_MICROS_PER_TICK);
            }
            let target = state.render_server_tick(
                estimated,
                frame_dt(FRAME_TICKS),
                0.0,
                0.0,
                DEFAULT_MICROS_PER_TICK,
            );
            if let Some(last) = last_target {
                assert!(target >= last, "render target rewound: {target} < {last}");
            }
            last_target = Some(target);
            if estimated < 40.0 {
                continue; // warm-up: the buffer and lag peak are still filling
            }

            let pose = buf.presented_pose(id, target).expect("buffered");
            assert_ne!(
                pose.source,
                PoseSource::HeldOldest,
                "target {target} fell behind the buffer at estimate {estimated}"
            );
            let starved = matches!(
                pose.source,
                PoseSource::Extrapolated | PoseSource::HeldNewest
            );
            if estimated < f64::from(RISE_AT_TICK) {
                assert!(
                    !starved,
                    "steady state before the rise is bracketed at estimate {estimated}"
                );
            }
            if starved {
                starved_ticks += FRAME_TICKS;
                if estimated >= first_late_arrival {
                    starved_after_late_arrival += FRAME_TICKS;
                }
            }
        }

        assert!(
            starved_after_late_arrival > 0.0,
            "the rise outran the buffer before the late snapshots landed"
        );
        // About RISE_TICKS − 1 after the first late arrival (the 0.75× floor gave
        // ~4 × (RISE_TICKS − 2)); the blackout before it adds about RISE_TICKS more.
        assert!(
            starved_after_late_arrival <= RISE_TICKS,
            "starved {starved_after_late_arrival} ticks after the first late snapshot"
        );
        assert!(
            starved_ticks <= 2.0 * RISE_TICKS,
            "starved {starved_ticks} ticks in all"
        );
        let estimated = 3_199.0 * FRAME_TICKS;
        let delay = f64::from(state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK));
        assert!(
            (last_target.expect("frames ran") - (estimated - delay)).abs() < TICK_EPS,
            "the target settles on the raised delay"
        );
    }

    // A time-sync reseed can step the estimate forward in one frame. Playback runs
    // on wall-clock time, so the target absorbs the jump at no more than 1.25×
    // instead of jumping remotes with it (the estimate's own advance would have
    // moved it 10.25 ticks in that frame).
    #[test]
    fn forward_estimate_jump_slews_instead_of_jumping() {
        const FRAME_TICKS: f64 = 0.25;
        const JUMP_TICKS: f64 = 10.0;
        let (min_rate, max_rate) = (
            *RENDER_TARGET_PLAYBACK_RATE.start(),
            *RENDER_TARGET_PLAYBACK_RATE.end(),
        );
        let mut state = InterpolationDelayState::new();
        let mut estimated = 100.0;
        let mut last_target = state.render_server_tick(
            estimated,
            frame_dt(FRAME_TICKS),
            0.0,
            0.0,
            DEFAULT_MICROS_PER_TICK,
        );
        let mut slewing_frames = 0u32;
        for step in 1..=400u32 {
            estimated += FRAME_TICKS;
            if step == 20 {
                estimated += JUMP_TICKS;
            }
            let target = state.render_server_tick(
                estimated,
                frame_dt(FRAME_TICKS),
                0.0,
                0.0,
                DEFAULT_MICROS_PER_TICK,
            );
            let advance = target - last_target;
            assert!(
                advance >= min_rate * FRAME_TICKS - TICK_EPS
                    && advance <= max_rate * FRAME_TICKS + TICK_EPS,
                "frame {step}: render advance {advance} outside the playback-rate band"
            );
            if (advance - max_rate * FRAME_TICKS).abs() < TICK_EPS {
                slewing_frames += 1;
            }
            last_target = target;
        }
        // Closing 10 ticks at +0.25 × 0.25 ticks per frame takes about 160 frames.
        assert!(
            (155..=165).contains(&slewing_frames),
            "the jump was absorbed over {slewing_frames} max-rate frames"
        );
        let delay = f64::from(state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK));
        assert!(
            (last_target - (estimated - delay)).abs() < TICK_EPS,
            "the target converges on the candidate: {last_target}"
        );
    }

    // A frame delta that is NaN, negative or zero plays nothing: the target
    // holds that frame and closes the gap at up to 1.25× on the frames after.
    #[test]
    fn degenerate_frame_delta_plays_nothing_then_catches_up() {
        const FRAME_TICKS: f64 = 0.25;
        let max_rate = *RENDER_TARGET_PLAYBACK_RATE.end();
        for degenerate_dt in [f64::NAN, -1.0, 0.0] {
            let mut state = InterpolationDelayState::new();
            let render = |state: &mut InterpolationDelayState, estimated: f64, dt: f64| {
                state.render_server_tick(estimated, dt, 0.0, 0.0, DEFAULT_MICROS_PER_TICK)
            };
            let first = render(&mut state, 100.0, frame_dt(FRAME_TICKS));
            let held = render(&mut state, 100.0 + FRAME_TICKS, degenerate_dt);
            assert!(
                (held - first).abs() < TICK_EPS,
                "dt {degenerate_dt}: the target played {} ticks",
                held - first
            );
            let mut estimated = 100.0 + FRAME_TICKS;
            let mut last = held;
            for frame in 0..8 {
                estimated += FRAME_TICKS;
                let target = render(&mut state, estimated, frame_dt(FRAME_TICKS));
                if frame < 4 {
                    assert!(
                        (target - last - max_rate * FRAME_TICKS).abs() < TICK_EPS,
                        "dt {degenerate_dt}: frame {frame} catches up at the max rate"
                    );
                }
                last = target;
            }
            let delay = f64::from(state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK));
            assert!(
                (last - (estimated - delay)).abs() < TICK_EPS,
                "dt {degenerate_dt}: the target caught up to the candidate: {last}"
            );
        }
    }

    // A half-second hitch plays at most the clamped quarter second (1.25× of
    // it). A gap of exactly the snap distance slews rather than snapping, and
    // what the clamp leaves slews out over the following frames.
    #[test]
    fn hitch_frame_is_clamped_and_a_gap_at_the_snap_distance_slews_out() {
        const FRAME_TICKS: f64 = 0.25;
        let (min_rate, max_rate) = (
            *RENDER_TARGET_PLAYBACK_RATE.start(),
            *RENDER_TARGET_PLAYBACK_RATE.end(),
        );
        let clamped_ticks = playback_ticks(MAX_FEEDBACK_DT_SECS, DEFAULT_MICROS_PER_TICK);
        let render = |state: &mut InterpolationDelayState, estimated: f64, dt: f64| {
            state.render_server_tick(estimated, dt, 0.0, 0.0, DEFAULT_MICROS_PER_TICK)
        };

        // Just past the snap distance, the target snaps to the candidate.
        let mut state = InterpolationDelayState::new();
        let first = render(&mut state, 100.0, frame_dt(FRAME_TICKS));
        let snapped = render(&mut state, 100.0 + RENDER_TARGET_SNAP_TICKS + 0.25, 0.5);
        assert!(
            (snapped - (first + RENDER_TARGET_SNAP_TICKS + 0.25)).abs() < TICK_EPS,
            "a gap past the snap distance snaps: {snapped}"
        );

        let mut state = InterpolationDelayState::new();
        let first = render(&mut state, 100.0, frame_dt(FRAME_TICKS));
        let mut estimated = 100.0 + RENDER_TARGET_SNAP_TICKS;
        let hitch = render(&mut state, estimated, 0.5);
        assert!(
            (hitch - first - max_rate * clamped_ticks).abs() < TICK_EPS,
            "the hitch frame advanced {} ticks, not 1.25× the clamped {clamped_ticks}",
            hitch - first
        );
        let mut last = hitch;
        for frame in 0..400 {
            estimated += FRAME_TICKS;
            let target = render(&mut state, estimated, frame_dt(FRAME_TICKS));
            let advance = target - last;
            assert!(
                advance >= min_rate * FRAME_TICKS - TICK_EPS
                    && advance <= max_rate * FRAME_TICKS + TICK_EPS,
                "frame {frame}: render advance {advance} outside the playback-rate band"
            );
            last = target;
        }
        let delay = f64::from(state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK));
        assert!(
            (last - (estimated - delay)).abs() < TICK_EPS,
            "the hitch's remainder slewed out: {last}"
        );
    }

    // Snapshots stop (the folded tick freezes) while the delay holds: the
    // target keeps tracking the candidate past the newest snapshot. The hold
    // bound only drops the floor to zero; it bites when the candidate is
    // behind the target or under 0.75× ahead of it.
    #[test]
    fn render_target_tracks_the_candidate_past_frozen_snapshots_at_a_steady_delay() {
        const FRAME_TICKS: f64 = 0.25;
        const NEWEST: u32 = 100;
        let render = |state: &mut InterpolationDelayState, estimated: f64| {
            state.render_server_tick(
                estimated,
                frame_dt(FRAME_TICKS),
                0.0,
                0.0,
                DEFAULT_MICROS_PER_TICK,
            )
        };
        let mut state = InterpolationDelayState::new();
        let mut estimated = f64::from(NEWEST) + 1.0;
        state.observe_newest_snapshot(estimated, NEWEST, DEFAULT_MICROS_PER_TICK);
        let delay = f64::from(state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK));
        let mut last = render(&mut state, estimated);
        for frame in 0..80 {
            estimated += FRAME_TICKS;
            state.observe_newest_snapshot(estimated, NEWEST, DEFAULT_MICROS_PER_TICK);
            assert_eq!(
                f64::from(state.delay_ticks(0.0, 0.0, DEFAULT_MICROS_PER_TICK)),
                delay,
                "a frozen snapshot folds no new lag"
            );
            last = render(&mut state, estimated);
            assert!(
                (last - (estimated - delay)).abs() < TICK_EPS,
                "frame {frame}: target {last} left the candidate"
            );
        }
        assert!(
            last > f64::from(NEWEST) + 10.0,
            "the target ran well past the newest snapshot: {last}"
        );

        // A candidate behind the target holds it there, with no 0.75× floor.
        estimated -= 1.0;
        let held = render(&mut state, estimated);
        assert!(
            (held - last).abs() < TICK_EPS,
            "a starved target holds: {held}"
        );
    }

    // --- Sample lookup by server tick: midpoint lands halfway. ---

    #[test]
    fn two_sample_interpolation_midpoint_lands_halfway() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        let mut first = sample(100, 0.0);
        first.aim_pitch = -0.6;
        let mut second = sample(110, 10.0);
        second.aim_pitch = 0.4;
        buf.record(id, first);
        buf.record(id, second);

        // Midpoint tick 105 -> alpha 0.5 -> x = 5.0.
        let pose = buf.presented_pose(id, 105.0).expect("bracketed");
        assert_eq!(pose.source, PoseSource::Interpolated);
        assert!((pose.transform.position.x - 5.0).abs() < POS_EPS);
        assert!((pose.aim_pitch - (-0.1)).abs() < POS_EPS);

        // A quarter of the way (tick 102.5) -> x = 2.5.
        let quarter = buf.presented_pose(id, 102.5).expect("bracketed");
        assert!((quarter.transform.position.x - 2.5).abs() < POS_EPS);
        assert!((quarter.aim_pitch - (-0.35)).abs() < POS_EPS);
    }

    #[test]
    fn interpolation_finds_a_later_adjacent_sample_pair() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        buf.record(id, sample(100, 0.0));
        buf.record(id, sample(110, 10.0));
        buf.record(id, sample(120, 30.0));

        // The target belongs to the second pair, not the first. This protects the
        // allocation-free adjacent traversal from failing to advance its left sample.
        let pose = buf.presented_pose(id, 115.0).expect("bracketed");
        assert_eq!(pose.source, PoseSource::Interpolated);
        assert!((pose.transform.position.x - 20.0).abs() < POS_EPS);
    }

    #[test]
    fn presented_speed_uses_bracketing_xz_motion_and_held_poses_are_stationary() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        // Ten metres over ten 60 Hz server ticks is roughly 60 m/s. Include a Y
        // displacement so this pins the ground-plane-only contract.
        let mut later = sample(110, 10.0);
        later.transform.position.y = 20.0;
        buf.record(id, sample(100, 0.0));
        buf.record(id, later);

        let bracketed = buf.presented_pose(id, 105.0).expect("bracketed");
        let expected = 10.0 / (10.0 * DEFAULT_MICROS_PER_TICK as f32 / 1_000_000.0);
        assert_eq!(bracketed.source, PoseSource::Interpolated);
        assert!(
            (bracketed.speed_xz - expected).abs() < POS_EPS,
            "bracketed horizontal speed {} != {expected}",
            bracketed.speed_xz
        );

        let held_oldest = buf.presented_pose(id, 90.0).expect("held oldest");
        assert_eq!(held_oldest.source, PoseSource::HeldOldest);
        assert_eq!(held_oldest.speed_xz, 0.0);

        let held_newest = buf.presented_pose(id, 200.0).expect("held newest");
        assert_eq!(held_newest.source, PoseSource::HeldNewest);
        assert_eq!(held_newest.speed_xz, 0.0);
    }

    #[test]
    fn target_at_a_sample_tick_is_exact() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        buf.record(id, sample(100, 0.0));
        buf.record(id, sample(110, 10.0));
        // Exactly on the older sample tick.
        let at_a = buf.presented_pose(id, 100.0).expect("at a");
        assert!((at_a.transform.position.x - 0.0).abs() < POS_EPS);
    }

    // --- Older-than-oldest: hold the oldest pose. ---

    #[test]
    fn target_older_than_oldest_holds_oldest() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        buf.record(id, sample(100, 3.0));
        buf.record(id, sample(110, 9.0));

        let pose = buf.presented_pose(id, 90.0).expect("has samples");
        assert_eq!(pose.source, PoseSource::HeldOldest);
        assert!((pose.transform.position.x - 3.0).abs() < POS_EPS);
    }

    // --- Extrapolation: forward by transform velocity up to exactly 100 ms, then hold. ---

    #[test]
    fn extrapolates_with_velocity_then_holds_at_cutoff() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        // Newest sample at tick 100, moving at (3, 10, 4) m/s. The vertical
        // component must not affect displayed ground-plane speed.
        let moving = TransformSample {
            teleport: None,
            server_tick: 100,
            transform: Transform {
                position: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            intrinsic_velocity: Some(Vec3::new(3.0, 10.0, 4.0)),
            stance: None,
            aim_pitch: 0.0,
        };
        let segment_secs = 3.0 * DEFAULT_MICROS_PER_TICK as f32 / 1_000_000.0;
        buf.record(
            id,
            TransformSample {
                teleport: None,
                server_tick: 97,
                transform: Transform {
                    position: -Vec3::new(3.0, 10.0, 4.0) * segment_secs,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                intrinsic_velocity: Some(Vec3::new(3.0, 10.0, 4.0)),
                stance: None,
                aim_pitch: 0.0,
            },
        );
        buf.record(id, moving);

        // 100 ms past tick 100 is the cutoff. At 60 Hz that is 6 ticks
        // (6 × 16_667 us = 100_002 us > 100_000) — so pick a tick exactly at the
        // 100 ms boundary by ticks: overshoot of 6 ticks slightly EXCEEDS 100 ms and
        // must hold. Use a sub-tick target to land just under the cutoff.
        // overshoot of 5.9 ticks = 98_335 us < 100_000 -> extrapolate.
        let within = buf.presented_pose(id, 105.9).expect("starved");
        assert_eq!(within.source, PoseSource::Extrapolated);
        // dt = 5.9 ticks × 16_667 us = 98_335 us = 0.098335 s. The x component
        // advances at 3 m/s; horizontal speed is sqrt(3² + 4²) = 5 m/s.
        let elapsed_secs = 5.9 * DEFAULT_MICROS_PER_TICK as f32 / 1_000_000.0;
        let expected_x = 3.0 * elapsed_secs;
        assert!(
            (within.transform.position.x - expected_x).abs() < POS_EPS,
            "extrapolated x {} != {}",
            within.transform.position.x,
            expected_x
        );
        assert!(
            (within.speed_xz - 5.0).abs() < POS_EPS,
            "extrapolated horizontal speed {} != 5",
            within.speed_xz
        );

        // Past the 100 ms window: hold the newest pose (x stays at 0, no extrapolation).
        let beyond = buf.presented_pose(id, 110.0).expect("starved");
        assert_eq!(beyond.source, PoseSource::HeldNewest);
        assert!((beyond.transform.position.x - 0.0).abs() < POS_EPS);
        assert_eq!(beyond.speed_xz, 0.0);
    }

    #[test]
    fn extrapolation_cutoff_is_exactly_100ms() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        let moving = TransformSample {
            teleport: None,
            server_tick: 1000,
            transform: Transform {
                position: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            intrinsic_velocity: Some(Vec3::new(2.0, 0.0, 0.0)),
            stance: None,
            aim_pitch: 0.0,
        };
        let segment_secs = 3.0 * DEFAULT_MICROS_PER_TICK as f32 / 1_000_000.0;
        buf.record(id, sample(997, -2.0 * segment_secs));
        buf.record(id, moving);

        // The cutoff in ticks: overshoot_micros == 100_000 at exactly this overshoot.
        // The boundary itself is floating-point ambiguous (division-then-multiplication
        // round-trips to ~100_000 ± epsilon), so the test exercises just-within and
        // clearly-past rather than the exact bit — the meaningful contract is "still
        // extrapolating just under the window, holding past it". The production `<=`
        // includes the exact boundary by construction.
        let cutoff_ticks = MAX_EXTRAPOLATION_MICROS / DEFAULT_MICROS_PER_TICK as f64;
        let just_within = buf
            .presented_pose(id, 1000.0 + cutoff_ticks - 0.01)
            .expect("starved");
        assert_eq!(
            just_within.source,
            PoseSource::Extrapolated,
            "just under the 100 ms window still extrapolates"
        );

        // Clearly past the cutoff holds the last pose.
        let past = buf
            .presented_pose(id, 1000.0 + cutoff_ticks + 0.1)
            .expect("starved");
        assert_eq!(past.source, PoseSource::HeldNewest);
    }

    #[test]
    fn transform_only_entity_without_transform_history_holds_on_starvation() {
        // A single transform cannot establish velocity, so starvation holds the last
        // pose even for Transform-only entities.
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        buf.record(id, sample(200, 7.0)); // velocity None

        let pose = buf.presented_pose(id, 205.0).expect("starved");
        assert_eq!(pose.source, PoseSource::HeldNewest);
        assert!((pose.transform.position.x - 7.0).abs() < POS_EPS);
    }

    #[test]
    fn transform_only_entity_extrapolates_from_transform_history() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        let segment_secs = 3.0 * DEFAULT_MICROS_PER_TICK as f32 / 1_000_000.0;
        buf.record(id, sample(200, 0.0));
        buf.record(id, sample(203, 2.0 * segment_secs));

        let pose = buf.presented_pose(id, 205.0).expect("starved");
        let elapsed_secs = 2.0 * DEFAULT_MICROS_PER_TICK as f32 / 1_000_000.0;
        assert_eq!(pose.source, PoseSource::Extrapolated);
        assert!((pose.transform.position.x - 2.0 * (segment_secs + elapsed_secs)).abs() < POS_EPS);
        assert_eq!(pose.intrinsic_velocity, None);
    }

    #[test]
    fn starvation_extrapolates_stationary_carried_rider_from_transform_history() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        let carry_velocity = 2.0;
        let segment_secs = 3.0 * DEFAULT_MICROS_PER_TICK as f32 / 1_000_000.0;
        buf.record(
            id,
            TransformSample {
                teleport: None,
                server_tick: 100,
                transform: Transform {
                    position: Vec3::ZERO,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                intrinsic_velocity: Some(Vec3::ZERO),
                stance: None,
                aim_pitch: 0.0,
            },
        );
        buf.record(
            id,
            TransformSample {
                teleport: None,
                server_tick: 103,
                transform: Transform {
                    position: Vec3::new(carry_velocity * segment_secs, 0.0, 0.0),
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                intrinsic_velocity: Some(Vec3::ZERO),
                stance: None,
                aim_pitch: 0.0,
            },
        );

        let pose = buf.presented_pose(id, 105.9).expect("starved");
        let elapsed_secs = 2.9 * DEFAULT_MICROS_PER_TICK as f32 / 1_000_000.0;
        assert_eq!(pose.source, PoseSource::Extrapolated);
        assert!(
            (pose.transform.position.x - carry_velocity * (segment_secs + elapsed_secs)).abs()
                < POS_EPS
        );
        assert!((pose.speed_xz - carry_velocity).abs() < POS_EPS);
        assert_eq!(pose.intrinsic_velocity, Some(Vec3::ZERO));
    }

    #[test]
    fn starvation_extrapolates_walking_carried_rider_from_transform_history() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        let carry_velocity = 2.0;
        let intrinsic_velocity = Vec3::new(1.0, 0.0, 0.0);
        let world_velocity = carry_velocity + intrinsic_velocity.x;
        let segment_secs = 3.0 * DEFAULT_MICROS_PER_TICK as f32 / 1_000_000.0;
        buf.record(
            id,
            TransformSample {
                teleport: None,
                server_tick: 100,
                transform: Transform {
                    position: Vec3::ZERO,
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                intrinsic_velocity: Some(intrinsic_velocity),
                stance: None,
                aim_pitch: 0.0,
            },
        );
        buf.record(
            id,
            TransformSample {
                teleport: None,
                server_tick: 103,
                transform: Transform {
                    position: Vec3::new(world_velocity * segment_secs, 0.0, 0.0),
                    rotation: Quat::IDENTITY,
                    scale: Vec3::ONE,
                },
                intrinsic_velocity: Some(intrinsic_velocity),
                stance: None,
                aim_pitch: 0.0,
            },
        );

        let pose = buf.presented_pose(id, 105.9).expect("starved");
        let elapsed_secs = 2.9 * DEFAULT_MICROS_PER_TICK as f32 / 1_000_000.0;
        assert_eq!(pose.source, PoseSource::Extrapolated);
        assert!(
            (pose.transform.position.x - world_velocity * (segment_secs + elapsed_secs)).abs()
                < POS_EPS
        );
        assert!((pose.speed_xz - world_velocity).abs() < POS_EPS);
        assert_eq!(pose.intrinsic_velocity, Some(intrinsic_velocity));
    }

    #[test]
    fn presented_footing_steps_with_the_sample_while_velocity_blends() {
        use crate::sim::{LocomotionFooting, LocomotionStance};
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        let stance = |footing| LocomotionStance {
            footing,
            ..Default::default()
        };
        let takeoff = LocomotionFooting::Airborne {
            air_ticks: 1,
            vertical_speed: 6.0,
            jumped: true,
        };
        buf.record(
            id,
            TransformSample {
                intrinsic_velocity: Some(Vec3::ZERO),
                stance: Some(stance(LocomotionFooting::Grounded)),
                ..sample(100, 0.0)
            },
        );
        buf.record(
            id,
            TransformSample {
                intrinsic_velocity: Some(Vec3::Y * 6.0),
                stance: Some(stance(takeoff)),
                ..sample(110, 0.0)
            },
        );
        // The takeoff lands on the tick the host saw it, not partway through
        // the blend toward it.
        let pose = buf.presented_pose(id, 109.0).expect("interpolated");
        assert_eq!(pose.stance.footing, LocomotionFooting::Grounded);
        assert!(pose.intrinsic_velocity.unwrap().y > 0.0);
        let pose = buf.presented_pose(id, 110.0).expect("interpolated");
        assert_eq!(pose.stance.footing, takeoff);
        // A transform-only sample inherits the last movement payload's stance.
        buf.record(id, sample(120, 0.0));
        let pose = buf.presented_pose(id, 120.0).expect("interpolated");
        assert_eq!(pose.stance.footing, takeoff);
    }

    // --- Buffer keyed by NetworkId: samples for different entities never cross. ---

    #[test]
    fn buffers_are_isolated_per_network_id() {
        let mut buf = RemoteInterpolationBuffer::new();
        let a = NetworkId(1);
        let b = NetworkId(2);
        buf.record(a, sample(100, 0.0));
        buf.record(a, sample(110, 10.0));
        buf.record(b, sample(100, 100.0));
        buf.record(b, sample(110, 200.0));

        assert_eq!(buf.entity_count(), 2);
        // Entity A midpoint -> 5.0; entity B midpoint -> 150.0. No crossover.
        let pa = buf.presented_pose(a, 105.0).expect("a");
        let pb = buf.presented_pose(b, 105.0).expect("b");
        assert!((pa.transform.position.x - 5.0).abs() < POS_EPS);
        assert!((pb.transform.position.x - 150.0).abs() < POS_EPS);

        // Forgetting A leaves B intact.
        buf.forget(a);
        assert_eq!(buf.entity_count(), 1);
        assert!(buf.presented_pose(a, 105.0).is_none());
        assert!(buf.presented_pose(b, 105.0).is_some());
    }

    #[test]
    fn empty_buffer_yields_no_pose() {
        let buf = RemoteInterpolationBuffer::new();
        assert!(buf.presented_pose(NetworkId(1), 100.0).is_none());
    }

    // --- Out-of-order and duplicate arrivals are tick-ordered/merged. ---

    #[test]
    fn out_of_order_arrival_is_inserted_by_tick() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        // Arrive 110 before 100 (reordered on the unreliable channel).
        buf.record(id, sample(110, 10.0));
        buf.record(id, sample(100, 0.0));
        // The bracketing search still finds the right pair: midpoint -> 5.0.
        let pose = buf.presented_pose(id, 105.0).expect("bracketed");
        assert_eq!(pose.source, PoseSource::Interpolated);
        assert!((pose.transform.position.x - 5.0).abs() < POS_EPS);
        assert_eq!(buf.sample_count(id), 2);
    }

    #[test]
    fn duplicate_tick_replaces_latest_wins() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        buf.record(id, sample(100, 0.0));
        buf.record(id, sample(100, 5.0)); // same tick, newer value
        assert_eq!(buf.sample_count(id), 1, "duplicate tick merged");
        // The held-oldest path returns the replaced value.
        let pose = buf.presented_pose(id, 100.0).expect("has sample");
        assert!((pose.transform.position.x - 5.0).abs() < POS_EPS);
    }

    #[test]
    fn buffer_is_bounded_evicting_oldest() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        for tick in 0..(MAX_SAMPLES_PER_ENTITY as u32 + 5) {
            buf.record(id, sample(tick, tick as f32));
        }
        assert_eq!(
            buf.sample_count(id),
            MAX_SAMPLES_PER_ENTITY,
            "buffer never exceeds its cap"
        );
    }

    // --- Continuity: stepping the render target forward across a sample boundary
    // produces a monotonically advancing position (no jump at the seam). ---

    #[test]
    fn stepping_target_across_boundary_is_continuous() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        // Three colinear samples 10 ticks apart, x advancing by 10 each.
        buf.record(id, sample(100, 0.0));
        buf.record(id, sample(110, 10.0));
        buf.record(id, sample(120, 20.0));

        // Walk the target from 100 to 120 in small steps; x must be non-decreasing
        // and land on the analytic value (x == target - 100).
        let mut prev_x = f32::NEG_INFINITY;
        let mut t = 100.0;
        while t <= 120.0 {
            let pose = buf.presented_pose(id, t).expect("bracketed");
            let x = pose.transform.position.x;
            assert!(x + POS_EPS >= prev_x, "position regressed across the seam");
            assert!(
                (x - (t as f32 - 100.0)).abs() < 1e-3,
                "x {} != analytic {}",
                x,
                t as f32 - 100.0
            );
            prev_x = x;
            t += 1.3;
        }
    }

    // --- Demo mover: deterministic, on the loop, distinct poses around the path. ---

    #[test]
    fn demo_mover_path_is_deterministic_and_on_radius() {
        // Same tick -> identical pose (pure function of the tick).
        let a = DemoMover::pose_at(42);
        let b = DemoMover::pose_at(42);
        assert_eq!(a, b);

        // The position stays on the loop radius around CENTER on the XZ plane.
        let offset = a.position - DemoMover::CENTER;
        let radial = (offset.x * offset.x + offset.z * offset.z).sqrt();
        assert!((radial - DemoMover::RADIUS).abs() < POS_EPS);
        assert!(offset.y.abs() < POS_EPS, "loop is planar (XZ)");

        // A quarter period later the pose differs (the mover actually moves).
        let quarter = DemoMover::pose_at(42 + (DemoMover::PERIOD_TICKS as u32 / 4));
        assert!((quarter.position - a.position).length() > 1.0);

        // A full period later returns to (approximately) the same point.
        let full = DemoMover::pose_at(42 + DemoMover::PERIOD_TICKS as u32);
        assert!((full.position - a.position).length() < POS_EPS);
    }
}
