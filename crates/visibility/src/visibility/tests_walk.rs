// determine_visible_cells over the bounded-region walk: set shape, scratch reuse, fallback reach.

use glam::{Mat4, Vec3};
use postretro_level_loader::{CellLocatorChild, LevelWorld};

use super::frustum::is_aabb_outside_planes_by;
use super::*;
use crate::portal_vis::CLIP_EPSILON;
use crate::region_walk::test_fixtures::{
    CameraPose, axis_node, axis_rect, boxes_world, camera_view_proj, cyclic_rings,
    look_at_view_proj, portal, sample_cameras, stacked_slab_shaft,
};
use crate::region_walk::tests::{SWEEP_CAMERAS, SWEEP_SEED, fixtures};
use crate::region_walk::walk_from_camera;

fn run(
    world: &LevelWorld,
    eye: Vec3,
    view_proj: Mat4,
    blocked: &[bool],
    step_limit: u32,
    scratch: &mut PortalWalkScratch,
) -> VisibilityResult {
    determine_visible_cells_with_step_limit(
        eye,
        view_proj,
        world,
        blocked,
        false,
        scratch,
        step_limit,
        TimingGate::OFF,
    )
    .0
}

fn culled(cells: &VisibleCells) -> &[u32] {
    match cells {
        VisibleCells::Culled(cells) => cells,
        VisibleCells::DrawAll => panic!("expected Culled"),
    }
}

/// Everything a frame reads from one call.
#[derive(Debug, PartialEq)]
struct Outcome {
    cells: Vec<u32>,
    fog_reachable: Vec<u32>,
    path: String,
}

fn outcome(result: &VisibilityResult) -> Outcome {
    Outcome {
        cells: culled(&result.visible_cells).to_vec(),
        fog_reachable: result.fog_reachable.clone(),
        path: format!("{:?}", result.stats.path),
    }
}

/// The uncapped walk's reach from the cell the locator gives `eye`.
fn uncapped_reach(world: &LevelWorld, eye: Vec3, view_proj: Mat4) -> Vec<u32> {
    let mut scratch = PortalWalkScratch::new();
    let input = WalkWorld {
        world,
        blocked_portals: &[],
    };
    let view = WalkView::camera(eye, view_proj);
    let stats = walk_from_camera(input, &view, world.locate_cell(eye), u32::MAX, &mut scratch);
    assert!(!stats.step_limit_hit);
    let mut reached = scratch.reached().to_vec();
    reached.sort_unstable();
    reached
}

#[test]
fn walk_sets_keep_their_shape_at_every_sweep_camera() {
    // Row 20 guard (O4, O12).
    let mut walk_frames = 0;
    for (name, grid) in fixtures() {
        let world = &grid.world;
        let mut scratch = PortalWalkScratch::new();
        for pose in sample_cameras(&grid, SWEEP_CAMERAS, SWEEP_SEED) {
            let view_proj = camera_view_proj(pose.eye, pose.yaw, pose.pitch);
            let result = run(
                world,
                pose.eye,
                view_proj,
                &[],
                MAX_PORTAL_WALK_STEPS,
                &mut scratch,
            );
            let at = format!("{name} at {pose:?}");
            let camera_cell = result.stats.camera_cell;
            assert_eq!(camera_cell as usize, pose.cell, "{at}");
            let cells = culled(&result.visible_cells);
            for &cell in cells {
                assert!((cell as usize) < world.cell_count(), "{at}");
                assert!(world.cells[cell as usize].is_drawable, "{at}");
            }
            for &cell in &result.fog_reachable {
                assert!((cell as usize) < world.cell_count(), "{at}");
                assert!(!world.cells[cell as usize].is_solid, "{at}");
            }
            if world.cells[pose.cell].is_solid {
                // A wall-hugging eye can locate into the solid core.
                assert!(
                    matches!(result.stats.path, VisibilityPath::SolidCellFallback),
                    "{at}"
                );
            } else {
                assert!(
                    matches!(result.stats.path, VisibilityPath::PrlPortal { .. }),
                    "{at}: {:?}",
                    result.stats.path
                );
                walk_frames += 1;
                assert!(result.fog_reachable.contains(&camera_cell), "{at}");
                let drawable_fog: Vec<u32> = result
                    .fog_reachable
                    .iter()
                    .copied()
                    .filter(|&c| world.cells[c as usize].is_drawable)
                    .collect();
                assert_eq!(cells, drawable_fog.as_slice(), "{at}");
            }
            let VisibleCells::Culled(cells) = result.visible_cells else {
                unreachable!();
            };
            scratch.recycle_cells(cells);
        }
    }
    assert!(walk_frames > 0);
}

