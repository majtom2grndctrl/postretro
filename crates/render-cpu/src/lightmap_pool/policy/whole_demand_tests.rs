// Whole-demand repacks: the downward pass judged on every target, one repack per demand, the compaction gate.

use log::Level;
use postretro_test_log_capture::LogCapture;

use super::pool_test_fixtures::*;
use super::*;

const LEAVES: u32 = 2;
const PAIR: u32 = 5;
const WAITING: u32 = 6;

/// The room each drain's pair needs is there only once a block slides over,
/// which a downward pass does. Block 6 fits only if block 1 moves up beside
/// block 4, since block 4 is too tall for block 1's row: no downward pass can
/// seat it, and a compaction can.
///
/// ```text
/// layer 0   0 (64x44)                 layer 1   2 (8x40) 3 (48x40) free 8
///           1 (24x20)  free 40                  4 (40x24)  free 24
/// ```
fn two_rows() -> Harness {
    let extents = [
        (64, 44),
        (24, 20),
        (8, 40),
        (48, 40),
        (40, 24),
        (16, 40),
        (64, 20),
    ];
    let mut h = Harness::new(&extents, 2);
    h.drain(&[mandatory(0), mandatory(1)], &[], &[0, 1]);
    h.drain(&[mandatory(2)], &[], &[2]);
    h.drain(&[mandatory(3), mandatory(4)], &[], &[3, 4]);
    assert_eq!(h.model.placement(1).unwrap().layer, 0);
    assert_eq!(h.model.placement(3).unwrap().x, 8);
    assert_eq!(h.model.placement(4).unwrap().layer, 1);
    h
}

fn layers_of(h: &Harness, blocks: &[u32]) -> Vec<u32> {
    blocks
        .iter()
        .map(|&block| h.model.placement(block).unwrap().layer)
        .collect()
}

fn is_compaction(plan: &DrainPlan) -> bool {
    matches!(plan.repack, Some(RepackKind::Compaction(_)))
}

#[test]
fn downward_repack_must_cover_the_whole_demand() {
    // The whole demand fits downward, so nothing moves up. Then a target joins
    // that only a move up can seat: that drain compacts (R10).
    let mut h = two_rows();
    let residents = [0, 1, 3, 4];
    let before = layers_of(&h, &residents);
    let plan = h.drain(&[mandatory(PAIR)], &[LEAVES], &[PAIR]);
    assert_eq!(plan.repack, Some(RepackKind::Downward));
    assert_eq!(plan.installed, vec![PAIR]);
    assert_eq!(layers_of(&h, &residents), before, "no block moved up");

    let plan = h.drain(&[mandatory(WAITING)], &[], &[WAITING]);
    assert!(is_compaction(plan), "{:?}", plan.repack);
    assert_eq!(plan.installed, vec![WAITING]);
    assert!(plan.growth.is_none());
    assert!(
        layers_of(&h, &[1])[0] > before[1],
        "block 1 had to move up beside block 4"
    );
    assert_disjoint_within_layers(&h.model);

    // The same pair fits downward alone, but the demand behind it does not:
    // the downward pass counts as failed and compaction runs in this drain,
    // seating the target whose pair is still to come (R18).
    let mut h = two_rows();
    let plan = h.drain(&[mandatory(PAIR), mandatory(WAITING)], &[LEAVES], &[PAIR]);
    assert!(is_compaction(plan), "{:?}", plan.repack);
    assert_eq!(plan.installed, vec![PAIR]);
    assert!(plan.growth.is_none());
    assert_eq!(plan.room_reserved, vec![WAITING]);
    assert_disjoint_within_layers(&h.model);
    let plan = h.drain(&[], &[], &[WAITING]);
    assert_eq!(plan.repack, None, "the compaction seated it");
    assert_eq!(plan.installed, vec![WAITING]);
}

/// R20's sizes, in the order its pairs arrive.
const R20: [(u32, u32); 3] = [(28, 36), (8, 28), (4, 44)];

/// Feed `order`, one pair a drain, counting repacks; a drain's copies stop
/// with the last repack, and no drain grows the pool or defers a pair.
fn repacks_placing(h: &mut Harness, order: &[u32], first_set: &[LightmapTarget]) -> u32 {
    let mut repacks = 0;
    for (at, &block) in order.iter().enumerate() {
        let set = if at == 0 { first_set } else { &[] };
        let plan = h.drain(set, &[], &[block]);
        assert_eq!(plan.installed, vec![block]);
        assert!(plan.growth.is_none() && plan.deferred.is_empty());
        if plan.repack.is_some() {
            repacks += 1;
        } else {
            assert!(plan.copies.is_empty(), "no repack, so no copy");
        }
    }
    assert_disjoint_within_layers(&h.model);
    repacks
}

