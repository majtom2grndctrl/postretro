// Per-value flash clamp: the photosensitivity limiter's rules applied to one
// presented value (a strength and the redness of its color).
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

use crate::flash_limiter::LimiterFrameUniform;

// The limiter's rules (WCAG 2.3.1 thresholds, flash window, rate cap), applied
// to one value.
pub(super) const FLASH_LUMINANCE_THRESHOLD: f32 = 0.1;
const FLASH_DARK_LIMIT: f32 = 0.8;
pub(super) const FLASH_MAX_TRANSITIONS: usize = 6;
pub(super) const FLASH_WINDOW_SECONDS: f32 = 1.0;
/// The limiter's intensity rate cap in blend strength per second: none to full
/// takes 250 ms. A ramp that moves no faster than this never reads as limited.
pub const INTENSITY_RATE_PER_SECOND: f32 = 4.0;
pub(super) const SUPPRESSION_DEADBAND: f32 = 0.02;
pub(super) const RED_SATURATED: f32 = 0.7;
pub(super) const RED_TRANSITION_THRESHOLD: f32 = 0.2;
pub(super) const RED_SUPPRESSION_DEADBAND: f32 = 0.05;
const WINDOW_SLOTS: usize = 8;

const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

pub(super) fn luminance(rgb: [f32; 3]) -> f32 {
    rgb[0] * LUMA[0] + rgb[1] * LUMA[1] + rgb[2] * LUMA[2]
}

/// Redness from chromaticity alone: 0 for any neutral or non-red color, 1 for
/// pure red.
pub(super) fn redness(rgb: [f32; 3]) -> f32 {
    let total = rgb[0] + rgb[1] + rgb[2];
    if total < 0.003 {
        return 0.0;
    }
    ((rgb[0] / total - 1.0 / 3.0) * 1.5).clamp(0.0, 1.0)
}

/// How far to mix `rgb` toward its own luminance so its redness falls to
/// `held`. Mixing by d moves the red share R/(R+G+B) along
/// (r + d(L − r)) / (T + d(3L − T)), which is not linear in d, so this solves
/// for d exactly. Luminance is unchanged by the mix.
pub(super) fn desaturation_to_redness(rgb: [f32; 3], held: f32) -> f32 {
    let total = rgb[0] + rgb[1] + rgb[2];
    let lum = luminance(rgb);
    // The red share whose redness is `held` (inverse of `redness`).
    let share = 1.0 / 3.0 + held / 1.5;
    let excess = rgb[0] - share * total;
    if excess <= 0.0 {
        return 0.0;
    }
    // Positive whenever `excess` is: `share` ≥ 1/3 and L ≤ T.
    let denom = (rgb[0] - lum) - share * (total - 3.0 * lum);
    (excess / denom.max(1e-6)).clamp(0.0, 1.0)
}

/// A finite strength in [0, 1]; anything else is no effect.
pub(super) fn unit_strength(strength: f32) -> f32 {
    if strength.is_finite() {
        strength.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// One tracked value: what was presented, its last extremum, its direction
/// and the furthest value reached in that direction since the extremum.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Tracker {
    pub(super) out: f32,
    ext: f32,
    pub(super) dir: f32,
    pub(super) peak: f32,
    pub(super) last_sign: i8,
    pub(super) suppressing: bool,
}

/// Where a tracked value's excursion stands after moving to a level.
struct Step {
    ext: f32,
    dir: f32,
    peak: f32,
}

impl Tracker {
    pub(super) fn restart(&mut self, level: f32) {
        *self = Self {
            out: level,
            ext: level,
            peak: level,
            ..Self::default()
        };
    }

    /// The excursion after moving to `next`: moving back from the peak by
    /// `reversal` or more makes the peak the new extremum; a smaller dip
    /// leaves the excursion running. WCAG counts a flash as a pair of opposing
    /// changes of at least the threshold, so a smaller dip is not an opposing
    /// change.
    fn step(&self, next: f32, reversal: f32) -> Step {
        let mut step = Step {
            ext: self.ext,
            dir: self.dir,
            peak: self.peak,
        };
        let beyond = (next - self.peak) * self.dir;
        if self.dir == 0.0 {
            if next != self.peak {
                step.dir = (next - self.peak).signum();
                step.peak = next;
            }
        } else if beyond > 0.0 {
            step.peak = next;
        } else if -beyond >= reversal {
            step.ext = self.peak;
            step.dir = -self.dir;
            step.peak = next;
        }
        step
    }

    /// Excursion from the last extremum if the value moved to `next`, and that
    /// extremum.
    pub(super) fn excursion(&self, next: f32, reversal: f32) -> (f32, f32) {
        let ext = self.step(next, reversal).ext;
        (next - ext, ext)
    }

    pub(super) fn present(&mut self, next: f32, reversal: f32) {
        let step = self.step(next, reversal);
        self.ext = step.ext;
        self.dir = step.dir;
        self.peak = step.peak;
        self.out = next;
    }
}

/// Which tracked part of a value a window slot's transition belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Part {
    #[default]
    Level,
    Red,
}

