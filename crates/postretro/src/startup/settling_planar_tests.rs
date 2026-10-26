// Settling with the planar mirror: the held frame runs O1 and its drawn set is the settle set.
// See: context/lib/boot_sequence.md §1 (Settling) · context/lib/rendering_pipeline.md §4

use postretro_render_cpu::fluid_surface::reflection_quality::FluidReflectionQuality;
use postretro_render_cpu::planar_reflection::{PlanarPreset, planar_preset};
use postretro_visibility::{MirrorVisibilityPath, VisibilityPath, VisibleCells};

use crate::App;
use crate::app::world_less_frame::HeldLevelView;
use crate::lightmap_streaming::controller::BlockPhase;
use crate::lightmap_streaming::test_fixtures::{Rig, headroom};
use crate::render_preparation::VisibleRenderPreparation;
use crate::render_preparation::planar_test_fixtures::{
    EMPTY_ATTIC, MIRROR_ROOMS, PLANAR, SOLID_CAMERA_CELL, WATER_ROOM, camera, cells, is_height,
    world,
};
use crate::session::level_streaming::settle::mirror_only_cells;
use crate::startup::lifecycle::tests::test_app;

fn planar() -> Option<PlanarPreset> {
    planar_preset(PLANAR)
}

/// An app holding the planar fixture level, its water installed as a
/// candidate, with the main locator answering `locator_cell`.
fn held_app(locator_cell: u32) -> App {
    let mut app = test_app();
    let world = world(locator_cell);
    app.planar_render.install(&world, &[0]);
    app.level = Some(world);
    app
}

fn held_view() -> HeldLevelView {
    HeldLevelView { camera: camera() }
}

/// The first world frame at the held pose, as the play frame prepares it:
/// the camera walk, then the mirror step at the tier's preset.
fn reveal_frame(app: &mut App, view: &HeldLevelView) -> VisibleRenderPreparation {
    let world = app.level.as_ref().expect("a held level");
    let main = VisibleRenderPreparation::for_level(
        world,
        view.camera.eye_position,
        view.camera.view_projection,
        &app.blocked_portals,
        false,
        &mut app.portal_walk_scratch,
        app.cpu_timer.gate(),
    );
    app.prepare_planar_mirror_at(&view.camera, &main, planar());
    main
}

/// Settling frames at the held pose until the lightmap's settle set is
/// installed: each runs the held frame's visibility and hands its drawn set
/// (main and mirror) to Settling's demand. Returns the plane they selected.
fn settle(app: &mut App, rig: &mut Rig, path: fn(&VisibilityPath) -> bool) -> Option<f32> {
    let view = held_view();
    let mut plane = None;
    for frame in 0..16 {
        let held = app
            .held_level_visibility(&view, planar())
            .expect("a level and a session");
        assert!(path(&held.stats.path), "{:?}", held.stats.path);
        assert_eq!(
            app.planar_render.path(),
            Some(MirrorVisibilityPath::Walked),
            "the held pose's mirror walk completes"
        );
        let drawn = app.planar_render.drawn_cells(&held.visible_cells);
        for room in MIRROR_ROOMS {
            assert!(cells(drawn).contains(&room), "frame {frame}: room {room}");
            assert!(!cells(&held.visible_cells).contains(&room));
        }
        plane = app.planar_render.camera().map(|mirror| mirror.h);
        let batch = rig.settling_frame(held.stats.camera_cell, held.stats.path, drawn);
        if frame == 0 {
            let requested = rig.requested_blocks();
            assert!(
                MIRROR_ROOMS.iter().all(|room| requested.contains(room)),
                "Settling reads the mirror's blocks on {:?}: {requested:?}",
                held.stats.path
            );
        }
        if let Some(batch) = batch {
            rig.install_all(&batch, headroom(8));
        }
        rig.complete_all();
    }
    assert_eq!(
        rig.controller.unsettled_blocks(),
        Some(0),
        "the settle set settles"
    );
    for room in MIRROR_ROOMS {
        assert_eq!(rig.controller.phase(room), BlockPhase::Installed);
    }
    plane
}

/// The mirror's drawable cells, as the `[Settling]` line reads them: none
/// without a mirror view.
fn mirror_cells(app: &App) -> &[u32] {
    match app.planar_render.visible() {
        VisibleCells::Culled(cells) if app.planar_render.camera().is_some() => cells,
        _ => &[],
    }
}

/// The held pose's mirror-only blocks outside the camera cell's baked set,
/// as the `[Settling]` line counts them.
fn mirror_only_blocks(app: &mut App, rig: &Rig) -> Option<usize> {
    let held = app.held_level_visibility(&held_view(), planar()).unwrap();
    let only: Vec<u32> = mirror_only_cells(&held.visible_cells, mirror_cells(app)).collect();
    assert!(
        app.planar_render.reach().contains(&EMPTY_ATTIC),
        "the mirror reaches the empty attic"
    );
    assert_eq!(
        only,
        MIRROR_ROOMS.to_vec(),
        "the mirror draws what the camera does not; its empty reach adds no cell"
    );
    rig.controller
        .blocks_outside_camera_set(mirror_only_cells(&held.visible_cells, mirror_cells(app)))
}