#[test]
fn demand_arriving_over_drains_repacks_once() {
    // A fragmented pool: the first pair finds no room, the first repack
    // reserves for every target, and the later pairs land there (R11, R20).
    let extents = [(64, 12), (64, 20), R20[0], R20[1], R20[2]];
    let mut h = Harness::new(&extents, 1);
    h.drain(&[mandatory(0)], &[], &[0]);
    h.drain(&[mandatory(1)], &[], &[1]);
    h.drain(&[], &[0], &[]);
    let demand = [mandatory(2), mandatory(3), mandatory(4)];
    let plan = h.plan(&demand, &[], &[2]);
    assert_eq!(plan.repack, Some(RepackKind::Downward));
    assert_eq!(plan.room_reserved, vec![3, 4]);
    h.model.abort_drain();
    assert_eq!(repacks_placing(&mut h, &[2, 3, 4], &demand), 1);

    // The first pair already resident when the rest arrive: its repack comes
    // with the last pair, and there is only that one.
    let mut h = Harness::new(&R20, 1);
    h.drain(&[mandatory(0)], &[], &[0]);
    let later = [mandatory(1), mandatory(2)];
    assert_eq!(repacks_placing(&mut h, &[1, 2], &later), 1);

    // An empty pool: the targets' room is free as it stands, tallest first, so
    // the same arrival takes no repack at all. One is a ceiling, not a floor.
    let mut h = Harness::new(&R20, 1);
    let all = [mandatory(0), mandatory(1), mandatory(2)];
    assert_eq!(repacks_placing(&mut h, &[0, 1, 2], &all), 0);
}

/// Two half-full layers no arrival fits: 32 + 20 wide each, so a 24-wide block
/// needs a compaction to trade a 20 for a 32. Block 5 fills a whole layer, so
/// demand with it is past the pool even compacted.
fn crossed_layers_with_a_block_past_them(layer_limit: u32) -> Harness {
    let extents = [
        (32, 64),
        (20, 64),
        (32, 64),
        (20, 64),
        (24, 64),
        (64, 64),
        (64, 64),
        (64, 64),
    ];
    let mut h = Harness::with_layer_limit(&extents, 2, layer_limit);
    h.drain(&[mandatory(0), mandatory(1)], &[], &[0, 1]);
    h.drain(&[mandatory(2), mandatory(3)], &[], &[2, 3]);
    h
}

#[test]
fn demand_past_the_pool_compacts_once_then_grows() {
    // Both targets arrive first: the first pair fits by compaction, and the
    // demand past the pool does not grow while no pair needs it (R19).
    let mut h = crossed_layers_with_a_block_past_them(u32::MAX);
    let plan = h.drain(&[mandatory(4), mandatory(5)], &[], &[4]);
    assert!(is_compaction(plan), "{:?}", plan.repack);
    assert_eq!(plan.installed, vec![4]);
    assert!(plan.growth.is_none());
    let plan = h.drain(&[], &[], &[]);
    assert_eq!(plan.repack, None);
    assert!(plan.growth.is_none(), "no ready pair needs it");

    // The later pair does not fit as the pool stands: growth, no second
    // compaction (R32).
    let plan = h.drain(&[], &[], &[5]);
    assert!(!is_compaction(plan), "{:?}", plan.repack);
    assert!(plan.growth.is_some());
    assert_eq!(plan.installed, vec![5]);
    assert_disjoint_within_layers(&h.model);

    // The past-the-pool target arrives with its own pair, after the first
    // compaction seated all there was: one compaction still, since that
    // demand does not pack.
    let mut h = crossed_layers_with_a_block_past_them(u32::MAX);
    assert!(is_compaction(h.drain(&[mandatory(4)], &[], &[4])));
    let plan = h.drain(&[mandatory(5)], &[], &[5]);
    assert!(!is_compaction(plan), "{:?}", plan.repack);
    assert!(plan.growth.is_some());
    assert_eq!(plan.installed, vec![5]);
}

