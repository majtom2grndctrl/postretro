// Planar mirror SH visible targeting from the app's real preparation (AC 19, 20, 21).
// See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use postretro_visibility::VisibleCells;

use super::tests::{controller, topology};
use super::*;
use crate::render_preparation::planar_test_fixtures::*;

/// One cluster per fixture cell, so a cell's cluster id is its cell id.
fn per_cell_clusters() -> ShResidencyController {
    let cells = 9;
    controller(topology(
        (0..cells as u32).collect(),
        vec![Vec::new(); cells],
        vec![Vec::new(); cells],
        vec![1; cells],
    ))
}

fn targets_after(visible: &VisibleCells) -> ShResidencyController {
    let mut controller = per_cell_clusters();
    controller.update_targets(visible, None, 0.0).unwrap();
    controller
}

// AC 21 (SH half): on a walk path with a completed mirror walk, the
// mirror-only rooms' clusters become SH visible targets.
#[test]
fn planar_mirror_cells_join_sh_visible_targeting_on_a_walk_path() {
    let world = world(WATER_ROOM);
    let main = main_view(&world);
    let mut mirror = installed(&world);
    prepare(&mut mirror, &world, &main, PLANAR);
    let controller = targets_after(mirror.sh_visible_cells(&main.visible_cells));
    assert!(controller.is_targeted(WATER_ROOM));
    for room in MIRROR_ROOMS {
        assert!(controller.is_targeted(room), "room {room}");
    }
    assert!(
        !controller.is_targeted(EMPTY_ATTIC),
        "the mirror's empty reach draws nothing and adds no SH read"
    );
}

// AC 19 (SH half): with the main view on its solid-cell fallback, SH
// targets only the main view's drawn clusters; the mirror's add no read.
#[test]
fn planar_mirror_adds_no_sh_reads_on_main_fallback() {
    let world = world(SOLID_CAMERA_CELL);
    let main = main_view(&world);
    let mut mirror = installed(&world);
    prepare(&mut mirror, &world, &main, PLANAR);
    assert!(mirror.camera().is_some(), "the mirror still draws");
    let controller = targets_after(mirror.sh_visible_cells(&main.visible_cells));
    assert!(controller.is_targeted(WATER_ROOM));
    for room in MIRROR_ROOMS {
        assert!(!controller.is_targeted(room), "room {room}");
    }
}

// AC 20 (SH half): a mirror walk forced to trip adds no SH visible target.
#[test]
fn planar_forced_mirror_trip_adds_no_sh_reads() {
    let world = world(WATER_ROOM);
    let main = main_view(&world);
    let mut mirror = installed(&world);
    mirror.set_step_cap(Some(0));
    prepare(&mut mirror, &world, &main, PLANAR);
    let controller = targets_after(mirror.sh_visible_cells(&main.visible_cells));
    assert!(controller.is_targeted(WATER_ROOM));
    for room in MIRROR_ROOMS {
        assert!(!controller.is_targeted(room), "room {room}");
    }
}
