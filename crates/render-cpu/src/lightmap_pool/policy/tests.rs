// Placement policy tests: AC 9, AC 10 (plan half), AC 15 (table half), P6, P7 (model half), P8.

use postretro_level_loader::{LightmapBlockClass, LightmapTarget};

use super::gpu_mirror::GpuMirror;
use super::*;

const EDGE: u32 = 64;
const ALIGN: u32 = 4;

fn target(block: u32, class: LightmapBlockClass, lead: u32) -> LightmapTarget {
    LightmapTarget { block, class, lead }
}

fn mandatory(block: u32) -> LightmapTarget {
    target(block, LightmapBlockClass::Mandatory, 0)
}

fn visible(block: u32) -> LightmapTarget {
    target(block, LightmapBlockClass::Visible, 0)
}

fn band(block: u32, lead: u32) -> LightmapTarget {
    target(block, LightmapBlockClass::Band, lead)
}

/// A model plus the GPU mirror every plan executes on.
struct Harness {
    model: LightmapPoolModel,
    gpu: GpuMirror,
    cap: u32,
}

impl Harness {
    fn new(extents: &[(u32, u32)], cap: u32) -> Self {
        Self::with_layer_limit(extents, cap, u32::MAX)
    }

    fn with_layer_limit(extents: &[(u32, u32)], cap: u32, max_layers: u32) -> Self {
        let extents = extents.to_vec();
        let model =
            LightmapPoolModel::with_layer_limit(extents, ALIGN, EDGE, cap, max_layers).unwrap();
        let gpu = GpuMirror::new(&model);
        Self { model, gpu, cap }
    }

    /// Plan without executing.
    fn plan(&mut self, set: &[LightmapTarget], remove: &[u32], ready: &[u32]) -> &DrainPlan {
        self.model.plan_drain(DrainRequest {
            pool_cap_layers: self.cap,
            target_reset: None,
            target_set: set,
            target_remove: remove,
            ready,
        })
    }

    fn execute(&mut self) {
        self.gpu.execute(self.model.plan());
        self.gpu.assert_matches(&self.model);
    }

    fn drain(&mut self, set: &[LightmapTarget], remove: &[u32], ready: &[u32]) -> &DrainPlan {
        self.plan(set, remove, ready);
        self.execute();
        self.model.plan()
    }

    fn evicted(&self) -> Vec<(u32, EvictionReason)> {
        self.model
            .plan()
            .evicted
            .iter()
            .map(|e| (e.block, e.reason))
            .collect()
    }
}

fn assert_disjoint_within_layers(model: &LightmapPoolModel) {
    let rects: Vec<(u32, Slot)> = model
        .resident_blocks()
        .iter()
        .map(|&block| (block, model.slots[block as usize].unwrap()))
        .collect();
    for (i, &(a, sa)) in rects.iter().enumerate() {
        assert!(
            sa.layer < model.layers(),
            "block {a} past the active layers"
        );
        assert!(sa.x + sa.width <= EDGE && sa.y + sa.height <= EDGE);
        assert_eq!((sa.x % ALIGN, sa.y % ALIGN), (0, 0), "block {a} unaligned");
        for &(b, sb) in &rects[i + 1..] {
            let apart = sa.layer != sb.layer
                || sa.x + sa.width <= sb.x
                || sb.x + sb.width <= sa.x
                || sa.y + sa.height <= sb.y
                || sb.y + sb.height <= sa.y;
            assert!(apart, "blocks {a} and {b} overlap: {sa:?} {sb:?}");
        }
    }
}

/// Three full-layer mandatory blocks and a visible one against a one-layer
/// cap: the first generation holds one layer, and the drain grows to four.
fn grown_past_the_cap() -> Harness {
    let mut h = Harness::new(&[(64, 64), (64, 64), (64, 64), (32, 32)], 1);
    assert_eq!(h.model.layers(), 1, "first generation holds the cap");
    let set = [mandatory(0), mandatory(1), mandatory(2), visible(3)];
    h.plan(&set, &[], &[0, 1, 2, 3]);
    h
}

