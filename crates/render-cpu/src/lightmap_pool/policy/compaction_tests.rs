// Compaction tests: escalation from the downward repack, every move shape, rollback to growth.

use log::Level;
use postretro_test_log_capture::LogCapture;

use super::copy_order::{BlockMove, CopyOrderScratch, NoCopyOrder, plan_copy_order};
use super::gpu_mirror::GpuMirror;
use super::pool_test_fixtures::*;
use super::*;

fn at(layer: u32, x: u32, y: u32) -> BlockPlacement {
    BlockPlacement { layer, x, y }
}

fn moved(src: BlockPlacement, dst: BlockPlacement, width: u32, height: u32) -> BlockMove {
    BlockMove {
        src,
        dst,
        width,
        height,
    }
}

/// Plan `moves` and run the copies on a mirror holding each block at its
/// source: every block must land whole, and the mirror refuses a copy within
/// one layer.
fn lands_intact(moves: &[BlockMove], layers: u32) -> CopyOrderStats {
    let mut plan = DrainPlan::default();
    let stats = plan_copy_order(
        moves,
        layers,
        EDGE,
        EDGE,
        false,
        &mut CopyOrderScratch::default(),
        &mut plan.copies,
    )
    .expect("an order exists");
    let mut gpu = GpuMirror::blank(EDGE, layers);
    for (block, m) in moves.iter().enumerate() {
        gpu.fill(block as u32, m.src, m.width, m.height);
    }
    gpu.execute(&plan);
    for (block, m) in moves.iter().enumerate() {
        gpu.assert_holds(block as u32, m.dst, m.width, m.height);
    }
    stats
}

/// Two half-full layers no pending block fits: 32 + 20 texels wide each, with
/// a 24-wide block arriving. Tallest first, the 32s share a layer and the 20s
/// join the arrival, so the 20 in layer 0 and the 32 in layer 1 trade places
/// while each waits on the other.
fn crossed_layers() -> Harness {
    let mut h = Harness::new(&[(32, 64), (20, 64), (32, 64), (20, 64), (24, 64)], 2);
    h.drain(&[mandatory(0), mandatory(1)], &[], &[0, 1]);
    h.drain(&[mandatory(2), mandatory(3)], &[], &[2, 3]);
    assert_eq!(h.model.layers(), 2);
    assert_eq!(h.model.placement(0).unwrap().layer, 0);
    assert_eq!(h.model.placement(2).unwrap().layer, 1);
    h
}

fn compaction_stats(plan: &DrainPlan) -> CopyOrderStats {
    match plan.repack {
        Some(RepackKind::Compaction(stats)) => stats,
        other => panic!("expected a compaction, got {other:?}"),
    }
}

#[test]
fn every_move_shape_lands_intact() {
    let (w, h) = (24, 32);
    let up = moved(at(0, 0, 0), at(1, 8, 8), w, h);
    let down = moved(at(1, 0, 0), at(0, 8, 8), w, h);
    let within_layer = moved(at(0, 0, 0), at(0, 32, 32), w, h);
    let within_itself = moved(at(0, 0, 0), at(0, 4, 4), w, h);
    let swap_in_a_layer = [
        moved(at(0, 0, 0), at(0, 32, 0), 32, 64),
        moved(at(0, 32, 0), at(0, 0, 0), 32, 64),
    ];
    let swap_across_layers = [
        moved(at(0, 0, 0), at(1, 0, 0), 64, 64),
        moved(at(1, 0, 0), at(0, 0, 0), 64, 64),
    ];

    for single in [up, down, within_layer, within_itself] {
        lands_intact(&[single], 2);
    }
    assert_eq!(lands_intact(&swap_in_a_layer, 1).cycles_staged, 1);
    assert_eq!(lands_intact(&swap_across_layers, 2).cycles_staged, 1);
}

#[test]
fn a_three_layer_rotation_of_full_blocks_breaks_with_one_block_staged() {
    let rotation = [
        moved(at(0, 0, 0), at(1, 0, 0), 64, 64),
        moved(at(1, 0, 0), at(2, 0, 0), 64, 64),
        moved(at(2, 0, 0), at(0, 0, 0), 64, 64),
    ];
    let stats = lands_intact(&rotation, 3);
    assert_eq!((stats.cycles_staged, stats.peak_spare_blocks), (1, 1));
}

#[test]
fn small_blockers_share_the_spare_and_held_blocks_stay_untouched() {
    // Two 32-square blocks reverse in layer 0 while a held block keeps the
    // layer's lower half: the spare takes one, and nothing copies the third.
    let moves = [
        moved(at(0, 0, 0), at(0, 32, 0), 32, 32),
        moved(at(0, 32, 0), at(0, 0, 0), 32, 32),
        moved(at(0, 0, 32), at(0, 0, 32), 64, 32),
    ];
    let mut plan = DrainPlan::default();
    plan_copy_order(
        &moves,
        1,
        EDGE,
        EDGE,
        false,
        &mut CopyOrderScratch::default(),
        &mut plan.copies,
    )
    .unwrap();
    assert!(plan.copies.iter().all(|c| c.height == 32 && c.width == 32));
    lands_intact(&moves, 1);
}

