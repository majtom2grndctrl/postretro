// Visibility determination tests: frustum culling, fallback paths and CPU stages.

use super::*;

// -- Frustum plane extraction tests --

/// Build a view-projection matrix that sees everything in front along -Z.
/// Camera at the given position, looking down -Z, with a wide FOV.
fn wide_view_proj(position: Vec3) -> Mat4 {
    let view = glam::camera::rh::view::look_at_mat4(position, position + Vec3::NEG_Z, Vec3::Y);
    let proj = glam::camera::rh::proj::directx::perspective(
        std::f32::consts::FRAC_PI_2, // 90-degree vertical FOV
        16.0 / 9.0,
        0.1,
        4096.0,
    );
    proj * view
}

#[test]
fn frustum_planes_are_normalized() {
    let vp = wide_view_proj(Vec3::ZERO);
    let frustum = extract_frustum_planes(vp);

    for (i, plane) in frustum.planes.iter().enumerate() {
        let len = plane.normal.length();
        assert!(
            (len - 1.0).abs() < 1e-5,
            "plane {i} normal not normalized: length = {len}"
        );
    }
}

#[test]
fn frustum_planes_count() {
    let vp = wide_view_proj(Vec3::ZERO);
    let frustum = extract_frustum_planes(vp);
    assert_eq!(frustum.planes.len(), 6, "should have exactly 6 planes");
}

#[test]
fn frustum_origin_is_inside() {
    // Camera at origin looking down -Z. The origin should be inside the frustum.
    let vp = wide_view_proj(Vec3::ZERO);
    let frustum = extract_frustum_planes(vp);

    // A point just in front of the camera (past the near plane) should be inside.
    let test_point = Vec3::new(0.0, 0.0, -1.0);
    let mut inside = true;
    for plane in &frustum.planes {
        if plane.normal.dot(test_point) + plane.dist < 0.0 {
            inside = false;
            break;
        }
    }
    assert!(
        inside,
        "point just in front of camera should be inside frustum"
    );
}

#[test]
fn point_behind_camera_is_outside() {
    // Camera at origin looking down -Z. A point behind (+Z) should be outside.
    let vp = wide_view_proj(Vec3::ZERO);
    let frustum = extract_frustum_planes(vp);

    let test_point = Vec3::new(0.0, 0.0, 10.0);
    let mut outside = false;
    for plane in &frustum.planes {
        if plane.normal.dot(test_point) + plane.dist < 0.0 {
            outside = true;
            break;
        }
    }
    assert!(outside, "point behind camera should be outside frustum");
}

// -- AABB-frustum tests --

#[test]
fn aabb_fully_inside_frustum_is_not_culled() {
    // Camera at origin looking down -Z. Box centered at (0, 0, -50).
    let vp = wide_view_proj(Vec3::ZERO);
    let frustum = extract_frustum_planes(vp);

    let mins = Vec3::new(-10.0, -10.0, -60.0);
    let maxs = Vec3::new(10.0, 10.0, -40.0);
    assert!(
        !is_aabb_outside_frustum(mins, maxs, &frustum),
        "box directly in front should not be culled"
    );
}

#[test]
fn aabb_fully_behind_camera_is_culled() {
    // Camera at origin looking down -Z. Box behind at (0, 0, +50).
    let vp = wide_view_proj(Vec3::ZERO);
    let frustum = extract_frustum_planes(vp);

    let mins = Vec3::new(-10.0, -10.0, 40.0);
    let maxs = Vec3::new(10.0, 10.0, 60.0);
    assert!(
        is_aabb_outside_frustum(mins, maxs, &frustum),
        "box behind camera should be culled"
    );
}

#[test]
fn aabb_far_left_is_culled() {
    // Camera at origin looking down -Z. Box far to the left.
    let vp = wide_view_proj(Vec3::ZERO);
    let frustum = extract_frustum_planes(vp);

    let mins = Vec3::new(-500.0, -10.0, -60.0);
    let maxs = Vec3::new(-490.0, 10.0, -40.0);
    assert!(
        is_aabb_outside_frustum(mins, maxs, &frustum),
        "box far to the left should be culled"
    );
}

