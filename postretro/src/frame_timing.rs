// Fixed-timestep accumulator and interpolable game state.
// See: context/lib/rendering_pipeline.md §1

use glam::{Mat4, Vec3};
use std::time::{Duration, Instant};

use crate::camera;

/// Fixed tick duration: 60 Hz (16.667ms per tick).
const TICK_DURATION: Duration = Duration::from_micros(16_667);

/// Maximum accumulator value to prevent spiral-of-death catch-up after stalls.
const MAX_ACCUMULATOR: Duration = Duration::from_millis(250);

/// Snapshot of game state that the renderer interpolates between frames.
/// Plain struct — no traits, generics, or macros.
pub struct InterpolableState {
    pub position: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

impl InterpolableState {
    pub fn new(position: Vec3, yaw: f32, pitch: f32) -> Self {
        Self {
            position,
            yaw,
            pitch,
        }
    }

    /// Linearly interpolate position and pitch; shortest-path angular lerp for yaw.
    pub fn lerp(&self, other: &InterpolableState, alpha: f32) -> InterpolableState {
        InterpolableState {
            position: self.position.lerp(other.position, alpha),
            yaw: lerp_angle(self.yaw, other.yaw, alpha),
            pitch: self.pitch + (other.pitch - self.pitch) * alpha,
        }
    }

    /// Build a view-projection matrix from this interpolated state and a Camera's
    /// aspect ratio / projection settings. We recompute the matrix from scratch
    /// rather than interpolating matrices (which doesn't produce correct results).
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        let look_dir = Vec3::new(
            -self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            -self.yaw.cos() * self.pitch.cos(),
        );
        let target = self.position + look_dir;
        let view = Mat4::look_at_rh(self.position, target, Vec3::Y);

        // Clamp aspect to avoid degenerate projection (near-zero aspect produces
        // vfov near PI, which makes tan(vfov/2) explode).
        let safe_aspect = aspect.max(0.1);
        let vfov = 2.0 * ((camera::HFOV / 2.0).tan() / safe_aspect).atan();
        let projection = Mat4::perspective_rh(vfov, safe_aspect, camera::NEAR, camera::FAR);

        projection * view
    }
}

/// Fixed-timestep accumulator. Tracks wall-clock time and ticks game logic
/// at a constant rate, independent of render framerate.
pub struct FrameTiming {
    pub accumulator: Duration,
    pub tick_duration: Duration,
    pub previous_state: InterpolableState,
    pub current_state: InterpolableState,
    pub last_frame: Instant,
    first_tick_done: bool,
}

impl FrameTiming {
    pub fn new(initial_state: InterpolableState) -> Self {
        // Duplicate initial state so interpolation on the first frame
        // produces the initial state with no blending artifact.
        let previous = InterpolableState::new(
            initial_state.position,
            initial_state.yaw,
            initial_state.pitch,
        );
        Self {
            accumulator: Duration::ZERO,
            tick_duration: TICK_DURATION,
            previous_state: previous,
            current_state: initial_state,
            last_frame: Instant::now(),
            first_tick_done: false,
        }
    }

    /// Call at the start of each frame. Returns the number of ticks that ran
    /// and the interpolation alpha for rendering.
    pub fn begin_frame(&mut self, now: Instant) -> FrameTickResult {
        let elapsed = now.duration_since(self.last_frame);
        self.last_frame = now;
        self.accumulate(elapsed)
    }

    /// Accumulate a duration and return tick count + alpha. Separated from
    /// `begin_frame` for testability with deterministic durations.
    pub fn accumulate(&mut self, elapsed: Duration) -> FrameTickResult {
        // Zero-time frame: skip ticking, render with previous alpha.
        if elapsed.is_zero() {
            return FrameTickResult {
                ticks: 0,
                alpha: self.current_alpha(),
            };
        }

        self.accumulator += elapsed;

        // Clamp to prevent spiral-of-death after long stalls.
        if self.accumulator > MAX_ACCUMULATOR {
            self.accumulator = MAX_ACCUMULATOR;
        }

        let mut ticks = 0u32;
        while self.accumulator >= self.tick_duration {
            self.accumulator -= self.tick_duration;
            ticks += 1;
        }

        FrameTickResult {
            ticks,
            alpha: self.current_alpha(),
        }
    }