/// The first world frame at the held pose: no new read, no visible miss,
/// and the plane the held frames selected.
fn assert_reveal_reads_nothing(app: &mut App, rig: &mut Rig, held_plane: Option<f32>) {
    rig.requests.clear();
    let view = held_view();
    let reveal = reveal_frame(app, &view);
    assert_eq!(
        app.planar_render.camera().map(|mirror| mirror.h),
        held_plane,
        "the reveal frame selects the held frames' plane"
    );
    let drawn = app.planar_render.drawn_cells(&reveal.visible_cells);
    for room in MIRROR_ROOMS {
        assert!(
            cells(drawn).contains(&room),
            "the reveal mirror draws {room}"
        );
    }
    let batch = rig.frame(reveal.stats.camera_cell, reveal.stats.path, drawn);
    if let Some(batch) = batch {
        rig.install_all(&batch, headroom(8));
    }
    rig.controller.count_visible_misses();
    assert!(
        rig.requests.is_empty(),
        "the reveal frame requests no block it did not hold: {:?}",
        rig.requested_blocks()
    );
    assert_eq!(
        rig.controller.counters().last_frame_drawn_not_resident,
        0,
        "the reveal frame draws every mirror block resident"
    );
}

// AC 24 (O3): with the main view on its solid-cell fallback and the mirror
// walk completed, the held frame's drawn set carries the mirror's cells, so
// the lightmap settle set holds the mirror rooms' blocks, which the play
// path rules would never read on this path. On the first world frame on the
// same fallback the mirror requests no block it did not already hold, and
// draws them all resident.
#[test]
fn planar_settling_on_a_main_fallback_settles_the_mirror_blocks_and_the_reveal_reads_none() {
    let mut app = held_app(SOLID_CAMERA_CELL);
    let mut rig = Rig::corridor(None);
    let held_plane = settle(&mut app, &mut rig, |path| {
        matches!(path, VisibilityPath::SolidCellFallback)
    });
    assert!(is_height(held_plane, 0.0));
    // The solid camera cell has no baked set: both mirror rooms' blocks lie
    // outside it; the attic has none.
    assert_eq!(mirror_only_blocks(&mut app, &rig), Some(2));
    assert_reveal_reads_nothing(&mut app, &mut rig, held_plane);
    for room in MIRROR_ROOMS {
        assert_eq!(rig.controller.phase(room), BlockPhase::Installed);
    }
}

// O2 on a walk path: the held frames' settle set holds the mirror-only
// rooms' blocks (outside camera 0's baked set), the reveal frame selects the
// plane the held frames selected and draws zero lightmap misses.
#[test]
fn planar_settling_on_a_walk_path_settles_the_mirror_blocks_and_reveals_the_same_plane() {
    let mut app = held_app(WATER_ROOM);
    let mut rig = Rig::corridor(None);
    let held_plane = settle(&mut app, &mut rig, |path| {
        matches!(path, VisibilityPath::PrlPortal { .. })
    });
    assert!(is_height(held_plane, 0.0));
    assert_eq!(
        mirror_only_blocks(&mut app, &rig),
        Some(2),
        "rooms 5 and 6 lie outside camera 0's baked set"
    );
    assert_reveal_reads_nothing(&mut app, &mut rig, held_plane);
}

// Control for both: below very high the held frame runs no mirror, so its
// settle set is the main view's alone and holds no mirror room's block.
#[test]
fn planar_settling_below_very_high_settles_the_main_view_alone() {
    let mut app = held_app(SOLID_CAMERA_CELL);
    let mut rig = Rig::corridor(None);
    let held = app
        .held_level_visibility(&held_view(), planar_preset(FluidReflectionQuality::High))
        .unwrap();
    assert!(app.planar_render.camera().is_none());
    let drawn = app.planar_render.drawn_cells(&held.visible_cells);
    assert!(std::ptr::eq(drawn, &held.visible_cells));
    rig.settling_frame(held.stats.camera_cell, held.stats.path, drawn);
    let requested = rig.requested_blocks();
    assert!(
        MIRROR_ROOMS.iter().all(|room| !requested.contains(room)),
        "{requested:?}"
    );
    assert!(
        mirror_only_cells(&held.visible_cells, mirror_cells(&app))
            .next()
            .is_none()
    );
    assert!(
        mirror_only_cells(&VisibleCells::DrawAll, &MIRROR_ROOMS)
            .next()
            .is_none(),
        "a main view that draws all leaves the mirror nothing of its own"
    );
}