#[test]
fn aabb_far_right_is_culled() {
    // Camera at origin looking down -Z. Box far to the right.
    let vp = wide_view_proj(Vec3::ZERO);
    let frustum = extract_frustum_planes(vp);

    let mins = Vec3::new(490.0, -10.0, -60.0);
    let maxs = Vec3::new(500.0, 10.0, -40.0);
    assert!(
        is_aabb_outside_frustum(mins, maxs, &frustum),
        "box far to the right should be culled"
    );
}

#[test]
fn aabb_far_above_is_culled() {
    // Camera at origin looking down -Z. Box far above.
    let vp = wide_view_proj(Vec3::ZERO);
    let frustum = extract_frustum_planes(vp);

    let mins = Vec3::new(-10.0, 490.0, -60.0);
    let maxs = Vec3::new(10.0, 500.0, -40.0);
    assert!(
        is_aabb_outside_frustum(mins, maxs, &frustum),
        "box far above should be culled"
    );
}

#[test]
fn aabb_far_below_is_culled() {
    // Camera at origin looking down -Z. Box far below.
    let vp = wide_view_proj(Vec3::ZERO);
    let frustum = extract_frustum_planes(vp);

    let mins = Vec3::new(-10.0, -500.0, -60.0);
    let maxs = Vec3::new(10.0, -490.0, -40.0);
    assert!(
        is_aabb_outside_frustum(mins, maxs, &frustum),
        "box far below should be culled"
    );
}

#[test]
fn aabb_beyond_far_plane_is_culled() {
    // Camera at origin looking down -Z. Box beyond the far plane (4096).
    let vp = wide_view_proj(Vec3::ZERO);
    let frustum = extract_frustum_planes(vp);

    let mins = Vec3::new(-10.0, -10.0, -5000.0);
    let maxs = Vec3::new(10.0, 10.0, -4500.0);
    assert!(
        is_aabb_outside_frustum(mins, maxs, &frustum),
        "box beyond far plane should be culled"
    );
}

#[test]
fn aabb_straddling_frustum_edge_is_not_culled() {
    // Camera at origin looking down -Z. Large box that straddles the left edge.
    let vp = wide_view_proj(Vec3::ZERO);
    let frustum = extract_frustum_planes(vp);

    // This box extends from inside to outside the left plane — conservative test keeps it.
    let mins = Vec3::new(-100.0, -10.0, -60.0);
    let maxs = Vec3::new(0.0, 10.0, -40.0);
    assert!(
        !is_aabb_outside_frustum(mins, maxs, &frustum),
        "box straddling frustum edge should not be culled (conservative)"
    );
}

#[test]
fn aabb_enclosing_camera_is_not_culled() {
    // Camera at origin, box encloses the camera entirely.
    let vp = wide_view_proj(Vec3::ZERO);
    let frustum = extract_frustum_planes(vp);

    let mins = Vec3::splat(-1000.0);
    let maxs = Vec3::splat(1000.0);
    assert!(
        !is_aabb_outside_frustum(mins, maxs, &frustum),
        "box enclosing the camera should not be culled"
    );
}

// -- PRL cell-based visibility tests --

use postretro_level_loader::{CellData, LevelWorld};

fn prl_cell(
    bounds_min: Vec3,
    bounds_max: Vec3,
    face_start: u32,
    face_count: u32,
    is_solid: bool,
    is_exterior: bool,
) -> CellData {
    CellData {
        bounds_min,
        bounds_max,
        face_start,
        face_count,
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid,
        is_exterior,
        is_drawable: !is_solid && !is_exterior && face_count > 0,
    }
}

