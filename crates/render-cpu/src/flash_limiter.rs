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

/// What the App tells the limiter about the frame being presented. Time is
/// presented-frame time, never script time: dev tools freeze script time while
/// frames keep presenting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimiterFrameInput {
    /// Seconds since the previous resolve frame presented.
    pub elapsed_seconds: f32,
}

impl Default for LimiterFrameInput {
    fn default() -> Self {
        Self {
            elapsed_seconds: DEFAULT_FRAME_SECONDS,
        }
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
    /// 0 passes content unchanged (both stages off).
    pub enabled: u32,
    /// 1 on the frame history starts: the first enabled frame, never reusing an
    /// earlier on-period's transitions.
    pub reset: u32,
    /// 1 on a fresh limiter's first frame: adopt the measured frame as the last
    /// presented one rather than ramping from black.
    pub init: u32,
    pub _pad: [u32; 3],
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
    reset: bool,
    init: bool,
) -> LimiterFrameUniform {
    let elapsed = if input.elapsed_seconds.is_finite() {
        input.elapsed_seconds.max(0.0)
    } else {
        0.0
    };
    LimiterFrameUniform {
        dt_window: elapsed,
        dt_rate: elapsed.min(HITCH_CEILING_SECONDS),
        enabled: u32::from(enabled),
        reset: u32::from(reset),
        init: u32::from(init),
        _pad: [0; 3],
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
            },
            true,
            false,
            false,
        );
        assert_eq!(frame.dt_window, 2.0);
        assert!((frame.dt_rate - HITCH_CEILING_SECONDS).abs() < 1e-9);

        let steady = pack_limiter_frame(
            LimiterFrameInput {
                elapsed_seconds: 1.0 / 240.0,
            },
            true,
            false,
            false,
        );
        assert_eq!(steady.dt_window, steady.dt_rate);
    }

    #[test]
    fn pack_limiter_frame_zeroes_non_finite_or_negative_time() {
        for bad in [f32::NAN, f32::INFINITY, -0.5] {
            let frame = pack_limiter_frame(
                LimiterFrameInput {
                    elapsed_seconds: bad,
                },
                true,
                false,
                false,
            );
            assert_eq!(frame.dt_window, 0.0);
            assert_eq!(frame.dt_rate, 0.0);
        }
    }
}
