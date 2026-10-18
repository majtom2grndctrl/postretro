//! GPU-backed SH growth limit and drain transaction proofs.
use super::*;

use crate::render::sh_streaming::gpu::load_fixture;
use crate::render::gpu_test_harness::{GpuCtx, bc_gpu_or_skip};
use crate::render::sh_streaming::floor::{
    DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES, plan_initial_pool_floor,
};

fn replace_shape(
    fixture: &mut UploadOrderSh,
    device: &wgpu::Device,
    raw: &wgpu::Queue,
    limits: &wgpu::Limits,
    shape: AtlasShape,
) {
    // Every override is at or below the grant; alignments remain the grant's.
    let grant = device.limits();
    assert!(limits.max_texture_array_layers <= grant.max_texture_array_layers);
    assert!(limits.max_texture_dimension_2d <= grant.max_texture_dimension_2d);
    assert!(limits.max_buffer_size <= grant.max_buffer_size);
    assert!(limits.max_storage_buffer_binding_size <= grant.max_storage_buffer_binding_size);
    fixture.state.gpu = Some(
        StreamingGpuPools::new_with_shape(
            device,
            limits,
            raw,
            &fixture.map.base(),
            &fixture.map.sources(),
            shape,
            &BTreeMap::new(),
            false,
            &mut fixture.sh,
            &fixture.uniforms,
            &fixture.weights,
            0,
            0,
            BTreeMap::new(),
        )
        .unwrap(),
    );
}

fn grow_dense(
    f: &mut UploadOrderSh,
    device: &wgpu::Device,
    queue: &UploadQueue,
    slots: u32,
) -> Result<(), ShResidencyDrainError> {
    f.state.gpu.as_mut().unwrap().grow_dense(
        slots,
        DenseGrowthInputs {
            device,
            queue,
            base: &f.map.base(),
            sources: &f.map.sources(),
            probe_occlusion_enabled: false,
            sh: &mut f.sh,
            selection_weights: &f.weights,
        },
    )
}

fn grow_sparse(
    f: &mut UploadOrderSh,
    device: &wgpu::Device,
    queue: &UploadQueue,
    required: &SparseCapacityFloors,
) -> Result<(), ShResidencyDrainError> {
    f.state.gpu.as_mut().unwrap().grow_sparse(
        device,
        queue,
        &f.map.base(),
        &f.map.sources(),
        required,
        &mut f.sh,
        &f.uniforms,
        &f.weights,
    )
}

fn finish_retirement(f: &mut UploadOrderSh, device: &wgpu::Device) {
    // Waiting is test-only: production growth returns retirement pressure.
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    f.state.release_retired_generations();
}

