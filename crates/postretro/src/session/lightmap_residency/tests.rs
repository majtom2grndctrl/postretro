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
                residency_set: set,
                camera_cell: 0,
                path: PORTAL,
                visible_cells: &visible,
            },
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
