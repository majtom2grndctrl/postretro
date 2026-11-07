// GPU proofs of the two pools under one drain: each repacks and grows in its own
// generation, one submission carries both, counted at the queue, and each
// retires alone with the meter following both.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use super::*;

/// A block as wide as a layer: nothing shares its shelf.
const FULL: u32 = 2048;
const PLANE_A: [bool; 2] = [true, false];
const BOTH_PLANES: [bool; 2] = [true, true];
const NO_PLANES: [bool; 2] = [false, false];

fn planned_fixture(blocks: &[((u32, u32), [bool; 2])]) -> BlockFixture {
    let extents: Vec<(u32, u32)> = blocks.iter().map(|&(extent, _)| extent).collect();
    let planes: Vec<[bool; 2]> = blocks.iter().map(|&(_, planes)| planes).collect();
    fixture(&extents).with_planes(&planes)
}

fn sized_stream(
    test: &str,
    blocks: &[((u32, u32), [bool; 2])],
    lightmap: u32,
    mask: u32,
) -> Option<Stream> {
    Stream::with_fixture(
        test,
        planned_fixture(blocks),
        PerPool { lightmap, mask },
        u32::MAX,
    )
}

/// Blocks 0 and 1 stacked above a gap block 2 fits only after a repack, with a
/// band block 3 beside them.
fn stacked(band_block: (u32, u32)) -> [((u32, u32), [bool; 2]); 4] {
    [
        ((FULL, 768), PLANE_A),
        ((FULL, 512), PLANE_A),
        ((FULL, 1280), PLANE_A),
        (band_block, PLANE_A),
    ]
}

fn stacked_and_band(stream: &mut Stream) {
    let ready = vec![stream.prepared(0), stream.prepared(1), stream.prepared(3)];
    let outcome = stream.drain(stream.reset(vec![mandatory(0), mandatory(1), band(3, 3)], ready));
    assert_eq!(outcome.installed, vec![0, 1, 3]);
}

/// Run the drain that lands block 2 in block 0's place, one queue submission.
fn replace_block_0_with_2(stream: &mut Stream) -> LightmapDrainOutcome {
    let submits = stream.queue.counts().submits;
    let outcome = stream.drain(stream.delta(vec![mandatory(2)], vec![0], vec![stream.prepared(2)]));
    assert_eq!(outcome.installed, vec![2]);
    assert_eq!(
        stream.queue.counts().submits,
        submits + 1,
        "copies, uploads and table writes reach the queue once"
    );
    outcome
}

fn written_blocks(state: &LightmapStreamState) -> Vec<u32> {
    state
        .model()
        .plan()
        .table_writes
        .iter()
        .map(|write| write.index - 1)
        .collect()
}

// One pool's repack moves nothing in the other, except that a band block it
// drops leaves both, its entry turning non-resident once. Each direction lands
// in one submission with every resident block on its own texels.
#[test]
fn one_pool_repack_leaves_the_other() {
    const TEST: &str = "one_pool_repack_leaves_the_other";
    let blocks = stacked((FULL, 512));

    let Some(mut stream) = sized_stream(TEST, &blocks, 4, 1) else {
        return;
    };
    stacked_and_band(&mut stream);
    let lightmap_at = stream.state.model().placement(1);
    assert!(stream.state.model().plane_placement(3, 0).is_some());
    let outcome = replace_block_0_with_2(&mut stream);
    let plan = stream.state.model().plan();
    assert!(outcome.pool.mask.repacked && !outcome.pool.lightmap.repacked);
    assert!(plan.pools.lightmap.copies.is_empty());
    assert!(!plan.pools.mask.copies.is_empty());
    assert_eq!(outcome.evicted, vec![0, 3], "the band block leaves both");
    assert!(!stream.state.model().is_resident(3));
    assert_eq!(stream.state.model().plane_placement(3, 0), None);
    assert_eq!(
        written_blocks(&stream.state)
            .iter()
            .filter(|&&b| b == 3)
            .count(),
        1,
        "its entry goes non-resident once"
    );
    assert_eq!(
        stream.state.model().placement(1),
        lightmap_at,
        "the lightmap pool stood"
    );
    let counters = stream.state.counters();
    assert_eq!(counters.repacks.lightmap, 0);
    assert_eq!(counters.repacks.mask, 1);
    assert_eq!(
        counters.repack_copy_commands.mask,
        plan.pools.mask.copies.len() as u64
    );
    assert_eq!(counters.pool_texture_sets, PerPool::both(1));
    stream.assert_samples_own_texels();

    let Some(mut stream) = sized_stream(TEST, &blocks, 1, 4) else {
        return;
    };
    stacked_and_band(&mut stream);
    let plane_at = stream.state.model().plane_placement(1, 0);
    let outcome = replace_block_0_with_2(&mut stream);
    assert!(outcome.pool.lightmap.repacked && !outcome.pool.mask.repacked);
    assert!(stream.state.model().plan().pools.mask.copies.is_empty());
    assert!(!stream.state.model().is_resident(3), "it leaves both");
    assert_eq!(stream.state.model().plane_placement(3, 0), None);
    assert_eq!(
        stream.state.model().plane_placement(1, 0),
        plane_at,
        "the mask pool stood"
    );
    stream.assert_samples_own_texels();
}