#[test]
fn sh_growth_production_zero_share_load_holds_every_cluster() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH production zero-share growth")
    else {
        return;
    };
    let (_temp, path) = load_fixture::write_nine_cluster_prl();
    let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
    let manifest = world
        .sh_stream_manifest()
        .expect("the valid id-49/id-50 fixture streams by default");
    assert_eq!(manifest.cluster_count(), 9);
    let map = SyntheticMap::new([36, 4, 4], [1, 1, 1]).direct_base_only();
    let mut f = UploadOrderSh::with_map(&device, &raw, 128, map);
    f.state = ShResidencyState::from_manifest(manifest, &world.cells).unwrap();
    let mut limits = device.limits();
    limits.max_texture_array_layers = 4;
    assert!(limits.max_texture_array_layers <= device.limits().max_texture_array_layers);
    let largest = f
        .state
        .nodes_by_owner
        .values()
        .map(|nodes| {
            nodes
                .iter()
                .map(|node| f.state.node_layouts[node].tile_count)
                .sum::<u32>()
        })
        .max()
        .unwrap();
    let whole = f
        .state
        .node_layouts
        .values()
        .map(|layout| layout.global_base_slot + layout.tile_count)
        .max()
        .unwrap();
    // This precondition fails if the fixture ever ceases to distinguish the
    // old floor-share geometry from the whole-level load rule.
    assert!(whole > limits.max_texture_array_layers * largest);
    f.state
        .initialize_gpu_with_limits(
            &device,
            &raw,
            manifest.base(),
            manifest.sources(),
            false,
            &mut f.sh,
            &f.uniforms,
            &f.weights,
            &limits,
            DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES,
        )
        .unwrap();
    let initial = f.state.gpu.as_ref().unwrap().shape;
    assert!(initial.tiles_per_layer >= (whole * 9).div_ceil(4 * limits.max_texture_array_layers));
    assert_eq!(initial.layers, 1);
    let fixed =
        initial_fixed_metadata_bytes(manifest.base(), manifest.sources(), &limits, &f.sh).unwrap();
    let floor = plan_initial_pool_floor(
        manifest.base(),
        manifest.sources(),
        largest,
        whole,
        &f.state.sparse_capacity_floors,
        fixed + DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES,
        f.sh.billboard_direct_scatter.capacity_bytes(),
        &limits,
    )
    .unwrap();
    assert_eq!(floor.dense_slots, largest);
    assert_eq!(
        floor.dense_group_minimum_bytes,
        u64::from(initial.slots) * 1024
    );
    assert_eq!(
        floor.effective_floor_bytes,
        f.state.gpu.as_ref().unwrap().effective_floor_bytes
    );
    assert_eq!(
        floor.effective_floor_bytes,
        fixed
            + DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES
            + f.sh.billboard_direct_scatter.capacity_bytes()
            + f.state.gpu.as_ref().unwrap().active_capacity_bytes
    );
    let queue = UploadQueue::new(&device, raw, true);
    for cluster in 0..manifest.cluster_count() {
        let ready = postretro_level_loader::PreparedShCluster {
            generation: 1,
            content_tag: manifest.content_tag(),
            chunk: manifest.read_and_decode_cluster(cluster).unwrap(),
        };
        let outcome = f
            .state
            .drain(
                &device,
                &queue,
                &mut f.sh,
                &f.uniforms,
                &f.weights,
                ShDrainBatch {
                    generation: 1,
                    content_tag: manifest.content_tag(),
                    target_reset: (cluster == 0).then(|| vec![(1 << 9) - 1]),
                    ready: vec![ready],
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(outcome.accepted, [cluster]);
        assert!(outcome.refused.is_empty());
        let shape = f.state.gpu.as_ref().unwrap().shape;
        assert_eq!(
            (shape.tiles_per_row, shape.tiles_per_layer),
            (initial.tiles_per_row, initial.tiles_per_layer)
        );
        // Canonical stencil halos make a later cluster depend on previous
        // owners. Match the production issuer: compose and promote an owner
        // before requesting its dependent, retaining every earlier resident.
        f.compose(&device, &queue);
        f.state
            .drain(
                &device,
                &queue,
                &mut f.sh,
                &f.uniforms,
                &f.weights,
                ShDrainBatch {
                    generation: 1,
                    content_tag: manifest.content_tag(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(f.state.sampleable.contains(&cluster));
        finish_retirement(&mut f, &device);
    }
    f.compose(&device, &queue);
    f.state
        .drain(
            &device,
            &queue,
            &mut f.sh,
            &f.uniforms,
            &f.weights,
            ShDrainBatch {
                generation: 1,
                content_tag: manifest.content_tag(),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(f.state.installed.len(), 9);
    assert_eq!(f.state.sampleable.len(), 9);
    assert_eq!(f.state.dense_slots.capacity, whole);
    eprintln!(
        "[ShGrowthProof] actual PRL→loader→manifest→zero-share GPU load: all 9 clusters resident together, ran"
    );
}

#[test]
fn sh_growth_dense_clamps_and_limit_precedes_retirement() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH dense clamp") else {
        return;
    };
    let mut limits = device.limits();
    limits.max_texture_array_layers = 256;
    let queue = UploadQueue::new(&device, raw.clone(), true);
    for (layers, expected) in [(200, 256), (100, 200)] {
        let map = SyntheticMap::new([4, 4, 4], [1, 1, 1])
            .direct_base_only()
            .with_empty_rows(27, &[0])
            .with_density_levels(&[2]);
        let mut f = UploadOrderSh::with_map(&device, &raw, 128, map);
        replace_shape(
            &mut f,
            &device,
            &raw,
            &limits,
            AtlasShape {
                tiles_per_row: 1,
                tiles_per_layer: 1,
                layers,
                slots: layers,
            },
        );
        // A one-tile writer arrives at the retained allocation high-water.
        f.state.dense_slots.capacity = layers;
        let outcome = f.drain_many(&device, &queue, &[0]).unwrap();
        assert_eq!(outcome.accepted, [0]);
        assert!(outcome.refused.is_empty());
        let pools = f.state.gpu.as_ref().unwrap();
        assert_eq!(pools.shape.layers, expected);
        assert_eq!(
            (pools.shape.extent().width, pools.shape.extent().height),
            (8, 8)
        );
        assert!(pools.retiring_dense.is_some());
        assert!(matches!(
            grow_dense(&mut f, &device, &queue, 257),
            Err(ShResidencyDrainError::GpuDeviceLimit { .. })
        ));
        assert_eq!(f.state.gpu.as_ref().unwrap().shape.layers, expected);
        if expected == 200 {
            assert!(matches!(
                grow_dense(&mut f, &device, &queue, 201),
                Err(ShResidencyDrainError::GpuRetirementPressure { .. })
            ));
        }
        f.compose(&device, &queue);
        f.drain_many(&device, &queue, &[]).unwrap();
        assert!(f.state.sampleable.contains(&0));
        assert_eq!(
            first_buffer_word(
                &device,
                &queue,
                &f.state.gpu.as_ref().unwrap().sampled_indirection
            ),
            2 | 4 | (layers << 5)
        );
        finish_retirement(&mut f, &device);
    }
    let map = SyntheticMap::new([4, 4, 4], [1, 1, 1])
        .direct_base_only()
        .with_empty_rows(27, &[0])
        .with_density_levels(&[2]);
    let mut f = UploadOrderSh::with_map(&device, &raw, 128, map);
    replace_shape(
        &mut f,
        &device,
        &raw,
        &limits,
        AtlasShape {
            tiles_per_row: 1,
            tiles_per_layer: 1,
            layers: 200,
            slots: 200,
        },
    );
    f.state.dense_slots.capacity = 256;
    let outcome = f.drain_many(&device, &queue, &[0]).unwrap();
    assert_eq!(outcome.refused, [0]);
    assert_eq!(f.state.gpu.as_ref().unwrap().shape.layers, 200);
    assert_eq!(f.state.dense_slots.capacity, 256);
    assert!(f.state.installed.is_empty());
    eprintln!(
        "[ShGrowthProof] dense drains installed 200→256 and 100→200; 257 refused; GPU sampled-indirection readback ran"
    );
}

#[test]
fn sh_growth_sparse_clamps_every_family_and_preflights_the_whole_transaction() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH sparse clamp") else {
        return;
    };
    let queue = UploadQueue::new(&device, raw.clone(), true);
    // Each device bound must be decisive on its own. The other keeps the
    // actual grant throughout both the family matrix and sibling transaction.
    for buffer_limit_is_tighter in [false, true] {
        // GPU initialization always creates this fixed compose record, even
        // when the sparse storage-binding limit is the tighter bound.
        let tightest_buffer_limit: u64 = if buffer_limit_is_tighter { 65_536 } else { 512 };
        let entry_limit = u32::try_from(tightest_buffer_limit / 4 - 3 * 2).unwrap();
        let tile_limit = u32::try_from(tightest_buffer_limit / 2).unwrap();
        let entry_seed = entry_limit * 3 / 4;
        let tile_seed = tile_limit * 3 / 4;
        assert!(entry_seed < entry_limit);
        assert!(tile_seed + 48 <= tile_limit);
        assert!(entry_seed * 2 > entry_limit);
        assert!(tile_seed * 2 > tile_limit);
        for section in [27, 41, 45] {
            let mut f = UploadOrderSh::with_direct_compose(&device, &raw);
            let mut limits = device.limits();
            if buffer_limit_is_tighter {
                limits.max_buffer_size = tightest_buffer_limit;
            } else {
                limits.max_storage_buffer_binding_size = tightest_buffer_limit;
            }
            replace_shape(
                &mut f,
                &device,
                &raw,
                &limits,
                AtlasShape::for_slots(128, 128, &limits).unwrap(),
            );
            grow_sparse(
                &mut f,
                &device,
                &queue,
                &BTreeMap::from([(section, (entry_seed, tile_seed))]),
            )
            .unwrap();
            // The entry limit subtracts (3 × 2 affinity rows) words from its
            // buffer bound; packed tiles use two bytes per f16 element.
            let pools = f.state.gpu.as_ref().unwrap();
            assert_eq!(pools.sparse_entry_capacity(section), entry_seed);
            assert_eq!(pools.sparse_tile_f16_capacity(section), tile_seed);
            assert!(matches!(
                grow_sparse(
                    &mut f,
                    &device,
                    &queue,
                    &BTreeMap::from([(section, (entry_seed + 1, tile_seed + 48))])
                ),
                Err(ShResidencyDrainError::GpuRetirementPressure { .. })
            ));
            for required in [(entry_limit + 1, tile_seed), (entry_seed, tile_limit + 1)] {
                assert!(matches!(
                    grow_sparse(
                        &mut f,
                        &device,
                        &queue,
                        &BTreeMap::from([(section, required)])
                    ),
                    Err(ShResidencyDrainError::GpuDeviceLimit { .. })
                ));
            }
            finish_retirement(&mut f, &device);
            // One real row fits each bound, while doubling either seed passes
            // its bound; both families must install after clamping.
            let pool = f.state.sparse_pools.get_mut(&section).unwrap();
            pool.entries.capacity = entry_seed;
            pool.tiles.capacity = tile_seed;
            let outcome = f.drain_many(&device, &queue, &[0]).unwrap();
            assert_eq!(outcome.accepted, [0]);
            assert!(outcome.refused.is_empty());
            assert_eq!(
                f.state.gpu.as_ref().unwrap().sparse_entry_capacity(section),
                entry_limit
            );
            assert_eq!(
                f.state
                    .gpu
                    .as_ref()
                    .unwrap()
                    .sparse_tile_f16_capacity(section),
                tile_limit
            );
            // A later row exceeds each clamped high-water by one entry or 48
            // f16 values while the clamped generation still retires.
            let before = f.state.sparse_pools[&section].clone();
            f.state
                .sparse_pools
                .get_mut(&section)
                .unwrap()
                .entries
                .capacity = entry_limit;
            let outcome = f.drain_many(&device, &queue, &[1]).unwrap();
            assert_eq!(outcome.refused, [1]);
            assert!(outcome.deferred.is_empty() && outcome.evicted.is_empty());
            *f.state.sparse_pools.get_mut(&section).unwrap() = before;
            f.state
                .sparse_pools
                .get_mut(&section)
                .unwrap()
                .tiles
                .capacity = tile_limit;
            let outcome = f.drain_many(&device, &queue, &[1]).unwrap();
            assert_eq!(outcome.refused, [1]);
            assert!(outcome.deferred.is_empty() && outcome.evicted.is_empty());
            f.compose(&device, &queue);
            f.drain_many(&device, &queue, &[]).unwrap();
            assert!(f.state.sampleable.contains(&0));
            assert_ne!(
                first_buffer_word(
                    &device,
                    &queue,
                    &f.state.gpu.as_ref().unwrap().sampled_indirection
                ),
                0
            );
            finish_retirement(&mut f, &device);
        }
        // id-27 waits on retirement; id-45 exceeds its compaction limit. The
        // latter wins before any wait or replacement in any sibling family.
        let mut f = UploadOrderSh::with_direct_compose(&device, &raw);
        let mut limits = device.limits();
        if buffer_limit_is_tighter {
            limits.max_buffer_size = tightest_buffer_limit;
        } else {
            limits.max_storage_buffer_binding_size = tightest_buffer_limit;
        }
        replace_shape(
            &mut f,
            &device,
            &raw,
            &limits,
            AtlasShape::for_slots(128, 128, &limits).unwrap(),
        );
        grow_sparse(
            &mut f,
            &device,
            &queue,
            &BTreeMap::from([(27, (entry_seed, tile_seed))]),
        )
        .unwrap();
        let before = [27, 41, 45].map(|id| {
            (
                f.state.gpu.as_ref().unwrap().sparse_entry_capacity(id),
                f.state.gpu.as_ref().unwrap().sparse_tile_f16_capacity(id),
            )
        });
        let growth = f.growth_events();
        assert!(matches!(
            grow_sparse(
                &mut f,
                &device,
                &queue,
                &BTreeMap::from([
                    (27, (entry_seed + 1, tile_seed + 48)),
                    (41, (entry_seed, tile_seed)),
                    (45, (entry_limit + 1, tile_seed)),
                ])
            ),
            Err(ShResidencyDrainError::GpuDeviceLimit { .. })
        ));
        assert_eq!(
            before,
            [27, 41, 45].map(|id| (
                f.state.gpu.as_ref().unwrap().sparse_entry_capacity(id),
                f.state.gpu.as_ref().unwrap().sparse_tile_f16_capacity(id)
            ))
        );
        assert_eq!(growth, f.growth_events());
        finish_retirement(&mut f, &device);
        // No retirement this time: the same refusal still commits no siblings.
        assert!(matches!(
            grow_sparse(
                &mut f,
                &device,
                &queue,
                &BTreeMap::from([
                    (27, (entry_seed + 1, tile_seed + 48)),
                    (41, (entry_seed, tile_seed)),
                    (45, (entry_limit + 1, tile_seed)),
                ])
            ),
            Err(ShResidencyDrainError::GpuDeviceLimit { .. })
        ));
        assert_eq!(growth, f.growth_events());
    }
    eprintln!(
        "[ShGrowthProof] all 3 sparse families: tightest buffer clamp, refusal priority and atomic siblings ran on GPU"
    );
}

#[test]
fn sh_growth_refusal_rolls_back_addresses_in_both_drain_orders() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH refusal rollback") else {
        return;
    };
    let queue = UploadQueue::new(&device, raw.clone(), true);
    for order in [[0, 1], [1, 0]] {
        let map = SyntheticMap::new([8, 4, 4], [1, 1, 1])
            .direct_base_only()
            .with_density_levels(&[1, 0]);
        let mut f = UploadOrderSh::with_map(&device, &raw, 128, map);
        let mut limits = device.limits();
        limits.max_texture_array_layers = 2;
        replace_shape(
            &mut f,
            &device,
            &raw,
            &limits,
            AtlasShape {
                tiles_per_row: 4,
                tiles_per_layer: 8,
                layers: 1,
                slots: 8,
            },
        );
        let outcome = f.drain_many(&device, &queue, &order).unwrap();
        assert_eq!(outcome.accepted, [0]);
        assert_eq!(outcome.refused, [1]);
        assert!(
            outcome.deferred.is_empty() && outcome.evicted.is_empty() && outcome.dropped.is_empty()
        );
        assert_eq!(f.state.dense_slots.capacity, 8);
        assert_eq!(f.state.node_slots.len(), 1);
        assert_eq!(f.state.installed.keys().copied().collect::<Vec<_>>(), [0]);
        assert_eq!(f.state.sparse_pools[&27].entries.capacity, 2);
        assert_eq!(f.state.sparse_pools[&27].tiles.capacity, 50);
        assert!(f.state.compose_words[..4].iter().all(|word| *word != 0));
        assert!(f.state.compose_words[4..8].iter().all(|word| *word == 0));
    }
    eprintln!(
        "[ShGrowthProof] fitting/refused clusters in both drain orders; exact allocator rollback ran on GPU"
    );
}