/// Transitions counted in the last second, aged by presented-frame time,
/// oldest first. Each slot records its part and whether it was an onset.
#[derive(Clone, Copy, Debug, Default)]
struct FlashWindow {
    ages: [f32; WINDOW_SLOTS],
    kinds: [(Part, bool); WINDOW_SLOTS],
    count: usize,
}

impl FlashWindow {
    fn age(&mut self, dt: f32) {
        let mut kept = 0;
        for i in 0..self.count {
            let age = self.ages[i] + dt;
            if age < FLASH_WINDOW_SECONDS {
                self.ages[kept] = age;
                self.kinds[kept] = self.kinds[i];
                kept += 1;
            }
        }
        self.count = kept;
    }

    fn push(&mut self, part: Part, onset: bool) {
        if self.count < WINDOW_SLOTS {
            self.ages[self.count] = 0.0;
            self.kinds[self.count] = (part, onset);
            self.count += 1;
        }
    }

    /// Returns the window still owes: onsets in it whose return it does not
    /// hold yet. A return with no onset before it owes nothing.
    fn owed_returns(&self) -> usize {
        let mut open = [0usize; 2];
        for &(part, onset) in &self.kinds[..self.count] {
            let owed = &mut open[part as usize];
            if onset {
                *owed += 1;
            } else {
                *owed = owed.saturating_sub(1);
            }
        }
        open[0] + open[1]
    }

    /// An onset is admitted only while the window holds it, the returns
    /// earlier onsets still owe, and its own return.
    fn onset_fits(&self) -> bool {
        self.count + self.owed_returns() + 2 <= FLASH_MAX_TRANSITIONS
    }
}

/// Decide one tracked part's next presented level. Onsets (rises) are admitted
/// only while the window holds them, their return, and the returns earlier
/// onsets still owe; over budget the part holds. Returns always pass.
///
/// A ghost's redness runs the same rule on its own window
/// (`GhostRedness::limit`, `GhostWindow::red_onset_fits`); a change to one
/// belongs in both.
fn decide(
    tracker: &mut Tracker,
    window: &mut FlashWindow,
    part: Part,
    target: f32,
    threshold: f32,
    deadband: f32,
    counts: bool,
) -> f32 {
    let fits = window.onset_fits();
    if fits {
        tracker.suppressing = false;
    }
    let (excursion, _) = tracker.excursion(target, threshold);
    let sign: i8 = if excursion > 0.0 { 1 } else { -1 };
    let onset = sign > 0;
    if counts && sign != tracker.last_sign && excursion.abs() >= threshold {
        if !onset || fits {
            window.push(part, onset);
            tracker.last_sign = sign;
        } else {
            tracker.suppressing = true;
        }
    } else if counts && onset && !fits && sign != tracker.last_sign && excursion >= deadband {
        tracker.suppressing = true;
    }
    if tracker.suppressing && target > tracker.out {
        tracker.out
    } else {
        target
    }
}

/// The photosensitivity limiter for one presented value, judged as a
/// full-screen change. The input is a blend strength in 0..=1 (anything
/// non-finite is no effect) and the value's color, whose redness counts
/// weighted by that strength. Strength and redness share one flash window, so
/// red transitions spend the same budget. The output is the presented strength
/// and color:
///
/// - strength moves at most [`INTENSITY_RATE_PER_SECOND`] × `dt_rate` a frame
///   (hitch-clamped presented time);
/// - at most 3 flashes in any 1 s window of `dt_window` (full presented time);
///   an over-budget onset holds at the last presented level, so a limited
///   strobe rests at its pre-flash level, and returns always pass;
/// - an over-budget red onset desaturates the color toward its own luminance,
///   solved exactly, so luminance is unchanged and the value presents exactly
///   its held redness. Under budget, the color passes unchanged.
///
/// The clamp has no enable flag of its own: its owner passes values through
/// while the limiter is off and [`Self::restart`]s it on the frame that turns
/// it on, so that frame presents unchanged.
#[derive(Clone, Copy, Debug, Default)]
pub struct ValueClamp {
    window: FlashWindow,
    level: Tracker,
    red: Tracker,
    /// Whether the last presented color was saturated red, so a move away from
    /// it counts as the return of a red transition.
    was_saturated_red: bool,
}

impl ValueClamp {
    /// A clamp starting fresh against this value; see [`Self::restart`].
    pub fn fresh(strength: f32, rgb: [f32; 3]) -> Self {
        let mut clamp = Self::default();
        clamp.restart(strength, rgb);
        clamp
    }