// One pool repacks while the other grows: the moves land in the repacking pool's
// live generation, each upload in its own pool's live or new generation, one
// submission carries it all, and only the grown pool retires.
#[test]
fn repack_beside_growth_lands_in_one_submission() {
    const TEST: &str = "repack_beside_growth_lands_in_one_submission";

    // The mask pool repacks for block 2; the lightmap pool, full of mandatory
    // blocks, must grow for it.
    let blocks = [
        ((FULL, 768), PLANE_A),
        ((FULL, 512), PLANE_A),
        ((FULL, 1280), PLANE_A),
        ((FULL, 768), NO_PLANES),
    ];
    let Some(mut stream) = sized_stream(TEST, &blocks, 1, 1) else {
        return;
    };
    let ready = vec![stream.prepared(0), stream.prepared(1), stream.prepared(3)];
    stream.drain(stream.reset(vec![mandatory(0), mandatory(1), mandatory(3)], ready));
    let mask_texture = stream.state.shadowmask().expect("planes").clone();
    let outcome = replace_block_0_with_2(&mut stream);
    assert!(outcome.pool.mask.repacked && !outcome.pool.mask.grew);
    assert!(outcome.pool.lightmap.grew);
    assert!(stream.state.retiring_pool(PoolKind::Lightmap).is_some());
    assert!(stream.state.retiring_pool(PoolKind::Mask).is_none());
    assert!(stream.state.shadowmask() == Some(&mask_texture));
    let counters = stream.state.counters();
    assert_eq!(counters.growths.lightmap, 1);
    assert_eq!(counters.growths.mask, 0);
    assert_eq!(counters.retiring_pool_bytes.mask, 0);
    assert!(counters.retiring_pool_bytes.lightmap > 0);
    stream.assert_samples_own_texels();

    // The lightmap pool repacks for block 2; its two planes outgrow the mask pool.
    let blocks = [
        ((FULL, 768), PLANE_A),
        ((FULL, 512), PLANE_A),
        ((FULL, 1280), BOTH_PLANES),
    ];
    let Some(mut stream) = sized_stream(TEST, &blocks, 1, 1) else {
        return;
    };
    let ready = vec![stream.prepared(0), stream.prepared(1)];
    stream.drain(stream.reset(vec![mandatory(0), mandatory(1)], ready));
    let irradiance = stream.state.irradiance().clone();
    let outcome = replace_block_0_with_2(&mut stream);
    assert!(outcome.pool.lightmap.repacked && !outcome.pool.lightmap.grew);
    assert!(outcome.pool.mask.grew);
    assert!(stream.state.retiring_pool(PoolKind::Lightmap).is_none());
    assert!(stream.state.retiring_pool(PoolKind::Mask).is_some());
    assert!(*stream.state.irradiance() == irradiance);
    stream.assert_samples_own_texels();
}

// Both pools repack on one drain: each pool's copies land in its own live
// generation, one submission carries them, and a block moved in both has one
// table write with both new addresses.
#[test]
fn both_pools_repack_with_one_table_write() {
    let Some(mut stream) = sized_stream(
        "both_pools_repack_with_one_table_write",
        &stacked((128, 128)),
        1,
        1,
    ) else {
        return;
    };
    stacked_and_band(&mut stream);
    let was = (
        stream.state.model().placement(1),
        stream.state.model().plane_placement(1, 0),
    );
    let outcome = replace_block_0_with_2(&mut stream);
    assert!(outcome.pool.lightmap.repacked && outcome.pool.mask.repacked);
    let model = stream.state.model();
    let plan = model.plan();
    assert!(!plan.pools.lightmap.copies.is_empty() && !plan.pools.mask.copies.is_empty());
    assert!(
        model.placement(1) != was.0 && model.plane_placement(1, 0) != was.1,
        "block 1 moved in both"
    );
    let mut written = written_blocks(&stream.state);
    written.sort_unstable();
    let count = written.len();
    written.dedup();
    assert_eq!(written.len(), count, "one write per block");
    let moved = plan
        .table_writes
        .iter()
        .find(|write| write.index == 2)
        .expect("block 1's entry is written");
    assert_eq!(moved.entry, model.table_entry(1), "both new addresses");
    assert_eq!(stream.table(), model.table_bytes());
    stream.assert_samples_own_texels();
}

