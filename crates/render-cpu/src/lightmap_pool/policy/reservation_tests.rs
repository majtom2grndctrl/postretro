// Reserved room: a miss until its pair uploads, off limits to band, returned when its target leaves.

use proptest::prelude::*;

use super::pool_test_fixtures::*;
use super::*;

const LAYER: u64 = (EDGE * EDGE) as u64;

fn held(h: &Harness, block: u32) -> Option<BlockPlacement> {
    h.model.demand_room.slot(block).map(placement_of)
}

fn headroom(h: &Harness) -> u64 {
    h.model.plan().report.band_headroom_texels
}

/// No resident rect overlaps a reserved one, and no two reserved ones overlap.
fn assert_room_is_clear(model: &LightmapPoolModel) {
    let rects: Vec<Slot> = (0..model.block_count())
        .filter_map(|block| {
            model
                .demand_room
                .slot(block)
                .or(model.slots[block as usize])
        })
        .collect();
    for (i, a) in rects.iter().enumerate() {
        for b in &rects[i + 1..] {
            let apart = a.layer != b.layer
                || a.x + a.width <= b.x
                || b.x + b.width <= a.x
                || a.y + a.height <= b.y
                || b.y + b.height <= a.y;
            assert!(apart, "{a:?} and {b:?} overlap");
        }
    }
}

// Block 0 (64x40) holds room over the top rows; band block 1 (64x40) cannot
// share the layer with it, and band block 2 fits beneath.
fn one_reservation() -> Harness {
    let mut h = Harness::new(&[(64, 40), (64, 40), (32, 16)], 1);
    let plan = h.drain(&[mandatory(0), band(1, 1), band(2, 2)], &[], &[1, 2]);
    assert_eq!(plan.room_reserved, vec![0]);
    h
}

#[test]
fn reserved_room_stays_a_miss_and_holds_off_band() {
    let mut h = one_reservation();
    let room = held(&h, 0).unwrap();
    let plan = h.model.plan();
    assert_eq!(plan.refused, vec![1], "reserved in this drain");
    assert_eq!(plan.installed, vec![2], "band fits clear of the room");
    assert_eq!(plan.table_writes.len(), 1, "band block 2's alone");
    assert_eq!(plan.table_writes[0].index, 3);
    assert!(plan.copies.is_empty() && plan.evicted.is_empty());
    assert!(!h.model.is_resident(0));
    assert_eq!(
        h.model.table_entry(0),
        BlockTableEntry::Missing {
            width: 64,
            height: 40
        }
    );

    // A later drain, and a draw between: band still finds no way in.
    let plan = h.drain(&[], &[], &[1]);
    assert_eq!(plan.refused, vec![1], "reserved in an earlier drain");
    assert!(plan.table_writes.is_empty() && plan.evicted.is_empty());
    assert!(!h.model.is_resident(0));

    // The pair uploads into the room and turns resident in the drain it
    // arrives, with no repack.
    let plan = h.drain(&[], &[], &[0]);
    assert_eq!(plan.installed, vec![0]);
    assert_eq!(plan.uploads[0].placement, room);
    assert_eq!(plan.table_writes.len(), 1);
    assert!(plan.repack.is_none());
    assert_eq!(h.model.placement(0), Some(room));
    assert_eq!(held(&h, 0), None);
    assert_disjoint_within_layers(&h.model);
}

#[test]
fn room_a_repack_reserved_is_a_miss_and_holds_off_band_in_that_drain() {
    // A resident block sits mid-layer, so neither pair nor target fits until
    // it moves. The repack reserves for block 3; band block 4 must land clear.
    let extents = [(64, 12), (64, 20), (64, 36), (64, 4), (64, 4)];
    let mut h = Harness::new(&extents, 1);
    h.drain(&[mandatory(0)], &[], &[0]);
    h.drain(&[mandatory(1)], &[], &[1]);
    let plan = h.drain(&[mandatory(2), mandatory(3), band(4, 1)], &[0], &[2, 4]);
    assert_eq!(plan.repack, Some(RepackKind::Downward));
    assert_eq!(plan.installed, vec![2, 4]);
    assert_eq!(plan.room_reserved, vec![3]);
    assert!(plan.table_writes.iter().all(|write| write.index != 4));
    assert!(
        plan.copies
            .iter()
            .all(|copy| copy.width == 64 && copy.height == 20)
    );
    assert!(held(&h, 3).is_some() && !h.model.is_resident(3));
    assert_room_is_clear(&h.model);
    let plan = h.drain(&[], &[], &[3]);
    assert!(plan.repack.is_none());
    assert_eq!(plan.installed, vec![3]);
}