    /// Swap current state into previous, write new current state.
    /// Called once per tick from the game logic.
    pub fn push_state(&mut self, new_state: InterpolableState) {
        self.previous_state = InterpolableState::new(
            self.current_state.position,
            self.current_state.yaw,
            self.current_state.pitch,
        );
        self.current_state = new_state;
        self.first_tick_done = true;
    }

    /// Compute the interpolated state for rendering.
    pub fn interpolated_state(&self) -> InterpolableState {
        self.previous_state
            .lerp(&self.current_state, self.current_alpha())
    }

    fn current_alpha(&self) -> f32 {
        if !self.first_tick_done {
            // Before any tick has run, return the initial state (alpha = 1.0
            // means "fully current_state", and both states are identical).
            return 1.0;
        }
        let tick_secs = self.tick_duration.as_secs_f32();
        if tick_secs == 0.0 {
            return 1.0;
        }
        self.accumulator.as_secs_f32() / tick_secs
    }

    /// The fixed tick duration as seconds, for use in game logic.
    pub fn tick_dt(&self) -> f32 {
        self.tick_duration.as_secs_f32()
    }
}

pub struct FrameTickResult {
    pub ticks: u32,
    /// Interpolation factor: 0.0 = previous state, 1.0 = current state.
    /// Available for callers that need direct access; `FrameTiming::interpolated_state`
    /// uses this internally.
    #[allow(dead_code)]
    pub alpha: f32,
}

/// Shortest-path angular interpolation for angles in radians.
/// Wraps the difference to [-PI, PI] before lerping.
pub fn lerp_angle(from: f32, to: f32, alpha: f32) -> f32 {
    let mut diff = to - from;
    // Wrap to [-PI, PI]
    diff = diff - (diff / std::f32::consts::TAU).round() * std::f32::consts::TAU;
    from + diff * alpha
}

// --- CPU frametime measurement ---

/// Number of frame samples retained in the ring buffer. 120 samples at 60fps
/// is two seconds of history — enough to smooth out jitter for averaging and
/// wide enough to catch recent hitches in the max.
const FRAMETIME_RING_SIZE: usize = 120;

/// Fixed-size ring buffer of recent per-frame CPU times. The buffer is a
/// stack-allocated array; `record()` and `stats()` never allocate, which is
/// the same invariant `App::scratch_ranges` enforces for visibility work.
pub struct FrameRateMeter {
    samples: [Duration; FRAMETIME_RING_SIZE],
    /// Index where the next sample will be written.
    head: usize,
    /// Number of valid samples, saturating at `FRAMETIME_RING_SIZE`.
    filled: usize,
}

/// Aggregated frame-time statistics over the ring buffer window, in
/// milliseconds. Reported as min/avg/max because averages hide hitches:
/// a 50ms spike in a 2-second window disappears into a 17ms average.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrametimeStats {
    pub min_ms: f32,
    pub avg_ms: f32,
    pub max_ms: f32,
}

impl FrameRateMeter {
    pub fn new() -> Self {
        Self {
            samples: [Duration::ZERO; FRAMETIME_RING_SIZE],
            head: 0,
            filled: 0,
        }
    }

    /// Record a single frame's measured CPU span. Called every frame.
    pub fn record(&mut self, frame_cpu_time: Duration) {
        self.samples[self.head] = frame_cpu_time;
        self.head = (self.head + 1) % self.samples.len();
        if self.filled < self.samples.len() {
            self.filled += 1;
        }
    }

