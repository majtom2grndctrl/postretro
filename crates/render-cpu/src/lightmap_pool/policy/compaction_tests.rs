// Compaction tests: escalation from the downward repack, every move shape, rollback to growth.

use log::Level;
use postretro_test_log_capture::LogCapture;

use super::copy_order::{BlockMove, CopyOrderInput, CopyOrderScratch, plan_copy_order};
use super::gpu_mirror::GpuMirror;
use super::pool_test_fixtures::*;
use super::*;

fn at(layer: u32, x: u32, y: u32) -> BlockPlacement {
    BlockPlacement { layer, x, y }
}

fn moved(src: BlockPlacement, dst: BlockPlacement, width: u32, height: u32) -> BlockMove {
    BlockMove {
        src: Some(src),
        dst,
        width,
        height,
    }
}

fn input(moves: &[BlockMove], layers: u32) -> CopyOrderInput<'_> {
    CopyOrderInput {
        moves,
        layers,
        edge: EDGE,
    }
}

/// Plan `moves` and run the copies on a mirror holding each block at its
/// source: every block must land whole on its target layer's pool layer, and
/// the mirror refuses a copy within one layer.
fn lands_intact(input: CopyOrderInput<'_>) -> (CopyOrderStats, Vec<PoolCopy>) {
    let mut plan = DrainPlan::default();
    let mut scratch = CopyOrderScratch::default();
    let stats = plan_copy_order(input, &mut scratch, &mut plan.pools[POOL].copies).expect("an order exists");
    let mut gpu = GpuMirror::blank(POOL, EDGE, input.layers);
    for (block, m) in input.moves.iter().enumerate() {
        gpu.fill(POOL, block as u32, m.src.unwrap(), m.width, m.height);
    }
    gpu.execute(&plan);
    for (block, m) in input.moves.iter().enumerate() {
        let landed = BlockPlacement {
            layer: scratch.homes[m.dst.layer as usize],
            ..m.dst
        };
        assert!(
            landed.layer < input.layers,
            "block {block} left on the spare"
        );
        gpu.assert_holds(POOL, block as u32, landed, m.width, m.height);
    }
    (stats, std::mem::take(&mut plan.pools[POOL].copies))
}

/// Two half-full layers no pending block fits: 32 + 20 texels wide each, with
/// a 24-wide block arriving. Tallest first, the 32s share a layer and the 20s
/// join the arrival, so the 20 in layer 0 and the 32 in layer 1 trade places
/// while each waits on the other.
fn crossed_layers() -> Harness {
    let mut h = Harness::new(&[(32, 64), (20, 64), (32, 64), (20, 64), (24, 64)], 2);
    h.drain(&[mandatory(0), mandatory(1)], &[], &[0, 1]);
    h.drain(&[mandatory(2), mandatory(3)], &[], &[2, 3]);
    assert_eq!(h.model.pool(POOL).layers(), 2);
    assert_eq!(h.model.at(0).unwrap().layer, 0);
    assert_eq!(h.model.at(2).unwrap().layer, 1);
    h
}

fn compaction_stats(plan: &DrainPlan) -> CopyOrderStats {
    match plan.pools[POOL].repack {
        Some(RepackKind::Compaction(stats)) => stats,
        other => panic!("expected a compaction, got {other:?}"),
    }
}

#[test]
fn every_move_shape_lands_intact() {
    let (w, h) = (24, 32);
    let to_the_other_layer = moved(at(0, 0, 0), at(1, 8, 8), w, h);
    let from_the_other_layer = moved(at(1, 0, 0), at(0, 8, 8), w, h);
    let within_layer = moved(at(0, 0, 0), at(0, 32, 32), w, h);
    let within_itself = moved(at(0, 0, 0), at(0, 4, 4), w, h);
    let swap_in_a_layer = [
        moved(at(0, 0, 0), at(0, 32, 0), 32, 64),
        moved(at(0, 32, 0), at(0, 0, 0), 32, 64),
    ];
    let swap_across_layers = [
        moved(at(0, 0, 8), at(1, 0, 0), 64, 56),
        moved(at(1, 0, 0), at(0, 0, 8), 64, 56),
    ];

    for single in [
        to_the_other_layer,
        from_the_other_layer,
        within_layer,
        within_itself,
    ] {
        lands_intact(input(&[single], 2));
    }
    lands_intact(input(&swap_in_a_layer, 1));
    lands_intact(input(&swap_across_layers, 2));
}

