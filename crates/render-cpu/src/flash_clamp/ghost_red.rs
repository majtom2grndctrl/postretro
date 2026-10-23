// Ghost redness: a ghost tint's redness weighted by its presented opacity,
// counted on the ghost's flash window and desaturated over budget.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use super::ghost_window::GhostWindow;
use super::value::{
    RED_SATURATED, RED_SUPPRESSION_DEADBAND, RED_TRANSITION_THRESHOLD, Tracker,
    desaturation_to_redness, luminance, redness,
};

/// A ghost tint's redness, on the screen channels' red rule: a transition is a
/// move of at least 0.2 toward or away from saturated red, weighted by the
/// presented opacity. Red transitions spend the ghost's window, each in a slot
/// of its own. An onset is admitted only with room for itself, its return and
/// the returns earlier red onsets owe; over budget the tint desaturates toward
/// its own luminance, solved exactly, so it presents the held redness at
/// unchanged luminance. Returns always pass.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct GhostRedness {
    tracker: Tracker,
    /// Whether the last judged tint was saturated red, so a move away from it
    /// counts as the return of a red transition.
    was_saturated_red: bool,
}

impl GhostRedness {
    /// Redness starting fresh against `rgb` presented at `opacity`.
    pub(super) fn fresh(opacity: f32, rgb: [f32; 3]) -> Self {
        let mut red = Self::default();
        red.tracker.restart(opacity * redness(rgb));
        red.was_saturated_red = redness(rgb) >= RED_SATURATED;
        red
    }

    /// The tint `rgb` presents as at presented `opacity`, its redness counted
    /// on `window`: itself, or over budget desaturated to the held redness.
    pub(super) fn limit(
        &mut self,
        window: &mut GhostWindow,
        opacity: f32,
        rgb: [f32; 3],
    ) -> [f32; 3] {
        let color_red = redness(rgb);
        let target = opacity * color_red;
        let saturated = color_red >= RED_SATURATED || self.was_saturated_red;
        self.was_saturated_red = color_red >= RED_SATURATED;

        let tracker = &mut self.tracker;
        let (excursion, _) = tracker.excursion(target, RED_TRANSITION_THRESHOLD);
        let sign: i8 = if excursion > 0.0 { 1 } else { -1 };
        let onset = sign > 0;
        let fits = window.red_onset_fits();
        if fits {
            tracker.suppressing = false;
        }
        if saturated && sign != tracker.last_sign && excursion.abs() >= RED_TRANSITION_THRESHOLD {
            if !onset || fits {
                window.count_red(onset);
                tracker.last_sign = sign;
            } else {
                tracker.suppressing = true;
            }
        } else if saturated
            && onset
            && !fits
            && sign != tracker.last_sign
            && excursion >= RED_SUPPRESSION_DEADBAND
        {
            tracker.suppressing = true;
        }
        let red = if tracker.suppressing && target > tracker.out {
            tracker.out
        } else {
            target
        };
        tracker.present(red, RED_TRANSITION_THRESHOLD);

        if red < target && target > 0.0 {
            // `target > 0` means `opacity > 0`.
            let held = (red / opacity).clamp(0.0, 1.0);
            let d = desaturation_to_redness(rgb, held);
            let gray = luminance(rgb);
            rgb.map(|channel| channel + (gray - channel) * d)
        } else {
            rgb
        }
    }

    /// The redness last presented.
    pub(super) fn presented(&self) -> f32 {
        self.tracker.out
    }
}