#[test]
fn back_to_back_calls_on_one_scratch_match_fresh_scratch_calls() {
    // Row 21 (O7, O25): door toggle, camera cell change, a larger world.
    let small = cyclic_rings(5, 4);
    let large = stacked_slab_shaft(3, 24);
    assert!(large.world.cell_count() > small.world.cell_count());
    let poses = sample_cameras(&small, 32, 11);
    let open_pose = |p: &&CameraPose| !small.world.cells[p.cell].is_solid;
    let sees_out = |p: &&CameraPose| {
        let view_proj = camera_view_proj(p.eye, p.yaw, p.pitch);
        uncapped_reach(&small.world, p.eye, view_proj).len() > 1
    };
    let pose_1 = *poses
        .iter()
        .filter(open_pose)
        .find(sees_out)
        .expect("an open camera that sees past its cell");
    let pose_2 = *poses
        .iter()
        .filter(open_pose)
        .find(|p| p.cell != pose_1.cell)
        .expect("a second camera cell");
    let pose_large = sample_cameras(&large, 1, 11)[0];
    // Doors on every portal of pose 1's cell: closing them changes its sets.
    let mut door = vec![false; small.world.portals.len()];
    for i in 0..small.world.cell_portal_count(pose_1.cell) {
        door[small
            .world
            .cell_portal_index(pose_1.cell, i)
            .expect("valid portal ref")] = true;
    }

    let steps: [(&LevelWorld, &CameraPose, &[bool]); 6] = [
        (&small.world, &pose_1, &[]),
        (&small.world, &pose_1, &door),
        (&small.world, &pose_1, &[]),
        (&small.world, &pose_2, &[]),
        (&large.world, &pose_large, &[]),
        (&small.world, &pose_1, &door),
    ];
    let mut shared = PortalWalkScratch::new();
    let mut outcomes = Vec::new();
    for (world, pose, blocked) in steps {
        let view_proj = camera_view_proj(pose.eye, pose.yaw, pose.pitch);
        let reused = run(
            world,
            pose.eye,
            view_proj,
            blocked,
            MAX_PORTAL_WALK_STEPS,
            &mut shared,
        );
        let fresh = run(
            world,
            pose.eye,
            view_proj,
            blocked,
            MAX_PORTAL_WALK_STEPS,
            &mut PortalWalkScratch::new(),
        );
        assert!(matches!(
            reused.stats.path,
            VisibilityPath::PrlPortal { .. }
        ));
        assert_eq!(outcome(&reused), outcome(&fresh), "{pose:?}");
        outcomes.push(outcome(&reused));
        let VisibleCells::Culled(cells) = reused.visible_cells else {
            unreachable!();
        };
        shared.recycle_cells(cells);
    }
    assert_ne!(outcomes[0], outcomes[1], "the door toggle changes the sets");
    assert_eq!(outcomes[0], outcomes[2]);
}