#[test]
fn full_blocks_that_only_trade_layers_take_no_copy() {
    // Target layers are not tied to pool layers: each block's rect is its own
    // layer's already.
    let rotation = [
        moved(at(0, 0, 0), at(1, 0, 0), 64, 64),
        moved(at(1, 0, 0), at(2, 0, 0), 64, 64),
        moved(at(2, 0, 0), at(0, 0, 0), 64, 64),
    ];
    let (stats, copies) = lands_intact(input(&rotation, 3));
    assert_eq!(stats.moved_blocks, 0);
    assert!(copies.is_empty());
}

#[test]
fn a_target_layer_left_on_the_spare_moves_off_it() {
    // The pair must leave each other's way, so the first target layer takes
    // the empty spare; its pool layer ends up the one its blocks vacated.
    let swap = [
        moved(at(0, 0, 0), at(0, 32, 0), 32, 64),
        moved(at(0, 32, 0), at(0, 0, 0), 32, 64),
    ];
    let (_, copies) = lands_intact(input(&swap, 1));
    assert_eq!(copies.len(), 4);
    assert!(copies[..2].iter().all(|copy| copy.dst.layer == 1));
    assert!(copies[2..].iter().all(|copy| copy.src.layer == 1));
}

#[test]
fn a_layer_that_must_empty_parks_its_blocks_in_free_pool_space() {
    // The 32s build on the spare; the 20s then share neither layer's rects,
    // so one parks in the other's layer to leave a layer empty for them.
    let moves = [
        moved(at(0, 0, 0), at(0, 0, 0), 32, 64),
        moved(at(0, 32, 0), at(1, 0, 0), 20, 64),
        moved(at(1, 0, 0), at(0, 32, 0), 32, 64),
        moved(at(1, 32, 0), at(1, 20, 0), 20, 64),
    ];
    let (stats, _) = lands_intact(input(&moves, 2));
    assert!(stats.parked > 0);
}

#[test]
fn a_downward_repack_that_fits_never_escalates_to_compaction() {
    // Freeing the middle row leaves two 16-tall gaps: the 32-tall arrival
    // fits only once the third block slides up, which a downward pass can do.
    let mut h = Harness::new(&[(64, 24), (64, 16), (64, 8), (64, 32)], 1);
    h.drain(&[mandatory(0), mandatory(1), mandatory(2)], &[], &[0, 1, 2]);
    h.drain(&[], &[1], &[]);
    h.plan(&[mandatory(3)], &[], &[3]);
    let plan = h.model.plan();
    assert_eq!(plan.pools[POOL].repack, Some(RepackKind::Downward));
    assert!(plan.pools[POOL].growth.is_none());
    h.execute();
}

#[test]
fn crossed_layers_compact_by_moving_blocks_up_as_well_as_down() {
    let mut h = crossed_layers();
    let before: Vec<_> = (0..4).map(|b| h.model.at(b).unwrap()).collect();
    h.plan(&[mandatory(4)], &[], &[4]);
    let plan = h.model.plan();
    assert!(plan.report[POOL].repacked && plan.pools[POOL].growth.is_none());
    assert_eq!(plan.installed, vec![4]);
    compaction_stats(plan);
    let after: Vec<_> = (0..4).map(|b| h.model.at(b).unwrap()).collect();
    assert_eq!(after[0].layer, after[2].layer, "the 32s share a layer");
    assert_eq!(after[1].layer, after[3].layer, "the 20s share the other");
    assert!(before.iter().zip(&after).any(|(b, a)| a.layer > b.layer));
    assert!(before.iter().zip(&after).any(|(b, a)| a.layer < b.layer));
    assert_eq!(h.model.pool(POOL).texture_allocations(), 1, "no second pool");
    h.execute();
    assert_disjoint_within_layers(&h.model);
}