// AC 9, first half.
#[test]
fn cap_below_the_mandatory_set_grows_and_installs_every_mandatory_and_visible_pair() {
    let mut h = grown_past_the_cap();
    let plan = h.model.plan();
    assert_eq!(plan.installed, vec![0, 1, 2, 3]);
    assert!(plan.refused.is_empty() && plan.deferred.is_empty());
    assert_eq!(
        plan.growth,
        Some(PoolGrowth {
            from_layers: 1,
            to_layers: 4
        })
    );
    assert!(plan.report.grew && !plan.report.repacked && plan.report.retiring);
    assert_eq!(plan.report.layers, 4);
    h.execute();
    assert_eq!(
        h.model.texture_allocations(),
        2,
        "one growth, one generation"
    );
    assert_disjoint_within_layers(&h.model);
}

// AC 9, second half.
#[test]
fn cap_above_the_mandatory_set_refuses_band_past_the_cap_and_frees_untargeted_blocks_next_drain() {
    // All-resident needs three layers; the cap of two bounds the pool.
    let mut h = Harness::new(&[(64, 64), (64, 64), (32, 32)], 2);
    assert_eq!(h.model.layers(), 2);
    let plan = h.drain(&[mandatory(0), band(1, 10), band(2, 5)], &[], &[0, 1, 2]);
    assert_eq!(plan.installed, vec![0, 2], "nearest band lead placed first");
    assert_eq!(plan.refused, vec![1], "no room under the cap for block 1");
    assert!(plan.growth.is_none() && !plan.report.repacked);
    assert_eq!(
        plan.report.band_headroom_texels,
        2 * 64 * 64 - 64 * 64 - 32 * 32
    );

    // Block 2 leaves every target: freed at the next drain, entry cleared.
    h.drain(&[], &[2], &[]);
    assert_eq!(h.evicted(), vec![(2, EvictionReason::Untargeted)]);
    assert_eq!(
        h.model.plan().table_writes,
        vec![TableWrite {
            index: 3,
            entry: BlockTableEntry::Missing {
                width: 32,
                height: 32
            }
        }]
    );
    assert_eq!(h.model.plan().report.band_headroom_texels, 64 * 64);
    assert_eq!(h.model.texture_allocations(), 1);
}

/// Blocks 0 (24 high) and 1 (16 high) stacked, band block 3 below them. Then
/// 0 leaves and 2 (40 high) fits no shelf even with 3 evicted, while 1 and 2
/// together fit the one-layer cap: a repack.
fn fragmented_single_layer() -> Harness {
    let mut h = Harness::new(&[(64, 24), (64, 16), (64, 40), (4, 4)], 1);
    let plan = h.drain(&[mandatory(0), mandatory(1), band(3, 3)], &[], &[0, 1, 3]);
    assert_eq!(plan.installed, vec![0, 1, 3]);
    assert_eq!(h.model.placement(1).map(|p| p.y), Some(24));
    assert_eq!(h.model.placement(3).map(|p| p.y), Some(40));
    h.plan(&[mandatory(2)], &[0], &[2]);
    h
}

fn assert_repack_copies_between_distinct_layers(plan: &DrainPlan) {
    assert!(!plan.copies.is_empty());
    for copy in &plan.copies {
        assert_ne!(copy.src.layer, copy.dst.layer, "{copy:?}");
    }
}

// AC 10, plan half: one layer, so every move stages through the spare.
#[test]
fn repack_compacts_one_layer_through_the_spare_and_allocates_no_second_pool() {
    let mut h = fragmented_single_layer();
    let plan = h.model.plan();
    assert!(plan.report.repacked && !plan.report.grew);
    assert!(!plan.allocates_texture());
    assert_eq!(plan.installed, vec![2]);
    assert!(
        plan.evicted.iter().all(|e| e.block == 0),
        "band block 3 moved, not dropped"
    );
    assert_repack_copies_between_distinct_layers(plan);
    let spare = h.model.spare_layer();
    assert!(
        plan.copies
            .iter()
            .all(|c| c.src.layer == spare || c.dst.layer == spare)
    );
    h.execute();
    assert_eq!(h.model.texture_allocations(), 1, "repack allocated nothing");
    assert_disjoint_within_layers(&h.model);
    assert!(h.model.is_resident(3));
}

