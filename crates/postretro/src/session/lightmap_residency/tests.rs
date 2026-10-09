//! Lightmap session proofs: the parked renderer batch and route handoff.
//! See: context/lib/testing_guide.md

use std::sync::Arc;

use postretro_level_loader::LightmapDrainOutcome;
use postretro_visibility::VisibleCells;

use super::*;
use crate::lightmap_streaming::test_fixtures::*;

fn drain_once(session: &mut LightmapStreamingSession, set: &CellResidencySetSection) {
    let visible = VisibleCells::Culled(Vec::new());
    let mut drain = SharedDrain::default();
    drain.begin();
    session
        .begin_drain(
            DemandFrame {
                lead: crate::streaming::cell_demand::DEFAULT_LEAD,
                residency_set: set,
                camera_cell: 0,
                path: PORTAL,
                visible_cells: &visible,
            },
            false,
            &mut drain,
        )
        .unwrap();
    drain.admit().unwrap();
    session.finish_drain(&drain, None).unwrap();
}

// The frame's batch waits in the session for the renderer;
// while it waits, later frames build no further batch, and the deltas they
// accumulate ride the first batch after the outcome returns.
#[test]
fn parked_batch_holds_later_batches_until_the_renderer_returns_its_outcome() {
    let source = TestBlockSource::new(corridor_blocks(64, true));
    let set = corridor_set();
    let mut session = LightmapStreamingSession::with_source(
        Arc::clone(&source) as Arc<dyn LightmapBlockSource>,
        &set,
        None,
    )
    .unwrap();
    // The spawned issuer would own the route; holding it keeps the
    // completion queue connected.
    let _route = session
        .take_route()
        .expect("the issuer takes the route once");
    assert!(session.take_route().is_none());

    drain_once(&mut session, &set);
    let reset = session
        .parked_batch()
        .unwrap()
        .target_reset
        .clone()
        .unwrap();
    assert_eq!(reset.len(), 5, "camera 0: three mandatory, two band");
    for _ in 0..3 {
        drain_once(&mut session, &set);
    }
    let first = session.take_drain_batch_for_renderer().unwrap();
    assert!(first.target_reset.is_some());
    drain_once(&mut session, &set);
    assert!(
        session.parked_batch().is_none(),
        "outcome still outstanding"
    );

    session
        .apply_outcome(LightmapDrainOutcome::default())
        .unwrap();
    drain_once(&mut session, &set);
    let second = session.take_drain_batch_for_renderer().unwrap();
    assert!(second.target_reset.is_none());
    assert!(second.target_set.is_empty() && second.target_remove.is_empty());
    assert!(
        session
            .apply_outcome(LightmapDrainOutcome::default())
            .is_ok()
    );
    assert!(
        session
            .apply_outcome(LightmapDrainOutcome::default())
            .is_err(),
        "an outcome needs an outstanding batch"
    );
    assert_eq!(session.ledger().in_memory_bytes(), 0);
}

// Level install's preload through the session: the renderer's drain installs
// the spawn cell's mandatory set in one batch, the outcome is applied, and the
// session then drains frames as usual.
#[test]
fn session_preload_installs_the_spawn_set_through_one_renderer_drain() {
    let source = TestBlockSource::new(corridor_blocks(64, true));
    let set = corridor_set();
    let mut session = LightmapStreamingSession::with_source(
        Arc::clone(&source) as Arc<dyn LightmapBlockSource>,
        &set,
        None,
    )
    .unwrap();
    let mut model = pool_model(&source);
    session.update_camera_set(&set, 2);
    let mut drains = 0;

    let summary = session
        .preload(&[], |batch| {
            drains += 1;
            Ok(model_drain(&mut model, batch))
        })
        .unwrap();

    assert_eq!(drains, 1, "install time is not frame time: one batch");
    assert_eq!((summary.reads.pairs, summary.installed), (5, 5));
    assert_eq!((summary.deferred, summary.failed_installs), (0, 0));
    assert_eq!(
        summary.reads.bytes,
        5 * 2 * 64,
        "both 64-byte halves of five pairs"
    );
    assert!(session.settled());
    assert!((0..5).all(|block| model.is_resident(block)));

    drain_once(&mut session, &set);
    assert!(
        session.parked_batch().is_some(),
        "the first frame drains after the preload's outcome"
    );
}

// Settling presents nothing: its drawn blocks, cold or not, count no
// lightmap visible miss. The first frame after reveal counts them.
#[test]
fn settling_frames_count_no_lightmap_visible_miss() {
    let source = TestBlockSource::new(corridor_blocks(64, true));
    let set = corridor_set();
    let mut session = LightmapStreamingSession::with_source(
        Arc::clone(&source) as Arc<dyn LightmapBlockSource>,
        &set,
        None,
    )
    .unwrap();
    let _route = session.take_route();
    let visible = VisibleCells::Culled(vec![0, 5]);
    let frame = |settling: bool, session: &mut LightmapStreamingSession| {
        let mut drain = SharedDrain::default();
        drain.begin();
        session
            .begin_drain(
                DemandFrame {
                    lead: crate::streaming::cell_demand::DEFAULT_LEAD,
                    residency_set: &set,
                    camera_cell: 0,
                    path: PORTAL,
                    visible_cells: &visible,
                },
                settling,
                &mut drain,
            )
            .unwrap();
        drain.admit().unwrap();
        session.finish_drain(&drain, None).unwrap();
        session.refresh_diagnostics(None);
    };
    frame(true, &mut session);
    assert_eq!(session.controller().counters().drawn_not_resident, 0);
    // The parked batch returns before the next frame drains.
    session
        .take_drain_batch_for_renderer()
        .expect("the settling frame parks a batch");
    session
        .apply_outcome(LightmapDrainOutcome {
            pool: headroom(8),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
    frame(false, &mut session);
    assert_eq!(
        session.controller().counters().drawn_not_resident,
        2,
        "the first presented frame counts both cold drawn blocks"
    );
}
