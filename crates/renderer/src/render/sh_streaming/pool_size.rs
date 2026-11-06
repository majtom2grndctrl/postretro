//! Sizes the streamed SH pools once at load from the stored mandatory bound.
//! See: context/lib/rendering_pipeline.md §4 (Bounded SH pool)

use std::collections::BTreeMap;

use postretro_level_format::delta_sh_volumes::delta_probe_f16_stride;
use postretro_level_format::sh_pool_cost::{
    SH_SPARSE_F16_WORD_BYTES, ShDenseSlotTextures, sh_dense_slot_bytes, sh_sparse_capacity_bytes,
};
use postretro_level_format::sh_reconstruct::{Level, stored_delta_tiles};
use postretro_level_loader::{
    ShStreamBaseMetadata, ShStreamSourceMetadata, ShStreamSparseMetadata,
};
use postretro_render_cpu::sh_residency::ShPoolBound;

use super::allocator::reserved_tile_f16;
use super::gpu::sparse_device_limits;
use super::{AtlasShape, ShResidencyDrainError, SparseCapacities};

mod validation;

use validation::{validate_fixed_storage_bindings, validate_sparse_caps, validate_sparse_plan};

/// Over the bound, dense and sparse alike: first-fit's address high-water runs
/// above what is live.
const POOL_HEADROOM_PERCENT: u64 = 25;

/// The most a pool may ever hold: the level's unique union. Dense reads it in
/// whole layers, as `AtlasShape` allocates them. Growth clamps to it as load
/// does, so the safety net cannot outgrow the map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WholeMapCap {
    pub(crate) dense_slots: u32,
    pub(crate) sparse: SparseCapacities,
}

#[cfg(test)]
impl WholeMapCap {
    /// For fixtures whose pools are not sized from their own map, so a proof
    /// about device limits never reads the cap instead.
    pub(crate) fn uncapped() -> Self {
        Self {
            dense_slots: u32::MAX,
            sparse: [27, 41, 45]
                .map(|section_id| (section_id, (u32::MAX, u32::MAX)))
                .into(),
        }
    }
}

/// Finalized initial capacities before the GPU owner chooses atlas geometry.
/// `AtlasShape::for_slots` may add final-layer padding to `dense_slots`;
/// `pool_bytes` already charges it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InitialPool {
    pub(crate) dense_slots: u32,
    pub(crate) sparse_capacities: SparseCapacities,
    pub(crate) whole_map: WholeMapCap,
    pub(crate) pool_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
struct SparsePlan {
    capacity: (u32, u32),
    whole: (u32, u32),
    cell_count: u32,
    has_descriptor_indices: bool,
    descriptor_index_count: usize,
}

/// Plans the first and only intended pool generation, without creating GPU
/// objects: `bound` plus headroom, capped at the whole map and clamped to the
/// device. Headroom never fails a load: only a bound itself past the device
/// does. Dense takes at least one slot, so an all-invalid map keeps the sampler
/// contract's bound texture.
///
/// A dense pool that holds the map starts every family whole, because a level
/// holding the map in every family stops evicting. A family past the device
/// stays at its clamped size instead. The level then keeps evicting, and growth
/// refuses by name if it must, rather than failing a load its bound would pass.
pub(crate) fn plan_initial_pool(
    base: &ShStreamBaseMetadata,
    sources: &ShStreamSourceMetadata,
    bound: &ShPoolBound,
    whole_dense_slots: u32,
    limits: &wgpu::Limits,
) -> Result<InitialPool, ShResidencyDrainError> {
    if (sources.direct_delta.is_some() || sources.animated_direct_delta.is_some())
        && sources.direct.is_none()
    {
        return Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed direct compose has no id-35 base atlas",
        });
    }

    validate_fixed_storage_bindings(base, sources, limits)?;
    let dense_slot_bytes = dense_slot_bytes(base, sources)?;
    let mut sparse = sparse_plans(sources, &bound.sparse, limits)?;

    let dense_slots = with_headroom_capped(bound.dense_slots, whole_dense_slots)
        .min(AtlasShape::device_slot_limit(limits))
        .max(bound.dense_slots.min(whole_dense_slots))
        .max(1);
    let shape = AtlasShape::for_slots(dense_slots, whole_dense_slots, limits)?;
    if shape.slots >= whole_dense_slots {
        for plan in sparse.values_mut() {
            let whole = SparsePlan {
                capacity: plan.whole,
                ..*plan
            };
            if validate_sparse_plan(&whole, limits).is_ok() {
                *plan = whole;
            }
        }
    }
    validate_sparse_caps(&sparse, limits)?;
    let pool_bytes = sparse.values().try_fold(
        u64::from(shape.slots)
            .checked_mul(dense_slot_bytes)
            .ok_or(ShResidencyDrainError::SlotOverflow)?,
        |total, plan| {
            let (entries, tiles) = plan.capacity;
            total
                .checked_add(sh_sparse_capacity_bytes(entries, tiles))
                .ok_or(ShResidencyDrainError::SlotOverflow)
        },
    )?;

    Ok(InitialPool {
        dense_slots,
        sparse_capacities: sparse
            .iter()
            .map(|(&section_id, plan)| (section_id, plan.capacity))
            .collect(),
        whole_map: WholeMapCap {
            dense_slots: whole_dense_slots,
            sparse: sparse
                .iter()
                .map(|(&section_id, plan)| (section_id, plan.whole))
                .collect(),
        },
        pool_bytes,
    })
}

fn with_headroom_capped(units: u32, whole: u32) -> u32 {
    let padded = (u64::from(units) * (100 + POOL_HEADROOM_PERCENT)).div_ceil(100);
    padded.min(u64::from(whole)) as u32
}