#[test]
fn a_compaction_holds_room_for_a_target_without_placing_it() {
    // Crossed layers, as in `compaction_tests`: 32 + 20 wide each. Pair 4 needs
    // a 32 and a 20 to trade, and target 5 needs the gap that leaves.
    let extents = [(32, 56), (20, 56), (32, 56), (20, 56), (24, 56), (64, 8)];
    let mut h = Harness::new(&extents, 2);
    h.drain(&[mandatory(0), mandatory(1)], &[], &[0, 1]);
    h.drain(&[mandatory(2), mandatory(3)], &[], &[2, 3]);
    let plan = h.plan(&[mandatory(4), mandatory(5)], &[], &[4]);
    assert!(matches!(plan.repack, Some(RepackKind::Compaction(_))));
    assert_eq!(plan.installed, vec![4], "5 has room, not a place");
    assert_eq!(plan.uploads.len(), 1);
    assert!(plan.table_writes.iter().all(|write| write.index != 6));
    assert_eq!(plan.room_reserved, vec![5]);
    assert!(!h.model.is_resident(5));
    h.execute();
    assert_room_is_clear(&h.model);
    let room = held(&h, 5).unwrap();
    let plan = h.drain(&[], &[], &[5]);
    assert_eq!(plan.uploads[0].placement, room);
    assert!(plan.repack.is_none() && plan.copies.is_empty());
}

#[test]
fn reservation_returns_when_its_target_leaves() {
    // By removal: the room returns in that drain, and a band pair takes it.
    let mut h = one_reservation();
    let before = headroom(&h);
    let plan = h.drain(&[], &[0], &[1]);
    assert_eq!(plan.installed, vec![1], "band landed in the returned room");
    assert_eq!(plan.room_returned, vec![0]);
    assert!(plan.evicted.is_empty(), "nothing was resident to evict");
    assert_eq!(headroom(&h), before, "band took all the room that returned");
    assert_eq!(held(&h, 0), None);

    // By dropping to band: the same, and its pair arriving later is a band
    // pair, placed clear of the cap like any other.
    let mut h = one_reservation();
    let before = headroom(&h);
    let plan = h.drain(&[band(0, 3)], &[], &[]);
    assert_eq!(plan.room_returned, vec![0]);
    assert_eq!(headroom(&h), before + 64 * 40);
    let plan = h.drain(&[], &[], &[0]);
    assert_eq!(plan.installed, vec![0]);
    assert!(plan.repack.is_none());
    assert_eq!(held(&h, 0), None);

    // An aborted drain restores the reservation, in place.
    let mut h = one_reservation();
    let room = held(&h, 0);
    let before = headroom(&h);
    h.plan(&[], &[0], &[]);
    assert_eq!(held(&h, 0), None);
    h.model.abort_drain();
    assert_eq!(held(&h, 0), room);
    assert_eq!(h.model.plan().report.band_headroom_texels, before);
    let plan = h.drain(&[], &[], &[0]);
    assert_eq!(plan.uploads[0].placement, room.unwrap());
}

#[test]
fn band_headroom_tracks_reservations() {
    let mut h = Harness::new(&[(64, 32), (64, 16)], 1);
    assert_eq!(h.drain(&[], &[], &[]).report.band_headroom_texels, LAYER);

    let plan = h.drain(&[mandatory(0), mandatory(1)], &[], &[]);
    assert_eq!(plan.room_reserved, vec![0, 1]);
    assert_eq!(headroom(&h), LAYER - 64 * 32 - 64 * 16);

    // A pair landing moves its rect from reserved to used: no room comes back.
    let plan = h.drain(&[], &[], &[0]);
    assert_eq!(plan.room_returned, vec![0]);
    assert_eq!(headroom(&h), LAYER - 64 * 32 - 64 * 16);

    // Room returning raises headroom.
    h.drain(&[], &[1], &[]);
    assert_eq!(headroom(&h), LAYER - 64 * 32);
}

