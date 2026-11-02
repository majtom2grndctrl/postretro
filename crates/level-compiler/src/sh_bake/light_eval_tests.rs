// Tests for per-light radiance evaluation: falloff, spot cone, bounce guard.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::{DVec3, Vec3};

use super::light_eval::*;
use super::test_fixtures::*;
use super::*;
use crate::map_data::{FalloffModel, LightType, MapLight};

// Pins the contract that `sh_bake::light_eval::falloff` mirrors `falloff` in `forward.wgsl`.
// Drift here produces "ghost glow" (indirect picked up where direct is culled).

#[test]
fn linear_falloff_matches_shader_curve() {
    let light = point_light_with_falloff(FalloffModel::Linear, 10.0);
    assert!((falloff(&light, 0.0) - 1.0).abs() < 1e-6);
    assert!((falloff(&light, 5.0) - 0.5).abs() < 1e-6);
    assert!(falloff(&light, 10.0).abs() < 1e-6);
    assert_eq!(falloff(&light, 15.0), 0.0);
}

#[test]
fn inverse_distance_zeroes_past_range() {
    let light = point_light_with_falloff(FalloffModel::InverseDistance, 10.0);
    assert!((falloff(&light, 1.0) - 1.0).abs() < 1e-6);
    assert!((falloff(&light, 5.0) - 0.2).abs() < 1e-6);
    assert_eq!(falloff(&light, 10.001), 0.0);
    assert_eq!(falloff(&light, 50.0), 0.0);
}

#[test]
fn inverse_squared_zeroes_past_range() {
    let light = point_light_with_falloff(FalloffModel::InverseSquared, 10.0);
    assert!((falloff(&light, 1.0) - 1.0).abs() < 1e-6);
    assert!((falloff(&light, 0.5) - 4.0).abs() < 1e-6); // close-range exceeds 1.0 deliberately
    assert_eq!(falloff(&light, 10.001), 0.0);
    assert_eq!(falloff(&light, 100.0), 0.0);
}

#[test]
fn light_reaches_point_keeps_range_boundary_and_skips_only_zero_falloff() {
    let range = 10.0;
    let at_range = Vec3::new(range, 0.0, 0.0);
    let beyond_range = Vec3::new(range + 0.001, 0.0, 0.0);

    for light_type in [LightType::Point, LightType::Spot] {
        for falloff_model in [
            FalloffModel::Linear,
            FalloffModel::InverseDistance,
            FalloffModel::InverseSquared,
        ] {
            let mut light = point_light_with_falloff(falloff_model, range);
            light.light_type = light_type;

            assert!(
                light_reaches_point(&light, at_range),
                "{light_type:?} light with {falloff_model:?} falloff must reach exactly at range"
            );
            assert!(
                !light_reaches_point(&light, beyond_range),
                "{light_type:?} light with {falloff_model:?} falloff must skip strictly beyond range"
            );
            assert_eq!(
                falloff(&light, beyond_range.length()),
                0.0,
                "skipped {light_type:?} light with {falloff_model:?} falloff must contribute zero"
            );
        }
    }

    let mut directional = point_light_with_falloff(FalloffModel::Linear, range);
    directional.light_type = LightType::Directional;
    directional.origin = DVec3::new(10_000.0, 0.0, 0.0);
    assert!(
        light_reaches_point(&directional, Vec3::new(-10_000.0, 0.0, 0.0)),
        "directional lights must never be skipped by range"
    );
}

// The `bounce_guard_*` tests pin both sides of each factor the bounce guard
// folds — range, cone, and facing — through the production predicate, so a
// guard that stops culling, or culls a light that still contributes, fails.
fn bounce_skips(light: &MapLight, point: Vec3, normal: Vec3) -> bool {
    bounce_contribution(light, point, normal).is_none()
}

#[test]
fn bounce_guard_range_skips_beyond_and_at_range_only_for_linear() {
    let range = 10.0;
    let at_range = Vec3::new(range, 0.0, 0.0);
    let beyond_range = Vec3::new(range + 0.001, 0.0, 0.0);
    let facing_light = Vec3::NEG_X;

    for light_type in [LightType::Point, LightType::Spot] {
        for falloff_model in [
            FalloffModel::Linear,
            FalloffModel::InverseDistance,
            FalloffModel::InverseSquared,
        ] {
            let mut light = point_light_with_falloff(falloff_model, range);
            light.light_type = light_type;
            // Aim spots at the receivers so only range varies.
            light.cone_direction = Some([1.0, 0.0, 0.0]);
            light.cone_angle_inner = Some(0.5);
            light.cone_angle_outer = Some(0.8);

            assert!(
                bounce_skips(&light, beyond_range, facing_light),
                "{light_type:?} light with {falloff_model:?} falloff must skip strictly beyond range"
            );
            // Inverse models are nonzero exactly at range and must keep the
            // ray; Linear is exactly zero there, so skipping it is lossless.
            let expect_skip = falloff_model == FalloffModel::Linear;
            assert_eq!(
                bounce_skips(&light, at_range, facing_light),
                expect_skip,
                "{light_type:?} light with {falloff_model:?} falloff at exactly range"
            );
        }
    }
}