#[test]
fn a_blocker_the_spare_cannot_hold_parks_in_free_pool_space() {
    let swap = [
        moved(at(0, 0, 0), at(1, 0, 0), 64, 32),
        moved(at(1, 0, 0), at(0, 0, 0), 64, 32),
    ];
    let mut plan = DrainPlan::default();
    let stats = plan_copy_order(
        &swap,
        3,
        EDGE,
        0,
        false,
        &mut CopyOrderScratch::default(),
        &mut plan.copies,
    )
    .expect("layer 2 is free");
    assert!(stats.parked > 0);
    let mut gpu = GpuMirror::blank(EDGE, 3);
    gpu.fill(0, swap[0].src, 64, 32);
    gpu.fill(1, swap[1].src, 64, 32);
    gpu.execute(&plan);
    gpu.assert_holds(0, swap[0].dst, 64, 32);
    gpu.assert_holds(1, swap[1].dst, 64, 32);
}

/// One real move graph, read from a fixture of `edge layers count` then one
/// `src dst width height` line per block.
fn parse_move_graph(text: &str) -> (u32, u32, Vec<BlockMove>) {
    let mut numbers = text.split_whitespace().map(|n| n.parse::<u32>().unwrap());
    let mut next = || numbers.next().unwrap();
    let (edge, layers, count) = (next(), next(), next());
    let moves = (0..count)
        .map(|_| {
            let src = at(next(), next(), next());
            let dst = at(next(), next(), next());
            moved(src, dst, next(), next())
        })
        .collect();
    (edge, layers, moves)
}

// Captured from a far-point tour of the stress hallway at 27 layers, the one
// rollback that walk had: 247 blocks, 89.8% full, a target that packs. Whether
// an order exists is unknown; thousands of tries through wider budgets found
// none. A search that orders this graph should turn this assertion over.
#[test]
fn a_real_graph_near_full_is_beyond_the_search() {
    let (edge, layers, moves) = parse_move_graph(include_str!(
        "compaction_fixtures/hallway_no_order_found.txt"
    ));
    let mut copies = Vec::new();
    let found = plan_copy_order(
        &moves,
        layers,
        edge,
        edge,
        false,
        &mut CopyOrderScratch::default(),
        &mut copies,
    );
    assert_eq!(found, Err(NoCopyOrder));
    assert!(copies.is_empty());
}

#[test]
fn an_order_not_found_leaves_the_copy_list_as_it_was() {
    let swap = [
        moved(at(0, 0, 0), at(1, 0, 0), 64, 64),
        moved(at(1, 0, 0), at(0, 0, 0), 64, 64),
    ];
    let earlier = PoolCopy {
        src: at(0, 0, 0),
        dst: at(1, 0, 0),
        width: 4,
        height: 4,
    };
    let mut copies = vec![earlier];
    let found = plan_copy_order(
        &swap,
        2,
        EDGE,
        0,
        false,
        &mut CopyOrderScratch::default(),
        &mut copies,
    );
    assert_eq!(found, Err(NoCopyOrder));
    assert_eq!(copies, vec![earlier]);
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
    assert_eq!(plan.repack, Some(RepackKind::Downward));
    assert!(plan.growth.is_none());
    h.execute();
}

#[test]
fn crossed_layers_compact_with_blocks_moving_up_down_and_within_their_layer() {
    let mut h = crossed_layers();
    let (before_1, before_2, before_3) = (
        h.model.placement(1).unwrap(),
        h.model.placement(2).unwrap(),
        h.model.placement(3).unwrap(),
    );
    h.plan(&[mandatory(4)], &[], &[4]);
    let plan = h.model.plan();
    assert!(plan.report.repacked && plan.growth.is_none());
    assert_eq!(plan.installed, vec![4]);
    let stats = compaction_stats(plan);
    assert!(stats.cycles_staged > 0, "the crossed pair waits on itself");
    let (after_1, after_2, after_3) = (
        h.model.placement(1).unwrap(),
        h.model.placement(2).unwrap(),
        h.model.placement(3).unwrap(),
    );
    assert!(after_1.layer > before_1.layer, "moved up");
    assert!(after_2.layer < before_2.layer, "moved down");
    assert_eq!(after_3.layer, before_3.layer, "kept its layer");
    assert_ne!(after_3, before_3, "moved within its layer");
    assert_eq!(h.model.texture_allocations(), 1, "no second pool");
    h.execute();
    assert_disjoint_within_layers(&h.model);
}

