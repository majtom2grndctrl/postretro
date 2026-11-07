// GPU proofs of a level reload while a generation retires, through the resource set.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use super::*;

fn streamed_resources(ctx: &GpuCtx, fixture: &BlockFixture, floor: u64) -> LightmapResources {
    let cap = PerPool {
        lightmap: 1,
        mask: MASK_ROOM,
    };
    sized_resources(ctx, fixture, cap, floor)
}

// Growth through the resource set, then P9 / AC 13: group 4 rebinds the new
// generation and the meter follows it with the retiring set counted apart;
// a reload while that set is alive releases both pools (the queue callback
// keeps only its flag), and the new level's first drain sees none of the
// old level's state and refuses its generation.
#[test]
fn reload_while_a_generation_retires_releases_both_pools_and_refuses_the_old_generation() {
    let Some(ctx) = bc_gpu_or_skip(
        "reload_while_a_generation_retires_releases_both_pools_and_refuses_the_old_generation",
    ) else {
        return;
    };
    let queue = crate::render::uploads::UploadQueue::new(&ctx.device, ctx.queue.clone(), true);
    let level_a = fixture(&[(WIDE, 1100), (WIDE, 1200)]);
    let mut resources = streamed_resources(&ctx, &level_a, 0);
    let installed_rows = resources.residency.clone();
    let first = resources
        .drain_streaming(
            &ctx.device,
            &queue,
            batch(
                5,
                Some(vec![mandatory(0)]),
                vec![],
                vec![prepared(&level_a, 5, 0)],
            ),
        )
        .expect("install A")
        .0;
    assert_eq!(first.installed, vec![0]);
    let (grown, meter_changed) = resources
        .drain_streaming(
            &ctx.device,
            &queue,
            batch(5, None, vec![mandatory(1)], vec![prepared(&level_a, 5, 1)]),
        )
        .expect("grow for C");
    assert!(grown.pool.lightmap.grew && meter_changed);
    let state = resources.stream_state().expect("streamed");
    assert_eq!(state.counters().pool_layers.lightmap, 2);
    assert_eq!(
        resources.residency[0].bytes,
        installed_rows[0].bytes * 3 / 2,
        "the irradiance row follows the 3-array-layer generation"
    );
    assert_eq!(
        resources.retiring_bytes(),
        state.counters().retiring_pool_bytes.total()
    );
    assert!(resources.retiring_bytes() > 0);
    let flag = Arc::clone(
        state
            .retiring_pool(PoolKind::Lightmap)
            .expect("retiring before any poll")
            .completion_flag(),
    );
    assert!(!flag.load(Ordering::Acquire));
    let floor = resources.generation_high_water();
    assert_eq!(floor, 5);

    // Reload: the new level's resources replace the old ones, as install does.
    drop(resources);
    assert_eq!(
        Arc::strong_count(&flag),
        2,
        "only this test and the pending queue callback hold the flag: the retiring set dropped"
    );
    let level_b = fixture(&[(8, 8), (8, 8)]);
    let mut resources = streamed_resources(&ctx, &level_b, floor);
    let old = resources.drain_streaming(
        &ctx.device,
        &queue,
        batch(
            5,
            Some(vec![mandatory(0)]),
            vec![],
            vec![prepared(&level_b, 5, 0)],
        ),
    );
    assert_eq!(
        old.unwrap_err(),
        LightmapResidencyDrainError::StaleGeneration {
            current: 5,
            received: 5
        }
    );
    let state = resources.stream_state().expect("streamed");
    let counters = state.counters();
    assert_eq!(counters.drains, 0);
    assert_eq!(counters.pool_texture_sets, PerPool::both(1));
    assert_eq!(counters.retiring_pool_bytes.total(), 0);
    assert!(state.retiring_pool(PoolKind::Lightmap).is_none());
    assert_eq!(resources.retiring_bytes(), 0);

    let outcome = resources
        .drain_streaming(
            &ctx.device,
            &queue,
            batch(
                6,
                Some(vec![mandatory(1)]),
                vec![],
                vec![prepared(&level_b, 6, 1)],
            ),
        )
        .expect("the new level's first drain")
        .0;
    assert_eq!(outcome.installed, vec![1]);
    assert!(!outcome.pool.lightmap.retiring);
    let state = resources.stream_state().expect("streamed");
    assert_samples_own_texels(&ctx, state, &level_b);

    // The poll above ran the old callback: it set the flag and dropped its
    // clone, touching nothing of level B.
    assert!(flag.load(Ordering::Acquire));
    assert_eq!(Arc::strong_count(&flag), 1);
}
