//! Demand past the pool: the model grows it, and the controller warns by name.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use log::Level;
use postretro_render_cpu::lightmap_pool::{LightmapPoolModel, PerPool};
use postretro_test_log_capture::LogCapture;
use postretro_visibility::{VisibilityPath, VisibleCells};

use super::*;
use crate::lightmap_streaming::test_fixtures::*;

/// Each 64×64 corridor block fills a layer of this edge.
const EDGE: u32 = 64;
/// Every corridor block's two planes, one to a layer.
const EVERY_PLANE: u32 = 14;

fn pool(rig: &Rig, lightmap: u32, mask: u32) -> LightmapPoolModel {
    LightmapPoolModel::new(source_demand(&rig.source), EDGE, PerPool { lightmap, mask })
        .expect("the blocks fit a layer")
}

/// `frames` frames at `camera_cell`: reads land between them, every batch runs
/// through `model`, and a retiring generation's work completes at once.
fn run(rig: &mut Rig, model: &mut LightmapPoolModel, camera_cell: u32, drawn: &[u32], frames: u32) {
    for _ in 0..frames {
        let batch = rig
            .portal(camera_cell, drawn)
            .expect("no outcome outstanding");
        let outcome = model_drain(model, batch);
        rig.controller.apply_outcome(outcome).unwrap();
        rig.complete_all();
        for kind in PoolKind::ALL {
            model.release_retirement(kind);
        }
    }
}

// Camera 0's three mandatory blocks past a one-layer pool: one growth, sized for
// all three however their pairs arrive, and one warning naming the pool and the
// cell. The mask pool holds its planes and says nothing.
#[test]
fn demand_past_pool_grows_once_and_warns() {
    let capture = LogCapture::start();
    let mut rig = Rig::corridor(None);
    let mut model = pool(&rig, 1, EVERY_PLANE);

    // Targets past the pool grow nothing until a ready pair cannot be placed.
    run(&mut rig, &mut model, 0, &[], 1);
    assert_eq!(model.pool(PoolKind::Lightmap).texture_allocations(), 1);
    capture.assert_not_logged(Level::Warn, "pool grew");

    run(&mut rig, &mut model, 0, &[], 5);
    assert_eq!(rig.controller.unsettled_blocks(), Some(0));
    assert_eq!(rig.controller.pool_report().lightmap.layers, 4, "3 × 5/4");
    assert_eq!(model.pool(PoolKind::Lightmap).texture_allocations(), 2);
    capture.assert_logged_once(
        Level::Warn,
        "The lightmap pool grew to 4 layers at camera cell 0",
    );
    capture.assert_not_logged(Level::Warn, "The mask pool grew");
}

#[test]
fn the_mask_pool_warns_for_itself() {
    let capture = LogCapture::start();
    let mut rig = Rig::corridor(None);
    let mut model = pool(&rig, 7, 1);
    run(&mut rig, &mut model, 0, &[], 6);

    assert_eq!(model.pool(PoolKind::Mask).texture_allocations(), 2);
    assert_eq!(model.pool(PoolKind::Lightmap).texture_allocations(), 1);
    capture.assert_logged_once(
        Level::Warn,
        "The mask pool grew to 8 layers at camera cell 0",
    );
    capture.assert_not_logged(Level::Warn, "The lightmap pool grew");
}

// Band blocks give way first: demand they make room for neither grows the pool
// nor warns.
#[test]
fn demand_band_blocks_make_room_for_grows_nothing() {
    let capture = LogCapture::start();
    let mut rig = Rig::corridor(None);
    let mut model = pool(&rig, 5, EVERY_PLANE);
    run(&mut rig, &mut model, 0, &[], 6);
    assert_eq!(
        rig.controller.phase(4),
        BlockPhase::Installed,
        "band resident"
    );

    run(&mut rig, &mut model, 0, &[5, 6], 6);
    assert_eq!(rig.controller.phase(5), BlockPhase::Installed);
    assert_eq!(rig.controller.phase(6), BlockPhase::Installed);
    assert_eq!(model.pool(PoolKind::Lightmap).texture_allocations(), 1);
    capture.assert_not_logged(Level::Warn, "pool grew");
}

