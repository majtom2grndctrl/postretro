// Traversal guards ported from the chain walk: its fixtures, through the region walk.
// See: context/lib/rendering_pipeline.md §2

use glam::{Mat4, Vec3};
use postretro_level_loader::{CellData, LevelWorld, PortalData};

use super::entries::portal_traverse_capped;
use super::test_fixtures::{camera_view_proj, sample_cameras, stacked_slab_shaft};
use super::tests::SWEEP_SEED;
use super::{
    PortalWalkScratch, RegionWalkStats, WalkTrace, WalkView, WalkWorld, portal_traverse,
    walk_from_camera, walk_from_camera_traced,
};
use crate::visibility::extract_frustum_planes;

/// Runtime-default projection looking along `look_dir`.
fn make_view_proj(position: Vec3, look_dir: Vec3) -> Mat4 {
    let target = position + look_dir;
    let view = glam::camera::rh::view::look_at_mat4(position, target, Vec3::Y);
    let aspect = 16.0 / 9.0;
    let hfov = 100.0_f32.to_radians();
    let vfov = 2.0 * ((hfov / 2.0).tan() / aspect).atan();
    let proj = glam::camera::rh::proj::directx::perspective(vfov, aspect, 0.1, 4096.0);
    proj * view
}

/// The bake-facing camera entry from `camera_cell`, as a per-cell flag.
fn traverse(eye: Vec3, view_proj: Mat4, camera_cell: usize, world: &LevelWorld) -> Vec<bool> {
    let mut scratch = PortalWalkScratch::new();
    let walk = portal_traverse(eye, view_proj, camera_cell, world, &mut scratch);
    assert!(!walk.step_limit_hit);
    let mut visible = vec![false; world.cell_count()];
    for &cell in walk.reached {
        visible[cell as usize] = true;
    }
    visible
}

struct TracedWalk {
    visible: Vec<bool>,
    stats: RegionWalkStats,
    trace: Option<String>,
}

/// The camera walk with blocked portals, capturing the trace when `capture`.
fn traced_walk(
    eye: Vec3,
    view_proj: Mat4,
    camera_cell: usize,
    world: &LevelWorld,
    blocked_portals: &[bool],
    capture: bool,
) -> TracedWalk {
    let mut scratch = PortalWalkScratch::new();
    let mut trace = capture.then(|| WalkTrace::begin(eye, camera_cell, world));
    let input = WalkWorld {
        world,
        blocked_portals,
    };
    let view = WalkView::camera(eye, view_proj);
    let stats = walk_from_camera_traced(
        input,
        &view,
        camera_cell,
        u32::MAX,
        &mut scratch,
        trace.as_mut(),
    );
    let trace = trace.map(|trace| trace.finish(&stats, scratch.reached().len()));
    let visible = (0..world.cell_count())
        .map(|cell| scratch.is_reached(cell))
        .collect();
    TracedWalk {
        visible,
        stats,
        trace,
    }
}

fn test_cell(bounds_min: Vec3, bounds_max: Vec3, is_solid: bool) -> CellData {
    CellData {
        bounds_min,
        bounds_max,
        face_start: 0,
        face_count: 0,
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid,
        is_exterior: false,
        is_drawable: false,
    }
}

fn set_portal_refs(cells: &mut [CellData], refs_by_cell: &[&[u32]]) -> Vec<u32> {
    let mut refs = Vec::new();
    for (cell, cell_refs) in cells.iter_mut().zip(refs_by_cell.iter()) {
        if cell_refs.is_empty() {
            cell.portal_ref_start = 0;
            cell.portal_ref_count = 0;
        } else {
            cell.portal_ref_start = refs.len() as u32;
            cell.portal_ref_count = cell_refs.len() as u32;
            refs.extend_from_slice(cell_refs);
        }
    }
    refs
}

fn test_world(
    mut cells: Vec<CellData>,
    refs_by_cell: &[&[u32]],
    portals: Vec<PortalData>,
) -> LevelWorld {
    let cell_portal_refs = set_portal_refs(&mut cells, refs_by_cell);
    LevelWorld::new_visibility_only(
        cells,
        cell_portal_refs,
        postretro_level_loader::CellLocatorChild::Cell(0),
        vec![],
        portals,
        true,
    )
    .expect("valid portal visibility test world")
}

fn empty_test_world() -> LevelWorld {
    LevelWorld::new_visibility_only(
        Vec::new(),
        Vec::new(),
        postretro_level_loader::CellLocatorChild::Cell(0),
        vec![],
        Vec::new(),
        false,
    )
    .expect("valid empty visibility-only test world")
}

