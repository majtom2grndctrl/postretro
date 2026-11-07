// Controller batches into the model: planning a batch, and a new generation's first one.

use postretro_level_format::lightmap::LightmapBlockPayload;
use postretro_level_loader::{LightmapDrainBatch, PreparedLightmapBlock};

use super::pool_test_fixtures::*;
use super::*;

// A new generation's first batch starts from nothing resident: every entry
// turns non-resident in the same plan, and no free is reported evicted.
#[test]
fn a_batch_from_empty_frees_every_block_without_reporting_evictions() {
    let new_generation = || LightmapDrainBatch {
        generation: 2,
        content_tag: [3; 32],
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
    assert_eq!(h.model.pool(POOL).texture_allocations(), 1, "the textures stand");

    // An aborted reset restores the residency it started from.
    let mut h = installed();
    h.model.plan_batch_from_empty(&new_generation());
    h.model.abort_drain();
    assert!(h.model.is_resident(0) && h.model.is_resident(1));
    h.gpu.assert_matches(&h.model);
}

#[test]
fn plan_batch_plans_the_controller_batch_from_its_ready_block_ids() {
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
        target_set: vec![mandatory(0), band(1, 2)],
        ready: vec![prepared(1), prepared(0)],
        ..Default::default()
    };
    batch.validate_contract(2, [3; 32]).unwrap();
    let plan = h.model.plan_batch(&batch);
    assert_eq!(plan.installed, vec![0]);
    assert_eq!(refused(plan), vec![1], "the band pair finds the pool full");
    h.execute();
}
