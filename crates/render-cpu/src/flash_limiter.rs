// Photosensitivity flash limiter: per-frame uniform packing and the fail-safe
// enable flag. The limiter's rules run on the GPU (`flash_limiter.wgsl`).
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use postretro_entities::SlotValue;

/// The slot carrying the player's limiter setting. Readonly to scripts.
pub const FLASH_LIMITER_SLOT: &str = "accessibility.flashLimiter";

/// Measurement grid: a fixed 16×9 at any resolution and aspect, so each cell is
/// 1/144 of the frame and about sixteen cells make up the WCAG flash-area
/// threshold. Mirrored as constants in `screen_effects.wgsl`.
pub const LIMITER_CELLS_X: u32 = 16;
pub const LIMITER_CELLS_Y: u32 = 9;
pub const LIMITER_CELL_COUNT: u32 = LIMITER_CELLS_X * LIMITER_CELLS_Y;

/// A presented frame longer than this is a hitch: its intensity allowance is
/// clamped to this span, while window aging still takes the full elapsed time.
/// 1/30 s is the slowest cadence the limiter's tests prove.
pub const HITCH_CEILING_SECONDS: f32 = 1.0 / 30.0;

/// Frame time assumed before the App reports one (the first frames of a fresh
/// renderer).
pub const DEFAULT_FRAME_SECONDS: f32 = 1.0 / 60.0;

/// A stretch of splash frames between two resolve frames. The splash path
/// writes the swapchain without the limiter, so the App hands the first resolve
/// frame after it what the player saw and when it began.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplashHandOff {
    /// The splash's presented color, linear.
    pub rgb: [f32; 3],
    /// Seconds since the stretch began.
    pub seconds: f32,
}

/// What the App tells the limiter about the frame being presented. Time is
/// presented-frame time, never script time: dev tools freeze script time while
/// frames keep presenting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimiterFrameInput {
    /// Seconds since the previous resolve frame presented, splash frames
    /// between them included.
    pub elapsed_seconds: f32,
    /// Splash frames presented since the previous resolve frame.
    pub splash: Option<SplashHandOff>,
}

impl Default for LimiterFrameInput {
    fn default() -> Self {
        Self {
            elapsed_seconds: DEFAULT_FRAME_SECONDS,
            splash: None,
        }
    }
}

impl LimiterFrameInput {
    /// Fold the next frame's input into one the renderer never consumed: a frame
    /// whose surface acquire failed never resolved, so its time and any splash
    /// stretch it carried still belong to the next resolve frame. Elapsed time
    /// sums. The earliest stretch is kept and ages by the time that followed
    /// it, so the hand-off reaches the limiter at its true age; the color is the
    /// latest splash the player saw.
    pub fn merge(self, next: LimiterFrameInput) -> LimiterFrameInput {
        let next_elapsed = finite_non_negative(next.elapsed_seconds);
        let splash = match (self.splash, next.splash) {
            (Some(earliest), latest) => Some(SplashHandOff {
                rgb: latest.map_or(earliest.rgb, |latest| latest.rgb),
                seconds: finite_non_negative(earliest.seconds) + next_elapsed,
            }),
            (None, latest) => latest,
        };
        LimiterFrameInput {
            elapsed_seconds: finite_non_negative(self.elapsed_seconds) + next_elapsed,
            splash,
        }
    }
}

/// The App's limiter input between `set` and the resolve that consumes it.
/// The App sets an input every frame it asks to render, but a frame whose
/// surface acquire fails never resolves; the next `set` merges into the
/// unconsumed input instead of replacing it, so no presented time and no
/// splash hand-off is lost.
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
    /// set takes the default frame time and no splash.
    pub fn take(&mut self) -> LimiterFrameInput {
        self.pending.take().unwrap_or_default()
    }
}

/// Mirrors `LimiterFrame` in `flash_limiter.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct LimiterFrameUniform {
    /// Full elapsed presented time; ages the flash window.
    pub dt_window: f32,
    /// Elapsed time clamped to the hitch ceiling; scales the intensity allowance.
    pub dt_rate: f32,
    /// 0 passes content unchanged (both stages off, keeping no history).
    pub enabled: u32,
    /// 1 on the frame history starts: a fresh limiter's first enabled frame, or
    /// the frame that turns the limiter back on. Both stages start fresh against
    /// that frame's own values — an empty window, nothing to rate-cap from — so
    /// it presents unchanged, and no earlier on-period's transitions count.
    pub init: u32,
    /// 1 when a splash stretch preceded this frame.
    pub splash_active: u32,
    /// Seconds since that stretch began.
    pub splash_seconds: f32,
    pub _pad0: u32,
    pub _pad1: u32,
    /// The splash's presented color (linear), `w` unused.
    pub splash_rgb: [f32; 4],
}

/// Fail-safe read of the enable flag: an absent, non-boolean, or `true` value
/// leaves the limiter on. Only an explicit `false` turns it off.
pub fn flash_limiter_enabled(slot_values: &HashMap<String, SlotValue>) -> bool {
    !matches!(
        slot_values.get(FLASH_LIMITER_SLOT),
        Some(SlotValue::Boolean(false))
    )
}

