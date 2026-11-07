// GPU proofs of repack through the spare, growth and retirement, and the device limit's deferral.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use super::*;

// AC 10, GPU half: a repack moves blocks through the spare layer of the one
// pool texture; every resident block then samples its own texels, and no
// second pool texture is ever created.
#[test]
fn repack_through_the_spare_layer_keeps_every_block_on_its_own_texels_without_a_second_pool() {
    let Some(mut stream) = Stream::new(
        "repack_through_the_spare_layer_keeps_every_block_on_its_own_texels_without_a_second_pool",
        &FRAGMENTED,
        1,
    ) else {
        return;
    };
    fragmented_single_layer(&mut stream);
    let placed_before = stream.state.model().placement(1);

    let outcome = stream.drain(stream.delta(vec![mandatory(2)], vec![0], vec![stream.prepared(2)]));
    assert!(outcome.pool.lightmap.repacked && !outcome.pool.lightmap.grew);
    assert_eq!(outcome.installed, vec![2]);
    assert_eq!(outcome.evicted, vec![0]);
    let plan = stream.state.model().plan();
    let spare = stream.state.model().pool(PoolKind::Lightmap).spare_layer();
    assert!(
        !plan.pools.lightmap.copies.is_empty(),
        "the repack moved resident blocks"
    );
    assert!(
        plan.pools
            .lightmap
            .copies
            .iter()
            .all(|copy| copy.src.layer == spare || copy.dst.layer == spare),
        "one layer: every move stages through the spare"
    );
    assert_ne!(
        stream.state.model().placement(1),
        placed_before,
        "block 1 moved"
    );

    let counters = stream.state.counters();
    assert_eq!(counters.repacks.lightmap, 1);
    assert_eq!(
        counters.repack_copy_commands.lightmap,
        plan.pools.lightmap.copies.len() as u64
    );
    assert_eq!(
        counters.pool_texture_sets.lightmap, 1,
        "a repack creates no pool texture"
    );
    assert_eq!(
        stream
            .state
            .model()
            .pool(PoolKind::Lightmap)
            .texture_allocations(),
        1
    );
    assert_eq!(counters.growths.lightmap, 0);
    assert_eq!(counters.submissions, 2, "one submission per drain");
    stream.assert_samples_own_texels();
}

// P7, repack half: an install that fails after a repack leaves the repack's
// moves standing and every entry addressing its own block.
#[test]
fn a_failed_install_after_a_repack_leaves_every_entry_on_its_own_block() {
    let Some(mut stream) = Stream::new(
        "a_failed_install_after_a_repack_leaves_every_entry_on_its_own_block",
        &FRAGMENTED,
        1,
    ) else {
        return;
    };
    fragmented_single_layer(&mut stream);
    let mut broken = stream.prepared(2);
    broken.payload.irradiance.truncate(64);

    let outcome = stream.drain(stream.delta(vec![mandatory(2)], vec![0], vec![broken]));
    assert!(outcome.pool.lightmap.repacked);
    assert!(outcome.installed.is_empty());
    assert_eq!(outcome.failed, vec![2], "the failed pair fails whole");
    assert!(!stream.state.model().is_resident(2));
    assert!(stream.state.model().is_resident(1) && stream.state.model().is_resident(3));
    let counters = stream.state.counters();
    assert_eq!(counters.failed_installs, 1);
    assert_eq!(counters.last_drain_uploads, 0);
    assert_eq!(
        counters.last_drain_install_bytes, 0,
        "no plane of block 2 uploaded"
    );
    stream.assert_samples_own_texels();
}