// The device limit defers as it always has: a counted miss, and no growth to
// warn about.
#[test]
fn demand_past_the_device_limit_defers_and_warns_of_no_growth() {
    let capture = LogCapture::start();
    let mut rig = Rig::corridor(None);
    let mut model = LightmapPoolModel::with_layer_limit(
        source_demand(&rig.source),
        EDGE,
        PerPool {
            lightmap: 2,
            mask: 2,
        },
        2,
    )
    .expect("the blocks fit a layer");
    run(&mut rig, &mut model, 0, &[], 6);

    assert!(rig.controller.counters().deferrals > 0);
    assert_eq!(model.pool(PoolKind::Lightmap).layers(), 2);
    capture.assert_not_logged(Level::Warn, "pool grew");
}

// "Once" is per demand. A later demand past the grown pool, after its
// generation retired, grows it again and warns again, from the cell it met.
#[test]
fn second_demand_grows_and_warns_again() {
    let capture = LogCapture::start();
    let mut rig = Rig::corridor(None);
    let mut model = pool(&rig, 1, EVERY_PLANE);
    run(&mut rig, &mut model, 0, &[], 6);
    assert_eq!(model.pool(PoolKind::Lightmap).layers(), 4);

    // Camera 5 demands 3 to 6, and draws 0 to 2: all seven blocks.
    run(&mut rig, &mut model, 5, &[0, 1, 2], 6);
    assert_eq!(rig.controller.unsettled_blocks(), Some(0));
    assert_eq!(
        model.pool(PoolKind::Lightmap).layers(),
        7,
        "capped at all-resident"
    );
    assert_eq!(model.pool(PoolKind::Lightmap).texture_allocations(), 3);
    capture.assert_logged_once(
        Level::Warn,
        "The lightmap pool grew to 4 layers at camera cell 0",
    );
    capture.assert_logged_once(
        Level::Warn,
        "The lightmap pool grew to 7 layers at camera cell 5",
    );
}

// A Settling frame demands everything its view draws, on any path, so a
// fallback frame's set can pass the pool before the first frame shows. It grows
// once and warns, the level reveals, and the pool keeps its size through play.
#[test]
fn settling_past_the_pool_grows_once_and_keeps_size() {
    let capture = LogCapture::start();
    let mut rig = Rig::corridor(None);
    let mut model = pool(&rig, 2, EVERY_PLANE);
    let fallback = VisibilityPath::PortalStepLimitFallback {
        considered: 0,
        accepted: 0,
    };
    let drawn = VisibleCells::Culled(vec![0, 1, 2, 3, 4, 5]);
    for _ in 0..8 {
        let batch = rig.settling_frame(0, fallback, &drawn).unwrap();
        let outcome = model_drain(&mut model, batch);
        rig.controller.apply_outcome(outcome).unwrap();
        rig.complete_all();
    }
    assert_eq!(rig.controller.settle_set_blocks(), Some(6));
    assert_eq!(rig.controller.unsettled_blocks(), Some(0), "the reveal");
    assert_eq!(model.pool(PoolKind::Lightmap).texture_allocations(), 2);
    let settled_layers = model.pool(PoolKind::Lightmap).layers();
    assert_eq!(settled_layers, 7, "6 × 5/4, capped at all-resident");
    capture.assert_logged_once(
        Level::Warn,
        "The lightmap pool grew to 7 layers at camera cell 0",
    );

    // Play moves on to a small set and the blocks leave; the layers stay.
    model.release_retirement(PoolKind::Lightmap);
    run(&mut rig, &mut model, 6, &[], 6);
    assert!(rig.controller.counters().evictions > 0);
    assert_eq!(model.pool(PoolKind::Lightmap).layers(), settled_layers);
    assert_eq!(rig.controller.pool_report().lightmap.layers, settled_layers);
    assert_eq!(model.pool(PoolKind::Lightmap).texture_allocations(), 2);
}