    /// Drop all recorded samples. Used when a runtime diagnostic (e.g. the
    /// vsync toggle) invalidates the window's contents — holding onto the
    /// old samples would mean stats() keeps reporting the pre-toggle
    /// frametimes for two full seconds after the change.
    ///
    /// The sample array itself isn't zeroed: `filled == 0` is the sole
    /// signal `stats()` reads, and `record()` overwrites entries in place.
    pub fn clear(&mut self) {
        self.head = 0;
        self.filled = 0;
    }

    /// Compute min/avg/max over the filled portion of the ring buffer.
    /// Returns `None` if no samples have been recorded yet. O(N) over the
    /// window; intended to be called at title-update cadence (~4Hz), not
    /// per-frame.
    pub fn stats(&self) -> Option<FrametimeStats> {
        if self.filled == 0 {
            return None;
        }
        let slice = &self.samples[..self.filled];
        let mut min = slice[0];
        let mut max = slice[0];
        let mut total_secs = 0.0f64;
        for &s in slice {
            if s < min {
                min = s;
            }
            if s > max {
                max = s;
            }
            total_secs += s.as_secs_f64();
        }
        let avg_secs = total_secs / slice.len() as f64;
        Some(FrametimeStats {
            min_ms: (min.as_secs_f64() * 1000.0) as f32,
            avg_ms: (avg_secs * 1000.0) as f32,
            max_ms: (max.as_secs_f64() * 1000.0) as f32,
        })
    }
}

impl Default for FrameRateMeter {
    fn default() -> Self {
        Self::new()
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const EPSILON: f32 = 1e-4;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < EPSILON
    }

    fn assert_approx(actual: f32, expected: f32, msg: &str) {
        assert!(
            approx_eq(actual, expected),
            "{msg}: expected {expected:.5}, got {actual:.5}"
        );
    }

    // -- lerp_angle --

    #[test]
    fn lerp_angle_returns_start_at_alpha_zero() {
        let result = lerp_angle(1.0, 2.0, 0.0);
        assert_approx(result, 1.0, "alpha=0 should return start");
    }

    #[test]
    fn lerp_angle_returns_end_at_alpha_one() {
        let result = lerp_angle(1.0, 2.0, 1.0);
        assert_approx(result, 2.0, "alpha=1 should return end");
    }

    #[test]
    fn lerp_angle_returns_midpoint_at_alpha_half() {
        let result = lerp_angle(0.0, 1.0, 0.5);
        assert_approx(result, 0.5, "alpha=0.5 should return midpoint");
    }

    #[test]
    fn lerp_angle_takes_shortest_path_across_positive_wrap() {
        // From nearly 2*PI to just past 0 — should go forward, not backward.
        let from = 2.0 * PI - 0.1;
        let to = 0.1;
        let result = lerp_angle(from, to, 0.5);
        // Midpoint should be near 2*PI (i.e., near 0), not near PI.
        let normalized = result.rem_euclid(2.0 * PI);
        assert!(
            normalized > 2.0 * PI - 0.2 || normalized < 0.2,
            "should be near 0/2PI, got {normalized:.5}"
        );
    }

    #[test]
    fn lerp_angle_takes_shortest_path_across_negative_wrap() {
        // From just past 0 to nearly 2*PI — should go backward.
        let from = 0.1;
        let to = 2.0 * PI - 0.1;
        let result = lerp_angle(from, to, 0.5);
        let normalized = result.rem_euclid(2.0 * PI);
        assert!(
            normalized > 2.0 * PI - 0.2 || normalized < 0.2,
            "should be near 0/2PI, got {normalized:.5}"
        );
    }

    #[test]
    fn lerp_angle_handles_identical_angles() {
        let result = lerp_angle(1.5, 1.5, 0.5);
        assert_approx(result, 1.5, "identical angles should return that angle");
    }

    #[test]
    fn lerp_angle_handles_opposite_angles() {
        // PI apart — either direction is equally short. Just verify no NaN.
        let result = lerp_angle(0.0, PI, 0.5);
        assert!(result.is_finite(), "result should be finite");
    }

