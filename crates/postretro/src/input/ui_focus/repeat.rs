// Hold-to-repeat clocks: one dt-clocked timer behind directional and
// confirm repeat.
// See: context/lib/ui.md §4

use super::traversal::Dir;

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

    /// Advance the timer by `dt` and return how many repeats fired this tick
    /// (robust to a large dt spanning multiple intervals). A non-positive threshold
    /// disables repeating; a pathological dt/interval ratio is clamped at 64 fires.
    pub(super) fn advance(&mut self, dt: f32) -> u32 {
        self.elapsed += dt;
        let mut fires = 0u32;
        loop {
            let threshold = if self.repeating {
                self.interval
            } else {
                self.initial_delay
            };
            if threshold <= 0.0 || self.elapsed < threshold {
                break;
            }
            self.elapsed -= threshold;
            self.repeating = true;
            fires += 1;
            if fires > 64 {
                // 64 is a conservative power-of-two bound well above any
                // realistic burst (e.g. ~0.3 fires/tick at a 50 ms interval /
                // 16 ms dt), guarding a pathological dt/interval ratio.
                self.elapsed = 0.0;
                break;
            }
        }
        fires
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
}

/// Hold-to-repeat clock for a held activation (confirm) on a `repeatOnHold`-flagged
/// button (M13 Text-Entry, Task 2 — the on-screen keyboard backspace). The ONE
/// activation-repeat exception: armed only when a confirm lands on a focused button
/// carrying a `repeat_on_hold` policy, it re-fires the button's activation on the
/// SAME [`RepeatTimer`] mechanics the nav repeat uses. A confirm release clears it
/// (mirroring how the directional release clears the nav clock).
#[derive(Debug, Clone)]
pub(super) struct ConfirmRepeatClock {
    pub(super) timer: RepeatTimer,
}
