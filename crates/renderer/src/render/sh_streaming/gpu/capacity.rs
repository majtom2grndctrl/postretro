//! Exact streamed-pool sizing and adapter-limit preflight.
//!
//! These pure helpers are shared by initial construction and the 256 MiB
//! floor planner. They reject unsupported backing before renderer-owned wgpu
//! allocation begins.

use super::*;

pub(in crate::render::sh_streaming) fn initial_fixed_metadata_bytes(
    base: &ShStreamBaseMetadata,
    sources: &ShStreamSourceMetadata,
    limits: &wgpu::Limits,
    sh: &ShVolumeResources,
) -> Result<u64, ShResidencyDrainError> {
    let indirection_len = u64::try_from(base.probes.len())
        .map_err(|_| ShResidencyDrainError::SlotOverflow)?
        .checked_mul(4)
        .ok_or(ShResidencyDrainError::SlotOverflow)?
        .max(4);
    let depth_extent = wgpu::Extent3d {
        width: base.grid_dimensions[0].max(1),
        height: base.grid_dimensions[1].max(1),
        depth_or_array_layers: base.grid_dimensions[2].max(1),
    };
    let mut bytes = indirection_len
        .checked_mul(2)
        .and_then(|bytes| bytes.checked_add(96))
        .and_then(|bytes| {
            bytes.checked_add(texture_bytes(wgpu::TextureFormat::Rgba16Uint, depth_extent))
        })
        .and_then(|bytes| bytes.checked_add(sh.animation.descriptors.size()))
        .and_then(|bytes| bytes.checked_add(sh.animation.anim_samples.size()))
        .and_then(|bytes| bytes.checked_add(sh.scripted_light_descriptors.size()))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    bytes = bytes
        .checked_add(indirect_fixed_metadata_bytes(
            base,
            sources.indirect_delta.as_ref(),
            limits,
        )?)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    if sources.direct_delta.is_some() || sources.animated_direct_delta.is_some() {
        bytes = bytes
            .checked_add(direct_pass_fixed_metadata_bytes(
                base,
                sources.direct_delta.as_ref(),
                limits,
                48,
            )?)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        if let Some(source) = sources.animated_direct_delta.as_ref() {
            let descriptor_bytes = u64::try_from(source.animation_descriptor_indices.len().max(1))
                .map_err(|_| ShResidencyDrainError::SlotOverflow)?
                .checked_mul(4)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            bytes = bytes
                .checked_add(direct_pass_fixed_metadata_bytes(
                    base,
                    Some(source),
                    limits,
                    1_040,
                )?)
                .and_then(|bytes| bytes.checked_add(descriptor_bytes))
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
        }
    }
    Ok(bytes)
}

/// Validate every storage/uniform carrier that the streamed constructors will
/// create, including the dummy bindings required when a sparse family is
/// absent. This runs before the first texture or buffer allocation so an
/// adapter-limit failure cannot leave a partially installed streamed level.
pub(super) fn preflight_initial_resource_limits(
    base: &ShStreamBaseMetadata,
    sources: &ShStreamSourceMetadata,
    sparse_compose: &SparseComposeLayouts,
    shape: AtlasShape,
    sparse_capacities: &SparseCapacityFloors,
    limits: &wgpu::Limits,
) -> Result<(), ShResidencyDrainError> {
    let indirection_len = u64::try_from(base.probes.len())
        .map_err(|_| ShResidencyDrainError::SlotOverflow)?
        .checked_mul(4)
        .ok_or(ShResidencyDrainError::SlotOverflow)?
        .max(4);
    validate_storage_limits(
        indirection_len,
        limits,
        "streamed SH compose indirection exceeds adapter storage limits",
    )?;
    validate_storage_limits(
        indirection_len,
        limits,
        "streamed SH sampled indirection exceeds adapter storage limits",
    )?;
    let depth_dimensions = base.grid_dimensions.map(|axis| axis.max(1));
    if depth_dimensions
        .into_iter()
        .any(|axis| axis > limits.max_texture_dimension_3d)
    {
        return Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed SH depth-moment grid exceeds adapter 3D texture limits",
        });
    }
    preflight_sparse_carrier(
        &sparse_compose.indirect,
        sparse_capacities.get(&27).copied(),
        limits,
        true,
    )?;
    if sources.direct_delta.is_some() || sources.animated_direct_delta.is_some() {
        preflight_sparse_carrier(
            planned_direct_layout(sparse_compose)?,
            sparse_capacities.get(&DIRECT_DELTA_SECTION).copied(),
            limits,
            false,
        )?;
        if sources.animated_direct_delta.is_some() {
            preflight_sparse_carrier(
                planned_animated_direct_layout(sparse_compose)?,
                sparse_capacities
                    .get(&ANIMATED_DIRECT_DELTA_SECTION)
                    .copied(),
                limits,
                true,
            )?;
        }
    }
    let extent = shape.extent();
    if extent.width > limits.max_texture_dimension_2d
        || extent.height > limits.max_texture_dimension_2d
        || extent.depth_or_array_layers > limits.max_texture_array_layers
    {
        return Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed dense atlas exceeds adapter texture limits",
        });
    }
    Ok(())
}

