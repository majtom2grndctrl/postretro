// Indexed point-to-region lookup against the linear scans it replaces: equal
// answers everywhere, and on a 4,216-region level only nearby regions examined.
// See: context/lib/build_pipeline.md §Navigation bake

use glam::Vec3;
use postretro_level_format::navmesh::{NAVMESH_VERSION, NavMeshSection, NavRegion};

use super::NavGraph;
use super::arena_fixture::multi_arena_map;
use super::region_index::REGIONS_EXAMINED;

/// Deterministic LCG in [0, 1).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / ((1u64 << 24) as f32)
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next() * (hi - lo)
    }
}

fn assert_matches_linear(graph: &NavGraph, point: Vec3) {
    assert_eq!(
        graph.region_at(point),
        graph.region_at_linear(point),
        "region_at at {point:?}"
    );
    assert_eq!(
        graph.resolve_region_at(point),
        graph.resolve_region_at_linear(point),
        "resolve_region_at at {point:?}"
    );
}

fn region(x0: u32, z0: u32, x1: u32, z1: u32, floor: (f32, f32)) -> NavRegion {
    NavRegion {
        x0,
        z0,
        x1,
        z1,
        floor_y_min: floor.0,
        floor_y_max: floor.1,
    }
}

/// Edge cases on a non-integral origin and a 0.7 m cell. Snap tolerance is
/// 0.3 + 1.5 × 0.7 = 1.35 m.
fn edge_case_graph() -> NavGraph {
    NavGraph::from_section(&NavMeshSection {
        version: NAVMESH_VERSION,
        origin: [0.37, 1.0, -5.13],
        cell_size: 0.7,
        dim_x: 20,
        dim_z: 20,
        agent_radius: 0.3,
        agent_height: 1.8,
        step_height: 0.4,
        max_slope_deg: 45.0,
        regions: vec![
            // 0, 1: stacked floors at one XZ.
            region(0, 0, 4, 4, (0.0, 0.25)),
            region(0, 0, 4, 4, (3.0, 3.25)),
            // 2, 3: identical footprint and band, so equal floor distance
            // everywhere: the lower index wins.
            region(6, 0, 9, 3, (0.0, 0.25)),
            region(6, 0, 9, 3, (0.0, 0.25)),
            // 4, 5: two cells apart on X with equal bands: the midpoint is an
            // equal-XZ-distance snap tie.
            region(0, 8, 2, 10, (0.0, 0.25)),
            region(4, 8, 6, 10, (0.0, 0.25)),
            // 6, 7: the same gap, bands differing, so the tie breaks on Y.
            region(10, 8, 12, 10, (1.0, 1.25)),
            region(14, 8, 16, 10, (0.0, 0.25)),
            // 8: a rect reaching past the grid's dimensions.
            region(17, 15, 26, 23, (0.0, 0.25)),
            // 9: abutting region 8 on X.
            region(12, 15, 17, 18, (0.0, 0.25)),
        ],
        portals: Vec::new(),
        gates: Vec::new(),
        links: Vec::new(),
    })
}

fn nudged(value: f32) -> [f32; 3] {
    [value.next_down(), value, value.next_up()]
}

