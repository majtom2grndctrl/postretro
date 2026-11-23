// Per-texel specular sharpness and tint read from the specular slot's B and A channels.
// See: context/lib/resource_management.md §4.7 · rendering_pipeline.md §4

// The smoothness decode lives in level-format, where the compiler's glossy
// opt-in reads it too; re-exported here as the shader's reference.
pub use postretro_level_format::specular_smoothness::{
    SPECULAR_EXPONENT_MAX, SPECULAR_EXPONENT_MIN, SPECULAR_ROUGHNESS4_FLOOR,
    SPECULAR_SMOOTHNESS_MARKER, smoothness_exponent,
};

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
/// The exponent a texel shades at: its smoothness where B holds one, else the
/// material's prefix exponent.
pub fn texel_exponent(b: f32, prefix_exponent: f32) -> f32 {
    if b >= SPECULAR_SMOOTHNESS_MARKER {
        smoothness_exponent(b)
    } else {
        prefix_exponent
    }
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
