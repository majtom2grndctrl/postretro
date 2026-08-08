// Bake→runtime funnel contract tests.
// See: context/lib/build_pipeline.md §Navigation bake

use glam::Vec3;

use postretro_level_compiler::geometry::{FaceIndexRange, GeometryResult};
use postretro_level_compiler::map_data::NavParams;
use postretro_level_compiler::navmesh_bake::bake_navmesh;
use postretro_level_format::geometry::{FaceMeta, GeometrySection, Vertex};
use postretro_level_format::texture_names::TextureNamesSection;

use crate::collision::SKIN_DISTANCE;
use crate::nav::{NavGraph, find_path};

/// Build a `GeometryResult` from world-space triangles. Mirrors the bake's own
/// `#[cfg(test)]`-private helper (those are private to the compiler crate, so
/// this replicates rather than imports). The bake reads only vertex positions
/// and index triples.
fn geo_from_triangles(triangles: &[[[f32; 3]; 3]]) -> GeometryResult {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for tri in triangles {
        let base = vertices.len() as u32;
        for &pos in tri {
            vertices.push(Vertex::new(
                pos,
                [0.0, 0.0],
                [0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0],
                true,
                [0.0, 0.0],
                0,
            ));
        }
        indices.extend_from_slice(&[base, base + 1, base + 2]);
    }
    GeometryResult {
        geometry: GeometrySection {
            vertices,
            indices,
            faces: vec![FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            }],
        },
        texture_names: TextureNamesSection { names: Vec::new() },
        face_index_ranges: vec![FaceIndexRange {
            index_offset: 0,
            index_count: 3,
        }],
    }
}

/// Two upward-facing triangles for a floor quad at height `y` over
/// `[x0,x1] x [z0,z1]`, wound so the bake treats it as walkable.
fn floor_quad(x0: f32, z0: f32, x1: f32, z1: f32, y: f32) -> [[[f32; 3]; 3]; 2] {
    [
        [[x0, y, z0], [x1, y, z1], [x1, y, z0]],
        [[x0, y, z0], [x0, y, z1], [x1, y, z1]],
    ]
}

/// Zero-erosion bake params at cell_size 0.25 so the region/portal logic is
/// exercised without an eroded margin (the effective funnel inset is then just
/// `SKIN_DISTANCE`).
fn no_erode_params() -> NavParams {
    NavParams {
        agent_radius: 0.0,
        agent_height: 1.8,
        step_height: 0.3,
        max_slope_deg: 45.0,
        cell_size: 0.25,
    }
}

/// Bake the floor triangles and build the runtime graph.
fn baked_graph(triangles: &[[[f32; 3]; 3]]) -> NavGraph {
    let geo = geo_from_triangles(triangles);
    let section = bake_navmesh(&geo, &no_erode_params()).expect("fixture floor must bake");
    NavGraph::from_section(&section)
}

/// H-shaped floor: two rooms abutting in X joined by a 1 m neck at z in
/// [3.5,4.5]. West room x[0,4], east room x[5,9], both z[0,8]; neck x[4,5].
fn ew_doorway_triangles() -> Vec<[[f32; 3]; 3]> {
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    tris.extend_from_slice(&floor_quad(0.0, 0.0, 4.0, 8.0, 0.0)); // west room
    tris.extend_from_slice(&floor_quad(5.0, 0.0, 9.0, 8.0, 0.0)); // east room
    tris.extend_from_slice(&floor_quad(4.0, 3.5, 5.0, 4.5, 0.0)); // 1 m neck
    tris
}

/// N-S doorway: same shape rotated. South room z[0,4], north room z[5,9], both
/// x[0,8]; neck z[4,5] at x[3.5,4.5].
fn ns_doorway_triangles() -> Vec<[[f32; 3]; 3]> {
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    tris.extend_from_slice(&floor_quad(0.0, 0.0, 8.0, 4.0, 0.0)); // south room
    tris.extend_from_slice(&floor_quad(0.0, 5.0, 8.0, 9.0, 0.0)); // north room
    tris.extend_from_slice(&floor_quad(3.5, 4.0, 4.5, 5.0, 0.0)); // 1 m neck
    tris
}

/// L-shaped floor: a full-width bottom bar joined to a left column rising out of
/// it, so the walkable set omits the top-right quadrant. The two rectangles meet
/// at a horizontal (constant-Z) portal whose high-X endpoint is the inner corner
/// at world (2, 2).
fn l_corridor_triangles() -> Vec<[[f32; 3]; 3]> {
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    tris.extend_from_slice(&floor_quad(0.0, 0.0, 6.0, 2.0, 0.0)); // bottom bar
    tris.extend_from_slice(&floor_quad(0.0, 2.0, 2.0, 6.0, 0.0)); // left column
    tris
}

const EPS: f32 = 1.0e-3;

/// The funnel inset off a raw portal endpoint for the zero-erosion fixtures
/// (`agent_radius == 0`, so the effective clearance is just `SKIN_DISTANCE`).
fn inset() -> f32 {
    SKIN_DISTANCE
}

fn approx(a: f32, b: f32) -> bool {
    (a - b).abs() <= EPS
}