/// Two layers of two 32-high blocks each. Blocks 0 and 3 leave, and 4 needs
/// a whole layer: the repack moves 1 within layer 0 (via the spare) and 2
/// down from layer 1 into the region 1 vacates, then 4 uploads into layer 1.
fn fragmented_two_layers() -> Harness {
    let extents = [(64, 32), (64, 32), (64, 32), (64, 32), (64, 64)];
    let mut h = Harness::new(&extents, 2);
    assert_eq!(h.model.layers(), 2);
    let set = [mandatory(0), mandatory(1), mandatory(2), mandatory(3)];
    h.drain(&set, &[], &[0, 1, 2, 3]);
    assert_eq!(h.model.placement(1).map(|p| (p.layer, p.y)), Some((0, 32)));
    assert_eq!(h.model.placement(2).map(|p| (p.layer, p.y)), Some((1, 0)));
    h.plan(&[mandatory(4)], &[0, 3], &[4]);
    h
}

// AC 10, plan half: a move from a higher layer lands where a staged
// same-layer move used to be.
#[test]
fn repack_moves_blocks_down_across_layers_and_uploads_after_the_moves() {
    let mut h = fragmented_two_layers();
    let plan = h.model.plan();
    assert!(plan.report.repacked);
    assert_repack_copies_between_distinct_layers(plan);
    assert_eq!(
        plan.copies.len(),
        3,
        "stage 1, copy 2 down directly, unstage 1"
    );
    assert_eq!(plan.copies[1].src.layer, 1);
    assert_eq!(plan.copies[1].dst.layer, 0);
    h.execute();
    assert_eq!(h.model.placement(4).map(|p| p.layer), Some(1));
    assert_eq!(h.model.texture_allocations(), 1);
    assert_disjoint_within_layers(&h.model);
}

// P6.
#[test]
fn an_evicted_block_turns_non_resident_in_the_plan_that_writes_its_region() {
    let mut h = Harness::new(&[(64, 64), (64, 64)], 1);
    h.drain(&[band(0, 7)], &[], &[0]);
    let a_region = h.model.placement(0).unwrap();
    let plan = h.drain(&[mandatory(1)], &[], &[1]);
    assert_eq!(plan.uploads.len(), 1);
    assert_eq!(plan.uploads[0].placement, a_region, "B lands in A's region");
    assert_eq!(
        plan.table_writes,
        vec![
            TableWrite {
                index: 1,
                entry: BlockTableEntry::Missing {
                    width: 64,
                    height: 64
                }
            },
            TableWrite {
                index: 2,
                entry: BlockTableEntry::Resident {
                    placement: a_region,
                    width: 64,
                    height: 64
                }
            },
        ]
    );
    assert_eq!(h.evicted(), vec![(0, EvictionReason::Pressure)]);
    // The mirror executed both writes with B's texels: no entry reads A
    // through B's texels (assert_matches checked every resident entry).

    // The same holds when A leaves every target instead.
    let mut h = Harness::new(&[(64, 64), (64, 64)], 1);
    h.drain(&[mandatory(0)], &[], &[0]);
    let plan = h.drain(&[mandatory(1)], &[0], &[1]);
    assert_eq!(plan.uploads[0].placement, a_region);
    assert_eq!(plan.table_writes.len(), 2);
    assert_eq!(h.evicted(), vec![(0, EvictionReason::Untargeted)]);
}

// P7, model half: after a repack.
#[test]
fn a_failed_install_after_a_repack_leaves_every_entry_addressing_its_own_block() {
    let mut h = fragmented_two_layers();
    let moved = h.model.plan().copies.clone();
    h.model.fail_install(4).unwrap();
    let plan = h.model.plan();
    assert_eq!(plan.failed, vec![4]);
    assert!(plan.installed.is_empty() && plan.uploads.is_empty());
    assert_eq!(plan.copies, moved, "the repack's moves stand");
    assert!(plan.table_writes.iter().all(|w| w.index != 5));
    assert_eq!(h.model.fail_install(4), Err(NotAPlannedUpload));
    h.execute();
    assert!(!h.model.is_resident(4));
    assert_eq!(
        h.model.placement(2).map(|p| p.layer),
        Some(0),
        "2 moved down"
    );
    assert_disjoint_within_layers(&h.model);
}

