use super::*;

use postretro_level_loader::ShStreamDirectMetadata;

fn base(format: u32) -> ShStreamBaseMetadata {
    ShStreamBaseMetadata {
        grid_origin: [0.0; 3],
        cell_size: [1.0; 3],
        grid_dimensions: [1, 1, 1],
        probe_stride: 8,
        tile_dimension: 6,
        tile_border: 1,
        atlas_dimensions: [8, 8],
        layer_count: 1,
        tiles_per_layer: 1,
        atlas_tiles_per_row: 1,
        irradiance_format: format,
        probes: Vec::new(),
        animation_descriptors: Vec::new(),
        slot_for_map_light: Vec::new(),
    }
}

fn sparse_source(section_id: u32, entries: u32) -> ShStreamSparseMetadata {
    ShStreamSparseMetadata {
        section_id,
        internal_version: 1,
        affinity_dims: [1, 1, 1],
        tile_dimension: 6,
        tile_border: 1,
        valid_probe_masks: vec![1],
        cell_levels: vec![0],
        affinity_offsets: vec![0, entries],
        affinity_lights: vec![0; entries as usize],
        animation_descriptor_indices: Vec::new(),
    }
}

fn direct(format: u32) -> ShStreamDirectMetadata {
    ShStreamDirectMetadata {
        grid_origin: [0.0; 3],
        cell_size: [1.0; 3],
        grid_dimensions: [1, 1, 1],
        tile_dimension: 6,
        tile_border: 1,
        atlas_dimensions: [8, 8],
        layer_count: 1,
        tiles_per_layer: 1,
        atlas_tiles_per_row: 1,
        irradiance_format: format,
    }
}

#[test]
fn floor_adds_proportional_capacity_to_dense_minimum() {
    let plan = plan_initial_pool_floor(
        &base(IRRADIANCE_FORMAT_RGBA16F),
        &ShStreamSourceMetadata::default(),
        4,
        1_000_000,
        &BTreeMap::new(),
        1024,
        2048,
        &wgpu::Limits::default(),
    )
    .unwrap();

    assert!(plan.dense_slots > 4);
    assert!(plan.dense_slots <= 1_000_000);
    assert!(plan.sparse_capacities.is_empty());
    assert!(plan.effective_floor_bytes >= DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES);
}

#[test]
fn proportional_share_uses_maximum_not_an_additive_minimum_remainder() {
    let shares = capped_proportional_shares(100, &[(90, 90), (10, 10)]).unwrap();
    let dense = dense_slots_for_share(80, 90, 1, shares[0]).unwrap();
    let sparse = dense_slots_for_share(0, 10, 1, shares[1]).unwrap();

    assert_eq!(shares, vec![90, 10]);
    assert_eq!((dense, sparse), (90, 10));
}

#[test]
fn capped_family_redistributes_its_unused_share() {
    assert_eq!(
        capped_proportional_shares(100, &[(90, 90), (10, 2), (50, 50)]).unwrap(),
        vec![63, 2, 35]
    );
}

#[test]
fn mandatory_cluster_minimum_can_raise_the_effective_floor() {
    let plan = plan_initial_pool_floor(
        &base(IRRADIANCE_FORMAT_RGBA16F),
        &ShStreamSourceMetadata::default(),
        300_000,
        300_000,
        &BTreeMap::new(),
        0,
        0,
        &wgpu::Limits::default(),
    )
    .unwrap();

    assert_eq!(plan.dense_slots, 300_000);
    assert!(plan.effective_floor_bytes > DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES);
}

#[test]
fn floor_charges_final_layer_padding_not_only_requested_dense_slots() {
    let plan = plan_initial_pool_floor(
        &base(IRRADIANCE_FORMAT_BC6H),
        &ShStreamSourceMetadata::default(),
        3,
        3,
        &BTreeMap::new(),
        DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES,
        0,
        &wgpu::Limits::default(),
    )
    .unwrap();

    assert_eq!(plan.dense_slots, 3);
    // A 3-slot request uses a 2 by 2 physical atlas: BC6H base plus
    // RGBA16F composed storage costs 64 + 512 bytes per physical slot.
    assert_eq!(
        plan.effective_floor_bytes,
        DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES + 4 * (64 + 512)
    );
}

#[test]
fn floor_charges_every_coupled_direct_dense_texture() {
    let sources = ShStreamSourceMetadata {
        direct: Some(direct(IRRADIANCE_FORMAT_BC6H)),
        direct_delta: Some(sparse_source(41, 0)),
        animated_direct_delta: Some(sparse_source(45, 0)),
        ..Default::default()
    };
    let plan = plan_initial_pool_floor(
        &base(IRRADIANCE_FORMAT_BC6H),
        &sources,
        3,
        100,
        &BTreeMap::from([(41, (1, 2)), (45, (1, 2))]),
        17,
        19,
        &wgpu::Limits::default(),
    )
    .unwrap();

    // id34 BC6H base + indirect total + id35 BC6H base + direct total
    // + animated direct intermediate: 64 + 512 + 64 + 512 + 512 bytes.
    let expected_dense_bytes = 100_u64 * 1_664;
    let expected_sparse_bytes = 2 * sparse_capacity_bytes((1, 2)).unwrap();
    assert_eq!(plan.dense_slots, 100);
    assert_eq!(plan.dense_group_minimum_bytes, 4 * 1_664);
    assert_eq!(
        plan.sparse_group_minimum_bytes[&41],
        sparse_capacity_bytes((1, 2)).unwrap()
    );
    assert_eq!(
        plan.sparse_group_minimum_bytes[&45],
        sparse_capacity_bytes((1, 2)).unwrap()
    );
    assert_eq!(
        plan.effective_floor_bytes,
        17 + 19 + expected_dense_bytes + expected_sparse_bytes
    );
}

