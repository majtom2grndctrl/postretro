use super::*;

use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
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

fn dense_bound(dense_slots: u32) -> ShPoolBound {
    ShPoolBound {
        dense_slots,
        sparse: BTreeMap::new(),
    }
}

fn sparse_bound(section_id: u32, worst: (u32, u32)) -> ShPoolBound {
    ShPoolBound {
        dense_slots: 1,
        sparse: BTreeMap::from([(section_id, worst)]),
    }
}

// Rows from `research.md` where the figures are real: the hallway, where the
// headroom stands, and `campaign-test`, where the whole map caps it.
#[test]
fn initial_pool_is_bound_plus_headroom_capped() {
    for (bound, whole, expected) in [
        (168_017, 731_008, 210_022),
        (39_634, 41_726, 41_726),
        (18_327, 18_327, 18_327),
        (4, 1_000_000, 5),
    ] {
        let plan = plan_initial_pool(
            &base(IRRADIANCE_FORMAT_RGBA16F),
            &ShStreamSourceMetadata::default(),
            false,
            &dense_bound(bound),
            whole,
            &wgpu::Limits::default(),
        )
        .unwrap();
        assert_eq!(plan.dense_slots, expected, "bound {bound}, whole {whole}");
        assert_eq!(plan.whole_map.dense_slots, whole);
    }
}

#[test]
fn sparse_takes_the_same_headroom_plus_its_sentinel() {
    let sources = ShStreamSourceMetadata {
        indirect_delta: Some(sparse_source(27, 100)),
        ..Default::default()
    };
    let plan = plan_initial_pool(
        &base(IRRADIANCE_FORMAT_RGBA16F),
        &sources,
        composes_direct(&sources),
        &sparse_bound(27, (10, 20)),
        100,
        &wgpu::Limits::default(),
    )
    .unwrap();
    assert_eq!(plan.sparse_capacities[&27], (13 + 1, 26 + 2));
}

// Eviction stops only once every family holds the map, so a family sized from
// the bound would keep a dense-whole level evicting for no reason.
#[test]
fn a_dense_pool_holding_the_map_starts_every_family_whole() {
    let source = sparse_source(27, 100);
    let sources = ShStreamSourceMetadata {
        indirect_delta: Some(source.clone()),
        ..Default::default()
    };
    let plan = plan_initial_pool(
        &base(IRRADIANCE_FORMAT_RGBA16F),
        &sources,
        composes_direct(&sources),
        &ShPoolBound {
            dense_slots: 90,
            sparse: BTreeMap::from([(27, (10, 20))]),
        },
        100,
        &wgpu::Limits::default(),
    )
    .unwrap();
    assert_eq!(plan.dense_slots, 100);
    let whole = with_sentinel(whole_sparse_payload(&source).unwrap()).unwrap();
    assert_eq!(plan.sparse_capacities[&27], whole);
}

#[test]
fn a_family_past_the_device_stays_at_its_bound_when_dense_holds_the_map() {
    let sources = ShStreamSourceMetadata {
        indirect_delta: Some(sparse_source(27, 100)),
        ..Default::default()
    };
    let limits = wgpu::Limits {
        max_buffer_size: 256,
        max_storage_buffer_binding_size: 256,
        ..Default::default()
    };
    let plan = plan_initial_pool(
        &base(IRRADIANCE_FORMAT_RGBA16F),
        &sources,
        composes_direct(&sources),
        &ShPoolBound {
            dense_slots: 1,
            sparse: BTreeMap::from([(27, (10, 20))]),
        },
        1,
        &limits,
    )
    .unwrap();
    assert_eq!(plan.sparse_capacities[&27], (13 + 1, 26 + 2));
}

