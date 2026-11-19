//! Lightmap visible-miss causes: each drawn-not-resident block-frame lands in
//! exactly one bucket (streaming-drain-budget-spike AC 2; pins P4, P6-P8, P15).
//! See: context/lib/testing_guide.md

use postretro_level_loader::{LightmapDrainBatch, LightmapDrainOutcome, PrlLoadError};
use postretro_renderer::LightmapMissBuckets;
use postretro_visibility::{VisibilityPath, VisibleCells};

use super::*;
use crate::lightmap_streaming::route::{LightmapCompletion, LightmapReadResult};
use crate::lightmap_streaming::test_fixtures::*;
use crate::streaming::request::ReadRequest;

const STEP_LIMIT: VisibilityPath = VisibilityPath::PortalStepLimitFallback {
    considered: 10,
    accepted: 3,
};

/// Exactly one block-frame, in the bucket `pick` names.
fn only(pick: fn(&mut LightmapMissBuckets) -> &mut u64) -> LightmapMissBuckets {
    let mut buckets = LightmapMissBuckets::default();
    *pick(&mut buckets) = 1;
    buckets
}

/// Counts the frame's misses and returns its buckets, after checking that
/// they sum to the drawn-not-resident gauge and the cumulative buckets to
/// the cumulative count.
fn count(rig: &mut Rig) -> LightmapMissBuckets {
    rig.controller.count_visible_misses();
    let counters = rig.controller.counters();
    assert_eq!(
        counters.last_frame_miss_buckets.total(),
        u64::from(counters.last_frame_drawn_not_resident),
        "the frame's buckets sum to its drawn-not-resident block-frames"
    );
    assert_eq!(
        counters.miss_buckets.total(),
        counters.drawn_not_resident,
        "the cumulative buckets sum to drawn-not-resident"
    );
    counters.last_frame_miss_buckets
}

/// Delivers the captured read of `block` alone.
fn deliver(rig: &mut Rig, block: u32) {
    let index = rig
        .requests
        .iter()
        .position(|request| request.key == block)
        .expect("a captured read");
    let request = rig.requests.remove(index);
    let completion = rig.source.completion(request);
    rig.controller.admit_completion(completion).unwrap();
}

fn blocks_of(batch: &LightmapDrainBatch) -> Vec<u32> {
    batch.ready.iter().map(|prepared| prepared.block).collect()
}