/// Regression: the baked constant-X (east-west doorway) portals were emitted
/// z-ascending regardless of `region_a`, so the funnel bent at the FAR jamb and
/// sent enemies to a far wall. The bake and the funnel meet here for the first
/// time.
#[test]
fn baked_ew_doorway_funnels_at_near_jamb_both_directions() {
    let graph = baked_graph(&ew_doorway_triangles());
    // Straight start→goal line at z=7 misses the neck (z in [3.5,4.5]); the near
    // jamb (nearer z=7) is the raw endpoint z=4.5, the far jamb z=3.5. Interior
    // waypoints land at the near jamb or its inward inset (raw − SKIN_DISTANCE),
    // never at the far jamb — the handedness the bake fix restores.
    let near_raw_z = 4.5;
    let far_raw_z = 3.5;
    let band = inset() + EPS;
    for (start, goal) in [
        (Vec3::new(2.0, 0.1, 7.0), Vec3::new(7.0, 0.1, 7.0)),
        (Vec3::new(7.0, 0.1, 7.0), Vec3::new(2.0, 0.1, 7.0)),
    ] {
        let path = find_path(&graph, start, goal).expect("baked EW doorway routes");
        assert!(
            path.len() >= 3,
            "a doorway off the straight line must bend: {path:?}"
        );
        assert!(approx(path[0].x, start.x) && approx(path[0].z, start.z));
        let last = *path.last().unwrap();
        assert!(approx(last.x, goal.x) && approx(last.z, goal.z));
        for w in &path[1..path.len() - 1] {
            assert!(
                (w.z - near_raw_z).abs() <= band,
                "interior waypoint must sit at the NEAR jamb z≈{near_raw_z}, not the far jamb \
                 z≈{far_raw_z}: {path:?}"
            );
        }
    }
}

/// The mirror of the EW case over the constant-Z emitter (which the research
/// showed also produced room-depth vertical portals). Near jamb is x = 3.5.
#[test]
fn baked_ns_doorway_funnels_at_near_jamb_both_directions() {
    let graph = baked_graph(&ns_doorway_triangles());
    // Straight line at x=1 misses the neck (x in [3.5,4.5]); near jamb is the raw
    // endpoint x=3.5, far jamb x=4.5.
    let near_raw_x = 3.5;
    let far_raw_x = 4.5;
    let band = inset() + EPS;
    for (start, goal) in [
        (Vec3::new(1.0, 0.1, 2.0), Vec3::new(1.0, 0.1, 7.0)),
        (Vec3::new(1.0, 0.1, 7.0), Vec3::new(1.0, 0.1, 2.0)),
    ] {
        let path = find_path(&graph, start, goal).expect("baked NS doorway routes");
        assert!(
            path.len() >= 3,
            "a doorway off the straight line must bend: {path:?}"
        );
        assert!(approx(path[0].x, start.x) && approx(path[0].z, start.z));
        let last = *path.last().unwrap();
        assert!(approx(last.x, goal.x) && approx(last.z, goal.z));
        for w in &path[1..path.len() - 1] {
            assert!(
                (w.x - near_raw_x).abs() <= band,
                "interior waypoint must sit at the NEAR jamb x≈{near_raw_x}, not the far jamb \
                 x≈{far_raw_x}: {path:?}"
            );
        }
    }
}

/// AC1 over an explicit L: a bend at the inner corner where the two baked
/// rectangles meet, in both traversal directions, with every leg on the
/// walkable floor.
#[test]
fn baked_l_corridor_bends_at_inner_corner_both_directions() {
    let graph = baked_graph(&l_corridor_triangles());
    let inner_corner = Vec3::new(2.0, 0.0, 2.0);
    for (start, goal) in [
        (Vec3::new(5.0, 0.1, 1.0), Vec3::new(1.0, 0.1, 5.0)),
        (Vec3::new(1.0, 0.1, 5.0), Vec3::new(5.0, 0.1, 1.0)),
    ] {
        let path = find_path(&graph, start, goal).expect("baked L corridor routes");
        assert!(
            path.len() >= 3,
            "the L must introduce an interior bend: {path:?}"
        );
        assert!(approx(path[0].x, start.x) && approx(path[0].z, start.z));
        let last = *path.last().unwrap();
        assert!(approx(last.x, goal.x) && approx(last.z, goal.z));
        // Some interior waypoint sits within a small radius of the inner corner
        // (inset by SKIN_DISTANCE off the raw endpoint), not off at a far wall.
        let bends_at_corner = path[1..path.len() - 1].iter().any(|w| {
            (w.x - inner_corner.x).hypot(w.z - inner_corner.z) <= 3.0 * inset() + EPS
        });
        assert!(
            bends_at_corner,
            "expected a bend at the inner corner {inner_corner:?}: {path:?}"
        );
    }
}

/// AC3: a start→goal line that passes straight through every doorway collapses
/// to `[start, goal]`. The EW fixture routed along z=4.0 threads both neck
/// portals with no bend.
#[test]
fn baked_straight_line_through_doorways_collapses_to_two_points() {
    let graph = baked_graph(&ew_doorway_triangles());
    let start = Vec3::new(2.0, 0.1, 4.0);
    let goal = Vec3::new(7.0, 0.1, 4.0);
    let path = find_path(&graph, start, goal).expect("straight route exists");
    assert_eq!(
        path.len(),
        2,
        "a line through every doorway must not bend: {path:?}"
    );
    assert!(approx(path[0].x, start.x) && approx(path[0].z, start.z));
    assert!(approx(path[1].x, goal.x) && approx(path[1].z, goal.z));
}
