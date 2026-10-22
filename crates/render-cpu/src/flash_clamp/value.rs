// Per-value flash clamp: the limiter's rules for one presented value (a strength
// and its color's redness), and the tracked parts a ghost's look clamp judges.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

use super::window::{FlashWindow, Part, decide};
use crate::flash_limiter::LimiterFrameUniform;

// The limiter's rules (WCAG 2.3.1 thresholds, rate cap), applied to one value.
pub(super) const FLASH_LUMINANCE_THRESHOLD: f32 = 0.1;
/// A swing's darker state counts as dark below this luminance.
pub(super) const FLASH_DARK_LIMIT: f32 = 0.8;
/// The limiter's intensity rate cap in blend strength per second: none to full
/// takes 250 ms. A ramp that moves no faster than this never reads as limited.
pub const INTENSITY_RATE_PER_SECOND: f32 = 4.0;
pub(super) const SUPPRESSION_DEADBAND: f32 = 0.02;
pub(super) const RED_SATURATED: f32 = 0.7;
const RED_TRANSITION_THRESHOLD: f32 = 0.2;
const RED_SUPPRESSION_DEADBAND: f32 = 0.05;

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
fn desaturation_to_redness(rgb: [f32; 3], held: f32) -> f32 {
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

/// A level judged as it is, as every strength but a dithered opacity presents.
pub(super) fn as_is(level: f32) -> f32 {
    level
}

/// One tracked value: what was presented, its last extremum, its direction
/// and the furthest value reached in that direction since the extremum.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Tracker {
    pub(super) out: f32,
    ext: f32,
    dir: f32,
    peak: f32,
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
    fn restart(&mut self, level: f32) {
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

    fn present(&mut self, next: f32, reversal: f32) {
        let step = self.step(next, reversal);
        self.ext = step.ext;
        self.dir = step.dir;
        self.peak = step.peak;
        self.out = next;
    }
}

/// Which side of a swing is its darker state, as drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DarkState {
    /// The weaker level: a strength that brightens a dark background.
    Weaker,
    /// The stronger level: a value that darkens a white background, which
    /// draws one minus it.
    Stronger,
    /// Either side that qualifies: a ghost's opacity, which brightens a dark
    /// background and darkens a white one.
    Either,
}

impl DarkState {
    /// Whether a swing between `level` and `ext` has a darker state dark
    /// enough to count.
    fn counts(self, level: f32, ext: f32) -> bool {
        let weaker = || level.min(ext) < FLASH_DARK_LIMIT;
        let stronger = || 1.0 - level.max(ext) < FLASH_DARK_LIMIT;
        match self {
            DarkState::Weaker => weaker(),
            DarkState::Stronger => stronger(),
            DarkState::Either => weaker() || stronger(),
        }
    }
}

/// How a [`Strength`] judges its level.
#[derive(Clone, Copy, Debug)]
pub(super) struct Judging {
    /// Which side of a swing is its darker state.
    pub(super) dark: DarkState,
    /// The level a rate-capped strength presents as, which is what counts:
    /// itself, or the share of pixels a ghost's screen door keeps.
    pub(super) presents: fn(f32) -> f32,
    /// How far this frame's rate cap starts from the last presented level: a
    /// step something already judged carries it through (a screen-door step).
    pub(super) shift: f32,
}

impl Judging {
    /// A strength over a dark background, presented as it is.
    pub(super) const OVER_DARK: Self = Self {
        dark: DarkState::Weaker,
        presents: as_is,
        shift: 0.0,
    };
}

/// One frame's decision for a [`Strength`], before it presents.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Decided {
    /// The level it presents as, the one judged.
    pub(super) level: f32,
    /// The rate-capped strength behind it, where the next rate cap starts.
    pub(super) ramp: f32,
}

/// One strength-like level (a blend strength, a ghost's opacity, drawn
/// brightness or darkening) limited on a flash window its owner holds and ages
/// once a frame. The rate cap moves the strength itself; the window judges the
/// level it presents as.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Strength {
    tracker: Tracker,
    /// The rate-capped strength last presented. Equal to the presented level
    /// unless that level is quantized.
    ramp: f32,
}

impl Strength {
    /// Start fresh at `strength`: nothing to rate-cap from.
    pub(super) fn restart(&mut self, strength: f32) {
        self.restart_presenting(strength, as_is);
    }