/// Build a three-cell chain: A (cell 0) -- portal 0 -- B (cell 1) -- portal 1 -- C (cell 2)
/// arranged along the X axis.
fn three_cell_chain() -> LevelWorld {
    let portal_0 = PortalData {
        polygon: vec![
            Vec3::new(32.0, 0.0, 0.0),
            Vec3::new(32.0, 64.0, 0.0),
            Vec3::new(32.0, 64.0, 64.0),
            Vec3::new(32.0, 0.0, 64.0),
        ],
        front_cell: 0,
        back_cell: 1,
    };
    let portal_1 = PortalData {
        polygon: vec![
            Vec3::new(64.0, 0.0, 0.0),
            Vec3::new(64.0, 64.0, 0.0),
            Vec3::new(64.0, 64.0, 64.0),
            Vec3::new(64.0, 0.0, 64.0),
        ],
        front_cell: 1,
        back_cell: 2,
    };

    test_world(
        vec![
            test_cell(Vec3::new(0.0, 0.0, 0.0), Vec3::new(32.0, 64.0, 64.0), false),
            test_cell(
                Vec3::new(32.0, 0.0, 0.0),
                Vec3::new(64.0, 64.0, 64.0),
                false,
            ),
            test_cell(
                Vec3::new(64.0, 0.0, 0.0),
                Vec3::new(96.0, 64.0, 64.0),
                false,
            ),
        ],
        &[&[0], &[0, 1], &[1]],
        vec![portal_0, portal_1],
    )
}

#[test]
fn portal_traverse_camera_cell_always_visible() {
    let world = three_cell_chain();
    // Camera in cell 0, looking away from all portals.
    let view_proj = make_view_proj(Vec3::new(16.0, 32.0, 32.0), Vec3::NEG_X);
    let visible = traverse(Vec3::new(16.0, 32.0, 32.0), view_proj, 0, &world);
    assert!(visible[0], "camera cell should always be visible");
}

#[test]
fn portal_traverse_straight_corridor_sees_all_three() {
    let world = three_cell_chain();
    // Camera in cell 0, looking through portals toward +X.
    let camera_pos = Vec3::new(16.0, 32.0, 32.0);
    let view_proj = make_view_proj(camera_pos, Vec3::X);
    let visible = traverse(camera_pos, view_proj, 0, &world);
    assert!(visible[0], "camera cell A should be visible");
    assert!(visible[1], "cell B should be visible through portal 0");
    assert!(visible[2], "cell C should be visible through portals 0+1");
}

#[test]
fn blocked_portal_rejects_in_both_directions_and_reports_trace_stat() {
    let world = three_cell_chain();
    let camera_from_a = Vec3::new(16.0, 32.0, 32.0);
    let view_proj_from_a = make_view_proj(camera_from_a, Vec3::X);
    let from_a = traced_walk(camera_from_a, view_proj_from_a, 0, &world, &[true], true);

    assert!(from_a.visible[0], "camera cell stays visible");
    assert!(!from_a.visible[1], "blocked portal must reject A→B");
    assert_eq!(from_a.stats.rejected_blocked, 1);
    let trace = from_a
        .trace
        .expect("captured portal walk must include a trace");
    assert!(
        trace.contains("blocked"),
        "blocked reject missing from trace: {trace}"
    );
    assert!(
        trace.contains("blocked=1"),
        "blocked reject missing from trace summary: {trace}"
    );

    let camera_from_c = Vec3::new(80.0, 32.0, 32.0);
    let view_proj_from_c = make_view_proj(camera_from_c, Vec3::NEG_X);
    let from_c = traced_walk(
        camera_from_c,
        view_proj_from_c,
        2,
        &world,
        &[false, true],
        false,
    );

    assert!(from_c.visible[2], "camera cell stays visible");
    assert!(!from_c.visible[1], "blocked portal must reject C→B");
    assert_eq!(from_c.stats.rejected_blocked, 1);
    assert!(from_c.trace.is_none());
}

#[test]
fn portal_traverse_reports_step_limit_before_unbounded_walk() {
    let world = three_cell_chain();
    let camera_pos = Vec3::new(16.0, 32.0, 32.0);
    let view_proj = make_view_proj(camera_pos, Vec3::X);

    let mut scratch = PortalWalkScratch::new();
    let input = WalkWorld {
        world: &world,
        blocked_portals: &[],
    };
    let stats = walk_from_camera(
        input,
        &WalkView::camera(camera_pos, view_proj),
        0,
        0,
        &mut scratch,
    );

    assert!(
        stats.step_limit_hit,
        "zero step budget should trip the traversal fuse"
    );
    assert_eq!(
        stats.considered, 0,
        "no portal should be considered after the budget trips"
    );
    assert!(scratch.is_reached(0), "camera cell remains visible");
    assert!(
        !scratch.is_reached(1),
        "walk stops before entering neighbor cells"
    );
}

#[test]
fn portal_traverse_looking_away_hides_distant_leaves() {
    let world = three_cell_chain();
    // Camera in cell 0, looking away from the portals (toward -X).
    let camera_pos = Vec3::new(16.0, 32.0, 32.0);
    let view_proj = make_view_proj(camera_pos, Vec3::NEG_X);
    let visible = traverse(camera_pos, view_proj, 0, &world);
    assert!(visible[0], "camera cell should be visible");
    // Portals are at X=32 and X=64, camera looks toward -X, so they're behind.
    assert!(
        !visible[1],
        "cell B should not be visible when looking away"
    );
    assert!(
        !visible[2],
        "cell C should not be visible when looking away"
    );
}