#[test]
fn sh_growth_keeps_dense_commit_when_sparse_refuses_and_classifies_later_clusters() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH partial growth refusal") else {
        return;
    };
    let map = SyntheticMap::new([16, 4, 4], [1, 1, 1])
        .direct_base_only()
        .with_empty_rows(27, &[0])
        .with_density_levels(&[2, 2, 1, 0]);
    let mut f = UploadOrderSh::with_map(&device, &raw, 128, map);
    let mut limits = device.limits();
    limits.max_texture_array_layers = 16;
    limits.max_storage_buffer_binding_size = 1024;
    replace_shape(
        &mut f,
        &device,
        &raw,
        &limits,
        AtlasShape {
            tiles_per_row: 1,
            tiles_per_layer: 1,
            layers: 1,
            slots: 1,
        },
    );
    let queue = UploadQueue::new(&device, raw, true);
    f.drain(&device, &queue, 0);
    f.compose(&device, &queue);
    f.drain_many(&device, &queue, &[]).unwrap();
    assert!(f.state.sampleable.contains(&0));
    let sample_words = f.state.sampled_words.clone();
    let marker = [0, 60, 0, 56, 0, 52, 0, 60];
    let mut uploads = f.state.begin_uploads(marker.len());
    uploads
        .write_texture(
            &f.state.gpu.as_ref().unwrap().total,
            wgpu::Origin3d::ZERO,
            &marker,
            8,
            1,
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        )
        .unwrap();
    f.state.submit_uploads(uploads, &device, &queue);
    let before_dense = f.state.dense_slots.clone();
    // Model fragmentation's retained address high-water, without allocating
    // a large payload. Required need, not live payload bytes, drives growth.
    f.state.sparse_pools.get_mut(&27).unwrap().tiles.capacity = 512;
    let before_sparse = f.state.sparse_pools[&27].clone();
    let outcome = f.drain_many(&device, &queue, &[1, 2, 3]).unwrap();
    assert_eq!(outcome.refused, [1, 3]);
    assert_eq!(
        outcome
            .deferred
            .iter()
            .map(|p| p.chunk.cluster_id)
            .collect::<Vec<_>>(),
        [2]
    );
    assert!(outcome.accepted.is_empty() && outcome.evicted.is_empty());
    assert_eq!(f.state.gpu.as_ref().unwrap().shape.layers, 2);
    assert!(f.state.gpu.as_ref().unwrap().retiring_dense.is_some());
    assert_eq!(f.state.dense_slots, before_dense);
    assert_eq!(f.state.sparse_pools[&27], before_sparse);
    assert_eq!(f.state.sampled_words, sample_words);
    assert_eq!(
        first_texture_pixel(&device, &queue, &f.state.gpu.as_ref().unwrap().total),
        marker
    );
    assert_ne!(
        first_buffer_word(
            &device,
            &queue,
            &f.state.gpu.as_ref().unwrap().sampled_indirection
        ),
        0
    );
    assert!(f.state.sampleable.contains(&0));
    assert_eq!(f.state.installed.len(), 1);
    finish_retirement(&mut f, &device);
    eprintln!(
        "[ShGrowthProof] dense commit+sparse refusal; later retirement and limit outcomes; resident sampling preserved, ran on GPU"
    );
}