// P7, model half: after growth.
#[test]
fn a_failed_install_after_growth_keeps_the_grown_pool_retiring_until_released() {
    let mut h = grown_past_the_cap();
    h.model.fail_install(2).unwrap();
    let plan = h.model.plan();
    assert!(plan.growth.is_some(), "the new generation stands");
    assert_eq!(plan.installed, vec![0, 1, 3]);
    h.execute();
    assert!(h.model.retiring());
    assert_eq!(h.model.layers(), 4);
    assert!(!h.model.is_resident(2));
    assert_disjoint_within_layers(&h.model);

    // A no-op drain does not release it; only the GPU layer's report does.
    h.drain(&[], &[], &[]);
    assert!(h.model.retiring() && h.model.plan().report.retiring);
    h.model.release_retirement();
    let plan = h.drain(&[], &[], &[2]);
    assert!(!plan.report.retiring);
    assert_eq!(plan.installed, vec![2]);
}

#[test]
fn aborting_a_drain_restores_the_model_it_started_from() {
    // The mirror never executed the planned drain, so it holds the state
    // the drain started from.
    for mut h in [fragmented_two_layers(), grown_past_the_cap()] {
        h.model.abort_drain();
        assert!(h.model.plan().uploads.is_empty());
        assert!(!h.model.retiring());
        h.gpu.assert_matches(&h.model);
    }
    let mut h = fragmented_two_layers();
    let before: Vec<_> = h.model.plan().uploads.clone();
    h.model.abort_drain();
    h.plan(&[mandatory(4)], &[0, 3], &[4]);
    assert_eq!(
        h.model.plan().uploads,
        before,
        "replanning is deterministic"
    );
    h.execute();
}

// P8.
#[test]
fn lowering_the_cap_evicts_only_band_blocks_at_or_past_it() {
    let mut h = Harness::new(&[(64, 32); 6], 3);
    assert_eq!(h.model.layers(), 3);
    let set = [
        mandatory(0),
        mandatory(1),
        band(2, 1),
        band(3, 2),
        band(4, 3),
        band(5, 4),
    ];
    h.drain(&set, &[], &[0, 1, 2, 3, 4, 5]);
    let layer = |h: &Harness, b| h.model.placement(b).unwrap().layer;
    assert_eq!(
        (0..6).map(|b| layer(&h, b)).collect::<Vec<_>>(),
        vec![0, 0, 1, 1, 2, 2]
    );
    // Block 4 becomes mandatory where it sits, past the cap to come.
    h.drain(&[mandatory(4)], &[], &[]);
    h.cap = 1;
    h.drain(&[], &[], &[]);
    assert_eq!(
        h.evicted(),
        vec![
            (2, EvictionReason::OverCap),
            (3, EvictionReason::OverCap),
            (5, EvictionReason::OverCap),
        ]
    );
    assert!([0, 1, 4].iter().all(|&b| h.model.is_resident(b)));
    assert_eq!(h.model.layers(), 3, "the texture does not shrink");
}

#[test]
fn band_pairs_never_grow_or_repack_the_pool() {
    let mut h = Harness::new(&[(64, 64), (64, 64), (64, 64)], 1);
    h.drain(&[mandatory(0)], &[], &[0]);
    let plan = h.drain(&[band(1, 1), band(2, 2)], &[], &[1, 2]);
    assert_eq!(plan.refused, vec![1, 2]);
    assert!(plan.growth.is_none() && !plan.report.repacked && plan.copies.is_empty());
    assert_eq!(h.model.texture_allocations(), 1);
}

#[test]
fn a_mandatory_pair_needing_growth_while_a_generation_retires_is_deferred() {
    let mut h = Harness::new(&[(64, 64), (64, 64), (64, 64)], 1);
    let plan = h.drain(&[mandatory(0), mandatory(1)], &[], &[0, 1]);
    assert!(plan.growth.is_some());
    let plan = h.drain(&[visible(2)], &[], &[2]);
    assert_eq!(plan.deferred, vec![2]);
    assert!(plan.installed.is_empty() && plan.refused.is_empty());
    assert!(plan.growth.is_none());
    assert_eq!(h.model.texture_allocations(), 2);
    h.model.release_retirement();
    let plan = h.drain(&[], &[], &[2]);
    assert_eq!(plan.installed, vec![2]);
    assert_eq!(h.model.texture_allocations(), 3);
}