#[test]
fn indexed_lookup_equals_linear_scan_on_edge_cases() {
    let graph = edge_case_graph();
    let grid = graph.grid();
    let tolerance = graph.region_snap_tolerance();
    let ys = [-1.0, 0.1, 0.25, 1.1, 1.625, 2.0, 3.1, 5.0];

    // Every cell centre of the grid and of a ring beyond it.
    for z in -4..(grid.dim_z as i32 + 8) {
        for x in -4..(grid.dim_x as i32 + 8) {
            for y in ys {
                let point = Vec3::new(
                    grid.origin[0] + (x as f32 + 0.5) * grid.cell_size,
                    y,
                    grid.origin[2] + (z as f32 + 0.5) * grid.cell_size,
                );
                assert_matches_linear(&graph, point);
            }
        }
    }

    for record in graph.regions() {
        let [x_min, x_max] = [record.world_min_xz[0], record.world_max_xz[0]];
        let [z_min, z_max] = [record.world_min_xz[1], record.world_max_xz[1]];
        let mid_x = (x_min + x_max) * 0.5;
        let mid_z = (z_min + z_max) * 0.5;
        // Exactly on, and one ulp either side of, each min and max edge.
        for x in nudged(x_min).into_iter().chain(nudged(x_max)) {
            for z in nudged(z_min)
                .into_iter()
                .chain(nudged(z_max))
                .chain([mid_z])
            {
                for y in ys {
                    assert_matches_linear(&graph, Vec3::new(x, y, z));
                }
            }
        }
        // Just inside and just outside the snap tolerance off each side.
        for offset in nudged(tolerance) {
            for y in ys {
                assert_matches_linear(&graph, Vec3::new(x_min - offset, y, mid_z));
                assert_matches_linear(&graph, Vec3::new(x_max + offset, y, mid_z));
                assert_matches_linear(&graph, Vec3::new(mid_x, y, z_min - offset));
                assert_matches_linear(&graph, Vec3::new(mid_x, y, z_max + offset));
                // Off a corner, at the tolerance on the diagonal.
                let diagonal = offset / std::f32::consts::SQRT_2;
                assert_matches_linear(&graph, Vec3::new(x_max + diagonal, y, z_max + diagonal));
            }
        }
    }

    // Equal-XZ-distance snap ties: the midpoint of each two-cell gap.
    let regions = graph.regions();
    for (a, b) in [(4, 5), (6, 7)] {
        let mid_x = (regions[a].world_max_xz[0] + regions[b].world_min_xz[0]) * 0.5;
        let mid_z = (regions[a].world_min_xz[1] + regions[a].world_max_xz[1]) * 0.5;
        for y in ys {
            let point = Vec3::new(mid_x, y, mid_z);
            assert!(graph.region_at(point).is_none(), "the gap is off-mesh");
            assert!(graph.resolve_region_at(point).is_some(), "within tolerance");
            assert_matches_linear(&graph, point);
        }
    }

    // Non-finite points take the linear scan's answer, whatever it is.
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for point in [
            Vec3::new(bad, 0.1, 1.0),
            Vec3::new(1.0, 0.1, bad),
            Vec3::new(1.0, bad, -4.0),
            Vec3::new(bad, bad, bad),
        ] {
            assert_matches_linear(&graph, point);
        }
    }

    let mut rng = Lcg(7);
    for _ in 0..50_000 {
        let point = Vec3::new(
            rng.range(-6.0, 26.0),
            rng.range(-1.0, 4.0),
            rng.range(-12.0, 18.0),
        );
        assert_matches_linear(&graph, point);
    }
}

#[test]
fn indexed_lookup_equals_linear_scan_on_the_multi_arena_level() {
    let graph = NavGraph::from_section(&multi_arena_map().section);
    let grid = graph.grid();
    for z in -2..(grid.dim_z as i32 + 2) {
        for x in -2..(grid.dim_x as i32 + 2) {
            for y in [-0.5, 0.1, 2.0] {
                let point = Vec3::new(
                    grid.origin[0] + (x as f32 + 0.5) * grid.cell_size,
                    y,
                    grid.origin[2] + (z as f32 + 0.5) * grid.cell_size,
                );
                assert_matches_linear(&graph, point);
            }
        }
    }
    let mut rng = Lcg(0x5eed);
    for _ in 0..20_000 {
        let point = Vec3::new(
            rng.range(-12.0, 390.0),
            rng.range(-0.5, 1.5),
            rng.range(-8.0, 72.0),
        );
        assert_matches_linear(&graph, point);
    }
}

fn examined(lookup: impl FnOnce()) -> usize {
    REGIONS_EXAMINED.with(|count| count.set(0));
    lookup();
    REGIONS_EXAMINED.with(|count| count.get())
}

// P3: on a level of 4,000+ regions a lookup examines the candidate lists of
// the buckets its snap tolerance spans, never every region. A linear fallback
// examines all 4,216 and fails.
#[test]
fn region_lookup_on_a_large_level_examines_only_nearby_regions() {
    let graph = NavGraph::from_section(&multi_arena_map().section);
    assert!(graph.regions().len() >= 4_000);
    // Buckets are max(cell, tolerance) = 1.85 m wide over 2 m tiles: a point's
    // bucket meets at most 4 tiles, and the snap span (6 × 6 buckets, 11.1 m)
    // at most 7 × 7.
    const ON_MESH_BOUND: usize = 4;
    const SNAP_BOUND: usize = ON_MESH_BOUND + 49;

    let mut rng = Lcg(3);
    let mut worst = (0, 0);
    for _ in 0..20_000 {
        let point = Vec3::new(
            rng.range(-12.0, 390.0),
            rng.range(-0.5, 1.5),
            rng.range(-8.0, 72.0),
        );
        let at = examined(|| {
            graph.region_at(point);
        });
        let resolve = examined(|| {
            graph.resolve_region_at(point);
        });
        assert!(at <= ON_MESH_BOUND, "region_at examined {at} at {point:?}");
        assert!(
            resolve <= SNAP_BOUND,
            "resolve_region_at examined {resolve} at {point:?}"
        );
        worst = (worst.0.max(at), worst.1.max(resolve));
    }
    assert!(
        worst.1 > ON_MESH_BOUND,
        "some samples were off-mesh and snapped"
    );
}