#[test]
fn sh_growth_contract_and_metadata_errors_remain_fatal() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH fatal contract guards") else {
        return;
    };
    let mut f = UploadOrderSh::new(&device, &raw, 128);
    f.state.generation = 2;
    let queue = UploadQueue::new(&device, raw.clone(), true);
    let targets = f.state.targets.clone();
    let bad_identity = f
        .state
        .drain(
            &device,
            &queue,
            &mut f.sh,
            &f.uniforms,
            &f.weights,
            ShDrainBatch {
                generation: f.state.generation,
                content_tag: [0; 32],
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(
        bad_identity,
        ShResidencyDrainError::InvalidBatch(_)
    ));
    let stale = f
        .state
        .drain(
            &device,
            &queue,
            &mut f.sh,
            &f.uniforms,
            &f.weights,
            ShDrainBatch {
                generation: 1,
                content_tag: f.state.content_tag,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(
        stale,
        ShResidencyDrainError::StaleGeneration { .. }
    ));
    let reset = f
        .state
        .drain(
            &device,
            &queue,
            &mut f.sh,
            &f.uniforms,
            &f.weights,
            ShDrainBatch {
                generation: 3,
                content_tag: f.state.content_tag,
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(
        reset,
        ShResidencyDrainError::GenerationResetRequired { .. }
    ));
    assert_eq!(f.state.targets, targets);
    assert!(f.state.installed.is_empty());
    let prepared = f.prepared(0);
    let mut outcome = postretro_level_loader::ShDrainOutcome::default();
    assert!(matches!(
        f.state.install_ready(None, vec![prepared], &mut outcome),
        Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed install requires device handles while GPU pools exist",
        })
    ));
    assert!(outcome.refused.is_empty() && outcome.accepted.is_empty());

    // Regression: a broken replacement's backing must escape the refusal
    // classifier. Grow real buffers first, then undersize the installed
    // compaction carrier without altering its declared capacity or limits.
    let mut f = UploadOrderSh::new(&device, &raw, 128);
    grow_sparse(&mut f, &device, &queue, &BTreeMap::from([(27, (100, 150))])).unwrap();
    finish_retirement(&mut f, &device);
    f.state
        .gpu
        .as_mut()
        .unwrap()
        .indirect_compose
        .undersize_compaction_for_test(&device);
    let before_dense = f.state.dense_slots.clone();
    let before_sparse = f.state.sparse_pools[&27].clone();
    assert!(matches!(
        f.drain_many(&device, &queue, &[0]),
        Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed indirect compaction metadata needs growth",
        })
    ));
    assert_eq!(f.state.dense_slots, before_dense);
    assert_eq!(f.state.sparse_pools[&27], before_sparse);
    assert!(f.state.installed.is_empty());
    eprintln!(
        "[ShGrowthProof] identity, generation, missing handles and broken post-growth backing escape drain classifier; ran on GPU"
    );
}