/// Mandatory 0 fills layer 0 and mandatory 1 the top of layer 1 under a
/// two-layer cap; band block 2 sits below block 1. A full-layer mandatory
/// pair then fits nowhere even with block 2 evicted.
fn band_beside_a_full_pool() -> Harness {
    let extents = [(64, 64), (64, 48), (16, 16), (64, 64), (64, 64)];
    let mut h = Harness::new(&extents, 2);
    assert_eq!(h.model.layers(), 2);
    let plan = h.drain(&[mandatory(0), mandatory(1), band(2, 5)], &[], &[0, 1, 2]);
    assert_eq!(plan.installed, vec![0, 1, 2]);
    assert_eq!(h.model.placement(2).map(|p| (p.layer, p.y)), Some((1, 48)));
    h
}

#[test]
fn growth_keeps_the_band_blocks_its_pair_could_not_use() {
    let mut h = band_beside_a_full_pool();
    let band_region = h.model.placement(2);
    let plan = h.drain(&[mandatory(3)], &[], &[3]);
    assert_eq!(
        plan.growth,
        Some(PoolGrowth {
            from_layers: 2,
            to_layers: 3
        })
    );
    assert_eq!(plan.installed, vec![3]);
    assert!(plan.evicted.is_empty(), "{:?}", plan.evicted);
    assert_eq!(h.model.placement(2), band_region, "band block 2 stays put");
    assert_disjoint_within_layers(&h.model);
}

#[test]
fn a_deferral_while_retiring_keeps_the_band_blocks_its_pair_could_not_use() {
    let mut h = band_beside_a_full_pool();
    let band_region = h.model.placement(2);
    h.drain(&[mandatory(3)], &[], &[3]);
    assert!(h.model.retiring());
    let plan = h.drain(&[visible(4)], &[], &[4]);
    assert_eq!(plan.deferred, vec![4]);
    assert!(plan.evicted.is_empty(), "{:?}", plan.evicted);
    assert!(plan.table_writes.is_empty() && plan.uploads.is_empty());
    assert!(plan.growth.is_none() && !plan.report.repacked);
    assert_eq!(h.model.placement(2), band_region, "band block 2 stays put");
}

// Block 0 fills the top of the one layer beside far band block 2; near band
// block 1 fills the bottom. A full-width pair evicts block 2 first (farthest
// lead), which frees nothing it can use, then block 1: block 2 goes back.
#[test]
fn a_pair_keeps_only_the_evictions_its_slot_needs() {
    let mut h = Harness::new(&[(48, 32), (64, 32), (16, 16), (64, 32)], 1);
    let plan = h.drain(&[mandatory(0), band(1, 1), band(2, 9)], &[], &[0, 1, 2]);
    assert_eq!(plan.installed, vec![0, 1, 2]);
    let far = h.model.placement(2);
    assert_eq!(far.map(|p| (p.x, p.y)), Some((48, 0)));
    let near = h.model.placement(1);
    assert_eq!(near.map(|p| p.y), Some(32));

    let plan = h.drain(&[mandatory(3)], &[], &[3]);
    assert_eq!(plan.installed, vec![3]);
    assert_eq!(
        plan.uploads[0].placement,
        near.unwrap(),
        "3 lands in 1's region"
    );
    assert!(
        plan.table_writes.iter().all(|w| w.index != 3),
        "block 2's entry never changed"
    );
    assert_eq!(h.evicted(), vec![(1, EvictionReason::Pressure)]);
    assert_eq!(h.model.placement(2), far, "block 2 reinstated where it was");
    assert_disjoint_within_layers(&h.model);
}

