// Tests for the probe gather's patch-cache radiance source against the per-hit estimator.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::{DVec3, Vec3};

use super::bounce::sample_radiance_rgb;
use super::probe::bake_probe_rgb_with_moments;
use super::test_fixtures::*;
use super::trace::sphere_directions;
use super::*;
use crate::bvh_build::build_bvh;
use crate::surface_radiance::{PatchGather, PatchLookup, PatchRadiance, PatchSet};
use crate::texel_footprint::NO_EMBEDDED_EMITTERS;

// AC 26: id 34's depth moments are the one-sample bake's whatever the
// gather's radiance source.
#[test]
fn patch_gather_leaves_depth_moments_byte_identical_to_the_one_sample_bake() {
    let geometry = floor_and_walls_geometry();
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let tree = tree_all_empty();
    let exterior = HashSet::new();
    let lights = [soft_point_light(DVec3::new(2.0, 2.5, 2.0), 0.3)];
    let static_lights = StaticBakedLights::from_lights(&lights);
    let animated_lights = AnimatedBakedLights::from_lights(&lights);
    let inputs = ShBakeCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: lights.len(),
    };
    let light_refs = static_light_refs(&inputs);
    let patches = PatchSet::build(&geometry);
    let control = BakeControl::unrestricted();
    let radiance = PatchRadiance::single_bounce(
        &inputs.ray_ctx(),
        &patches,
        &light_refs,
        &NO_EMBEDDED_EMITTERS,
        &control,
    );
    let gather = PatchGather::new(&patches, &radiance, PatchLookup::Nearest);
    let far_sentinel = 4.0 * Vec3::ONE.length();
    for (index, probe) in [Vec3::new(1.0, 1.0, 1.5), Vec3::new(3.2, 0.4, 2.7)]
        .into_iter()
        .enumerate()
    {
        let bake = |source| {
            bake_probe_rgb_with_moments(
                &inputs,
                probe,
                &light_refs,
                None,
                far_sentinel,
                index as u64,
                source,
            )
        };
        let (one_sample, one_d, one_d2) = bake(RadianceSource::Lights(BounceVisibility::OneSample));
        let (cached, cached_d, cached_d2) = bake(RadianceSource::Patches(&gather));
        assert_eq!(cached_d.to_bits(), one_d.to_bits());
        assert_eq!(cached_d2.to_bits(), one_d2.to_bits());
        assert!(one_sample[0] > 0.0 && cached[0] > 0.0, "both gather bounce");
    }
}

// AC 2, back faces: a ray that hits a face from behind reads the back side's
// patch, shaded as the reference shades the hit, with the normal facing the
// ray.
#[test]
fn a_back_face_hit_reads_what_the_reference_reads_for_it() {
    // One floor, wound to face +Y: the light and one probe sit below it.
    let geometry = multi_triangle_geometry(&[
        [[-3.0, 0.0, -3.0], [-3.0, 0.0, 3.0], [3.0, 0.0, 3.0]],
        [[-3.0, 0.0, -3.0], [3.0, 0.0, 3.0], [3.0, 0.0, -3.0]],
    ]);
    let (bvh, primitives, _) = build_bvh(&geometry).unwrap();
    let ctx = RaytracingCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &primitives,
        geometry: &geometry,
    };
    let light = point_light_colored(DVec3::new(0.4, -1.5, -0.3), [1.0, 1.0, 1.0], 1.0);
    let patches = PatchSet::build(&geometry);
    let control = BakeControl::unrestricted();
    let radiance =
        PatchRadiance::single_bounce(&ctx, &patches, &[&light], &NO_EMBEDDED_EMITTERS, &control);
    let gather = PatchGather::new(&patches, &radiance, PatchLookup::Nearest);
    let reference = BounceVisibility::FullSet(&NO_EMBEDDED_EMITTERS);
    let directions = sphere_directions(RAYS_PER_PROBE, SAMPLING_LATTICE_OFFSET);
    for (probe, lit) in [
        (Vec3::new(0.3, -0.8, -0.2), true),
        (Vec3::new(0.3, 0.8, -0.2), false),
    ] {
        let (mut want, mut got, mut hits) = (Vec3::ZERO, Vec3::ZERO, 0);
        for (ray, &dir) in directions.iter().enumerate() {
            let (expected, expected_d) = sample_radiance_rgb(
                &ctx,
                probe,
                dir,
                &[&light],
                None,
                1.0,
                0,
                ray as u64,
                reference,
            );
            let (actual, actual_d) = gather.sample(&ctx, probe, dir, 1.0);
            assert_eq!(actual_d.to_bits(), expected_d.to_bits());
            assert_eq!(actual == Vec3::ZERO, expected == Vec3::ZERO, "ray {ray}");
            hits += usize::from(gather_hit(&ctx, probe, dir).is_some());
            want += expected;
            got += actual;
        }
        assert!(hits > 0);
        if lit {
            assert!(want.x > 0.0, "the light reaches the floor's back side");
            let error = (got - want).abs().max_element() / want.max_element();
            assert!(error < 0.02, "patch gather {got} vs reference {want}");
        } else {
            assert_eq!((want, got), (Vec3::ZERO, Vec3::ZERO), "the front is unlit");
        }
    }
}
