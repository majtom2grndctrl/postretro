// Hold-to-repeat clocks: one dt-clocked timer behind directional and
// confirm repeat.
// See: context/lib/ui.md §4

use postretro_ui::tree::RepeatPolicy;

use super::traversal::Dir;

/// Hold-to-repeat for a focus group that authors none: a console-convention
/// delay, then a steady interval.
pub(super) const ENGINE_DEFAULT_REPEAT: RepeatPolicy = RepeatPolicy {
    initial_delay_ms: 400.0,
    interval_ms: 100.0,
};

/// Held time after which a slider's repeat steps double, then quadruple.
const SLIDER_DOUBLE_AFTER_S: f32 = 1.0;
const SLIDER_QUADRUPLE_AFTER_S: f32 = 2.0;

/// The dt-clocked timing core shared by the directional-nav repeat and the
/// activation (confirm) repeat. Each engine tick adds `dt` to `elapsed`; once the
/// elapsed time passes the initial delay (then each interval after) it yields one
/// fire. Both hold-to-repeat consumers drive THIS one timer — there is no second
/// repeat clock — differing only in what a fire does (move focus vs. re-activate).
#[derive(Debug, Clone)]
pub(super) struct RepeatTimer {
    pub(super) initial_delay: f32,
    pub(super) interval: f32,
    /// Seconds since the press (or since the last emitted repeat after the delay).
    pub(super) elapsed: f32,
    /// Whether the initial delay has elapsed (we are in the steady interval phase).
    pub(super) repeating: bool,
}

impl RepeatTimer {
    /// Arm a fresh timer from a [`RepeatPolicy`] (milliseconds → seconds), before
    /// the initial delay has elapsed.
    pub(super) fn armed(initial_delay_ms: f32, interval_ms: f32) -> Self {
        Self {
            initial_delay: initial_delay_ms / 1000.0,
            interval: interval_ms / 1000.0,
            elapsed: 0.0,
            repeating: false,
        }
    }

    /// Advance the timer by `dt` and report whether a repeat fired this tick.
    /// At most one fires per tick, and a backlog from a long frame is dropped,
    /// so a repeat never lands focus or a value past what the player saw.
    /// A non-positive delay never repeats; a non-positive interval repeats once
    /// after the delay.
    pub(super) fn advance(&mut self, dt: f32) -> bool {
        self.elapsed += dt;
        let threshold = if self.repeating {
            self.interval
        } else {
            self.initial_delay
        };
        if threshold <= 0.0 || self.elapsed < threshold {
            return false;
        }
        self.elapsed -= threshold;
        if self.elapsed >= self.interval {
            self.elapsed = 0.0;
        }
        self.repeating = true;
        true
    }
}

/// Hold-to-repeat clock for a held directional nav. Only a directional press starts
/// one; confirm repeat for `repeatOnHold` buttons is handled by the separate
/// [`ConfirmRepeatClock`]. Releasing the direction (a return to no held direction)
/// clears it. Wraps the shared [`RepeatTimer`].
#[derive(Debug, Clone)]
pub(super) struct RepeatClock {
    pub(super) dir: Dir,
    pub(super) timer: RepeatTimer,
    /// Seconds held since arming; a held slider accelerates with it.
    pub(super) held: f32,
}

impl RepeatClock {
    pub(super) fn armed(dir: Dir, policy: RepeatPolicy) -> Self {
        Self {
            dir,
            timer: RepeatTimer::armed(policy.initial_delay_ms, policy.interval_ms),
            held: 0.0,
        }
    }

    /// Slider steps per repeat for the time held past the initial delay.
    pub(super) fn slider_multiplier(&self) -> i32 {
        let repeating_for = self.held - self.timer.initial_delay;
        if repeating_for >= SLIDER_QUADRUPLE_AFTER_S {
            4
        } else if repeating_for >= SLIDER_DOUBLE_AFTER_S {
            2
        } else {
            1
        }
    }
}

/// Hold-to-repeat clock for a held activation (confirm) on a `repeatOnHold`-flagged
/// button (the on-screen keyboard backspace). The ONE
/// activation-repeat exception: armed only when a confirm lands on a focused button
/// carrying a `repeat_on_hold` policy, it re-fires the button's activation on the
/// SAME [`RepeatTimer`] mechanics the nav repeat uses. A confirm release clears it
/// (mirroring how the directional release clears the nav clock).
#[derive(Debug, Clone)]
pub(super) struct ConfirmRepeatClock {
    pub(super) timer: RepeatTimer,
}
