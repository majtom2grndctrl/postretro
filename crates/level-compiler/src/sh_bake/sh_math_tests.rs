// Tests for SH projection and the D3 direct-at-probe assembly.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::Vec3;

use super::sh_math::*;
use super::test_fixtures::*;
use super::*;
use crate::lightmap_bake::soft_visibility;
use crate::map_data::{LightType, MapLight};

fn apply_cosine_lobe_mono(acc: &mut [f32; 9]) {
    for (band, v) in acc.iter_mut().enumerate() {
        *v *= cosine_lobe_factor(band);
    }
}

fn accumulate_sh(acc: &mut [f32; 9], dir: Vec3, value: f32, weight: f32) {
    let basis = sh_basis_l2(dir);
    for (a, b) in acc.iter_mut().zip(basis.iter()) {
        *a += *b * value * weight;
    }
}

#[test]
fn cosine_lobe_makes_constant_radiance_reconstruct_to_pi() {
    // For constant L=1, diffuse irradiance = π. Verify the cosine-lobe convolution
    // produces this for any normal direction.
    let samples = 8192usize;
    let weight = 4.0 * std::f32::consts::PI / samples as f32;
    let mut coeffs = [0f32; 9];
    let phi = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt()); // golden angle
    for i in 0..samples {
        let t = (i as f32 + 0.5) / samples as f32;
        let z = 1.0 - 2.0 * t;
        let r = (1.0 - z * z).max(0.0).sqrt();
        let theta = phi * i as f32;
        let dir = Vec3::new(r * theta.cos(), r * theta.sin(), z);
        accumulate_sh(&mut coeffs, dir, 1.0, weight);
    }
    apply_cosine_lobe_mono(&mut coeffs);

    let reconstruct = |n: Vec3| -> f32 {
        let b = sh_basis_l2(n);
        (0..9).map(|i| coeffs[i] * b[i]).sum()
    };

    let expected = std::f32::consts::PI;
    for n in [
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        -Vec3::X,
        -Vec3::Y,
        -Vec3::Z,
        Vec3::new(0.577, 0.577, 0.577).normalize(),
    ] {
        let got = reconstruct(n);
        assert!(
            (got - expected).abs() < 0.05,
            "constant radiance L=1 should reconstruct to π irradiance; \
             got {got} for normal {n:?}, expected {expected}",
        );
    }
}

// D3 GATING HARNESS — empirically validates the direct-at-probe SH projection
// convention (plan "Baked Static Direct SH for Entities + Billboards", Task 0).
// This is a PERMANENT regression test, not a build-to-learn spike: it pins the
// exact assembly downstream tasks bake with. Source of truth — if the assembly
// drifts (lobe direction, normalization, cosine-lobe factors), this fails.
//
// D3 assembly under test, for one probe under one static light:
//   1. (radiance, l) = incident_radiance_at_point(light, probe)  // l = probe→light
//   2. vis           = soft_visibility(probe, l, light, ...)      // shadow factor
//   3. accumulate_sh_rgb(acc, l, radiance * vis, 1.0)             // delta lobe along l
//   4. apply_cosine_lobe_rgb(acc)                                  // irradiance convolve
// Reconstructed at a receiver normal `n`, the pinned equality is
//   evaluate_sh_rgb(acc, n) == radiance · max(dot(n, l), 0)
// where `radiance` is the normal-free falloff·intensity·color·visibility term.

// Pinned per-channel relative-error gate on the light-aligned cone, observed
// worst-case 4.03% (colored light, normals at 20°/30°/40°/45° × 4 azimuths).
// A-priori ceiling is 5%; pinned tighter at 4.5% with headroom over observed.
// NOTE: the cone for this tight bound EXCLUDES the exact peak n=l — see
// `d3_direct_at_probe_peak_overshoots_by_known_l2_truncation`.
const D3_LIGHT_ALIGNED_REL_TOLERANCE: f32 = 0.045;

