// Locomotion playback-rate policy and rebased clip-local timeline evaluation
// for `MeshAnimation`.
// See: context/lib/scripting.md §10.3 (Mesh Animation)

use super::{default_playback_rate, MeshAnimation};

/// Lowest locomotion playback-rate multiplier. The animation-rate producers
/// pass their raw `speed_xz / move_speed` ratio through
/// [`MeshAnimation::update_playback_rate`], which owns this clamp so host and
/// remote sampling cannot drift apart.
pub const RATE_MIN: f32 = 0.5;

/// Highest locomotion playback-rate multiplier. See [`RATE_MIN`].
pub const RATE_MAX: f32 = 1.5;

/// Ignore tiny rate changes so noisy motion samples do not rebase every tick.
pub const RATE_CHANGE_EPSILON: f32 = 0.02;

impl MeshAnimation {
    /// Resolve the raw locomotion speed ratio for one tick, before the shared
    /// clamp. This is the calibration policy for speed-scaled playback.
    ///
    /// When the active state carries a calibrated travel speed — a per-state
    /// `travelSpeed` override or the clip's load-derived stride, resolved by the
    /// caller into `effective_travel_speed` — the ratio is
    /// `measured_ground_speed / effective_travel_speed`, so playback cadence
    /// tracks the *authored stride* regardless of the agent's chase `move_speed`.
    /// A character moving faster than its clip's authored travel speed plays it
    /// proportionally faster, slower when slower.
    ///
    /// When the state has neither — the degenerate case that keeps the shipped
    /// in-place walk byte-for-byte — it falls back to the historical
    /// `measured_ground_speed / move_speed` reference. A non-positive denominator
    /// rests at the authored rate, guarding the division from NaN/inf.
    ///
    /// The result is pre-clamp: pass it through [`Self::update_playback_rate`]
    /// (or gate with [`Self::playback_rate_needs_update`]) so the one shared
    /// `RATE_MIN`/`RATE_MAX` clamp and epsilon policy still apply — host and
    /// remote sampling cannot drift apart.
    pub fn locomotion_rate_ratio(
        measured_ground_speed: f32,
        effective_travel_speed: Option<f32>,
        move_speed: f32,
    ) -> f32 {
        let denominator = effective_travel_speed.unwrap_or(move_speed);
        if denominator > 0.0 {
            measured_ground_speed / denominator
        } else {
            default_playback_rate()
        }
    }

    /// Update the current state's locomotion playback rate from a raw speed
    /// ratio. The caller supplies the same animation clock used by sampling;
    /// rebasing before changing the rate keeps clip-local time continuous.
    ///
    /// Pending entries have no clock origin yet: retain the new rate but accrue
    /// nothing, so they still sample as just-entered until resolution.
    pub fn update_playback_rate(&mut self, raw_ratio: f32, now: f64) {
        let rate = Self::normalized_playback_rate(raw_ratio);
        if !self.playback_rate_needs_update(raw_ratio) {
            return;
        }

        if let Some(rebase_time) = self.rebase_time {
            self.rebase_elapsed += (now - rebase_time) * f64::from(self.rate);
            self.rebase_time = Some(now);
        }
        self.rate = rate;
    }

    /// Normalize a locomotion speed ratio through the one shared rate policy.
    /// Non-finite input rests at the authored playback rate instead of carrying
    /// an invalid value into animation sampling.
    pub fn normalized_playback_rate(raw_ratio: f32) -> f32 {
        if raw_ratio.is_finite() {
            raw_ratio.clamp(RATE_MIN, RATE_MAX)
        } else {
            default_playback_rate()
        }
    }

    /// Whether applying `raw_ratio` would rebase this animation timeline.
    /// Producers use this read-only predicate to avoid cloning and writing an
    /// unchanged `MeshComponent` on steady-state simulation and client frames.
    pub fn playback_rate_needs_update(&self, raw_ratio: f32) -> bool {
        (Self::normalized_playback_rate(raw_ratio) - self.rate).abs() > RATE_CHANGE_EPSILON
    }