#[test]
fn portal_traverse_skips_solid_neighbors() {
    let mut world = three_cell_chain();
    world.cells[1].is_solid = true;

    let camera_pos = Vec3::new(16.0, 32.0, 32.0);
    let view_proj = make_view_proj(camera_pos, Vec3::X);
    let visible = traverse(camera_pos, view_proj, 0, &world);
    assert!(visible[0], "camera cell should be visible");
    assert!(!visible[1], "solid cell should not be visible");
    // Cell 2 is behind solid cell 1, so it can't be reached.
    assert!(!visible[2], "cell behind solid should not be visible");
}

#[test]
fn portal_traverse_empty_world() {
    let world = empty_test_world();

    let view_proj = make_view_proj(Vec3::ZERO, Vec3::NEG_Z);
    let mut scratch = PortalWalkScratch::new();
    let walk = portal_traverse(Vec3::ZERO, view_proj, 0, &world, &mut scratch);
    assert!(walk.reached.is_empty());
    assert!(!walk.step_limit_hit);
}

#[test]
fn portal_traverse_l_shaped_corridor_hides_c() {
    // L-shaped corridor: A -- portal 0 (at X=32 in YZ plane) -- B -- portal 1 (at Z=64 in XY plane) -- C
    // Camera in A looking along +X sees B through portal 0,
    // but portal 1 is perpendicular (in the Z direction), so C is not visible
    // through the rect portal 0 leaves on B: portal 1's clip against that rect is empty.
    let portal_0 = PortalData {
        polygon: vec![
            Vec3::new(32.0, 0.0, 0.0),
            Vec3::new(32.0, 64.0, 0.0),
            Vec3::new(32.0, 64.0, 64.0),
            Vec3::new(32.0, 0.0, 64.0),
        ],
        front_cell: 0,
        back_cell: 1,
    };
    // Portal 1 is on the Z=64 plane — perpendicular to the camera's line of sight.
    // Positioned far to the +Z side of the corridor.
    let portal_1 = PortalData {
        polygon: vec![
            Vec3::new(32.0, 0.0, 200.0),
            Vec3::new(64.0, 0.0, 200.0),
            Vec3::new(64.0, 64.0, 200.0),
            Vec3::new(32.0, 64.0, 200.0),
        ],
        front_cell: 1,
        back_cell: 2,
    };

    let world = test_world(
        vec![
            test_cell(Vec3::new(0.0, 0.0, 0.0), Vec3::new(32.0, 64.0, 64.0), false),
            test_cell(
                Vec3::new(32.0, 0.0, 0.0),
                Vec3::new(64.0, 64.0, 200.0),
                false,
            ),
            test_cell(
                Vec3::new(32.0, 0.0, 200.0),
                Vec3::new(64.0, 64.0, 264.0),
                false,
            ),
        ],
        &[&[0], &[0, 1], &[1]],
        vec![portal_0, portal_1],
    );

    // Camera in cell A, looking straight along +X toward portal 0.
    let camera_pos = Vec3::new(16.0, 32.0, 32.0);
    let view_proj = make_view_proj(camera_pos, Vec3::X);
    let visible = traverse(camera_pos, view_proj, 0, &world);
    assert!(visible[0], "camera cell A should be visible");
    assert!(visible[1], "cell B should be visible through portal 0");
    assert!(
        !visible[2],
        "cell C should not be visible — portal 1 is around the corner at Z=200"
    );
}

