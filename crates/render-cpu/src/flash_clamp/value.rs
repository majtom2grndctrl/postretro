// Per-value flash clamp: the photosensitivity limiter's rules applied to one
// presented value (a strength and the redness of its color), and the parts it
// is built from, which a ghost's look clamp shares one flash window across.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

use crate::flash_limiter::LimiterFrameUniform;

// The limiter's rules (WCAG 2.3.1 thresholds, flash window, rate cap), applied
// to one value.
pub(super) const FLASH_LUMINANCE_THRESHOLD: f32 = 0.1;
const FLASH_DARK_LIMIT: f32 = 0.8;
const FLASH_MAX_TRANSITIONS: usize = 6;
const FLASH_WINDOW_SECONDS: f32 = 1.0;
/// The limiter's intensity rate cap in blend strength per second: none to full
/// takes 250 ms. A ramp that moves no faster than this never reads as limited.
pub const INTENSITY_RATE_PER_SECOND: f32 = 4.0;
pub(super) const SUPPRESSION_DEADBAND: f32 = 0.02;
pub(super) const RED_SATURATED: f32 = 0.7;
const RED_TRANSITION_THRESHOLD: f32 = 0.2;
const RED_SUPPRESSION_DEADBAND: f32 = 0.05;
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

/// One tracked value: what was presented, its last extremum, its direction
/// and the furthest value reached in that direction since the extremum.
#[derive(Clone, Copy, Debug, Default)]
struct Tracker {
    out: f32,
    ext: f32,
    dir: f32,
    peak: f32,
    last_sign: i8,
    suppressing: bool,
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
    fn excursion(&self, next: f32, reversal: f32) -> (f32, f32) {
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

/// Which tracked part of a value a window slot's transition belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Part {
    Level,
    Red,
    /// A ghost's drawn brightness (presented opacity times its tint's
    /// luminance), on the ghost's window beside its opacity and redness.
    Brightness,
}

const PARTS: usize = 3;

impl Part {
    /// This part's bit in a slot's part set.
    fn bit(self) -> u8 {
        1u8 << self as u8
    }

    /// Whether this part's transitions merge with another part's on a merging
    /// window. Opacity and drawn brightness are two views of one drawn change.
    /// Redness is a separate criterion, so a red transition always spends a
    /// slot of its own.
    fn merges(self) -> bool {
        !matches!(self, Part::Red)
    }
}

/// One counted transition: the parts it counts for, and whether it was an
/// onset.
#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    parts: u8,
    onset: bool,
}

/// Transitions counted in the last second, aged by presented-frame time,
/// oldest first. Each slot records the parts it counts for and whether it was
/// an onset. Every part limited on one window spends the same budget of six
/// transitions, so together they present at most three flashes a second.
///
/// A merging window (a ghost's look) counts opacity and brightness
/// transitions on one frame, in one direction, as one transition: both are
/// views of one change in what draws, and any one background sees a single
/// change. A screen channel's window never merges, so its strength and red
/// transitions each spend their own slot.
///
/// An onset is admitted with room for itself and one return. A part joining it
/// on the same frame owes its own return too, which later onsets must leave
/// room for. If the joined parts then return on different frames, the second
/// return is one past what the onset's admission reserved: the same excess a
/// return whose onset aged out, or predates a fresh start, already allows.
/// The window never holds more than eight transitions: those it holds plus
/// the parts that may still return never exceed eight.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FlashWindow {
    ages: [f32; WINDOW_SLOTS],
    kinds: [Slot; WINDOW_SLOTS],
    count: usize,
    /// Slots from here on were counted this frame.
    frame_start: usize,
    /// Whether transitions of mergeable parts on one frame, in one direction,
    /// share a slot.
    merges: bool,
}

impl FlashWindow {
    /// An empty window that merges same-frame, same-direction transitions of
    /// a ghost's opacity and brightness.
    pub(super) fn merging() -> Self {
        Self {
            merges: true,
            ..Self::default()
        }
    }

    /// Age the window by a frame's time and start that frame: slots counted
    /// from here on are this frame's.
    pub(super) fn age(&mut self, dt: f32) {
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
        self.frame_start = kept;
    }