    /// Start fresh at `strength`, presented as `presents` says.
    pub(super) fn restart_presenting(&mut self, strength: f32, presents: fn(f32) -> f32) {
        let strength = unit_strength(strength);
        self.tracker.restart(presents(strength));
        self.ramp = strength;
    }

    /// The level last presented.
    pub(super) fn presented(&self) -> f32 {
        self.tracker.out
    }

    /// The rate-capped strength last presented.
    pub(super) fn ramp(&self) -> f32 {
        self.ramp
    }

    /// Whether it holds its onsets until the window has room.
    pub(super) fn suppressing(&self) -> bool {
        self.tracker.suppressing
    }

    /// Hold its onsets until the window has room if `suppressing`.
    pub(super) fn keep_suppressing(&mut self, suppressing: bool) {
        self.tracker.suppressing |= suppressing;
    }

    /// `strength`, no further than [`INTENSITY_RATE_PER_SECOND`] × `dt_rate`
    /// from the last presented strength moved by `shift`.
    pub(super) fn capped(&self, strength: f32, shift: f32, frame: &LimiterFrameUniform) -> f32 {
        // Full-screen rate cap, hitch-clamped.
        let cap = INTENSITY_RATE_PER_SECOND * frame.dt_rate;
        let from = self.ramp + shift;
        unit_strength(strength).clamp(from - cap, from + cap)
    }

    /// This frame's decision for `strength`, its transitions counted on
    /// `window` as `part`: rate-capped, judged on the level it presents as,
    /// and an over-budget onset held at the last presented level. Nothing
    /// presents until [`Self::present`].
    pub(super) fn decide(
        &mut self,
        window: &mut FlashWindow,
        part: Part,
        strength: f32,
        judging: Judging,
        frame: &LimiterFrameUniform,
    ) -> Decided {
        let capped = self.capped(strength, judging.shift, frame);
        let judged = (judging.presents)(capped);
        let (_, ext) = self.tracker.excursion(judged, FLASH_LUMINANCE_THRESHOLD);
        let counts = judging.dark.counts(judged, ext);
        let level = decide(
            &mut self.tracker,
            window,
            part,
            judged,
            FLASH_LUMINANCE_THRESHOLD,
            SUPPRESSION_DEADBAND,
            counts,
        );
        Decided {
            level,
            // A held level keeps the strength it was presented at.
            ramp: if level == judged { capped } else { self.ramp },
        }
    }

    /// Present a decided level.
    pub(super) fn present(&mut self, decided: Decided) {
        self.tracker
            .present(decided.level, FLASH_LUMINANCE_THRESHOLD);
        self.ramp = decided.ramp;
    }

    /// The level presented this frame for `strength` over a dark background,
    /// its transitions counted on `window` as `part`.
    fn limit(
        &mut self,
        window: &mut FlashWindow,
        part: Part,
        strength: f32,
        frame: &LimiterFrameUniform,
    ) -> f32 {
        let decided = self.decide(window, part, strength, Judging::OVER_DARK, frame);
        self.present(decided);
        decided.level
    }
}

/// The redness of a color weighted by the strength it presents at, limited on
/// a flash window its owner holds and ages once a frame.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Redness {
    tracker: Tracker,
    /// Whether the last presented color was saturated red, so a move away from
    /// it counts as the return of a red transition.
    was_saturated_red: bool,
}

impl Redness {
    /// This frame's presented redness and color for `rgb` at presented
    /// strength `level`, counted on `window`. Over budget a red onset
    /// desaturates the color toward its own luminance, solved exactly, so its
    /// luminance is unchanged and it presents exactly the held redness.
    /// Nothing presents until [`Self::present`].
    pub(super) fn decide(
        &mut self,
        window: &mut FlashWindow,
        level: f32,
        rgb: [f32; 3],
    ) -> (f32, [f32; 3]) {
        let color_red = redness(rgb);
        let red_target = level * color_red;
        let saturated = color_red >= RED_SATURATED || self.was_saturated_red;
        self.was_saturated_red = color_red >= RED_SATURATED;
        let red = decide(
            &mut self.tracker,
            window,
            Part::Red,
            red_target,
            RED_TRANSITION_THRESHOLD,
            RED_SUPPRESSION_DEADBAND,
            saturated,
        );
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
        (red, rgb)
    }

