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
//     newest pose using its last-known velocity for at most `MAX_EXTRAPOLATION` of
//     server time, then HOLD the newest pose. Velocity is only present when the
//     wire carried a movement payload; the Phase 2 dumb mover is `Transform`-only,
//     so its starvation path is hold-immediately (no velocity to extrapolate with).

use std::collections::HashMap;
use std::collections::VecDeque;

use glam::{Quat, Vec3};

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
const MAX_DELAY_MICROS: u64 = 250_000;
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
/// feedback. A hitch (long GC pause, alt-tab, breakpoint) would otherwise let one
/// frame add a pathological amount of margin or decay it fully to zero; clamping to
/// 0.25 s bounds the per-frame influence to a quarter-second of adaptation.
const MAX_FEEDBACK_DT_SECS: f64 = 0.25;
/// Below this, the starvation margin is treated as settled and snapped to zero.
const STARVATION_MARGIN_EPSILON_MICROS: f64 = 500.0;
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

/// Base interpolation delay in **whole sim ticks**, sized from measured link jitter.
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
    adaptive_interpolation_delay_ticks(jitter_micros, 0.0, micros_per_tick)
}

/// Client-side feedback state for the remote interpolation delay.
///
/// Time-sync jitter is the feed-forward signal. Buffer starvation is the feedback
/// signal: if remotes had to hold their newest pose, the current delay was too tight
/// for recent traffic, so subsequent frames temporarily add headroom.
///
/// Production delay is:
/// `clamp(50 ms + 2 × jitter + starvation_margin, 50 ms, 250 ms)`,
/// rounded up to whole sim ticks.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct InterpolationDelayState {
    starvation_margin_micros: f64,
    last_render_server_tick: Option<f64>,
}

impl InterpolationDelayState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Drop level-scoped delay feedback after remote interpolation buffers are reset.
    pub(crate) fn reset_for_level_unload(&mut self) {
        *self = Self::default();
    }

    /// Delay in whole sim ticks using both measured jitter and recent starvation.
    pub(crate) fn delay_ticks(&self, jitter_micros: f64, micros_per_tick: u64) -> u32 {
        adaptive_interpolation_delay_ticks(
            jitter_micros,
            self.starvation_margin_micros,
            micros_per_tick,
        )
    }

    /// Render target tick for the remote interpolation buffers. Delay changes may
    /// pause remote presentation while the server estimate catches up, but never
    /// move the sampled server time backward.
    pub(crate) fn render_server_tick(
        &mut self,
        estimated_tick: f64,
        jitter_micros: f64,
        micros_per_tick: u64,
    ) -> f64 {
        let delay_ticks = self.delay_ticks(jitter_micros, micros_per_tick);
        let candidate = estimated_tick - f64::from(delay_ticks);
        let render_tick = self
            .last_render_server_tick
            .map_or(candidate, |last| candidate.max(last));
        self.last_render_server_tick = Some(render_tick);
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
}

fn adaptive_interpolation_delay_ticks(
    jitter_micros: f64,
    starvation_margin_micros: f64,
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
    // Compute the clamped delay in microseconds, then round UP to whole ticks.
    let raw_micros = MIN_DELAY_MICROS as f64 + JITTER_MULTIPLIER * jitter + starvation_margin;
    let clamped = raw_micros.clamp(MIN_DELAY_MICROS as f64, MAX_DELAY_MICROS as f64);
    let ticks = (clamped / micros_per_tick as f64).ceil();
    // `clamped` is bounded by MAX_DELAY_MICROS, so this cast never saturates u32.
    ticks as u32
}

/// One buffered remote pose, stamped by the server tick it was valid at. `velocity`
/// is the last-known world-space velocity from a movement payload, used only for
/// bounded forward extrapolation on starvation; `None` for `Transform`-only entities
/// (the Phase 2 dumb mover), whose starvation path holds immediately.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TransformSample {
    pub(crate) server_tick: u32,
    pub(crate) transform: Transform,
    pub(crate) velocity: Option<Vec3>,
}

/// The pose the state machine resolved for a render target, plus which branch
/// produced it. The branch tag is observable so tests assert the interpolate /
/// extrapolate / hold transitions directly (testing_guide §Behavior over
/// implementation: the branch *is* the contract here, not an internal detail).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PresentedPose {
    pub(crate) transform: Transform,
    pub(crate) source: PoseSource,
}