    /// The slot this frame's transition of `part` in this direction joins: a
    /// slot counted this frame in the same direction that does not count
    /// `part` yet. Only on a merging window.
    fn joinable(&self, part: Part, onset: bool) -> Option<usize> {
        if !self.merges || !part.merges() {
            return None;
        }
        (self.frame_start..self.count).find(|&slot| {
            let kind = self.kinds[slot];
            kind.onset == onset && kind.parts & part.bit() == 0
        })
    }

    fn push(&mut self, part: Part, onset: bool) {
        if let Some(slot) = self.joinable(part, onset) {
            self.kinds[slot].parts |= part.bit();
            return;
        }
        debug_assert!(
            self.count < WINDOW_SLOTS,
            "the flash window never holds more than {WINDOW_SLOTS} transitions"
        );
        if self.count < WINDOW_SLOTS {
            self.ages[self.count] = 0.0;
            self.kinds[self.count] = Slot {
                parts: part.bit(),
                onset,
            };
            self.count += 1;
        }
    }

    /// Returns the window still owes: onsets in it whose return it does not
    /// hold yet, counted per part across every part. A return with no onset
    /// of its part before it owes nothing. A slot counting several parts
    /// opens or settles each of them.
    fn owed_returns(&self) -> usize {
        let mut open = [0usize; PARTS];
        for kind in &self.kinds[..self.count] {
            for (part, owed) in open.iter_mut().enumerate() {
                if kind.parts & (1u8 << part) == 0 {
                    continue;
                }
                if kind.onset {
                    *owed += 1;
                } else {
                    *owed = owed.saturating_sub(1);
                }
            }
        }
        open.iter().sum()
    }

    /// An onset is admitted only while the window holds it, the returns
    /// earlier onsets of any part still owe, and its own return.
    fn onset_fits(&self) -> bool {
        self.count + self.owed_returns() + 2 <= FLASH_MAX_TRANSITIONS
    }