    // -- InterpolableState::lerp --

    #[test]
    fn interpolable_state_lerp_returns_start_at_alpha_zero() {
        let a = InterpolableState::new(Vec3::new(0.0, 0.0, 0.0), 0.0, 0.0);
        let b = InterpolableState::new(Vec3::new(10.0, 20.0, 30.0), 1.0, 0.5);
        let result = a.lerp(&b, 0.0);
        assert_approx(result.position.x, 0.0, "position.x at alpha=0");
        assert_approx(result.yaw, 0.0, "yaw at alpha=0");
        assert_approx(result.pitch, 0.0, "pitch at alpha=0");
    }

    #[test]
    fn interpolable_state_lerp_returns_end_at_alpha_one() {
        let a = InterpolableState::new(Vec3::new(0.0, 0.0, 0.0), 0.0, 0.0);
        let b = InterpolableState::new(Vec3::new(10.0, 20.0, 30.0), 1.0, 0.5);
        let result = a.lerp(&b, 1.0);
        assert_approx(result.position.x, 10.0, "position.x at alpha=1");
        assert_approx(result.yaw, 1.0, "yaw at alpha=1");
        assert_approx(result.pitch, 0.5, "pitch at alpha=1");
    }

    #[test]
    fn interpolable_state_lerp_interpolates_position_linearly() {
        let a = InterpolableState::new(Vec3::new(0.0, 0.0, 0.0), 0.0, 0.0);
        let b = InterpolableState::new(Vec3::new(100.0, 200.0, 300.0), 0.0, 0.0);
        let result = a.lerp(&b, 0.25);
        assert_approx(result.position.x, 25.0, "position.x at alpha=0.25");
        assert_approx(result.position.y, 50.0, "position.y at alpha=0.25");
        assert_approx(result.position.z, 75.0, "position.z at alpha=0.25");
    }

    // -- FrameTiming: accumulator --

    #[test]
    fn accumulator_produces_one_tick_for_one_tick_duration() {
        let state = InterpolableState::new(Vec3::ZERO, 0.0, 0.0);
        let mut timing = FrameTiming::new(state);
        let result = timing.accumulate(TICK_DURATION);
        assert_eq!(result.ticks, 1);
    }

    #[test]
    fn accumulator_produces_multiple_ticks_for_long_elapsed() {
        let state = InterpolableState::new(Vec3::ZERO, 0.0, 0.0);
        let mut timing = FrameTiming::new(state);
        let result = timing.accumulate(TICK_DURATION * 3);
        assert_eq!(result.ticks, 3);
    }

    #[test]
    fn accumulator_produces_zero_ticks_for_short_elapsed() {
        let state = InterpolableState::new(Vec3::ZERO, 0.0, 0.0);
        let mut timing = FrameTiming::new(state);
        let result = timing.accumulate(Duration::from_millis(5));
        assert_eq!(result.ticks, 0);
    }

    #[test]
    fn accumulator_carries_remainder_across_frames() {
        let state = InterpolableState::new(Vec3::ZERO, 0.0, 0.0);
        let mut timing = FrameTiming::new(state);
        // Add 10ms — not enough for a tick (16.667ms).
        let r1 = timing.accumulate(Duration::from_millis(10));
        assert_eq!(r1.ticks, 0);
        // Add another 10ms — total 20ms, enough for one tick with ~3.3ms remainder.
        let r2 = timing.accumulate(Duration::from_millis(10));
        assert_eq!(r2.ticks, 1);
    }

    #[test]
    fn accumulator_clamps_after_long_stall() {
        let state = InterpolableState::new(Vec3::ZERO, 0.0, 0.0);
        let mut timing = FrameTiming::new(state);
        // 2 seconds of stall — should be clamped to 250ms.
        let result = timing.accumulate(Duration::from_secs(2));
        // 250ms / 16.667ms = ~15 ticks max.
        assert!(
            result.ticks <= 15,
            "ticks should be clamped, got {}",
            result.ticks
        );
    }