#[test]
fn an_aborted_drain_restores_reservations_landings_and_the_compaction_gate() {
    let extents = [(32, 64), (20, 64), (32, 64), (20, 64), (24, 64), (64, 64)];
    let mut h = Harness::new(&extents, 2);
    h.drain(&[mandatory(0), mandatory(1)], &[], &[0, 1]);
    h.drain(&[mandatory(2), mandatory(3)], &[], &[2, 3]);
    let before: Vec<_> = (0..4).map(|block| h.model.placement(block)).collect();

    // A compaction past the pool spends the gate, then the abort restores it.
    h.plan(&[mandatory(4), mandatory(5)], &[], &[4]);
    assert!(h.model.demand_room.compaction_spent);
    h.model.abort_drain();
    assert!(!h.model.demand_room.compaction_spent);
    assert_eq!(
        (0..4)
            .map(|block| h.model.placement(block))
            .collect::<Vec<_>>(),
        before
    );
    h.gpu.assert_matches(&h.model);

    // A landing undone goes back to being reserved room.
    let mut h = one_reservation();
    let room = held(&h, 0);
    h.plan(&[], &[], &[0]);
    assert!(h.model.is_resident(0));
    h.model.abort_drain();
    assert!(!h.model.is_resident(0));
    assert_eq!(held(&h, 0), room);
    h.gpu.assert_matches(&h.model);
}

#[test]
fn room_survives_growth_and_growth_seats_the_targets_it_makes_room_for() {
    let mut h = Harness::new(&[(64, 40), (64, 16), (64, 64), (64, 64), (64, 64)], 1);
    h.drain(&[mandatory(0)], &[], &[0]);
    h.drain(&[mandatory(1)], &[], &[]);
    let room = held(&h, 1);
    let set = [mandatory(2), mandatory(3), mandatory(4)];
    let plan = h.drain(&set, &[], &[2]);
    assert!(plan.growth.is_some());
    assert_eq!(plan.room_reserved, vec![3, 4], "the new layers hold them");
    assert_eq!(held(&h, 1), room);
    let plan = h.drain(&[], &[], &[1, 3, 4]);
    assert!(plan.growth.is_none() && plan.repack.is_none());
    assert_eq!(plan.installed, vec![1, 3, 4]);
    assert_disjoint_within_layers(&h.model);
}

#[test]
fn a_new_controller_hears_of_the_room_held_for_it() {
    use postretro_level_loader::LightmapDrainBatch;

    let mut h = one_reservation();
    let batch = LightmapDrainBatch {
        generation: 2,
        content_tag: [3; 32],
        pool_cap_layers: 1,
        target_reset: Some(vec![mandatory(0)]),
        ..Default::default()
    };
    let plan = h.model.plan_batch_from_empty(&batch);
    assert_eq!(plan.room_reserved, vec![0], "its reset is all it knows");
    assert!(plan.room_returned.is_empty());
    assert!(plan.evicted.is_empty());
}

// Hot path: reservation work rides on target change, never the frame.
#[test]
fn a_drain_with_unchanged_targets_does_no_reservation_work() {
    let mut h = one_reservation();
    let passes = h.model.demand_passes;
    let capacities = h.model.scratch_capacities();
    for _ in 0..64 {
        let plan = h.drain(&[], &[], &[]);
        assert!(plan.room_reserved.is_empty() && plan.room_returned.is_empty());
        assert!(h.model.journal.is_empty(), "nothing moved at all");
    }
    assert_eq!(h.model.demand_passes, passes);
    assert_eq!(h.model.scratch_capacities(), capacities);

    // A pair landing is the one thing an unchanged drain does with room.
    h.drain(&[], &[], &[0]);
    assert_eq!(h.model.demand_passes, passes);

    // A band reclassified in place changes no demand.
    h.drain(&[band(2, 5)], &[], &[]);
    assert_eq!(h.model.demand_passes, passes);

    // A new target is the work's trigger.
    h.drain(&[mandatory(1)], &[], &[]);
    assert_eq!(h.model.demand_passes, passes + 1);
}