#[test]
fn sh_growth_retained_metadata_error_precedes_dense_device_limit() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH metadata at dense limit") else {
        return;
    };
    let map = SyntheticMap::new([4, 4, 4], [1, 1, 1])
        .direct_base_only()
        .with_empty_rows(27, &[0]);
    let mut f = UploadOrderSh::with_map(&device, &raw, 64, map);
    let ready = loader_validated_chunk(&f);
    let mut limits = device.limits();
    limits.max_texture_array_layers = 1;
    let shape = AtlasShape {
        tiles_per_row: 1,
        tiles_per_layer: 1,
        layers: 1,
        slots: 1,
    };
    replace_shape(&mut f, &device, &raw, &limits, shape);
    // The loader-valid body needs 64 slots; the same install would refuse
    // growth, but its corrupted retained metadata must remain fatal.
    assert!(matches!(
        shape.grown_for_slots(64, &limits),
        Err(ShResidencyDrainError::GpuDeviceLimit { .. })
    ));
    f.state.dense_node[0].as_mut().unwrap().level ^= 1;
    let before_dense = f.state.dense_slots.clone();
    let before_sparse = f.state.sparse_pools[&27].clone();
    let before_words = f.state.compose_words.clone();
    let queue = UploadQueue::new(&device, raw, true);
    let error = f
        .state
        .drain(
            &device,
            &queue,
            &mut f.sh,
            &f.uniforms,
            &f.weights,
            ShDrainBatch {
                generation: f.state.generation,
                content_tag: f.state.content_tag,
                ready: vec![ready],
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(matches!(
        error,
        ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "canonical node lacks id-34 prefix metadata",
        }
    ));
    assert_eq!(f.state.dense_slots, before_dense);
    assert_eq!(f.state.sparse_pools[&27], before_sparse);
    assert_eq!(f.state.compose_words, before_words);
    assert!(f.state.node_slots.is_empty() && f.state.installed.is_empty());
    assert_eq!(f.state.gpu.as_ref().unwrap().shape, shape);
    assert_eq!(f.growth_events(), 0);
    eprintln!(
        "[ShGrowthProof] loader-valid retained metadata corruption at dense limit remains named fatal error; ran on GPU"
    );
}

