//! One-sample SH bounce: stratification, parity with the full set, determinism.
//! See: context/lib/build_pipeline.md §PRL Compilation (Soft-shadow bake cost)

use glam::{DVec3, Vec3};

use super::bounce::*;
use super::light_eval::*;
use super::test_fixtures::{multi_triangle_geometry, soft_point_light, tree_all_empty};
use super::trace::*;
use super::*;
use crate::bvh_build::build_bvh;
use crate::lightmap_bake::{soft_visibility, soft_visibility_one_sample};
use crate::map_data::LightType;

/// Floor at y = 0 with a half-plane occluder at y = 2.5 over +X: a soft
/// light overhead throws a wide penumbra across the floor near x = 0.
fn penumbra_geometry() -> GeometryResult {
    let floor_a = [[-8.0, 0.0, -8.0], [8.0, 0.0, -8.0], [8.0, 0.0, 8.0]];
    let floor_b = [[-8.0, 0.0, -8.0], [8.0, 0.0, 8.0], [-8.0, 0.0, 8.0]];
    let occluder_a = [[0.0, 2.5, -4.0], [4.0, 2.5, -4.0], [4.0, 2.5, 4.0]];
    let occluder_b = [[0.0, 2.5, -4.0], [4.0, 2.5, 4.0], [0.0, 2.5, 4.0]];
    multi_triangle_geometry(&[floor_a, floor_b, occluder_a, occluder_b])
}

fn bake_volume_both_ways(
    geo: &GeometryResult,
    lights: &[MapLight],
) -> (OctahedralShVolumeSection, OctahedralShVolumeSection) {
    let (bvh, prims, _) = build_bvh(geo).unwrap();
    let tree = tree_all_empty();
    let exterior: HashSet<usize> = HashSet::new();
    let static_lights = StaticBakedLights::from_lights(lights);
    let animated_lights = AnimatedBakedLights::from_lights(lights);
    let inputs = ShBakeCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &prims,
        geometry: geo,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: lights.len(),
    };
    let config = ShConfig { probe_spacing: 1.0 };
    let control = BakeControl::unrestricted();
    (
        bake_sh_volume_with_visibility(&inputs, &config, &control, BounceVisibility::OneSample),
        bake_sh_volume_with_visibility(&inputs, &config, &control, BounceVisibility::FullSet),
    )
}

/// A hard emitter traces its one hard ray under either mode, so a volume
/// lit only by hard lights (point, spot, zero-diameter sun) bakes the same
/// bytes one-sample and full-set; the same fixture under a soft light does
/// not, so the comparison can tell the modes apart.
#[test]
fn one_sample_bounce_is_byte_identical_to_full_set_for_hard_lights_only() {
    let geo = penumbra_geometry();
    let hard_point = soft_point_light(DVec3::new(-1.0, 5.0, 0.5), 0.0);
    let mut hard_spot = soft_point_light(DVec3::new(1.0, 6.0, -1.0), 0.0);
    hard_spot.light_type = LightType::Spot;
    hard_spot.cone_direction = Some([0.0, -1.0, 0.0]);
    hard_spot.cone_angle_inner = Some(0.4);
    hard_spot.cone_angle_outer = Some(0.9);
    let mut hard_sun = soft_point_light(DVec3::ZERO, 0.0);
    hard_sun.light_type = LightType::Directional;
    hard_sun.cone_direction = Some(Vec3::new(0.3, -1.0, 0.2).normalize().to_array());
    hard_sun.angular_diameter = 0.0;

    let (one, full) = bake_volume_both_ways(&geo, &[hard_point, hard_spot, hard_sun]);
    assert!(
        one.compact_atlas.iter().any(|&b| b != 0),
        "the hard-light fixture must bounce light"
    );
    assert_eq!(
        one.to_bytes(),
        full.to_bytes(),
        "hard lights must bake byte-identically one-sample and full-set"
    );

    let soft = soft_point_light(DVec3::new(0.0, 5.0, 0.0), 2.0);
    let (one, full) = bake_volume_both_ways(&geo, &[soft]);
    assert_ne!(
        one.to_bytes(),
        full.to_bytes(),
        "a soft light in a penumbra must resolve differently per hit"
    );
}

