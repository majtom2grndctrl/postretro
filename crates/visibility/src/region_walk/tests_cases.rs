// Region walk named cases: camera-plane, bypass, re-expansion, cap and scratch reuse.

use std::f32::consts::FRAC_PI_2;

use glam::{Mat4, Vec3};
use postretro_level_loader::{CellLocatorChild, LevelWorld};

use super::oracle::{OracleWalk, oracle_from_camera};
use super::test_fixtures::{
    CameraPose, GridWorld, SplitMix64, axis_node, axis_rect, boxes_world, camera_view_proj,
    cyclic_rings, look_at_view_proj, open_lattice, portal, sample_cameras, stacked_slab_shaft,
};
use super::tests::{
    SWEEP_CAMERAS, SWEEP_SEED, WALK_STEP_LIMIT, fixtures, oracle, unclassified_misses, walk,
};
use super::{
    PortalWalkScratch, RegionWalkStats, ScreenRect, WalkView, WalkWorld, drain, walk_from_camera,
};

fn view_walk(
    world: &LevelWorld,
    blocked: &[bool],
    eye: Vec3,
    view_proj: Mat4,
    camera_cell: usize,
    scratch: &mut PortalWalkScratch,
) -> RegionWalkStats {
    let view = WalkView::camera(eye, view_proj);
    let input = WalkWorld {
        world,
        blocked_portals: blocked,
    };
    walk_from_camera(input, &view, camera_cell, u32::MAX, scratch)
}

fn view_oracle(
    world: &LevelWorld,
    blocked: &[bool],
    eye: Vec3,
    view_proj: Mat4,
    camera_cell: usize,
) -> OracleWalk {
    oracle_from_camera(
        world,
        blocked,
        &WalkView::camera(eye, view_proj),
        camera_cell,
    )
}

fn pose_walk(
    world: &LevelWorld,
    blocked: &[bool],
    pose: &CameraPose,
    step_limit: u32,
    scratch: &mut PortalWalkScratch,
) -> RegionWalkStats {
    let view = WalkView::camera(pose.eye, camera_view_proj(pose.eye, pose.yaw, pose.pitch));
    let input = WalkWorld {
        world,
        blocked_portals: blocked,
    };
    walk_from_camera(input, &view, pose.cell, step_limit, scratch)
}

fn sorted_reached(scratch: &PortalWalkScratch) -> Vec<u32> {
    let mut cells = scratch.reached().to_vec();
    cells.sort_unstable();
    cells
}

fn rect_contains(outer: &ScreenRect, inner: &ScreenRect) -> bool {
    outer.x0 <= inner.x0 && inner.x1 <= outer.x1 && outer.y0 <= inner.y0 && inner.y1 <= outer.y1
}

/// Two 4 m boxes along X, A = cell 0 and B = cell 1, joined by `opening` on
/// x = 4. On-plane points locate to B.
fn two_box_world(b_max_x: f32, opening: Vec<Vec3>) -> LevelWorld {
    boxes_world(
        &[
            (Vec3::ZERO, Vec3::splat(4.0)),
            (Vec3::new(4.0, 0.0, 0.0), Vec3::new(b_max_x, 4.0, 4.0)),
        ],
        vec![portal(opening, 0, 1)],
        (
            CellLocatorChild::Node(0),
            vec![axis_node(
                0,
                4.0,
                CellLocatorChild::Cell(1),
                CellLocatorChild::Cell(0),
            )],
        ),
    )
}

/// The window fixture: a 2 m square window in the middle of the shared wall.
fn window_world() -> LevelWorld {
    two_box_world(8.0, axis_rect(0, 4.0, [1.0, 1.0], [3.0, 3.0]))
}

/// `grid` with the chosen cells made solid after construction. Validation
/// rejects a portal into a solid cell, so only a mutated world exercises the
/// walk's solid-neighbour rejection.
fn with_solid_cells(mut grid: GridWorld, solid: impl Fn(usize) -> bool) -> GridWorld {
    for (index, cell) in grid.world.cells.iter_mut().enumerate() {
        if solid(index) {
            cell.is_solid = true;
        }
    }
    grid
}