// A pool grown past its one-layer cap holds block 0 on layer 0 and block 2
// mid-layer 1. Block 1 leaves and block 3 (40 tall) fits no shelf of layer
// 1, nor anything under the cap, but layer 1 compacted holds 3 and 2: the
// drain repacks within its two layers rather than growing a third.
#[test]
fn a_pool_grown_past_the_cap_repacks_within_its_layers_before_growing() {
    let mut h = Harness::new(&[(64, 64), (64, 24), (64, 16), (64, 40)], 1);
    let plan = h.drain(&[mandatory(0), mandatory(1), mandatory(2)], &[], &[0, 1, 2]);
    assert_eq!(plan.installed, vec![0, 1, 2]);
    assert_eq!(h.model.layers(), 2);
    assert_eq!(h.model.placement(2).map(|p| (p.layer, p.y)), Some((1, 24)));
    h.model.release_retirement();

    let plan = h.drain(&[mandatory(3)], &[1], &[3]);
    assert!(plan.report.repacked, "compacted in place");
    assert!(plan.growth.is_none() && !plan.allocates_texture());
    assert_eq!(plan.installed, vec![3]);
    assert!(plan.deferred.is_empty());
    assert_repack_copies_between_distinct_layers(plan);
    assert_eq!(h.evicted(), vec![(1, EvictionReason::Untargeted)]);
    assert_eq!(h.model.layers(), 2);
    assert_eq!(h.model.texture_allocations(), 2, "one growth, no second");
    assert_eq!(h.model.placement(3).map(|p| (p.layer, p.y)), Some((1, 0)));
    assert_eq!(h.model.placement(2).map(|p| (p.layer, p.y)), Some((1, 40)));
    assert_disjoint_within_layers(&h.model);
}

#[test]
fn a_pair_needing_a_layer_past_the_device_limit_is_deferred_not_grown() {
    let first = LightmapPoolModel::with_layer_limit(vec![(64, 64); 3], ALIGN, EDGE, 5, 2);
    assert_eq!(
        first.map(|m| m.layers()),
        Some(2),
        "the first generation too"
    );

    let mut h = Harness::with_layer_limit(&[(64, 64); 3], 1, 2);
    let set = [mandatory(0), mandatory(1), mandatory(2)];
    let plan = h.drain(&set, &[], &[0, 1, 2]);
    assert_eq!(plan.installed, vec![0, 1]);
    assert_eq!(plan.deferred, vec![2]);
    assert_eq!(
        plan.growth,
        Some(PoolGrowth {
            from_layers: 1,
            to_layers: 2
        })
    );

    // With the retirement released, the limit alone still defers it.
    h.model.release_retirement();
    let plan = h.drain(&[], &[], &[2]);
    assert_eq!(plan.deferred, vec![2]);
    assert!(plan.growth.is_none() && plan.installed.is_empty());
    assert_eq!(h.model.texture_allocations(), 2);

    let plan = h.drain(&[], &[1], &[2]);
    assert_eq!(plan.installed, vec![2]);
    assert_eq!(h.model.layers(), 2);
    assert_disjoint_within_layers(&h.model);
}

// One usable layer at the device limit: mandatory block 0 fills the top-left,
// band block 1 the bottom shelf. Full-layer block 2 is deferred at the limit.
// Block 5 is as large as 2, so it skips the victim walk and is deferred.
// Block 3 is smaller: it still walks, evicting band block 1, so an oversized
// pair that stays wanted never starves it. Block 4 fits free space beside 0.
#[test]
fn after_a_deferral_at_the_device_limit_only_pairs_as_large_skip_the_victim_walk() {
    let extents = [(48, 32), (64, 32), (64, 64), (64, 32), (16, 16), (64, 64)];
    let mut h = Harness::with_layer_limit(&extents, 1, 1);
    let plan = h.drain(&[mandatory(0), band(1, 1)], &[], &[0, 1]);
    assert_eq!(plan.installed, vec![0, 1]);

    let set = [mandatory(2), mandatory(3), mandatory(4), mandatory(5)];
    let plan = h.drain(&set, &[], &[2, 3, 4, 5]);
    assert!(plan.device_limited);
    let mut deferred = plan.deferred.clone();
    deferred.sort_unstable();
    assert_eq!(
        deferred,
        vec![2, 5],
        "5 is as large as 2 and skips the walk"
    );
    let mut installed = plan.installed.clone();
    installed.sort_unstable();
    assert_eq!(installed, vec![3, 4], "3 walks and evicts band block 1");
    assert!(plan.growth.is_none());
    assert_eq!(h.evicted(), vec![(1, EvictionReason::Pressure)]);
    assert_disjoint_within_layers(&h.model);

    // Blocks 2 and 5 stay wanted and come back ready: still deferred, and
    // nothing already placed moves.
    let plan = h.drain(&[], &[], &[2, 5]);
    assert!(plan.device_limited);
    let mut deferred = plan.deferred.clone();
    deferred.sort_unstable();
    assert_eq!(deferred, vec![2, 5]);
    assert!(h.model.is_resident(3) && h.model.is_resident(4));
    assert_disjoint_within_layers(&h.model);

    // The skip lasts one drain: with the layer holding only band block 1,
    // block 5 walks the next drain and evicts it.
    let plan = h.drain(&[band(1, 1)], &[0, 2, 3, 4], &[1]);
    assert_eq!(plan.installed, vec![1]);
    let plan = h.drain(&[], &[], &[5]);
    assert!(!plan.device_limited);
    assert_eq!(plan.installed, vec![5]);
    assert_eq!(h.evicted(), vec![(1, EvictionReason::Pressure)]);
    assert_disjoint_within_layers(&h.model);
}

