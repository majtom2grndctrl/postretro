// Glossy floor layer blur reference tests: the chain level a hit reads
// follows the probe term's blur and the hit length (the level
// function's half).
// See: context/lib/testing_guide.md

use super::*;
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_MIN_BLUR_MIP, REFLECTION_PROBE_TILE_DIMENSION, reflection_lobe_half_width,
    reflection_probe_full_colour_mip_count, reflection_probe_mip_exponent,
    reflection_probe_term_half_width,
};
use postretro_level_format::specular_smoothness::{SPECULAR_EXPONENT_MAX, SPECULAR_EXPONENT_MIN};

use crate::glossy_reflection::GLOSSY_SHARPNESS_FLOOR_EXPONENT;

/// A 1080-row scene at a 90° vertical field of view: 540 pixels per unit of
/// tangent at depth 1, its chain at half the scene, eleven levels.
const FOCAL: f32 = 540.0;
const CHAIN_SCALE: f32 = 0.5;
const LEVELS: u32 = 11;

/// The engine section's term blur: 128² tiles, the full six-mip chain.
fn engine() -> GlossyFloorTermBlur {
    GlossyFloorTermBlur::of_section(
        REFLECTION_PROBE_TILE_DIMENSION,
        reflection_probe_full_colour_mip_count(REFLECTION_PROBE_TILE_DIMENSION),
    )
}

fn level(exponent: f32, hit_length: f32, hit_depth: f32) -> f32 {
    glossy_floor_blur_level(
        exponent,
        engine(),
        hit_length,
        hit_depth,
        FOCAL,
        CHAIN_SCALE,
        LEVELS,
    )
}

/// Exponents from the minimum to the cap, a few per octave.
fn exponents() -> impl Iterator<Item = f32> {
    (0..=52).map(|step| SPECULAR_EXPONENT_MIN * 2f32.powf(step as f32 / 4.0))
}

#[test]
fn the_floor_blur_reads_the_probe_terms_chain_and_its_minimum_blur() {
    // Every texel that reflects reads the probe term's own half-width; the
    // cap only keeps the chain's last mip finite.
    assert!(
        reflection_lobe_half_width(GLOSSY_SHARPNESS_FLOOR_EXPONENT)
            < GLOSSY_FLOOR_WIDEST_HALF_ANGLE
    );
    // At every section shape: the engine's, a 64² and a 256² capture with
    // their full chains, and a one-mip section, whose probe term reads mip 0.
    for (tile, mips) in [
        (REFLECTION_PROBE_TILE_DIMENSION, 6),
        (64, reflection_probe_full_colour_mip_count(64)),
        (256, reflection_probe_full_colour_mip_count(256)),
        (REFLECTION_PROBE_TILE_DIMENSION, 1),
    ] {
        let term = GlossyFloorTermBlur::of_section(tile, mips);
        for exponent in exponents().filter(|&n| n >= GLOSSY_SHARPNESS_FLOOR_EXPONENT) {
            assert_eq!(
                glossy_floor_blur_half_angle(exponent, term),
                reflection_probe_term_half_width(exponent, term.mip0_texel_angle, mips),
                "{tile}² × {mips}: n = {exponent}"
            );
        }
    }
    // The 256² capture's half-width at n = 64 is the texel's lobe, as at
    // 128²; a one-mip section's is mip 0's texel, the probe term's own.
    let lobe = reflection_lobe_half_width(64.0);
    let at_256 = glossy_floor_blur_half_angle(64.0, GlossyFloorTermBlur::of_section(256, 7));
    assert!((at_256 - lobe).abs() <= 1e-4 * lobe);
    let one = GlossyFloorTermBlur::of_section(REFLECTION_PROBE_TILE_DIMENSION, 1);
    assert_eq!(
        glossy_floor_blur_half_angle(64.0, one),
        one.mip0_texel_angle
    );
    let widest = glossy_floor_blur_half_angle(SPECULAR_EXPONENT_MIN, engine());
    assert_eq!(widest, GLOSSY_FLOOR_WIDEST_HALF_ANGLE);
    assert!(widest.tan().is_finite() && widest.tan() > 0.0);
}