/// A copy of `world` with each cell's portal list permuted. Validation
/// requires ascending refs, so the copy is built sorted and permuted after.
fn permuted_world(world: &LevelWorld, seed: u64) -> LevelWorld {
    let mut copy = LevelWorld::new_visibility_only(
        world.cells.clone(),
        world.cell_portal_refs.clone(),
        world.cell_locator_root,
        world.cell_locator_nodes.clone(),
        world.portals.clone(),
        world.has_portals,
    )
    .expect("copy stays valid");
    let mut rng = SplitMix64(seed);
    for cell in &world.cells {
        let start = cell.portal_ref_start as usize;
        let refs = &mut copy.cell_portal_refs[start..start + cell.portal_ref_count as usize];
        for i in (1..refs.len()).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            refs.swap(i, j);
        }
    }
    copy
}

/// Everything a walk leaves behind that a later walk could corrupt.
#[derive(Debug, PartialEq)]
struct WalkSnapshot {
    stats: RegionWalkStats,
    reached: Vec<u32>,
    rects: Vec<Option<ScreenRect>>,
    expansions: Vec<u32>,
}

fn snapshot(stats: RegionWalkStats, scratch: &PortalWalkScratch) -> WalkSnapshot {
    let reached = scratch.reached().to_vec();
    WalkSnapshot {
        stats,
        rects: reached.iter().map(|&c| scratch.rect(c as usize)).collect(),
        expansions: reached
            .iter()
            .map(|&c| scratch.expansions(c as usize))
            .collect(),
        reached,
    }
}

fn fresh_snapshot(world: &LevelWorld, blocked: &[bool], pose: &CameraPose) -> WalkSnapshot {
    let mut scratch = PortalWalkScratch::new();
    let stats = pose_walk(world, blocked, pose, u32::MAX, &mut scratch);
    snapshot(stats, &scratch)
}

fn reused_snapshot(
    world: &LevelWorld,
    blocked: &[bool],
    pose: &CameraPose,
    scratch: &mut PortalWalkScratch,
) -> WalkSnapshot {
    let stats = pose_walk(world, blocked, pose, u32::MAX, scratch);
    snapshot(stats, scratch)
}

#[test]
fn region_walk_includes_neighbour_through_portal_crossing_camera_plane() {
    // Row 5: half the portal lies behind the eye; clip in 3D before projecting.
    let grid = GridWorld::new(
        vec![0.0, 4.0, 8.0],
        vec![0.0, 4.0],
        vec![0.0, 4.0],
        |_, _, _| false,
    );
    let world = &grid.world;
    let eye = Vec3::new(3.0, 2.0, 2.0);
    let view_proj = look_at_view_proj(eye, eye + Vec3::Z);
    let camera_cell = world.locate_cell(eye);
    assert_eq!(camera_cell, 0);

    let mut scratch = PortalWalkScratch::new();
    view_walk(world, &[], eye, view_proj, camera_cell, &mut scratch);
    assert!(scratch.is_reached(1));
    let exact = view_oracle(world, &[], eye, view_proj, camera_cell);
    assert!(unclassified_misses(&exact, &scratch).is_empty());
}

#[test]
fn region_walk_sees_through_window_from_on_polygon_and_just_inside_wall() {
    // Row 6 guard: eye on the polygon (bypass) or 3 cm off it (clipped).
    let world = window_world();
    let cases = [
        (Vec3::new(4.0, 2.0, 2.0), -Vec3::X),
        (Vec3::new(4.0, 2.0, 2.0), Vec3::X),
        (Vec3::new(3.97, 2.0, 2.0), Vec3::X),
        (Vec3::new(4.03, 2.0, 2.0), -Vec3::X),
    ];
    let mut scratch = PortalWalkScratch::new();
    for (eye, look) in cases {
        let view_proj = look_at_view_proj(eye, eye + look);
        let located = world.locate_cell(eye);
        for camera_cell in [located, 1 - located] {
            // Off the plane, only the cell holding the eye is a camera cell.
            if eye.x != 4.0 && camera_cell != located {
                continue;
            }
            view_walk(&world, &[], eye, view_proj, camera_cell, &mut scratch);
            assert!(
                scratch.is_reached(1 - camera_cell),
                "eye {eye} looking {look} from cell {camera_cell}"
            );
            let exact = view_oracle(&world, &[], eye, view_proj, camera_cell);
            assert!(unclassified_misses(&exact, &scratch).is_empty());
        }
    }
}