#[test]
fn sparse_is_word_aligned_and_capped_at_its_whole_source() {
    let source = sparse_source(27, 2);
    let sources = ShStreamSourceMetadata {
        indirect_delta: Some(source.clone()),
        ..Default::default()
    };
    let (whole_entries, whole_tiles) = whole_sparse_payload(&source).unwrap();
    let whole = with_sentinel((whole_entries, whole_tiles)).unwrap();
    let plan = |worst| {
        plan_initial_pool(
            &base(IRRADIANCE_FORMAT_RGBA16F),
            &sources,
            composes_direct(&sources),
            &sparse_bound(27, worst),
            100,
            &wgpu::Limits::default(),
        )
        .unwrap()
    };

    assert_eq!(plan((2, 3)).sparse_capacities[&27].1 % 2, 0);
    assert_eq!(plan((u32::MAX, u32::MAX)).sparse_capacities[&27], whole);
    assert_eq!(plan((2, 3)).whole_map.sparse[&27], whole);
}

// An odd stride makes a row's reservation exceed its payload. A cap of the
// plain sum would refuse the last rows of a wholly mandatory map.
#[test]
fn whole_sparse_cap_counts_each_rows_word_reservation() {
    let source = ShStreamSparseMetadata {
        affinity_dims: [2, 1, 1],
        tile_dimension: 5,
        valid_probe_masks: vec![1, 1],
        cell_levels: vec![0, 0],
        affinity_offsets: vec![0, 1, 2],
        affinity_lights: vec![0, 0],
        ..sparse_source(27, 0)
    };
    assert_eq!(delta_probe_f16_stride(5) % 2, 1);
    let (entries, tiles) = whole_sparse_payload(&source).unwrap();
    assert_eq!(entries, 2);
    assert_eq!(tiles, 2 * (delta_probe_f16_stride(5) as u32 + 1));
}

#[test]
fn a_zero_family_in_the_bound_starts_at_its_sentinel() {
    let sources = ShStreamSourceMetadata {
        indirect_delta: Some(sparse_source(27, 4)),
        ..Default::default()
    };
    let plan = plan_initial_pool(
        &base(IRRADIANCE_FORMAT_RGBA16F),
        &sources,
        composes_direct(&sources),
        &sparse_bound(27, (0, 0)),
        100,
        &wgpu::Limits::default(),
    )
    .unwrap();
    assert_eq!(plan.sparse_capacities[&27], (1, 2));
}

// Regression: an omitted family was read as a zero bound, so corrupt data sized
// a pool from nothing.
#[test]
fn a_bound_omitting_a_streamed_family_is_refused_by_name() {
    let sources = ShStreamSourceMetadata {
        indirect_delta: Some(sparse_source(27, 4)),
        ..Default::default()
    };
    let error = plan_initial_pool(
        &base(IRRADIANCE_FORMAT_RGBA16F),
        &sources,
        composes_direct(&sources),
        &dense_bound(1),
        100,
        &wgpu::Limits::default(),
    )
    .unwrap_err();
    assert_eq!(
        error,
        ShResidencyDrainError::MandatoryBoundMissingFamily { section_id: 27 }
    );
    assert!(error.to_string().contains("id-27"));
}

// Regression: an extra family failed as a capacity problem, as if the device
// were short.
#[test]
fn a_bound_naming_an_absent_source_is_refused_by_name() {
    let error = plan_initial_pool(
        &base(IRRADIANCE_FORMAT_RGBA16F),
        &ShStreamSourceMetadata::default(),
        false,
        &sparse_bound(48, (1, 2)),
        1,
        &wgpu::Limits::default(),
    )
    .unwrap_err();
    assert_eq!(
        error,
        ShResidencyDrainError::MandatoryBoundExtraFamily { section_id: 48 }
    );
    assert!(error.to_string().contains("id-48"));
}

// Regression: a bound the device could hold failed the load once headroom took
// it past the limit. Sixteen words and one row leave 13 entries.
#[test]
fn headroom_clamps_to_a_sparse_device_limit_and_only_the_bound_can_fail_it() {
    let sources = ShStreamSourceMetadata {
        indirect_delta: Some(sparse_source(27, 100)),
        ..Default::default()
    };
    let limits = wgpu::Limits {
        max_buffer_size: 64,
        max_storage_buffer_binding_size: 64,
        ..Default::default()
    };
    let plan = |worst| {
        plan_initial_pool(
            &base(IRRADIANCE_FORMAT_RGBA16F),
            &sources,
            composes_direct(&sources),
            &sparse_bound(27, worst),
            100,
            &limits,
        )
    };

    assert_eq!(plan((10, 20)).unwrap().sparse_capacities[&27], (13, 28));
    assert_eq!(
        plan((20, 20)),
        Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed sparse entry pool",
        })
    );
}