#[test]
fn sparse_capacity_is_word_aligned_and_capped_at_whole_source() {
    let source = sparse_source(27, 2);
    let sources = ShStreamSourceMetadata {
        indirect_delta: Some(source.clone()),
        ..Default::default()
    };
    let (_, whole_tiles) = whole_sparse_payload(&source).unwrap();
    let expected = whole_sparse_capacity(2, whole_tiles).unwrap();
    let plan = plan_initial_pool_floor(
        &base(IRRADIANCE_FORMAT_RGBA16F),
        &sources,
        1,
        1,
        &BTreeMap::from([(27, (2, 3))]),
        0,
        0,
        &wgpu::Limits::default(),
    )
    .unwrap();

    assert_eq!(plan.sparse_capacities[&27], expected);
    assert_eq!(plan.sparse_capacities[&27].1 % 2, 0);
}

#[test]
fn all_invalid_dense_source_keeps_one_dummy_binding_slot() {
    let plan = plan_initial_pool_floor(
        &base(IRRADIANCE_FORMAT_RGBA16F),
        &ShStreamSourceMetadata::default(),
        0,
        0,
        &BTreeMap::new(),
        0,
        0,
        &wgpu::Limits::default(),
    )
    .unwrap();

    assert_eq!(plan.dense_slots, 1);
    assert_eq!(plan.effective_floor_bytes, 1_024);
}

#[test]
fn floor_rejects_an_adapter_without_an_8x8_cell() {
    let mut limits = wgpu::Limits::default();
    limits.max_texture_dimension_2d = 7;
    assert_eq!(
        plan_initial_pool_floor(
            &base(IRRADIANCE_FORMAT_RGBA16F),
            &ShStreamSourceMetadata::default(),
            1,
            1,
            &BTreeMap::new(),
            0,
            0,
            &limits,
        ),
        Err(ShResidencyDrainError::GpuCapacity {
            reason: "adapter cannot allocate an 8x8 streamed SH cell",
        })
    );
}

#[test]
fn floor_rejects_sparse_fixed_or_active_storage_above_adapter_limits() {
    let sources = ShStreamSourceMetadata {
        indirect_delta: Some(sparse_source(27, 1)),
        ..Default::default()
    };
    let mut limits = wgpu::Limits::default();
    limits.max_buffer_size = 8;
    limits.max_storage_buffer_binding_size = 8;
    assert_eq!(
        plan_initial_pool_floor(
            &base(IRRADIANCE_FORMAT_RGBA16F),
            &sources,
            1,
            1,
            &BTreeMap::from([(27, (1, 2))]),
            0,
            0,
            &limits,
        ),
        Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed sparse compaction table",
        })
    );
}

#[test]
fn floor_rejects_indirection_before_any_gpu_allocation() {
    let mut limits = wgpu::Limits::default();
    limits.max_buffer_size = 3;
    limits.max_storage_buffer_binding_size = 3;
    assert_eq!(
        plan_initial_pool_floor(
            &base(IRRADIANCE_FORMAT_RGBA16F),
            &ShStreamSourceMetadata::default(),
            1,
            1,
            &BTreeMap::new(),
            0,
            0,
            &limits,
        ),
        Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed SH compose indirection",
        })
    );
}

#[test]
fn floor_rejects_an_absent_indirect_carrier_above_adapter_limits() {
    let mut base = base(IRRADIANCE_FORMAT_RGBA16F);
    base.grid_dimensions = [8, 1, 1];
    let mut limits = wgpu::Limits::default();
    limits.max_buffer_size = 8;
    limits.max_storage_buffer_binding_size = 8;
    assert_eq!(
        plan_initial_pool_floor(
            &base,
            &ShStreamSourceMetadata::default(),
            1,
            1,
            &BTreeMap::new(),
            0,
            0,
            &limits,
        ),
        Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed sparse absent CSR row-pair table",
        })
    );
}

#[test]
fn floor_rejects_an_absent_direct_promotion_carrier_above_adapter_limits() {
    let mut base = base(IRRADIANCE_FORMAT_RGBA16F);
    base.grid_dimensions = [8, 1, 1];
    let sources = ShStreamSourceMetadata {
        indirect_delta: Some(sparse_source(27, 0)),
        direct: Some(direct(IRRADIANCE_FORMAT_RGBA16F)),
        animated_direct_delta: Some(sparse_source(45, 0)),
        ..Default::default()
    };
    let mut limits = wgpu::Limits::default();
    limits.max_buffer_size = 16;
    limits.max_storage_buffer_binding_size = 16;
    assert_eq!(
        plan_initial_pool_floor(
            &base,
            &sources,
            1,
            1,
            &BTreeMap::from([(27, (1, 2)), (45, (1, 2))]),
            0,
            0,
            &limits,
        ),
        Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed sparse absent compaction table",
        })
    );
}

#[test]
fn floor_rejects_overflowing_fixed_physical_bytes() {
    assert_eq!(
        plan_initial_pool_floor(
            &base(IRRADIANCE_FORMAT_RGBA16F),
            &ShStreamSourceMetadata::default(),
            1,
            1,
            &BTreeMap::new(),
            u64::MAX,
            1,
            &wgpu::Limits::default(),
        ),
        Err(ShResidencyDrainError::SlotOverflow)
    );
}