/// Two-cell PRL world. The locator returns cell 0 for these tests.
fn two_cell_prl_world() -> LevelWorld {
    LevelWorld::new_visibility_only(
        vec![
            prl_cell(
                Vec3::new(0.0, -100.0, -100.0),
                Vec3::new(100.0, 100.0, 100.0),
                0,
                1,
                false,
                false,
            ),
            prl_cell(
                Vec3::new(-100.0, -100.0, -100.0),
                Vec3::new(0.0, 100.0, 100.0),
                1,
                1,
                false,
                false,
            ),
        ],
        vec![],
        postretro_level_loader::CellLocatorChild::Cell(0),
        vec![],
        vec![],
        false,
    )
    .expect("valid visibility-only test world")
}

#[test]
fn visible_cells_returns_camera_cell() {
    let world = two_cell_prl_world();
    let vp = wide_view_proj(Vec3::new(50.0, 0.0, 0.0));
    let mut scratch = PortalWalkScratch::new();
    let (result, _frustum) = determine_visible_cells(
        Vec3::new(50.0, 0.0, 0.0),
        vp,
        &world,
        &[],
        false,
        &mut scratch,
        TimingGate::OFF,
    );
    match result.visible_cells {
        VisibleCells::Culled(cells) => {
            assert!(cells.contains(&0), "camera cell 0 should be visible");
        }
        VisibleCells::DrawAll => panic!("expected Culled"),
    }
    assert_eq!(result.stats.total_faces, 2);
    assert!(matches!(
        result.stats.path,
        VisibilityPath::NoPortalsFallback
    ));
    // No-portals fallback does not populate the wider fog/light set.
    assert!(result.fog_reachable.is_empty());
}

#[test]
fn visible_cells_empty_world_draws_all() {
    let world = LevelWorld::new_visibility_only(
        vec![],
        vec![],
        postretro_level_loader::CellLocatorChild::Cell(0),
        vec![],
        vec![],
        false,
    )
    .expect("valid empty visibility-only test world");
    let vp = wide_view_proj(Vec3::ZERO);
    let mut scratch = PortalWalkScratch::new();
    let (result, _frustum) = determine_visible_cells(
        Vec3::ZERO,
        vp,
        &world,
        &[],
        false,
        &mut scratch,
        TimingGate::OFF,
    );
    assert!(matches!(result.visible_cells, VisibleCells::DrawAll));
    assert_eq!(result.stats.total_faces, 0);
    assert!(matches!(
        result.stats.path,
        VisibilityPath::EmptyWorldFallback
    ));
    assert!(result.fog_reachable.is_empty());
}

#[test]
fn visible_cells_frustum_culling_removes_offscreen_cell() {
    let world = two_cell_prl_world();
    let position = Vec3::new(50.0, 0.0, 0.0);
    // Looking down +X, away from cell 1.
    let view = glam::camera::rh::view::look_at_mat4(position, position + Vec3::X, Vec3::Y);
    let proj =
        glam::camera::rh::proj::directx::perspective(std::f32::consts::FRAC_PI_4, 1.0, 0.1, 4096.0);
    let vp = proj * view;

    let mut scratch = PortalWalkScratch::new();
    let (result, _frustum) = determine_visible_cells(
        position,
        vp,
        &world,
        &[],
        false,
        &mut scratch,
        TimingGate::OFF,
    );
    match result.visible_cells {
        VisibleCells::Culled(cells) => {
            assert_eq!(cells.len(), 1, "should cull cell behind camera");
            assert_eq!(cells[0], 0);
        }
        VisibleCells::DrawAll => panic!("expected Culled"),
    }
    assert!(matches!(
        result.stats.path,
        VisibilityPath::NoPortalsFallback
    ));
}

