// Tests for the per-hit bounce estimator: light guard, soft visibility, seeds.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::{DVec3, Vec3};

use super::bounce::*;
use super::light_eval::*;
use super::test_fixtures::*;
use super::trace::*;
use super::*;
use crate::bvh_build::build_bvh;
use crate::lightmap_bake::soft_visibility;
use crate::map_data::{FalloffModel, LightType, MapLight};

#[test]
fn sample_radiance_rgb_ignores_trailing_out_of_range_light_bit_identically() {
    let floor_a = [[-4.0, 0.0, -4.0], [4.0, 0.0, -4.0], [4.0, 0.0, 4.0]];
    let floor_b = [[-4.0, 0.0, -4.0], [4.0, 0.0, 4.0], [-4.0, 0.0, 4.0]];
    let geometry = multi_triangle_geometry(&[floor_a, floor_b]);
    let (bvh, primitives, _) = build_bvh(&geometry).expect("floor geometry builds a BVH");
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
    };

    let mut kept_light = point_light_with_falloff(FalloffModel::Linear, 10.0);
    kept_light.origin = DVec3::new(0.0, 5.0, -1.0);
    let mut out_of_range_light = point_light_with_falloff(FalloffModel::InverseSquared, 1.0);
    out_of_range_light.origin = DVec3::new(100.0, 5.0, -1.0);

    let origin = Vec3::new(0.0, 1.0, -1.0);
    let direction = Vec3::NEG_Y;
    let without_trailing_light = sample_radiance_rgb(
        &ctx,
        origin,
        direction,
        &[&kept_light],
        None,
        100.0,
        17,
        23,
        BounceVisibility::OneSample,
    );
    let with_trailing_light = sample_radiance_rgb(
        &ctx,
        origin,
        direction,
        &[&kept_light, &out_of_range_light],
        None,
        100.0,
        17,
        23,
        BounceVisibility::OneSample,
    );

    assert_eq!(
        [
            with_trailing_light.0.x.to_bits(),
            with_trailing_light.0.y.to_bits(),
            with_trailing_light.0.z.to_bits(),
            with_trailing_light.1.to_bits(),
        ],
        [
            without_trailing_light.0.x.to_bits(),
            without_trailing_light.0.y.to_bits(),
            without_trailing_light.0.z.to_bits(),
            without_trailing_light.1.to_bits(),
        ],
        "a trailing point light beyond falloff range must leave the per-ray sample byte-identical"
    );
}