#[test]
fn portal_traversal_sees_room_through_both_sides_of_pillar() {
    // Room layout with NARROW portals (2 units wide) matching the pillar
    // gap dimensions that cause issues in portal generation:
    //
    // Cell A (camera room, X=0..120) --[portal 0 at X=120, Z=62..64]--> Cell B (left gap)
    //                                --[portal 1 at X=120, Z=66..68]--> Cell C (right gap)
    // Cell B --[portal 2 at X=136, Z=62..64]--> Cell D (far room, X=136..256)
    // Cell C --[portal 3 at X=136, Z=66..68]--> Cell D
    //
    // The portals are only 2 units wide (matching a narrow doorway gap).
    let portal_a_b = PortalData {
        polygon: vec![
            Vec3::new(120.0, 16.0, 62.0),
            Vec3::new(120.0, 112.0, 62.0),
            Vec3::new(120.0, 112.0, 64.0),
            Vec3::new(120.0, 16.0, 64.0),
        ],
        front_cell: 0,
        back_cell: 1,
    };
    let portal_a_c = PortalData {
        polygon: vec![
            Vec3::new(120.0, 16.0, 66.0),
            Vec3::new(120.0, 112.0, 66.0),
            Vec3::new(120.0, 112.0, 68.0),
            Vec3::new(120.0, 16.0, 68.0),
        ],
        front_cell: 0,
        back_cell: 2,
    };
    let portal_b_d = PortalData {
        polygon: vec![
            Vec3::new(136.0, 16.0, 62.0),
            Vec3::new(136.0, 112.0, 62.0),
            Vec3::new(136.0, 112.0, 64.0),
            Vec3::new(136.0, 16.0, 64.0),
        ],
        front_cell: 1,
        back_cell: 3,
    };
    let portal_c_d = PortalData {
        polygon: vec![
            Vec3::new(136.0, 16.0, 66.0),
            Vec3::new(136.0, 112.0, 66.0),
            Vec3::new(136.0, 112.0, 68.0),
            Vec3::new(136.0, 16.0, 68.0),
        ],
        front_cell: 2,
        back_cell: 3,
    };

    let world = test_world(
        vec![
            test_cell(
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(120.0, 128.0, 128.0),
                false,
            ),
            test_cell(
                Vec3::new(120.0, 16.0, 62.0),
                Vec3::new(136.0, 112.0, 64.0),
                false,
            ),
            test_cell(
                Vec3::new(120.0, 16.0, 66.0),
                Vec3::new(136.0, 112.0, 68.0),
                false,
            ),
            test_cell(
                Vec3::new(136.0, 0.0, 0.0),
                Vec3::new(256.0, 128.0, 128.0),
                false,
            ),
        ],
        &[&[0, 1], &[0, 2], &[1, 3], &[2, 3]],
        vec![portal_a_b, portal_a_c, portal_b_d, portal_c_d],
    );

    // Camera looking through the LEFT passage (Z=63, center of Z=62..64 gap).
    // Camera is in cell A, looking toward +X.
    {
        let camera_pos = Vec3::new(16.0, 64.0, 63.0);
        let view_proj = make_view_proj(camera_pos, Vec3::X);
        let visible = traverse(camera_pos, view_proj, 0, &world);
        assert!(visible[0], "camera cell A should be visible");
        assert!(
            visible[1],
            "cell B (left gap) should be visible when looking through left doorway"
        );
        assert!(
            visible[3],
            "cell D (far room) should be visible through left passage (A->B->D). \
             If not, B's rect (the clipped bounds of the 2-unit-wide portal A-B) may be \
             clipping the 2-unit-wide portal B-D to nothing."
        );
    }

    // Camera looking through the RIGHT passage (Z=67, center of Z=66..68 gap).
    {
        let camera_pos = Vec3::new(16.0, 64.0, 67.0);
        let view_proj = make_view_proj(camera_pos, Vec3::X);
        let visible = traverse(camera_pos, view_proj, 0, &world);
        assert!(visible[0], "camera cell A should be visible");
        assert!(
            visible[2],
            "cell C (right gap) should be visible when looking through right doorway"
        );
        assert!(
            visible[3],
            "cell D (far room) should be visible through right passage (A->C->D). \
             If not, C's rect (the clipped bounds of the 2-unit-wide portal A-C) may be \
             clipping the 2-unit-wide portal C-D to nothing."
        );
    }
}

#[test]
fn portal_traverse_straddling_portal_hides_unreachable_side_branch() {
    // Straight-through layout: camera in cell 0 looking +X.
    // Portal 0 (A -> B) straddles the camera's side plane — it extends
    // far beyond the frustum to the +Y direction. If the portal were not
    // clipped against A's rect, the rect it hands to B would extend into
    // off-screen regions the camera cannot see, and B's rect could then
    // admit off-axis neighbors.
    //
    // This test asserts that with the clip against A's rect in place, cell B is still
    // visible (the portal is in view) and a far off-axis cell C reached
    // through an orthogonal portal at cell B is correctly hidden.
    let portal_a_b = PortalData {
        polygon: vec![
            // 1000-unit-tall portal at X=10, centered on Z=0.
            Vec3::new(10.0, -500.0, -1.0),
            Vec3::new(10.0, 500.0, -1.0),
            Vec3::new(10.0, 500.0, 1.0),
            Vec3::new(10.0, -500.0, 1.0),
        ],
        front_cell: 0,
        back_cell: 1,
    };
    // Portal 1 (B -> C) is far out in +Y, well outside the camera's
    // actual view cone even though cell B is reachable.
    let portal_b_c = PortalData {
        polygon: vec![
            Vec3::new(15.0, 400.0, -1.0),
            Vec3::new(20.0, 400.0, -1.0),
            Vec3::new(20.0, 400.0, 1.0),
            Vec3::new(15.0, 400.0, 1.0),
        ],
        front_cell: 1,
        back_cell: 2,
    };

    let world = test_world(
        vec![
            test_cell(
                Vec3::new(0.0, -500.0, -500.0),
                Vec3::new(10.0, 500.0, 500.0),
                false,
            ),
            test_cell(
                Vec3::new(10.0, -500.0, -500.0),
                Vec3::new(25.0, 500.0, 500.0),
                false,
            ),
            test_cell(
                Vec3::new(15.0, 400.0, -500.0),
                Vec3::new(25.0, 600.0, 500.0),
                false,
            ),
        ],
        &[&[0], &[0, 1], &[1]],
        vec![portal_a_b, portal_b_c],
    );

    let camera_pos = Vec3::new(1.0, 0.0, 0.0);
    let view_proj = make_view_proj(camera_pos, Vec3::X);
    let visible = traverse(camera_pos, view_proj, 0, &world);

    assert!(visible[0], "camera cell should always be visible");
    assert!(
        visible[1],
        "cell B should be visible through the straddling portal"
    );
    assert!(
        !visible[2],
        "cell C should be hidden: portal 1 is far off-axis and \
         unreachable through B's clipped rect"
    );
}