/// Every texel check the draw through the bound resources reads for `blocks`.
fn assert_draw_reads_own_texels(
    ctx: &GpuCtx,
    resources: &LightmapResources,
    blocks: &[(usize, (u32, u32))],
) {
    let mut probes = Vec::new();
    let mut expected = Vec::new();
    for &(block, (width, height)) in blocks {
        for (x, y) in [(0, 0), (width / 2, height / 2), (width - 1, height - 1)] {
            let uv = [
                (x as f32 + 0.5) / width as f32,
                (y as f32 + 0.5) / height as f32,
            ];
            probes.push(resident_probe(uv, block as u32));
            expected.push((block, x, y));
        }
    }
    let results = draw_probes(ctx, resources, &probes);
    assert_eq!(results.len(), expected.len());
    for (result, &(block, x, y)) in results.iter().zip(&expected) {
        let what = format!("block {block} local ({x}, {y})");
        let (irradiance, direction, mask) = expected_texel(block, x, y);
        assert_near(&result.irradiance[..3], &irradiance, 1.0e-3, &what);
        assert_near(&result.direction[..2], &direction, 1.0e-5, &what);
        assert_near(&result.mask, &mask, 1.0e-5, &what);
    }
}

// After either pool grows alone, the next draw through the level's bound
// resources samples a block in a grown layer, with no poll between: group 4
// rebinds the new generation and keeps the other pool's texture.
#[test]
fn next_draw_samples_a_grown_layer() {
    let Some(ctx) = bc_gpu_or_skip("next_draw_samples_a_grown_layer") else {
        return;
    };
    let queue = crate::render::uploads::UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let extents = [(WIDE, 1100), (WIDE, 1200)];
    let level = fixture(&extents);
    let blocks = [(0, extents[0]), (1, extents[1])];

    // The lightmap pool grows alone: block 1 needs a second layer.
    let cap = PerPool {
        lightmap: 1,
        mask: MASK_ROOM,
    };
    let mut resources = sized_resources(&ctx, &level, cap, 0);
    let ready = vec![prepared(&level, 1, 0)];
    resources
        .drain_streaming(
            &ctx.device,
            &queue,
            batch(1, Some(vec![mandatory(0)]), vec![], ready),
        )
        .expect("block 0 installs");
    let state = resources.stream_state().expect("streamed");
    let mask_texture = state.shadowmask().expect("planes").clone();
    let ready = vec![prepared(&level, 1, 1)];
    let (outcome, _) = resources
        .drain_streaming(
            &ctx.device,
            &queue,
            batch(1, None, vec![mandatory(1)], ready),
        )
        .expect("block 1 grows the lightmap pool");
    assert!(outcome.pool.lightmap.grew && !outcome.pool.mask.grew);
    let state = resources.stream_state().expect("streamed");
    assert_eq!(state.model().placement(1).expect("resident").layer, 1);
    assert!(state.shadowmask() == Some(&mask_texture));
    assert_draw_reads_own_texels(&ctx, &resources, &blocks);

    // The mask pool grows alone: block 0's plane B needs a second layer.
    let cap = PerPool {
        lightmap: 4,
        mask: 1,
    };
    let mut resources = sized_resources(&ctx, &level, cap, 1);
    let irradiance = resources
        .stream_state()
        .expect("streamed")
        .irradiance()
        .clone();
    let ready = vec![prepared(&level, 2, 0)];
    let (outcome, _) = resources
        .drain_streaming(
            &ctx.device,
            &queue,
            batch(2, Some(vec![mandatory(0)]), vec![], ready),
        )
        .expect("block 0 grows the mask pool");
    assert!(outcome.pool.mask.grew && !outcome.pool.lightmap.grew);
    let state = resources.stream_state().expect("streamed");
    assert!(state.model().plane_placement(0, 1).expect("resident").layer >= 1);
    assert!(*state.irradiance() == irradiance);
    assert_draw_reads_own_texels(&ctx, &resources, &blocks[..1]);
}

