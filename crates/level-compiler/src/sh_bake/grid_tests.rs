// Tests for the SH probe grid: dimensions, iteration order, probe validity.
// See: context/lib/build_pipeline.md §Compiler pipeline

use std::collections::HashSet;

use glam::DVec3;

use super::grid::*;
use super::test_fixtures::*;
use super::*;
use crate::bvh_build::build_bvh;
use crate::light_namespaces::{AnimatedBakedLights, StaticBakedLights};
use crate::map_data::MapLight;
use crate::partition::{Aabb as CompilerAabb, BspLeaf, BspTree};

#[test]
fn grid_dimensions_cover_world_aabb() {
    let min = DVec3::new(0.0, 0.0, 0.0);
    let max = DVec3::new(3.0, 0.0, 2.0);
    let dims = grid_dimensions(min, max, 1.0);
    assert_eq!(dims, [4, 1, 3]);
}

#[test]
fn probe_iteration_matches_z_major_convention() {
    let dims = [2, 3, 4];
    let total = 2 * 3 * 4;
    assert_eq!(probe_index_to_xyz(0, dims), (0, 0, 0));
    assert_eq!(probe_index_to_xyz(total - 1, dims), (1, 2, 3));
    assert_eq!(probe_index_to_xyz(1, dims), (1, 0, 0)); // x advances first
    assert_eq!(probe_index_to_xyz(2, dims), (0, 1, 0)); // then y
    assert_eq!(probe_index_to_xyz(2 * 3, dims), (0, 0, 1)); // then z
}

#[test]
fn solid_probes_are_flagged_invalid() {
    let tree = BspTree {
        nodes: Vec::new(),
        leaves: vec![BspLeaf {
            face_indices: Vec::new(),
            bounds: CompilerAabb {
                min: DVec3::splat(-1000.0),
                max: DVec3::splat(1000.0),
            },
            is_solid: true,
            defining_planes: Vec::new(),
        }],
    };
    let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
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
    for probe in &section.probes {
        assert_eq!(probe.validity, 0);
    }
}

#[test]
fn exterior_leaf_probes_are_flagged_invalid() {
    // Regression: sub-plan 10 Fix D — exercises probe_is_valid directly for
    // solid, interior-empty, and exterior-empty leaves (no BSP nodes, so leaf 0 catches all).
    let solid_leaf = BspLeaf {
        face_indices: Vec::new(),
        bounds: CompilerAabb {
            min: DVec3::splat(-1.0),
            max: DVec3::splat(1.0),
        },
        is_solid: true,
        defining_planes: Vec::new(),
    };
    let interior_leaf = BspLeaf {
        face_indices: Vec::new(),
        bounds: CompilerAabb {
            min: DVec3::splat(-1.0),
            max: DVec3::splat(1.0),
        },
        is_solid: false,
        defining_planes: Vec::new(),
    };
    let exterior_leaf = BspLeaf {
        face_indices: Vec::new(),
        bounds: CompilerAabb {
            min: DVec3::splat(-1.0),
            max: DVec3::splat(1.0),
        },
        is_solid: false,
        defining_planes: Vec::new(),
    };

    let tree_interior = BspTree {
        nodes: Vec::new(),
        leaves: vec![interior_leaf.clone()],
    };
    let mut exterior_set_empty: HashSet<usize> = HashSet::new();
    assert!(probe_is_valid(
        &tree_interior,
        &exterior_set_empty,
        DVec3::ZERO
    ));

    let tree_solid = BspTree {
        nodes: Vec::new(),
        leaves: vec![solid_leaf],
    };
    assert!(!probe_is_valid(
        &tree_solid,
        &exterior_set_empty,
        DVec3::ZERO
    ));

    let tree_exterior = BspTree {
        nodes: Vec::new(),
        leaves: vec![exterior_leaf],
    };
    exterior_set_empty.insert(0);
    assert!(!probe_is_valid(
        &tree_exterior,
        &exterior_set_empty,
        DVec3::ZERO
    ));
}

#[test]
fn bake_sh_volume_marks_exterior_leaf_probes_invalid() {
    // Integration: verifies ShBakeCtx correctly wires exterior_leaves through the full bake.
    let geo = one_triangle_geometry([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]);
    let (bvh, prims, _) = build_bvh(&geo).unwrap();
    let tree = tree_all_empty();
    let mut exterior: HashSet<usize> = HashSet::new();
    exterior.insert(0);
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
    assert!(
        !section.probes.is_empty(),
        "expected at least one probe in the bake output",
    );
    for probe in &section.probes {
        assert_eq!(
            probe.validity, 0,
            "probes in an exterior-flagged leaf must be invalid after bake",
        );
        assert_eq!(
            probe.mean_distance, 0,
            "invalid probes must carry zeroed depth moments (AC#3)"
        );
        assert_eq!(
            probe.mean_sq_distance, 0,
            "invalid probes must carry zeroed depth moments (AC#3)"
        );
    }
}
