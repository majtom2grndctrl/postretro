// Tests for per-probe depth moments.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::collections::HashSet;

use glam::Vec3;

use super::probe::*;
use super::test_fixtures::*;
use super::*;
use crate::bvh_build::build_bvh;
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::map_data::MapLight;

// Regression: guards variance non-negativity (`E[d²] >= E[d]²`) for a valid,
// non-degenerate probe. Compares the pre-rounding f32 moments — two
// independently-rounded f16 values can violate a naive `>=` and flake.
#[test]
fn probe_depth_moments_keep_squared_distance_at_least_mean_squared() {
    // Open-topped box gives this probe a genuine spread of ray distances:
    // some rays hit a near wall, some a far wall, some sky-miss to the
    // sentinel. A non-degenerate spread is what makes the variance check
    // meaningful rather than vacuously satisfied by equal distances.
    let geo = floor_and_walls_geometry();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let tree = tree_all_empty();
    let exterior: HashSet<usize> = HashSet::new();
    let lights: &[MapLight] = &[];
    let static_lights = StaticBakedLights::from_lights(lights);
    let animated_lights = AnimatedBakedLights::from_lights(lights);
    let inputs = ShBakeCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &prims,
        geometry: &geo,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: 0,
    };
    let static_light_refs: Vec<&MapLight> = Vec::new();

    // Probe off-center so the four bounding walls sit at distinct distances.
    let probe_pos = Vec3::new(1.0, 1.0, 1.5);
    // 1 m cell → far sentinel matches the bake's `4 * length(cell_size)`.
    let far_sentinel = 4.0 * Vec3::splat(1.0).length();

    // Pre-rounding f32 moments: divide the raw sums by RAYS_PER_PROBE before
    // any f16 encoding (the f16 store happens later in `bake_sh_volume`).
    let (_coeffs, sum_d, sum_d2) = bake_probe_rgb_with_moments(
        &inputs,
        probe_pos,
        &static_light_refs,
        None,
        far_sentinel,
        0,
        RadianceSource::Lights(BounceVisibility::OneSample),
    );
    let mean_d = sum_d / RAYS_PER_PROBE as f32;
    let mean_sq_d = sum_d2 / RAYS_PER_PROBE as f32;

    // Variance = E[d²] - E[d]² is non-negative by construction; allow a tiny
    // epsilon on the boundary for f32 summation rounding.
    assert!(
        mean_sq_d >= mean_d * mean_d - 1.0e-3,
        "variance must be non-negative: E[d²]={mean_sq_d} < E[d]²={}",
        mean_d * mean_d,
    );
    // Guard against a vacuous pass: a degenerate probe (all rays the same
    // distance) would make variance ~0. This fixture must produce spread.
    assert!(
        mean_sq_d - mean_d * mean_d > 1.0e-2,
        "fixture is degenerate — no distance spread to exercise the moments \
         (E[d²]={mean_sq_d}, E[d]²={})",
        mean_d * mean_d,
    );
}

// Anchors AC#2: the depth moments encode local occlusion, so an open-space
// probe must bake a meaningfully larger mean distance than a cornered one.
#[test]
fn open_probe_mean_distance_exceeds_cornered_probe() {
    let geo = floor_and_walls_geometry();
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let tree = tree_all_empty();
    let exterior: HashSet<usize> = HashSet::new();
    let lights: &[MapLight] = &[];
    let static_lights = StaticBakedLights::from_lights(lights);
    let animated_lights = AnimatedBakedLights::from_lights(lights);
    let inputs = ShBakeCtx {
        bounce_materials: None,
        bvh: &bvh,
        primitives: &prims,
        geometry: &geo,
        tree: &tree,
        exterior_leaves: &exterior,
        static_lights: &static_lights,
        animated_lights: &animated_lights,
        total_light_count: 0,
    };
    let section = bake_sh_volume(&inputs, &ShConfig { probe_spacing: 1.0 });

    // Fixture grid: 5×4×5 probes at 1 m spacing over the [0,4] box, origin
    // at (0,0,0). Walls sit at x=0, x=4, z=4; the z=0 face and the top are
    // open. Flat index is z-major then y then x (see `probe_index_to_xyz`).
    let dims = section.grid_dimensions;
    assert_eq!(dims, [5, 4, 5], "fixture grid changed; revisit probe picks");
    let nx = dims[0] as usize;
    let ny = dims[1] as usize;
    let flat = |x: usize, y: usize, z: usize| z * nx * ny + y * nx + x;

    // Open probe: x-center (x=2) on the open z=0 face, on the floor — its
    // rays escape through the open side and open top, so it sees far.
    let open = &section.probes[flat(2, 0, 0)];
    // Cornered probe: tucked against the x=0 and z=4 walls, one step off the
    // floor — three nearby occluders pull its mean ray distance down.
    let corner = &section.probes[flat(1, 1, 3)];

    assert_eq!(
        open.validity, 1,
        "open probe must be a valid interior probe"
    );
    assert_eq!(
        corner.validity, 1,
        "corner probe must be a valid interior probe"
    );

    let open_mean = f16_bits_to_f32(open.mean_distance);
    let corner_mean = f16_bits_to_f32(corner.mean_distance);

    // Observed gap is ~2.5 m (open ≈6.4 m vs corner ≈3.9 m); require ≥1.5 m
    // so the assertion is non-vacuous yet has ample headroom against f16
    // rounding and any minor sampler tweak.
    assert!(
        open_mean - corner_mean >= 1.5,
        "open-space probe should bake a meaningfully larger mean distance \
         than a cornered probe (open={open_mean}, corner={corner_mean})",
    );
}
