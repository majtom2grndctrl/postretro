// The frame's mirror step: plane switches and tier drops read this frame's selection and tier.
// See: context/lib/rendering_pipeline.md §7.3a · context/lib/testing_guide.md

use glam::Vec3;
use postretro_render_cpu::fluid_surface::fog::FLUID_FOG_BAND;
use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::planar_reflection::{aabb_in_planes, planar_preset};
use postretro_visibility::{MirrorVisibilityPath, VisibilityStage, VisibleCells};

use crate::App;
use crate::camera::RenderCamera;
use crate::render_preparation::planar_test_fixtures::{
    MIRROR_ROOMS, WATER_ROOM, WaterRect, camera, camera_at, cells, installed, main_view_from,
    prepare_from, world, world_with_waters,
};
use crate::render_preparation::{PlanarRenderPreparation, VisibleRenderPreparation};
use crate::startup::lifecycle::tests::test_app;

const PLANAR: FluidReflectionQuality = FluidReflectionQuality::VeryHigh;
/// The water room's whole floor.
const ROOM_WATER: WaterRect = ([-4.0, -4.0], [4.0, 4.0]);
const PLANE_A: f32 = 0.0;
const PLANE_B: f32 = -0.9;

/// Plane A at the water line and plane B below it, both over the room.
fn two_planes() -> postretro_level_loader::LevelWorld {
    world_with_waters(
        WATER_ROOM,
        &[(PLANE_B, &[ROOM_WATER]), (PLANE_A, &[ROOM_WATER])],
    )
}

/// Frame N−1's pose, which selects A (nearer, larger on screen).
fn before_switch() -> RenderCamera {
    camera()
}

/// Frame N's pose: the eye inside A's fog band, so A cannot hold and B is
/// selected that frame.
fn switch_frame() -> RenderCamera {
    camera_at(Vec3::new(0.0, FLUID_FOG_BAND * 0.5, 3.0))
}

/// A mirror that held A on frame N−1.
fn holding_a(world: &postretro_level_loader::LevelWorld) -> PlanarRenderPreparation {
    let mut mirror = installed(world);
    let before = before_switch();
    let main = main_view_from(world, &before);
    prepare_from(&mut mirror, world, &before, &main, PLANAR);
    assert_eq!(mirror.held_selection().map(|plane| plane.h), Some(PLANE_A));
    assert_eq!(mirror.camera().map(|mirror| mirror.h), Some(PLANE_A));
    mirror
}

// AC 29 (O4, O5), app half: on the frame selection moves from plane A to
// plane B, the mirror camera the renderer receives (its plane, which routes
// B's draws to this frame's planar layer and skips them in SSR while A's go
// to SSR), its crop and its walk all read frame N's selection: they equal a
// mirror that never held A. When that frame's walk trips, the trip set comes
// from B's cropped frustum and the camera's visible cells, so it adds no
// reads.
#[test]
fn planar_switch_frame_routes_new_plane_to_this_mirror() {
    let world = two_planes();
    let pose = switch_frame();
    let main = main_view_from(&world, &pose);

    let mut switched = holding_a(&world);
    prepare_from(&mut switched, &world, &pose, &main, PLANAR);
    let mut fresh = installed(&world);
    prepare_from(&mut fresh, &world, &pose, &main, PLANAR);
    assert_eq!(
        switched.held_selection().map(|plane| plane.h),
        Some(PLANE_B)
    );
    let mirror = switched
        .camera()
        .expect("B's mirror draws on the switch frame");
    assert_eq!(mirror.h, PLANE_B, "routing reads frame N's plane");
    assert_eq!(
        Some(mirror),
        fresh.camera(),
        "the clip plane, raster and crop are B's alone"
    );
    assert_eq!(switched.path(), Some(MirrorVisibilityPath::Walked));
    assert_eq!(switched.reach(), fresh.reach(), "the walk is B's");
    assert_eq!(cells(switched.visible()), cells(fresh.visible()));

    // The same switch on a frame whose mirror walk trips.
    let mut tripped = holding_a(&world);
    tripped.set_step_cap(Some(0));
    prepare_from(&mut tripped, &world, &pose, &main, PLANAR);
    assert!(matches!(
        tripped.path(),
        Some(MirrorVisibilityPath::Tripped { .. })
    ));
    let mirror = tripped.camera().expect("a tripped mirror still draws");
    assert_eq!(mirror.h, PLANE_B);
    assert_eq!(Some(mirror), fresh.camera(), "O5: B's cropped frustum");
    let planes = mirror.cull_planes();
    assert!(!cells(tripped.visible()).is_empty());
    for &cell in cells(tripped.visible()) {
        assert!(cells(&main.visible_cells).contains(&cell));
        let data = &world.cells[cell as usize];
        assert!(aabb_in_planes(data.bounds_min, data.bounds_max, &planes));
    }
    assert!(std::ptr::eq(
        tripped.sh_visible_cells(&main.visible_cells),
        &main.visible_cells
    ));
    assert_eq!(
        cells(tripped.drawn_cells(&main.visible_cells)),
        cells(&main.visible_cells),
        "no new reads"
    );
}