/// Which branch of the sample state machine produced a [`PresentedPose`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PoseSource {
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
                transform: oldest.transform,
                source: PoseSource::HeldOldest,
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
        for window in self.samples.iter().collect::<Vec<_>>().windows(2) {
            let (a, b) = (window[0], window[1]);
            if f64::from(a.server_tick) <= target_tick && target_tick <= f64::from(b.server_tick) {
                let span = f64::from(b.server_tick) - f64::from(a.server_tick);
                // Equal-tick neighbors (span 0) cannot happen — insert dedupes ticks —
                // but guard the divide rather than risk a NaN alpha.
                let alpha = if span > 0.0 {
                    ((target_tick - f64::from(a.server_tick)) / span) as f32
                } else {
                    0.0
                };
                return Some(PresentedPose {
                    transform: lerp_transform(&a.transform, &b.transform, alpha),
                    source: PoseSource::Interpolated,
                });
            }
        }

        // Unreachable: target is strictly between oldest and newest, so some adjacent
        // pair must bracket it. Fall back to the newest pose rather than panic.
        Some(PresentedPose {
            transform: newest.transform,
            source: PoseSource::HeldNewest,
        })
    }

    /// Starvation branch: the render target is at/after the newest sample. If the
    /// newest sample carried a velocity and the overshoot is within the bounded
    /// window, extrapolate position forward (rotation/scale held — no angular wire
    /// velocity to extrapolate); otherwise hold the newest pose.
    fn extrapolate_or_hold(&self, newest: &TransformSample, target_tick: f64) -> PresentedPose {
        let overshoot_ticks = target_tick - f64::from(newest.server_tick);
        let overshoot_micros = overshoot_ticks * crate::netcode::SERVER_TICK_MICROS as f64;

        match newest.velocity {
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
                    transform: predicted,
                    source: PoseSource::Extrapolated,
                }
            }
            // No velocity (Transform-only mover) or past the extrapolation window:
            // hold the last known pose.
            _ => PresentedPose {
                transform: newest.transform,
                source: PoseSource::HeldNewest,
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
        if let Some(velocity) = newest.velocity {
            return velocity.length_squared() > STARVATION_VELOCITY_EPSILON_SQ;
        }
        let Some(previous) = self.samples.iter().rev().nth(1) else {
            return false;
        };
        transform_changed(&previous.transform, &newest.transform)
    }
}

/// Per-remote-entity interpolation buffers, keyed by `NetworkId`. Receives
/// server-tick-stamped `Transform` samples on each apply and resolves a presented
/// pose for a render target tick. Buffers are independent per entity — a sample for
/// one `NetworkId` never affects another's bracketing search.
#[derive(Debug, Default)]
pub(crate) struct RemoteInterpolationBuffer {
    buffers: HashMap<NetworkId, EntityBuffer>,
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
    pub(crate) fn record(&mut self, network_id: NetworkId, sample: TransformSample) {
        self.buffers.entry(network_id).or_default().insert(sample);
    }

