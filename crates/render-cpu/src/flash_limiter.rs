// Photosensitivity limiter: the fail-safe enable flag, presented-frame time and
// the per-frame input the channel clamp (`flash_clamp.rs`) limits against.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

use std::collections::HashMap;

use postretro_entities::SlotValue;

/// The slot carrying the player's limiter setting. Readonly to scripts.
pub const FLASH_LIMITER_SLOT: &str = "accessibility.flashLimiter";

/// A presented frame longer than this is a hitch: its intensity allowance is
/// clamped to this span, while window aging still takes the full elapsed time.
/// 1/30 s is the slowest cadence the limiter's tests prove.
pub const HITCH_CEILING_SECONDS: f32 = 1.0 / 30.0;

/// Frame time assumed before the App reports one (the first frames of a fresh
/// renderer).
pub const DEFAULT_FRAME_SECONDS: f32 = 1.0 / 60.0;

/// What the App tells the limiter about the frame being presented. Time is
/// presented-frame time, never script time: dev tools freeze script time while
/// frames keep presenting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimiterFrameInput {
    /// Seconds since the previous resolve frame presented, splash frames
    /// between them included.
    pub elapsed_seconds: f32,
}

impl Default for LimiterFrameInput {
    fn default() -> Self {
        Self {
            elapsed_seconds: DEFAULT_FRAME_SECONDS,
        }
    }
}

impl LimiterFrameInput {
    /// Fold the next frame's input into one the renderer never consumed: a frame
    /// whose surface acquire failed never resolved, so its time still belongs to
    /// the next resolve frame. Elapsed time sums.
    pub fn merge(self, next: LimiterFrameInput) -> LimiterFrameInput {
        LimiterFrameInput {
            elapsed_seconds: finite_non_negative(self.elapsed_seconds)
                + finite_non_negative(next.elapsed_seconds),
        }
    }
}

/// The App's limiter input between `set` and the resolve that consumes it.
/// The App sets an input every frame it asks to render, but a frame whose
/// surface acquire fails never resolves; the next `set` merges into the
/// unconsumed input instead of replacing it, so no presented time is lost.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PendingLimiterFrame {
    pending: Option<LimiterFrameInput>,
}

impl PendingLimiterFrame {
    /// Record the input for the next resolve frame.
    pub fn set(&mut self, input: LimiterFrameInput) {
        self.pending = Some(match self.pending.take() {
            Some(unconsumed) => unconsumed.merge(input),
            None => input,
        });
    }

    /// Consume the input for the frame resolving now. A resolve with no input
    /// set takes the default frame time.
    pub fn take(&mut self) -> LimiterFrameInput {
        self.pending.take().unwrap_or_default()
    }
}

/// One presented frame as the limiter sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LimiterFrameUniform {
    /// Full elapsed presented time; ages the flash window.
    pub dt_window: f32,
    /// Elapsed time clamped to the hitch ceiling; scales the intensity allowance.
    pub dt_rate: f32,
    /// Off passes content unchanged, keeping no history.
    pub enabled: bool,
    /// Set on the frame history starts: a fresh limiter's first enabled frame,
    /// or the frame that turns the limiter back on. The limiter starts fresh
    /// against that frame's own values — an empty window, nothing to rate-cap
    /// from — so that frame presents unchanged and no earlier on-period's
    /// transitions count.
    pub init: bool,
}

/// Fail-safe read of the enable flag: an absent, non-boolean, or `true` value
/// leaves the limiter on. Only an explicit `false` turns it off.
pub fn flash_limiter_enabled(slot_values: &HashMap<String, SlotValue>) -> bool {
    !matches!(
        slot_values.get(FLASH_LIMITER_SLOT),
        Some(SlotValue::Boolean(false))
    )
}

/// Pack one frame's limiter input. A non-finite or negative elapsed time packs
/// as zero so a bad clock can never age the window backwards.
pub fn pack_limiter_frame(
    input: LimiterFrameInput,
    enabled: bool,
    init: bool,
) -> LimiterFrameUniform {
    let elapsed = finite_non_negative(input.elapsed_seconds);
    LimiterFrameUniform {
        dt_window: elapsed,
        dt_rate: elapsed.min(HITCH_CEILING_SECONDS),
        enabled,
        init,
    }
}

fn finite_non_negative(seconds: f32) -> f32 {
    if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_flag(value: SlotValue) -> HashMap<String, SlotValue> {
        HashMap::from([(FLASH_LIMITER_SLOT.to_string(), value)])
    }

    fn elapsed(seconds: f32) -> LimiterFrameInput {
        LimiterFrameInput {
            elapsed_seconds: seconds,
        }
    }

    #[test]
    fn flash_limiter_enabled_fails_safe_unless_explicitly_false() {
        assert!(flash_limiter_enabled(&HashMap::new()));
        assert!(flash_limiter_enabled(&with_flag(SlotValue::Boolean(true))));
        assert!(flash_limiter_enabled(&with_flag(SlotValue::Number(0.0))));
        assert!(flash_limiter_enabled(&with_flag(SlotValue::String(
            "false".into()
        ))));
        assert!(!flash_limiter_enabled(&with_flag(SlotValue::Boolean(
            false
        ))));
    }

    #[test]
    fn pack_limiter_frame_clamps_rate_time_to_hitch_ceiling_but_not_window_time() {
        let frame = pack_limiter_frame(elapsed(2.0), true, false);
        assert_eq!(frame.dt_window, 2.0);
        assert!((frame.dt_rate - HITCH_CEILING_SECONDS).abs() < 1e-9);

        let steady = pack_limiter_frame(elapsed(1.0 / 240.0), true, false);
        assert_eq!(steady.dt_window, steady.dt_rate);
    }

    #[test]
    fn a_skipped_frame_leaves_its_input_for_the_next_resolve_which_takes_it_once() {
        let mut pending = PendingLimiterFrame::default();
        pending.set(elapsed(0.02));
        // Acquire fails: nothing is taken. The next frame's time adds to it.
        pending.set(elapsed(0.03));
        let consumed = pending.take();
        assert!((consumed.elapsed_seconds - 0.05).abs() < 1e-6);
        // Consumed exactly once: the following resolve starts clean.
        assert_eq!(pending.take(), LimiterFrameInput::default());
        pending.set(elapsed(0.01));
        assert_eq!(pending.take(), elapsed(0.01));
    }

    #[test]
    fn merging_ignores_a_non_finite_or_negative_time() {
        for bad in [f32::NAN, f32::INFINITY, -0.5] {
            assert_eq!(elapsed(0.1).merge(elapsed(bad)), elapsed(0.1));
            assert_eq!(elapsed(bad).merge(elapsed(0.1)), elapsed(0.1));
        }
    }

    #[test]
    fn pack_limiter_frame_zeroes_non_finite_or_negative_time() {
        for bad in [f32::NAN, f32::INFINITY, -0.5] {
            let frame = pack_limiter_frame(elapsed(bad), true, false);
            assert_eq!(frame.dt_window, 0.0);
            assert_eq!(frame.dt_rate, 0.0);
        }
    }
}