/// Regression test for the "two paths to the same cell, narrower path
/// wins, downstream reach is lost" topology. Kept for a failure shape the old
/// chain walk had (it planted the first-arriving narrow frustum at X); the
/// region walk avoids it by growing X's rect from every arrival.
///
/// Topology (abstract; bounding boxes are not used by portal_traverse):
///
///   A (camera) -- portal 0 (NARROW 0.1x0.1 at X=10) --> B
///   A          -- portal 1 (WIDE   4.0x4.0 at X=10) --> C
///   B          -- portal 2 (2.0x2.0 at X=20) --------> X
///   C          -- portal 3 (2.0x2.0 at X=20) --------> X
///   X          -- portal 4 (1.0x1.0 at X=30, offset  -> Y
///                           to Y=1..2, Z=-0.5..0.5)
///
/// All portals lie in YZ planes perpendicular to +X. Portal 0 and portal 1
/// share the same spatial position (both at X=10 centered on the origin)
/// — portal_traverse cares about topology and polygon shape, not physical
/// room layout, so this is a legal test fixture. Likewise portals 2 and 3
/// overlap at X=20.
///
/// The load-bearing geometry:
///
/// - Camera frustum (100° hfov) easily contains both A-outbound portals.
/// - B's rect is the clip of portal 0's 0.1x0.1 slit: a tiny screen region.
///   Portal 2 clipped against it yields a tiny rect for X.
/// - C's rect is the clip of portal 1's 4.0x4.0 aperture: a wide screen
///   region. Portal 3 clipped against it yields a wide rect for X.
/// - Portal 4 is offset to Y=1..2 so it lies **outside** the tiny rect X
///   gets via B (its clip is empty) and **inside** the wide rect X gets
///   via C (passes through cleanly).
///
/// Old failure shape (cell-keyed BFS): A's outbound order is [0, 1], so A→B
/// ran first and planted the narrow frustum at X; A→C→X was then skipped as
/// already visited, portal 4 was tested against the narrow frustum, and Y was
/// missed.
///
/// Region walk: X's rect is grow-only. The arrival via B contributes a tiny
/// rect and the arrival via C a wide one; X's rect is their union, so portal 4
/// clipped against X's rect is non-empty and Y is reached. Arrival order does
/// not matter (the test also runs the reversed portal order).
///
/// The test asserts visibility of all five cells; the old BFS failed on
/// `visible[4]` (Y).
#[test]
fn portal_traverse_two_paths_to_same_leaf_uses_widest_frustum() {
    // Portal 0: A→B, NARROW slit at X=10.
    let portal_a_b = PortalData {
        polygon: vec![
            Vec3::new(10.0, -0.05, -0.05),
            Vec3::new(10.0, 0.05, -0.05),
            Vec3::new(10.0, 0.05, 0.05),
            Vec3::new(10.0, -0.05, 0.05),
        ],
        front_cell: 0, // A
        back_cell: 1,  // B
    };

    // Portal 1: A→C, WIDE aperture at X=10 (same spatial position as
    // portal 0; they serve different topology roles in this abstract
    // fixture).
    let portal_a_c = PortalData {
        polygon: vec![
            Vec3::new(10.0, -2.0, -2.0),
            Vec3::new(10.0, 2.0, -2.0),
            Vec3::new(10.0, 2.0, 2.0),
            Vec3::new(10.0, -2.0, 2.0),
        ],
        front_cell: 0, // A
        back_cell: 2,  // C
    };

    // Portal 2: B→X at X=20.
    let portal_b_x = PortalData {
        polygon: vec![
            Vec3::new(20.0, -1.0, -1.0),
            Vec3::new(20.0, 1.0, -1.0),
            Vec3::new(20.0, 1.0, 1.0),
            Vec3::new(20.0, -1.0, 1.0),
        ],
        front_cell: 1, // B
        back_cell: 3,  // X
    };

    // Portal 3: C→X at X=20 (same spatial position as portal 2).
    let portal_c_x = PortalData {
        polygon: vec![
            Vec3::new(20.0, -1.0, -1.0),
            Vec3::new(20.0, 1.0, -1.0),
            Vec3::new(20.0, 1.0, 1.0),
            Vec3::new(20.0, -1.0, 1.0),
        ],
        front_cell: 2, // C
        back_cell: 3,  // X
    };

    // Portal 4: X→Y at X=30, offset to Y=1..2 so it sits outside the
    // tiny rect X gets via B and inside the wide rect X gets via C.
    let portal_x_y = PortalData {
        polygon: vec![
            Vec3::new(30.0, 1.0, -0.5),
            Vec3::new(30.0, 2.0, -0.5),
            Vec3::new(30.0, 2.0, 0.5),
            Vec3::new(30.0, 1.0, 0.5),
        ],
        front_cell: 3, // X
        back_cell: 4,  // Y
    };

    let cell_template = || {
        test_cell(
            Vec3::new(-1000.0, -1000.0, -1000.0),
            Vec3::new(1000.0, 1000.0, 1000.0),
            false,
        )
    };

    let ordered = test_world(
        vec![
            cell_template(), // 0: A
            cell_template(), // 1: B
            cell_template(), // 2: C
            cell_template(), // 3: X
            cell_template(), // 4: Y
        ],
        // A lists portal 0 (narrow) BEFORE portal 1 (wide), the order in
        // which the old BFS planted the narrow frustum at X first.
        &[&[0, 1], &[0, 2], &[1, 3], &[2, 3, 4], &[4]],
        vec![portal_a_b, portal_a_c, portal_b_x, portal_c_x, portal_x_y],
    );

    // Row 4: the wider opening wins under both portal orders. Validation
    // wants ascending refs, so the reversed copy is permuted after building.
    let mut reversed = test_world(
        ordered.cells.clone(),
        &[&[0, 1], &[0, 2], &[1, 3], &[2, 3, 4], &[4]],
        ordered.portals.clone(),
    );
    for cell in &ordered.cells {
        let start = cell.portal_ref_start as usize;
        reversed.cell_portal_refs[start..start + cell.portal_ref_count as usize].reverse();
    }
    assert_ne!(reversed.cell_portal_refs, ordered.cell_portal_refs);

    for world in [ordered, reversed] {
        // Camera at origin looking +X. The camera frustum is wide enough that
        // both A-outbound portals are accepted on the first hop.
        let camera_pos = Vec3::new(0.0, 0.0, 0.0);
        let view_proj = make_view_proj(camera_pos, Vec3::X);
        let visible = traverse(camera_pos, view_proj, 0, &world);

        assert!(visible[0], "cell A (camera) must be visible");
        assert!(visible[1], "cell B must be visible (A→B direct)");
        assert!(visible[2], "cell C must be visible (A→C direct)");
        assert!(
            visible[3],
            "cell X must be visible (reachable via either path)"
        );
        assert!(
            visible[4],
            "cell Y must be visible via the A→C→X→Y route. X's rect must \
         grow to include the wide arrival via C; if only the narrow \
         arrival via B counted (the old BFS behaviour), X→Y would clip \
         to empty."
        );
    }
}

