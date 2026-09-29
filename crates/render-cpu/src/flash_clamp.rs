// Channel clamp: the photosensitivity limiter. `screen.flash` and
// `screen.vignette` are limited as they pack into the resolve's effect uniform,
// each channel judged as a full-screen change.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

use crate::flash_limiter::{LimiterFrameInput, LimiterFrameUniform, pack_limiter_frame};
use crate::screen_effects::EffectUniform;

// The limiter's rules (WCAG 2.3.1 thresholds, flash window, rate cap), applied
// to one channel.
const FLASH_LUMINANCE_THRESHOLD: f32 = 0.1;
const FLASH_DARK_LIMIT: f32 = 0.8;
const FLASH_MAX_TRANSITIONS: usize = 6;
const FLASH_WINDOW_SECONDS: f32 = 1.0;
const INTENSITY_RATE_PER_SECOND: f32 = 4.0;
const SUPPRESSION_DEADBAND: f32 = 0.02;
const RED_SATURATED: f32 = 0.7;
const RED_TRANSITION_THRESHOLD: f32 = 0.2;
const RED_SUPPRESSION_DEADBAND: f32 = 0.05;
const WINDOW_SLOTS: usize = 8;

const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

fn luminance(rgb: [f32; 3]) -> f32 {
    rgb[0] * LUMA[0] + rgb[1] * LUMA[1] + rgb[2] * LUMA[2]
}