#[test]
fn sh_growth_slot_address_overflow_escapes_drain_refusal_classifier() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH fatal slot address overflow")
    else {
        return;
    };
    let map = SyntheticMap::new([4, 4, 4], [1, 1, 1])
        .direct_base_only()
        .with_empty_rows(27, &[0]);
    let mut f = UploadOrderSh::with_map(&device, &raw, 64, map);
    let ready = loader_validated_chunk(&f);
    let node = f.state.nodes_by_owner[&0][0];
    // An existing canonical CPU range outside the 27-bit wire address space
    // reaches the real patch rewrite, without allocating a huge GPU atlas.
    let range = crate::render::sh_streaming::PoolRange {
        start: 1 << 27,
        len: 64,
    };
    f.state.node_slots.insert(node, range);
    let before_dense = f.state.dense_slots.clone();
    let before_words = f.state.compose_words.clone();
    let queue = UploadQueue::new(&device, raw, true);
    assert!(matches!(
        f.state.drain(
            &device,
            &queue,
            &mut f.sh,
            &f.uniforms,
            &f.weights,
            ShDrainBatch {
                generation: f.state.generation,
                content_tag: f.state.content_tag,
                ready: vec![ready],
                ..Default::default()
            },
        ),
        Err(ShResidencyDrainError::SlotOverflow)
    ));
    assert_eq!(f.state.node_slots[&node], range);
    assert_eq!(f.state.dense_slots, before_dense);
    assert_eq!(f.state.compose_words, before_words);
    assert!(f.state.installed.is_empty());
    assert_eq!(f.growth_events(), 0);
    eprintln!(
        "[ShGrowthProof] loader-valid canonical cluster slot overflow escapes drain classifier; ran on GPU"
    );
}

