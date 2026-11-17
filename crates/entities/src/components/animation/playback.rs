// Locomotion playback-rate policy and rebased clip-local timeline evaluation
// for `MeshAnimation`.
// See: context/lib/scripting.md §10.3 (Mesh Animation)

use super::{MeshAnimation, default_playback_rate};

/// Lowest locomotion playback-rate multiplier. Producers pass calibrated
/// `measured_ground_speed / effective_travel_speed` through
/// [`MeshAnimation::update_playback_rate`]. V1's uncalibrated E10 walk falls
/// back to `measured_ground_speed / move_speed`. This shared clamp keeps host
/// and remote sampling aligned.
pub const RATE_MIN: f32 = 0.5;

/// Highest locomotion playback-rate multiplier by default. See [`RATE_MIN`].
/// An animation state may raise its own ceiling with `maxPlaybackRate`
/// ([`super::AnimationState::max_playback_rate`]); production content keeps
/// this one, because legs flail past it.
pub const RATE_MAX: f32 = 1.5;

/// The rate an enemy's current animation state should play at: the stride
/// rule's clamped ratio, or a fixed authored rate. See
/// [`MeshAnimation::stride_playback_target`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlaybackRateTarget {
    /// Speed-scaled: `raw_ratio` clamped to `RATE_MIN..=max_rate`.
    Stride { raw_ratio: f32, max_rate: f32 },
    /// The state's authored `playbackRate`, applied exactly.
    Fixed(f32),
}

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
        self.update_playback_rate_capped(raw_ratio, RATE_MAX, now);
    }

    /// [`Self::update_playback_rate`] against a caller-chosen ceiling — the
    /// active state's [`super::AnimationState::max_playback_rate`] on the enemy
    /// stride paths. A ceiling below [`RATE_MAX`] or non-finite reads as
    /// `RATE_MAX`, so a cap only ever raises the default.
    pub fn update_playback_rate_capped(&mut self, raw_ratio: f32, max_rate: f32, now: f64) {
        let rate = Self::normalized_playback_rate_capped(raw_ratio, max_rate);
        if !self.playback_rate_needs_update_capped(raw_ratio, max_rate) {
            return;
        }
        self.rebase_to_rate(rate, now);
    }

    /// Normalize a locomotion speed ratio through the one shared rate policy.
    /// Non-finite input rests at the authored playback rate instead of carrying
    /// an invalid value into animation sampling.
    pub fn normalized_playback_rate(raw_ratio: f32) -> f32 {
        Self::normalized_playback_rate_capped(raw_ratio, RATE_MAX)
    }

    /// [`Self::normalized_playback_rate`] against a caller-chosen ceiling. The
    /// floor stays [`RATE_MIN`]; a ceiling below [`RATE_MAX`] or non-finite
    /// reads as `RATE_MAX` (descriptor validation already rejects both).
    pub fn normalized_playback_rate_capped(raw_ratio: f32, max_rate: f32) -> f32 {
        let max_rate = if max_rate.is_finite() {
            max_rate.max(RATE_MAX)
        } else {
            RATE_MAX
        };
        if raw_ratio.is_finite() {
            raw_ratio.clamp(RATE_MIN, max_rate)
        } else {
            default_playback_rate()
        }
    }

    /// Whether applying `raw_ratio` would rebase this animation timeline.
    /// Producers use this read-only predicate to avoid cloning and writing an
    /// unchanged `MeshComponent` on steady-state simulation and client frames.
    pub fn playback_rate_needs_update(&self, raw_ratio: f32) -> bool {
        self.playback_rate_needs_update_capped(raw_ratio, RATE_MAX)
    }

    /// [`Self::playback_rate_needs_update`] against a caller-chosen ceiling.
    pub fn playback_rate_needs_update_capped(&self, raw_ratio: f32, max_rate: f32) -> bool {
        (Self::normalized_playback_rate_capped(raw_ratio, max_rate) - self.rate).abs()
            > RATE_CHANGE_EPSILON
    }

    /// The playback rate the current state plays at this tick, given whether
    /// the stride rule scales it and, if so, its raw speed ratio. The one
    /// policy the enemy host and remote paths share; player locomotion keeps
    /// the global cap and does not consult it.
    ///
    /// - A stride-scaled state (`stride_ratio` is `Some` and the state leaves
    ///   `scaleWithSpeed` on) clamps the ratio to `RATE_MIN..=maxPlaybackRate`;
    ///   its `playbackRate` is ignored.
    /// - Any other state plays its fixed authored `playbackRate` (default 1),
    ///   unbounded by the stride clamp.
    ///
    /// A current state missing from the map reads as the defaults.
    pub fn stride_playback_target(&self, stride_ratio: Option<f32>) -> PlaybackRateTarget {
        let state = self.states.get(&self.current_state);
        let scales = state.is_none_or(|state| state.scale_with_speed);
        match stride_ratio {
            Some(raw_ratio) if scales => PlaybackRateTarget::Stride {
                raw_ratio,
                max_rate: state.map_or(RATE_MAX, super::AnimationState::max_playback_rate),
            },
            _ => PlaybackRateTarget::Fixed(state.map_or(
                default_playback_rate(),
                super::AnimationState::playback_rate,
            )),
        }
    }

    /// Whether applying `target` would change this timeline's rate. A stride
    /// target uses the shared clamp and epsilon; a fixed target compares
    /// exactly, so an authored rate lands precisely and then holds.
    pub fn playback_rate_target_needs_update(&self, target: PlaybackRateTarget) -> bool {
        match target {
            PlaybackRateTarget::Stride {
                raw_ratio,
                max_rate,
            } => self.playback_rate_needs_update_capped(raw_ratio, max_rate),
            PlaybackRateTarget::Fixed(rate) => rate.is_finite() && rate > 0.0 && rate != self.rate,
        }
    }

    /// Apply `target`, rebasing clip-local time like
    /// [`Self::update_playback_rate`].
    pub fn apply_playback_rate_target(&mut self, target: PlaybackRateTarget, now: f64) {
        match target {
            PlaybackRateTarget::Stride {
                raw_ratio,
                max_rate,
            } => self.update_playback_rate_capped(raw_ratio, max_rate, now),
            PlaybackRateTarget::Fixed(rate) => {
                if self.playback_rate_target_needs_update(target) {
                    self.rebase_to_rate(rate, now);
                }
            }
        }
    }

    fn rebase_to_rate(&mut self, rate: f32, now: f64) {
        if let Some(rebase_time) = self.rebase_time {
            self.rebase_elapsed += (now - rebase_time) * f64::from(self.rate);
            self.rebase_time = Some(now);
        }
        self.rate = rate;
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
        assert!((MeshAnimation::locomotion_rate_ratio(4.0, None, 0.0) - 1.0).abs() < f32::EPSILON,);
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

    #[test]
    fn default_state_policy_keeps_the_global_stride_clamp() {
        let anim = two_state_animation();
        assert_eq!(
            anim.stride_playback_target(Some(3.0)),
            PlaybackRateTarget::Stride {
                raw_ratio: 3.0,
                max_rate: RATE_MAX
            }
        );
        assert_eq!(
            anim.stride_playback_target(None),
            PlaybackRateTarget::Fixed(1.0),
            "an unscaled default state rests at 1x"
        );
    }

    #[test]
    fn raised_cap_bounds_only_its_own_state() {
        let mut anim = two_state_animation();
        anim.entered_at = Some(0.0);
        anim.rebase_time = Some(0.0);
        anim.states.get_mut("idle").unwrap().max_playback_rate = Some(3.0);

        let target = anim.stride_playback_target(Some(2.7));
        anim.apply_playback_rate_target(target, 0.0);
        assert!((anim.rate - 2.7).abs() < 1e-6, "{}", anim.rate);
        anim.apply_playback_rate_target(anim.stride_playback_target(Some(10.0)), 1.0);
        assert!((anim.rate - 3.0).abs() < 1e-6, "clamps at the raised cap");
        anim.apply_playback_rate_target(anim.stride_playback_target(Some(0.1)), 2.0);
        assert!(
            (anim.rate - RATE_MIN).abs() < 1e-6,
            "the floor is unchanged"
        );
        // Continuity across the rebases: 1 s at 2.7, then 1 s at 3.0.
        assert!((anim.scaled_elapsed(2.0) - 5.7).abs() < 1e-5);

        // The other state, and the uncapped API, keep 1.5.
        assert!((MeshAnimation::normalized_playback_rate(2.7) - RATE_MAX).abs() < 1e-6);
        anim.current_state = "attack".into();
        assert_eq!(
            anim.stride_playback_target(Some(2.7)),
            PlaybackRateTarget::Stride {
                raw_ratio: 2.7,
                max_rate: RATE_MAX
            }
        );
    }

    #[test]
    fn capped_clamp_never_lowers_the_default_cap() {
        for cap in [0.25, 1.0, f32::NAN, f32::INFINITY] {
            assert!(
                (MeshAnimation::normalized_playback_rate_capped(9.0, cap) - RATE_MAX).abs() < 1e-6,
                "cap {cap} reads as RATE_MAX"
            );
        }
    }

    #[test]
    fn scale_with_speed_false_plays_the_fixed_rate() {
        let mut anim = two_state_animation();
        let idle = anim.states.get_mut("idle").unwrap();
        idle.scale_with_speed = false;
        idle.max_playback_rate = Some(3.0);
        assert_eq!(
            anim.stride_playback_target(Some(2.7)),
            PlaybackRateTarget::Fixed(1.0),
            "an opted-out state ignores its stride ratio"
        );
        anim.states.get_mut("idle").unwrap().playback_rate = Some(2.5);
        assert_eq!(
            anim.stride_playback_target(Some(2.7)),
            PlaybackRateTarget::Fixed(2.5)
        );
    }

    #[test]
    fn fixed_rate_is_exact_unclamped_and_ignored_while_stride_scaled() {
        let mut anim = two_state_animation();
        anim.entered_at = Some(0.0);
        anim.rebase_time = Some(0.0);
        anim.states.get_mut("idle").unwrap().playback_rate = Some(2.51);

        // Stride-scaled: the rule decides, `playbackRate` is ignored.
        assert_eq!(
            anim.stride_playback_target(Some(1.0)),
            PlaybackRateTarget::Stride {
                raw_ratio: 1.0,
                max_rate: RATE_MAX
            }
        );

        // Unscaled: exactly the authored rate, past RATE_MAX, then it holds.
        let target = anim.stride_playback_target(None);
        assert!(anim.playback_rate_target_needs_update(target));
        anim.apply_playback_rate_target(target, 1.0);
        assert_eq!(anim.rate, 2.51);
        assert!(!anim.playback_rate_target_needs_update(target));
        assert!((anim.scaled_elapsed(2.0) - (1.0 + 2.51)).abs() < 1e-5);

        // Below RATE_MIN too, with no epsilon swallowing a small change.
        anim.states.get_mut("idle").unwrap().playback_rate = Some(0.25);
        anim.apply_playback_rate_target(anim.stride_playback_target(None), 2.0);
        assert_eq!(anim.rate, 0.25);
    }
}