fn app_with(world: postretro_level_loader::LevelWorld) -> App {
    let mut app = test_app();
    app.planar_render.install(&world, &[0]);
    app.level = Some(world);
    app
}

/// One play frame's camera walk and mirror step at `quality`'s preset.
fn frame(app: &mut App, quality: FluidReflectionQuality) -> VisibleRenderPreparation {
    let pose = camera();
    let world = app.level.as_ref().unwrap();
    let mut main = VisibleRenderPreparation::for_level(
        world,
        pose.eye_position,
        pose.view_projection,
        &app.blocked_portals,
        false,
        &mut app.portal_walk_scratch,
        postretro_visibility::TimingGate::ON,
    );
    app.prepare_planar_mirror_at(&pose, &mut main, planar_preset(quality));
    main
}

// AC 31 and 32 (O8), app half: on the frame the tier drops below very high,
// the mirror step reads the dropped tier before selection, so no plane is
// selected, no mirror walk runs, and the mirror adds no lightmap, SH or
// light-ranking demand, though the frame before walked it. Raising the tier
// again selects afresh.
#[test]
fn planar_tier_drop_frame_walks_no_mirror_and_adds_no_demand() {
    let mut app = app_with(world(WATER_ROOM));
    let planar = frame(&mut app, PLANAR);
    assert!(app.planar_render.camera().is_some());
    assert!(
        planar
            .stats
            .cpu
            .value(VisibilityStage::MirrorWalk)
            .is_some()
    );
    // The mirror's light reach comes tagged apart from the main view's (Task 8).
    assert!(
        !planar.mirror_light_cell_aabbs.is_empty(),
        "the mirror lights its rooms"
    );

    for quality in [
        FluidReflectionQuality::High,
        FluidReflectionQuality::Medium,
        FluidReflectionQuality::Low,
        FluidReflectionQuality::Off,
    ] {
        let dropped = frame(&mut app, quality);
        let mirror = &app.planar_render;
        assert!(mirror.camera().is_none(), "{quality:?}");
        assert!(mirror.held_selection().is_none(), "{quality:?}");
        assert!(
            dropped
                .stats
                .cpu
                .value(VisibilityStage::MirrorWalk)
                .is_none(),
            "{quality:?}: no mirror walk on the drop frame"
        );
        assert!(std::ptr::eq(
            mirror.drawn_cells(&dropped.visible_cells),
            &dropped.visible_cells
        ));
        assert!(std::ptr::eq(
            mirror.sh_visible_cells(&dropped.visible_cells),
            &dropped.visible_cells
        ));
        assert!(mirror.reach().is_empty());
        assert!(matches!(mirror.visible(), VisibleCells::Culled(cells) if cells.is_empty()));
        for room in MIRROR_ROOMS {
            assert!(
                !dropped.light_reachable_cell_mask[room as usize],
                "{quality:?}: no light ranking reaches room {room}"
            );
        }
        assert!(dropped.mirror_light_cell_aabbs.is_empty(), "{quality:?}");
    }
    frame(&mut app, PLANAR);
    assert_eq!(app.planar_render.camera().map(|mirror| mirror.h), Some(0.0));
}