#[test]
fn region_walk_bypasses_portal_just_outside_its_edge_in_both_directions() {
    // O8, O10: inside the bypass distance either camera cell sees the other.
    let world = window_world();
    let eye = Vec3::new(4.0, 2.0, 3.005);
    let views = [
        look_at_view_proj(eye, eye + Vec3::X),
        look_at_view_proj(eye, eye - Vec3::X),
        look_at_view_proj(eye, Vec3::new(4.0, 2.0, 2.0)),
    ];
    let mut scratch = PortalWalkScratch::new();
    for camera_cell in [0, 1] {
        for (v, &view_proj) in views.iter().enumerate() {
            let stats = view_walk(&world, &[], eye, view_proj, camera_cell, &mut scratch);
            assert!(stats.bypassed >= 1, "view {v} from cell {camera_cell}");
            assert!(
                scratch.is_reached(1 - camera_cell),
                "view {v} from cell {camera_cell}"
            );
        }
    }
}

#[test]
fn region_walk_clips_portal_normally_just_beyond_bypass_distance() {
    // O10: 2 cm off the edge, no bypass; the result still covers the oracle.
    let world = window_world();
    let eye = Vec3::new(4.0, 2.0, 3.02);
    let views = [
        look_at_view_proj(eye, eye + Vec3::X),
        look_at_view_proj(eye, eye - Vec3::X),
        look_at_view_proj(eye, Vec3::new(4.0, 2.0, 2.0)),
    ];
    let mut scratch = PortalWalkScratch::new();
    for camera_cell in [0, 1] {
        for (v, &view_proj) in views.iter().enumerate() {
            let stats = view_walk(&world, &[], eye, view_proj, camera_cell, &mut scratch);
            assert_eq!(stats.bypassed, 0, "view {v} from cell {camera_cell}");
            let exact = view_oracle(&world, &[], eye, view_proj, camera_cell);
            assert!(
                unclassified_misses(&exact, &scratch).is_empty(),
                "view {v} from cell {camera_cell}"
            );
        }
    }
}

#[test]
fn region_walk_on_shared_slab_plane_reaches_cells_above_below_and_beside() {
    // O10: eye on a plane shared by stacked-slab portals, either locator answer.
    let grid = stacked_slab_shaft(3, 24);
    let world = &grid.world;
    let centre = grid.centre(grid.cell_at(1, 10, 1));
    let eye = Vec3::new(centre.x, grid.ys[10], centre.z);
    let view_proj = camera_view_proj(eye, -FRAC_PI_2, 0.0);
    let expected = [
        grid.cell_at(1, 10, 1),
        grid.cell_at(1, 9, 1),
        grid.cell_at(2, 10, 1),
        grid.cell_at(2, 9, 1),
    ];
    let mut scratch = PortalWalkScratch::new();
    for camera_cell in [grid.cell_at(1, 10, 1), grid.cell_at(1, 9, 1)] {
        view_walk(world, &[], eye, view_proj, camera_cell, &mut scratch);
        for cell in expected {
            assert!(
                scratch.is_reached(cell),
                "cell {cell} from camera cell {camera_cell}"
            );
        }
        let exact = view_oracle(world, &[], eye, view_proj, camera_cell);
        assert!(unclassified_misses(&exact, &scratch).is_empty());
    }
}

#[test]
fn region_walk_classifies_misses_through_distant_portal_on_eye_plane() {
    // O36: one sign rule for the facing test and the oracle's backward class.
    let grid = cyclic_rings(5, 1);
    let world = &grid.world;
    let eye = Vec3::new(3.0 - 0.0005, 1.25, 7.5);
    let camera_cell = world.locate_cell(eye);
    assert_eq!(camera_cell, grid.cell_at(0, 0, 2));
    let view_proj = camera_view_proj(eye, 0.0, 0.0);

    let mut scratch = PortalWalkScratch::new();
    let stats = view_walk(world, &[], eye, view_proj, camera_cell, &mut scratch);
    let exact = view_oracle(world, &[], eye, view_proj, camera_cell);
    assert!(unclassified_misses(&exact, &scratch).is_empty());
    // The oracle crosses it clipped; the crossing is classified backward.
    let edge_on = grid.cell_at(1, 0, 0);
    assert!(exact.visible[edge_on] && !exact.clean[edge_on]);
    assert!(scratch.is_reached(grid.cell_at(0, 0, 0)));
    // Seen edge-on 4.5 m away: rejected normally, never bypassed.
    assert!(!scratch.is_reached(edge_on));
    assert_eq!(stats.bypassed, 0);
    assert!(stats.rejected_facing >= 1);
}