#[test]
fn a_compaction_rebuilds_only_the_layers_the_demand_needs() {
    // A full third layer has no room to give, so the round that fits the
    // arrival rebuilds the two crossed layers and never touches it.
    let extents = [(32, 64), (20, 64), (32, 64), (20, 64), (24, 64), (64, 64)];
    let mut h = Harness::new(&extents, 3);
    h.drain(&[mandatory(0), mandatory(1)], &[], &[0, 1]);
    h.drain(&[mandatory(2), mandatory(3)], &[], &[2, 3]);
    h.drain(&[mandatory(5)], &[], &[5]);
    let full = h.model.placement(5).unwrap();
    assert_eq!(full.layer, 2);

    h.plan(&[mandatory(4)], &[], &[4]);
    let plan = h.model.plan();
    compaction_stats(plan);
    assert_eq!(plan.installed, vec![4]);
    assert!(
        plan.copies
            .iter()
            .all(|copy| copy.src.layer != full.layer && copy.dst.layer != full.layer)
    );
    assert_eq!(h.model.placement(5), Some(full));
    h.execute();
    assert_disjoint_within_layers(&h.model);
}

#[test]
fn a_block_the_target_can_leave_in_place_takes_no_copy() {
    // The first 32-wide block stays at the origin of its layer through the
    // relabel and reflow, so no copy reads or writes it.
    let mut h = crossed_layers();
    let held = h.model.placement(0).unwrap();
    h.plan(&[mandatory(4)], &[], &[4]);
    assert_eq!(h.model.placement(0), Some(held));
    let plan = h.model.plan();
    assert!(plan.copies.iter().all(|copy| copy.src != held));
    assert!(plan.table_writes.iter().all(|write| write.index != 1));
    h.execute();
}

// Uploads run after the copies, so an arrival may take a moved block's old rect.
#[test]
fn a_pending_block_lands_in_room_a_compaction_vacated() {
    let mut h = crossed_layers();
    let before: Vec<_> = (0..4)
        .map(|block| h.model.placement(block).unwrap())
        .collect();
    h.drain(&[mandatory(4)], &[], &[4]);
    let arrival = h.model.placement(4).unwrap();
    let on_an_old_rect = (0..4).any(|block| {
        let was = before[block];
        let (width, _) = h.model.alloc_extent(block as u32);
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

#[test]
fn a_band_block_stays_under_the_cap_when_a_pool_past_it_compacts() {
    // Three layers under a cap of two. The tallest-first target wants the
    // full block's layer first, and it sits past the cap; the relabel must not
    // carry the band block there with it.
    let extents = [
        (64, 56),
        (32, 56),
        (20, 56),
        (32, 56),
        (20, 56),
        (24, 56),
        (8, 8),
    ];
    let mut h = Harness::new(&extents, 3);
    h.drain(&[mandatory(1), mandatory(2)], &[], &[1, 2]);
    h.drain(&[mandatory(3), mandatory(4)], &[], &[3, 4]);
    h.drain(&[mandatory(0)], &[], &[0]);
    assert_eq!(h.model.placement(0).unwrap().layer, 2);
    h.cap = 2;
    h.drain(&[band(6, 1)], &[], &[6]);
    h.plan(&[mandatory(5)], &[], &[5]);
    compaction_stats(h.model.plan());
    let band_block = h.model.placement(6).expect("the band block still fits");
    assert!(
        band_block.layer < h.cap,
        "band past the cap: {band_block:?}"
    );
    h.execute();
    assert_disjoint_within_layers(&h.model);
}

#[test]
fn aborting_a_compaction_restores_the_layout_and_records_nothing() {
    let mut h = crossed_layers();
    let before: Vec<_> = (0..4).map(|block| h.model.placement(block)).collect();
    h.plan(&[mandatory(4)], &[], &[4]);
    compaction_stats(h.model.plan());
    h.model.abort_drain();
    assert!(h.model.plan().copies.is_empty());
    assert_eq!(h.model.plan().repack, None);
    let after: Vec<_> = (0..4).map(|block| h.model.placement(block)).collect();
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

// No small layout has been found that a whole spare cannot order; the one
// known miss is the captured real graph, whose pending demand was not kept. So
// this test narrows the spare to nothing: the crossed layers then have no place
// to stage a blocker, and every free span is too narrow to park one.
#[test]
fn no_copy_order_rolls_back_to_growth() {
    let capture = LogCapture::start();
    let mut h = crossed_layers();
    h.model.staging_edge = Some(0);
    let before: Vec<_> = (0..4).map(|block| h.model.placement(block)).collect();
    h.plan(&[mandatory(4)], &[], &[4]);

    let plan = h.model.plan();
    assert!(plan.compaction_rolled_back, "no order through the spare");
    assert_eq!(plan.repack, None);
    assert!(!plan.report.repacked);
    assert!(plan.copies.is_empty(), "no partial copy list");
    assert!(plan.growth.is_some(), "the demand took growth");
    assert_eq!(plan.installed, vec![4]);
    assert!(plan.evicted.is_empty());
    let after: Vec<_> = (0..4).map(|block| h.model.placement(block)).collect();
    assert_eq!(before, after, "the failed repack's layout stands exactly");
    capture.assert_logged_once(
        Level::Warn,
        "Compaction found no copy order through the spare",
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
    assert!(plan.growth.is_some());
    assert!(!plan.compaction_rolled_back);
    capture.assert_not_logged(Level::Warn, "Compaction found no copy order");
}
