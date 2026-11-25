// Exponent → mip function tests.
// See: context/lib/testing_guide.md

use super::*;
use crate::reflection_probes::reflection_probe_colour_mips_valid;
use crate::specular_smoothness::{SPECULAR_EXPONENT_MAX, SPECULAR_EXPONENT_MIN};

const ENGINE_MIPS: u32 = 6;

#[test]
fn the_engine_tile_carries_six_colour_mips_to_one_block() {
    assert_eq!(
        reflection_probe_full_colour_mip_count(REFLECTION_PROBE_TILE_DIMENSION),
        ENGINE_MIPS
    );
    assert!(reflection_probe_colour_mips_valid(
        REFLECTION_PROBE_TILE_DIMENSION,
        ENGINE_MIPS
    ));
    assert!(!reflection_probe_colour_mips_valid(
        REFLECTION_PROBE_TILE_DIMENSION,
        ENGINE_MIPS + 1
    ));
    assert_eq!(reflection_probe_full_colour_mip_count(4), 1);
}

#[test]
fn the_mip0_texel_angle_is_180_degrees_over_64() {
    assert!((REFLECTION_PROBE_MIP0_TEXEL_ANGLE.to_degrees() - 2.8125).abs() < 1e-5);
}

#[test]
fn half_width_matches_the_reference_table() {
    // The lobe's half-width at half maximum: n = 256 → 8.4°, 64 → 16.8°, 6 → 54°.
    for (n, degrees) in [(256.0, 8.43), (64.0, 16.85), (6.0, 54.0)] {
        let theta = reflection_lobe_half_width(n).to_degrees();
        assert!((theta - degrees).abs() < 0.1, "n = {n}: {theta}°");
    }
}

#[test]
fn exponent_mip_matches_the_delegated_answer() {
    // n = 64 reads ≈ 2.6 and n = 256 ≈ 1.6, both their own lobes; n ≥ ~576
    // reads the minimum blur.
    let at_64 = reflection_probe_exponent_mip(64.0, ENGINE_MIPS);
    assert!((at_64 - 2.58).abs() < 0.01, "{at_64}");
    let at_256 = reflection_probe_exponent_mip(256.0, ENGINE_MIPS);
    assert!((at_256 - 1.58).abs() < 0.01, "{at_256}");
    let at_574 = reflection_probe_exponent_mip(574.0, ENGINE_MIPS);
    assert!(at_574 > REFLECTION_PROBE_MIN_BLUR_MIP, "{at_574}");
    for n in [576.0, 1024.0, 4096.0, SPECULAR_EXPONENT_MAX] {
        assert_eq!(
            reflection_probe_exponent_mip(n, ENGINE_MIPS),
            REFLECTION_PROBE_MIN_BLUR_MIP,
            "n = {n}"
        );
    }
}

#[test]
fn blur_grows_monotonically_as_the_exponent_falls() {
    // Steps of 1/256 in log₂ n from the cap down to 1.
    let mut previous = reflection_probe_exponent_mip(SPECULAR_EXPONENT_MAX, ENGINE_MIPS);
    let mut log_n = SPECULAR_EXPONENT_MAX.log2();
    while log_n >= 0.0 {
        let mip = reflection_probe_exponent_mip(log_n.exp2(), ENGINE_MIPS);
        assert!(mip >= previous, "falls at log₂ n = {log_n}");
        assert!(mip.is_finite());
        previous = mip;
        log_n -= 1.0 / 256.0;
    }
}

#[test]
fn selection_never_reads_sharper_than_the_minimum_blur_or_past_the_last_mip() {
    let last = (ENGINE_MIPS - 1) as f32;
    let mut log_n = 0.0f32;
    while log_n <= SPECULAR_EXPONENT_MAX.log2() {
        let mip = reflection_probe_exponent_mip(log_n.exp2(), ENGINE_MIPS);
        assert!(
            (REFLECTION_PROBE_MIN_BLUR_MIP..=last).contains(&mip),
            "log₂ n = {log_n}: {mip}"
        );
        log_n += 1.0 / 64.0;
    }
    // At the cap and at the minimum exponent.
    assert_eq!(
        reflection_probe_exponent_mip(SPECULAR_EXPONENT_MAX, ENGINE_MIPS),
        REFLECTION_PROBE_MIN_BLUR_MIP
    );
    assert_eq!(
        reflection_probe_exponent_mip(SPECULAR_EXPONENT_MIN, ENGINE_MIPS),
        last
    );
    // A chain shorter than the minimum blur: the last mip wins.
    for count in 1..=3u32 {
        for n in [1.0, 16.0, SPECULAR_EXPONENT_MAX] {
            assert!(
                reflection_probe_exponent_mip(n, count) <= (count - 1) as f32,
                "{count} mip(s), n = {n}"
            );
        }
    }
    assert_eq!(reflection_probe_exponent_mip(64.0, 1), 0.0);
}

#[test]
fn each_mips_prefilter_exponent_maps_back_to_that_mip() {
    for mip in 0..ENGINE_MIPS {
        let n = reflection_probe_mip_exponent(mip);
        assert!(n.is_finite() && n > 0.0, "mip {mip}: n = {n}");
        let theta = reflection_lobe_half_width(n);
        let expected = reflection_probe_mip_half_width(mip);
        assert!(
            (theta - expected).abs() <= 1e-3 * expected,
            "mip {mip}: θ {theta} vs {expected}"
        );
        if (REFLECTION_PROBE_MIN_BLUR_MIP as u32..ENGINE_MIPS).contains(&mip) {
            let back = reflection_probe_exponent_mip(n, ENGINE_MIPS);
            assert!((back - mip as f32).abs() < 1e-3, "mip {mip}: {back}");
        }
    }
    // The last mip's lobe is the n = 2 lobe (θ = 90°).
    assert!((reflection_probe_mip_exponent(ENGINE_MIPS - 1) - 2.0).abs() < 1e-3);
}

// The probe term's spread per exponent, the reference
// the floor layer's blur is held to: the texel's own lobe between the
// minimum blur and the last mip, the minimum blur's lobe at the cap.
#[test]
fn the_probe_terms_spread_is_the_texels_lobe_within_the_chain() {
    for n in [16.0f32, 40.0, 64.0, 100.0, 256.0, 512.0] {
        let spread = reflection_probe_term_half_width(n, ENGINE_MIPS);
        let lobe = reflection_lobe_half_width(n);
        assert!(
            (spread - lobe).abs() <= 1e-4 * lobe,
            "n = {n}: {spread} vs {lobe}"
        );
    }
    let min_blur = reflection_probe_mip_half_width(REFLECTION_PROBE_MIN_BLUR_MIP as u32);
    for n in [1024.0, SPECULAR_EXPONENT_MAX] {
        assert!((reflection_probe_term_half_width(n, ENGINE_MIPS) - min_blur).abs() < 1e-6);
    }
    // Monotone: blur never narrows as the exponent falls.
    let mut previous = 0.0;
    for n in (1..=8192).rev().step_by(7) {
        let spread = reflection_probe_term_half_width(n as f32, ENGINE_MIPS);
        assert!(spread >= previous, "n = {n}");
        previous = spread;
    }
}