    /// Drop an entity's buffer (it despawned). Idempotent.
    pub(crate) fn forget(&mut self, network_id: NetworkId) {
        self.buffers.remove(&network_id);
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

    fn sample(tick: u32, x: f32) -> TransformSample {
        TransformSample {
            server_tick: tick,
            transform: Transform {
                position: Vec3::new(x, 0.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            velocity: None,
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
            state.delay_ticks(0.0, DEFAULT_MICROS_PER_TICK),
            3,
            "clean link starts at the 50 ms floor"
        );

        state.observe_sampled_frame(true, DT_60HZ);
        assert_eq!(
            state.delay_ticks(0.0, DEFAULT_MICROS_PER_TICK),
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
            state.delay_ticks(0.0, DEFAULT_MICROS_PER_TICK),
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
        let raised = state.delay_ticks(0.0, DEFAULT_MICROS_PER_TICK);
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
            state.delay_ticks(0.0, DEFAULT_MICROS_PER_TICK),
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

        let ticks = state.delay_ticks(1_000_000.0, DEFAULT_MICROS_PER_TICK);
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
            state_60.delay_ticks(0.0, DEFAULT_MICROS_PER_TICK),
            state_144.delay_ticks(0.0, DEFAULT_MICROS_PER_TICK),
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
    // than the delay increase.
    #[test]
    fn adaptive_delay_increase_never_rewinds_render_target() {
        let mut state = InterpolationDelayState::new();

        let first = state.render_server_tick(100.0, 0.0, DEFAULT_MICROS_PER_TICK);
        assert_eq!(first, 97.0, "50 ms floor is 3 ticks");

        state.observe_sampled_frame(true, DT_60HZ);
        state.observe_sampled_frame(true, DT_60HZ);

        let next = state.render_server_tick(101.0, 0.0, DEFAULT_MICROS_PER_TICK);
        assert!(
            next >= first,
            "render target must not regress after a delay increase: {next} < {first}"
        );
        assert_eq!(
            next, first,
            "presentation may pause while the server estimate catches up"
        );

        let caught_up = state.render_server_tick(104.0, 0.0, DEFAULT_MICROS_PER_TICK);
        assert!(
            caught_up > next,
            "render target resumes advancing once the estimate catches up"
        );
    }

    // --- Sample lookup by server tick: midpoint lands halfway. ---

    #[test]
    fn two_sample_interpolation_midpoint_lands_halfway() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        buf.record(id, sample(100, 0.0));
        buf.record(id, sample(110, 10.0));

        // Midpoint tick 105 -> alpha 0.5 -> x = 5.0.
        let pose = buf.presented_pose(id, 105.0).expect("bracketed");
        assert_eq!(pose.source, PoseSource::Interpolated);
        assert!((pose.transform.position.x - 5.0).abs() < POS_EPS);

        // A quarter of the way (tick 102.5) -> x = 2.5.
        let quarter = buf.presented_pose(id, 102.5).expect("bracketed");
        assert!((quarter.transform.position.x - 2.5).abs() < POS_EPS);
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

    // --- Extrapolation: forward by velocity up to exactly 100 ms, then hold. ---

    #[test]
    fn extrapolates_with_velocity_then_holds_at_cutoff() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        // Newest sample at tick 100, x=0, moving +1 m/s along x.
        let moving = TransformSample {
            server_tick: 100,
            transform: Transform {
                position: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            velocity: Some(Vec3::new(1.0, 0.0, 0.0)),
        };
        // A prior sample so the buffer is not single-sample (does not change the
        // starvation branch, but mirrors real traffic).
        buf.record(id, sample(97, -3.0));
        buf.record(id, moving);

        // 100 ms past tick 100 is the cutoff. At 60 Hz that is 6 ticks
        // (6 × 16_667 us = 100_002 us > 100_000) — so pick a tick exactly at the
        // 100 ms boundary by ticks: overshoot of 6 ticks slightly EXCEEDS 100 ms and
        // must hold. Use a sub-tick target to land just under the cutoff.
        // overshoot of 5.9 ticks = 98_335 us < 100_000 -> extrapolate.
        let within = buf.presented_pose(id, 105.9).expect("starved");
        assert_eq!(within.source, PoseSource::Extrapolated);
        // dt = 5.9 ticks × 16_667 us = 98_335 us = 0.098335 s -> x ≈ 0.098335.
        let expected_x = 5.9 * DEFAULT_MICROS_PER_TICK as f32 / 1_000_000.0;
        assert!(
            (within.transform.position.x - expected_x).abs() < POS_EPS,
            "extrapolated x {} != {}",
            within.transform.position.x,
            expected_x
        );

        // Past the 100 ms window: hold the newest pose (x stays at 0, no extrapolation).
        let beyond = buf.presented_pose(id, 110.0).expect("starved");
        assert_eq!(beyond.source, PoseSource::HeldNewest);
        assert!((beyond.transform.position.x - 0.0).abs() < POS_EPS);
    }

    #[test]
    fn extrapolation_cutoff_is_exactly_100ms() {
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        let moving = TransformSample {
            server_tick: 1000,
            transform: Transform {
                position: Vec3::ZERO,
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
            velocity: Some(Vec3::new(2.0, 0.0, 0.0)),
        };
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
    fn transform_only_mover_holds_immediately_on_starvation() {
        // The Phase 2 dumb mover is Transform-only (velocity None): starvation holds
        // the last pose immediately, never extrapolates.
        let mut buf = RemoteInterpolationBuffer::new();
        let id = NetworkId(1);
        buf.record(id, sample(200, 7.0)); // velocity None

        let pose = buf.presented_pose(id, 205.0).expect("starved");
        assert_eq!(pose.source, PoseSource::HeldNewest);
        assert!((pose.transform.position.x - 7.0).abs() < POS_EPS);
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