// Dense takes the same rule: four layers of 64 tiles hold a bound of 240, whose
// headroom asks for 300.
#[test]
fn headroom_clamps_to_the_dense_device_limit_and_only_the_bound_can_fail_it() {
    let limits = wgpu::Limits {
        max_texture_dimension_2d: 64,
        max_texture_array_layers: 4,
        ..Default::default()
    };
    let plan = |bound| {
        plan_initial_pool(
            &base(IRRADIANCE_FORMAT_RGBA16F),
            &ShStreamSourceMetadata::default(),
            false,
            &dense_bound(bound),
            1_000,
            &limits,
        )
    };

    assert_eq!(plan(240).unwrap().dense_slots, 256);
    assert_eq!(
        plan(257),
        Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed SH pool needs unsupported array layers",
        })
    );
}

#[test]
fn pool_bytes_charge_final_layer_padding_not_only_requested_dense_slots() {
    let plan = plan_initial_pool(
        &base(IRRADIANCE_FORMAT_BC6H),
        &ShStreamSourceMetadata::default(),
        false,
        &dense_bound(9),
        9,
        &wgpu::Limits::default(),
    )
    .unwrap();

    assert_eq!(plan.dense_slots, 9);
    // A 9-slot request takes five 2-tile layers: BC6H base plus RGBA16F
    // composed storage costs 64 + 512 bytes per physical slot.
    assert_eq!(plan.pool_bytes, 10 * (64 + 512));
}

#[test]
fn pool_bytes_charge_every_coupled_direct_dense_texture() {
    let sources = ShStreamSourceMetadata {
        direct: Some(direct(IRRADIANCE_FORMAT_BC6H)),
        direct_delta: Some(sparse_source(41, 0)),
        animated_direct_delta: Some(sparse_source(45, 0)),
        ..Default::default()
    };
    let plan = plan_initial_pool(
        &base(IRRADIANCE_FORMAT_BC6H),
        &sources,
        composes_direct(&sources),
        &ShPoolBound {
            dense_slots: u32::MAX,
            sparse: BTreeMap::from([(41, (0, 0)), (45, (0, 0))]),
        },
        100,
        &wgpu::Limits::default(),
    )
    .unwrap();

    // id34 BC6H base + indirect total + id35 BC6H base + promotion-free
    // direct atlas + promotion handoff atlas + animated direct intermediate:
    // 64 + 512 + 64 + 512 + 512 + 512 bytes.
    assert_eq!(plan.dense_slots, 100);
    // Seven 16-tile layers.
    assert_eq!(
        plan.pool_bytes,
        112 * 2_176 + 2 * sh_sparse_capacity_bytes(1, 2)
    );
}

#[test]
fn all_invalid_dense_source_keeps_one_dummy_binding_slot() {
    let plan = plan_initial_pool(
        &base(IRRADIANCE_FORMAT_RGBA16F),
        &ShStreamSourceMetadata::default(),
        false,
        &dense_bound(0),
        0,
        &wgpu::Limits::default(),
    )
    .unwrap();

    assert_eq!(plan.dense_slots, 1);
    assert_eq!(plan.pool_bytes, 1_024);
}

#[test]
fn a_zero_bound_on_a_real_map_still_starts_with_capacity() {
    let plan = plan_initial_pool(
        &base(IRRADIANCE_FORMAT_RGBA16F),
        &ShStreamSourceMetadata::default(),
        false,
        &dense_bound(0),
        64,
        &wgpu::Limits::default(),
    )
    .unwrap();
    assert_eq!(plan.dense_slots, 1);
}