/// Redness from chromaticity alone: 0 for any neutral or non-red color, 1 for
/// pure red.
fn redness(rgb: [f32; 3]) -> f32 {
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
fn unit_strength(strength: f32) -> f32 {
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

/// Which value of a channel a window slot's transition belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Value {
    #[default]
    Level,
    Red,
}

/// Transitions counted in the last second, aged by presented-frame time,
/// oldest first. Each slot records its value and whether it was an onset.
#[derive(Clone, Copy, Debug, Default)]
struct FlashWindow {
    ages: [f32; WINDOW_SLOTS],
    kinds: [(Value, bool); WINDOW_SLOTS],
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

    fn push(&mut self, value: Value, onset: bool) {
        if self.count < WINDOW_SLOTS {
            self.ages[self.count] = 0.0;
            self.kinds[self.count] = (value, onset);
            self.count += 1;
        }
    }

    /// Returns the window still owes: onsets in it whose return it does not
    /// hold yet. A return with no onset before it owes nothing.
    fn owed_returns(&self) -> usize {
        let mut open = [0usize; 2];
        for &(value, onset) in &self.kinds[..self.count] {
            let owed = &mut open[value as usize];
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

/// Decide one tracked value's next presented level. Onsets (rises) are admitted
/// only while the window holds them, their return, and the returns earlier
/// onsets still owe; over budget the value holds. Returns always pass.
fn decide(
    tracker: &mut Tracker,
    window: &mut FlashWindow,
    value: Value,
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
            window.push(value, onset);
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

/// One effect channel: its strength (flash alpha, vignette strength) and its
/// redness share one flash window, so red transitions spend the same budget.
#[derive(Clone, Copy, Debug, Default)]
struct EffectChannel {
    window: FlashWindow,
    level: Tracker,
    red: Tracker,
    /// Whether the last presented tint was saturated red, so a move away from
    /// it counts as the return of a red transition.
    was_saturated_red: bool,
}

impl EffectChannel {
    /// Limit a channel's `strength` and tint `rgb` for one frame. Returns the
    /// limited strength and color.
    fn limit(
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
            Value::Level,
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
            Value::Red,
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

    /// Start fresh against this frame's own values: an empty window and
    /// nothing to rate-cap from.
    fn restart(&mut self, strength: f32, rgb: [f32; 3]) {
        let strength = unit_strength(strength);
        self.window = FlashWindow::default();
        self.level.restart(strength);
        self.red.restart(strength * redness(rgb));
        self.was_saturated_red = redness(rgb) >= RED_SATURATED;
    }
}

/// The channel clamp's state across frames. Off, it passes the uniform through
/// and keeps no history. The frame that turns it on (`init`) starts fresh
/// against its own values — an empty window, nothing to rate-cap from — so
/// that frame packs unchanged and nothing from before the off counts.
#[derive(Clone, Copy, Debug, Default)]
pub struct ChannelClamp {
    flash: EffectChannel,
    vignette: EffectChannel,
    /// Whether the previous frame ran with the clamp on. The first enabled
    /// frame after an off one, or of a new clamp, starts fresh.
    history_live: bool,
}

impl ChannelClamp {
    /// This frame's limiter input: the enable flag, presented-frame time, and
    /// whether history starts fresh. Called once per resolve frame, before
    /// [`Self::apply`].
    pub fn begin_frame(&mut self, input: LimiterFrameInput, enabled: bool) -> LimiterFrameUniform {
        let fresh = enabled && !self.history_live;
        self.history_live = enabled;
        pack_limiter_frame(input, enabled, fresh)
    }

    /// Limit `uniform`'s flash and vignette in place for one presented frame.
    pub fn apply(&mut self, uniform: &mut EffectUniform, frame: &LimiterFrameUniform) {
        if !frame.enabled {
            return;
        }
        let flash_rgb = [uniform.flash[0], uniform.flash[1], uniform.flash[2]];
        let vignette_rgb = [
            uniform.vignette[0],
            uniform.vignette[1],
            uniform.vignette[2],
        ];
        if frame.init {
            self.flash.restart(uniform.flash[3], flash_rgb);
            self.vignette.restart(uniform.vignette[3], vignette_rgb);
        }
        let (flash_a, flash_rgb) = self.flash.limit(uniform.flash[3], flash_rgb, frame);
        uniform.flash = [flash_rgb[0], flash_rgb[1], flash_rgb[2], flash_a];
        let (vignette_w, vignette_rgb) =
            self.vignette
                .limit(uniform.vignette[3], vignette_rgb, frame);
        uniform.vignette = [
            vignette_rgb[0],
            vignette_rgb[1],
            vignette_rgb[2],
            vignette_w,
        ];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flash_limiter::{FLASH_LIMITER_SLOT, HITCH_CEILING_SECONDS, flash_limiter_enabled};
    use crate::screen_effects::pack_effect_uniform;
    use postretro_entities::SlotValue;
    use std::collections::HashMap;

    fn elapsed(dt: f32) -> LimiterFrameInput {
        LimiterFrameInput {
            elapsed_seconds: dt,
        }
    }

    fn frame(dt: f32, enabled: bool) -> LimiterFrameUniform {
        pack_limiter_frame(elapsed(dt), enabled, false)
    }

    fn flash(rgb: [f32; 3], a: f32) -> EffectUniform {
        EffectUniform {
            flash: [rgb[0], rgb[1], rgb[2], a],
            ..EffectUniform::default()
        }
    }

    /// Count transitions of `series` (one value per frame at `dt`) in any one
    /// second, WCAG-style, from the last extremum. A reversal ends an
    /// excursion only once it reaches the 0.1 threshold itself.
    fn worst_transitions(series: &[f32], dt: f32) -> usize {
        let mut times = Vec::new();
        let (mut ext, mut peak, mut dir, mut counted) = (series[0], series[0], 0.0f32, false);
        for (n, &v) in series.iter().enumerate().skip(1) {
            let beyond = (v - peak) * dir;
            if dir == 0.0 {
                if v != peak {
                    dir = (v - peak).signum();
                    peak = v;
                }
            } else if beyond > 0.0 {
                peak = v;
            } else if -beyond >= 0.1 {
                ext = peak;
                peak = v;
                dir = -dir;
                counted = false;
            }
            if !counted && (v - ext).abs() >= 0.1 && v.min(ext) < 0.8 {
                counted = true;
                times.push(n as f32 * dt);
            }
        }
        (0..times.len())
            .map(|i| {
                times[i..]
                    .iter()
                    .take_while(|&&t| t < times[i] + 1.0)
                    .count()
            })
            .max()
            .unwrap_or(0)
    }

    fn run_flash_strobe(hz: f32, fps: f32, seconds: f32, rgb: [f32; 3]) -> Vec<EffectUniform> {
        let mut clamp = ChannelClamp::default();
        let dt = 1.0 / fps;
        (0..(seconds * fps) as usize)
            .map(|n| {
                let on = ((n as f32 * dt) * hz).fract() >= 0.5;
                let mut uniform = flash(rgb, if on { 1.0 } else { 0.0 });
                clamp.apply(&mut uniform, &frame(dt, true));
                uniform
            })
            .collect()
    }

    #[test]
    fn a_four_flash_screen_flash_strobe_packs_at_most_three_flashes() {
        let fps = 60.0;
        let packed = run_flash_strobe(4.0, fps, 2.0, [1.0, 1.0, 1.0]);
        let alphas: Vec<f32> = packed.iter().map(|u| u.flash[3]).collect();
        let worst = worst_transitions(&alphas, 1.0 / fps);
        assert!(
            (4..=6).contains(&worst),
            "{worst} transitions in one second"
        );
        // Limited, it rests at its pre-flash level.
        assert!(alphas.windows(8).any(|w| w.iter().all(|&a| a < 0.02)));
    }

    #[test]
    fn a_flash_strobe_that_dips_slightly_on_the_way_packs_at_most_three_flashes() {
        // 0 ↔ 0.19 at 6 Hz in 0.065 steps (under the rate cap) with a 0.001
        // dip between them. Restarting the excursion on each dip, no step
        // reached the threshold and twelve transitions a second packed.
        let cycle = [
            0.0, 0.065, 0.064, 0.129, 0.128, 0.193, 0.128, 0.129, 0.064, 0.065,
        ];
        let dt = 1.0 / 60.0;
        let mut clamp = ChannelClamp::default();
        let alphas: Vec<f32> = (0..120)
            .map(|n| {
                let mut uniform = flash([1.0; 3], cycle[n % cycle.len()]);
                clamp.apply(&mut uniform, &frame(dt, true));
                uniform.flash[3]
            })
            .collect();
        let worst = worst_transitions(&alphas, dt);
        assert!(worst <= 6, "{worst} transitions in one second");
    }

    #[test]
    fn a_red_screen_flash_strobe_over_budget_packs_desaturated() {
        let packed = run_flash_strobe(5.0, 60.0, 2.0, [1.0, 0.0, 0.0]);
        let desaturated = packed
            .iter()
            .filter(|u| u.flash[3] > 0.05)
            .any(|u| redness([u.flash[0], u.flash[1], u.flash[2]]) < RED_SATURATED);
        assert!(desaturated, "an over-budget red onset is desaturated");
    }

    #[test]
    fn a_single_saturated_red_flash_under_budget_keeps_its_hue() {
        // Dark, then one full-strength red flash for half a second and its
        // return: two onsets (strength and red) and their returns fit the
        // budget, so nothing is held or desaturated.
        let dt = 1.0 / 60.0;
        let red = [1.0, 0.0, 0.0];
        let mut clamp = ChannelClamp::default();
        let packed: Vec<EffectUniform> = (0..90)
            .map(|n| {
                let alpha = if (6..36).contains(&n) { 1.0 } else { 0.0 };
                let mut uniform = flash(red, alpha);
                clamp.apply(&mut uniform, &frame(dt, true));
                uniform
            })
            .collect();
        let alphas: Vec<f32> = packed.iter().map(|u| u.flash[3]).collect();
        // Rate-capped to 4.0/s, it still reaches full strength and returns.
        assert!(alphas.iter().any(|&a| a >= 1.0), "{alphas:?}");
        assert_eq!(alphas.last(), Some(&0.0));
        assert_eq!(worst_transitions(&alphas, dt), 2);
        for u in &packed {
            assert_eq!(
                [u.flash[0], u.flash[1], u.flash[2]],
                red,
                "desaturated at strength {}",
                u.flash[3]
            );
        }
    }

    #[test]
    fn off_passes_through_and_the_enabling_frame_starts_fresh() {
        let dt = 1.0 / 60.0;
        let mut clamp = ChannelClamp::default();
        let pack = |clamp: &mut ChannelClamp, alpha: f32, enabled: bool| {
            let frame = clamp.begin_frame(elapsed(dt), enabled);
            let mut uniform = flash([1.0; 3], alpha);
            clamp.apply(&mut uniform, &frame);
            (uniform, frame)
        };
        // On: dark for half a second, then a 7.5 Hz strobe spends the whole
        // budget in the half second before the off.
        for n in 0..60 {
            let alpha = if n >= 30 && ((n - 30) / 4) % 2 == 0 {
                1.0
            } else {
                0.0
            };
            let (_, frame) = pack(&mut clamp, alpha, true);
            assert_eq!(
                frame.init,
                n == 0,
                "only a new clamp's first frame starts fresh"
            );
        }
        // Off for half a second, mid-strobe, ending on a dark frame.
        for n in 60..90 {
            let alpha = ((n + 1) % 2) as f32;
            let (off, _) = pack(&mut clamp, alpha, false);
            assert_eq!(off, flash([1.0; 3], alpha), "off packs content unchanged");
        }
        // Turned on on a frame that jumps 0 → 1: that frame starts fresh and
        // packs unchanged, not rate-capped.
        let (enabling, frame) = pack(&mut clamp, 1.0, true);
        assert!(frame.init);
        assert_eq!(enabling.flash[3], 1.0);
        // Nothing from before the off counts: the fall that follows and the
        // next onset are both admitted, though the six transitions before the
        // off were all under a second old when it turned back on.
        let alphas: Vec<f32> = (1..=12)
            .map(|k| {
                let alpha = if (1..6).contains(&k) { 0.0 } else { 1.0 };
                let (packed, frame) = pack(&mut clamp, alpha, true);
                assert!(!frame.init, "history is live again");
                packed.flash[3]
            })
            .collect();
        let trough = alphas[..5].iter().copied().fold(f32::INFINITY, f32::min);
        let peak = alphas[5..].iter().copied().fold(0.0, f32::max);
        assert!(trough <= 0.9, "the fall is admitted: {alphas:?}");
        assert!(
            peak >= trough + 0.2,
            "the next onset is admitted: {alphas:?}"
        );
    }

    #[test]
    fn an_onset_waits_for_the_returns_earlier_onsets_still_owe() {
        let mut window = FlashWindow::default();
        // A return with no onset before it owes nothing.
        window.push(Value::Level, false);
        window.push(Value::Level, true);
        assert!(window.onset_fits(), "2 held + 1 owed + 2 fits in six");
        window.push(Value::Red, true);
        // Two onsets now owe their returns: 3 + 2 + 2 is over six, though
        // counting held transitions alone would admit it.
        assert!(!window.onset_fits());
        window.push(Value::Level, false);
        window.push(Value::Red, false);
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

    #[test]
    fn the_rate_cap_slows_a_full_strength_flash_and_clamps_a_hitch() {
        let mut clamp = ChannelClamp::default();
        let mut uniform = flash([1.0; 3], 1.0);
        clamp.apply(&mut uniform, &frame(2.0, true));
        assert!(
            uniform.flash[3] <= 4.0 / 30.0 + 1e-6,
            "{}",
            uniform.flash[3]
        );
    }

    /// A 5 Hz strobe of `shape` (strength over time) packed at `fps` for
    /// `seconds`, as flash alphas.
    fn packed_strobe(shape: fn(f32) -> f32, fps: f32, seconds: f32) -> Vec<f32> {
        let dt = 1.0 / fps;
        let mut clamp = ChannelClamp::default();
        (0..(seconds * fps).round() as usize)
            .map(|n| {
                let mut uniform = flash([1.0; 3], shape(n as f32 * dt));
                clamp.apply(&mut uniform, &frame(dt, true));
                uniform.flash[3]
            })
            .collect()
    }

    fn square_5hz(t: f32) -> f32 {
        if (t * 5.0).fract() < 0.5 { 1.0 } else { 0.0 }
    }

    fn sine_5hz(t: f32) -> f32 {
        0.5 - 0.5 * (std::f32::consts::TAU * 5.0 * t).cos()
    }

    #[test]
    fn a_5hz_strobe_packs_the_same_three_flashes_at_30_and_240_hz() {
        for (name, shape) in [("square", square_5hz as fn(f32) -> f32), ("sine", sine_5hz)] {
            // Unlimited, the strobe is ten transitions a second.
            let raw: Vec<f32> = (0..240).map(|n| shape(n as f32 / 240.0)).collect();
            assert!(worst_transitions(&raw, 1.0 / 240.0) >= 9, "{name}");

            let counts = [30.0, 240.0].map(|fps| {
                let alphas = packed_strobe(shape, fps, 3.0);
                worst_transitions(&alphas, 1.0 / fps)
            });
            assert!(
                (4..=6).contains(&counts[0]),
                "{name}: {} transitions in one second at 30 Hz",
                counts[0]
            );
            assert_eq!(counts[0], counts[1], "{name}: 30 Hz vs 240 Hz");
        }
    }

    #[test]
    fn a_strobe_alternating_every_frame_changes_no_faster_than_the_rate_cap() {
        for fps in [30.0, 60.0, 144.0, 240.0] {
            let dt = 1.0 / fps;
            let mut clamp = ChannelClamp::default();
            let mut previous = 0.0;
            for n in 0..(2.0 * fps) as usize {
                let mut uniform = flash([1.0; 3], (n % 2) as f32);
                clamp.apply(&mut uniform, &frame(dt, true));
                let step = (uniform.flash[3] - previous).abs();
                assert!(
                    step <= INTENSITY_RATE_PER_SECOND * dt + 1e-6,
                    "{fps} Hz frame {n}: step {step}"
                );
                previous = uniform.flash[3];
            }
        }
    }

    #[test]
    fn a_hitch_ages_out_earlier_transitions_and_caps_its_own_step() {
        let dt = 1.0 / 60.0;
        // Dark for half a second, then a 7.5 Hz strobe spends the whole
        // budget in the next half second. It settles dark for 0.1 s, then a
        // frame that jumps to full strength arrives after `gap` seconds and a
        // 5 Hz strobe runs for another second.
        let run = |gap: f32| {
            let mut clamp = ChannelClamp::default();
            let mut pack = |alpha: f32, dt: f32| {
                let mut uniform = flash([1.0; 3], alpha);
                clamp.apply(&mut uniform, &frame(dt, true));
                uniform.flash[3]
            };
            for n in 0..60 {
                let on = n >= 30 && ((n - 30) / 4) % 2 == 0;
                pack(if on { 1.0 } else { 0.0 }, dt);
            }
            let mut after = Vec::new();
            for _ in 0..6 {
                after.push(pack(0.0, dt));
            }
            after.push(pack(1.0, gap));
            for n in 1..60 {
                after.push(pack(square_5hz(n as f32 * dt), dt));
            }
            after
        };

        let hitched = run(2.0);
        let settled = hitched[5];
        assert_eq!(settled, 0.0, "the strobe settled dark before the hitch");
        let step = hitched[6] - settled;
        assert!(
            step > 0.0 && step <= INTENSITY_RATE_PER_SECOND * HITCH_CEILING_SECONDS + 1e-6,
            "the hitch frame steps {step}"
        );
        // A fresh budget: the onset that starts at the hitch is admitted, and
        // the second after it shows the full three flashes. The same frames at
        // a steady cadence hold that onset, because the window still holds the
        // strobe before it.
        let onset_peak = |alphas: &[f32]| alphas[6..12].iter().copied().fold(0.0, f32::max);
        assert!(onset_peak(&hitched) >= 0.3, "{hitched:?}");
        let steady = run(dt);
        assert!(
            onset_peak(&steady) < FLASH_LUMINANCE_THRESHOLD,
            "{steady:?}"
        );
        let fresh = worst_transitions(&hitched[5..], dt);
        assert!(
            (4..=6).contains(&fresh),
            "{fresh} transitions after the hitch"
        );
    }

    #[test]
    fn a_vignette_strobe_is_limited_on_its_own_channel() {
        let fps = 60.0;
        let dt = 1.0 / fps;
        let four_hz = |t: f32| if (t * 4.0).fract() < 0.5 { 1.0 } else { 0.0 };
        // The vignette strobes at 4 Hz; with `flash_too`, a 4 Hz flash strobe
        // spends the flash channel's budget out of phase with it.
        let run = |flash_too: bool| {
            let mut clamp = ChannelClamp::default();
            (0..(2.0 * fps) as usize)
                .map(|n| {
                    let t = n as f32 * dt;
                    let flash_a = if flash_too { four_hz(t + 0.125) } else { 0.0 };
                    let mut uniform = EffectUniform {
                        vignette: [0.0, 0.0, 0.0, four_hz(t)],
                        ..flash([1.0; 3], flash_a)
                    };
                    clamp.apply(&mut uniform, &frame(dt, true));
                    uniform
                })
                .collect::<Vec<_>>()
        };
        let alone = run(false);
        let strengths: Vec<f32> = alone.iter().map(|u| u.vignette[3]).collect();
        let worst = worst_transitions(&strengths, dt);
        assert!((4..=6).contains(&worst), "{worst} vignette transitions");
        // Its own window: a flash strobe beside it changes none of it.
        let beside = run(true);
        let flashes: Vec<f32> = beside.iter().map(|u| u.flash[3]).collect();
        assert!((4..=6).contains(&worst_transitions(&flashes, dt)));
        for (a, b) in alone.iter().zip(&beside) {
            assert_eq!(a.vignette, b.vignette);
        }
    }

    /// Pack a red flash and vignette strobe through the resolve's own path:
    /// the slots pack, the enable flag is read fail-safe, and the clamp
    /// limits. Returns what was authored and what packed, frame by frame.
    fn pack_slots(flag: Option<SlotValue>) -> Vec<(EffectUniform, EffectUniform)> {
        let dt = 1.0 / 60.0;
        let mut clamp = ChannelClamp::default();
        (0..120)
            .map(|n| {
                let t = n as f32 * dt;
                let mut slots = HashMap::from([
                    (
                        "screen.flash".to_string(),
                        SlotValue::Array(vec![1.0, 0.0, 0.0, square_5hz(t)]),
                    ),
                    (
                        "screen.vignette".to_string(),
                        SlotValue::Array(vec![1.0, 0.0, 0.0, square_5hz(t + 0.1)]),
                    ),
                ]);
                if let Some(flag) = &flag {
                    slots.insert(FLASH_LIMITER_SLOT.to_string(), flag.clone());
                }
                let authored = pack_effect_uniform(&slots);
                let frame = clamp.begin_frame(elapsed(dt), flash_limiter_enabled(&slots));
                let mut packed = authored;
                clamp.apply(&mut packed, &frame);
                (authored, packed)
            })
            .collect()
    }

    #[test]
    fn the_limiter_runs_when_its_flag_is_absent_or_malformed() {
        let dt = 1.0 / 60.0;
        for flag in [
            None,
            Some(SlotValue::Boolean(true)),
            Some(SlotValue::Number(0.0)),
            Some(SlotValue::String("off".into())),
        ] {
            let packed = pack_slots(flag.clone());
            let flashes: Vec<f32> = packed.iter().map(|(_, p)| p.flash[3]).collect();
            let vignettes: Vec<f32> = packed.iter().map(|(_, p)| p.vignette[3]).collect();
            assert!(worst_transitions(&flashes, dt) <= 6, "flash, {flag:?}");
            assert!(worst_transitions(&vignettes, dt) <= 6, "vignette, {flag:?}");
        }
    }

    #[test]
    fn an_explicit_false_packs_flash_and_vignette_as_authored() {
        let dt = 1.0 / 60.0;
        let packed = pack_slots(Some(SlotValue::Boolean(false)));
        for (authored, packed) in &packed {
            assert_eq!(packed, authored);
        }
        // What passed through is a strobe the limiter would have held.
        let flashes: Vec<f32> = packed.iter().map(|(_, p)| p.flash[3]).collect();
        let vignettes: Vec<f32> = packed.iter().map(|(_, p)| p.vignette[3]).collect();
        assert!(worst_transitions(&flashes, dt) > 6);
        assert!(worst_transitions(&vignettes, dt) > 6);
    }
}