/// Regression probe: camera sits 0.03 units from a vertical portal
/// wall, reproducing the blank-frame scenario captured from
/// `test-3.prl` at 2026-04-11T22:52:11Z. Camera at `(4.91, 0.92,
/// -14.67)` inside cell 99 whose -X wall is on `x = 4.88`.
///
/// **Root cause (confirmed by diagnostic trace on 2026-04-11):** the
/// render-pipeline near clip (`camera::NEAR = 0.1`) is baked into the
/// view-projection matrix used to build the visibility frustum. When
/// the camera sits within 0.1 units of a portal plane, the entire
/// portal polygon lies between the camera apex and the near plane, so
/// every vertex fails the near-plane test and Sutherland-Hodgman
/// clips the polygon to empty. Side-plane clipping is not the
/// culprit: pushing only the near plane up to the camera apex (near
/// ≈ 0) makes the portal reach its neighbor on this exact fixture.
///
/// The test geometry is copied from the live trace verbatim — same
/// camera position, same cell bounds, same 6.5×4.88 portal rectangle.
/// Cell A is the camera cell (+X side), cell B is the -X neighbor.
///
/// Routes through `visibility::extract_frustum_planes` directly
/// rather than the module-private `extract_test_frustum` copy to
/// exercise the real production path end-to-end.
#[test]
fn portal_traverse_reaches_neighbor_when_camera_is_close_to_portal_wall() {
    const HFOV: f32 = 100.0_f32.to_radians();
    const NEAR: f32 = 0.1;
    const FAR: f32 = 4096.0;

    let portal = PortalData {
        polygon: vec![
            Vec3::new(4.88, 0.00, -17.88),
            Vec3::new(4.88, 0.00, -11.38),
            Vec3::new(4.88, 4.88, -11.38),
            Vec3::new(4.88, 4.88, -17.88),
        ],
        front_cell: 0,
        back_cell: 1,
    };

    let world = test_world(
        vec![
            test_cell(
                Vec3::new(4.88, 0.00, -17.88),
                Vec3::new(11.38, 4.88, -11.38),
                false,
            ),
            test_cell(
                Vec3::new(-50.00, 0.00, -17.88),
                Vec3::new(4.88, 4.88, -11.38),
                false,
            ),
        ],
        &[&[0], &[0]],
        vec![portal],
    );

    // Camera pose from the captured blank-frame trace. The live trace
    // did not record view direction; -X stares straight at the portal
    // wall, which is the natural "camera pressed against a wall"
    // orientation and reproduces the same clip-to-empty symptom.
    let camera_pos = Vec3::new(4.91, 0.92, -14.67);
    let look_dir = Vec3::NEG_X;

    let aspect = 16.0 / 9.0;
    let vfov = 2.0 * ((HFOV / 2.0).tan() / aspect).atan();
    let view = glam::camera::rh::view::look_at_mat4(camera_pos, camera_pos + look_dir, Vec3::Y);
    let proj = glam::camera::rh::proj::directx::perspective(vfov, aspect, NEAR, FAR);
    let view_proj = proj * view;
    let frustum = extract_frustum_planes(view_proj);

    // Precondition: at least one portal vertex must lie outside at
    // least one frustum plane, i.e. the clip step has real work to do.
    // A test that passes because every vertex is trivially inside
    // every plane proves nothing about the clip behaviour.
    let any_vertex_outside = world.portals[0].polygon.iter().any(|&v| {
        frustum
            .planes
            .iter()
            .any(|p| p.normal.dot(v) + p.dist < 0.0)
    });
    assert!(
        any_vertex_outside,
        "precondition failed: every portal vertex is inside every \
         frustum plane, so the clip step is a no-op and this test \
         does not exercise the blank-frame scenario."
    );

    let visible = traverse(camera_pos, view_proj, 0, &world);

    assert!(visible[0], "camera cell must always be visible");
    assert!(
        visible[1],
        "cell B must be reachable through the portal even when the \
         camera sits 0.03 units from the portal plane. Failure here \
         means the visibility frustum's near plane (inherited from \
         the render pipeline's 0.1-unit near clip) is clipping the \
         entire portal polygon to empty — the blank-frame bug in \
         tight corridors."
    );
}