#[test]
fn bounce_guard_cone_skips_iff_cone_attenuation_is_zero() {
    // Spot at the origin aimed down +X; receivers on a 5 m arc sweep the
    // outer cone edge in both directions, each facing the light.
    let mut spot = point_light_with_falloff(FalloffModel::InverseSquared, 50.0);
    spot.light_type = LightType::Spot;
    spot.cone_direction = Some([1.0, 0.0, 0.0]);
    spot.cone_angle_inner = Some(0.3);
    spot.cone_angle_outer = Some(0.6);

    let mut skipped = 0;
    let mut kept = 0;
    for step in 0..=120 {
        let angle = 0.5 + step as f32 * (0.2 / 120.0);
        let point = Vec3::new(angle.cos(), angle.sin(), 0.0) * 5.0;
        let normal = -point.normalize();
        // The cone factor exactly as `incident_radiance_at_point` derives it.
        let dist = point.length();
        let cone = spot_cone_attenuation(&spot, -(-point / dist));
        assert_eq!(
            bounce_skips(&spot, point, normal),
            cone == 0.0,
            "spot at {angle} rad off axis: skip must match exactly-zero cone attenuation ({cone})"
        );
        if cone == 0.0 {
            skipped += 1;
        } else {
            kept += 1;
        }
    }
    assert!(
        skipped > 0 && kept > 0,
        "the sweep must straddle the outer cone edge"
    );
}

#[test]
fn bounce_guard_facing_skips_perpendicular_and_back_for_every_light_type() {
    let point = Vec3::ZERO;
    let mut lights = Vec::new();
    let mut light = point_light_with_falloff(FalloffModel::InverseSquared, 50.0);
    light.origin = DVec3::new(0.0, 5.0, 0.0);
    lights.push(light.clone());
    let mut spot = light.clone();
    spot.light_type = LightType::Spot;
    spot.cone_direction = Some([0.0, -1.0, 0.0]);
    spot.cone_angle_inner = Some(0.3);
    spot.cone_angle_outer = Some(0.6);
    lights.push(spot);
    let mut sun = light;
    sun.light_type = LightType::Directional;
    sun.cone_direction = Some([0.0, -1.0, 0.0]);
    lights.push(sun);

    for light in &lights {
        let kind = light.light_type;
        assert!(
            !bounce_skips(light, point, Vec3::Y),
            "{kind:?}: a surface facing the light must keep its ray"
        );
        assert!(
            !bounce_skips(light, point, Vec3::new(0.01, 1.0, 0.0).normalize()),
            "{kind:?}: a slightly front-facing surface must keep its ray"
        );
        assert!(
            bounce_skips(light, point, Vec3::X),
            "{kind:?}: a surface exactly perpendicular to the light must skip"
        );
        assert!(
            bounce_skips(light, point, Vec3::NEG_Y),
            "{kind:?}: a surface facing away from the light must skip"
        );
    }

    // A front-facing sun is never culled, however far away the receiver.
    let sun = &lights[2];
    assert!(!bounce_skips(
        sun,
        Vec3::new(-10_000.0, 0.0, 10_000.0),
        Vec3::Y
    ));
}

#[test]
fn smoothstep_matches_wgsl_definition() {
    assert_eq!(smoothstep(0.0, 1.0, -0.5), 0.0);
    assert_eq!(smoothstep(0.0, 1.0, 1.5), 1.0);
    assert!((smoothstep(0.0, 1.0, 0.5) - 0.5).abs() < 1e-6);
    let lower = smoothstep(0.0, 1.0, 0.25);
    let upper = smoothstep(0.0, 1.0, 0.75);
    assert!((lower + upper - 1.0).abs() < 1e-6); // symmetric
}

/// Cross-check (NOT the comparison target for the convolved irradiance): the
/// normal-free `radiance` from `incident_radiance_at_point` must match the
/// lightmap baker's `light_contribution_and_direction` with the cosine divided
/// back out — pinning that the two bakers agree on falloff/intensity/cone
/// magnitude for all three light types. The lightmap value bundles `n_dot_l`;
/// recovering `radiance = contribution / n_dot_l` is the only valid bridge.
#[test]
fn incident_radiance_matches_lightmap_baker_magnitude_for_all_light_types() {
    use crate::lightmap_bake::light_contribution_and_direction;

    let probe = Vec3::new(0.0, 0.0, 0.0);
    let point = point_light_colored(glam::DVec3::new(0.9, 3.0, 0.6), [1.4, 0.6, 2.3], 1.1);

    let mut spot = point_light_colored(glam::DVec3::new(0.0, 4.0, 0.0), [0.8, 1.1, 0.5], 1.2);
    spot.light_type = LightType::Spot;
    spot.cone_direction = Some([0.0, -1.0, 0.0]);
    spot.cone_angle_inner = Some(0.6);
    spot.cone_angle_outer = Some(0.9);

    let mut directional = point_light_colored(glam::DVec3::ZERO, [1.0, 0.9, 1.3], 0.7);
    directional.light_type = LightType::Directional;
    directional.cone_direction = Some([0.2, -1.0, 0.1]);

    for (light, name) in [
        (&point, "Point"),
        (&spot, "Spot"),
        (&directional, "Directional"),
    ] {
        let (radiance, l) = incident_radiance_at_point(light, probe).unwrap();
        // Use a normal tilted off `l` so n_dot_l is a clean, non-degenerate
        // divisor (not 1.0, not near 0.0).
        let n = normals_on_cone(l, 30.0)[0];
        let n_dot_l = n.dot(l).max(0.0);
        let (contribution, lm_l) = light_contribution_and_direction(light, probe, n);
        // Both bakers must agree on the incident direction.
        assert!(
            (lm_l - l).length() < 1.0e-5,
            "{name}: incident direction mismatch: {lm_l:?} vs {l:?}",
        );
        let recovered = contribution / n_dot_l;
        for c in 0..3 {
            let rel = (recovered[c] - radiance[c]).abs() / radiance[c].max(1.0e-3);
            assert!(
                rel < 1.0e-4,
                "{name}: recovered radiance channel {c} = {} disagrees with \
                 incident_radiance_at_point {} (rel {rel:.5})",
                recovered[c],
                radiance[c],
            );
        }
    }
}