/// The one-sample helper draws from the very lattice `soft_visibility`'s
/// escalated set traces: at a penumbra hit, the mean over every sample
/// index is exactly that set's fraction. A hard light returns the same
/// value at every index, equal to `soft_visibility`.
#[test]
fn one_sample_lattice_mean_equals_escalated_full_set_fraction() {
    let geo = penumbra_geometry();
    let (bvh, primitives, _) = build_bvh(&geo).unwrap();
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geo,
    };
    let trace = |from, to| segment_clear(&ctx, from, to);
    let soft = soft_point_light(DVec3::new(0.0, 5.0, 0.0), 2.0);
    let hard = soft_point_light(DVec3::new(0.0, 5.0, 0.0), 0.0);
    let n = DEFAULT_AREA_SAMPLE_COUNT;

    let mut penumbra_hits = 0;
    for (k, x) in [-0.5f32, 0.0, 0.25, 0.5, 1.0].into_iter().enumerate() {
        let point = Vec3::new(x, 0.0, 0.3);
        let seed = soft_visibility_seed(3, k as u64, 0);
        let full = soft_visibility(point, Vec3::Y, &soft, seed, n, trace);
        let clear: f32 = (0..n)
            .map(|i| soft_visibility_one_sample(point, Vec3::Y, &soft, seed, |_| i, n, trace))
            .sum();
        if full > 0.0 && full < 1.0 {
            penumbra_hits += 1;
            assert_eq!(
                (clear / n as f32).to_bits(),
                full.to_bits(),
                "x = {x}: the lattice mean must equal the escalated fraction"
            );
        }

        let hard_full = soft_visibility(point, Vec3::Y, &hard, seed, n, trace);
        for i in 0..n {
            let one = soft_visibility_one_sample(point, Vec3::Y, &hard, seed, |_| i, n, trace);
            assert_eq!(one.to_bits(), hard_full.to_bits(), "x = {x}, index {i}");
        }
    }
    assert!(penumbra_hits >= 3, "only {penumbra_hits} penumbra hits");
}

/// Every (probe, light) pass over the 256 gather ray indices assigns each
/// lattice sample exactly 256 / 32 of them, and every polar band of 32
/// consecutive indices assigns each once; the starting sample varies with
/// probe and light.
#[test]
fn bounce_sample_index_strides_every_ray_band_across_the_whole_lattice() {
    let n = DEFAULT_AREA_SAMPLE_COUNT as usize;
    let mut first_samples = std::collections::BTreeSet::new();
    for probe in [0u64, 1, 2, 977, 123_456] {
        for light in [0u64, 1, 5] {
            let mut counts = vec![0u32; n];
            for band in 0..(RAYS_PER_PROBE as usize / n) {
                let mut band_counts = vec![0u32; n];
                for r in band * n..(band + 1) * n {
                    let i = bounce_sample_index(probe, r as u64, light, n as u32) as usize;
                    band_counts[i] += 1;
                    counts[i] += 1;
                }
                assert!(
                    band_counts.iter().all(|&c| c == 1),
                    "probe {probe} light {light} band {band}: {band_counts:?}"
                );
            }
            assert!(
                counts
                    .iter()
                    .all(|&c| c == RAYS_PER_PROBE / DEFAULT_AREA_SAMPLE_COUNT),
                "probe {probe} light {light}: {counts:?}"
            );
            first_samples.insert(bounce_sample_index(probe, 0, light, n as u32));
        }
    }
    assert!(
        first_samples.len() > 8,
        "probe and light must offset the pass: {first_samples:?}"
    );
}

/// Stratification is exact over the pass offset: for a fixed probe and light,
/// each gather ray's one-sample visibility summed over all `lattice` offsets
/// (`lattice` consecutive ray indices through `bounce_sample_index`, under
/// that ray's own seed) is that ray's full-lattice clear count, as an integer.
/// So the probe's one-sample bounce averaged over offsets is the lattice-mean
/// bounce with no bias at all.
#[test]
fn one_sample_bounce_summed_over_every_pass_offset_is_the_lattice_sum_exactly() {
    let geo = penumbra_geometry();
    let (bvh, primitives, _) = build_bvh(&geo).unwrap();
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geo,
    };
    let trace = |from, to| segment_clear(&ctx, from, to);
    let soft = soft_point_light(DVec3::new(0.0, 5.0, 0.0), 2.0);
    let directions = sphere_directions(RAYS_PER_PROBE, SAMPLING_LATTICE_OFFSET);
    let n = DEFAULT_AREA_SAMPLE_COUNT;

    let mut penumbra_rays = 0;
    for (probe_index, light_axis, origin) in [
        (11u64, 0u64, Vec3::new(0.25, 1.0, 0.0)),
        (12, 3, Vec3::new(-0.5, 2.0, 0.5)),
    ] {
        for (ray_index, dir) in directions.iter().enumerate() {
            let ray = ray_index as u64;
            let Some(hit) = closest_hit(&ctx, origin + *dir * RAY_EPSILON, *dir, f32::INFINITY)
            else {
                continue;
            };
            if bounce_contribution(&soft, hit.point, hit.normal).is_none() {
                continue;
            }
            let seed = soft_visibility_seed(probe_index, ray, light_axis);
            let clear = |i: u32| {
                soft_visibility_one_sample(hit.point, hit.normal, &soft, seed, |_| i, n, trace)
                    as u32
            };
            let lattice_clear: u32 = (0..n).map(clear).sum();
            let offsets_clear: u32 = (0..u64::from(n))
                .map(|k| clear(bounce_sample_index(probe_index, ray + k, light_axis, n)))
                .sum();
            assert_eq!(
                offsets_clear, lattice_clear,
                "probe {probe_index} ray {ray_index}"
            );
            if 0 < lattice_clear && lattice_clear < n {
                penumbra_rays += 1;
            }
        }
    }
    assert!(penumbra_rays > 20, "only {penumbra_rays} penumbra rays");
}