#[test]
fn region_walk_reaches_cell_past_two_portals_within_render_near_distance() {
    // O9: the slid near plane clips every cell's portals, not just the first.
    let grid = GridWorld::new(
        vec![0.0, 4.0, 8.0],
        vec![0.0, 4.0],
        vec![0.0, 4.0, 8.0],
        |_, _, _| false,
    );
    let world = &grid.world;
    let eye = Vec3::new(3.95, 2.0, 3.95);
    let camera_cell = world.locate_cell(eye);
    assert_eq!(camera_cell, grid.cell_at(0, 0, 0));
    let view_proj = look_at_view_proj(eye, Vec3::new(8.0, 2.0, 8.0));

    let mut scratch = PortalWalkScratch::new();
    let stats = view_walk(world, &[], eye, view_proj, camera_cell, &mut scratch);
    assert_eq!(stats.bypassed, 0);
    assert!(scratch.is_reached(grid.cell_at(1, 0, 1)));
    let exact = view_oracle(world, &[], eye, view_proj, camera_cell);
    assert!(unclassified_misses(&exact, &scratch).is_empty());
}

#[test]
fn region_walk_bypass_off_camera_cell_hands_on_source_rect_without_reexpansion() {
    // O23: the far cell gets no more than the near cell's rect; crossing back adds nothing.
    let world = boxes_world(
        &[
            (Vec3::ZERO, Vec3::splat(4.0)),
            (Vec3::new(4.0, 0.0, 0.0), Vec3::new(8.0, 2.0, 4.0)),
            (Vec3::new(4.0, 2.0, 0.0), Vec3::new(8.0, 4.0, 4.0)),
        ],
        vec![
            portal(axis_rect(0, 4.0, [0.5, 1.0], [1.0, 1.5]), 0, 1),
            portal(axis_rect(1, 2.0, [4.0, 0.0], [8.0, 4.0]), 2, 1),
        ],
        (
            CellLocatorChild::Node(0),
            vec![
                axis_node(0, 4.0, CellLocatorChild::Node(1), CellLocatorChild::Cell(0)),
                axis_node(1, 2.0, CellLocatorChild::Cell(2), CellLocatorChild::Cell(1)),
            ],
        ),
    );
    let (a, b, c) = (0, 1, 2);
    let eye = Vec3::new(3.995, 2.0, 2.0);
    assert_eq!(world.locate_cell(eye), a);
    let view_proj = look_at_view_proj(eye, Vec3::new(4.0, 0.75, 1.25));

    let mut scratch = PortalWalkScratch::new();
    let stats = view_walk(&world, &[], eye, view_proj, a, &mut scratch);
    assert!(stats.bypassed >= 1);
    let b_rect = scratch.rect(b).expect("B reached through the window");
    assert_ne!(b_rect, ScreenRect::FULL);
    let c_rect = scratch.rect(c).expect("C reached through the bypass");
    assert!(
        rect_contains(&b_rect, &c_rect),
        "{c_rect:?} within {b_rect:?}"
    );
    assert_eq!(scratch.expansions(b), 1);
    assert_eq!(scratch.expansions(c), 1);
    let exact = view_oracle(&world, &[], eye, view_proj, a);
    assert!(unclassified_misses(&exact, &scratch).is_empty());
}

#[test]
fn region_walk_reexpands_descendants_when_a_parent_rect_grows_late() {
    // O2: a cell first exposed by its parent's second or later expansion.
    let mut cases: Vec<(&str, GridWorld, u64)> = fixtures()
        .into_iter()
        .map(|(name, grid)| (name, grid, SWEEP_SEED))
        .collect();
    for seed in 1..=3 {
        cases.push(("open lattice", open_lattice(6), seed));
    }
    let mut found = 0;
    for (name, grid, seed) in &cases {
        let world = &grid.world;
        let mut scratch = PortalWalkScratch::new();
        for pose in sample_cameras(grid, SWEEP_CAMERAS, *seed) {
            walk(world, &pose, &mut scratch);
            let late = scratch.reached().iter().any(|&c| {
                matches!(
                    scratch.first_reached_by(c as usize),
                    Some((parent, k)) if parent != u32::MAX && k >= 2
                )
            });
            if !late {
                continue;
            }
            found += 1;
            let exact = oracle(world, &pose);
            let misses = unclassified_misses(&exact, &scratch);
            assert!(
                misses.is_empty(),
                "{name} seed {seed}: {misses:?} at {pose:?}"
            );
        }
    }
    eprintln!("[region_walk cases] late re-expansion poses found: {found}");
    assert!(found > 0, "no pose exposes a cell through a re-expansion");
}