    /// Evaluate the current state's rebased clip-local elapsed time at the
    /// animation clock instant used by the renderer. Missing origins use the
    /// entry stamp when available; genuinely pending entries read as zero.
    pub fn scaled_elapsed(&self, anim_time: f64) -> f64 {
        self.rebase_time.map_or_else(
            || {
                self.entered_at.map_or(0.0, |entered_at| {
                    (anim_time - entered_at) * f64::from(self.rate)
                })
            },
            |rebase_time| self.rebase_elapsed + (anim_time - rebase_time) * f64::from(self.rate),
        )
    }

    /// Evaluate the outgoing fade leg's rebased clip-local elapsed time. This
    /// is distinct from [`Self::scaled_elapsed`] because a fade snapshots the
    /// outgoing state before the incoming state's timing is reset.
    pub fn previous_scaled_elapsed(&self, anim_time: f64) -> f64 {
        self.previous_rebase_time.map_or_else(
            || {
                self.previous_entered_at.map_or(0.0, |entered_at| {
                    (anim_time - entered_at) * f64::from(self.previous_rate)
                })
            },
            |rebase_time| {
                self.previous_rebase_elapsed
                    + (anim_time - rebase_time) * f64::from(self.previous_rate)
            },
        )
    }

    pub(super) fn reset_incoming_playback_time(&mut self) {
        self.rate = default_playback_rate();
        self.rebase_time = None;
        self.rebase_elapsed = 0.0;
    }

    pub(super) fn clear_previous_playback_time(&mut self) {
        self.previous_rate = default_playback_rate();
        self.previous_rebase_time = None;
        self.previous_rebase_elapsed = 0.0;
    }