/// Bake one probe's direct-at-probe SH under one static light using the exact
/// D3 assembly, with an unoccluded trace (probe in open air). Returns the L2
/// SH RGB coefficients the runtime would sample.
fn bake_direct_at_probe_d3(light: &MapLight, probe: Vec3) -> [f32; 27] {
    let (radiance, l) =
        incident_radiance_at_point(light, probe).expect("light must reach the probe");
    // Open air: nothing occludes, so the trace is always clear and visibility
    // collapses to 1.0. soft_visibility uses `l` as the offset normal (a probe
    // has no surface normal; the incident direction stands in).
    let vis = soft_visibility(probe, l, light, 0, 0, |_from, _to| true);
    let mut acc = [0f32; 27];
    accumulate_sh_rgb(&mut acc, l, radiance * vis, 1.0);
    apply_cosine_lobe_rgb(&mut acc);
    acc
}

/// Asserts the D3 pinned equality at light-aligned normals across the ±45° cone
/// (AC 8): per-channel rel error `abs(diff)/orig.max(1e-3)` ≤ pinned threshold.
/// `name` identifies the LightType arm in failure output.
fn assert_d3_light_aligned(light: &MapLight, probe: Vec3, name: &str) {
    let coeffs = bake_direct_at_probe_d3(light, probe);
    let (radiance, l) = incident_radiance_at_point(light, probe).unwrap();

    // ≥3 distinct light-aligned normals spanning the cone — peak excluded
    // (its known L2 overshoot is pinned separately).
    let mut tested = 0usize;
    for cone_deg in [20.0f32, 30.0, 40.0, 45.0] {
        for n in normals_on_cone(l, cone_deg) {
            tested += 1;
            let reconstructed = evaluate_sh_rgb(&coeffs, n);
            let expected = radiance * n.dot(l).max(0.0);
            for c in 0..3 {
                let rel = (reconstructed[c] - expected[c]).abs() / expected[c].max(1.0e-3);
                assert!(
                    rel <= D3_LIGHT_ALIGNED_REL_TOLERANCE,
                    "{name}: D3 rel error {rel:.4} exceeds {D3_LIGHT_ALIGNED_REL_TOLERANCE} \
                     at cone {cone_deg}°, channel {c}, normal {n:?}; \
                     got {reconstructed:?}, expected {expected:?}",
                );
            }
        }
    }
    assert!(
        tested >= 3,
        "AC 8 requires ≥3 light-aligned normals, tested {tested}"
    );
}

#[test]
fn d3_direct_at_probe_reconstructs_lambert_for_point_light() {
    // Colored, scaled light so a per-channel or constant-factor normalization
    // error can't be tuned away — the bound must hold on all three channels.
    let probe = Vec3::new(0.0, 0.0, 0.0);
    let light = point_light_colored(glam::DVec3::new(0.9, 3.0, 0.6), [1.4, 0.6, 2.3], 1.0);
    assert_d3_light_aligned(&light, probe, "Point");
}

#[test]
fn d3_direct_at_probe_reconstructs_lambert_for_spot_light() {
    // Probe sits well inside the inner cone so spot attenuation is ~1.0 and the
    // reconstruction is a clean Lambert lobe (cone attenuation is folded into
    // `radiance`, validated separately by the lightmap cross-check test).
    let probe = Vec3::new(0.0, 0.0, 0.0);
    let mut light = point_light_colored(glam::DVec3::new(0.0, 4.0, 0.0), [0.8, 1.1, 0.5], 1.2);
    light.light_type = LightType::Spot;
    light.cone_direction = Some([0.0, -1.0, 0.0]); // aimed straight down at the probe
    light.cone_angle_inner = Some(0.6);
    light.cone_angle_outer = Some(0.9);
    assert_d3_light_aligned(&light, probe, "Spot");
}