/// The full-term guard must leave every per-ray sample bit-identical to the
/// range-only guard it replaced. The reference below is that prior loop: it
/// traces every in-range light and multiplies by the Lambert term afterward.
/// The mixed light set includes lights the new guard culls (out-of-cone spot,
/// back-facing point, spot and sun) beside lights it keeps, on soft and hard
/// emitters, so each kept light's seed and trace are exercised alongside.
/// The reference traces the full set, so the guarded call does too; the
/// guard precedes the visibility mode and is shared by both.
#[test]
fn sample_radiance_rgb_full_term_guard_matches_range_only_reference_bit_identically() {
    fn range_only_reference(
        ctx: &RaytracingCtx<'_>,
        origin: Vec3,
        dir: Vec3,
        lights: &[&MapLight],
        far_sentinel: f32,
        probe_index: u64,
        ray_index: u64,
    ) -> (Vec3, f32) {
        match closest_hit(ctx, origin + dir * RAY_EPSILON, dir, f32::INFINITY) {
            None => (Vec3::from(SKY_COLOR), far_sentinel),
            Some(hit) => {
                let mut radiance = Vec3::ZERO;
                for (light_index, light) in lights.iter().enumerate() {
                    if !light_reaches_point(light, hit.point) {
                        continue;
                    }
                    let seed = soft_visibility_seed(probe_index, ray_index, light_index as u64);
                    let v = soft_visibility(
                        hit.point,
                        hit.normal,
                        light,
                        seed,
                        crate::lightmap_bake::DEFAULT_AREA_SAMPLE_COUNT,
                        |from, to| segment_clear(ctx, from, to),
                    );
                    if v <= 0.0 {
                        continue;
                    }
                    radiance += light_contribution_lambert(light, hit.point, hit.normal) * v;
                }
                (
                    radiance * BOUNCE_ALBEDO / std::f32::consts::PI,
                    hit.distance,
                )
            }
        }
    }

    // Floor at y = 0 plus a half-plane occluder at y = 2.5 over +X, so the
    // soft overhead light sits in a penumbra and escalates.
    let floor_a = [[-8.0, 0.0, -8.0], [8.0, 0.0, -8.0], [8.0, 0.0, 8.0]];
    let floor_b = [[-8.0, 0.0, -8.0], [8.0, 0.0, 8.0], [-8.0, 0.0, 8.0]];
    let occluder_a = [[0.0, 2.5, -4.0], [4.0, 2.5, -4.0], [4.0, 2.5, 4.0]];
    let occluder_b = [[0.0, 2.5, -4.0], [4.0, 2.5, 4.0], [0.0, 2.5, 4.0]];
    let geometry = multi_triangle_geometry(&[floor_a, floor_b, occluder_a, occluder_b]);
    let (bvh, primitives, _) = build_bvh(&geometry).expect("fixture builds a BVH");
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
    };

    let soft_overhead = soft_point_light(DVec3::new(0.0, 5.0, 0.0), 2.0);
    let hard_overhead = soft_point_light(DVec3::new(-3.0, 4.0, 1.0), 0.0);
    let mut in_cone_spot = soft_point_light(DVec3::new(1.0, 6.0, -1.0), 0.5);
    in_cone_spot.light_type = LightType::Spot;
    in_cone_spot.cone_direction = Some([0.0, -1.0, 0.0]);
    in_cone_spot.cone_angle_inner = Some(0.4);
    in_cone_spot.cone_angle_outer = Some(0.9);
    let mut out_of_cone_spot = in_cone_spot.clone();
    out_of_cone_spot.cone_direction = Some([0.0, 1.0, 0.0]);
    let below_floor_point = soft_point_light(DVec3::new(0.0, -3.0, 0.0), 1.0);
    let mut below_floor_spot = below_floor_point.clone();
    below_floor_spot.light_type = LightType::Spot;
    below_floor_spot.cone_direction = Some([0.0, 1.0, 0.0]);
    below_floor_spot.cone_angle_inner = Some(0.4);
    below_floor_spot.cone_angle_outer = Some(0.9);
    let mut front_sun = soft_point_light(DVec3::ZERO, 0.0);
    front_sun.light_type = LightType::Directional;
    front_sun.cone_direction = Some(Vec3::new(0.3, -1.0, 0.2).normalize().to_array());
    front_sun.angular_diameter = 2.0;
    let mut back_sun = front_sun.clone();
    back_sun.cone_direction = Some([0.0, 1.0, 0.0]);
    // A non-finite origin a `.map` can author: its Lambert term is NaN, not
    // zero, so only the range test skips it — as the range-only guard did.
    let nan_origin_point = soft_point_light(DVec3::new(f64::NAN, 0.0, 0.0), 1.0);

    // Culled lights lead, so the penumbra light behind them would take a
    // different visibility seed if the guard renumbered kept lights.
    let lights = [
        &out_of_cone_spot,
        &nan_origin_point,
        &soft_overhead,
        &hard_overhead,
        &below_floor_point,
        &in_cone_spot,
        &back_sun,
        &below_floor_spot,
        &front_sun,
    ];

    // Every culled light really is culled at the floor hit; every kept light
    // really contributes — otherwise the comparison proves nothing.
    let floor_hit = Vec3::new(0.5, 0.0, 0.5);
    for (light, culled) in [
        (&out_of_cone_spot, true),
        (&below_floor_point, true),
        (&below_floor_spot, true),
        (&back_sun, true),
        (&soft_overhead, false),
        (&hard_overhead, false),
        (&in_cone_spot, false),
        (&front_sun, false),
        (&nan_origin_point, true),
    ] {
        assert_eq!(
            bounce_contribution(light, floor_hit, Vec3::Y).is_none(),
            culled,
            "fixture light {:?} at {:?} must be {}",
            light.light_type,
            light.origin,
            if culled { "culled" } else { "kept" }
        );
    }

    let mut lit_samples = 0;
    let mut seed_sensitive_samples = 0;
    for (ray_index, (x, z)) in [
        (0.5, 0.5),
        (2.0, -1.0),
        (-2.5, 3.0),
        (1.5, 1.5),
        (-4.0, -4.0),
    ]
    .into_iter()
    .enumerate()
    {
        let origin = Vec3::new(x, 1.0, z);
        let dir = Vec3::new(0.05, -1.0, 0.03).normalize();
        let guarded = sample_radiance_rgb(
            &ctx,
            origin,
            dir,
            &lights,
            None,
            100.0,
            7,
            ray_index as u64,
            BounceVisibility::FullSet,
        );
        let reference =
            range_only_reference(&ctx, origin, dir, &lights, 100.0, 7, ray_index as u64);
        assert_eq!(
            [
                guarded.0.x.to_bits(),
                guarded.0.y.to_bits(),
                guarded.0.z.to_bits(),
                guarded.1.to_bits(),
            ],
            [
                reference.0.x.to_bits(),
                reference.0.y.to_bits(),
                reference.0.z.to_bits(),
                reference.1.to_bits(),
            ],
            "ray from ({x}, {z}): full-term guard must match the range-only reference bit-for-bit"
        );
        if guarded.0 != Vec3::ZERO {
            lit_samples += 1;
        }
        // Renumbering kept lights — dropping the culled ones before seeding —
        // must change the bits, or this test could not catch a seed shift.
        let renumbered =
            range_only_reference(&ctx, origin, dir, &lights[2..], 100.0, 7, ray_index as u64);
        if renumbered.0 != reference.0 {
            seed_sensitive_samples += 1;
        }
    }
    assert!(
        lit_samples > 0,
        "the fixture must produce lit samples to compare"
    );
    assert!(
        seed_sensitive_samples > 0,
        "some sample must depend on the kept lights' seeds"
    );
}