#[test]
fn scratch_requeues_on_one_step_growth_and_not_on_contained_offer() {
    // O3: growth compares snapped values; one step re-queues, a no-op never does.
    let mut scratch = PortalWalkScratch::new();
    scratch.begin(2);
    let start = ScreenRect {
        x0: -10,
        x1: 10,
        y0: -10,
        y1: 10,
    };
    scratch.offer(0, &start, 1.0);
    assert_eq!(scratch.pop(), Some((0, start)));
    assert_eq!(scratch.pop(), None);

    let grown = ScreenRect { x1: 11, ..start };
    scratch.offer(0, &grown, 1.0);
    assert_eq!(scratch.pop(), Some((0, grown)));
    assert_eq!(scratch.expansions(0), 2);

    let inside = ScreenRect {
        x0: -5,
        x1: 5,
        y0: -5,
        y1: 5,
    };
    scratch.offer(0, &inside, 1.0);
    scratch.offer(0, &grown, 1.0);
    assert_eq!(scratch.pop(), None);
    assert_eq!(scratch.expansions(0), 2);
}

#[test]
fn scratch_expands_once_against_union_when_rect_grows_twice_while_queued() {
    // O22: growth while queued never re-queues; the pop sees the union.
    let mut scratch = PortalWalkScratch::new();
    scratch.begin(2);
    let first = ScreenRect {
        x0: -10,
        x1: 0,
        y0: -10,
        y1: 0,
    };
    let right = ScreenRect {
        x0: 0,
        x1: 20,
        y0: -5,
        y1: 0,
    };
    let top = ScreenRect {
        x0: -5,
        x1: 0,
        y0: 0,
        y1: 30,
    };
    scratch.offer(1, &first, 1.0);
    scratch.offer(1, &right, 1.0);
    scratch.offer(1, &top, 1.0);
    let union = ScreenRect {
        x0: -10,
        x1: 20,
        y0: -10,
        y1: 30,
    };
    assert_eq!(scratch.pop(), Some((1, union)));
    assert_eq!(scratch.pop(), None);
    assert_eq!(scratch.expansions(1), 1);
}

/// A at the origin looking −Z, B beyond z = −5, joined by `opening`.
fn tangent_world(opening: Vec<Vec3>) -> LevelWorld {
    boxes_world(
        &[
            (Vec3::new(-10.0, -10.0, -5.0), Vec3::new(10.0, 10.0, 1.0)),
            (Vec3::new(-10.0, -10.0, -10.0), Vec3::new(10.0, 10.0, -5.0)),
        ],
        vec![portal(opening, 0, 1)],
        (
            CellLocatorChild::Node(0),
            vec![axis_node(
                2,
                -5.0,
                CellLocatorChild::Cell(0),
                CellLocatorChild::Cell(1),
            )],
        ),
    )
}

#[test]
fn region_walk_includes_portal_tangent_to_rect_edge() {
    // A portal touching the source rect only along an edge is kept.
    let eye = Vec3::ZERO;
    let view_proj = camera_view_proj(eye, 0.0, 0.0);
    let view = WalkView::camera(eye, view_proj);
    let full = ScreenRect::FULL;
    let cases = [
        // NDC x from 0 to ~0.17 against a left-half rect.
        (
            axis_rect(2, -5.0, [0.0, -1.0], [1.0, 1.0]),
            ScreenRect { x1: 0, ..full },
        ),
        // NDC y from 0 up against a bottom-half rect.
        (
            axis_rect(2, -5.0, [-1.0, 0.0], [1.0, 1.0]),
            ScreenRect { y1: 0, ..full },
        ),
    ];
    for (opening, seed_rect) in cases {
        let world = tangent_world(opening);
        assert_eq!(world.locate_cell(eye), 0);
        let input = WalkWorld {
            world: &world,
            blocked_portals: &[],
        };
        let mut scratch = PortalWalkScratch::new();
        let mut stats = RegionWalkStats::default();
        scratch.begin(world.cell_count());
        scratch.offer(0, &seed_rect, 0.0);
        drain(input, &view, u32::MAX, &mut scratch, &mut stats);
        assert!(scratch.is_reached(1), "seed rect {seed_rect:?}");
        // The clip pad keeps the portal but never widens the region.
        let reached = scratch.rect(1).expect("B reached");
        assert!(rect_contains(&seed_rect, &reached), "{reached:?}");
    }
}

#[test]
fn region_walk_includes_portal_tangent_to_screen_edge() {
    // A portal starting exactly at NDC x = 1, the full rect's snapped edge.
    let eye = Vec3::ZERO;
    let view_proj = camera_view_proj(eye, 0.0, 0.0);
    // At depth 5, NDC x = 1 where x = 5 / P00.
    let edge = 5.0 / view_proj.col(0).x;
    let world = tangent_world(axis_rect(2, -5.0, [edge, -1.0], [edge + 1.0, 1.0]));
    let mut scratch = PortalWalkScratch::new();
    view_walk(&world, &[], eye, view_proj, 0, &mut scratch);
    assert!(scratch.is_reached(1));
}