/// Camera sitting **exactly on** a portal plane, looking through it.
///
/// This reproduces the gray-patch flicker captured on
/// `occlusion-test.prl` at 2026-04-17T04:50:11Z. The live trace at
/// frame 360 showed `cam=(-34.54, 6.50, -13.00) leaf=31` with all 7
/// outbound portals rejecting `v=0/4 clip` and `reach=1` — the
/// renderer fell back to drawing just the camera cell while the
/// player could see into several neighbors. The camera Z matched the
/// cell's max-Z face to the float, and adjacent frames oscillated
/// between two view-proj hashes (sub-texel camera jitter), one of
/// which clipped every portal to empty.
///
/// Setup:
/// - Cell A: a slab `x ∈ [−36.37, −34.34], y ∈ [0, 13], z ∈ [−13.41, −13.00]`
///   matching cell 31's bounds from the trace.
/// - Portal on cell A's `+Z` face (`z = −13.00`), shared with cell B
///   on the `+Z` side.
/// - Camera at `(−34.54, 6.50, −13.00)` — the same position as the
///   captured trace, sitting exactly on the portal plane.
/// - Look direction `+Z`, staring straight through the portal.
///
/// If the near-plane slide leaves portal vertices sitting exactly on
/// the slid plane and any side/near plane's `CLIP_EPSILON`
/// classification rejects them as BACK, Sutherland-Hodgman clips the
/// polygon to empty and `cell B` is unreachable — the blank-frame
/// bug, but for the "camera on the portal plane" case rather than
/// "0.03 units in front of it".
#[test]
fn portal_traverse_reaches_neighbor_when_camera_is_on_portal_plane() {
    const HFOV: f32 = 100.0_f32.to_radians();
    const NEAR: f32 = 0.1;
    const FAR: f32 = 4096.0;

    // Portal on the +Z face of cell A, shared with cell B. Vertices
    // are all at z = -13.00 (the plane the camera will sit on).
    let portal = PortalData {
        polygon: vec![
            Vec3::new(-36.37, 0.00, -13.00),
            Vec3::new(-34.34, 0.00, -13.00),
            Vec3::new(-34.34, 13.00, -13.00),
            Vec3::new(-36.37, 13.00, -13.00),
        ],
        front_cell: 0,
        back_cell: 1,
    };

    let world = test_world(
        vec![
            // Cell A — camera cell, matches bounds of legacy leaf 31 from
            // the live trace.
            test_cell(
                Vec3::new(-36.37, 0.00, -13.41),
                Vec3::new(-34.34, 13.00, -13.00),
                false,
            ),
            // Cell B — neighbor on the +Z side of the portal.
            test_cell(
                Vec3::new(-36.37, 0.00, -13.00),
                Vec3::new(-34.34, 13.00, -5.00),
                false,
            ),
        ],
        &[&[0], &[0]],
        vec![portal],
    );

    // Camera pose from the captured blank-frame trace. z = -13.00
    // puts the camera exactly on the portal plane.
    let camera_pos = Vec3::new(-34.54, 6.50, -13.00);
    let look_dir = Vec3::Z;

    let aspect = 16.0 / 9.0;
    let vfov = 2.0 * ((HFOV / 2.0).tan() / aspect).atan();
    let view = glam::camera::rh::view::look_at_mat4(camera_pos, camera_pos + look_dir, Vec3::Y);
    let proj = glam::camera::rh::proj::directx::perspective(vfov, aspect, NEAR, FAR);
    let view_proj = proj * view;

    let visible = traverse(camera_pos, view_proj, 0, &world);

    assert!(visible[0], "camera cell must always be visible");
    assert!(
        visible[1],
        "cell B must be reachable through the portal when the camera \
         sits exactly on the portal plane and looks through it. \
         Failure here means Sutherland-Hodgman is being used even \
         though the view-frustum cross-section at apex depth is a \
         single point — the `camera_on_polygon_plane` bypass in \
         `flood` is not firing. Reproduces the occlusion-test.prl \
         gray-patch flicker captured 2026-04-17T04:50:11Z at \
         cam=(-34.54,6.50,-13.00)."
    );
}