/// The wired bounce, end to end: summed over many probes' gather rays, the
/// one-sample radiance matches the lattice-mean radiance within four standard
/// deviations of the per-hit Bernoulli noise, `sqrt(sum c^2 f (1 - f))` for
/// bounce contribution `c` and lattice clear fraction `f` at each hit. That
/// bound treats hits as independent; the stratified passes only lower the
/// variance. The same bound must hold against `soft_visibility`'s full set,
/// which equals the lattice mean wherever it escalates. At this hit count the
/// bound resolves only a gross bias; the exact-sum test above pins
/// unbiasedness.
#[test]
fn one_sample_bounce_mean_converges_to_full_set_fraction() {
    let geo = penumbra_geometry();
    let (bvh, primitives, _) = build_bvh(&geo).unwrap();
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geo,
    };
    let trace = |from, to| segment_clear(&ctx, from, to);
    let soft = soft_point_light(DVec3::new(0.0, 5.0, 0.0), 2.0);
    let directions = sphere_directions(RAYS_PER_PROBE, SAMPLING_LATTICE_OFFSET);
    let scale = BOUNCE_ALBEDO / std::f32::consts::PI;
    let n = DEFAULT_AREA_SAMPLE_COUNT;

    let (mut one_sum, mut full_sum) = (0.0f64, 0.0f64);
    let (mut lattice_sum, mut variance) = (0.0f64, 0.0f64);
    let (mut penumbra_hits, mut differing_hits) = (0usize, 0usize);
    let mut probe_index = 0u64;
    for x in -3..=3 {
        for z in -3..=3 {
            let origin = Vec3::new(x as f32 * 0.5, 1.0, z as f32 * 0.5);
            for (ray_index, dir) in directions.iter().enumerate() {
                let ray_index = ray_index as u64;
                let sample = |visibility| {
                    let lights = [&soft];
                    sample_radiance_rgb(
                        &ctx,
                        origin,
                        *dir,
                        &lights,
                        None,
                        100.0,
                        probe_index,
                        ray_index,
                        visibility,
                    )
                    .0
                    .x
                };
                let one = sample(BounceVisibility::OneSample);
                let full = sample(BounceVisibility::FullSet);
                let Some(hit) = closest_hit(&ctx, origin + *dir * RAY_EPSILON, *dir, f32::INFINITY)
                else {
                    continue;
                };
                let Some(contribution) = bounce_contribution(&soft, hit.point, hit.normal) else {
                    continue;
                };
                let seed = soft_visibility_seed(probe_index, ray_index, 0);
                let clear: f32 = (0..n)
                    .map(|i| {
                        soft_visibility_one_sample(
                            hit.point,
                            hit.normal,
                            &soft,
                            seed,
                            |_| i,
                            n,
                            trace,
                        )
                    })
                    .sum();
                let f = f64::from(clear / n as f32);
                let c = f64::from(contribution.x * scale);
                if f > 0.0 && f < 1.0 {
                    penumbra_hits += 1;
                }
                if one != full {
                    differing_hits += 1;
                }
                one_sum += f64::from(one);
                full_sum += f64::from(full);
                lattice_sum += c * f;
                variance += c * c * f * (1.0 - f);
            }
            probe_index += 1;
        }
    }
    let tolerance = 4.0 * variance.sqrt();
    eprintln!(
        "one-sample {one_sum:.4} full set {full_sum:.4} lattice mean {lattice_sum:.4} \
         4-sigma {tolerance:.4}; {penumbra_hits} penumbra hits, {differing_hits} differ"
    );
    assert!(penumbra_hits > 500, "only {penumbra_hits} penumbra hits");
    assert!(
        differing_hits > 0,
        "one-sample must differ from the full set per hit"
    );
    assert!(
        (one_sum - lattice_sum).abs() <= tolerance,
        "one-sample {one_sum} vs lattice mean {lattice_sum}: tolerance {tolerance}"
    );
    assert!(
        (one_sum - full_sum).abs() <= tolerance,
        "one-sample {one_sum} vs full set {full_sum}: tolerance {tolerance}"
    );
    // The bound's resolution: a bias below this fraction of the signal passes.
    assert!(
        tolerance < 0.05 * lattice_sum,
        "tolerance {tolerance} vs lattice mean {lattice_sum}"
    );
}