/// Two 4 m boxes along X, A = cell 0 and B = cell 1, joined by `opening` on
/// x = 4. B spans `b_max_z` deep. On-plane points locate to B.
fn side_by_side(b_max_z: f32, opening: Vec<Vec3>) -> LevelWorld {
    boxes_world(
        &[
            (Vec3::ZERO, Vec3::splat(4.0)),
            (Vec3::new(4.0, 0.0, 0.0), Vec3::new(8.0, 4.0, b_max_z)),
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

/// Force a trip and check the fallback's fog reach covers the uncapped walk.
fn assert_fallback_covers_uncapped_walk(case: &str, world: &LevelWorld, eye: Vec3, target: Vec3) {
    let view_proj = look_at_view_proj(eye, target);
    let reach = uncapped_reach(world, eye, view_proj);
    assert!(reach.contains(&1), "{case}: the walk reaches B");
    let result = run(world, eye, view_proj, &[], 0, &mut PortalWalkScratch::new());
    assert!(
        matches!(
            result.stats.path,
            VisibilityPath::PortalStepLimitFallback { .. }
        ),
        "{case}"
    );
    for cell in &reach {
        assert!(
            result.fog_reachable.contains(cell),
            "{case}: fallback fog reach dropped cell {cell}"
        );
    }
    // The draw set keeps the camera frustum: B, behind or beside the eye,
    // stays undrawn.
    assert!(!culled(&result.visible_cells).contains(&1), "{case}");
}

/// Whether B's box passes the fallback's plane test alone, the walk's
/// outermost clip region, without the distance keep.
fn clip_region_keeps_b(world: &LevelWorld, eye: Vec3, target: Vec3) -> bool {
    let view = WalkView::camera(eye, look_at_view_proj(eye, target));
    let mut scratch = PortalWalkScratch::new();
    let planes = region_walk::screen_clip_planes(&view, &mut scratch);
    let b = &world.cells[1];
    !is_aabb_outside_planes_by(b.bounds_min, b.bounds_max, planes, CLIP_EPSILON)
}

#[test]
fn step_limit_fallback_fog_reach_covers_a_bypassed_neighbour_behind_or_beside_the_eye() {
    // Row 37 (O6, O26, O30). Each eye is within the bypass distance of the
    // portal, so the walk hands B the camera cell's rect unclipped.

    // Beside the window, just past its edge, looking away from it.
    let window = side_by_side(4.0, axis_rect(0, 4.0, [1.0, 1.0], [3.0, 3.0]));
    let eye = Vec3::new(3.995, 2.0, 3.005);
    assert_fallback_covers_uncapped_walk("beside, looking away", &window, eye, eye - Vec3::X);

    // In front of a full-face portal, looking away: B lies wholly behind the
    // eye, past the slid near plane.
    let face = side_by_side(4.0, axis_rect(0, 4.0, [0.0, 0.0], [4.0, 4.0]));
    let eye = Vec3::new(3.995, 2.0, 2.0);
    assert!(!clip_region_keeps_b(&face, eye, eye - Vec3::X));
    assert_fallback_covers_uncapped_walk("behind the eye", &face, eye, eye - Vec3::X);

    // Looking along the portal plane: B ends 1 mm ahead of the eye, beside
    // it and outside the view's left edge. The near plane passes B; the left
    // side plane drops it, so the clip region alone, or one widened only at
    // the near plane, fails here. Only the distance keep holds B.
    let shallow = side_by_side(2.0, axis_rect(0, 4.0, [0.0, 0.0], [4.0, 2.0]));
    let eye = Vec3::new(3.995, 2.0, 1.999);
    assert!(!clip_region_keeps_b(&shallow, eye, eye + Vec3::Z));
    assert_fallback_covers_uncapped_walk("beside the eye", &shallow, eye, eye + Vec3::Z);
}

#[test]
fn step_limit_fallback_fog_reach_keeps_a_cell_reached_through_the_screen_edge_pad() {
    // Row 37. A's portal lies wholly in the clip pad's sliver just past the
    // right screen edge (NDC 1 to 1 + half a snap step). The walk keeps it,
    // so it reaches B, a thin box behind it lying wholly outside the true
    // right plane. The unpadded camera frustum would drop B from fog reach.
    let eye = Vec3::ZERO;
    let target = Vec3::NEG_Z;
    let view_proj = look_at_view_proj(eye, target);
    // At depth 5, NDC x = `ndc` where x = ndc * 5 / P00.
    let at_ndc = |ndc: f32| ndc * 5.0 / view_proj.col(0).x;
    let (x_lo, x_hi) = (at_ndc(1.001), at_ndc(1.0018));
    let world = boxes_world(
        &[
            (Vec3::new(-10.0, -10.0, -5.0), Vec3::new(10.0, 10.0, 1.0)),
            (Vec3::new(x_lo, -1.0, -5.002), Vec3::new(x_hi, 1.0, -5.0)),
        ],
        vec![portal(axis_rect(2, -5.0, [x_lo, -1.0], [x_hi, 1.0]), 0, 1)],
        (
            CellLocatorChild::Node(0),
            vec![axis_node(
                2,
                -5.0,
                CellLocatorChild::Cell(0),
                CellLocatorChild::Cell(1),
            )],
        ),
    );
    assert_eq!(world.locate_cell(eye), 0);
    let b = &world.cells[1];
    let frustum = extract_frustum_planes(view_proj);
    assert!(is_aabb_outside_frustum(
        b.bounds_min,
        b.bounds_max,
        &frustum
    ));
    assert!(clip_region_keeps_b(&world, eye, target));
    assert_fallback_covers_uncapped_walk("screen-edge pad", &world, eye, target);
}

#[test]
fn step_limit_fallback_fog_reach_covers_uncapped_walk_over_sweep() {
    // Row 37 over the superset sweep: a forced trip at every walk camera.
    let mut trips = 0;
    for (name, grid) in fixtures() {
        let world = &grid.world;
        let mut scratch = PortalWalkScratch::new();
        for pose in sample_cameras(&grid, SWEEP_CAMERAS, SWEEP_SEED) {
            if world.cells[pose.cell].is_solid {
                continue;
            }
            let view_proj = camera_view_proj(pose.eye, pose.yaw, pose.pitch);
            let reach = uncapped_reach(world, pose.eye, view_proj);
            let result = run(world, pose.eye, view_proj, &[], 0, &mut scratch);
            assert!(
                matches!(
                    result.stats.path,
                    VisibilityPath::PortalStepLimitFallback { .. }
                ),
                "{name} at {pose:?}"
            );
            trips += 1;
            for &cell in &reach {
                let data = &world.cells[cell as usize];
                if data.is_solid || data.is_exterior {
                    continue;
                }
                assert!(
                    result.fog_reachable.binary_search(&cell).is_ok(),
                    "{name} at {pose:?}: fallback fog reach dropped cell {cell}"
                );
            }
            let VisibleCells::Culled(cells) = result.visible_cells else {
                unreachable!();
            };
            scratch.recycle_cells(cells);
        }
    }
    assert!(trips > 0);
}