// Both pools grow on one drain and retire apart: each release frees only its
// own bytes and leaves the other's counted, a later growth waits only on its
// own pool, and the meter rows carry both pools' live layers, spares included.
#[test]
fn pools_retire_independently_and_meter_counts_both() {
    let Some(ctx) = bc_gpu_or_skip("pools_retire_independently_and_meter_counts_both") else {
        return;
    };
    const IRRADIANCE_LAYER_BYTES: u64 = 2048 * 2048 * 8;
    const MASK_LAYER_BYTES: u64 = 2048 * 2048;
    let queue = crate::render::uploads::UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let level = planned_fixture(&[
        ((WIDE, 1100), PLANE_A),
        ((WIDE, 1100), PLANE_A),
        ((WIDE, 1100), NO_PLANES),
        ((WIDE, 1100), NO_PLANES),
    ]);
    let mut resources = sized_resources(&ctx, &level, PerPool::both(1), 0);
    let first = resources.residency.clone();
    assert_eq!(
        first[0].bytes,
        2 * IRRADIANCE_LAYER_BYTES,
        "a layer and its spare"
    );
    assert_eq!(
        first[2].bytes,
        2 * MASK_LAYER_BYTES,
        "a layer and its spare"
    );
    let drain = |resources: &mut LightmapResources, batch| {
        resources
            .drain_streaming(&ctx.device, &queue, batch)
            .expect("the drain succeeds")
    };
    drain(
        &mut resources,
        batch(
            1,
            Some(vec![mandatory(0)]),
            vec![],
            vec![prepared(&level, 1, 0)],
        ),
    );

    // Both pools need a second layer for block 1, on one drain.
    let (outcome, meter_changed) = drain(
        &mut resources,
        batch(1, None, vec![mandatory(1)], vec![prepared(&level, 1, 1)]),
    );
    assert!(outcome.pool.lightmap.grew && outcome.pool.mask.grew && meter_changed);
    let state = resources.stream_state_mut().expect("streamed");
    let counters = state.counters();
    let lightmap_flag = state
        .retiring_pool_mut(PoolKind::Lightmap)
        .expect("the lightmap pool retires")
        .hold_completion();
    let mask_flag = state
        .retiring_pool_mut(PoolKind::Mask)
        .expect("the mask pool retires")
        .hold_completion();
    let lightmap_retiring = counters.retiring_pool_bytes.lightmap;
    let mask_retiring = counters.retiring_pool_bytes.mask;
    assert_eq!(mask_retiring, 2 * MASK_LAYER_BYTES);
    assert!(
        lightmap_retiring > 2 * IRRADIANCE_LAYER_BYTES,
        "direction rides along"
    );
    assert_eq!(
        resources.retiring_bytes(),
        lightmap_retiring + mask_retiring
    );
    let layers = counters.pool_layers;
    assert_eq!(
        resources.residency[0].bytes,
        u64::from(layers.lightmap + 1) * IRRADIANCE_LAYER_BYTES,
        "the irradiance row is the live generation, spare included"
    );
    assert_eq!(
        resources.residency[2].bytes,
        u64::from(layers.mask + 1) * MASK_LAYER_BYTES,
        "the shadowmask row is the live generation, spare included"
    );

    // The lightmap pool's release leaves the mask pool's bytes counted.
    lightmap_flag.store(true, Ordering::Release);
    let (outcome, meter_changed) = drain(&mut resources, batch(1, None, vec![], vec![]));
    assert!(meter_changed, "a release changes the meter");
    assert!(!outcome.pool.lightmap.retiring && outcome.pool.mask.retiring);
    assert_eq!(resources.retiring_bytes(), mask_retiring);

    // A later growth waits only on its own pool: blocks 2 and 3 hold no plane,
    // and take the lightmap pool past its reserve while the mask pool retires.
    let ready = vec![prepared(&level, 1, 2), prepared(&level, 1, 3)];
    let (outcome, _) = drain(
        &mut resources,
        batch(1, None, vec![mandatory(2), mandatory(3)], ready),
    );
    assert_eq!(outcome.installed, vec![2, 3]);
    assert!(outcome.pool.lightmap.grew, "{:?}", outcome.pool);
    assert!(!outcome.pool.mask.grew && outcome.pool.mask.retiring);
    let state = resources.stream_state_mut().expect("streamed");
    let lightmap_flag = state
        .retiring_pool_mut(PoolKind::Lightmap)
        .expect("the lightmap pool retires again")
        .hold_completion();
    let counters = state.counters();
    assert_eq!(counters.pool_texture_sets.lightmap, 3);
    assert_eq!(counters.pool_texture_sets.mask, 2);
    let lightmap_retiring = counters.retiring_pool_bytes.lightmap;
    assert_eq!(
        resources.retiring_bytes(),
        lightmap_retiring + mask_retiring
    );

    // The mask pool's release leaves the lightmap pool's bytes counted.
    mask_flag.store(true, Ordering::Release);
    let (outcome, meter_changed) = drain(&mut resources, batch(1, None, vec![], vec![]));
    assert!(meter_changed);
    assert!(outcome.pool.lightmap.retiring && !outcome.pool.mask.retiring);
    assert_eq!(resources.retiring_bytes(), lightmap_retiring);

    lightmap_flag.store(true, Ordering::Release);
    let (outcome, _) = drain(&mut resources, batch(1, None, vec![], vec![]));
    assert!(!outcome.pool.lightmap.retiring && !outcome.pool.mask.retiring);
    assert_eq!(resources.retiring_bytes(), 0);
    let state = resources.stream_state().expect("streamed");
    assert_samples_own_texels(&ctx, state, &level);
}