fn dense_slot_bytes(
    base: &ShStreamBaseMetadata,
    sources: &ShStreamSourceMetadata,
) -> Result<u64, ShResidencyDrainError> {
    sh_dense_slot_bytes(ShDenseSlotTextures {
        indirect_format: base.irradiance_format,
        direct_format: sources
            .direct
            .as_ref()
            .map(|direct| direct.irradiance_format),
        direct_compose: sources.direct_delta.is_some() || sources.animated_direct_delta.is_some(),
        animated_direct_compose: sources.animated_direct_delta.is_some(),
    })
    .ok_or(ShResidencyDrainError::GpuCapacity {
        reason: "streamed SH metadata has an unsupported atlas format",
    })
}

fn sparse_plans(
    sources: &ShStreamSourceMetadata,
    bound: &BTreeMap<u32, (u32, u32)>,
    limits: &wgpu::Limits,
) -> Result<BTreeMap<u32, SparsePlan>, ShResidencyDrainError> {
    let mut plans = BTreeMap::new();
    for (source, has_descriptor_indices) in [
        (sources.indirect_delta.as_ref(), true),
        (sources.direct_delta.as_ref(), false),
        (sources.animated_direct_delta.as_ref(), true),
    ] {
        let Some(source) = source else {
            continue;
        };
        let section_id = source.section_id;
        let (whole_entries, whole_tiles) = whole_sparse_payload(source)?;
        let &(worst_entries, worst_tiles) = bound
            .get(&section_id)
            .ok_or(ShResidencyDrainError::MandatoryBoundMissingFamily { section_id })?;
        let cell_count = checked_cell_count(source.affinity_dims)?;
        let padded = with_sentinel((
            with_headroom_capped(worst_entries, whole_entries),
            with_headroom_capped(worst_tiles, whole_tiles),
        ))?;
        let unpadded = with_sentinel((
            worst_entries.min(whole_entries),
            worst_tiles.min(whole_tiles),
        ))?;
        let (entry_limit, tile_limit) = sparse_device_limits(cell_count, limits);
        let prior = plans.insert(
            section_id,
            SparsePlan {
                capacity: (
                    padded.0.min(entry_limit).max(unpadded.0),
                    padded.1.min(tile_limit).max(unpadded.1),
                ),
                whole: with_sentinel((whole_entries, whole_tiles))?,
                cell_count,
                has_descriptor_indices,
                descriptor_index_count: source.animation_descriptor_indices.len(),
            },
        );
        if prior.is_some() {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed sparse metadata duplicates a section id",
            });
        }
    }
    if let Some(&section_id) = bound
        .keys()
        .find(|&section_id| !plans.contains_key(section_id))
    {
        return Err(ShResidencyDrainError::MandatoryBoundExtraFamily { section_id });
    }
    Ok(plans)
}

/// Every row's `(entries, tile_f16)` as the allocator places them, each row
/// once: the family's unique union.
fn whole_sparse_payload(
    source: &ShStreamSparseMetadata,
) -> Result<(u32, u32), ShResidencyDrainError> {
    let row_count = checked_cell_count(source.affinity_dims)?;
    let row_count = usize::try_from(row_count).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
    if source.valid_probe_masks.len() != row_count
        || source.cell_levels.len() != row_count
        || source.affinity_offsets.len()
            != row_count
                .checked_add(1)
                .ok_or(ShResidencyDrainError::SlotOverflow)?
    {
        return Err(ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "streamed sparse metadata has inconsistent row tables",
        });
    }
    let entries = *source
        .affinity_offsets
        .last()
        .ok_or(ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "streamed sparse metadata has no CSR offsets",
        })?;
    if usize::try_from(entries).ok() != Some(source.affinity_lights.len())
        || source
            .affinity_offsets
            .windows(2)
            .any(|pair| pair[0] > pair[1])
    {
        return Err(ShResidencyDrainError::MalformedChunk {
            cluster_id: 0,
            reason: "streamed sparse metadata has invalid CSR offsets",
        });
    }

    let stride = u32::try_from(delta_probe_f16_stride(source.tile_dimension))
        .map_err(|_| ShResidencyDrainError::SlotOverflow)?;
    let mut tiles = 0_u32;
    for row in 0..row_count {
        let level = Level::from_u8(source.cell_levels[row]).ok_or(
            ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed sparse metadata has an invalid reconstruction level",
            },
        )?;
        let per_entry = u32::try_from(stored_delta_tiles(level, source.valid_probe_masks[row]))
            .map_err(|_| ShResidencyDrainError::SlotOverflow)?
            .checked_mul(stride)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let entries_in_row = source.affinity_offsets[row + 1]
            .checked_sub(source.affinity_offsets[row])
            .ok_or(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed sparse metadata has non-monotonic CSR offsets",
            })?;
        let row_tiles = entries_in_row
            .checked_mul(per_entry)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        tiles = tiles
            .checked_add(reserved_tile_f16(row_tiles)?)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
    }
    Ok((entries, tiles))
}

/// Add the allocator's missing-row sentinel, one entry and one word, and round
/// tiles up to whole words.
fn with_sentinel((entries, tiles): (u32, u32)) -> Result<(u32, u32), ShResidencyDrainError> {
    let entries = entries
        .checked_add(1)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let tiles = reserved_tile_f16(tiles)?
        .checked_add(2)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    Ok((entries, tiles))
}

fn checked_cell_count(dimensions: [u32; 3]) -> Result<u32, ShResidencyDrainError> {
    dimensions.into_iter().try_fold(1_u32, |count, dimension| {
        count
            .checked_mul(dimension)
            .ok_or(ShResidencyDrainError::SlotOverflow)
    })
}

#[cfg(test)]
#[path = "pool_size/tests.rs"]
mod tests;