#[test]
fn region_walk_hides_cell_behind_opening_whose_rect_misses_its_portal() {
    // Row 23 guard: C is in the frustum but outside the small window's cone.
    let world = boxes_world(
        &[
            (Vec3::new(0.0, -4.0, -4.0), Vec3::new(2.0, 4.0, 4.0)),
            (Vec3::new(2.0, -4.0, -4.0), Vec3::new(6.0, 4.0, 4.0)),
            (Vec3::new(6.0, -4.0, -4.0), Vec3::new(8.0, 4.0, 4.0)),
        ],
        vec![
            portal(axis_rect(0, 2.0, [-0.1, -0.1], [0.1, 0.1]), 0, 1),
            portal(axis_rect(0, 6.0, [-0.5, 2.0], [0.5, 3.0]), 1, 2),
        ],
        (
            CellLocatorChild::Node(0),
            vec![
                axis_node(0, 2.0, CellLocatorChild::Node(1), CellLocatorChild::Cell(0)),
                axis_node(0, 6.0, CellLocatorChild::Cell(2), CellLocatorChild::Cell(1)),
            ],
        ),
    );
    let eye = Vec3::new(0.5, 0.0, 0.0);
    assert_eq!(world.locate_cell(eye), 0);
    let view_proj = look_at_view_proj(eye, eye + Vec3::X);
    // The B–C portal's centre is on screen.
    let clip = view_proj * Vec3::new(6.0, 0.0, 2.5).extend(1.0);
    assert!(clip.w > 0.0 && (clip.x / clip.w).abs() < 1.0 && (clip.y / clip.w).abs() < 1.0);

    let mut scratch = PortalWalkScratch::new();
    view_walk(&world, &[], eye, view_proj, 0, &mut scratch);
    assert!(scratch.is_reached(1));
    assert!(!scratch.is_reached(2));
    let exact = view_oracle(&world, &[], eye, view_proj, 0);
    assert!(exact.visible[1] && !exact.visible[2]);
}

#[test]
fn region_walk_rejects_portal_wholly_behind_camera() {
    // Row 24 guard.
    let grid = GridWorld::new(
        vec![0.0, 4.0, 8.0, 12.0],
        vec![0.0, 4.0],
        vec![0.0, 4.0],
        |_, _, _| false,
    );
    let world = &grid.world;
    let eye = Vec3::new(6.0, 2.0, 2.0);
    assert_eq!(world.locate_cell(eye), 1);
    let view_proj = look_at_view_proj(eye, eye + Vec3::X);
    let mut scratch = PortalWalkScratch::new();
    view_walk(world, &[], eye, view_proj, 1, &mut scratch);
    assert!(!scratch.is_reached(0));
    assert!(scratch.is_reached(2));
    let exact = view_oracle(world, &[], eye, view_proj, 1);
    assert!(!exact.visible[0]);
}

#[test]
fn region_walk_keeps_closed_door_shut_with_camera_pressed_against_it() {
    // O37: blocked runs before the bypass, both directions, counted once.
    let world = two_box_world(6.0, axis_rect(0, 4.0, [0.0, 1.0], [3.0, 3.0]));
    let blocked = [true];
    let cases = [
        (Vec3::new(3.995, 2.0, 2.0), Vec3::X, 0, 1),
        (Vec3::new(4.005, 2.0, 2.0), -Vec3::X, 1, 0),
    ];
    let mut scratch = PortalWalkScratch::new();
    for (eye, look, camera_cell, closet) in cases {
        assert_eq!(world.locate_cell(eye), camera_cell);
        let view_proj = look_at_view_proj(eye, eye + look);
        let stats = view_walk(&world, &blocked, eye, view_proj, camera_cell, &mut scratch);
        assert!(!scratch.is_reached(closet), "from cell {camera_cell}");
        assert_eq!(stats.considered, 1);
        assert_eq!(stats.rejected_blocked, 1);
        assert_eq!(stats.bypassed, 0);
        let exact = view_oracle(&world, &blocked, eye, view_proj, camera_cell);
        assert!(!exact.visible[closet]);
    }
}