    /// Present a decided redness.
    pub(super) fn present(&mut self, red: f32) {
        self.tracker.present(red, RED_TRANSITION_THRESHOLD);
    }

    /// Start fresh against `rgb` presented at strength `level`.
    pub(super) fn restart(&mut self, level: f32, rgb: [f32; 3]) {
        self.tracker.restart(level * redness(rgb));
        self.was_saturated_red = redness(rgb) >= RED_SATURATED;
    }

    /// The redness last presented.
    pub(super) fn presented(&self) -> f32 {
        self.tracker.out
    }

    /// Whether it holds its onsets until the window has room.
    pub(super) fn suppressing(&self) -> bool {
        self.tracker.suppressing
    }

    /// Hold its onsets until the window has room if `suppressing`.
    pub(super) fn keep_suppressing(&mut self, suppressing: bool) {
        self.tracker.suppressing |= suppressing;
    }
}

/// A blend strength and the redness of its color, weighted by that strength,
/// limited on a flash window its owner holds and ages once a frame: a screen
/// channel's whole value.
#[derive(Clone, Copy, Debug, Default)]
struct StrengthAndRed {
    level: Strength,
    red: Redness,
}

impl StrengthAndRed {
    /// The strength and color presented this frame, their transitions counted
    /// on `window`. See [`ValueClamp`] for the rules.
    fn limit(
        &mut self,
        window: &mut FlashWindow,
        strength: f32,
        rgb: [f32; 3],
        frame: &LimiterFrameUniform,
    ) -> (f32, [f32; 3]) {
        let level = self.level.limit(window, Part::Level, strength, frame);
        // Red: the overlay's redness, weighted by how strongly it blends.
        let (red, rgb) = self.red.decide(window, level, rgb);
        self.red.present(red);
        (level, rgb)
    }

    /// Start fresh against this value: nothing to rate-cap from.
    fn restart(&mut self, strength: f32, rgb: [f32; 3]) {
        let strength = unit_strength(strength);
        self.level.restart(strength);
        self.red.restart(strength, rgb);
    }

    /// Whether it presents nothing: no strength and no redness.
    fn presents_nothing(&self) -> bool {
        self.level.presented() == 0.0 && self.red.presented() == 0.0
    }
}

/// The frame a vanished value presents nothing on: a rate allowance of a full
/// blend lets its strength reach zero at once, since the clamp cannot hold
/// back what already vanished.
pub(super) fn vanishing(frame: &LimiterFrameUniform) -> LimiterFrameUniform {
    LimiterFrameUniform {
        dt_rate: 1.0 / INTENSITY_RATE_PER_SECOND,
        ..*frame
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
    value: StrengthAndRed,
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
        self.value.limit(&mut self.window, strength, rgb, frame)
    }

    /// Record a frame on which the value presented nothing: it fell to zero
    /// whatever the rate cap, since the clamp cannot hold back what already
    /// vanished. The fall counts as a return, so a value that strobes by
    /// appearing and vanishing spends its budget like any other strobe.
    pub fn absent(&mut self, frame: &LimiterFrameUniform) {
        self.limit(0.0, [0.0; 3], &vanishing(frame));
    }

    /// Start fresh against this frame's own values: an empty window and
    /// nothing to rate-cap from, so the next [`Self::limit`] of the same value
    /// presents it unchanged.
    pub fn restart(&mut self, strength: f32, rgb: [f32; 3]) {
        self.window = FlashWindow::default();
        self.value.restart(strength, rgb);
    }

    /// Whether the clamp holds nothing a fresh one at zero would not: its
    /// window is empty and it presents nothing.
    pub fn is_at_rest(&self) -> bool {
        self.window.is_empty() && self.value.presents_nothing()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn each_dark_state_rule_judges_its_own_side_of_a_swing() {
        // A swing between 1 and 0.81: dark only over a white background.
        assert!(!DarkState::Weaker.counts(1.0, 0.81));
        assert!(DarkState::Stronger.counts(1.0, 0.81));
        assert!(DarkState::Either.counts(1.0, 0.81));
        // A swing between 0.1 and 0.19: dark only over a dark background.
        assert!(DarkState::Weaker.counts(0.19, 0.1));
        assert!(!DarkState::Stronger.counts(0.19, 0.1));
        assert!(DarkState::Either.counts(0.19, 0.1));
    }
}