#[derive(Debug, Clone)]
struct Step {
    /// `(block, class)`: 0 untargets, 1 mandatory, 2 band.
    changes: Vec<(u32, u8)>,
    /// Bit `b` makes waiting block `b` ready.
    ready: u32,
    abort: bool,
    fail: bool,
}

const BLOCKS: u32 = 12;

fn step() -> impl Strategy<Value = Step> {
    (
        prop::collection::vec((0..BLOCKS, 0u8..3), 0..5),
        any::<u32>(),
        prop::bool::weighted(0.1),
        prop::bool::weighted(0.1),
    )
        .prop_map(|(changes, ready, abort, fail)| Step {
            changes,
            ready,
            abort,
            fail,
        })
}

fn deltas(changes: &[(u32, u8)]) -> (Vec<LightmapTarget>, Vec<u32>) {
    let mut last = std::collections::BTreeMap::new();
    last.extend(changes.iter().copied());
    let set = last
        .iter()
        .filter_map(|(&block, &code)| match code {
            1 => Some(mandatory(block)),
            2 => Some(band(block, block % 5)),
            _ => None,
        })
        .collect();
    let remove = last
        .iter()
        .filter(|&(_, &code)| code == 0)
        .map(|(&block, _)| block)
        .collect();
    (set, remove)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, failure_persistence: None, ..ProptestConfig::default() })]

    /// Any history: reservations stay clear of every placement, only mandatory
    /// targets hold room, no reserved block is sampleable, the plan's deltas
    /// track room held, and aborts undo exactly.
    #[test]
    fn reserved_room_holds_its_invariants_over_any_history(
        extents in prop::collection::vec((1u32..=16, 1u32..=16), BLOCKS as usize),
        layers in 1u32..=3,
        steps in prop::collection::vec(step(), 1..24),
    ) {
        let extents: Vec<_> = extents.iter().map(|&(w, h)| (w * 4, h * 4)).collect();
        let mut h = Harness::new(&extents, layers);
        let mut heard = vec![false; BLOCKS as usize];
        for step in &steps {
            let before: Vec<_> = (0..BLOCKS).map(|block| h.model.demand_room.slot(block)).collect();
            let (set, remove) = deltas(&step.changes);
            let ready: Vec<u32> = (0..BLOCKS)
                .filter(|&block| {
                    step.ready >> block & 1 != 0
                        && !h.model.is_resident(block)
                        && set.iter().all(|t| t.block != block)
                        && !remove.contains(&block)
                        && h.model.target(block).is_some()
                })
                .collect();
            h.plan(&set, &remove, &ready);
            if step.abort {
                h.model.abort_drain();
                let after: Vec<_> = (0..BLOCKS).map(|block| h.model.demand_room.slot(block)).collect();
                prop_assert_eq!(before, after);
            } else {
                if step.fail
                    && let Some(block) = h.model.plan().uploads.first().map(|upload| upload.block)
                {
                    h.model.fail_install(block).unwrap();
                }
                h.execute();
                let plan = h.model.plan();
                for &block in &plan.room_reserved {
                    heard[block as usize] = true;
                }
                for &block in &plan.room_returned {
                    heard[block as usize] = false;
                }
            }
            assert_room_is_clear(&h.model);
            for block in 0..BLOCKS {
                let room = h.model.demand_room.slot(block);
                if !step.abort {
                    prop_assert_eq!(heard[block as usize], room.is_some());
                }
                if room.is_some() {
                    prop_assert!(h.model.is_never_refused(block));
                    prop_assert!(!h.model.is_resident(block));
                    let missing = matches!(h.model.table_entry(block), BlockTableEntry::Missing { .. });
                    prop_assert!(missing);
                }
            }
        }
    }
}