#[test]
fn region_walk_finishes_open_lattice_sweep_on_small_fixed_stack() {
    // Row 30: the walk is iterative.
    let grid = open_lattice(6);
    let poses = sample_cameras(&grid, SWEEP_CAMERAS, SWEEP_SEED);
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .stack_size(64 * 1024)
            .spawn_scoped(scope, || {
                let mut scratch = PortalWalkScratch::new();
                for pose in &poses {
                    let stats = pose_walk(&grid.world, &[], pose, WALK_STEP_LIMIT, &mut scratch);
                    assert!(!stats.step_limit_hit, "{pose:?}");
                }
            })
            .expect("spawn small-stack thread")
            .join()
            .expect("small-stack sweep completes");
    });
}

#[test]
fn region_walk_counts_every_outbound_portal_once_per_expansion() {
    // O11: counted before any rejection; outcomes partition the count.
    let mut cases = fixtures();
    cases.push((
        "lattice with solid cells",
        with_solid_cells(open_lattice(4), |c| c % 7 == 3),
    ));
    let mut totals = RegionWalkStats::default();
    for (name, grid) in &cases {
        let world = &grid.world;
        let unblocked = vec![false; world.portals.len()];
        let some_blocked: Vec<bool> = (0..world.portals.len()).map(|p| p % 5 == 0).collect();
        let mut scratch = PortalWalkScratch::new();
        for blocked in [&unblocked, &some_blocked] {
            for pose in sample_cameras(grid, SWEEP_CAMERAS, SWEEP_SEED) {
                let stats = pose_walk(world, blocked, &pose, u32::MAX, &mut scratch);
                let by_degree: u32 = scratch
                    .reached()
                    .iter()
                    .map(|&c| {
                        let c = c as usize;
                        scratch.expansions(c) * world.cell_portal_count(c) as u32
                    })
                    .sum();
                assert_eq!(stats.considered, by_degree, "{name} at {pose:?}");
                let outcomes = stats.accepted
                    + stats.rejected_blocked
                    + stats.rejected_solid
                    + stats.rejected_clipped
                    + stats.rejected_invalid
                    + stats.rejected_facing;
                assert_eq!(stats.considered, outcomes, "{name} at {pose:?}");
                totals.accepted += stats.accepted;
                totals.rejected_blocked += stats.rejected_blocked;
                totals.rejected_solid += stats.rejected_solid;
                totals.rejected_clipped += stats.rejected_clipped;
                totals.rejected_facing += stats.rejected_facing;
            }
        }
    }
    assert!(totals.accepted > 0);
    assert!(totals.rejected_blocked > 0);
    assert!(totals.rejected_solid > 0);
    assert!(totals.rejected_clipped > 0);
    assert!(totals.rejected_facing > 0);
}

#[test]
fn region_walk_completes_at_exact_cap_and_trips_one_below() {
    // O24: the cap is checked before each portal test.
    let grid = open_lattice(6);
    let world = &grid.world;
    let mut scratch = PortalWalkScratch::new();
    let pose = sample_cameras(&grid, SWEEP_CAMERAS, SWEEP_SEED)
        .into_iter()
        .find(|pose| pose_walk(world, &[], pose, u32::MAX, &mut scratch).considered >= 10)
        .expect("a pose with several portal tests");
    let uncapped = pose_walk(world, &[], &pose, u32::MAX, &mut scratch);
    let uncapped_set = sorted_reached(&scratch);
    let n = uncapped.considered;

    let at_cap = pose_walk(world, &[], &pose, n, &mut scratch);
    assert!(!at_cap.step_limit_hit);
    assert_eq!(at_cap.considered, n);
    assert_eq!(sorted_reached(&scratch), uncapped_set);

    let below = pose_walk(world, &[], &pose, n - 1, &mut scratch);
    assert!(below.step_limit_hit);
}

#[test]
fn region_walk_never_trips_at_cap_zero_from_cell_without_portals() {
    // O24: a walk that tests no portal never trips.
    let world = boxes_world(
        &[(Vec3::ZERO, Vec3::splat(4.0))],
        vec![],
        (CellLocatorChild::Cell(0), vec![]),
    );
    let eye = Vec3::splat(2.0);
    let view = WalkView::camera(eye, look_at_view_proj(eye, eye + Vec3::X));
    let input = WalkWorld {
        world: &world,
        blocked_portals: &[],
    };
    let mut scratch = PortalWalkScratch::new();
    let stats = walk_from_camera(input, &view, 0, 0, &mut scratch);
    assert!(!stats.step_limit_hit);
    assert_eq!(stats.considered, 0);
    assert_eq!(scratch.reached(), &[0]);
}