// One usable layer at the device limit, two 32-high shelves: mandatory block
// 0 and band block 1 fill the top one, mandatory block 2 and band block 3 the
// bottom one. Block 4 (48x64) and block 5 (64x32) are deferred at the limit.
// Block 6 (56x32) contains neither extent, though it contains their
// componentwise minimum (48x32): it walks and fits where band block 1 was.
#[test]
fn a_pair_containing_no_single_deferred_extent_still_walks_at_the_device_limit() {
    let extents = [
        (8, 32),
        (56, 32),
        (16, 32),
        (48, 32),
        (48, 64),
        (64, 32),
        (56, 32),
    ];
    let mut h = Harness::with_layer_limit(&extents, 1, 1);
    let plan = h.drain(&[mandatory(0), band(1, 1)], &[], &[0, 1]);
    assert_eq!(plan.installed, vec![0, 1]);
    let plan = h.drain(&[mandatory(2), band(3, 2)], &[], &[2, 3]);
    assert_eq!(plan.installed, vec![2, 3]);
    let shelf = |h: &Harness, block: usize| h.model.slots[block].unwrap().y;
    assert_eq!((shelf(&h, 0), shelf(&h, 1)), (0, 0));
    assert_eq!((shelf(&h, 2), shelf(&h, 3)), (32, 32));

    let set = [mandatory(4), mandatory(5), mandatory(6)];
    let plan = h.drain(&set, &[], &[4, 5, 6]);
    assert!(plan.device_limited);
    assert_eq!(plan.deferred, vec![4, 5]);
    assert_eq!(plan.installed, vec![6], "6 walks and evicts band block 1");
    assert_eq!(h.evicted(), vec![(1, EvictionReason::Pressure)]);
    assert!(h.model.is_resident(3), "band block 3 is put back");
    assert_disjoint_within_layers(&h.model);

    // Blocks 4 and 5 come back ready and stay deferred; 6 stays resident.
    let plan = h.drain(&[], &[], &[4, 5]);
    assert_eq!(plan.deferred, vec![4, 5]);
    assert!(h.model.is_resident(6));
    assert_disjoint_within_layers(&h.model);
}

// A new generation's first batch starts from nothing resident: every entry
// turns non-resident in the same plan, and no free is reported evicted.
#[test]
fn a_batch_from_empty_frees_every_block_without_reporting_evictions() {
    use postretro_level_format::lightmap::LightmapBlockPayload;
    use postretro_level_loader::{LightmapDrainBatch, PreparedLightmapBlock};

    let new_generation = || LightmapDrainBatch {
        generation: 2,
        content_tag: [3; 32],
        pool_cap_layers: 1,
        target_reset: Some(vec![mandatory(1), mandatory(2)]),
        ready: vec![PreparedLightmapBlock {
            generation: 2,
            content_tag: [3; 32],
            block: 2,
            payload: LightmapBlockPayload::default(),
        }],
        ..Default::default()
    };
    let installed = || {
        let mut h = Harness::new(&[(16, 16); 3], 1);
        h.drain(&[mandatory(0), band(1, 3)], &[], &[0, 1]);
        h
    };

    let mut h = installed();
    let plan = h.model.plan_batch_from_empty(&new_generation());
    assert!(plan.evicted.is_empty(), "{:?}", plan.evicted);
    assert_eq!(
        plan.installed,
        vec![2],
        "block 1 was resident, not re-reported"
    );
    let missing = BlockTableEntry::Missing {
        width: 16,
        height: 16,
    };
    assert_eq!(plan.table_writes.len(), 3);
    assert_eq!(plan.table_writes[0].entry, missing);
    assert_eq!(plan.table_writes[1].entry, missing);
    h.execute();
    assert_eq!(h.model.resident_blocks(), &[2]);
    assert_eq!(h.model.texture_allocations(), 1, "the textures stand");

    // An aborted reset restores the residency it started from.
    let mut h = installed();
    h.model.plan_batch_from_empty(&new_generation());
    h.model.abort_drain();
    assert!(h.model.is_resident(0) && h.model.is_resident(1));
    h.gpu.assert_matches(&h.model);
}