/// Pack one frame's limiter uniform. A non-finite or negative elapsed time
/// packs as zero so a bad clock can never age the window backwards.
pub fn pack_limiter_frame(
    input: LimiterFrameInput,
    enabled: bool,
    init: bool,
) -> LimiterFrameUniform {
    let elapsed = finite_non_negative(input.elapsed_seconds);
    LimiterFrameUniform {
        dt_window: elapsed,
        dt_rate: elapsed.min(HITCH_CEILING_SECONDS),
        enabled: u32::from(enabled),
        init: u32::from(init),
        splash_active: u32::from(input.splash.is_some()),
        splash_seconds: input
            .splash
            .map_or(0.0, |splash| finite_non_negative(splash.seconds)),
        _pad0: 0,
        _pad1: 0,
        splash_rgb: input.splash.map_or([0.0; 4], |splash| {
            [splash.rgb[0], splash.rgb[1], splash.rgb[2], 0.0]
        }),
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
        let frame = pack_limiter_frame(
            LimiterFrameInput {
                elapsed_seconds: 2.0,
                splash: None,
            },
            true,
            false,
        );
        assert_eq!(frame.dt_window, 2.0);
        assert!((frame.dt_rate - HITCH_CEILING_SECONDS).abs() < 1e-9);

        let steady = pack_limiter_frame(
            LimiterFrameInput {
                elapsed_seconds: 1.0 / 240.0,
                splash: None,
            },
            true,
            false,
        );
        assert_eq!(steady.dt_window, steady.dt_rate);
    }

    #[test]
    fn an_unconsumed_splash_hand_off_survives_a_skipped_frame_at_its_true_age() {
        let splash_rgb = [0.01, 0.02, 0.03];
        // The first resolve frame after a splash stretch is set, but its
        // acquire fails; the next frame carries no stretch of its own.
        let skipped = LimiterFrameInput {
            elapsed_seconds: 0.5,
            splash: Some(SplashHandOff {
                rgb: splash_rgb,
                seconds: 0.4,
            }),
        };
        let next = LimiterFrameInput {
            elapsed_seconds: 1.0 / 60.0,
            splash: None,
        };
        let consumed = skipped.merge(next);
        assert!((consumed.elapsed_seconds - (0.5 + 1.0 / 60.0)).abs() < 1e-6);
        let splash = consumed.splash.expect("the stretch is still owed");
        assert_eq!(splash.rgb, splash_rgb);
        assert!((splash.seconds - (0.4 + 1.0 / 60.0)).abs() < 1e-6);

        // A second skip keeps aging the same stretch; a later stretch does not
        // replace it, and the latest splash color wins.
        let later = LimiterFrameInput {
            elapsed_seconds: 0.25,
            splash: Some(SplashHandOff {
                rgb: [0.5; 3],
                seconds: 0.2,
            }),
        };
        let consumed = consumed.merge(later);
        let splash = consumed.splash.expect("the earliest stretch is kept");
        assert!((splash.seconds - (0.4 + 1.0 / 60.0 + 0.25)).abs() < 1e-6);
        assert_eq!(splash.rgb, [0.5; 3]);
        assert!((consumed.elapsed_seconds - (0.5 + 1.0 / 60.0 + 0.25)).abs() < 1e-6);
    }

    #[test]
    fn a_skipped_frame_leaves_its_input_for_the_next_resolve_which_takes_it_once() {
        let mut pending = PendingLimiterFrame::default();
        pending.set(LimiterFrameInput {
            elapsed_seconds: 0.02,
            splash: Some(SplashHandOff {
                rgb: [0.0; 3],
                seconds: 2.0,
            }),
        });
        // Acquire fails: nothing is taken. The next frame sets no splash.
        pending.set(LimiterFrameInput {
            elapsed_seconds: 0.03,
            splash: None,
        });
        let consumed = pending.take();
        assert!((consumed.elapsed_seconds - 0.05).abs() < 1e-6);
        let splash = consumed.splash.expect("the hand-off reaches the resolve");
        assert!((splash.seconds - 2.03).abs() < 1e-6);
        // Consumed exactly once: the following resolve starts clean.
        assert_eq!(pending.take(), LimiterFrameInput::default());
        pending.set(LimiterFrameInput {
            elapsed_seconds: 0.01,
            splash: None,
        });
        assert_eq!(
            pending.take(),
            LimiterFrameInput {
                elapsed_seconds: 0.01,
                splash: None,
            }
        );
    }

    #[test]
    fn merging_into_an_input_without_a_stretch_takes_the_next_stretch() {
        let pending = LimiterFrameInput {
            elapsed_seconds: 0.1,
            splash: None,
        };
        let next = LimiterFrameInput {
            elapsed_seconds: 0.3,
            splash: Some(SplashHandOff {
                rgb: [0.0; 3],
                seconds: 0.25,
            }),
        };
        let merged = pending.merge(next);
        assert!((merged.elapsed_seconds - 0.4).abs() < 1e-6);
        assert_eq!(merged.splash, next.splash);
    }

    #[test]
    fn pack_limiter_frame_zeroes_non_finite_or_negative_time() {
        for bad in [f32::NAN, f32::INFINITY, -0.5] {
            let frame = pack_limiter_frame(
                LimiterFrameInput {
                    elapsed_seconds: bad,
                    splash: None,
                },
                true,
                false,
            );
            assert_eq!(frame.dt_window, 0.0);
            assert_eq!(frame.dt_rate, 0.0);
        }
    }
}
