// Independent WCAG 2.2 flash counter for limiter tests. Counts transitions on
// read-back presented color, written from the WCAG definitions (general flash;
// red flash by CIE 1976 u′v′) rather than the limiter's own shader rules, so a
// test does not grade the limiter by its own detector.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

/// WCAG general flash: a change of at least 10% of maximum relative luminance.
const GENERAL_FLASH_DELTA: f32 = 0.1;
/// The darker state of a counted change must be below 0.80.
const DARKER_STATE_LIMIT: f32 = 0.8;

/// sRGB8 channel → linear [0, 1].
pub(crate) fn srgb8_to_linear(v: u8) -> f32 {
    let c = f32::from(v) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Counts opposing changes in a luminance time series. A change is measured
/// from the last extremum, peak to valley, so a smooth strobe counts the same
/// at any sample rate; a transition counts once per excursion.
///
/// A reversal ends an excursion only once it reaches the flash threshold
/// itself. WCAG counts a flash as a pair of opposing changes of at least 10%,
/// so a smaller dip is not an opposing change: a strobe that climbs in
/// sub-threshold steps with tiny dips between them is one excursion, not many
/// uncounted ones.
#[derive(Default)]
pub(crate) struct TransitionCounter {
    started: bool,
    extremum: f32,
    /// The furthest value reached in `direction` since `extremum`.
    peak: f32,
    direction: f32,
    counted: bool,
    times: Vec<f32>,
}

impl TransitionCounter {
    pub(crate) fn push(&mut self, time: f32, luminance: f32) {
        if !self.started {
            self.started = true;
            self.extremum = luminance;
            self.peak = luminance;
            return;
        }
        let beyond = (luminance - self.peak) * self.direction;
        if self.direction == 0.0 {
            if luminance != self.peak {
                self.direction = (luminance - self.peak).signum();
                self.peak = luminance;
            }
        } else if beyond > 0.0 {
            self.peak = luminance;
        } else if -beyond >= GENERAL_FLASH_DELTA {
            self.extremum = self.peak;
            self.peak = luminance;
            self.direction = -self.direction;
            self.counted = false;
        }
        let excursion = (luminance - self.extremum).abs();
        let darker = luminance.min(self.extremum);
        if !self.counted && excursion >= GENERAL_FLASH_DELTA && darker < DARKER_STATE_LIMIT {
            self.counted = true;
            self.times.push(time);
        }
    }

    pub(crate) fn transition_times(&self) -> &[f32] {
        &self.times
    }
}

/// WCAG 2.2 red flash (Note 3, ISO 9241-391): one state is saturated red,
/// R/(R+G+B) ≥ 0.8, and the two states differ by more than 0.2 in CIE 1976
/// u′v′ chromaticity.
const RED_SATURATION: f32 = 0.8;
const RED_UV_DISTANCE: f32 = 0.2;

/// Linear sRGB → CIE 1976 u′v′.
fn uv_prime(rgb: [f32; 3]) -> Option<[f32; 2]> {
    let [r, g, b] = rgb;
    let x = 0.4124 * r + 0.3576 * g + 0.1805 * b;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = 0.0193 * r + 0.1192 * g + 0.9505 * b;
    let d = x + 15.0 * y + 3.0 * z;
    (d > 1e-6).then(|| [4.0 * x / d, 9.0 * y / d])
}

fn saturated_red(rgb: [f32; 3]) -> bool {
    let total = rgb[0] + rgb[1] + rgb[2];
    total > 1e-4 && rgb[0] / total >= RED_SATURATION
}

/// Counts WCAG 2.2 red transitions in a series of mean presented colors: a
/// change of more than 0.2 in u′v′ from the last counted state, where one of
/// the two states is saturated red. Measured from the last counted state, not
/// the last reversal, so small dips never restart it and it needs no
/// hysteresis.
#[derive(Default)]
pub(crate) struct RedTransitionCounter {
    reference: Option<[f32; 3]>,
    times: Vec<f32>,
}

impl RedTransitionCounter {
    pub(crate) fn push(&mut self, time: f32, rgb: [f32; 3]) {
        let Some(reference) = self.reference else {
            self.reference = Some(rgb);
            return;
        };
        let (Some(a), Some(b)) = (uv_prime(reference), uv_prime(rgb)) else {
            return;
        };
        let distance = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
        if distance > RED_UV_DISTANCE && (saturated_red(reference) || saturated_red(rgb)) {
            self.times.push(time);
            self.reference = Some(rgb);
        }
    }

    pub(crate) fn transition_times(&self) -> &[f32] {
        &self.times
    }
}

/// The most transitions inside any one-second window. Three flashes is six
/// transitions.
pub(crate) fn max_transitions_in_any_second(times: &[f32]) -> usize {
    let mut worst = 0;
    for (i, &start) in times.iter().enumerate() {
        let in_window = times[i..].iter().take_while(|&&t| t < start + 1.0).count();
        worst = worst.max(in_window);
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_counts_a_smooth_sine_strobe_the_same_at_30_and_240_hz() {
        let count = |fps: f32| {
            let mut counter = TransitionCounter::default();
            for n in 0..=(fps as usize) {
                let t = n as f32 / fps;
                counter.push(t, 0.25 + 0.25 * (t * 5.0 * std::f32::consts::TAU).sin());
            }
            counter.transition_times().len()
        };
        assert_eq!(count(30.0), count(240.0));
        assert!(count(240.0) >= 9, "5 Hz sine over 1 s has ~10 transitions");
    }

    #[test]
    fn counter_ignores_changes_below_threshold_or_above_the_dark_limit() {
        let mut small = TransitionCounter::default();
        let mut bright = TransitionCounter::default();
        for n in 0..60 {
            let t = n as f32 / 60.0;
            small.push(t, if n % 2 == 0 { 0.2 } else { 0.25 });
            bright.push(t, if n % 2 == 0 { 0.85 } else { 1.0 });
        }
        assert!(small.transition_times().is_empty());
        assert!(bright.transition_times().is_empty());
    }

    #[test]
    fn counter_ignores_sub_threshold_dips_but_resets_on_a_threshold_reversal() {
        // 0 → 0.3 → 0 in rises of 0.065 with 0.001 dips between them: one
        // rise and one fall, not a run of uncounted 0.065 excursions.
        let mut dithered = TransitionCounter::default();
        let up = [
            0.0, 0.065, 0.064, 0.129, 0.128, 0.193, 0.192, 0.257, 0.256, 0.3,
        ];
        let series = up.iter().chain(up.iter().rev());
        for (n, &level) in series.enumerate() {
            dithered.push(n as f32, level);
        }
        assert_eq!(dithered.transition_times().len(), 2);

        // A slow reversal that reaches the threshold ends the excursion: each
        // 0.12 swing of a triangle counts.
        let mut triangle = TransitionCounter::default();
        for n in 0..=40 {
            let phase = (n % 8) as f32 / 4.0;
            let tri = if phase <= 1.0 { phase } else { 2.0 - phase };
            triangle.push(n as f32, 0.35 + 0.12 * tri);
        }
        assert_eq!(triangle.transition_times().len(), 10);
    }

    #[test]
    fn red_counter_counts_an_equal_brightness_red_green_flicker_but_not_gray() {
        // Red and green at equal relative luminance.
        let red = [1.0, 0.0, 0.0];
        let green = [0.0, 0.2126 / 0.7152, 0.0];
        let mut flicker = RedTransitionCounter::default();
        let mut gray = RedTransitionCounter::default();
        for n in 0..10 {
            let t = n as f32 * 0.1;
            flicker.push(t, if n % 2 == 0 { red } else { green });
            gray.push(t, if n % 2 == 0 { [0.2; 3] } else { [0.21; 3] });
        }
        assert_eq!(flicker.transition_times().len(), 9);
        assert!(gray.transition_times().is_empty());
    }
}
