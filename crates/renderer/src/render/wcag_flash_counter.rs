// Independent WCAG 2.2 flash counter for limiter tests. Counts transitions on
// read-back presented luminance, written from the WCAG definitions rather than
// the limiter's own shader rules, so a test does not grade the limiter by its
// own detector.
// See: context/plans/in-progress/E23--preferences-comfort-floor/research.md
//      §Limiter mechanism (Definitions)

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
}