#[test]
fn region_walk_ignores_portal_list_order_over_sweep() {
    // Row 34 guard: ties break by cell id, never by portal order.
    for (name, grid) in fixtures() {
        let world = &grid.world;
        let poses = sample_cameras(&grid, SWEEP_CAMERAS, SWEEP_SEED);
        let mut scratch = PortalWalkScratch::new();
        let mut shuffled_scratch = PortalWalkScratch::new();
        for seed in [0xA11CE, 0xB0B] {
            let shuffled = permuted_world(world, seed);
            for pose in &poses {
                walk(world, pose, &mut scratch);
                walk(&shuffled, pose, &mut shuffled_scratch);
                assert_eq!(
                    sorted_reached(&scratch),
                    sorted_reached(&shuffled_scratch),
                    "{name} seed {seed:#x} at {pose:?}"
                );
            }
        }
    }
}

#[test]
fn walk_scratch_reuse_matches_fresh_state_across_poses_doors_and_worlds() {
    // O7, O25: no cell reads as reached from an earlier walk.
    let grid_a = open_lattice(4);
    let grid_b = open_lattice(6);
    let world_a = &grid_a.world;
    let world_b = &grid_b.world;
    assert!(world_b.cell_count() > world_a.cell_count());
    let poses = sample_cameras(&grid_a, 32, 7);
    let pose_1 = poses[0];
    let pose_2 = *poses
        .iter()
        .find(|p| p.cell != pose_1.cell)
        .expect("a second camera cell");
    let pose_b = sample_cameras(&grid_b, 1, 7)[0];
    let mut door = vec![false; world_a.portals.len()];
    door[world_a
        .cell_portal_index(pose_2.cell, 0)
        .expect("camera cell has a portal")] = true;

    let mut scratch = PortalWalkScratch::new();
    let first = reused_snapshot(world_a, &[], &pose_1, &mut scratch);
    assert_eq!(first, fresh_snapshot(world_a, &[], &pose_1));
    let open = reused_snapshot(world_a, &[], &pose_2, &mut scratch);
    assert_eq!(open, fresh_snapshot(world_a, &[], &pose_2));
    let closed = reused_snapshot(world_a, &door, &pose_2, &mut scratch);
    assert_eq!(closed, fresh_snapshot(world_a, &door, &pose_2));
    assert_ne!(open, closed, "the door toggle changes the walk");
    assert_eq!(
        reused_snapshot(world_b, &[], &pose_b, &mut scratch),
        fresh_snapshot(world_b, &[], &pose_b)
    );
    assert_eq!(reused_snapshot(world_a, &[], &pose_1, &mut scratch), first);
}

#[test]
fn walk_scratch_matches_fresh_state_across_generation_wrap() {
    // O25: the wrap clears stamps, so stale stamps never equal a new generation.
    let grid = open_lattice(6);
    let world = &grid.world;
    let poses = sample_cameras(&grid, SWEEP_CAMERAS, SWEEP_SEED);
    let fresh_count = |pose: &CameraPose| fresh_snapshot(world, &[], pose).reached.len();
    let wide = poses
        .iter()
        .max_by_key(|pose| fresh_count(pose))
        .copied()
        .expect("poses");
    let narrow = poses
        .iter()
        .min_by_key(|pose| fresh_count(pose))
        .copied()
        .expect("poses");
    assert!(fresh_count(&wide) > fresh_count(&narrow));

    let mut scratch = PortalWalkScratch::new();
    // Generation 1 stamps the wide set; after the wrap, generation 1 returns.
    reused_snapshot(world, &[], &wide, &mut scratch);
    scratch.set_generation_for_test(u32::MAX - 1);
    for pose in [wide, narrow, narrow] {
        assert_eq!(
            reused_snapshot(world, &[], &pose, &mut scratch),
            fresh_snapshot(world, &[], &pose)
        );
    }
}

#[test]
fn walk_scratch_per_thread_matches_single_thread_walks() {
    // O7: one scratch per thread; the world is shared read-only.
    let grid = open_lattice(6);
    let world = &grid.world;
    let poses = sample_cameras(&grid, 64, SWEEP_SEED);
    let walk_all = || {
        let mut scratch = PortalWalkScratch::new();
        poses
            .iter()
            .map(|pose| reused_snapshot(world, &[], pose, &mut scratch))
            .collect::<Vec<_>>()
    };
    let single = walk_all();
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4).map(|_| scope.spawn(walk_all)).collect();
        for handle in handles {
            assert_eq!(handle.join().expect("walk thread"), single);
        }
    });
}