#[test]
fn portal_stats_walk_reach_counts_drawable_cells_not_faces() {
    let mut world = two_cell_prl_world();
    world.cells[0].face_count = 2;
    world.cells[1].face_count = 3;
    let result = CellVisResult {
        fog_reachable: Vec::new(),
        camera_cell: 0,
        total_faces: 5,
        path: CellVisPath::Portal,
        frustum: extract_frustum_planes(wide_view_proj(Vec3::ZERO)),
    };

    let stats = build_visibility_stats(&result, &[0, 1], &world, StageFrame::default());

    assert_eq!(stats.drawn_faces, 5);
    assert_eq!(stats.walk_reach(), Some(2));
}

#[test]
fn visible_cells_solid_cell_fallback() {
    let mut world = two_cell_prl_world();
    // Mark cell 0 as solid so the camera at X=50 lands in it.
    world.cells[0].is_solid = true;
    world.cells[0].is_drawable = false;
    let vp = wide_view_proj(Vec3::new(50.0, 0.0, 0.0));
    let mut scratch = PortalWalkScratch::new();
    let (result, _frustum) = determine_visible_cells(
        Vec3::new(50.0, 0.0, 0.0),
        vp,
        &world,
        &[],
        false,
        &mut scratch,
        TimingGate::OFF,
    );
    // Solid fallback draws all drawable cells.
    match result.visible_cells {
        VisibleCells::Culled(cells) => {
            assert!(cells.contains(&1), "non-solid cell 1 should be visible");
            assert!(!cells.contains(&0), "solid cell 0 should not appear");
        }
        VisibleCells::DrawAll => panic!("expected Culled"),
    }
    assert!(matches!(
        result.stats.path,
        VisibilityPath::SolidCellFallback
    ));
    assert!(result.fog_reachable.is_empty());
}

#[test]
fn visible_cells_exterior_camera_fallback() {
    let mut world = two_cell_prl_world();
    // Make cell 0 exterior.
    world.cells[0].face_count = 0;
    world.cells[0].is_drawable = false;
    world.cells[0].is_exterior = true;
    let vp = wide_view_proj(Vec3::new(50.0, 0.0, 0.0));
    let mut scratch = PortalWalkScratch::new();
    let (result, _frustum) = determine_visible_cells(
        Vec3::new(50.0, 0.0, 0.0),
        vp,
        &world,
        &[],
        false,
        &mut scratch,
        TimingGate::OFF,
    );
    match result.visible_cells {
        VisibleCells::Culled(cells) => {
            // Cell 1 still has faces; cell 0 is excluded (exterior).
            assert!(cells.contains(&1));
        }
        VisibleCells::DrawAll => panic!("expected Culled"),
    }
    assert!(matches!(
        result.stats.path,
        VisibilityPath::ExteriorCellFallback
    ));
    assert!(result.fog_reachable.is_empty());
}

// -- Portal step-limit fallback --