// Uploads run after the copies, so an arrival may take a moved block's old rect.
#[test]
fn a_pending_block_lands_in_room_a_compaction_vacated() {
    let mut h = crossed_layers();
    let before: Vec<_> = (0..4)
        .map(|block| h.model.at(block).unwrap())
        .collect();
    h.drain(&[mandatory(4)], &[], &[4]);
    let arrival = h.model.at(4).unwrap();
    let on_an_old_rect = (0..4).any(|block| {
        let was = before[block];
        let (width, _) = h.model.slot_extent(block as u32);
        was.layer == arrival.layer && arrival.x < was.x + width && was.x < arrival.x + 24
    });
    assert!(
        on_an_old_rect,
        "the arrival at {arrival:?} took no vacated room"
    );
}

#[test]
fn compaction_keeps_the_band_blocks_it_can_and_drops_the_rest_from_both_table_and_pool() {
    // Blocks 0 to 3 as in the crossed layers; a band block fills the 12-wide
    // gap of layer 0 and cannot survive the tighter target.
    let extents = [(32, 64), (20, 64), (32, 64), (20, 64), (24, 64), (12, 64)];
    let mut h = Harness::new(&extents, 2);
    h.drain(&[mandatory(0), mandatory(1)], &[], &[0, 1]);
    h.drain(&[mandatory(2), mandatory(3)], &[], &[2, 3]);
    h.drain(&[band(5, 3)], &[], &[5]);
    assert!(h.model.is_resident(5));
    h.plan(&[mandatory(4)], &[], &[4]);
    let plan = h.model.plan();
    compaction_stats(plan);
    assert_eq!(plan.installed, vec![4]);
    assert!(plan.evicted.contains(&PlannedEviction {
        block: 5,
        reason: EvictionReason::Pressure
    }));
    assert!(plan.table_writes.iter().any(|write| write.index == 6));
    h.execute();
    assert!(!h.model.is_resident(5));
}

// Grown or not, a pool is one size: a compaction keeps every band block its
// target layout has room for.
#[test]
fn a_grown_pool_keeps_its_band_through_compaction() {
    let extents = [
        (64, 56),
        (32, 56),
        (20, 56),
        (32, 56),
        (20, 56),
        (24, 56),
        (8, 8),
    ];
    let mut h = Harness::new(&extents, 2);
    h.drain(&[mandatory(1), mandatory(2)], &[], &[1, 2]);
    h.drain(&[mandatory(3), mandatory(4)], &[], &[3, 4]);
    h.drain(&[mandatory(0)], &[], &[0]);
    assert_eq!(h.model.pool(POOL).layers(), 3, "the pool grew");
    assert_eq!(h.model.at(0).unwrap().layer, 2);
    h.model.release_retirement(POOL);
    h.drain(&[band(6, 1)], &[], &[6]);
    h.plan(&[mandatory(5)], &[], &[5]);
    compaction_stats(h.model.plan());
    assert!(h.model.is_resident(6), "the band block stays");
    h.execute();
    assert_disjoint_within_layers(&h.model);
}

#[test]
fn aborting_a_compaction_restores_the_layout_and_records_nothing() {
    let mut h = crossed_layers();
    let before: Vec<_> = (0..4).map(|block| h.model.at(block)).collect();
    h.plan(&[mandatory(4)], &[], &[4]);
    compaction_stats(h.model.plan());
    h.model.abort_drain();
    assert!(h.model.plan().pools[POOL].copies.is_empty());
    assert_eq!(h.model.plan().pools[POOL].repack, None);
    let after: Vec<_> = (0..4).map(|block| h.model.at(block)).collect();
    assert_eq!(before, after);
    assert!(!h.model.is_resident(4));
    h.gpu.assert_matches(&h.model);
}