/// The cold whole-volume bake fans probes over rayon; the one-sample
/// estimator derives every sample from probe, ray and light indices alone,
/// so one worker and many bake the same bytes.
#[test]
fn one_sample_sh_volume_is_byte_identical_across_worker_counts() {
    let geo = penumbra_geometry();
    let lights = [
        soft_point_light(DVec3::new(0.0, 5.0, 0.0), 2.0),
        soft_point_light(DVec3::new(-3.0, 4.0, 2.0), 0.5),
    ];
    let bake = |workers: usize| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .unwrap()
            .install(|| bake_volume_both_ways(&geo, &lights).0.to_bytes())
    };
    let single = bake(1);
    assert_eq!(single, bake(4), "1 vs 4 workers");
    assert_eq!(single, bake(7), "1 vs 7 workers");
}

/// Warm SH bakes a bounded light subset under each kept light's GLOBAL
/// index, so a kept light traces the same one sample it does cold, and a
/// dropped light only removes a non-negative term. Per ray, and per probe
/// in the L0 band (a positive basis, so the probe's mean irradiance per
/// channel), warm is never brighter than cold.
#[test]
fn warm_light_subset_never_brightens_a_one_sample_bounce() {
    let geo = penumbra_geometry();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let tree = tree_all_empty();
    let exterior: HashSet<usize> = HashSet::new();
    let mut lights = vec![
        soft_point_light(DVec3::new(0.0, 5.0, 0.0), 2.0),
        soft_point_light(DVec3::new(-3.0, 4.0, 2.0), 0.5),
        soft_point_light(DVec3::new(3.0, 1.5, -2.0), 1.0),
    ];
    lights[1].color = [1.0, 0.25, 0.5];
    lights[2].color = [0.25, 0.5, 1.0];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let animated_lights = AnimatedBakedLights::from_lights(&lights);
    let inputs = ShBakeCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &prims,
        geometry: &geo,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: lights.len(),
    };
    let ctx = inputs.ray_ctx();
    let cold_refs = static_light_refs(&inputs);
    let warm_refs = [cold_refs[0], cold_refs[2]];
    let warm_globals = [0u64, 2];
    let directions = sphere_directions(RAYS_PER_PROBE, SAMPLING_LATTICE_OFFSET);

    let mut strictly_dimmer = 0;
    for (k, origin) in [
        Vec3::new(-1.0, 1.0, 0.5),
        Vec3::new(0.5, 1.5, -0.5),
        Vec3::new(2.0, 0.5, 1.0),
        Vec3::new(-3.0, 2.0, -2.0),
    ]
    .into_iter()
    .enumerate()
    {
        let probe_index = k as u64 * 31 + 5;
        for (ray_index, dir) in directions.iter().enumerate() {
            let sample = |refs: &[&MapLight], globals: Option<&[u64]>| {
                sample_radiance_rgb(
                    &ctx,
                    origin,
                    *dir,
                    refs,
                    globals,
                    100.0,
                    probe_index,
                    ray_index as u64,
                    BounceVisibility::OneSample,
                )
                .0
            };
            let cold = sample(&cold_refs, None);
            let warm = sample(&warm_refs, Some(&warm_globals));
            assert!(
                warm.cmple(cold).all(),
                "probe {probe_index} ray {ray_index}: warm {warm} brighter than cold {cold}"
            );
        }
        let cold = bake_probe(&inputs, origin, &cold_refs, None, 100.0, true, probe_index);
        let warm = bake_probe(
            &inputs,
            origin,
            &warm_refs,
            Some(&warm_globals),
            100.0,
            true,
            probe_index,
        );
        // Band 0 holds the first three coefficients, one per channel.
        for channel in 0..3 {
            let (w, c) = (warm.coefficients[channel], cold.coefficients[channel]);
            assert!(
                w <= c,
                "probe {probe_index} channel {channel}: warm L0 {w} > cold L0 {c}"
            );
            if w < c {
                strictly_dimmer += 1;
            }
        }
    }
    assert!(
        strictly_dimmer > 0,
        "the dropped light must reach some probe"
    );
}