/// Publish the tiny GPU fixture through the exact shared id-50 validator that
/// the loader uses. The synthetic GPU harness normally omits the wire table;
/// here it is complete and hashed, so a renderer metadata mismatch is proven
/// independently of corrupt-on-disk handling.
fn loader_validated_chunk(f: &UploadOrderSh) -> postretro_level_loader::PreparedShCluster {
    use postretro_level_format::cluster_directory::{
        ClusterDirectorySection, ClusterRangeRecord, ClusterRangeRole, ClusterRecord,
        ClusterResourceDomain, ClusterResourceRecord, DENSE_OWNER_SENTINEL,
    };
    use postretro_level_format::cluster_sh_payloads::*;
    use postretro_level_format::delta_sh_volumes::DELTA_SH_VOLUMES_VERSION;
    use postretro_level_format::direct_sh_volume::DIRECT_SH_VOLUME_VERSION;
    use postretro_level_format::sh_volume::SH_VOLUME_VERSION;
    let prepared = f.prepared(0);
    let mut blocks = prepared.chunk.blocks.clone();
    blocks.sort_by_key(|block| (block.section_id, block.kind));
    let mut bodies = Vec::new();
    for block in &blocks {
        let mut body = prepared.chunk.block_bytes(block).to_vec();
        if block.kind == 0 {
            for patch in body.as_chunks_mut::<16>().0 {
                patch[8..12].fill(0);
            }
        }
        if block.kind == 1 {
            let layout = cluster_sh_isolated_atlas_array_layout(block.element_count).unwrap();
            body[8..12].copy_from_slice(&layout.atlas_width.to_le_bytes());
            body[12..16].copy_from_slice(&layout.atlas_height.to_le_bytes());
            body[16..20].copy_from_slice(&layout.layer_count.to_le_bytes());
        }
        bodies.push(body);
    }
    let decoded_bytes = bodies.iter().map(|body| body.len() as u64).sum();
    let mut wire = Vec::new();
    for word in [CLUSTER_SH_PAYLOADS_VERSION, 0, blocks.len() as u32, 0] {
        wire.extend_from_slice(&word.to_le_bytes());
    }
    let mut offset =
        (CLUSTER_SH_CHUNK_HEADER_SIZE + blocks.len() * CLUSTER_SH_BLOCK_RECORD_SIZE) as u64;
    for (block, body) in blocks.iter().zip(&bodies) {
        for word in [block.section_id, block.kind, block.element_count, 0] {
            wire.extend_from_slice(&word.to_le_bytes());
        }
        wire.extend_from_slice(&offset.to_le_bytes());
        wire.extend_from_slice(&(body.len() as u64).to_le_bytes());
        offset += body.len() as u64;
    }
    for body in bodies {
        wire.extend_from_slice(&body);
    }
    let mut base = f.map.base();
    base.tile_dimension = CLUSTER_SH_LOGICAL_TILE_DIMENSION;
    let sources = f.map.sources();
    let sparse = sources.indirect_delta.as_ref().unwrap();
    let inventory = [
        ClusterShPayloadsSourceMetadata::Sparse {
            section_id: 27,
            internal_version: u32::from(DELTA_SH_VOLUMES_VERSION),
            affinity_dimensions: [1; 3],
            tile_dimension: CLUSTER_SH_LOGICAL_TILE_DIMENSION,
            tile_border: CLUSTER_SH_LOGICAL_TILE_BORDER,
            valid_probe_masks: &sparse.valid_probe_masks,
            cell_levels: &sparse.cell_levels,
            affinity_offsets: &sparse.affinity_offsets,
            affinity_lights: &sparse.affinity_lights,
        },
        ClusterShPayloadsSourceMetadata::Dense {
            section_id: 34,
            internal_version: SH_VOLUME_VERSION,
            irradiance_format: base.irradiance_format,
        },
        ClusterShPayloadsSourceMetadata::Dense {
            section_id: 35,
            internal_version: DIRECT_SH_VOLUME_VERSION,
            irradiance_format: base.irradiance_format,
        },
    ];
    let directory = ClusterDirectorySection {
        runtime_cell_count: 1,
        primitive_limit: 64,
        cell_limit: 32,
        clusters: vec![ClusterRecord {
            bounds_min: [0.0; 3],
            bounds_max: [4.0; 3],
            member_start: 0,
            member_count: 1,
            range_start: 0,
            range_count: 3,
            primitive_count: 0,
            flags: 0,
        }],
        resources: vec![
            ClusterResourceRecord {
                section_id: 27,
                domain: ClusterResourceDomain::AffinityCell,
                dimensions: [1; 3],
            },
            ClusterResourceRecord {
                section_id: 34,
                domain: ClusterResourceDomain::DenseProbe,
                dimensions: [4; 3],
            },
            ClusterResourceRecord {
                section_id: 35,
                domain: ClusterResourceDomain::DenseProbe,
                dimensions: [4; 3],
            },
        ],
        members: vec![0],
        ranges: vec![
            ClusterRangeRecord {
                resource_index: 0,
                start: 0,
                count: 1,
                owner_cluster_id: 0,
                role: ClusterRangeRole::Owned,
            },
            ClusterRangeRecord {
                resource_index: 1,
                start: 0,
                count: 64,
                owner_cluster_id: DENSE_OWNER_SENTINEL,
                role: ClusterRangeRole::Dense,
            },
            ClusterRangeRecord {
                resource_index: 2,
                start: 0,
                count: 64,
                owner_cluster_id: DENSE_OWNER_SENTINEL,
                role: ClusterRangeRole::Dense,
            },
        ],
        seam_portal_ids: vec![],
        cluster_hints: vec![],
    };
    let section = ClusterShPayloadsSection {
        header: ClusterShPayloadsHeader {
            cluster_count: 1,
            source_count: 3,
            grid_dimensions: [4; 3],
            affinity_dimensions: [1; 3],
            payload_bytes: wire.len() as u64,
        },
        sources: inventory
            .iter()
            .map(|source| ClusterShPayloadsSourceRecord {
                section_id: source.section_id(),
                internal_version: source.internal_version(),
                kind: source.kind(),
            })
            .collect(),
        index: vec![ClusterShPayloadsIndexRecord {
            payload_offset: 0,
            payload_len: wire.len() as u64,
            decoded_bytes,
            requested_resident_bytes: 64 * 512 * 2,
            stored_tile_count: 64,
            dense_patch_count: 64,
            affinity_patch_count: 1,
            hash: *blake3::hash(&wire).as_bytes(),
        }],
    };
    let validated = section
        .into_validated(ClusterShPayloadsValidationInputs {
            directory: &directory,
            base: ClusterShPayloadsBaseMetadata {
                grid_dimensions: base.grid_dimensions,
                tile_dimension: base.tile_dimension,
                tile_border: base.tile_border,
                irradiance_format: base.irradiance_format,
                probes: &base.probes,
            },
            sources: &inventory,
        })
        .expect("tiny published id-50 metadata validates through the loader substrate");
    let chunk = validated
        .decode_chunk(0, wire)
        .expect("loader hash and canonical body validation");
    postretro_level_loader::PreparedShCluster {
        chunk,
        generation: prepared.generation,
        content_tag: prepared.content_tag,
    }
}