/// Camera at the origin looking down -Z through a portal chain, with
/// in-frustum cells the walk cannot reach:
///
/// - 0: camera cell, drawable, portal 0 at z=-10 to cell 1.
/// - 1: drawable, portal 1 at z=-30 to cell 2.
/// - 2: empty (`face_count == 0`), portal-reachable.
/// - 3: drawable, in frustum, no portals (occluded from the walk).
/// - 4: solid, in frustum.
/// - 5: exterior, in frustum.
/// - 6: drawable, behind the camera.
fn portal_chain_world() -> LevelWorld {
    let square_at_z = |z: f32| {
        vec![
            Vec3::new(-10.0, -10.0, z),
            Vec3::new(10.0, -10.0, z),
            Vec3::new(10.0, 10.0, z),
            Vec3::new(-10.0, 10.0, z),
        ]
    };
    let mut cells = vec![
        prl_cell(
            Vec3::new(-10.0, -10.0, -10.0),
            Vec3::new(10.0, 10.0, 10.0),
            0,
            1,
            false,
            false,
        ),
        prl_cell(
            Vec3::new(-10.0, -10.0, -30.0),
            Vec3::new(10.0, 10.0, -10.0),
            1,
            1,
            false,
            false,
        ),
        prl_cell(
            Vec3::new(-10.0, -10.0, -50.0),
            Vec3::new(10.0, 10.0, -30.0),
            2,
            0,
            false,
            false,
        ),
        prl_cell(
            Vec3::new(20.0, -10.0, -50.0),
            Vec3::new(40.0, 10.0, -30.0),
            2,
            1,
            false,
            false,
        ),
        prl_cell(
            Vec3::new(-40.0, -10.0, -50.0),
            Vec3::new(-20.0, 10.0, -30.0),
            3,
            0,
            true,
            false,
        ),
        prl_cell(
            Vec3::new(-40.0, -10.0, -30.0),
            Vec3::new(-20.0, 10.0, -10.0),
            3,
            0,
            false,
            true,
        ),
        prl_cell(
            Vec3::new(-10.0, -10.0, 20.0),
            Vec3::new(10.0, 10.0, 40.0),
            3,
            1,
            false,
            false,
        ),
    ];
    // Portal refs: cell 0 -> [0], cell 1 -> [0, 1], cell 2 -> [1].
    cells[0].portal_ref_start = 0;
    cells[0].portal_ref_count = 1;
    cells[1].portal_ref_start = 1;
    cells[1].portal_ref_count = 2;
    cells[2].portal_ref_start = 3;
    cells[2].portal_ref_count = 1;
    LevelWorld::new_visibility_only(
        cells,
        vec![0, 0, 1, 1],
        postretro_level_loader::CellLocatorChild::Cell(0),
        vec![],
        vec![
            postretro_level_loader::PortalData {
                polygon: square_at_z(-10.0),
                front_cell: 0,
                back_cell: 1,
            },
            postretro_level_loader::PortalData {
                polygon: square_at_z(-30.0),
                front_cell: 1,
                back_cell: 2,
            },
        ],
        true,
    )
    .expect("valid portal-chain test world")
}

fn culled(cells: &VisibleCells) -> &[u32] {
    match cells {
        VisibleCells::Culled(cells) => cells,
        VisibleCells::DrawAll => panic!("expected Culled"),
    }
}

// Regression: stress-warren-hallway-inspection tripped the portal step
// limit and returned an empty fog_reachable, which fog, shadow-light
// eligibility, and the SH compose gate all read as "every cell".
#[test]
fn step_limit_fallback_fills_fog_reachable_with_frustum_culled_non_solid_cells() {
    let world = portal_chain_world();
    let eye = Vec3::ZERO;
    let mut scratch = PortalWalkScratch::new();
    let (result, _frustum) = determine_visible_cells_with_step_limit(
        eye,
        wide_view_proj(eye),
        &world,
        &[],
        false,
        &mut scratch,
        0,
        TimingGate::OFF,
    );

    assert!(matches!(
        result.stats.path,
        VisibilityPath::PortalStepLimitFallback { .. }
    ));
    // Drawing keeps the existing frustum-culled drawable set, occluded
    // cell 3 included.
    assert_eq!(culled(&result.visible_cells), &[0, 1, 3]);
    // Fog reach is the same set without the face-count filter: empty cell
    // 2 joins; solid 4, exterior 5, and behind-camera 6 stay out.
    assert_eq!(result.fog_reachable, vec![0, 1, 2, 3]);
}

#[test]
fn step_limit_fallback_fog_reachable_keeps_camera_cell_when_frustum_set_is_empty() {
    let world = portal_chain_world();
    // The locator always answers cell 0. Looking down +X from well outside
    // cell 0's AABB, no cell passes the frustum.
    let eye = Vec3::new(100.0, 0.0, 0.0);
    let view = glam::camera::rh::view::look_at_mat4(eye, eye + Vec3::X, Vec3::Y);
    let proj =
        glam::camera::rh::proj::directx::perspective(std::f32::consts::FRAC_PI_4, 1.0, 0.1, 4096.0);
    let mut scratch = PortalWalkScratch::new();
    let (result, _frustum) = determine_visible_cells_with_step_limit(
        eye,
        proj * view,
        &world,
        &[],
        false,
        &mut scratch,
        0,
        TimingGate::OFF,
    );

    assert!(matches!(
        result.stats.path,
        VisibilityPath::PortalStepLimitFallback { .. }
    ));
    assert!(culled(&result.visible_cells).is_empty());
    // Never the empty "no portal isolation" sentinel.
    assert_eq!(result.fog_reachable, vec![0]);
}