    /// Limit the value's `strength` and color `rgb` for one presented frame.
    /// Returns the presented strength and color.
    pub fn limit(
        &mut self,
        strength: f32,
        rgb: [f32; 3],
        frame: &LimiterFrameUniform,
    ) -> (f32, [f32; 3]) {
        self.window.age(frame.dt_window);
        let strength = unit_strength(strength);

        // Full-screen rate cap, hitch-clamped.
        let cap = INTENSITY_RATE_PER_SECOND * frame.dt_rate;
        let capped = strength.clamp(self.level.out - cap, self.level.out + cap);
        // The overlay's darker state is its weaker blend.
        let dark_ok = capped.min(self.level.excursion(capped, FLASH_LUMINANCE_THRESHOLD).1)
            < FLASH_DARK_LIMIT;
        let level = decide(
            &mut self.level,
            &mut self.window,
            Part::Level,
            capped,
            FLASH_LUMINANCE_THRESHOLD,
            SUPPRESSION_DEADBAND,
            dark_ok,
        );
        self.level.present(level, FLASH_LUMINANCE_THRESHOLD);

        // Red: the overlay's redness, weighted by how strongly it blends.
        let color_red = redness(rgb);
        let red_target = level * color_red;
        let saturated = color_red >= RED_SATURATED || self.was_saturated_red;
        self.was_saturated_red = color_red >= RED_SATURATED;
        let red = decide(
            &mut self.red,
            &mut self.window,
            Part::Red,
            red_target,
            RED_TRANSITION_THRESHOLD,
            RED_SUPPRESSION_DEADBAND,
            saturated,
        );
        self.red.present(red, RED_TRANSITION_THRESHOLD);
        let rgb = if red < red_target && red_target > 0.0 {
            // Hold the presented redness (strength × tint redness) by
            // desaturating the tint toward its own luminance; `red_target > 0`
            // means `level > 0`.
            let held = (red / level).clamp(0.0, 1.0);
            let d = desaturation_to_redness(rgb, held);
            let gray = luminance(rgb);
            rgb.map(|channel| channel + (gray - channel) * d)
        } else {
            rgb
        };
        (level, rgb)
    }

    /// Record a frame on which the value presented nothing: it fell to zero
    /// whatever the rate cap, since the clamp cannot hold back what already
    /// vanished. The fall counts as a return, so a value that strobes by
    /// appearing and vanishing spends its budget like any other strobe.
    pub fn absent(&mut self, frame: &LimiterFrameUniform) {
        // A rate allowance of a full blend lets the strength reach zero now.
        let vanish = LimiterFrameUniform {
            dt_rate: 1.0 / INTENSITY_RATE_PER_SECOND,
            ..*frame
        };
        self.limit(0.0, [0.0; 3], &vanish);
    }

    /// Start fresh against this frame's own values: an empty window and
    /// nothing to rate-cap from, so the next [`Self::limit`] of the same value
    /// presents it unchanged.
    pub fn restart(&mut self, strength: f32, rgb: [f32; 3]) {
        let strength = unit_strength(strength);
        self.window = FlashWindow::default();
        self.level.restart(strength);
        self.red.restart(strength * redness(rgb));
        self.was_saturated_red = redness(rgb) >= RED_SATURATED;
    }

    /// Whether the clamp holds nothing a fresh one at zero would not: its
    /// window is empty and it presents nothing.
    pub fn is_at_rest(&self) -> bool {
        self.window.count == 0 && self.level.out == 0.0 && self.red.out == 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_onset_waits_for_the_returns_earlier_onsets_still_owe() {
        let mut window = FlashWindow::default();
        // A return with no onset before it owes nothing.
        window.push(Part::Level, false);
        window.push(Part::Level, true);
        assert!(window.onset_fits(), "2 held + 1 owed + 2 fits in six");
        window.push(Part::Red, true);
        // Two onsets now owe their returns: 3 + 2 + 2 is over six, though
        // counting held transitions alone would admit it.
        assert!(!window.onset_fits());
        window.push(Part::Level, false);
        window.push(Part::Red, false);
        assert!(!window.onset_fits(), "five held leaves no room for a pair");
    }

    #[test]
    fn a_held_red_tint_presents_exactly_its_held_redness() {
        for rgb in [[1.0, 0.0, 0.0], [0.9, 0.1, 0.05], [0.6, 0.05, 0.2]] {
            for held in [0.0, 0.3, 0.55] {
                let d = desaturation_to_redness(rgb, held);
                let gray = luminance(rgb);
                let mixed = rgb.map(|channel| channel + (gray - channel) * d);
                assert!(
                    (redness(mixed) - held).abs() < 1e-4,
                    "{rgb:?} held at {held}: presents {}",
                    redness(mixed)
                );
                assert!((luminance(mixed) - gray).abs() < 1e-6);
            }
        }
        // Already at or below the held redness: nothing to take away.
        assert_eq!(desaturation_to_redness([0.3, 0.3, 0.3], 0.2), 0.0);
    }
}