    #[test]
    fn accumulator_handles_zero_elapsed_without_crash() {
        let state = InterpolableState::new(Vec3::ZERO, 0.0, 0.0);
        let mut timing = FrameTiming::new(state);
        let result = timing.accumulate(Duration::ZERO);
        assert_eq!(result.ticks, 0);
        assert!(result.alpha.is_finite());
    }

    // -- FrameTiming: interpolation alpha --

    #[test]
    fn alpha_is_one_before_first_tick() {
        let state = InterpolableState::new(Vec3::ZERO, 0.0, 0.0);
        let timing = FrameTiming::new(state);
        let interp = timing.interpolated_state();
        // Both states are identical, so any alpha produces the initial state.
        assert_approx(interp.position.x, 0.0, "initial interpolated position");
    }

    #[test]
    fn alpha_is_zero_immediately_after_exact_tick() {
        let state = InterpolableState::new(Vec3::ZERO, 0.0, 0.0);
        let mut timing = FrameTiming::new(state);
        // Push a new state (simulating a tick).
        timing.push_state(InterpolableState::new(Vec3::new(100.0, 0.0, 0.0), 0.0, 0.0));
        // Accumulate exactly one tick — accumulator should be zero after.
        timing.accumulator = Duration::ZERO;
        let result = timing.accumulate(TICK_DURATION);
        assert_eq!(result.ticks, 1);
        // After consuming exactly one tick, remainder is zero → alpha ≈ 0.
        assert!(
            result.alpha < 0.01,
            "alpha should be near zero, got {}",
            result.alpha
        );
    }

    // -- FrameTiming: state management --

    #[test]
    fn push_state_moves_current_to_previous() {
        let state = InterpolableState::new(Vec3::new(1.0, 2.0, 3.0), 0.5, 0.1);
        let mut timing = FrameTiming::new(state);

        timing.push_state(InterpolableState::new(
            Vec3::new(10.0, 20.0, 30.0),
            1.0,
            0.2,
        ));

        assert_approx(timing.previous_state.position.x, 1.0, "prev.x after push");
        assert_approx(timing.current_state.position.x, 10.0, "curr.x after push");
    }

    #[test]
    fn interpolated_state_blends_between_previous_and_current() {
        let state = InterpolableState::new(Vec3::new(0.0, 0.0, 0.0), 0.0, 0.0);
        let mut timing = FrameTiming::new(state);

        // Push a new state so previous and current differ.
        timing.push_state(InterpolableState::new(Vec3::new(100.0, 0.0, 0.0), 0.0, 0.0));

        // Set accumulator to half a tick for alpha ≈ 0.5.
        timing.accumulator = Duration::from_micros(TICK_DURATION.as_micros() as u64 / 2);

        let interp = timing.interpolated_state();
        assert!(
            (interp.position.x - 50.0).abs() < 1.0,
            "interpolated x should be near 50, got {}",
            interp.position.x
        );
    }

    // -- InterpolableState: view_projection --

    #[test]
    fn view_projection_produces_finite_matrix() {
        let state = InterpolableState::new(Vec3::new(0.0, 200.0, 500.0), 0.0, 0.0);
        let vp = state.view_projection(16.0 / 9.0);
        for (i, val) in vp.to_cols_array().iter().enumerate() {
            assert!(val.is_finite(), "view_proj[{i}] is not finite: {val}");
        }
    }

    #[test]
    fn view_projection_handles_zero_aspect_without_nan() {
        let state = InterpolableState::new(Vec3::ZERO, 0.0, 0.0);
        let vp = state.view_projection(0.0);
        for (i, val) in vp.to_cols_array().iter().enumerate() {
            assert!(!val.is_nan(), "view_proj[{i}] with zero aspect is NaN");
        }
    }

    // -- FrameRateMeter --

