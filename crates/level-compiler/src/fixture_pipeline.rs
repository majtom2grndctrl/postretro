// Shared `#[cfg(test)]` fixture-loading helper for compile-pipeline regression tests.
//
// The compiler is a BINARY crate with no `[lib]` target, so cross-module
// integration tests cannot live in `tests/` and `use` the in-crate modules —
// they must be co-located. This module runs the real compile pipeline
// (parse → occluder partition → runtime cells → geometry → BVH) on a
// `content/dev/maps/` fixture and hands the products to compile-pipeline
// fixture tests, so the determinism gates exercise the partition that ships.
//
// It deliberately stops before the data-script compile step: parsing a `.map`
// only stores `data_script` as a string, so tests need neither the
// `scripts-build` sidecar nor any script compilation to obtain geometry/lights.
//
// See: context/lib/build_pipeline.md §Build Cache

#![cfg(test)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use bvh::bvh::Bvh;

use crate::bvh_build::{BvhPrimitive, build_bvh};
use crate::geometry::{GeometryResult, extract_geometry};
use crate::map_data::{Face, MapLight};
use crate::map_format::MapFormat;
use crate::parse;
use crate::partition::BspTree;
use crate::pipeline::runtime_cells::{OccluderPartition, RuntimeCells, ShippedCells};
use crate::portals::Portal;

/// The fixtures the SH/lightmap determinism gates loop over. Names (no extension) under
/// `content/dev/maps/`. gate-heavily-lit is the compact, purpose-built heavily-lit
/// stress fixture (a long narrow corridor whose >24 m length makes the warm-vs-cold
/// SH approximation non-vacuous under the 16 m reach cutoff — see gate 3); it replaces
/// campaign-test (194k probes, ~10 min/SH-bake) at a fraction of the cost.
/// soft_shadow_test and the animated-weight-map maps cover the remaining cases.
///
/// Keep this list cheap. Bake cost tracks probe and texel counts, not `.map` size:
/// the heavily-lit entries dominate while the animated-weight-map fixtures are
/// sub-second each. occlusion-test was dropped as the single most expensive entry:
/// measured before/after, dropping it cut the gates 7× (1812s → 260s). A
/// full-compile timing is a poor proxy for a gate's share — the gates bake every
/// fixture twice and weight large texel counts far more heavily — so measure before/after
/// rather than trust a compile-time estimate. The coverage consciously given up is
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
/// outputs, including partition faces for regression coverage. Like
/// `pipeline.rs`, it carries two trees: every bake that reads cell ids takes
/// the cell view, `tree`; readers of exact sub-leaf solidity (buried lights,
/// reflection-probe placement) take `full_tree`.
pub struct FixturePipeline {
    pub geometry: GeometryResult,
    pub bvh: Bvh<f32, 3>,
    pub primitives: Vec<BvhPrimitive>,
    /// The cell view: the full tree's nodes, its leaves runtime cells.
    pub tree: BspTree,
    /// Exterior cells of `tree`.
    pub exterior_leaves: HashSet<usize>,
    /// Runtime portals between cells of `tree`.
    pub portals: Vec<Portal>,
    /// The full two-tier tree, whose leaves are BSP leaves.
    pub full_tree: BspTree,
    /// The full tree's exterior flood, in its leaf ids.
    pub full_exterior_leaves: HashSet<usize>,
    pub faces: Vec<Face>,
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

/// Run the pipeline on a fixture under `content/dev/maps/`. Panics with a
/// descriptive message on any pipeline failure; fixture tests are
/// meaningless if the fixture cannot be loaded.
pub fn load_fixture(name: &str) -> FixturePipeline {
    let path = fixture_path(name);
    assert!(path.exists(), "fixture map missing: {}", path.display());
    load_map(&path)
}

/// Run parse → occluder partition → runtime cells → geometry → BVH on a
/// `.map`, as `pipeline.rs` runs those stages.
pub fn load_map(path: &Path) -> FixturePipeline {
    let name = path.display();
    let map_data = parse::parse_map_file(path, MapFormat::IdTech2)
        .unwrap_or_else(|e| panic!("parse {name}: {e}"));

    let ShippedCells {
        partition: OccluderPartition { result, .. },
        full_exterior,
        runtime,
    } = ShippedCells::build(&map_data.brush_volumes, &map_data.kinematic_movers)
        .unwrap_or_else(|e| panic!("partition {name}: {e}"));
    let RuntimeCells { cells, portals } = runtime;

    let geometry = extract_geometry(&result.faces, &cells.tree, &cells.exterior_cells);
    let (bvh, primitives, _section) =
        build_bvh(&geometry).unwrap_or_else(|e| panic!("BVH build {name}: {e}"));

    FixturePipeline {
        geometry,
        bvh,
        primitives,
        tree: cells.tree,
        exterior_leaves: cells.exterior_cells,
        portals,
        full_tree: result.tree,
        full_exterior_leaves: full_exterior,
        faces: result.faces,
        lights: map_data.lights,
    }
}