// Growth, plus P7's growth half: a mandatory block that fits no repack grows
// a new generation; the old one retires and is released only once its
// submitted work is done, and a pair failing in the growth drain leaves every
// other entry on its own block in the new generation.
#[test]
fn growth_keeps_every_block_on_its_own_texels_and_retires_the_old_set_until_work_is_done() {
    let Some(mut stream) = Stream::new(
        "growth_keeps_every_block_on_its_own_texels_and_retires_the_old_set_until_work_is_done",
        &[(WIDE, 1100), (WIDE, 1200), (8, 8)],
        1,
    ) else {
        return;
    };
    stream.drain(stream.reset(vec![mandatory(0)], vec![stream.prepared(0)]));
    let first_generation_bytes = stream.state.counters().active_pool_bytes.lightmap;
    let mut broken = stream.prepared(2);
    broken.payload.shadowmask = None;

    let outcome = stream.drain(stream.delta(
        vec![mandatory(1), mandatory(2)],
        vec![],
        vec![stream.prepared(1), broken],
    ));
    assert!(outcome.pool.lightmap.grew && outcome.pool.lightmap.retiring);
    assert_eq!(outcome.installed, vec![1]);
    assert_eq!(outcome.failed, vec![2]);
    assert!(outcome.refused.is_empty());
    assert_eq!(outcome.pool.lightmap.layers, 2);
    let counters = stream.state.counters();
    assert_eq!(counters.growths.lightmap, 1);
    assert_eq!(counters.pool_texture_sets.lightmap, 2);
    assert_eq!(counters.pool_layers.lightmap, 2);
    assert_eq!(
        counters.retiring_pool_bytes.lightmap,
        first_generation_bytes
    );
    assert_eq!(
        counters.growth_transient_peak_bytes,
        counters.active_pool_bytes.total() + first_generation_bytes
    );
    assert_eq!(stream.state.irradiance().size().depth_or_array_layers, 3);

    // Nothing has polled the device, so the submitted-work callback has not
    // run: a no-op drain keeps the old set retiring.
    stream.drain(stream.delta(vec![], vec![], vec![]));
    let retiring = stream
        .state
        .retiring_pool(PoolKind::Lightmap)
        .expect("still retiring");
    assert!(!retiring.is_complete());
    assert!(stream.state.model().pool(PoolKind::Lightmap).retiring());

    // Sampling polls the device to completion; the next drain releases it.
    stream.assert_samples_own_texels();
    assert!(
        stream
            .state
            .retiring_pool(PoolKind::Lightmap)
            .is_some_and(|r| r.is_complete())
    );
    let outcome = stream.drain(stream.delta(vec![], vec![], vec![stream.prepared(2)]));
    assert!(!outcome.pool.lightmap.retiring);
    assert_eq!(
        outcome.installed,
        vec![2],
        "a retry installs in the new set"
    );
    assert!(stream.state.retiring_pool(PoolKind::Lightmap).is_none());
    assert!(!stream.state.model().pool(PoolKind::Lightmap).retiring());
    assert_eq!(stream.state.counters().retiring_pool_bytes.total(), 0);
    stream.assert_samples_own_texels();
}

// A pair that needs growth past the device's array-layer limit is deferred
// and owned back: nothing grows, nothing is submitted, and the table stays as
// it was. The miss lasts while the pair stays wanted, so it warns once per
// level however many drains defer it, naming the pool at the limit.
fn assert_block_1_defers_past_the_device_limit(mut stream: Stream, pool: &str) {
    stream.drain(stream.reset(vec![mandatory(0)], vec![stream.prepared(0)]));
    let table = stream.table();
    let before = stream.state.counters();
    let logs = LogCapture::start();
    let outcome = stream.drain(stream.delta(vec![mandatory(1)], vec![], vec![stream.prepared(1)]));
    assert!(outcome.installed.is_empty() && !outcome.pool.lightmap.grew);
    assert!(!outcome.pool.mask.grew);
    let deferred: Vec<u32> = outcome.deferred.iter().map(|p| p.block).collect();
    assert_eq!(deferred, vec![1], "the pair is owned back");
    let counters = stream.state.counters();
    assert_eq!(counters.deferred_pairs, before.deferred_pairs + 1);
    assert_eq!(
        counters.submissions, before.submissions,
        "nothing submitted"
    );
    assert_eq!(counters.pool_texture_sets, PerPool::both(1));
    assert_eq!(stream.state.model().pool(PoolKind::Lightmap).layers(), 1);
    assert!(!stream.state.model().pool(PoolKind::Lightmap).retiring());
    assert!(!stream.state.model().is_resident(1));
    assert_eq!(stream.table(), table);
    stream.assert_samples_own_texels();

    let outcome = stream.drain(stream.delta(vec![], vec![], vec![stream.prepared(1)]));
    assert_eq!(outcome.deferred.len(), 1, "deferred again");
    logs.assert_logged_once(Level::Warn, "a permanent miss");
    logs.assert_logged_once(Level::Warn, &format!("the {pool} pool holds"));
}

// The lightmap pool at the limit: block 1 stores no plane, so only its lightmap
// half wants a layer past the device's one usable layer.
#[test]
fn a_pair_needing_growth_past_the_device_layer_limit_is_deferred() {
    let Some(stream) = Stream::with_fixture(
        "a_pair_needing_growth_past_the_device_layer_limit_is_deferred",
        fixture(&[(WIDE, 1100), (WIDE, 1200)]).with_planes(&[[true, false], [false, false]]),
        PerPool::both(1),
        2,
    ) else {
        return;
    };
    assert_block_1_defers_past_the_device_limit(stream, "lightmap");
}

// The mask pool at the limit: block 0's planes each fill a mask layer, so the
// device's two usable layers hold them and no third, while the lightmap half
// alone would still fit.
#[test]
fn a_pair_needing_mask_layers_past_the_device_limit_is_deferred() {
    let Some(stream) = Stream::sized(
        "a_pair_needing_mask_layers_past_the_device_limit_is_deferred",
        &[(WIDE, 1100), (WIDE, 1200)],
        PerPool {
            lightmap: 1,
            mask: 2,
        },
        3,
    ) else {
        return;
    };
    assert_block_1_defers_past_the_device_limit(stream, "mask");
}