// Regression: a jam reran compaction, and its warning, on every drain.
#[test]
fn a_jam_at_the_device_limit_compacts_once_and_repacks_no_more() {
    let mut h = crossed_layers_with_a_block_past_them(2);
    let plan = h.drain(&[mandatory(4), mandatory(5)], &[], &[4]);
    assert_eq!(plan.installed, vec![4]);
    assert!(is_compaction(plan), "{:?}", plan.repack);
    let passes = h.model.demand_passes;
    for _ in 0..8 {
        let plan = h.drain(&[], &[], &[5]);
        assert_eq!(plan.deferred, vec![5]);
        assert!(plan.repack.is_none() && plan.copies.is_empty());
        assert!(plan.device_limited);
    }
    assert_eq!(h.model.demand_passes, passes, "no reservation work");
}

#[test]
fn a_jam_behind_a_retiring_generation_repacks_once_and_then_grows() {
    // Two layers plus headroom: three. Block 3 is past the reserve.
    let mut h = Harness::new(&[(64, 64); 4], 1);
    h.drain(&[mandatory(0), mandatory(1)], &[], &[0, 1]);
    let plan = h.drain(&[mandatory(2), mandatory(3)], &[], &[2, 3]);
    assert_eq!(plan.installed, vec![2]);
    assert_eq!(plan.deferred, vec![3]);
    assert!(h.model.demand_room.compaction_spent);
    let plan = h.drain(&[], &[], &[3]);
    assert_eq!(plan.deferred, vec![3]);
    assert!(plan.repack.is_none());
    h.model.release_retirement();
    let plan = h.drain(&[], &[], &[3]);
    assert!(plan.growth.is_some(), "the retirement was all it waited on");
    assert_eq!(plan.installed, vec![3]);
}

#[test]
fn a_spent_demand_ends_when_the_targets_pack_again() {
    let mut h = crossed_layers_with_a_block_past_them(2);
    h.drain(&[mandatory(4), mandatory(5)], &[], &[4, 5]);
    assert!(h.model.demand_room.compaction_spent);

    // Another target joins a demand still past the pool: the same demand.
    let plan = h.drain(&[mandatory(6)], &[], &[5, 6]);
    assert!(plan.repack.is_none() && plan.copies.is_empty());
    assert!(h.model.demand_room.compaction_spent);

    // What is left packs: a new demand, free to compact.
    h.drain(&[], &[5, 6], &[]);
    assert!(!h.model.demand_room.compaction_spent);
}

#[test]
fn a_second_demand_past_a_grown_pool_compacts_and_grows_again() {
    let mut extents = vec![(32, 64), (20, 64), (32, 64), (20, 64), (24, 64)];
    extents.extend([(64, 64); 8]);
    let mut h = Harness::new(&extents, 2);
    h.drain(&[mandatory(0), mandatory(1)], &[], &[0, 1]);
    h.drain(&[mandatory(2), mandatory(3)], &[], &[2, 3]);
    let first: Vec<_> = (4..6).map(mandatory).collect();
    let plan = h.drain(&first, &[], &[4]);
    assert!(is_compaction(plan));
    assert!(h.model.demand_room.compaction_spent);
    let plan = h.drain(&[], &[], &[5]);
    assert!(plan.growth.is_some());
    assert!(!h.model.demand_room.compaction_spent, "the pool is new");
    h.model.release_retirement();
    let layers = h.model.layers();

    let second: Vec<_> = (6..13).map(mandatory).collect();
    let ready: Vec<u32> = (6..13).collect();
    let plan = h.drain(&second, &[], &ready);
    assert!(plan.growth.is_some(), "grown again, for a new demand");
    assert_eq!(plan.installed, ready);
    assert!(h.model.layers() > layers);
    assert_eq!(h.model.texture_allocations(), 3);
    assert_disjoint_within_layers(&h.model);
}

/// `compaction_tests`' no-order pool is a jam once the device stops growth.
#[test]
fn a_demand_with_no_copy_order_warns_once() {
    let capture = LogCapture::start();
    let mut h = super::compaction_tests::a_pool_the_planner_cannot_order(3);
    h.plan(&[mandatory(8)], &[], &[8]);
    assert!(h.model.plan().compaction_rolled_back);
    assert_eq!(h.model.plan().deferred, vec![8]);
    h.execute();
    for _ in 0..4 {
        let plan = h.drain(&[], &[], &[8]);
        assert!(!plan.compaction_rolled_back);
        assert_eq!(plan.deferred, vec![8]);
    }
    capture.assert_logged_once(
        Level::Warn,
        "Compaction found no copy order through the spare",
    );
}
