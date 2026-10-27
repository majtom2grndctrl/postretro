//! Planar mirror lightmap demand under the path rules, from the app's real
//! preparation (AC 19, 20, 21). See: context/lib/rendering_pipeline.md §4

use postretro_level_loader::{LightmapBlockClass, LightmapTarget};
use postretro_visibility::VisibilityPath;

use crate::lightmap_streaming::controller::BlockPhase;
use crate::lightmap_streaming::test_fixtures::*;
use crate::render_preparation::planar_test_fixtures::*;

const ROOM_A: u32 = MIRROR_ROOMS[0];
const ROOM_B: u32 = MIRROR_ROOMS[1];

fn visible(block: u32) -> Option<LightmapTarget> {
    Some(LightmapTarget {
        block,
        class: LightmapBlockClass::Visible,
        lead: 0,
    })
}

// AC 21: on a walk path with a completed mirror walk, the mirror-only rooms'
// blocks (outside camera 0's baked set) are demanded as visible and read;
// once demand at the held pose drains, the pose draws zero visible misses.
#[test]
fn planar_mirror_cells_join_demand_on_a_walk_path() {
    let world = world(WATER_ROOM);
    let main = main_view(&world);
    let mut mirror = installed(&world);
    prepare(&mut mirror, &world, &main, PLANAR);
    let drawn = mirror.drawn_cells(&main.visible_cells);
    let camera = main.stats.camera_cell;
    let path = main.stats.path;
    assert!(matches!(path, VisibilityPath::PrlPortal { .. }));

    let mut rig = Rig::corridor(None);
    let batch = rig
        .frame(camera, path, drawn)
        .expect("the first frame drains");
    assert_eq!(rig.controller.target(ROOM_A), visible(ROOM_A));
    assert_eq!(rig.controller.target(ROOM_B), visible(ROOM_B));
    let requested = rig.requested_blocks();
    assert!(requested.contains(&ROOM_A) && requested.contains(&ROOM_B));
    rig.install_all(&batch, headroom(8));
    rig.complete_all();

    // Drain demand at the held pose.
    for _ in 0..16 {
        let batch = rig
            .frame(camera, path, drawn)
            .expect("no outcome outstanding");
        rig.install_all(&batch, headroom(8));
        rig.complete_all();
    }
    let batch = rig.frame(camera, path, drawn).unwrap();
    rig.install_all(&batch, headroom(8));
    rig.controller.count_visible_misses();
    assert_eq!(rig.controller.phase(ROOM_A), BlockPhase::Installed);
    assert_eq!(rig.controller.phase(ROOM_B), BlockPhase::Installed);
    assert_eq!(
        rig.controller.counters().last_frame_drawn_not_resident,
        0,
        "the drained pose draws every mirror block resident"
    );
}

// AC 19: with the main view on its solid-cell fallback at the same pose, the
// mirror still walks and draws its rooms but adds no lightmap read. Room A's
// block, resident from earlier walk-path frames, stays and draws; room B's,
// never read, is not requested and counts as a visible miss.
#[test]
fn planar_mirror_adds_no_lightmap_reads_on_main_fallback() {
    let world = world(SOLID_CAMERA_CELL);
    let main = main_view(&world);
    assert!(matches!(main.stats.path, VisibilityPath::SolidCellFallback));
    let mut mirror = installed(&world);
    prepare(&mut mirror, &world, &main, PLANAR);
    let drawn = mirror.drawn_cells(&main.visible_cells);
    assert!(cells(drawn).contains(&ROOM_A) && cells(drawn).contains(&ROOM_B));

    let mut rig = Rig::corridor(None);
    rig.settle(0, &[WATER_ROOM, ROOM_A], headroom(8));
    assert_eq!(rig.controller.phase(ROOM_A), BlockPhase::Installed);
    rig.requests.clear();

    let batch = rig
        .frame(main.stats.camera_cell, main.stats.path, drawn)
        .unwrap();
    rig.install_all(&batch, headroom(8));
    rig.controller.count_visible_misses();
    assert!(rig.requests.is_empty(), "the fallback frame reads nothing");
    assert_eq!(
        rig.controller.target(ROOM_A),
        visible(ROOM_A),
        "a resident mirror block is held"
    );
    assert_eq!(rig.controller.phase(ROOM_A), BlockPhase::Installed);
    assert_eq!(rig.controller.target(ROOM_B), None);
    assert_eq!(
        rig.controller.counters().last_frame_drawn_not_resident,
        1,
        "room B's block draws as a counted miss"
    );
}

// On every main-view fallback the mirror's drawn rooms request no lightmap
// read, and only the camera cell's baked set is read.
#[test]
fn planar_mirror_cells_request_no_lightmaps_on_main_visibility_fallbacks() {
    let world = world(SOLID_CAMERA_CELL);
    let main = main_view(&world);
    let mut mirror = installed(&world);
    prepare(&mut mirror, &world, &main, PLANAR);
    let drawn = mirror.drawn_cells(&main.visible_cells);
    for (camera, path) in [
        (0, VisibilityPath::NoPortalsFallback),
        (
            0,
            VisibilityPath::PortalStepLimitFallback {
                considered: 10,
                accepted: 3,
            },
        ),
        (main.stats.camera_cell, main.stats.path),
    ] {
        let mut rig = Rig::corridor(None);
        rig.frame(camera, path, drawn);
        assert_eq!(rig.controller.target(ROOM_A), None, "{path:?}");
        assert_eq!(rig.controller.target(ROOM_B), None, "{path:?}");
        let requested = rig.requested_blocks();
        assert!(
            requested
                .iter()
                .all(|block| ![ROOM_A, ROOM_B].contains(block)),
            "no mirror block is read on {path:?}"
        );
        if camera == 0 {
            assert_eq!(requested, vec![0, 1, 2], "the camera set alone");
        } else {
            assert!(requested.is_empty(), "a held main view reads nothing");
        }
    }
}

// AC 20 (lightmap half): a mirror walk forced to trip hands lightmap demand
// exactly the main view's drawn set, so the frame reads what it would read
// with no mirror at all.
#[test]
fn planar_forced_mirror_trip_adds_no_lightmap_reads() {
    let world = world(WATER_ROOM);
    let main = main_view(&world);
    let mut mirror = installed(&world);
    mirror.set_step_cap(Some(0));
    prepare(&mut mirror, &world, &main, PLANAR);

    let mut with_mirror = Rig::corridor(None);
    with_mirror.frame(
        main.stats.camera_cell,
        main.stats.path,
        mirror.drawn_cells(&main.visible_cells),
    );
    let mut without = Rig::corridor(None);
    without.frame(main.stats.camera_cell, main.stats.path, &main.visible_cells);
    assert_eq!(with_mirror.requested_blocks(), without.requested_blocks());
    assert_eq!(with_mirror.controller.target(ROOM_A), None);
    assert_eq!(with_mirror.controller.target(ROOM_B), None);
}
