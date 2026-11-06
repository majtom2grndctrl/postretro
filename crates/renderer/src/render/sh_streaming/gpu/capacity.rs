//! Exact streamed-pool sizing and adapter-limit preflight.
//!
//! These pure helpers reject unsupported backing before renderer-owned wgpu
//! allocation begins, and clamp growth to the device and the whole map.

use super::*;

/// One sparse family's growth: the `(entries, tiles)` an install cannot go
/// without, and what the mandatory tier would fill.
#[derive(Debug, Clone, Copy)]
pub(in crate::render::sh_streaming) struct SparseGrowth {
    pub(in crate::render::sh_streaming) needed: (u32, u32),
    pub(in crate::render::sh_streaming) wanted: (u32, u32),
}

impl StreamingGpuPools {
    /// The dense shape after a growth, or the refusal of the device or the
    /// whole-map cap.
    pub(super) fn grown_shape(
        &self,
        needed: u32,
        wanted: u32,
    ) -> Result<AtlasShape, ShResidencyDrainError> {
        self.shape
            .grown_for_slots(needed, wanted, self.whole_map.dense_slots, &self.limits)
    }

    pub(super) fn sparse_is_short(&self, section_id: u32, (entries, tiles): (u32, u32)) -> bool {
        entries > self.sparse_entry_capacity(section_id)
            || tiles > self.sparse_tile_f16_capacity(section_id)
    }

    /// `section_id`'s capacity after `growth`, or the refusal of the device or
    /// the whole-map cap.
    pub(super) fn sparse_growth_capacity(
        &self,
        section_id: u32,
        growth: SparseGrowth,
        sources: &ShStreamSourceMetadata,
    ) -> Result<(u32, u32), ShResidencyDrainError> {
        let source = match section_id {
            27 => sources.indirect_delta.as_ref(),
            DIRECT_DELTA_SECTION => sources.direct_delta.as_ref(),
            ANIMATED_DIRECT_DELTA_SECTION => sources.animated_direct_delta.as_ref(),
            _ => None,
        }
        .ok_or(ShResidencyDrainError::GpuCapacity {
            reason: "streamed sparse growth names an absent family",
        })?;
        let whole =
            *self
                .whole_map
                .sparse
                .get(&section_id)
                .ok_or(ShResidencyDrainError::GpuCapacity {
                    reason: "streamed sparse growth names a family with no whole-map cap",
                })?;
        clamped_sparse_capacity(
            (
                self.sparse_entry_capacity(section_id),
                self.sparse_tile_f16_capacity(section_id),
            ),
            growth,
            checked_cell_count(source.affinity_dims)?,
            whole,
            &self.limits,
        )
    }

    /// Which family's retiring generation holds back a need of `dense` slots
    /// and `sparse` backing. Limits read first, so a need no wait could serve
    /// is never parked behind one, and `None` also covers a need that fits.
    pub(in crate::render::sh_streaming) fn retirement_wait(
        &self,
        dense: u32,
        sparse: &SparseCapacityFloors,
        sources: &ShStreamSourceMetadata,
    ) -> Result<Option<&'static str>, ShResidencyDrainError> {
        let mut waits_on = None;
        if dense > self.shape.slots {
            match self.grown_shape(dense, dense) {
                Err(error) if error.is_growth_refusal() => return Ok(None),
                grown => {
                    grown?;
                }
            }
            if self.retiring_dense.is_some() {
                waits_on = Some("dense atlas");
            }
        }
        for (&section_id, &needed) in sparse {
            if !self.sparse_is_short(section_id, needed) {
                continue;
            }
            let growth = SparseGrowth {
                needed,
                wanted: needed,
            };
            match self.sparse_growth_capacity(section_id, growth, sources) {
                Err(error) if error.is_growth_refusal() => return Ok(None),
                grown => {
                    grown?;
                }
            }
            if sparse_family_has_retiring_generation(
                section_id,
                self.retiring_indirect.is_some(),
                self.retiring_direct_promotion.is_some(),
                self.retiring_direct_animated.is_some(),
            ) {
                waits_on = waits_on.or(Some("sparse compose"));
            }
        }
        Ok(waits_on)
    }
}

/// The compaction table includes three immutable words per affinity row plus
/// one word per entry. It therefore bounds entries before the entry buffer;
/// tile backing is packed into whole u32 words. Both obey buffer and binding
/// limits, including a lowered test grant. `whole` caps it as the dense layers
/// are capped.
fn clamped_sparse_capacity(
    current: (u32, u32),
    growth: SparseGrowth,
    rows: u32,
    whole: (u32, u32),
    limits: &wgpu::Limits,
) -> Result<(u32, u32), ShResidencyDrainError> {
    let words = limits
        .max_buffer_size
        .min(limits.max_storage_buffer_binding_size)
        / 4;
    let prefix = u64::from(rows) * 3;
    let entry_limit = words.saturating_sub(prefix).min(u64::from(u32::MAX)) as u32;
    let tile_limit = words.saturating_mul(2).min(u64::from(u32::MAX - 1)) as u32;
    let SparseGrowth { needed, wanted } = growth;
    if needed.0 > entry_limit || needed.1 > tile_limit {
        return Err(ShResidencyDrainError::GpuDeviceLimit {
            reason: "streamed sparse pool needs backing beyond its tightest buffer limit",
        });
    }
    if needed.0 > whole.0 || needed.1 > whole.1 {
        return Err(ShResidencyDrainError::PoolAtWholeMap {
            family: "sparse compose",
        });
    }
    Ok((
        current
            .0
            .max(needed.0)
            .max(wanted.0.min(entry_limit).min(whole.0)),
        current
            .1
            .max(needed.1)
            .max(wanted.1.min(tile_limit).min(whole.1)),
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    // AC 30: sparse capacity is two-dimensional, so each figure stops at its
    // own whole-map total, and needing past either refuses.
    #[test]
    fn sparse_growth_stops_at_the_whole_map() {
        let limits = wgpu::Limits::default();
        let whole = (100, 200);
        let grow = |needed, wanted| {
            clamped_sparse_capacity((10, 20), SparseGrowth { needed, wanted }, 1, whole, &limits)
        };

        assert_eq!(grow((11, 21), (5_000, 150)), Ok((100, 150)));
        assert_eq!(grow((11, 21), (50, 5_000)), Ok((50, 200)));
        for needed in [(101, 21), (11, 201)] {
            assert_eq!(
                grow(needed, needed),
                Err(ShResidencyDrainError::PoolAtWholeMap {
                    family: "sparse compose",
                })
            );
        }
    }
}