/// The planned direct-promotion layout; the plan derives it whenever any
/// direct compose runs.
pub(in crate::render::sh_streaming) fn planned_direct_layout(
    layouts: &SparseComposeLayouts,
) -> Result<&SparseComposeLayout, ShResidencyDrainError> {
    layouts
        .direct
        .as_ref()
        .ok_or(ShResidencyDrainError::GpuCapacity {
            reason: "streamed direct compose has no planned sparse layout",
        })
}

/// The planned animated-direct layout; the plan derives it whenever id 45
/// streams.
pub(in crate::render::sh_streaming) fn planned_animated_direct_layout(
    layouts: &SparseComposeLayouts,
) -> Result<&SparseComposeLayout, ShResidencyDrainError> {
    layouts
        .animated_direct
        .as_ref()
        .ok_or(ShResidencyDrainError::GpuCapacity {
            reason: "streamed animated direct compose has no planned sparse layout",
        })
}

fn preflight_sparse_carrier(
    layout: &SparseComposeLayout,
    sparse_floor: Option<(u32, u32)>,
    limits: &wgpu::Limits,
    has_descriptor_indices: bool,
) -> Result<(), ShResidencyDrainError> {
    let descriptors = &layout.animation_descriptor_indices;
    let entries = layout.entry_capacity(sparse_floor);
    let tiles = layout.tile_f16_capacity(sparse_floor);
    let rows = checked_cell_count(layout.affinity_dims)?;
    let grid_bytes = dynamic_grid_bytes(rows, limits)?;
    validate_uniform_limits(
        grid_bytes,
        limits,
        "streamed dirty compose records exceed adapter buffer limit",
    )?;
    let row_pairs = u64::from(rows.max(1))
        .checked_mul(8)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let entry_bytes = u64::from(entries)
        .checked_mul(4)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let tile_bytes = u64::from(tiles.div_ceil(2))
        .checked_mul(4)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let compaction_bytes = u64::from(rows)
        .checked_mul(3)
        .and_then(|words| words.checked_add(u64::from(entries)))
        .and_then(|words| words.checked_mul(4))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    for (bytes, reason) in [
        (
            row_pairs,
            "streamed sparse CSR row-pair table exceeds adapter storage limits",
        ),
        (
            entry_bytes,
            "streamed sparse entry pool exceeds adapter storage limits",
        ),
        (
            tile_bytes,
            "streamed sparse delta tile pool exceeds adapter storage limits",
        ),
        (
            compaction_bytes,
            "streamed sparse compaction table exceeds adapter storage limits",
        ),
    ] {
        validate_storage_limits(bytes, limits, reason)?;
    }
    if has_descriptor_indices {
        let descriptor_bytes = u64::try_from(descriptors.len().max(1))
            .map_err(|_| ShResidencyDrainError::SlotOverflow)?
            .checked_mul(4)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        validate_storage_limits(
            descriptor_bytes,
            limits,
            "streamed sparse descriptor indices exceed adapter storage limits",
        )?;
    }
    Ok(())
}

fn validate_storage_limits(
    bytes: u64,
    limits: &wgpu::Limits,
    reason: &'static str,
) -> Result<(), ShResidencyDrainError> {
    if bytes > limits.max_buffer_size || bytes > limits.max_storage_buffer_binding_size {
        return Err(ShResidencyDrainError::GpuCapacity { reason });
    }
    Ok(())
}

