//! Frame-continuous life-state time for the HUD, separate from gameplay time.
//! Ordinary prediction corrections change playback rate; only a caller-proven
//! life-state/pawn/teleport reset reseeds the displayed value.

use std::time::Duration;

const RESPONSE_MS: f64 = 250.0;
const MIN_RATE: f64 = 0.25;
const MAX_RATE: f64 = 1.25;

/// Preserve the fractional tick until the final UI conversion to f32. Gameplay
/// conditions instead floor the integer-tick conversion in the player slot.
pub(crate) fn predicted_target_ms(elapsed_ticks: u32, alpha: f32) -> f64 {
    (f64::from(elapsed_ticks) + f64::from(alpha.clamp(0.0, 1.0)))
        * postretro_foundation::TICK_DURATION.as_secs_f64()
        * 1000.0
}

/// Fixed-size presentation state, with no buffers or per-frame allocations.
///
/// The caller owns logical identity: compare resolved state names rather than
/// numeric ids, so a kept-name table remap preserves continuity. Increment the
/// serial for a changed pawn, name, teleport generation, or an authoritative
/// elapsed decrease. Replayed elapsed moving backwards is merely a correction.
#[derive(Debug, Default)]
pub(crate) struct LifeStateClock {
    displayed_ms: f64,
    reset_serial: Option<u64>,
}

impl LifeStateClock {
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// Advance once per rendered gameplay frame. A zero-duration read preserves
    /// both the display and its pending reset, which the next positive frame sees.
    pub(crate) fn advance(&mut self, frame_dt: Duration, target_ms: f64, reset_serial: u64) -> f64 {
        if frame_dt.is_zero() {
            return self.displayed_ms;
        }
        let target_ms = if target_ms.is_finite() {
            target_ms.max(0.0)
        } else {
            self.displayed_ms
        };
        if self.reset_serial != Some(reset_serial) {
            self.displayed_ms = target_ms;
            self.reset_serial = Some(reset_serial);
            return self.displayed_ms;
        }
        let dt_ms = frame_dt.as_secs_f64() * 1000.0;
        let error_ms = target_ms - (self.displayed_ms + dt_ms);
        // Exponential response remains stable for long game frames as well as
        // high refresh rates. Clamping the rate guarantees a positive advance
        // through backward corrections and bounds catch-up after packet loss.
        let response = -(-dt_ms / RESPONSE_MS).exp_m1();
        let rate = (1.0 + error_ms * response / dt_ms).clamp(MIN_RATE, MAX_RATE);
        self.displayed_ms += dt_ms * rate;
        self.displayed_ms
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_close(actual: f64, expected: f64, epsilon: f64) {
        assert!(
            (actual - expected).abs() <= epsilon,
            "{actual} != {expected}"
        );
    }

    #[test]
    fn life_state_clock_o25_rendered_value_advances_above_tick_rate_for_fifteen_seconds() {
        for fps in [30, 60, 144, 240, 360] {
            let dt = Duration::from_secs_f64(1.0 / f64::from(fps));
            let mut clock = LifeStateClock::default();
            clock.advance(dt, 0.0, 0);
            let mut previous = 0.0f32;
            let mut zero_tick_frames = 0;
            let mut previous_ticks = 0;
            for frame in 1..=15 * fps {
                let game_ms = f64::from(frame) * dt.as_secs_f64() * 1000.0;
                let tick_ms = postretro_foundation::TICK_DURATION.as_secs_f64() * 1000.0;
                let ticks = (game_ms / tick_ms).floor() as u32;
                let alpha = (game_ms / tick_ms - f64::from(ticks)) as f32;
                zero_tick_frames += u32::from(ticks == previous_ticks);
                previous_ticks = ticks;
                let target = predicted_target_ms(ticks, alpha);
                let displayed = clock.advance(dt, target, 0);
                let rendered = displayed as f32;
                assert!(
                    rendered > previous,
                    "{fps} Hz frame {frame}: {rendered} <= {previous}"
                );
                assert_close(displayed, game_ms, 0.002);
                let same = clock.advance(Duration::ZERO, target + 500.0, 99);
                assert_close(same, displayed, f64::EPSILON);
                previous = rendered;
            }
            if fps > 60 {
                assert!(zero_tick_frames > 0);
            }
        }
    }

    #[test]
    fn life_state_clock_o24_trim_and_delay_corrections_use_positive_bounded_rates() {
        for fps in [30, 60, 144, 240] {
            let dt = Duration::from_secs_f64(1.0 / f64::from(fps));
            let dt_ms = dt.as_secs_f64() * 1000.0;
            let mut clock = LifeStateClock::default();
            let mut previous = clock.advance(dt, 1000.0, 7);
            let mut previous_rendered = previous as f32;
            let mut offset = 0.0;
            for frame in 1..=15 * fps {
                // A trim removes counted prediction; later delayed data adds a
                // deficit. A reordered/omitted snapshot changes no reset serial.
                if frame == fps {
                    offset -= 350.0;
                }
                if frame == 5 * fps {
                    offset += 500.0;
                }
                if frame == 9 * fps {
                    offset -= 75.0;
                }
                let target = 1000.0 + f64::from(frame) * dt_ms + offset;
                let value = clock.advance(dt, target, 7);
                let delta = value - previous;
                assert!(delta > 0.0);
                assert!(delta >= dt_ms * MIN_RATE - 1e-8);
                assert!(delta <= dt_ms * MAX_RATE + 1e-8);
                assert!((value as f32) > previous_rendered);
                if frame == 4 * fps || frame == 15 * fps {
                    assert_close(value, target, 0.02);
                }
                if frame == 8 * fps {
                    assert_close(value, target, 0.5);
                }
                previous = value;
                previous_rendered = value as f32;
            }
        }
    }

    #[test]
    fn life_state_clock_reset_serial_reseeds_but_kept_name_remaps_preserve_continuity() {
        let dt = Duration::from_millis(10);
        let mut clock = LifeStateClock::default();
        clock.advance(dt, 4000.0, 1);
        // The caller keeps its serial when numeric ids/revisions remap a name.
        let kept_name = clock.advance(dt, 3900.0, 1);
        assert!(kept_name > 4000.0 && kept_name < 4010.0);
        // State transition, respawn/teleport, pawn change and an authoritative
        // elapsed decrease each arrive as a changed serial, including more than
        // one accepted reset in a single frame.
        for serial in [2, 3, 4, 7] {
            assert_close(clock.advance(dt, 16.667, serial), 16.667, 1e-9);
            assert!(clock.advance(dt, 26.667, serial) > 16.667);
        }
        let before = clock.advance(dt, 36.667, 7);
        assert_close(clock.advance(Duration::ZERO, 0.0, 8), before, 1e-9);
        assert_close(clock.advance(dt, 0.0, 8), 0.0, 1e-9);
        clock.clear();
        assert_close(clock.advance(dt, 500.0, 8), 500.0, 1e-9);
    }

    #[test]
    fn life_state_clock_positive_long_frames_remain_bounded_and_converge() {
        let mut clock = LifeStateClock::default();
        let dt = Duration::from_secs(1);
        let mut previous = clock.advance(dt, 1000.0, 1);
        for frame in 1..20 {
            let target = 1000.0 + f64::from(frame) * 1000.0 - 50.0;
            let next = clock.advance(dt, target, 1);
            assert!((250.0..=1250.0).contains(&(next - previous)));
            previous = next;
        }
        assert_close(previous, 19_950.0, 0.001);
    }
}
