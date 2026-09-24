use super::*;

pub(super) fn validate_sparse_caps(
    sparse: &BTreeMap<u32, SparsePlan>,
    limits: &wgpu::Limits,
) -> Result<(), ShResidencyDrainError> {
    for plan in sparse.values() {
        let entry_bytes = u64::from(plan.capacity.0)
            .checked_mul(4)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let tile_bytes = u64::from(plan.capacity.1.div_ceil(2))
            .checked_mul(SPARSE_F16_WORD_BYTES)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let row_pair_bytes = u64::from(plan.cell_count.max(1))
            .checked_mul(8)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let compaction_bytes = u64::from(plan.cell_count)
            .checked_mul(3)
            .and_then(|words| words.checked_add(u64::from(plan.capacity.0)))
            .and_then(|words| words.checked_mul(4))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        validate_storage_buffer_size(entry_bytes, limits, "streamed sparse entry pool")?;
        validate_storage_buffer_size(tile_bytes, limits, "streamed sparse delta tile pool")?;
        validate_storage_buffer_size(row_pair_bytes, limits, "streamed sparse CSR row-pair table")?;
        validate_storage_buffer_size(compaction_bytes, limits, "streamed sparse compaction table")?;
        if plan.has_descriptor_indices {
            let descriptor_bytes = u64::try_from(plan.descriptor_index_count.max(1))
                .map_err(|_| ShResidencyDrainError::SlotOverflow)?
                .checked_mul(4)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            validate_storage_buffer_size(
                descriptor_bytes,
                limits,
                "streamed sparse descriptor index table",
            )?;
        }
    }
    Ok(())
}

pub(super) fn validate_fixed_storage_bindings(
    base: &ShStreamBaseMetadata,
    sources: &ShStreamSourceMetadata,
    limits: &wgpu::Limits,
) -> Result<(), ShResidencyDrainError> {
    let indirection_bytes = u64::try_from(base.probes.len())
        .map_err(|_| ShResidencyDrainError::SlotOverflow)?
        .checked_mul(4)
        .ok_or(ShResidencyDrainError::SlotOverflow)?
        .max(4);
    validate_storage_buffer_size(indirection_bytes, limits, "streamed SH compose indirection")?;
    validate_storage_buffer_size(
        indirection_bytes,
        limits,
        "streamed SH sampled indirection mirror",
    )?;

    if sources.indirect_delta.is_none() {
        validate_absent_sparse_carrier(base, limits, true)?;
    }
    if sources.animated_direct_delta.is_some() && sources.direct_delta.is_none() {
        validate_absent_sparse_carrier(base, limits, false)?;
    }
    Ok(())
}

fn validate_storage_buffer_size(
    bytes: u64,
    limits: &wgpu::Limits,
    resource: &'static str,
) -> Result<(), ShResidencyDrainError> {
    if bytes > limits.max_buffer_size || bytes > limits.max_storage_buffer_binding_size {
        return Err(ShResidencyDrainError::GpuCapacity { reason: resource });
    }
    Ok(())
}

fn validate_absent_sparse_carrier(
    base: &ShStreamBaseMetadata,
    limits: &wgpu::Limits,
    has_descriptor_indices: bool,
) -> Result<(), ShResidencyDrainError> {
    let cells = checked_cell_count(base.grid_dimensions.map(|axis| axis.div_ceil(4)))?;
    validate_storage_buffer_size(4, limits, "streamed sparse absent entry pool")?;
    validate_storage_buffer_size(4, limits, "streamed sparse absent delta tile pool")?;
    validate_storage_buffer_size(
        u64::from(cells.max(1))
            .checked_mul(8)
            .ok_or(ShResidencyDrainError::SlotOverflow)?,
        limits,
        "streamed sparse absent CSR row-pair table",
    )?;
    validate_storage_buffer_size(
        u64::from(cells)
            .checked_mul(3)
            .and_then(|words| words.checked_add(1))
            .and_then(|words| words.checked_mul(4))
            .ok_or(ShResidencyDrainError::SlotOverflow)?,
        limits,
        "streamed sparse absent compaction table",
    )?;
    if has_descriptor_indices {
        validate_storage_buffer_size(4, limits, "streamed sparse absent descriptor index table")?;
    }
    Ok(())
}