#[test]
fn portal_walk_within_step_limit_keeps_exact_visible_and_fog_sets() {
    let world = portal_chain_world();
    let eye = Vec3::ZERO;
    let mut scratch = PortalWalkScratch::new();
    let (result, _frustum) = determine_visible_cells(
        eye,
        wide_view_proj(eye),
        &world,
        &[],
        false,
        &mut scratch,
        TimingGate::OFF,
    );

    assert!(matches!(
        result.stats.path,
        VisibilityPath::PrlPortal { walk_reach: 2 }
    ));
    // Exact portal sets: occluded cell 3 is absent from both.
    assert_eq!(culled(&result.visible_cells), &[0, 1]);
    assert_eq!(result.fog_reachable, vec![0, 1, 2]);
}

#[test]
fn step_limit_fallback_sets_are_supersets_of_exact_portal_sets() {
    let world = portal_chain_world();
    let eye = Vec3::ZERO;
    let vp = wide_view_proj(eye);
    let mut scratch = PortalWalkScratch::new();
    let (exact, _) =
        determine_visible_cells(eye, vp, &world, &[], false, &mut scratch, TimingGate::OFF);
    let exact_visible = culled(&exact.visible_cells).to_vec();
    let (fallback, _) = determine_visible_cells_with_step_limit(
        eye,
        vp,
        &world,
        &[],
        false,
        &mut scratch,
        0,
        TimingGate::OFF,
    );

    let fallback_visible = culled(&fallback.visible_cells);
    for cell in &exact_visible {
        assert!(
            fallback_visible.contains(cell),
            "fallback draw set dropped visible cell {cell}"
        );
    }
    for cell in &exact.fog_reachable {
        assert!(
            fallback.fog_reachable.contains(cell),
            "fallback fog reach dropped reachable cell {cell}"
        );
    }
}

