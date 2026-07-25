// Shared `#[cfg(test)]` fixture-loading helper for compile-pipeline regression tests.
//
// The compiler is a BINARY crate with no `[lib]` target, so cross-module
// integration tests cannot live in `tests/` and `use` the in-crate modules —
// they must be co-located. This module runs the real compile pipeline
// (parse → partition → visibility → geometry → BVH) on a `content/dev/maps/`
// fixture and hands the products (geometry, BVH, primitives, BSP tree, faces,
// exterior leaves, lights) to compile-pipeline fixture tests.
//
// It deliberately stops before the data-script compile step: parsing a `.map`
// only stores `data_script` as a string, so tests need neither the
// `scripts-build` sidecar nor any script compilation to obtain geometry/lights.
//
// See: context/lib/build_pipeline.md §Build Cache

#![cfg(test)]

use std::collections::HashSet;
use std::path::PathBuf;

use bvh::bvh::Bvh;

use crate::bvh_build::{BvhPrimitive, build_bvh};
use crate::geometry::{GeometryResult, extract_geometry};
use crate::map_data::{Face, MapLight};
use crate::map_format::MapFormat;
use crate::partition::BspTree;
use crate::{parse, partition, portals, visibility};

/// The fixtures the SH/lightmap determinism gates loop over. Names (no extension) under
/// `content/dev/maps/`. gate-heavily-lit is the compact, purpose-built heavily-lit
/// stress fixture (a long narrow corridor whose >24 m length makes the warm-vs-cold
/// SH approximation non-vacuous under the 16 m reach cutoff — see gate 3); it replaces
/// campaign-test (194k probes, ~10 min/SH-bake) at a fraction of the cost.
/// soft_shadow_test and the animated-weight-map maps cover the remaining cases.
///
/// Keep this list cheap. Bake cost tracks probe and texel counts, not `.map` size:
/// the heavily-lit entries dominate while the animated-weight-map fixtures are
/// sub-second each. occlusion-test was dropped as the single most expensive entry
/// (~65% of the gates' runtime on its own). The coverage consciously given up is
/// geometric complexity — gate-heavily-lit stresses light count over small
/// geometry, so a determinism break that needs large, occluder-dense brushwork to
/// show up is no longer gated here. Reach for a compact purpose-built fixture
/// before re-adding a large map.
pub const GATE_FIXTURES: &[&str] = &[
    "gate-heavily-lit",
    "soft_shadow_test",
    "test_animated_weight_maps_cap",
    "test_animated_weight_maps_mixed",
    "test_animated_weight_maps_occluded",
    "test_animated_weight_maps_single",
];

/// The products compile-pipeline fixture tests need. Owns the shared pipeline
/// outputs, including partition faces for regression coverage.
pub struct FixturePipeline {
    pub geometry: GeometryResult,
    pub bvh: Bvh<f32, 3>,
    pub primitives: Vec<BvhPrimitive>,
    pub tree: BspTree,
    pub faces: Vec<Face>,
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
/// shared test products. Panics with a descriptive message on any pipeline
/// failure; fixture tests are meaningless if the fixture cannot be loaded.
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
        faces: result.faces,
        exterior_leaves,
        lights: map_data.lights,
    }
}