    #[test]
    fn frame_rate_meter_stats_returns_none_when_empty() {
        let meter = FrameRateMeter::new();
        assert!(meter.stats().is_none());
    }

    #[test]
    fn frame_rate_meter_records_single_sample() {
        let mut meter = FrameRateMeter::new();
        meter.record(Duration::from_millis(16));
        let stats = meter.stats().expect("should have stats after one record");
        assert_approx(stats.min_ms, 16.0, "min after one sample");
        assert_approx(stats.avg_ms, 16.0, "avg after one sample");
        assert_approx(stats.max_ms, 16.0, "max after one sample");
    }

    #[test]
    fn frame_rate_meter_computes_min_avg_max() {
        let mut meter = FrameRateMeter::new();
        meter.record(Duration::from_millis(10));
        meter.record(Duration::from_millis(20));
        meter.record(Duration::from_millis(30));
        let stats = meter.stats().expect("three samples recorded");
        assert_approx(stats.min_ms, 10.0, "min of [10,20,30]");
        assert_approx(stats.avg_ms, 20.0, "avg of [10,20,30]");
        assert_approx(stats.max_ms, 30.0, "max of [10,20,30]");
    }

    #[test]
    fn frame_rate_meter_wraps_ring_buffer() {
        // Record 130 samples into a 120-slot ring. The first 10 should be
        // overwritten and invisible to stats. Strategy: record 10 low-cost
        // frames (1ms) that will be evicted, then 120 high-cost frames
        // (50..=169 ms, each unique). After wrap, stats should reflect only
        // the 50..=169 ms window.
        let mut meter = FrameRateMeter::new();
        for _ in 0..10 {
            meter.record(Duration::from_millis(1));
        }
        for i in 0..FRAMETIME_RING_SIZE {
            meter.record(Duration::from_millis(50 + i as u64));
        }
        let stats = meter.stats().expect("ring is filled");
        // If the early 1ms samples leaked through, min would be 1.0.
        assert_approx(stats.min_ms, 50.0, "min should reflect post-wrap window");
        assert_approx(
            stats.max_ms,
            (50 + FRAMETIME_RING_SIZE as u64 - 1) as f32,
            "max should be the last value in the window",
        );
        // Avg of 50..169 = (50 + 169) / 2 = 109.5
        assert_approx(stats.avg_ms, 109.5, "avg of the post-wrap window");
    }

    #[test]
    fn frame_rate_meter_clear_drops_samples_and_allows_reuse() {
        let mut meter = FrameRateMeter::new();
        for ms in [12, 14, 16, 18, 20] {
            meter.record(Duration::from_millis(ms));
        }
        assert!(meter.stats().is_some(), "stats should exist before clear");

        meter.clear();
        assert!(
            meter.stats().is_none(),
            "stats should be None immediately after clear",
        );

        // After clear, the meter must still accept new samples and report
        // them as if the buffer had never been used — pre-clear data must
        // not bleed into post-clear aggregates.
        meter.record(Duration::from_millis(5));
        let stats = meter
            .stats()
            .expect("stats should exist after a post-clear record");
        assert_approx(stats.min_ms, 5.0, "min reflects only post-clear sample");
        assert_approx(stats.avg_ms, 5.0, "avg reflects only post-clear sample");
        assert_approx(stats.max_ms, 5.0, "max reflects only post-clear sample");
    }

    #[test]
    fn frame_rate_meter_has_fixed_storage() {
        // Zero-allocation invariant pin. If a future refactor swaps the
        // fixed array for a growable container, this assertion breaks and
        // the reviewer has to re-read the "no per-frame alloc" contract
        // before approving the change.
        let meter = FrameRateMeter::new();
        assert_eq!(
            std::mem::size_of_val(&meter.samples),
            FRAMETIME_RING_SIZE * std::mem::size_of::<Duration>(),
            "samples buffer must be a fixed-size array",
        );
    }
}