/// The renderer installs `installed` and hands `deferred` back unplaced.
fn install_and_defer(
    rig: &mut Rig,
    batch: LightmapDrainBatch,
    installed: &[u32],
    deferred: &[u32],
) {
    rig.controller
        .apply_outcome(LightmapDrainOutcome {
            installed: installed.to_vec(),
            deferred: batch
                .ready
                .into_iter()
                .filter(|prepared| deferred.contains(&prepared.block))
                .collect(),
            pool: headroom(8),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
}

/// The corridor with `big` blocks' halves at `big_half` bytes and every other
/// block's at 64.
fn corridor_with_big(big: &[u32], big_half: u64) -> Rig {
    let blocks = (0..7)
        .map(|block| {
            let half = if big.contains(&block) { big_half } else { 64 };
            BlockSpec::standard(block, block, half, true)
        })
        .collect();
    Rig::new(blocks, corridor_set(), None)
}

// A non-portal frame draws a block outside the camera cell's baked set with
// no pair: the frame may not read it.
#[test]
fn drawn_block_a_held_frame_may_not_read_is_outside_the_reach() {
    let mut rig = Rig::corridor(None);
    rig.frame(0, STEP_LIMIT, &VisibleCells::Culled(vec![5]));
    assert_eq!(rig.controller.target(5), None);
    assert_eq!(count(&mut rig), only(|b| &mut b.outside_reach));
}

// A drawn block whose read was submitted this frame is in flight.
#[test]
fn drawn_block_with_its_read_in_flight_is_read_in_flight() {
    let mut rig = Rig::corridor(None);
    rig.portal(6, &[6]);
    assert_eq!(rig.controller.phase(6), BlockPhase::InFlight);
    assert_eq!(count(&mut rig), only(|b| &mut b.read_in_flight));
}

// A drawn block the request pass never reached because every permit was
// held waits on a permit, not on a read in flight.
#[test]
fn drawn_block_left_by_a_pass_with_every_permit_held_waits_on_a_permit() {
    let cells = MAX_LIGHTMAP_PERMITS as u32 + 1;
    let rows: Vec<_> = (0..cells - 1).map(|cell| (0, cell, 0)).collect();
    let blocks = (0..cells)
        .map(|block| BlockSpec::standard(block, block, 64, true))
        .collect();
    let mut rig = Rig::new(blocks, residency_set(cells, &rows, 32), None);
    rig.portal(0, &[]);
    assert_eq!(rig.controller.permits_in_use(), MAX_LIGHTMAP_PERMITS);

    let extra = cells - 1;
    rig.portal(0, &[extra]);
    assert_eq!(rig.controller.phase(extra), BlockPhase::Absent);
    assert_eq!(count(&mut rig), only(|b| &mut b.waiting_on_permit));
}

// A drawn block the request pass stopped before at the in-hand pair bytes
// waits on the in-hand cap.
#[test]
fn drawn_block_left_by_a_pass_at_the_in_hand_bytes_waits_on_the_in_hand_cap() {
    // Camera 6 demands blocks 5 and 6, 20 MiB each: one fits the 32 MiB in
    // hand.
    let mut rig = Rig::new(corridor_blocks(10 * MIB, true), corridor_set(), None);
    rig.portal(6, &[]);
    assert_eq!(rig.requested_blocks(), vec![5]);
    rig.portal(6, &[6]);
    assert_eq!(rig.controller.phase(6), BlockPhase::Absent);
    assert_eq!(count(&mut rig), only(|b| &mut b.waiting_on_in_hand_cap));
}

// Ready, offered to this frame's drain and past the budget's cut: held by
// the drain budget. The pair the budget admitted installs and is no miss.
#[test]
fn drawn_ready_block_past_the_drains_cut_is_held_by_the_drain_budget() {
    // Blocks 5 and 6 pair at 5 MiB each: the 8 MiB drain admits one.
    let mut rig = corridor_with_big(&[5, 6], 5 * MIB / 2);
    let batch = rig.portal(6, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    rig.complete_all();
    let batch = rig.portal(6, &[5, 6]).unwrap();
    assert_eq!(blocks_of(&batch), vec![5], "block 6 is past the cut");
    rig.install_all(&batch, headroom(8));
    assert_eq!(count(&mut rig), only(|b| &mut b.held_by_drain_budget));
}

// P4: a ready block not offered because the renderer has not returned the
// previous lightmap batch is not held by the drain budget; nor is a pair in
// a batch still outstanding.
#[test]
fn drawn_block_behind_an_outstanding_batch_awaits_the_renderer_drain() {
    let mut rig = Rig::corridor(None);
    let outstanding = rig.portal(6, &[]).unwrap();
    rig.complete_all();
    assert!(rig.portal(6, &[6]).is_none(), "the batch is outstanding");
    assert_eq!(rig.controller.phase(6), BlockPhase::Ready);
    assert_eq!(count(&mut rig), only(|b| &mut b.awaiting_renderer_drain));

    rig.install_all(&outstanding, headroom(8));
    rig.portal(6, &[6]).unwrap();
    assert_eq!(rig.controller.phase(6), BlockPhase::InDrain);
    assert_eq!(count(&mut rig), only(|b| &mut b.awaiting_renderer_drain));
}

// The drain carried the pair and the pool handed it back unplaced.
#[test]
fn drawn_block_the_pool_deferred_is_deferred_by_the_pool() {
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(6, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    rig.complete_all();
    let batch = rig.portal(6, &[6]).unwrap();
    assert_eq!(blocks_of(&batch), vec![5, 6]);
    install_and_defer(&mut rig, batch, &[5], &[6]);
    assert_eq!(rig.controller.phase(6), BlockPhase::Ready);
    assert_eq!(count(&mut rig), only(|b| &mut b.deferred_by_pool));
}

/// The corridor at camera 0 with its mandatory blocks installed and band
/// block 3 refused at one block of headroom.
fn rig_with_band_block_3_refused() -> Rig {
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(1));
    rig.complete_all();
    for _ in 0..2 {
        let batch = rig.portal(0, &[]).unwrap();
        rig.install_all(&batch, headroom(1));
        rig.complete_all();
    }
    let batch = rig.portal(0, &[]).unwrap();
    assert_eq!(blocks_of(&batch), vec![3]);
    rig.controller
        .apply_outcome(LightmapDrainOutcome {
            refused: vec![refusal(3)],
            pool: headroom(1),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
    rig.controller.count_visible_misses();
    rig
}

// A held frame draws a band block of the camera set the pool refused.
#[test]
fn drawn_band_block_the_pool_refused_is_refused() {
    let mut rig = rig_with_band_block_3_refused();
    rig.frame(0, STEP_LIMIT, &VisibleCells::Culled(vec![3]));
    assert_eq!(rig.controller.phase(3), BlockPhase::Refused);
    assert_eq!(count(&mut rig), only(|b| &mut b.refused));
}

// A held frame draws a band block whose read the pass held back for want of
// band headroom: refused before reading, not waiting on a read.
#[test]
fn drawn_band_block_the_pass_held_for_pool_room_is_refused() {
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(0));
    rig.complete_all();
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(0));
    rig.controller.count_visible_misses();

    rig.frame(0, STEP_LIMIT, &VisibleCells::Culled(vec![3]));
    assert_eq!(rig.controller.phase(3), BlockPhase::Absent);
    assert_eq!(count(&mut rig), only(|b| &mut b.refused));
}

// P8: a failed read, a failed install, and a rolled-back drain each land in
// failed that frame, never in a read bucket; the next frame classifies the
// rolled-back block afresh.
#[test]
fn drawn_block_whose_read_install_or_drain_failed_is_failed() {
    // A read that failed.
    let mut rig = Rig::corridor(None);
    rig.portal(6, &[6]);
    let index = rig.requests.iter().position(|r| r.key == 6).unwrap();
    let request: ReadRequest = rig.requests.remove(index);
    rig.controller
        .admit_completion(LightmapCompletion {
            request,
            charged_bytes: 0,
            result: LightmapReadResult::Failed(PrlLoadError::SectionValidation {
                section: "test source",
                message: "unreadable".into(),
            }),
        })
        .unwrap();
    assert_eq!(count(&mut rig), only(|b| &mut b.failed));

    // An install the renderer failed.
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(6, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    rig.complete_all();
    rig.portal(6, &[6]).unwrap();
    rig.controller
        .apply_outcome(LightmapDrainOutcome {
            installed: vec![5],
            failed: vec![6],
            pool: headroom(8),
            ..LightmapDrainOutcome::default()
        })
        .unwrap();
    assert_eq!(count(&mut rig), only(|b| &mut b.failed));

    // A drain the renderer rolled back: its pair is Absent when counted.
    let mut rig = Rig::corridor(None);
    let batch = rig.portal(6, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    rig.complete_all();
    rig.portal(6, &[6]).unwrap();
    rig.controller.abort_drain();
    assert_eq!(rig.controller.phase(6), BlockPhase::Absent);
    assert_eq!(count(&mut rig), only(|b| &mut b.failed));
    rig.portal(6, &[6]);
    assert_eq!(rig.controller.phase(6), BlockPhase::InFlight, "read again");
    assert_eq!(count(&mut rig), only(|b| &mut b.read_in_flight));
}

// P6: two drains in one frame (the completed walk). A pair deferred by the
// pool in drain 0 and left past the cut in drain 1 is deferred: the frame's
// latest drain to carry it deferred it. In the next frame, left past the cut
// again, it is held by the drain budget: a deferral is not carried forward.
#[test]
fn deferral_attaches_to_the_drain_that_deferred_it_and_ends_with_the_frame() {
    // Camera 0 demands blocks 0, 1 and 2 at lead 0; 0 and 1 pair at 8 MiB,
    // so either fills a drain and leaves block 2 past the cut.
    let mut rig = corridor_with_big(&[0, 1], 4 * MIB);
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(0));
    assert_eq!(rig.requested_blocks(), vec![0, 1, 2]);
    rig.controller.count_visible_misses();

    // Frame N, drain 0: block 2 alone is ready; it drains and is deferred.
    deliver(&mut rig, 2);
    let batch = rig.portal(0, &[2]).unwrap();
    assert_eq!(blocks_of(&batch), vec![2]);
    install_and_defer(&mut rig, batch, &[], &[2]);
    // Drain 1: block 0 takes the budget; block 2 is past the cut.
    deliver(&mut rig, 0);
    let batch = rig.portal(0, &[2]).unwrap();
    assert_eq!(blocks_of(&batch), vec![0]);
    install_and_defer(&mut rig, batch, &[0], &[]);
    assert_eq!(count(&mut rig), only(|b| &mut b.deferred_by_pool));

    // Frame N+1: block 1 takes the budget; block 2 is past the cut again.
    deliver(&mut rig, 1);
    let batch = rig.portal(0, &[2]).unwrap();
    assert_eq!(blocks_of(&batch), vec![1]);
    install_and_defer(&mut rig, batch, &[1], &[]);
    assert_eq!(count(&mut rig), only(|b| &mut b.held_by_drain_budget));
}

// P7 and P15: the budget leaves ready pairs in hand, the in-hand bytes fill,
// and a newly drawn block's read waits. It counts as waiting on the in-hand
// cap even though this frame's outcome, applied after the request pass,
// frees enough bytes for it: the verdict is the pass's.
#[test]
fn ready_pairs_the_budget_leaves_in_hand_stall_a_new_read_on_the_in_hand_cap() {
    // Camera 0 demands blocks 0, 1 and 2 at lead 0, 10 MiB each: three fill
    // 30 of the 32 MiB in hand. Block 5, drawn, is another 10 MiB.
    let mut rig = corridor_with_big(&[0, 1, 2, 5], 5 * MIB);
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(0));
    assert_eq!(rig.requested_blocks(), vec![0, 1, 2]);
    rig.complete_all();
    rig.controller.count_visible_misses();

    let batch = rig.portal(0, &[5]).unwrap();
    assert_eq!(
        blocks_of(&batch),
        vec![0],
        "the budget leaves 1 and 2 ready"
    );
    assert!(
        rig.requests.is_empty(),
        "block 5 waits on the in-hand bytes"
    );
    rig.install_all(&batch, headroom(0));
    assert!(
        rig.controller.in_hand_bytes() + 10 * MIB <= MAX_IN_HAND_PAIR_BYTES,
        "the outcome freed room for block 5 after the pass"
    );
    assert_eq!(count(&mut rig), only(|b| &mut b.waiting_on_in_hand_cap));
}

// A settling frame presents nothing: it counts no miss in any bucket.
#[test]
fn settling_frame_counts_no_miss_in_any_bucket() {
    let mut rig = Rig::corridor(None);
    rig.settling_frame(6, PORTAL, &VisibleCells::Culled(vec![6]));
    rig.controller.discard_frame_misses();
    assert_eq!(count(&mut rig), LightmapMissBuckets::default());
}
