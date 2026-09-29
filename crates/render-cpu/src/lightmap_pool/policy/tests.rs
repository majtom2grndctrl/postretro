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
        let model = LightmapPoolModel::new(extents.to_vec(), ALIGN, EDGE, cap).unwrap();
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