// Regression: the fallback fog-reachable set used the unslid camera
// frustum, whose near cutoff excludes an AABB that lies entirely between
// the camera and the near plane. The exact portal walk slides its near
// plane to the camera (`Frustum::slide_near_plane_to`) and can reach such
// a cell, so the fallback silently dropped it — breaking the "superset
// of the exact portal set" contract for a cell beside or just behind the
// camera plane.
#[test]
fn step_limit_fallback_fog_reachable_includes_camera_adjacent_near_slab_cell() {
    let camera_cell = prl_cell(
        Vec3::new(-10.0, -10.0, -10.0),
        Vec3::new(10.0, 10.0, 10.0),
        0,
        1,
        false,
        false,
    );
    // Lies entirely in the near slab: z is between the camera (z=0) and
    // the unslid frustum's near plane. `extract_frustum_planes` combines
    // rows for a -1..1 depth convention, but `wide_view_proj` projects
    // with wgpu's 0..1 depth, so the extracted near plane lands at
    // z ≈ -0.05 rather than the configured 0.1-unit near clip. The
    // unslid frustum classifies this AABB as fully outside; sliding the
    // near plane to the camera apex (z=0) brings it inside. `face_count`
    // 1 makes the cell drawable, so it also stands in for the drawing
    // set's "unslid frustum" half of the contract below.
    let mut near_slab_cell = prl_cell(
        Vec3::new(-0.01, -0.01, -0.04),
        Vec3::new(0.01, 0.01, -0.02),
        1,
        1,
        false,
        false,
    );
    near_slab_cell.portal_ref_start = 1;
    near_slab_cell.portal_ref_count = 1;

    let mut cells = vec![camera_cell, near_slab_cell];
    cells[0].portal_ref_start = 0;
    cells[0].portal_ref_count = 1;

    let world = LevelWorld::new_visibility_only(
        cells,
        vec![0, 0],
        postretro_level_loader::CellLocatorChild::Cell(0),
        vec![],
        vec![postretro_level_loader::PortalData {
            polygon: vec![
                Vec3::new(-0.01, -0.01, -0.02),
                Vec3::new(0.01, -0.01, -0.02),
                Vec3::new(0.01, 0.01, -0.02),
                Vec3::new(-0.01, 0.01, -0.02),
            ],
            front_cell: 0,
            back_cell: 1,
        }],
        true,
    )
    .expect("valid near-slab test world");

    let eye = Vec3::ZERO;
    let vp = wide_view_proj(eye);
    let mut scratch = PortalWalkScratch::new();

    // Ground truth: the exact walk with a large step limit.
    let (exact, _) =
        determine_visible_cells(eye, vp, &world, &[], false, &mut scratch, TimingGate::OFF);
    assert!(
        matches!(exact.stats.path, VisibilityPath::PrlPortal { .. }),
        "expected the exact portal walk to run without hitting the step limit"
    );
    assert!(
        exact.fog_reachable.contains(&1),
        "exact portal walk should reach the near-slab cell beside the camera"
    );

    // Force the step-limit fallback and check its fog-reachable set still
    // covers what the exact walk reached.
    let (fallback, _) = determine_visible_cells_with_step_limit(
        eye,
        vp,
        &world,
        &[],
        false,
        &mut scratch,
        0,
        TimingGate::OFF,
    );
    assert!(matches!(
        fallback.stats.path,
        VisibilityPath::PortalStepLimitFallback { .. }
    ));
    for cell in &exact.fog_reachable {
        assert!(
            fallback.fog_reachable.contains(cell),
            "fallback fog reach dropped near-slab reachable cell {cell}"
        );
    }

    // Pins the other half of the contract: the drawing set keeps the
    // unslid frustum, so the near-slab cell — reachable for fog — must
    // not be drawn. Only the slid fog frustum reaches into the near slab.
    assert!(
        !culled(&fallback.visible_cells).contains(&1),
        "fallback drawing set should not draw the near-slab cell; \
         only fog reach slides its near plane to the camera"
    );
}

// --- CPU stage timing ---

fn cpu(result: &VisibilityResult, stage: VisibilityStage) -> Option<u64> {
    result.stats.cpu.value(stage)
}

#[test]
fn timing_on_and_off_produce_identical_visibility() {
    // Behavior neutrality (P-gate): the gate reaches visibility as a value.
    let world = portal_chain_world();
    for (eye, limit) in [
        (Vec3::ZERO, MAX_PORTAL_WALK_STEPS),
        (Vec3::ZERO, 0),
        (Vec3::new(100.0, 0.0, 0.0), MAX_PORTAL_WALK_STEPS),
    ] {
        let vp = wide_view_proj(eye);
        let run = |timing| {
            let mut scratch = PortalWalkScratch::new();
            let (result, _) = determine_visible_cells_with_step_limit(
                eye,
                vp,
                &world,
                &[],
                false,
                &mut scratch,
                limit,
                timing,
            );
            (
                format!("{:?}", result.visible_cells),
                result.fog_reachable,
                format!("{:?}", result.stats.path),
            )
        };
        assert_eq!(
            run(TimingGate::ON),
            run(TimingGate::OFF),
            "eye {eye}, limit {limit}"
        );
    }
}

