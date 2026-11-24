// Glossy reflection weight, colour and composite reference tests.
// See: context/lib/testing_guide.md

use super::*;
use crate::fluid_surface::reflection::REFLECTION_SCREEN_MISS;
use postretro_level_format::specular_smoothness::smoothness_byte_exponent;

/// A slot sample of `_s` byte `specular`, smoothness byte `smoothness` and
/// tint byte `tint` (255 = no `_t`).
fn slot(specular: u8, smoothness: u8, tint: u8) -> Vec4 {
    Vec4::new(
        f32::from(specular) / 255.0,
        0.5,
        f32::from(smoothness) / 255.0,
        f32::from(tint) / 255.0,
    )
}

/// The smoothness byte that decodes to the cap: sharp enough for full weight.
const SHARP: u8 = 255;
const NO_TINT: u8 = 255;
const FULL_TINT: u8 = 254;
const ALBEDO: Vec3 = Vec3::new(0.6, 0.3, 0.15);

/// The last smoothness byte at or below the floor, then the first past it.
fn floor_bytes() -> (u8, u8) {
    let first = (1..=u8::MAX)
        .find(|&b| smoothness_byte_exponent(b) > GLOSSY_SHARPNESS_FLOOR_EXPONENT)
        .expect("a byte decodes past the floor");
    (first - 1, first)
}

/// The weight F of a full-`_s`, untinted texel of smoothness byte `b` at
/// `cos_theta`.
fn weight_at(b: u8, cos_theta: f32) -> f32 {
    glossy_fresnel(&glossy_texel(slot(255, b, NO_TINT), ALBEDO), cos_theta)
}

fn close(a: f32, b: f32, tolerance: f32) -> bool {
    (a - b).abs() <= tolerance
}

// Acceptance row 4: a texel at or below the floor draws no term, one past
// the band the full term, one inside the band a fraction rising with its
// exponent.
#[test]
fn sharpness_fade_withholds_the_floor_rises_through_the_band_and_fills_past_it() {
    let (at_floor, past_floor) = floor_bytes();
    for b in 0..=at_floor {
        assert_eq!(
            glossy_texel(slot(255, b, NO_TINT), ALBEDO).mask,
            0.0,
            "byte {b}"
        );
    }
    let full = (1..=u8::MAX)
        .find(|&b| smoothness_byte_exponent(b) >= GLOSSY_FULL_WEIGHT_EXPONENT)
        .expect("a byte decodes past the band");
    for b in full..=u8::MAX {
        assert_eq!(
            glossy_texel(slot(255, b, NO_TINT), ALBEDO).mask,
            1.0,
            "byte {b}"
        );
    }
    let mut previous = 0.0;
    for b in past_floor..full {
        let mask = glossy_texel(slot(255, b, NO_TINT), ALBEDO).mask;
        assert!(
            mask > previous && mask < 1.0,
            "byte {b}: {mask} after {previous}"
        );
        previous = mask;
    }
    assert!(
        full - past_floor >= 8,
        "the band spans bytes: {past_floor}..{full}"
    );
}

// Acceptance row 4: at grazing, where Fresnel reaches 1 and the fade alone
// shapes the weight, adjacent texels across the floor contour differ by no
// more than adjacent texels inside the band.
#[test]
fn adjacent_texels_across_the_floor_contour_differ_no_more_than_inside_the_band() {
    let (at_floor, past_floor) = floor_bytes();
    let full = (1..=u8::MAX)
        .find(|&b| smoothness_byte_exponent(b) >= GLOSSY_FULL_WEIGHT_EXPONENT)
        .expect("a byte decodes past the band");
    for cos_theta in [0.0, 0.01, 0.05, 0.2, 1.0] {
        let contour = weight_at(past_floor, cos_theta) - weight_at(at_floor, cos_theta);
        let inside = (past_floor..full - 1)
            .map(|b| weight_at(b + 1, cos_theta) - weight_at(b, cos_theta))
            .fold(f32::INFINITY, f32::min);
        assert!(contour > 0.0, "the contour steps up at cos {cos_theta}");
        assert!(
            contour <= inside,
            "at cos {cos_theta}: the contour step {contour} passes the band's smallest {inside}"
        );
    }
    // Grazing: Fresnel is 1, so the weight is the fade itself.
    assert_eq!(weight_at(SHARP, 0.0), 1.0);
}