#[test]
fn d3_direct_at_probe_reconstructs_lambert_for_directional_light() {
    let probe = Vec3::new(2.0, 2.0, -1.0); // position is irrelevant for directional
    let mut light = point_light_colored(glam::DVec3::ZERO, [1.0, 0.9, 1.3], 0.7);
    light.light_type = LightType::Directional;
    light.cone_direction = Some([0.2, -1.0, 0.1]); // photon travel; probe→light = -this
    assert_d3_light_aligned(&light, probe, "Directional");
}

/// D3 CORRECTION FINDING: the pinned equality holds across the ±45° cone EXCEPT
/// at the exact peak n=l, where the L2-SH cosine-lobe reconstruction overshoots
/// the true clamped cosine by a fixed ~6.25%. This is intrinsic L2 truncation
/// (the lobe axis is the WORST-conditioned point in the cone, not the best — the
/// spec's "well-conditioned cone" must exclude the inner peak), NOT an assembly
/// error: the lobe direction (probe→light) and cosine-lobe normalization match
/// the indirect bake exactly. Pinned here so a regression in the cosine-lobe
/// factors (which would shift this constant) is caught.
#[test]
fn d3_direct_at_probe_peak_overshoots_by_known_l2_truncation() {
    let probe = Vec3::ZERO;
    let light = point_light_colored(glam::DVec3::new(0.9, 3.0, 0.6), [1.4, 0.6, 2.3], 1.0);
    let coeffs = bake_direct_at_probe_d3(&light, probe);
    let (radiance, l) = incident_radiance_at_point(&light, probe).unwrap();
    let reconstructed = evaluate_sh_rgb(&coeffs, l); // n = l, the lobe axis
    // At n=l the true Lambert value is `radiance` (dot=1). The L2 overshoot is
    // a constant factor, so the per-channel ratio is identical on every channel.
    for c in 0..3 {
        let ratio = reconstructed[c] / radiance[c];
        assert!(
            (ratio - 1.0625).abs() < 0.01,
            "peak overshoot ratio drifted: channel {c} got {ratio:.4}, expected ~1.0625",
        );
    }
}

/// Off-axis / terminator normals (beyond the cone, toward grazing) — assert
/// ONLY sign and monotonicity, NOT a tight relative bound: near the terminator
/// the true Lambert value → 0 and L2 ringing makes relative error unbounded.
/// Monotonicity: reconstructed irradiance decreases as the normal tilts away
/// from the light toward grazing, and never goes negative (evaluate clamps).
#[test]
fn d3_direct_at_probe_monotone_toward_terminator() {
    let probe = Vec3::ZERO;
    let light = point_light_colored(glam::DVec3::new(0.0, 3.0, 0.0), [1.0, 1.0, 1.0], 1.0);
    let coeffs = bake_direct_at_probe_d3(&light, probe);
    let (_radiance, l) = incident_radiance_at_point(&light, probe).unwrap();

    let mut prev = f32::INFINITY;
    for cone_deg in [50.0f32, 65.0, 80.0, 90.0] {
        // Single azimuth is enough for a monotonicity sweep.
        let n = normals_on_cone(l, cone_deg)[0];
        let value = evaluate_sh_rgb(&coeffs, n).x;
        assert!(
            value >= 0.0,
            "irradiance went negative at {cone_deg}°: {value}"
        );
        assert!(
            value <= prev + 1.0e-4,
            "irradiance not monotone decreasing at {cone_deg}°: {value} > prev {prev}",
        );
        prev = value;
    }
    assert!(prev >= 0.0);
}

#[test]
fn sh_basis_band0_is_constant() {
    let b0 = sh_basis_l2(Vec3::X)[0];
    let b1 = sh_basis_l2(Vec3::Y)[0];
    let b2 = sh_basis_l2(Vec3::new(0.3, 0.4, 0.5).normalize())[0];
    assert!((b0 - b1).abs() < 1.0e-6);
    assert!((b0 - b2).abs() < 1.0e-6);
}