// AC 15, table half: only changed entries are written.
#[test]
fn a_target_change_writes_only_the_entries_that_changed() {
    let mut h = Harness::new(&[(16, 16); 8], 2);
    let set: Vec<_> = (0..8).map(mandatory).collect();
    let plan = h.drain(&set, &[], &(0..8).collect::<Vec<_>>());
    assert_eq!(plan.table_writes.len(), 8);
    let plan = h.drain(&[], &[5], &[]);
    assert_eq!(plan.table_writes.len(), 1);
    assert_eq!(plan.table_writes[0].index, 6);
    // Reclassifying a resident block changes no entry.
    let plan = h.drain(&[band(3, 2)], &[], &[]);
    assert!(plan.table_writes.is_empty());
}

// Hot path: a steady drain allocates nothing and writes nothing.
#[test]
fn steady_drains_write_no_table_entries_and_keep_every_buffer_capacity() {
    let mut h = fragmented_two_layers();
    h.execute();
    h.drain(&[band(1, 4)], &[], &[]);
    let capacities = h.model.scratch_capacities();
    for _ in 0..64 {
        let plan = h.drain(&[], &[], &[]);
        assert!(plan.table_writes.is_empty());
        assert!(plan.uploads.is_empty() && plan.copies.is_empty() && plan.evicted.is_empty());
        assert!(plan.growth.is_none() && !plan.report.repacked);
    }
    assert_eq!(h.model.scratch_capacities(), capacities);
}

#[test]
fn placements_land_on_the_block_alignment_for_unaligned_extents() {
    let mut h = Harness::new(&[(6, 6), (5, 9), (13, 3), (4, 4)], 1);
    let set: Vec<_> = (0..4).map(mandatory).collect();
    h.drain(&set, &[], &[0, 1, 2, 3]);
    assert_disjoint_within_layers(&h.model);
}

#[test]
fn a_ready_pair_for_an_untargeted_block_is_refused_and_a_resident_one_reports_installed() {
    let mut h = Harness::new(&[(16, 16), (16, 16)], 1);
    h.drain(&[mandatory(0)], &[], &[0]);
    let plan = h.drain(&[], &[], &[0, 1]);
    assert_eq!(plan.installed, vec![0]);
    assert!(plan.uploads.is_empty());
    assert_eq!(plan.refused, vec![1]);
}

#[test]
fn a_model_rejects_a_block_larger_than_a_layer_or_zero_sized() {
    assert!(LightmapPoolModel::new(vec![(68, 4)], ALIGN, EDGE, 1).is_none());
    assert!(LightmapPoolModel::new(vec![(0, 4)], ALIGN, EDGE, 1).is_none());
    let empty = LightmapPoolModel::new(Vec::new(), ALIGN, EDGE, 15).unwrap();
    assert_eq!((empty.layers(), empty.texture_allocations()), (0, 1));
}

#[test]
fn plan_batch_plans_the_controller_batch_from_its_ready_block_ids() {
    use postretro_level_format::lightmap::LightmapBlockPayload;
    use postretro_level_loader::{LightmapDrainBatch, PreparedLightmapBlock};

    let mut h = Harness::new(&[(64, 64), (64, 64)], 1);
    let prepared = |block| PreparedLightmapBlock {
        generation: 1,
        content_tag: [3; 32],
        block,
        payload: LightmapBlockPayload::default(),
    };
    let batch = LightmapDrainBatch {
        generation: 1,
        content_tag: [3; 32],
        pool_cap_layers: 1,
        target_set: vec![mandatory(0), band(1, 2)],
        ready: vec![prepared(1), prepared(0)],
        ..Default::default()
    };
    batch.validate_contract(2, [3; 32]).unwrap();
    let plan = h.model.plan_batch(&batch);
    assert_eq!(plan.installed, vec![0]);
    assert_eq!(plan.refused, vec![1], "the band pair finds the cap full");
    h.execute();
}
