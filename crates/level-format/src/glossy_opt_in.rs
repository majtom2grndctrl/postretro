// The glossy opt-in: which materials reflect their room, from their specular slot's bytes, and the sharpness fade.
// See: context/lib/resource_management.md §4.7, §5.4 · rendering_pipeline.md §7.3a

use crate::prm::PrmFormat;
use crate::specular_smoothness::{smoothness_byte_exponent, smoothness_byte_is_present};

/// At or below this exponent a texel reflects nothing: it keeps rough PBR
/// conversions, whose lobe would read as the SH ambient again, out of the
/// reflection.
pub const GLOSSY_SHARPNESS_FLOOR_EXPONENT: f32 = 8.0;
/// At or above this exponent a texel takes its full reflection weight.
pub const GLOSSY_FULL_WEIGHT_EXPONENT: f32 = 16.0;

/// A texel's reflection weight from its sharpness: 0 at or below the floor,
/// 1 at or above full weight, linear in log₂ of the exponent between, so a
/// sharpness gradient crossing the floor shows no seam.
pub fn glossy_sharpness_fade(exponent: f32) -> f32 {
    let floor = GLOSSY_SHARPNESS_FLOOR_EXPONENT.log2();
    let full = GLOSSY_FULL_WEIGHT_EXPONENT.log2();
    ((exponent.log2() - floor) / (full - floor)).clamp(0.0, 1.0)
}

/// Whether a material reflects, from its specular slot's mip-0 bytes: the
/// slot is the four-channel form (R `_s`, B smoothness) and some texel has
/// both specular above zero and an exponent strictly above the floor. A one-
/// or two-channel slot has no smoothness, so it never reflects.
///
/// The compiler applies it to place probes and the runtime loader to the same
/// bytes to mark glossy buckets: same bytes, same verdict, no format field.
pub fn specular_slot_reflects(format: PrmFormat, mip0: &[u8]) -> bool {
    if format != PrmFormat::Rgba8Unorm {
        return false;
    }
    // The decode rises with the byte, so one threshold replaces a decode per
    // texel.
    let Some(sharp_enough) = (0..=u8::MAX).find(|&b| texel_sharpness_reflects(b)) else {
        return false;
    };
    mip0.as_chunks::<4>()
        .0
        .iter()
        .any(|&[specular, _, smoothness, _]| specular > 0 && smoothness >= sharp_enough)
}

fn texel_sharpness_reflects(smoothness: u8) -> bool {
    smoothness_byte_is_present(smoothness)
        && smoothness_byte_exponent(smoothness) > GLOSSY_SHARPNESS_FLOOR_EXPONENT
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The last smoothness byte at or below the floor, and the first past it.
    fn floor_bytes() -> (u8, u8) {
        let first = (1..=u8::MAX)
            .find(|&b| smoothness_byte_exponent(b) > GLOSSY_SHARPNESS_FLOOR_EXPONENT)
            .expect("some byte decodes past the floor");
        (first - 1, first)
    }

    fn rgba(texels: &[[u8; 4]]) -> Vec<u8> {
        texels.concat()
    }

    #[test]
    fn specular_slot_without_smoothness_does_not_reflect() {
        let bytes = rgba(&[[255, 127, 0, 255]; 4]);
        assert!(!specular_slot_reflects(PrmFormat::Rgba8Unorm, &bytes));
    }

    #[test]
    fn specular_slot_without_specular_does_not_reflect() {
        let bytes = rgba(&[[0, 127, 255, 255]; 4]);
        assert!(!specular_slot_reflects(PrmFormat::Rgba8Unorm, &bytes));
    }

    #[test]
    fn specular_slot_with_texels_qualifying_on_one_condition_each_does_not_reflect() {
        let (at_floor, past_floor) = floor_bytes();
        // Specular but at the floor, and sharp but without specular.
        let bytes = rgba(&[[255, 127, at_floor, 255], [0, 127, past_floor, 255]]);
        assert!(!specular_slot_reflects(PrmFormat::Rgba8Unorm, &bytes));
    }

    #[test]
    fn specular_slot_with_one_texel_past_both_conditions_reflects() {
        let (_, past_floor) = floor_bytes();
        let bytes = rgba(&[
            [0, 127, 0, 255],
            [0, 127, 0, 255],
            [1, 127, past_floor, 255],
        ]);
        assert!(specular_slot_reflects(PrmFormat::Rgba8Unorm, &bytes));
    }

    #[test]
    fn specular_slot_at_the_floor_exponent_does_not_reflect() {
        let (at_floor, past_floor) = floor_bytes();
        assert!(smoothness_byte_exponent(at_floor) <= GLOSSY_SHARPNESS_FLOOR_EXPONENT);
        assert!(!specular_slot_reflects(
            PrmFormat::Rgba8Unorm,
            &[255, 127, at_floor, 255]
        ));
        assert!(specular_slot_reflects(
            PrmFormat::Rgba8Unorm,
            &[255, 127, past_floor, 255]
        ));
    }

    #[test]
    fn narrow_specular_slots_never_reflect() {
        // Bytes that would qualify in the four-channel form.
        let bytes = [255u8; 16];
        assert!(specular_slot_reflects(PrmFormat::Rgba8Unorm, &bytes));
        assert!(!specular_slot_reflects(PrmFormat::R8Unorm, &bytes));
        assert!(!specular_slot_reflects(PrmFormat::Rg8Unorm, &bytes));
    }

    #[test]
    fn glossy_sharpness_fade_is_zero_at_and_below_the_floor() {
        for exponent in [1.0, 4.0, 7.99, GLOSSY_SHARPNESS_FLOOR_EXPONENT] {
            assert_eq!(glossy_sharpness_fade(exponent), 0.0, "n = {exponent}");
        }
    }

    #[test]
    fn glossy_sharpness_fade_is_full_at_and_above_full_weight() {
        for exponent in [GLOSSY_FULL_WEIGHT_EXPONENT, 16.01, 256.0, 8192.0] {
            assert_eq!(glossy_sharpness_fade(exponent), 1.0, "n = {exponent}");
        }
    }

    #[test]
    fn glossy_sharpness_fade_rises_monotonically_and_continuously_in_log_exponent() {
        // Halfway in log₂ n is the geometric mean of the band's ends.
        let mid = (GLOSSY_SHARPNESS_FLOOR_EXPONENT * GLOSSY_FULL_WEIGHT_EXPONENT).sqrt();
        assert!((glossy_sharpness_fade(mid) - 0.5).abs() < 1.0e-6);
        // Steps of 1/1024 in log₂ n from 1 to the cap: never falling, and
        // never jumping more than the band's slope allows, across the floor
        // and the full-weight end included.
        let step = 1.0 / 1024.0f32;
        let mut previous = glossy_sharpness_fade(1.0);
        let mut log_n = step;
        while log_n <= 13.0 {
            let fade = glossy_sharpness_fade(log_n.exp2());
            assert!(fade >= previous, "falls at log₂ n = {log_n}");
            assert!(fade - previous <= step * 1.001, "jumps at log₂ n = {log_n}");
            previous = fade;
            log_n += step;
        }
    }
}