fn first_buffer_word(device: &wgpu::Device, queue: &UploadQueue, source: &wgpu::Buffer) -> u32 {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("SH sampled-indirection proof readback"),
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(source, 0, &buffer, 0, 4);
    queue.submit([encoder.finish()]);
    let (send, receive) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            send.send(result).unwrap();
        });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    receive.recv().unwrap().unwrap();
    let word = u32::from_le_bytes(
        buffer.slice(..).get_mapped_range().unwrap()[..4]
            .try_into()
            .unwrap(),
    );
    buffer.unmap();
    word
}

#[test]
fn sh_growth_floor_geometry_and_charges_match_gpu_for_every_load_shape() {
    let Some(GpuCtx { device, queue: raw }) = bc_gpu_or_skip("SH shared floor geometry") else {
        return;
    };
    for (grid, level, max_dimension, max_layers, extra) in [
        ([36, 4, 4], 0, 8192, 256, 0),
        ([36, 4, 4], 0, 8192, 4, DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES),
        ([4, 4, 4], 2, 8192, 256, 0),
        ([12, 4, 4], 0, 64, 4, 0),
    ] {
        let map = SyntheticMap::new(grid, [1, 1, 1]).direct_base_only();
        let levels = vec![level; map.cluster_count() as usize];
        let mut f = UploadOrderSh::with_map(&device, &raw, 128, map.with_density_levels(&levels));
        let grant = device.limits();
        let mut limits = grant.clone();
        limits.max_texture_dimension_2d = max_dimension.min(grant.max_texture_dimension_2d);
        limits.max_texture_array_layers = max_layers.min(grant.max_texture_array_layers);
        f.state
            .initialize_gpu_with_limits(
                &device,
                &raw,
                &f.map.base(),
                &f.map.sources(),
                false,
                &mut f.sh,
                &f.uniforms,
                &f.weights,
                &limits,
                extra,
            )
            .unwrap();
        let minimum = f
            .state
            .nodes_by_owner
            .values()
            .map(|nodes| {
                nodes
                    .iter()
                    .map(|node| f.state.node_layouts[node].tile_count)
                    .sum::<u32>()
            })
            .max()
            .unwrap();
        let whole = f
            .state
            .node_layouts
            .values()
            .map(|layout| layout.global_base_slot + layout.tile_count)
            .max()
            .unwrap();
        let fixed =
            initial_fixed_metadata_bytes(&f.map.base(), &f.map.sources(), &limits, &f.sh).unwrap();
        let floor = plan_initial_pool_floor(
            &f.map.base(),
            &f.map.sources(),
            minimum,
            whole,
            &f.state.sparse_capacity_floors,
            fixed + extra,
            f.sh.billboard_direct_scatter.capacity_bytes(),
            &limits,
        )
        .unwrap();
        let pools = f.state.gpu.as_ref().unwrap();
        assert_eq!(
            pools.shape,
            AtlasShape::for_slots(floor.dense_slots, whole, &limits).unwrap()
        );
        assert_eq!(
            floor.effective_floor_bytes,
            fixed
                + extra
                + f.sh.billboard_direct_scatter.capacity_bytes()
                + pools.active_capacity_bytes
        );
        assert_eq!(
            floor.dense_group_minimum_bytes,
            u64::from(pools.shape.capacity_for_slots(minimum).unwrap()) * 1536
        );
        assert_eq!(
            pools.dense_group_minimum_bytes,
            floor.dense_group_minimum_bytes
        );
        assert_eq!(
            pools.shape.layers,
            floor.dense_slots.div_ceil(pools.shape.tiles_per_layer)
        );
        assert!(pools.shape.layers <= limits.max_texture_array_layers);
        match (grid, level, extra) {
            ([36, 4, 4], 0, 0) => assert_eq!(
                (pools.shape.tiles_per_row, pools.shape.tiles_per_layer),
                (24, 576)
            ),
            ([36, 4, 4], 0, _) => assert!(pools.shape.tiles_per_layer > minimum),
            ([4, 4, 4], 2, _) => assert_eq!(pools.shape.tiles_per_layer, 1),
            ([12, 4, 4], 0, _) => {
                assert_eq!((pools.shape.tiles_per_layer, pools.shape.layers), (64, 3))
            }
            _ => unreachable!(),
        }
    }
    eprintln!(
        "[ShGrowthProof] shared floor/GPU geometry: floor-share, whole minimum, tiny, full layer; exact charges and writer minimum ran"
    );
}

fn first_texture_pixel(
    device: &wgpu::Device,
    queue: &UploadQueue,
    texture: &wgpu::Texture,
) -> [u8; 8] {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("SH retained dense texel proof"),
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(1),
            },
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let (send, receive) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            send.send(result).unwrap();
        });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    receive.recv().unwrap().unwrap();
    let pixel = buffer.slice(..).get_mapped_range().unwrap()[..8]
        .try_into()
        .unwrap();
    buffer.unmap();
    pixel
}