/// Shape-check the compact trace format: header fields, at least one
/// event line, and the summary. Reads the formatted buffer directly —
/// asserting on `log::info!` output would need a custom test logger and
/// would buy nothing over reading the source-of-truth string.
#[test]
fn portal_traverse_capture_emits_compact_header_fields() {
    let world = three_cell_chain();
    let camera_pos = Vec3::new(16.0, 32.0, 32.0);
    let view_proj = make_view_proj(camera_pos, Vec3::X);
    let walk = traced_walk(camera_pos, view_proj, 0, &world, &[], true);
    let buf = walk
        .trace
        .expect("capture: true should produce a trace buffer");

    // Header fields — these are the per-frame camera-cell diagnostics
    // added for the flicker bug hunt.
    assert!(buf.contains("cam=("), "header missing cam=(: {buf}");
    assert!(buf.contains("cell="), "header missing cell=: {buf}");
    assert!(buf.contains("faces="), "header missing faces=: {buf}");
    assert!(buf.contains("bnds=("), "header missing bnds=(: {buf}");
    assert!(buf.contains("cells="), "header missing cells=: {buf}");

    // At least one accepted/rejected event line under the header. The
    // straight corridor walks into cell B and cell C, so there's at
    // least one `  acc ` line.
    let has_event = buf
        .lines()
        .any(|line| line.starts_with("  acc ") || line.starts_with("  rej "));
    assert!(has_event, "expected at least one event line: {buf}");

    // Summary: starts with `  = reach=` and contains `rej[`.
    let summary = buf
        .lines()
        .find(|line| line.starts_with("  = reach="))
        .expect("expected a summary line starting with `  = reach=`");
    assert!(
        summary.contains("rej["),
        "summary missing rej[ bracket: {summary}"
    );
    assert!(summary.starts_with("  = reach=3 "), "{summary}");
}

#[test]
fn portal_trace_names_each_reject_and_the_cap_trip() {
    // Looking away: the facing filter passes (the eye is on A's side), the
    // slid near plane clips both portals behind the eye.
    let world = three_cell_chain();
    let camera_pos = Vec3::new(40.0, 32.0, 32.0);
    let away = traced_walk(
        camera_pos,
        make_view_proj(camera_pos, Vec3::X),
        1,
        &world,
        &[],
        true,
    );
    let buf = away.trace.expect("armed");
    assert!(buf.contains("  rej 1->0 v=0/4 clip"), "{buf}");
    assert!(buf.contains("  acc 1->2 v="), "{buf}");
    assert!(buf.contains("rej[clip=1 "), "{buf}");

    // From C looking back, each cell's portal toward C faces away from the
    // eye: B's portal 1 and A's portal 0.
    let camera_pos = Vec3::new(80.0, 32.0, 32.0);
    let facing = traced_walk(
        camera_pos,
        make_view_proj(camera_pos, Vec3::NEG_X),
        2,
        &world,
        &[],
        true,
    );
    let buf = facing.trace.expect("armed");
    assert!(buf.contains("  rej 1->2 v=4 facing"), "{buf}");
    assert!(buf.contains("  rej 0->1 v=4 facing"), "{buf}");
    assert!(buf.contains("rej[facing=2]"), "{buf}");

    let mut scratch = PortalWalkScratch::new();
    let mut trace = WalkTrace::begin(camera_pos, 2, &world);
    let input = WalkWorld {
        world: &world,
        blocked_portals: &[],
    };
    let view = WalkView::camera(camera_pos, make_view_proj(camera_pos, Vec3::NEG_X));
    let stats = walk_from_camera_traced(input, &view, 2, 0, &mut scratch, Some(&mut trace));
    let buf = trace.finish(&stats, scratch.reached().len());
    assert!(buf.contains("  rej cell=2 limit steps=0"), "{buf}");
    assert!(buf.contains("rej[limit=1]"), "{buf}");
}

#[test]
fn portal_traverse_on_a_cap_trip_keeps_only_cells_the_uncapped_walk_reaches() {
    // Row 36 guard (O5): the bake-facing entry's partial set.
    let grid = stacked_slab_shaft(3, 24);
    let world = &grid.world;
    let mut tripped = 0;
    for pose in sample_cameras(&grid, 60, SWEEP_SEED) {
        if world.cells[pose.cell].is_solid {
            continue;
        }
        let view_proj = camera_view_proj(pose.eye, pose.yaw, pose.pitch);
        let mut full_scratch = PortalWalkScratch::new();
        let full = portal_traverse(pose.eye, view_proj, pose.cell, world, &mut full_scratch);
        assert!(!full.step_limit_hit);
        let mut uncapped = full.reached.to_vec();
        uncapped.sort_unstable();

        let mut scratch = PortalWalkScratch::new();
        let capped =
            portal_traverse_capped(pose.eye, view_proj, pose.cell, world, 40, &mut scratch);
        if !capped.step_limit_hit {
            continue;
        }
        tripped += 1;
        assert_eq!(
            capped.reached.first(),
            Some(&(pose.cell as u32)),
            "{pose:?}"
        );
        for cell in capped.reached {
            assert!(
                uncapped.binary_search(cell).is_ok(),
                "{pose:?}: cell {cell}"
            );
        }
    }
    assert!(tripped > 0, "the small cap trips on some slab camera");
}
