use super::*;
use postretro_level_loader::{CellData, CellLocatorChild, PortalData};
use postretro_test_log_capture::LogCapture;

fn cell(min: Vec3, max: Vec3, solid: bool) -> CellData {
    CellData {
        bounds_min: min,
        bounds_max: max,
        face_start: 0,
        face_count: u32::from(!solid),
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid: solid,
        is_exterior: false,
        is_drawable: !solid,
    }
}

fn world() -> LevelWorld {
    let mut cells = vec![
        cell(Vec3::new(-2.0, -3.0, 0.0), Vec3::new(2.0, -0.1, 3.0), true),
        cell(Vec3::new(-2.0, -0.1, -5.0), Vec3::new(2.0, 3.0, 0.0), false),
        cell(
            Vec3::new(-2.0, 0.0, -10.0),
            Vec3::new(2.0, 3.0, -5.0),
            false,
        ),
        cell(
            Vec3::new(-2.0, 0.0, -30.0),
            Vec3::new(2.0, 3.0, -25.0),
            false,
        ),
        cell(
            Vec3::new(-2.0, -5.0, -10.0),
            Vec3::new(2.0, -1.0, -5.0),
            false,
        ),
    ];
    let mut refs = Vec::new();
    for (cell, cell_refs) in
        cells
            .iter_mut()
            .zip([&[][..], &[0][..], &[0, 1][..], &[1][..], &[][..]])
    {
        cell.portal_ref_start = refs.len() as u32;
        cell.portal_ref_count = cell_refs.len() as u32;
        refs.extend_from_slice(cell_refs);
    }
    let portal = |z, front_cell, back_cell| PortalData {
        polygon: vec![
            Vec3::new(-2.0, 0.0, z),
            Vec3::new(2.0, 0.0, z),
            Vec3::new(2.0, 3.0, z),
            Vec3::new(-2.0, 3.0, z),
        ],
        front_cell,
        back_cell,
    };
    // The locator always returns solid cell 0, including at the mirror apex.
    LevelWorld::new_visibility_only(
        cells,
        refs,
        CellLocatorChild::Cell(0),
        Vec::new(),
        vec![portal(-5.0, 1, 2), portal(-25.0, 2, 3)],
        true,
    )
    .unwrap()
}

fn cull_vp(far: f32) -> Mat4 {
    let eye = Vec3::new(0.0, -2.0, 2.0);
    let view = glam::camera::rh::view::look_at_mat4(eye, Vec3::new(0.0, 0.0, -8.0), Vec3::Y);
    glam::camera::rh::proj::directx::perspective(90.0_f32.to_radians(), 1.5, 0.1, far) * view
}

fn run(
    world: &LevelWorld,
    seeds: &[u32],
    blocked: &[bool],
    step_limit: u32,
) -> MirrorVisibilityResult {
    determine_with_step_limit(
        Vec3::new(0.0, -2.0, 2.0),
        cull_vp(15.0),
        Vec4::Y,
        seeds,
        world,
        blocked,
        &mut Vec::new(),
        TimingGate::ON,
        step_limit,
    )
    .0
}

fn draw_ids(result: &MirrorVisibilityResult) -> &[u32] {
    let VisibleCells::Culled(ids) = &result.visible_cells else {
        panic!("mirror always returns explicit cells")
    };
    ids
}

#[test]
fn mirror_solid_apex_floods_from_water_seeds_without_solid_fallback_log() {
    let capture = LogCapture::start();
    let world = world();
    assert!(world.cell_is_solid(world.locate_cell(Vec3::new(0.0, -2.0, 2.0))));
    let result = run(&world, &[1], &[], 20_000);
    assert_eq!(result.path, MirrorVisibilityPath::Seeded);
    assert_eq!(draw_ids(&result), &[1, 2]);
    assert_eq!(result.reachable_cells, vec![1, 2]);
    assert!(result.considered > 0 && result.accepted > 0);
    assert!(result.cpu.value(VisibilityStage::MirrorWalk).is_some());
    assert!(result.cpu.value(VisibilityStage::PortalWalk).is_none());
    capture.assert_not_logged(log::Level::Warn, "SolidCellFallback");
}