// Level function: blur grows monotonically as the
// exponent falls, at every hit length; the exponent cap and the minimum-blur
// exponent read the same level at each length.
#[test]
fn the_floor_blur_grows_as_the_exponent_falls_and_the_cap_reads_the_minimum_blur() {
    // The minimum-blur exponent: the sharpest lobe the prefilter maps to the
    // minimum-blur mip, nudged past its f32 rounding so it reads that mip.
    let angle = engine().mip0_texel_angle;
    let min_blur_exponent =
        reflection_probe_mip_exponent(REFLECTION_PROBE_MIN_BLUR_MIP as u32, angle) * 1.0001;
    let natural = (reflection_lobe_half_width(min_blur_exponent) / angle).log2();
    assert!(
        natural <= REFLECTION_PROBE_MIN_BLUR_MIP
            && natural > REFLECTION_PROBE_MIN_BLUR_MIP - 1.0e-3,
        "{natural}"
    );
    for hit_length in [0.25, 1.0, 4.0, 16.0, 64.0] {
        let mut previous = f32::INFINITY;
        for exponent in exponents() {
            let at = level(exponent, hit_length, 8.0);
            assert!(
                at <= previous,
                "length {hit_length}: n = {exponent} reads {at}, sharper than {previous}"
            );
            previous = at;
        }
        assert_eq!(
            level(SPECULAR_EXPONENT_MAX, hit_length, 8.0),
            level(min_blur_exponent, hit_length, 8.0),
            "length {hit_length}: the cap reads the minimum blur"
        );
        assert_eq!(
            level(SPECULAR_EXPONENT_MAX, hit_length, 8.0),
            level(min_blur_exponent * 4.0, hit_length, 8.0),
        );
    }
    // Strictly between the minimum blur and the floor, inside the chain.
    assert!(level(16.0, 4.0, 8.0) > level(64.0, 4.0, 8.0));
}

// Level function: at one exponent the footprint grows
// with hit length as the probe term's angular blur does, its width in
// proportion to the length at one hit depth, and shrinks with the hit's
// depth as its projection does. Clamped to the chain at both ends.
#[test]
fn the_floor_blur_widens_with_hit_length_as_a_cone() {
    for exponent in [GLOSSY_SHARPNESS_FLOOR_EXPONENT, 32.0, 256.0] {
        let near = level(exponent, 2.0, 8.0);
        let far = level(exponent, 8.0, 8.0);
        assert!(near > 0.0 && far < (LEVELS - 1) as f32, "n = {exponent}");
        assert!(
            (far - near - 2.0).abs() < 1.0e-5,
            "four times the length, two levels"
        );
        let deeper = level(exponent, 8.0, 16.0);
        assert!(
            (far - deeper - 1.0).abs() < 1.0e-5,
            "twice the depth, one level"
        );
        // The footprint itself: a level's texel spans its full width.
        let half_width = glossy_floor_blur_half_angle(exponent, engine()).tan() * 8.0 * FOCAL / 8.0;
        assert!(((2.0 * half_width * CHAIN_SCALE).log2() - far).abs() < 1.0e-5);
    }
    assert_eq!(
        level(64.0, 0.0, 8.0),
        0.0,
        "no length reads the sharpest level"
    );
    assert_eq!(level(64.0, 1.0e-4, 8.0), 0.0);
    assert_eq!(
        level(SPECULAR_EXPONENT_MIN, 1.0e3, 0.5),
        (LEVELS - 1) as f32
    );
    assert_eq!(
        level(64.0, 1.0, 0.0),
        level(64.0, 1.0, GLOSSY_FLOOR_NEAREST_DEPTH)
    );
}
