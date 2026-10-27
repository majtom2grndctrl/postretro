// Mirror visibility tests: seeded walk sets, the camera-bounded trip set, the shared cap.

use super::*;
use crate::visibility::determine_visible_cells;
use postretro_level_loader::{CellData, CellLocatorChild, PortalData};
use postretro_stage_timing::TimingGate;
use postretro_test_log_capture::LogCapture;

/// The mirrored eye. It lies in solid cell 0, as a mirror apex below a
/// floor does.
const EYE: Vec3 = Vec3::new(0.0, -2.0, 2.0);
/// The cull far plane: cell 3 lies beyond it.
const FAR: f32 = 15.0;

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

/// Water at y = 0. Cell 0: solid, holds the eye. Cell 1 straddles the water;
/// a portal at z = -5 leads to cell 2 above it, and one at z = -25 to cell 3,
/// past the far plane. Cell 4 lies wholly beneath the water, with no portal.
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
    // The locator answers solid cell 0 everywhere, the mirror eye included.
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

fn cull_view_proj() -> Mat4 {
    let view = glam::camera::rh::view::look_at_mat4(EYE, Vec3::new(0.0, 0.0, -8.0), Vec3::Y);
    glam::camera::rh::proj::directx::perspective(90.0_f32.to_radians(), 1.5, 0.1, FAR) * view
}

fn mirror_view() -> MirrorView {
    MirrorView {
        eye: EYE,
        view_proj: cull_view_proj(),
        near_plane: FrustumPlane {
            normal: Vec3::Y,
            dist: 0.0,
        },
    }
}

/// Every non-solid cell, as a camera that sees the whole room would draw it.
fn camera_sees_all() -> VisibleCells {
    VisibleCells::Culled(vec![1, 2, 3, 4])
}

struct Run {
    out: MirrorVisibility,
    cpu: StageFrame<VisibilityStage>,
}

fn run(
    world: &LevelWorld,
    seeds: &[usize],
    camera_visible: &VisibleCells,
    blocked: &[bool],
    step_cap: Option<u32>,
) -> Run {
    let cpu = StageFrame::new(TimingGate::ON);
    let mut out = MirrorVisibility::default();
    let mut scratch = PortalWalkScratch::new();
    match step_cap {
        None => determine_mirror_visible_cells(
            &mirror_view(),
            seeds,
            camera_visible,
            world,
            blocked,
            &mut scratch,
            &cpu,
            &mut out,
        ),
        Some(cap) => determine_mirror_visible_cells_capped(
            &mirror_view(),
            seeds,
            camera_visible,
            world,
            blocked,
            cap,
            &mut scratch,
            &cpu,
            &mut out,
        ),
    }
    Run { out, cpu }
}

#[test]
fn mirror_solid_apex_floods_from_water_seeds_without_solid_fallback_log() {
    let capture = LogCapture::start();
    let world = world();
    assert!(world.cell_is_solid(world.locate_cell(EYE)));
    let Run { out, cpu } = run(&world, &[1], &camera_sees_all(), &[], None);
    assert_eq!(out.path, MirrorVisibilityPath::Walked);
    assert_eq!(out.visible_cells, [1, 2]);
    assert_eq!(out.reach_cells, [1, 2]);
    assert!(out.considered > 0 && out.accepted > 0);
    assert!(cpu.value(VisibilityStage::MirrorWalk).is_some());
    assert!(cpu.value(VisibilityStage::PortalWalk).is_none());
    capture.assert_not_logged(log::Level::Warn, "SolidCellFallback");
}

#[test]
fn mirror_empty_or_invalid_seeds_never_widen_to_every_cell() {
    // Empty, solid, out of range, beyond the far plane, beneath the water.
    let unusable: [&[usize]; 5] = [&[], &[0], &[999], &[3], &[4]];
    let mut no_portals = world();
    no_portals.has_portals = false;
    for world in [world(), no_portals] {
        for seeds in unusable {
            for camera in [camera_sees_all(), VisibleCells::DrawAll] {
                let Run { out, cpu } = run(&world, seeds, &camera, &[], None);
                assert_eq!(out.path, MirrorVisibilityPath::NoSeeds, "seeds {seeds:?}");
                assert!(out.visible_cells.is_empty(), "seeds {seeds:?}");
                assert!(out.reach_cells.is_empty(), "seeds {seeds:?}");
                assert!(cpu.value(VisibilityStage::MirrorWalk).is_none());
            }
        }
    }
}

#[test]
fn mirror_seeded_walk_honors_blocked_portals_and_keeps_empty_lighting_cells() {
    let mut world = world();
    world.cells[2].face_count = 0;
    world.cells[2].is_drawable = false;
    let open = run(&world, &[1], &camera_sees_all(), &[], None).out;
    assert_eq!(open.visible_cells, [1]);
    assert_eq!(open.reach_cells, vec![1, 2]);
    let blocked = run(&world, &[1], &camera_sees_all(), &[true], None).out;
    assert_eq!(blocked.path, MirrorVisibilityPath::Walked);
    assert_eq!(blocked.reach_cells, vec![1]);
}