/// A spot term just inside the outer cone is tiny but nonzero, so the guard
/// must keep its ray: an epsilon cut (the lightmap's `1e-12` coverage test is
/// the nearest rival) would drop light the range-only guard kept.
#[test]
fn sample_radiance_rgb_keeps_a_tiny_nonzero_cone_fringe_term() {
    let floor_a = [[-8.0, 0.0, -8.0], [8.0, 0.0, -8.0], [8.0, 0.0, 8.0]];
    let floor_b = [[-8.0, 0.0, -8.0], [8.0, 0.0, 8.0], [-8.0, 0.0, 8.0]];
    let geometry = multi_triangle_geometry(&[floor_a, floor_b]);
    let (bvh, primitives, _) = build_bvh(&geometry).expect("floor builds a BVH");
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
    };

    // Spot 5 m above the origin aimed straight down; the ray lands at x = 2,
    // a hair inside the outer cone.
    let mut fringe_spot = soft_point_light(DVec3::new(0.0, 5.0, 0.0), 0.0);
    fringe_spot.light_type = LightType::Spot;
    fringe_spot.cone_direction = Some([0.0, -1.0, 0.0]);
    fringe_spot.cone_angle_inner = Some(0.1);
    fringe_spot.cone_angle_outer = Some((2.0_f32 / 5.0).atan() + 1.0e-5);

    let hit = Vec3::new(2.0, 0.0, 0.0);
    let term = bounce_contribution(&fringe_spot, hit, Vec3::Y)
        .expect("a nonzero fringe term must keep its ray");
    assert!(
        term.length_squared() > 0.0 && term.length_squared() < 1.0e-12,
        "fixture must sit in the near-zero fringe, got {term}"
    );

    let (radiance, _) = sample_radiance_rgb(
        &ctx,
        Vec3::new(2.0, 1.0, 0.0),
        Vec3::NEG_Y,
        &[&fringe_spot],
        None,
        100.0,
        0,
        0,
        BounceVisibility::OneSample,
    );
    assert_ne!(
        radiance,
        Vec3::ZERO,
        "the fringe term must reach the bounce sample"
    );
}

