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
#[derive(Default)]
pub(crate) struct TransitionCounter {
    last: Option<f32>,
    extremum: f32,
    direction: f32,
    counted: bool,
    times: Vec<f32>,
}

impl TransitionCounter {
    pub(crate) fn push(&mut self, time: f32, luminance: f32) {
        let Some(last) = self.last else {
            self.last = Some(luminance);
            self.extremum = luminance;
            return;
        };
        let step = luminance - last;
        if step != 0.0 && step.signum() != self.direction {
            if self.direction != 0.0 {
                self.extremum = last;
            }
            self.direction = step.signum();
            self.counted = false;
        }
        let excursion = (luminance - self.extremum).abs();
        let darker = luminance.min(self.extremum);
        if !self.counted && excursion >= GENERAL_FLASH_DELTA && darker < DARKER_STATE_LIMIT {
            self.counted = true;
            self.times.push(time);
        }
        self.last = Some(luminance);
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
/// the two states is saturated red.
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