    pub(super) fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// Decide one tracked part's next presented level. Onsets (rises) are admitted
/// only while the window holds them, their return, and the returns earlier
/// onsets still owe; over budget the part holds. Returns always pass. On a
/// merging window, an onset joining one another part counted this frame is
/// that admitted transition.
fn decide(
    tracker: &mut Tracker,
    window: &mut FlashWindow,
    part: Part,
    target: f32,
    threshold: f32,
    deadband: f32,
    counts: bool,
) -> f32 {
    let (excursion, _) = tracker.excursion(target, threshold);
    let sign: i8 = if excursion > 0.0 { 1 } else { -1 };
    let onset = sign > 0;
    let fits = window.onset_fits() || (onset && window.joinable(part, true).is_some());
    if fits {
        tracker.suppressing = false;
    }
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

/// The frame a vanished value presents nothing on: a rate allowance of a full
/// blend lets its strength reach zero at once, since the clamp cannot hold
/// back what already vanished.
pub(super) fn vanishing(frame: &LimiterFrameUniform) -> LimiterFrameUniform {
    LimiterFrameUniform {
        dt_rate: 1.0 / INTENSITY_RATE_PER_SECOND,
        ..*frame
    }
}

/// One strength-like level (a blend strength, a ghost's drawn brightness)
/// limited on a flash window its owner holds and ages once a frame.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Strength(Tracker);

impl Strength {
    /// Start fresh at `strength`: nothing to rate-cap from.
    pub(super) fn restart(&mut self, strength: f32) {
        self.0.restart(unit_strength(strength));
    }

    /// The level last presented.
    pub(super) fn presented(&self) -> f32 {
        self.0.out
    }

    /// The level presented this frame for `strength`, its transitions counted
    /// on `window` as `part`: rate-capped at [`INTENSITY_RATE_PER_SECOND`] ×
    /// `dt_rate`, and an over-budget onset held at the last presented level.
    pub(super) fn limit(
        &mut self,
        window: &mut FlashWindow,
        part: Part,
        strength: f32,
        frame: &LimiterFrameUniform,
    ) -> f32 {
        let tracker = &mut self.0;
        let strength = unit_strength(strength);
        // Full-screen rate cap, hitch-clamped.
        let cap = INTENSITY_RATE_PER_SECOND * frame.dt_rate;
        let capped = strength.clamp(tracker.out - cap, tracker.out + cap);
        // The darker state is the weaker level.
        let dark_ok =
            capped.min(tracker.excursion(capped, FLASH_LUMINANCE_THRESHOLD).1) < FLASH_DARK_LIMIT;
        let level = decide(
            tracker,
            window,
            part,
            capped,
            FLASH_LUMINANCE_THRESHOLD,
            SUPPRESSION_DEADBAND,
            dark_ok,
        );
        tracker.present(level, FLASH_LUMINANCE_THRESHOLD);
        level
    }
}

/// A blend strength and the redness of its color, weighted by that strength,
/// limited on a flash window its owner holds and ages once a frame: a screen
/// channel's whole value, or a ghost's opacity and tint.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct StrengthAndRed {
    level: Strength,
    red: Tracker,
    /// Whether the last presented color was saturated red, so a move away from
    /// it counts as the return of a red transition.
    was_saturated_red: bool,
}

impl StrengthAndRed {
    /// The strength and color presented this frame, their transitions counted
    /// on `window`. See [`ValueClamp`] for the rules.
    pub(super) fn limit(
        &mut self,
        window: &mut FlashWindow,
        strength: f32,
        rgb: [f32; 3],
        frame: &LimiterFrameUniform,
    ) -> (f32, [f32; 3]) {
        let level = self.level.limit(window, Part::Level, strength, frame);

        // Red: the overlay's redness, weighted by how strongly it blends.
        let color_red = redness(rgb);
        let red_target = level * color_red;
        let saturated = color_red >= RED_SATURATED || self.was_saturated_red;
        self.was_saturated_red = color_red >= RED_SATURATED;
        let red = decide(
            &mut self.red,
            window,
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

    /// Start fresh against this value: nothing to rate-cap from.
    pub(super) fn restart(&mut self, strength: f32, rgb: [f32; 3]) {
        let strength = unit_strength(strength);
        self.level.0.restart(strength);
        self.red.restart(strength * redness(rgb));
        self.was_saturated_red = redness(rgb) >= RED_SATURATED;
    }

    /// Whether it presents nothing: no strength and no redness.
    pub(super) fn presents_nothing(&self) -> bool {
        self.level.0.out == 0.0 && self.red.out == 0.0
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
    fn an_onset_of_any_part_waits_for_the_returns_other_parts_still_owe() {
        let mut window = FlashWindow::default();
        for _ in 0..2 {
            window.push(Part::Level, true);
            window.push(Part::Level, false);
        }
        assert!(window.onset_fits(), "4 held + 0 owed + 2 fits in six");
        window.push(Part::Level, true);
        // The opacity onset's return takes the last room: a brightness onset
        // admitted now would leave it none.
        assert!(!window.onset_fits());
        window.push(Part::Level, false);
        assert_eq!((window.count, window.owed_returns()), (6, 0));

        // Each part's onsets owe their own returns, summed across parts.
        let mut window = FlashWindow::default();
        window.push(Part::Brightness, true);
        window.push(Part::Red, true);
        assert_eq!(window.owed_returns(), 2);
        assert!(window.onset_fits(), "2 held + 2 owed + 2 fits in six");
        window.push(Part::Level, true);
        assert!(!window.onset_fits(), "3 held + 3 owed + 2 is over six");
        // A return of one part settles only that part's onset.
        window.push(Part::Brightness, false);
        assert_eq!(window.owed_returns(), 2);
    }

    #[test]
    fn a_merging_window_counts_one_frames_opacity_and_brightness_change_once() {
        let mut window = FlashWindow::merging();
        window.age(0.1);
        window.push(Part::Level, true);
        window.push(Part::Brightness, true);
        assert_eq!(window.count, 1, "one frame's rise in both is one onset");
        assert_eq!(window.owed_returns(), 2, "each part owes its own return");
        window.push(Part::Red, true);
        assert_eq!(window.count, 2, "a red onset spends a slot of its own");

        // The next frame starts new slots: a return there never joins the
        // previous frame's, and opposite directions never share one.
        window.age(0.1);
        window.push(Part::Brightness, false);
        window.push(Part::Red, false);
        assert_eq!(window.count, 4);
        window.push(Part::Level, false);
        assert_eq!(window.count, 4, "the opacity return joins this frame's");
        assert_eq!(
            window.owed_returns(),
            0,
            "a joined return settles each part"
        );

        // A screen channel's window keeps every part's transitions apart.
        let mut window = FlashWindow::default();
        window.age(0.1);
        window.push(Part::Level, true);
        window.push(Part::Brightness, true);
        assert_eq!(window.count, 2);
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