#[test]
fn a_repeated_compaction_keeps_every_buffer_capacity() {
    let mut h = crossed_layers();
    h.plan(&[mandatory(4)], &[], &[4]);
    compaction_stats(h.model.plan());
    h.model.abort_drain();
    let capacities = h.model.scratch_capacities();
    h.plan(&[mandatory(4)], &[], &[4]);
    compaction_stats(h.model.plan());
    h.model.abort_drain();
    assert_eq!(h.model.scratch_capacities(), capacities);
}

#[test]
fn a_failed_install_after_a_compaction_leaves_every_moved_block_on_its_own_texels() {
    let mut h = crossed_layers();
    h.plan(&[mandatory(4)], &[], &[4]);
    h.model.fail_install(4).unwrap();
    assert_eq!(h.model.plan().failed, vec![4]);
    compaction_stats(h.model.plan());
    h.execute();
    assert!(!h.model.is_resident(4));
}

/// A pool 98% full once the arrival lands, which neither build order can
/// finish. An order exists: the middle target layer first, on the spare. The
/// planner's miss stands in for any layout with no order, so a planner that
/// finds this one needs another layout here.
pub(super) fn a_pool_the_planner_cannot_order(layer_limit: u32) -> Harness {
    let extents = [
        (32, 64),
        (4, 64),
        (24, 64),
        (4, 64),
        (64, 4),
        (56, 48),
        (64, 12),
        (8, 48),
        (64, 24),
        (64, 40),
    ];
    let mut h = Harness::with_layer_limit(&extents, 3, layer_limit);
    let first = [0, 4, 6, 7, 8, 9];
    h.drain(&first.map(mandatory), &[], &first);
    h.drain(&[band(1, 1), band(2, 2), mandatory(5)], &[], &[1, 2, 5]);
    h.drain(&[mandatory(3)], &[1], &[3]);
    h.drain(&[mandatory(1), mandatory(2)], &[3, 8], &[1, 2]);
    h
}

#[test]
fn no_copy_order_rolls_back_to_growth() {
    let capture = LogCapture::start();
    let mut h = a_pool_the_planner_cannot_order(u32::MAX);
    let residents = [0, 1, 2, 4, 5, 6, 7, 9];
    let before = residents.map(|block| h.model.at(block));
    h.plan(&[mandatory(8)], &[], &[8]);

    let plan = h.model.plan();
    assert!(plan.pools[POOL].compaction_rolled_back, "no order through the spare");
    assert_eq!(plan.pools[POOL].repack, None);
    assert!(!plan.report[POOL].repacked);
    assert!(plan.pools[POOL].copies.is_empty(), "no partial copy list");
    assert!(plan.pools[POOL].growth.is_some(), "the demand took growth");
    assert_eq!(plan.installed, vec![8]);
    assert!(plan.evicted.is_empty());
    let after = residents.map(|block| h.model.at(block));
    assert_eq!(before, after, "the failed repack's layout stands exactly");
    capture.assert_logged_once(
        Level::Warn,
        "found no copy order through the spare",
    );
    h.execute();
    assert_disjoint_within_layers(&h.model);
}

#[test]
fn a_demand_that_does_not_pack_grows_without_warning_of_a_copy_order() {
    let capture = LogCapture::start();
    let mut h = Harness::new(&[(64, 64), (64, 64), (64, 64)], 2);
    h.drain(&[mandatory(0), mandatory(1)], &[], &[0, 1]);
    h.plan(&[mandatory(2)], &[], &[2]);
    let plan = h.model.plan();
    assert!(plan.pools[POOL].growth.is_some());
    assert!(!plan.pools[POOL].compaction_rolled_back);
    capture.assert_not_logged(Level::Warn, "found no copy order");
}
