// Per-texel specular sharpness and tint read from the specular slot's B and A channels.
// See: context/lib/resource_management.md §4.7 · rendering_pipeline.md §4

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
/// A below this is tinted: half a step below the A = 255 "no `_t`" marker.
pub const SPECULAR_TINT_MARKER: f32 = 254.5 / 255.0;
/// Stored A runs 0..=254, so t = A·255/254 (`A / 254` in bytes).
pub const SPECULAR_TINT_SCALE: f32 = 255.0 / 254.0;
/// At or below this largest channel, an albedo has no hue to take and its
/// tinted highlight glints white. Below sRGB byte 1's linear value
/// (about 3.0e-4), so of 8-bit albedos only black falls under it.
pub const SPECULAR_HUE_EPSILON: f32 = 1.0e-4;

/// GPU-free CPU reference for `material_shading.wgsl`'s
/// `specular_texel_exponent` and `specular_tint`, which mirror this module and
/// a test pins.
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

/// The exponent a texel shades at: its smoothness where B holds one, else the
/// material's prefix exponent.
pub fn texel_exponent(b: f32, prefix_exponent: f32) -> f32 {
    if b >= SPECULAR_SMOOTHNESS_MARKER {
        smoothness_exponent(b)
    } else {
        prefix_exponent
    }
}

/// The exponent an authored smoothness byte shades at, read as the unorm
/// sample a magnified texel centre returns.
pub fn smoothness_byte_exponent(byte: u8) -> f32 {
    smoothness_exponent(f32::from(byte) / 255.0)
}

/// The tint amount of a sampled A, or `None` for an untinted texel, whose
/// highlight keeps the raw-albedo multiply.
pub fn tint_amount(a: f32) -> Option<f32> {
    (a < SPECULAR_TINT_MARKER).then(|| (a * SPECULAR_TINT_SCALE).min(1.0))
}

/// The colour a tinted highlight's intensity multiplies: white at t = 0, the
/// albedo's hue at full brightness (albedo over its largest channel) at
/// t = 1, white for an albedo with no hue. Linear albedo, as sampled.
pub fn tint_color(albedo: [f32; 3], t: f32) -> [f32; 3] {
    let peak = albedo[0].max(albedo[1]).max(albedo[2]);
    let hue = if peak > SPECULAR_HUE_EPSILON {
        albedo.map(|c| c / peak.max(SPECULAR_HUE_EPSILON))
    } else {
        [1.0; 3]
    };
    // WGSL `mix(e1, e2, t)` = e1·(1 − t) + e2·t.
    hue.map(|h| (1.0 - t) + h * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1.0e-6 * b.abs().max(1.0)
    }

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
    fn smoothness_marker_selects_the_texel_or_prefix_exponent() {
        // B = 0 is "no `_p`": the prefix exponent, whatever it is.
        assert_eq!(texel_exponent(0.0, 64.0), 64.0);
        assert_eq!(texel_exponent(0.0, 4.0), 4.0);
        // The first authored step, B = 1, is smoothness: exponent 1.
        assert_eq!(texel_exponent(1.0 / 255.0, 64.0), 1.0);
        assert_eq!(texel_exponent(1.0, 64.0), 8192.0);
        // Filter rounding half a step either side keeps its side.
        assert_eq!(texel_exponent(0.49 / 255.0, 64.0), 64.0);
        assert_eq!(texel_exponent(0.51 / 255.0, 64.0), 1.0);
    }

    #[test]
    fn tint_marker_separates_tinted_texels_from_the_raw_albedo_highlight() {
        assert_eq!(tint_amount(1.0), None, "A = 255 is no `_t`");
        assert_eq!(tint_amount(254.6 / 255.0), None);
        assert_eq!(tint_amount(0.0), Some(0.0));
        assert_eq!(tint_amount(254.0 / 255.0), Some(1.0), "stored 254 is t = 1");
        assert!(close(tint_amount(127.0 / 255.0).unwrap(), 127.0 / 254.0));
    }

    #[test]
    fn zero_tint_glints_white_on_any_albedo() {
        for albedo in [[0.1, 0.4, 0.2], [0.9, 0.5, 0.1], [0.0, 0.0, 0.0], [0.5; 3]] {
            assert_eq!(tint_color(albedo, 0.0), [1.0; 3], "{albedo:?}");
        }
    }

    #[test]
    fn full_tint_glints_one_hue_on_dark_and_bright_albedo() {
        let bright = [0.8, 0.4, 0.2];
        let dark = [0.2, 0.1, 0.05];
        assert_eq!(tint_color(bright, 1.0), tint_color(dark, 1.0));
        assert_eq!(tint_color(bright, 1.0), [1.0, 0.5, 0.25]);
        // Not power-of-two related: the same hue to rounding.
        let dim = bright.map(|c| c * 0.3);
        for (a, b) in tint_color(dim, 1.0).iter().zip(tint_color(bright, 1.0)) {
            assert!(close(*a, b));
        }
    }

    #[test]
    fn grey_and_black_albedo_glint_white() {
        for albedo in [
            [0.5; 3],
            [0.3; 3],
            [0.02; 3],
            [0.0; 3],
            [5.0e-5, 0.0, 2.0e-5],
        ] {
            for t in [0.25, 0.5, 1.0] {
                for c in tint_color(albedo, t) {
                    assert!(close(c, 1.0), "{albedo:?} at t = {t}");
                }
            }
        }
    }

    #[test]
    fn mid_tint_lies_between_white_and_the_hue() {
        let albedo = [0.6, 0.3, 0.15];
        let white = tint_color(albedo, 0.0);
        let hue = tint_color(albedo, 1.0);
        let mid = tint_color(albedo, 127.0 / 254.0);
        for c in 0..3 {
            let (lo, hi) = (white[c].min(hue[c]), white[c].max(hue[c]));
            assert!(lo <= mid[c] && mid[c] <= hi, "channel {c}");
        }
        assert!(mid[1] < white[1] && mid[1] > hue[1]);
    }
}