    pub(super) fn snapshot_previous_playback_time(&mut self) {
        self.previous_rate = self.rate;
        self.previous_rebase_time = self.rebase_time;
        self.previous_rebase_elapsed = self.rebase_elapsed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::animation::test_support::two_state_animation;
    use crate::components::mesh::MeshComponent;

    #[test]
    fn locomotion_rate_ratio_scales_proportionally_to_effective_travel_speed() {
        // Faster than the authored 2.0 u/s stride plays proportionally faster.
        assert!(
            (MeshAnimation::locomotion_rate_ratio(3.0, Some(2.0), 5.0) - 1.5).abs() < f32::EPSILON,
            "measured 3.0 over authored 2.0 stride is a 1.5x cadence",
        );
        // Slower than the stride plays proportionally slower — and the ratio is
        // driven by the travel speed, never the agent's chase `move_speed`.
        assert!(
            (MeshAnimation::locomotion_rate_ratio(1.0, Some(2.0), 5.0) - 0.5).abs() < f32::EPSILON,
            "measured 1.0 over authored 2.0 stride is a 0.5x cadence, independent of move_speed",
        );
    }

    #[test]
    fn locomotion_rate_ratio_falls_back_to_move_speed_when_uncalibrated() {
        // The degenerate case (no override, no derived stride) reproduces the
        // historical `speed_xz / move_speed` reference exactly — the shipped E10
        // in-place walk depends on this being byte-for-byte unchanged.
        assert!(
            (MeshAnimation::locomotion_rate_ratio(3.0, None, 6.0) - 0.5).abs() < f32::EPSILON,
            "no calibration divides by move_speed like the shipped walk did",
        );
    }

    #[test]
    fn locomotion_rate_ratio_rests_on_nonpositive_denominator() {
        // Guard the division: a zero override or a zero move_speed rests at the
        // authored rate rather than emitting NaN/inf into sampling.
        assert!(
            (MeshAnimation::locomotion_rate_ratio(4.0, Some(0.0), 5.0) - 1.0).abs() < f32::EPSILON,
        );
        assert!(
            (MeshAnimation::locomotion_rate_ratio(4.0, None, 0.0) - 1.0).abs() < f32::EPSILON,
        );
    }

    #[test]
    fn playback_rate_clamps_and_scales_elapsed() {
        let mut anim = two_state_animation();
        anim.entered_at = Some(0.0);
        anim.rebase_time = Some(0.0);

        anim.update_playback_rate(1.0, 0.0);
        assert!((anim.scaled_elapsed(2.0) - 2.0).abs() < 1.0e-9);

        anim.update_playback_rate(0.5, 2.0);
        assert!((anim.rate - 0.5).abs() < f32::EPSILON);
        assert!((anim.scaled_elapsed(4.0) - 3.0).abs() < 1.0e-9);

        anim.update_playback_rate(-100.0, 4.0);
        assert!((anim.rate - RATE_MIN).abs() < f32::EPSILON);
        anim.update_playback_rate(100.0, 4.0);
        assert!((anim.rate - RATE_MAX).abs() < f32::EPSILON);
    }

    #[test]
    fn playback_rate_rebases_continuously_and_monotonically() {
        let mut anim = two_state_animation();
        anim.entered_at = Some(0.0);
        anim.rebase_time = Some(0.0);

        let before = anim.scaled_elapsed(1.0);
        anim.update_playback_rate(0.5, 1.0);
        assert!((anim.scaled_elapsed(1.0) - before).abs() < 1.0e-9);

        let mut samples = vec![anim.scaled_elapsed(1.25)];
        anim.update_playback_rate(1.5, 1.5);
        samples.push(anim.scaled_elapsed(1.75));
        anim.update_playback_rate(0.5, 2.0);
        samples.push(anim.scaled_elapsed(2.5));
        assert!(
            samples.windows(2).all(|pair| pair[0] <= pair[1]),
            "acceleration and deceleration must never run clip time backwards: {samples:?}"
        );
    }

    #[test]
    fn playback_rate_evaluation_is_stride_independent() {
        let mut anim = two_state_animation();
        anim.entered_at = Some(0.0);
        anim.rebase_time = Some(0.0);
        anim.update_playback_rate(0.5, 1.0);
        anim.update_playback_rate(1.5, 3.0);

        let dense: Vec<f64> = (0..=8)
            .map(|step| anim.scaled_elapsed(f64::from(step) * 0.5))
            .collect();
        for index in [0, 2, 4, 8] {
            let time = f64::from(index) * 0.5;
            assert!(
                (anim.scaled_elapsed(time) - dense[index as usize]).abs() < 1.0e-9,
                "sampling at a stride must equal dense evaluation at {time}"
            );
        }
    }

    #[test]
    fn pending_playback_rebase_sets_rate_without_accruing() {
        let mut anim = two_state_animation();
        anim.update_playback_rate(0.5, 7.0);
        assert!((anim.rate - 0.5).abs() < f32::EPSILON);
        assert_eq!(anim.rebase_time, None);
        assert_eq!(anim.rebase_elapsed, 0.0);
        assert_eq!(anim.scaled_elapsed(7.0), 0.0);
    }

    #[test]
    fn component_serde_restores_runtime_playback_rates_to_one() {
        let mut value = MeshComponent::animated("decraniated".into(), two_state_animation());
        let anim = value.animation.as_mut().unwrap();
        anim.rate = 0.5;
        anim.previous_rate = 1.5;
        anim.rebase_time = Some(3.0);
        anim.previous_rebase_time = Some(2.0);

        let back: MeshComponent =
            serde_json::from_value(serde_json::to_value(value).unwrap()).unwrap();
        let back = back.animation.unwrap();
        assert_eq!(back.rate, 1.0);
        assert_eq!(back.previous_rate, 1.0);
        assert_eq!(back.rebase_time, None);
        assert_eq!(back.previous_rebase_time, None);
    }
}