#[test]
fn mirror_distance_far_plane_and_water_halfspace_bound_step_limit_fallback() {
    let result = run(&world(), &[1], &[], 0);
    assert_eq!(result.path, MirrorVisibilityPath::StepLimitFallback);
    assert_eq!(draw_ids(&result), &[1, 2]);
    assert!(
        !result.reachable_cells.contains(&3),
        "cell beyond D is excluded"
    );
    assert!(
        !result.reachable_cells.contains(&4),
        "cell beneath water is excluded"
    );
}

#[test]
fn mirror_empty_or_invalid_seeds_never_widen_to_every_cell() {
    for seeds in [&[][..], &[0][..], &[999][..], &[3][..]] {
        let result = run(&world(), seeds, &[], 20_000);
        assert_eq!(result.path, MirrorVisibilityPath::EmptySeeds);
        assert!(draw_ids(&result).is_empty());
        assert!(result.reachable_cells.is_empty());
    }
}

#[test]
fn mirror_seeded_walk_honors_blocked_portals_and_keeps_empty_lighting_cells() {
    let mut world = world();
    world.cells[2].face_count = 0;
    world.cells[2].is_drawable = false;
    let result = run(&world, &[1], &[], 20_000);
    assert_eq!(draw_ids(&result), &[1]);
    assert_eq!(result.reachable_cells, vec![1, 2]);
    let blocked = run(&world, &[1], &[true], 20_000);
    assert_eq!(blocked.reachable_cells, vec![1]);
}

#[test]
fn mirror_multiple_seeds_share_a_step_budget_and_no_portals_fallback_stays_bounded() {
    let mut world = world();
    let result = run(&world, &[1, 2], &[], 1);
    assert!(result.considered <= 1);
    assert_eq!(result.path, MirrorVisibilityPath::StepLimitFallback);
    world.has_portals = false;
    let result = run(&world, &[1], &[], 20_000);
    assert_eq!(result.path, MirrorVisibilityPath::NoPortalsFallback);
    assert_eq!(draw_ids(&result), &[1, 2]);
}

#[test]
fn mirror_walk_budget_zero_and_small_take_the_bounded_frustum_fallback() {
    let world = world();
    for budget in [0, 1] {
        let result = run(&world, &[1], &[], budget);
        assert_eq!(result.path, MirrorVisibilityPath::StepLimitFallback);
        assert_eq!(draw_ids(&result), &[1, 2], "budget {budget}");
        assert!(!result.reachable_cells.is_empty());
        assert!(!result.reachable_cells.contains(&3), "beyond D excluded");
        assert!(
            !result.reachable_cells.contains(&4),
            "beneath water excluded"
        );
        assert!(result.considered <= budget);
        assert!(result.cpu.value(VisibilityStage::MirrorWalk).is_some());
    }
    assert_eq!(
        run(&world, &[1], &[], 0).considered,
        0,
        "zero skips the walk"
    );
    // The public entry point's default budget is the unchanged main-walk cap.
    assert_eq!(DEFAULT_MIRROR_WALK_STEPS, portal_vis::MAX_PORTAL_WALK_STEPS);
    assert_eq!(DEFAULT_MIRROR_WALK_STEPS, 20_000);
    let (default, _) = determine_mirror_visible_cells(
        Vec3::new(0.0, -2.0, 2.0),
        cull_vp(15.0),
        Vec4::Y,
        &[1],
        &world,
        &[],
        &mut Vec::new(),
        TimingGate::ON,
        DEFAULT_MIRROR_WALK_STEPS,
    );
    assert_eq!(default.path, MirrorVisibilityPath::Seeded);
    assert_eq!(draw_ids(&default), &[1, 2]);
}