#[test]
fn pool_rejects_an_adapter_without_an_8x8_cell() {
    let limits = wgpu::Limits {
        max_texture_dimension_2d: 7,
        ..Default::default()
    };
    assert_eq!(
        plan_initial_pool(
            &base(IRRADIANCE_FORMAT_RGBA16F),
            &ShStreamSourceMetadata::default(),
            false,
            &dense_bound(1),
            1,
            &limits,
        ),
        Err(ShResidencyDrainError::GpuCapacity {
            reason: "adapter cannot allocate an 8x8 streamed SH cell",
        })
    );
}

#[test]
fn pool_rejects_sparse_fixed_or_active_storage_above_adapter_limits() {
    let sources = ShStreamSourceMetadata {
        indirect_delta: Some(sparse_source(27, 1)),
        ..Default::default()
    };
    let limits = wgpu::Limits {
        max_buffer_size: 8,
        max_storage_buffer_binding_size: 8,
        ..Default::default()
    };
    assert_eq!(
        plan_initial_pool(
            &base(IRRADIANCE_FORMAT_RGBA16F),
            &sources,
            composes_direct(&sources),
            &sparse_bound(27, (0, 0)),
            10,
            &limits,
        ),
        Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed sparse compaction table",
        })
    );
}

#[test]
fn pool_rejects_indirection_before_any_gpu_allocation() {
    let limits = wgpu::Limits {
        max_buffer_size: 3,
        max_storage_buffer_binding_size: 3,
        ..Default::default()
    };
    assert_eq!(
        plan_initial_pool(
            &base(IRRADIANCE_FORMAT_RGBA16F),
            &ShStreamSourceMetadata::default(),
            false,
            &dense_bound(1),
            1,
            &limits,
        ),
        Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed SH compose indirection",
        })
    );
}

#[test]
fn pool_rejects_an_absent_indirect_carrier_above_adapter_limits() {
    let mut base = base(IRRADIANCE_FORMAT_RGBA16F);
    base.grid_dimensions = [8, 1, 1];
    let limits = wgpu::Limits {
        max_buffer_size: 8,
        max_storage_buffer_binding_size: 8,
        ..Default::default()
    };
    assert_eq!(
        plan_initial_pool(
            &base,
            &ShStreamSourceMetadata::default(),
            false,
            &dense_bound(1),
            1,
            &limits,
        ),
        Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed sparse absent CSR row-pair table",
        })
    );
}

#[test]
fn pool_rejects_an_absent_direct_promotion_carrier_above_adapter_limits() {
    let mut base = base(IRRADIANCE_FORMAT_RGBA16F);
    base.grid_dimensions = [8, 1, 1];
    let sources = ShStreamSourceMetadata {
        indirect_delta: Some(sparse_source(27, 0)),
        direct: Some(direct(IRRADIANCE_FORMAT_RGBA16F)),
        animated_direct_delta: Some(sparse_source(45, 0)),
        ..Default::default()
    };
    let limits = wgpu::Limits {
        max_buffer_size: 16,
        max_storage_buffer_binding_size: 16,
        ..Default::default()
    };
    assert_eq!(
        plan_initial_pool(
            &base,
            &sources,
            composes_direct(&sources),
            &dense_bound(1),
            1,
            &limits
        ),
        Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed sparse absent compaction table",
        })
    );
}

#[test]
fn impossible_streamed_pool_growth_is_a_named_gpu_capacity_error() {
    let limits = wgpu::Limits {
        max_texture_dimension_2d: 0,
        ..wgpu::Limits::default()
    };
    assert_eq!(
        AtlasShape::for_slots(1, 1, &limits),
        Err(ShResidencyDrainError::GpuCapacity {
            reason: "adapter cannot allocate an 8x8 streamed SH cell",
        })
    );
}

/// What the plan derives: a direct delta family means a direct compose.
fn composes_direct(sources: &ShStreamSourceMetadata) -> bool {
    sources.direct_delta.is_some() || sources.animated_direct_delta.is_some()
}