fn validate_uniform_limits(
    bytes: u64,
    limits: &wgpu::Limits,
    reason: &'static str,
) -> Result<(), ShResidencyDrainError> {
    if bytes > limits.max_buffer_size {
        return Err(ShResidencyDrainError::GpuCapacity { reason });
    }
    Ok(())
}

fn indirect_fixed_metadata_bytes(
    base: &ShStreamBaseMetadata,
    source: Option<&postretro_level_loader::ShStreamSparseMetadata>,
    limits: &wgpu::Limits,
) -> Result<u64, ShResidencyDrainError> {
    let (rows, descriptor_count, source_present) = sparse_fixed_shape(base, source)?;
    let grid = dynamic_grid_bytes(rows, limits)?;
    let pairs = u64::from(rows.max(1))
        .checked_mul(8)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let compaction_prefix = u64::from(rows)
        .checked_mul(3)
        .and_then(|words| words.checked_mul(4))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let descriptors = u64::try_from(descriptor_count.max(1))
        .map_err(|_| ShResidencyDrainError::SlotOverflow)?
        .checked_mul(4)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let dummy_backing = if source_present { 0 } else { 12 };
    grid.checked_add(pairs)
        .and_then(|bytes| bytes.checked_add(compaction_prefix))
        .and_then(|bytes| bytes.checked_add(32))
        .and_then(|bytes| bytes.checked_add(descriptors))
        .and_then(|bytes| bytes.checked_add(dummy_backing))
        .ok_or(ShResidencyDrainError::SlotOverflow)
}

fn direct_pass_fixed_metadata_bytes(
    base: &ShStreamBaseMetadata,
    source: Option<&postretro_level_loader::ShStreamSparseMetadata>,
    limits: &wgpu::Limits,
    pass_uniform_bytes: u64,
) -> Result<u64, ShResidencyDrainError> {
    let (rows, _, source_present) = sparse_fixed_shape(base, source)?;
    let grid = dynamic_grid_bytes(rows, limits)?;
    let pairs = u64::from(rows.max(1))
        .checked_mul(8)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let compaction_prefix = u64::from(rows)
        .checked_mul(3)
        .and_then(|words| words.checked_mul(4))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let dummy_backing = if source_present { 0 } else { 12 };
    grid.checked_add(pairs)
        .and_then(|bytes| bytes.checked_add(compaction_prefix))
        .and_then(|bytes| bytes.checked_add(pass_uniform_bytes))
        .and_then(|bytes| bytes.checked_add(dummy_backing))
        .ok_or(ShResidencyDrainError::SlotOverflow)
}

fn sparse_fixed_shape(
    base: &ShStreamBaseMetadata,
    source: Option<&postretro_level_loader::ShStreamSparseMetadata>,
) -> Result<(u32, usize, bool), ShResidencyDrainError> {
    match source {
        Some(source) => Ok((
            checked_cell_count(source.affinity_dims)?,
            source.animation_descriptor_indices.len(),
            true,
        )),
        None => Ok((
            checked_cell_count(base.grid_dimensions.map(|axis| axis.div_ceil(4)))?,
            0,
            false,
        )),
    }
}

fn dynamic_grid_bytes(rows: u32, limits: &wgpu::Limits) -> Result<u64, ShResidencyDrainError> {
    let record = u64::try_from(DYNAMIC_COMPOSE_GRID_DIMS_SIZE)
        .map_err(|_| ShResidencyDrainError::SlotOverflow)?;
    if record > limits.max_uniform_buffer_binding_size {
        return Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed compose gather record exceeds uniform binding limit",
        });
    }
    let alignment = u64::from(limits.min_uniform_buffer_offset_alignment.max(1));
    let stride = record
        .checked_add(alignment - 1)
        .and_then(|bytes| bytes.checked_div(alignment))
        .and_then(|records| records.checked_mul(alignment))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let chunk_capacity = crate::render::sh_compose_dispatch::gather_chunk_capacity(
        limits.max_compute_workgroups_per_dimension,
    )
    .ok_or(ShResidencyDrainError::GpuCapacity {
        reason: "streamed compose gather has zero row capacity",
    })?;
    let chunk_count = rows.max(1).div_ceil(chunk_capacity);
    let bytes = stride
        .checked_mul(u64::from(chunk_count))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    if bytes > limits.max_buffer_size {
        return Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed dirty compose records exceed adapter buffer limit",
        });
    }
    Ok(bytes)
}