/// Task 4b: the SH bounce term scales by the `[0, 1]` soft-visibility fraction,
/// not a binary 0/1 gate. A receiver floor under a soft (area) light, with a
/// half-plane occluder blocking part of the light's sphere, must bounce
/// strictly between fully-occluded (0) and fully-clear — the partial-penumbra
/// fraction. This exercises the wired `sample_radiance_rgb` call site through
/// real geometry tracing, not just the helper in isolation. A single ray of
/// the one-sample bounce is 0 or 1 by design, so this pins the full set;
/// `bounce_tests` pins the one-sample mean against the lattice.
#[test]
fn sh_bounce_scales_by_soft_visibility_fraction() {
    // Floor receiver around the origin (normal +Y) plus a half-plane occluder
    // at y = 2.5 covering only +X, so area samples toward the +X half of the
    // light sphere are blocked and the -X half stays clear → a true penumbra.
    let floor_a = [[-4.0, 0.0, -4.0], [4.0, 0.0, -4.0], [4.0, 0.0, 4.0]];
    let floor_b = [[-4.0, 0.0, -4.0], [4.0, 0.0, 4.0], [-4.0, 0.0, 4.0]];
    let occluder_a = [[0.0, 2.5, -4.0], [4.0, 2.5, -4.0], [4.0, 2.5, 4.0]];
    let occluder_b = [[0.0, 2.5, -4.0], [4.0, 2.5, 4.0], [0.0, 2.5, 4.0]];
    let geo = multi_triangle_geometry(&[floor_a, floor_b, occluder_a, occluder_b]);
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &prims,
        geometry: &geo,
    };

    // Probe just above the floor, ray straight down → hits the floor at ~origin.
    let probe = Vec3::new(0.0, 1.0, 0.0);
    let dir = Vec3::new(0.0, -1.0, 0.0);
    let far = 100.0;

    // Large area radius so the sphere straddles the +X occluder edge.
    let soft_light = soft_point_light(DVec3::new(0.0, 5.0, 0.0), 2.0);
    let soft = sample_radiance_rgb(
        &ctx,
        probe,
        dir,
        &[&soft_light],
        None,
        far,
        0,
        0,
        BounceVisibility::FullSet,
    )
    .0;

    // Reference: the same light over the floor with NO occluder → fully-clear
    // bounce (v = 1). Tracing against floor-only geometry is the clean way to
    // pin the unoccluded result now that every light always casts a shadow.
    let clear_geo = multi_triangle_geometry(&[floor_a, floor_b]);
    let (clear_bvh, clear_prims, _) = build_bvh(&clear_geo).unwrap();
    let clear_ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &clear_bvh,
        primitives: &clear_prims,
        geometry: &clear_geo,
    };
    let clear = sample_radiance_rgb(
        &clear_ctx,
        probe,
        dir,
        &[&soft_light],
        None,
        far,
        0,
        0,
        BounceVisibility::FullSet,
    )
    .0;

    // Fractional, not binary: strictly dimmer than fully-clear yet nonzero.
    assert!(clear.length() > 0.0, "reference clear bounce should be lit");
    assert!(
        soft.length() > 0.0,
        "partially-visible soft light must still bounce some light, got {soft}"
    );
    assert!(
        soft.length() < clear.length(),
        "occluded soft light should bounce less than fully-clear: soft={} clear={}",
        soft.length(),
        clear.length()
    );
}

/// The seed must vary across the three index axes (probe, ray, light) so
/// distinct shadow queries don't all rotate the same area-sample pattern, while
/// staying a pure function of the indices (reproducible across processes).
#[test]
fn soft_visibility_seed_varies_per_index_axis_and_is_pure() {
    let base = soft_visibility_seed(0, 0, 0);
    assert_ne!(base, soft_visibility_seed(1, 0, 0), "probe axis collapsed");
    assert_ne!(base, soft_visibility_seed(0, 1, 0), "ray axis collapsed");
    assert_ne!(base, soft_visibility_seed(0, 0, 1), "light axis collapsed");
    // Distinct axes must not alias onto each other (probe 1 vs ray 1 etc.).
    assert_ne!(
        soft_visibility_seed(1, 0, 0),
        soft_visibility_seed(0, 1, 0),
        "probe and ray axes alias"
    );
    // Pure function: identical indices → identical seed.
    assert_eq!(
        soft_visibility_seed(7, 13, 2),
        soft_visibility_seed(7, 13, 2)
    );
}
