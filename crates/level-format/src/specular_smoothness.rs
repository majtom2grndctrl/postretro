// The specular slot's smoothness channel decoded to a Blinn-Phong exponent, for compiler and runtime alike.
// See: context/lib/resource_management.md §4.7

/// The smallest Blinn-Phong exponent a smoothness byte decodes to.
pub const SPECULAR_EXPONENT_MIN: f32 = 1.0;
/// The cap: past it neither a pixel-style highlight nor a probe texel shows
/// more, so every byte that would decode higher reads as a mirror.
pub const SPECULAR_EXPONENT_MAX: f32 = 8192.0;
/// The floor on r⁴ before `2 / r⁴`: r⁴ at an exponent of 8198, a hair past the
/// cap, so the decode never divides by zero (byte 255, r = 0), and an
/// approximate GPU divide at the floor still lands above the cap and clamps
/// to it exactly. No authored byte changes: every r⁴ below about 2.44e-4
/// already decodes past 8192.
pub const SPECULAR_ROUGHNESS4_FLOOR: f32 = 2.0 / 8200.0;
/// B at or above this has smoothness: half a step above the B = 0
/// "no `_p`" marker, so filter rounding never crosses it.
pub const SPECULAR_SMOOTHNESS_MARKER: f32 = 0.5 / 255.0;

/// The one CPU decode: `material_shading.wgsl`'s `specular_texel_exponent`
/// mirrors it, the glossy opt-in reads it, and render-cpu re-exports it as
/// the shader's GPU-free reference.
///
/// The Blinn-Phong exponent of a sampled smoothness `b` (the slot's B, a
/// byte over 255): r = 1 − b, n = 2/r⁴ − 2, clamped to 1..=8192. The same
/// operations in the same order as the shader.
pub fn smoothness_exponent(b: f32) -> f32 {
    let r = 1.0 - b;
    let r2 = r * r;
    let r4 = (r2 * r2).max(SPECULAR_ROUGHNESS4_FLOOR);
    (2.0 / r4 - 2.0).clamp(SPECULAR_EXPONENT_MIN, SPECULAR_EXPONENT_MAX)
}

/// The exponent an authored smoothness byte shades at, read as the unorm
/// sample a magnified texel centre returns.
pub fn smoothness_byte_exponent(byte: u8) -> f32 {
    smoothness_exponent(f32::from(byte) / 255.0)
}

/// Whether a stored B byte carries smoothness, as the shader's marker test
/// reads its unorm sample: B = 0 is "no `_p`".
pub fn smoothness_byte_is_present(byte: u8) -> bool {
    f32::from(byte) / 255.0 >= SPECULAR_SMOOTHNESS_MARKER
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoothness_bytes_decode_to_the_conversion_curve() {
        assert_eq!(smoothness_byte_exponent(0), 1.0, "byte 0 is fully rough");
        assert_eq!(
            smoothness_byte_exponent(255),
            8192.0,
            "byte 255 (r = 0) is the cap"
        );
        let r = 127.0f64 / 255.0;
        let expected = 2.0 / r.powi(4) - 2.0;
        assert!(
            (f64::from(smoothness_byte_exponent(128)) - expected).abs() <= 1.0e-3,
            "byte 128 is 2/r⁴ − 2 at r = 127/255 ({expected})"
        );
        // Bytes 0–24 all shade at 1; 25 is the first above it.
        for byte in 0..=24 {
            assert_eq!(smoothness_byte_exponent(byte), 1.0, "byte {byte}");
        }
        assert!(smoothness_byte_exponent(25) > 1.0);
    }

    #[test]
    fn every_byte_past_the_cap_shades_at_the_cap() {
        let mut capped = 0;
        for byte in 0..=255u8 {
            let r = 1.0 - f64::from(byte) / 255.0;
            let exact = if r == 0.0 {
                f64::INFINITY
            } else {
                2.0 / r.powi(4) - 2.0
            };
            let decoded = smoothness_byte_exponent(byte);
            assert!(decoded.is_finite(), "byte {byte}");
            if exact >= 8192.0 {
                assert_eq!(decoded, 8192.0, "byte {byte} decodes past the cap");
                capped += 1;
            } else {
                assert!(decoded < 8192.0, "byte {byte}");
                assert!(
                    (f64::from(decoded) - exact.max(1.0)).abs() <= 1.0e-5 * exact.max(1.0),
                    "byte {byte}: {decoded} vs {exact}"
                );
            }
        }
        // r ≤ 0.125: bytes 224..=255.
        assert_eq!(capped, 32);
    }

    #[test]
    fn the_roughness_floor_lies_past_the_cap() {
        // At the floor the decode is past the cap, so it clamps to exactly
        // 8192 even with an approximate divide, and no byte below the cap
        // reaches the floor.
        const { assert!(2.0 / SPECULAR_ROUGHNESS4_FLOOR - 2.0 > SPECULAR_EXPONENT_MAX + 1.0) };
        let r = 32.0f32 / 255.0; // byte 223, the last below the cap
        assert!(r.powi(4) > SPECULAR_ROUGHNESS4_FLOOR);
    }

    #[test]
    fn smoothness_marker_separates_byte_zero_from_every_authored_byte() {
        assert!(!smoothness_byte_is_present(0), "B = 0 is no `_p`");
        for byte in 1..=255u8 {
            assert!(smoothness_byte_is_present(byte), "byte {byte}");
        }
    }
}
