// Shared `#[cfg(test)]` fixture-loading helper for the Task 8 determinism gates.
//
// The compiler is a BINARY crate with no `[lib]` target, so cross-module
// integration tests cannot live in `tests/` and `use` the in-crate modules —
// they must be co-located. This module runs the real compile pipeline
// (parse → partition → visibility → geometry → BVH) on a `content/dev/maps/`
// fixture and hands the products (geometry, BVH, primitives, BSP tree, exterior
// leaves, lights) to the gate tests in `lightmap_layer.rs` and `sh_group.rs`.
//
// It deliberately stops before the data-script compile step: parsing a `.map`
// only stores `data_script` as a string, so the gates need neither the
// `scripts-build` sidecar nor any script compilation to obtain geometry/lights.
//
// See: context/plans/in-progress/incremental-bake-per-element/index.md (Task 8)

#![cfg(test)]

use std::collections::HashSet;
use std::path::PathBuf;

use bvh::bvh::Bvh;

use crate::bvh_build::{BvhPrimitive, build_bvh};
use crate::geometry::{GeometryResult, extract_geometry};
use crate::map_data::MapLight;
use crate::map_format::MapFormat;
use crate::partition::BspTree;
use crate::{parse, partition, portals, visibility};

/// The fixtures Task 8's gates loop over. Names (no extension) under
/// `content/dev/maps/`. campaign-test + occlusion-test are the heavily-lit
/// pair; soft_shadow_test and the animated-weight-map maps round out coverage.
pub const GATE_FIXTURES: &[&str] = &[
    "campaign-test",
    "occlusion-test",
    "soft_shadow_test",
    "test_animated_weight_maps_cap",
    "test_animated_weight_maps_mixed",
    "test_animated_weight_maps_occluded",
    "test_animated_weight_maps_single",
];

/// The products of the compile pipeline a determinism gate needs. Owns the
/// geometry/BVH/lights so a test can borrow them for both the monolithic and the
/// per-element path.
pub struct FixturePipeline {
    pub geometry: GeometryResult,
    pub bvh: Bvh<f32, 3>,
    pub primitives: Vec<BvhPrimitive>,
    pub tree: BspTree,
    pub exterior_leaves: HashSet<usize>,
    pub lights: Vec<MapLight>,
}

/// Absolute path to a fixture `.map` under `content/dev/maps/`.
pub fn fixture_path(name: &str) -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/level-compiler/. Workspace root is ../../.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root")
        .join("content/dev/maps")
        .join(format!("{name}.map"))
}

/// Run parse → partition → visibility → geometry → BVH on a fixture, returning
/// the products the gates compare. Panics with a descriptive message on any
/// pipeline failure (a gate is meaningless if the fixture cannot be loaded).
pub fn load_fixture(name: &str) -> FixturePipeline {
    let path = fixture_path(name);
    assert!(path.exists(), "fixture map missing: {}", path.display());

    let map_data = parse::parse_map_file(&path, MapFormat::IdTech2)
        .unwrap_or_else(|e| panic!("parse {name}: {e}"));

    let result = partition::partition(&map_data.brush_volumes)
        .unwrap_or_else(|e| panic!("partition {name}: {e}"));

    let generated_portals = portals::generate_portals(&result.tree);
    let exterior_leaves = visibility::find_exterior_leaves(&result.tree, &generated_portals);

    let geometry = extract_geometry(&result.faces, &result.tree, &exterior_leaves);
    let (bvh, primitives, _section) =
        build_bvh(&geometry).unwrap_or_else(|e| panic!("BVH build {name}: {e}"));

    FixturePipeline {
        geometry,
        bvh,
        primitives,
        tree: result.tree,
        exterior_leaves,
        lights: map_data.lights,
    }
}