// Acceptance row 4: `_s` 0 draws no term in either layer; halving `_s`
// halves the weight.
#[test]
fn specular_zero_draws_nothing_and_half_specular_halves_the_weight() {
    let d_static = Vec3::new(0.3, 0.2, 0.1);
    let probe = Vec3::new(0.9, 0.8, 0.7);
    let screen_hit = Vec4::new(0.4, 0.4, 0.4, 0.5);
    for cos_theta in [0.0, 0.3, 1.0] {
        let none = glossy_texel(slot(0, SHARP, FULL_TINT), ALBEDO);
        let fresnel = glossy_fresnel(&none, cos_theta);
        assert_eq!(fresnel, 0.0);
        for screen in [REFLECTION_SCREEN_MISS, screen_hit] {
            assert_eq!(
                glossy_composite(d_static, probe, 1.0, screen, fresnel, none.colour),
                Vec3::ZERO,
                "no term in either layer at cos {cos_theta}"
            );
        }
        for (whole, half) in [(254u8, 127u8), (200, 100), (2, 1)] {
            let w = glossy_fresnel(
                &glossy_texel(slot(whole, SHARP, NO_TINT), ALBEDO),
                cos_theta,
            );
            let h = glossy_fresnel(&glossy_texel(slot(half, SHARP, NO_TINT), ALBEDO), cos_theta);
            assert_eq!(h * 2.0, w, "byte {whole} against {half} at cos {cos_theta}");
        }
    }
}

// Acceptance row 34: head-on, an untinted texel reflects at 0.04 and a
// tint-1 texel at 1; a texel without `_t` at 0.04, coloured by raw albedo.
// At grazing all reach 1. These hold at `_s` 1.
#[test]
fn strength_is_water_fresnel_from_the_untinted_base_to_one_at_full_tint() {
    let untinted = glossy_texel(slot(255, SHARP, 0), ALBEDO);
    let full = glossy_texel(slot(255, SHARP, FULL_TINT), ALBEDO);
    let no_tint = glossy_texel(slot(255, SHARP, NO_TINT), ALBEDO);
    assert!(close(glossy_fresnel(&untinted, 1.0), 0.04, 1.0e-7));
    assert_eq!(glossy_fresnel(&full, 1.0), 1.0);
    assert!(close(glossy_fresnel(&no_tint, 1.0), 0.04, 1.0e-7));
    for texel in [untinted, full, no_tint] {
        assert_eq!(glossy_fresnel(&texel, 0.0), 1.0, "grazing reaches 1");
        let mut previous = glossy_fresnel(&texel, 1.0);
        for step in (0..100).rev() {
            let f = glossy_fresnel(&texel, step as f32 / 100.0);
            assert!(f >= previous, "rises toward grazing");
            previous = f;
        }
    }
    // A mid tint lies between: water's scale, linear in the tint.
    let mid = glossy_texel(slot(255, SHARP, 127), ALBEDO);
    let t = 127.0 / 254.0;
    assert!(close(glossy_fresnel(&mid, 1.0), 0.04 + 0.96 * t, 1.0e-6));
    // The same function as water's at the same reflectivity.
    assert_eq!(
        glossy_fresnel(&full, 0.37),
        reflection_fresnel(0.37, FLUID_REFLECTIVITY_MAX)
    );
}