// A trip draws camera visible ∩ mirror frustum.
#[test]
fn mirror_trip_draws_only_camera_visible_cells_inside_the_far_plane_and_water_halfspace() {
    let world = world();
    let tripped = run(&world, &[1], &camera_sees_all(), &[], Some(0)).out;
    assert_eq!(
        tripped.path,
        MirrorVisibilityPath::Tripped { considered: 0 }
    );
    assert_eq!(tripped.visible_cells, [1, 2]);
    assert!(
        !tripped.reach_cells.contains(&3),
        "cell beyond D is excluded"
    );
    assert!(
        !tripped.reach_cells.contains(&4),
        "cell beneath water is excluded"
    );
    assert_eq!(tripped.reach_cells, tripped.visible_cells);

    // Cell 2 is in the mirror frustum but not in the camera's visible set:
    // the trip never widens to it.
    let camera = VisibleCells::Culled(vec![1, 3, 4]);
    let narrow = run(&world, &[1], &camera, &[], Some(0)).out;
    assert_eq!(narrow.visible_cells, [1]);
    assert_eq!(narrow.reach_cells, [1]);

    // A camera on its empty-world `DrawAll` reads as every drawable cell.
    let all = run(&world, &[1], &VisibleCells::DrawAll, &[], Some(0)).out;
    assert_eq!(all.visible_cells, [1, 2]);
}

// Seeds share one step cap, and a world with no portals takes the
// camera-bounded set.
#[test]
fn mirror_seeds_share_one_step_cap_and_no_portal_worlds_take_the_camera_bounded_set() {
    let mut world = world();
    let shared = run(&world, &[1, 2], &camera_sees_all(), &[], Some(1)).out;
    assert_eq!(shared.path, MirrorVisibilityPath::Tripped { considered: 1 });
    assert_eq!(shared.considered, 1);

    world.has_portals = false;
    for (camera, expected) in [
        (camera_sees_all(), &[1, 2][..]),
        (VisibleCells::Culled(vec![1, 3, 4]), &[1][..]),
    ] {
        let Run { out, cpu } = run(&world, &[1], &camera, &[], None);
        assert_eq!(out.path, MirrorVisibilityPath::NoPortals);
        assert_eq!(out.visible_cells, expected);
        assert_eq!(out.reach_cells, expected);
        assert_eq!(out.considered, 0, "no walk runs without portals");
        assert!(cpu.value(VisibilityStage::MirrorWalk).is_none());
    }
}

// The mirror has no budget of its own, only the shared cap.
#[test]
fn mirror_forced_trips_take_the_camera_bounded_set_and_the_default_is_the_shared_cap() {
    let world = world();
    for cap in [0, 1] {
        let Run { out, cpu } = run(&world, &[1], &camera_sees_all(), &[], Some(cap));
        assert_eq!(
            out.path,
            MirrorVisibilityPath::Tripped { considered: cap },
            "cap {cap}"
        );
        assert_eq!(out.visible_cells, [1, 2], "cap {cap}");
        assert!(!out.reach_cells.contains(&3), "beyond D excluded");
        assert!(!out.reach_cells.contains(&4), "beneath water excluded");
        assert!(out.considered <= cap);
        assert!(cpu.value(VisibilityStage::MirrorWalk).is_some());
    }

    assert_eq!(MAX_PORTAL_WALK_STEPS, 5_000);
    let default = run(&world, &[1], &camera_sees_all(), &[], None).out;
    let capped = run(
        &world,
        &[1],
        &camera_sees_all(),
        &[],
        Some(MAX_PORTAL_WALK_STEPS),
    )
    .out;
    assert_eq!(default.path, MirrorVisibilityPath::Walked);
    assert_eq!(default.visible_cells, [1, 2]);
    assert_eq!(capped.path, default.path);
    assert_eq!(capped.visible_cells, default.visible_cells);
    assert_eq!(capped.considered, default.considered);
}

#[test]
fn mirror_forced_trip_logs_no_solid_cell_fallback() {
    let capture = LogCapture::start();
    let world = world();
    let tripped = run(&world, &[1], &camera_sees_all(), &[], Some(0)).out;
    assert!(matches!(tripped.path, MirrorVisibilityPath::Tripped { .. }));
    capture.assert_not_logged(log::Level::Warn, "SolidCellFallback");

    // The camera entry at the same solid eye does log it: the capture sees
    // the line when it is emitted.
    let mut camera_scratch = PortalWalkScratch::new();
    let _ = determine_visible_cells(
        EYE,
        cull_view_proj(),
        &world,
        &[],
        false,
        &mut camera_scratch,
        TimingGate::OFF,
    );
    capture.assert_logged_once(log::Level::Warn, "SolidCellFallback");
}

#[test]
fn mirror_visibility_ignores_seed_order_and_reuses_its_buffers() {
    let world = world();
    let camera = camera_sees_all();
    let base = run(&world, &[1, 2], &camera, &[], None).out;
    let reordered = run(&world, &[2, 1, 1], &camera, &[], None).out;
    assert_eq!(reordered.visible_cells, base.visible_cells);
    assert_eq!(reordered.reach_cells, base.reach_cells);
    assert_eq!(reordered.considered, base.considered);

    let cpu = StageFrame::new(TimingGate::OFF);
    let mut out = MirrorVisibility::default();
    let mut scratch = PortalWalkScratch::new();
    let mut frame = |out: &mut MirrorVisibility| {
        determine_mirror_visible_cells(
            &mirror_view(),
            &[1, 2],
            &camera,
            &world,
            &[],
            &mut scratch,
            &cpu,
            out,
        )
    };
    frame(&mut out);
    let buffers = (out.visible_cells.as_ptr(), out.reach_cells.as_ptr());
    frame(&mut out);
    assert_eq!(
        (out.visible_cells.as_ptr(), out.reach_cells.as_ptr()),
        buffers,
        "a repeated frame reuses the caller's buffers"
    );
    assert_eq!(out.visible_cells, base.visible_cells);
}