#[test]
fn portal_frame_records_walk_time_and_every_counter() {
    let world = portal_chain_world();
    let eye = Vec3::ZERO;
    let mut scratch = PortalWalkScratch::new();
    let (result, _) = determine_visible_cells(
        eye,
        wide_view_proj(eye),
        &world,
        &[],
        false,
        &mut scratch,
        TimingGate::ON,
    );
    assert!(matches!(
        result.stats.path,
        VisibilityPath::PrlPortal { .. }
    ));
    assert!(cpu(&result, VisibilityStage::PortalWalk).is_some());
    assert!(cpu(&result, VisibilityStage::Considered).unwrap() >= 2);
    assert!(cpu(&result, VisibilityStage::Accepted).unwrap() >= 1);
    for stage in [
        VisibilityStage::RejectedBlocked,
        VisibilityStage::RejectedSolid,
        VisibilityStage::RejectedClipped,
        VisibilityStage::RejectedInvalid,
    ] {
        assert!(cpu(&result, stage).is_some(), "{stage:?} present");
    }
    assert_eq!(cpu(&result, VisibilityStage::StepLimit), None);
    assert_eq!(cpu(&result, VisibilityStage::PortalFallback), None);
}

#[test]
fn step_limit_frame_is_a_walk_frame_not_a_fallback_frame() {
    // P-steplimit
    let world = portal_chain_world();
    let eye = Vec3::ZERO;
    let mut scratch = PortalWalkScratch::new();
    let (result, _) = determine_visible_cells_with_step_limit(
        eye,
        wide_view_proj(eye),
        &world,
        &[],
        false,
        &mut scratch,
        0,
        TimingGate::ON,
    );
    assert!(matches!(
        result.stats.path,
        VisibilityPath::PortalStepLimitFallback { .. }
    ));
    assert!(cpu(&result, VisibilityStage::PortalWalk).is_some());
    assert!(cpu(&result, VisibilityStage::Considered).is_some());
    assert_eq!(cpu(&result, VisibilityStage::StepLimit), Some(1));
    assert_eq!(cpu(&result, VisibilityStage::PortalFallback), None);
}

#[test]
fn walk_that_considers_no_portal_is_present_with_zero_counters() {
    // P-zero-walk: a portal-path world whose camera cell has no portals.
    let mut world = portal_chain_world();
    world.cells[0].portal_ref_count = 0;
    let eye = Vec3::ZERO;
    let mut scratch = PortalWalkScratch::new();
    let (result, _) = determine_visible_cells(
        eye,
        wide_view_proj(eye),
        &world,
        &[],
        false,
        &mut scratch,
        TimingGate::ON,
    );
    assert!(matches!(
        result.stats.path,
        VisibilityPath::PrlPortal { .. }
    ));
    assert!(cpu(&result, VisibilityStage::PortalWalk).is_some());
    assert_eq!(cpu(&result, VisibilityStage::Considered), Some(0));
    assert_eq!(cpu(&result, VisibilityStage::Accepted), Some(0));
}

#[test]
fn fallback_frame_records_only_the_fallback_marker() {
    let world = two_cell_prl_world();
    let eye = Vec3::new(50.0, 0.0, 0.0);
    let mut scratch = PortalWalkScratch::new();
    let (result, _) = determine_visible_cells(
        eye,
        wide_view_proj(eye),
        &world,
        &[],
        false,
        &mut scratch,
        TimingGate::ON,
    );
    assert!(matches!(
        result.stats.path,
        VisibilityPath::NoPortalsFallback
    ));
    assert_eq!(cpu(&result, VisibilityStage::PortalFallback), Some(1));
    assert_eq!(cpu(&result, VisibilityStage::PortalWalk), None);
    assert_eq!(cpu(&result, VisibilityStage::Considered), None);
}

#[test]
fn timing_off_records_no_stage() {
    let world = portal_chain_world();
    let eye = Vec3::ZERO;
    let mut scratch = PortalWalkScratch::new();
    let (result, _) = determine_visible_cells(
        eye,
        wide_view_proj(eye),
        &world,
        &[],
        false,
        &mut scratch,
        TimingGate::OFF,
    );
    use postretro_stage_timing::StageSet;
    assert!(
        VisibilityStage::ALL
            .iter()
            .all(|&stage| cpu(&result, stage).is_none())
    );
}