// Acceptance rows 27, 34: the reflection takes its highlight's colour: the
// tint rule's colour for a tinted texel (white at t = 0, the albedo's hue at
// t = 1), raw albedo without `_t`.
#[test]
fn reflection_colour_is_the_highlights_colour() {
    assert_eq!(glossy_texel(slot(255, SHARP, 0), ALBEDO).colour, Vec3::ONE);
    assert_eq!(
        glossy_texel(slot(255, SHARP, FULL_TINT), ALBEDO).colour,
        Vec3::new(1.0, 0.5, 0.25)
    );
    assert_eq!(
        glossy_texel(slot(255, SHARP, NO_TINT), ALBEDO).colour,
        ALBEDO
    );
    let mid = glossy_texel(slot(255, SHARP, 127), ALBEDO).colour;
    assert_eq!(mid.to_array(), tint_color(ALBEDO.to_array(), 127.0 / 254.0));
    // Both layers take it.
    let screen = Vec4::new(0.5, 0.5, 0.5, 1.0);
    let colour = Vec3::new(1.0, 0.5, 0.25);
    let tinted = glossy_composite(Vec3::ZERO, Vec3::ONE, 1.0, screen, 1.0, colour);
    assert_eq!(tinted, colour * 0.5);
    let probe_only = glossy_composite(
        Vec3::ZERO,
        Vec3::ONE,
        1.0,
        REFLECTION_SCREEN_MISS,
        1.0,
        colour,
    );
    assert_eq!(probe_only, colour);
}

// Acceptance row 34: the probe share scales by the capture-shadow weight;
// at c = 0 the composite is s·F·(colour·R − D_static), and s = 0 adds
// exactly nothing (withheld, never filled).
#[test]
fn capture_shadow_scales_the_probe_share_and_withholds_at_zero() {
    let d_static = Vec3::new(0.3, 0.2, 0.1);
    let probe = Vec3::new(0.9, 0.8, 0.7);
    let colour = Vec3::new(1.0, 0.5, 0.25);
    let fresnel = 0.75;
    for shadow in [0.0, 0.25, 0.5, 1.0] {
        let delta = glossy_composite(
            d_static,
            probe,
            shadow,
            REFLECTION_SCREEN_MISS,
            fresnel,
            colour,
        );
        let expected = (colour * probe - d_static) * (fresnel * shadow);
        assert!(
            (delta - expected).abs().max_element() <= 1.0e-6,
            "s = {shadow}"
        );
    }
    assert_eq!(
        glossy_composite(
            d_static,
            probe,
            0.0,
            REFLECTION_SCREEN_MISS,
            fresnel,
            colour
        ),
        Vec3::ZERO
    );
    // IP rule: the screen-space contribution is the same at every s.
    let screen = Vec4::new(0.4, 0.2, 0.1, 1.0);
    let at = |shadow| glossy_composite(d_static, probe, shadow, screen, fresnel, colour);
    assert_eq!(at(0.0), at(1.0));
}

// Acceptance row 30: static diffuse (a character's promoted-light shadow
// included) fades by (1 − W), and no output channel falls below 0 over a dark
// reflected ceiling, head-on or grazing, untinted or tint-1, while D_static
// is stored no larger than forward's colour.
#[test]
fn static_diffuse_fades_by_one_minus_w_and_no_channel_undershoots() {
    let probe = Vec3::ZERO;
    for tint in [0u8, FULL_TINT, NO_TINT] {
        let texel = glossy_texel(slot(255, SHARP, tint), ALBEDO);
        for cos_theta in [1.0, 0.5, 0.05, 0.0] {
            let fresnel = glossy_fresnel(&texel, cos_theta);
            for d in [0.0, 1.0e-6, 0.01, 0.5, 1.0, 4.0, 1000.0] {
                for rest in [0.0, 0.25] {
                    let d_static = Vec3::splat(d);
                    let forward = d_static + rest;
                    let output = forward
                        + glossy_composite(
                            d_static,
                            probe,
                            1.0,
                            REFLECTION_SCREEN_MISS,
                            fresnel,
                            texel.colour,
                        );
                    assert!(
                        output.min_element() >= 0.0,
                        "tint {tint} cos {cos_theta} d {d} rest {rest}: {output}"
                    );
                    // The static term's share left is (1 − W).
                    let expected = d * (1.0 - fresnel) + rest;
                    assert!(close(output.x, expected, 1.0e-6 * expected.max(1.0)));
                }
            }
        }
    }
}

// The weight never passes 1, even where rounding would lift F·[c + (1 − c)·s].
#[test]
fn composite_weight_is_capped_at_one() {
    let delta = glossy_composite(
        Vec3::ONE,
        Vec3::ZERO,
        1.0,
        Vec4::new(0.0, 0.0, 0.0, 0.5),
        1.0 + f32::EPSILON,
        Vec3::ONE,
    );
    assert_eq!(delta, -Vec3::ONE);
}
