//! Pure initial-capacity planning for streamed SH pools.
//! See: context/lib/rendering_pipeline.md §4.

use std::collections::BTreeMap;

use postretro_level_format::delta_sh_volumes::delta_probe_f16_stride;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F};
use postretro_level_format::sh_reconstruct::{Level, stored_delta_tiles};
use postretro_level_loader::{
    ShStreamBaseMetadata, ShStreamSourceMetadata, ShStreamSparseMetadata,
};

use super::ShResidencyDrainError;

mod validation;

use validation::{validate_fixed_storage_bindings, validate_sparse_caps};

/// Native-desktop requested GPU floor for fixed streaming metadata, retained
/// billboard scatter, and the initial active pool generation.
pub(crate) const DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES: u64 = 256 * 1024 * 1024;

const PHYSICAL_TILE_DIMENSION: u32 = 8;
const DENSE_COMPOSED_CELL_BYTES: u64 = 8 * 8 * 8;
const SPARSE_ENTRY_BYTES: u64 = 8;
const SPARSE_F16_WORD_BYTES: u64 = 4;

/// Finalized initial capacities before the GPU owner chooses atlas geometry.
///
/// `dense_slots` is the requested shared id-34/id-35 slot count. The GPU
/// owner applies `AtlasShape::for_slots` once, which may add final-layer
/// padding; this planner already charges that physical padding in
/// `effective_floor_bytes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InitialPoolFloor {
    pub(crate) dense_slots: u32,
    pub(crate) sparse_capacities: BTreeMap<u32, (u32, u32)>,
    /// Exact physical backing needed for the largest canonical dense writer,
    /// including atlas-layer padding across the coupled id-34/id-35 group.
    pub(crate) dense_group_minimum_bytes: u64,
    /// Exact physical backing needed for each present sparse writer family.
    /// Fixed CSR metadata remains a separate renderer charge.
    pub(crate) sparse_group_minimum_bytes: BTreeMap<u32, u64>,
    pub(crate) effective_floor_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
struct SparsePlan {
    minimum: (u32, u32),
    capacity: (u32, u32),
    maximum: (u32, u32),
    cell_count: u32,
    has_descriptor_indices: bool,
    descriptor_index_count: usize,
    entry_weight_bytes: u64,
    tile_weight_bytes: u64,
}

impl SparsePlan {
    fn weight_bytes(self) -> Result<u64, ShResidencyDrainError> {
        self.entry_weight_bytes
            .checked_add(self.tile_weight_bytes)
            .ok_or(ShResidencyDrainError::SlotOverflow)
    }

    fn capacity_bytes(self) -> Result<u64, ShResidencyDrainError> {
        sparse_capacity_bytes(self.capacity)
    }
}

/// Plans the exact initial streamed-pool floor without creating GPU objects.
///
/// The dense minimum is the largest canonical writer closure needed to admit
/// one cluster; its whole-level counterpart is the id-34 global stored-node
/// prefix total. Sparse minima are likewise largest owner-cluster capacities,
/// including their reserved missing-row entry/word. Each family receives its
/// proportional share of the post-fixed/scatter budget, then keeps the larger
/// of that share and its required minimum.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_initial_pool_floor(
    base: &ShStreamBaseMetadata,
    sources: &ShStreamSourceMetadata,
    minimum_dense_slots: u32,
    whole_dense_slots: u32,
    minimum_sparse: &BTreeMap<u32, (u32, u32)>,
    fixed_bytes: u64,
    scatter_bytes: u64,
    limits: &wgpu::Limits,
) -> Result<InitialPoolFloor, ShResidencyDrainError> {
    if (sources.direct_delta.is_some() || sources.animated_direct_delta.is_some())
        && sources.direct.is_none()
    {
        return Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed direct compose has no id-35 base atlas",
        });
    }

    validate_fixed_storage_bindings(base, sources, limits)?;
    let dense_slot_bytes = dense_slot_bytes(base, sources)?;
    let minimum_dense_slots = normalized_dense_minimum(minimum_dense_slots, whole_dense_slots)?;
    let mut sparse = sparse_plans(sources, minimum_sparse)?;
    validate_sparse_caps(&sparse, limits)?;
    let dense_group_minimum_bytes = u64::from(atlas_capacity_slots(minimum_dense_slots, limits)?)
        .checked_mul(dense_slot_bytes)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let sparse_group_minimum_bytes = sparse
        .iter()
        .map(|(&section_id, plan)| Ok((section_id, sparse_capacity_bytes(plan.minimum)?)))
        .collect::<Result<BTreeMap<_, _>, ShResidencyDrainError>>()?;

    let fixed_and_scatter_bytes = fixed_bytes
        .checked_add(scatter_bytes)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let pool_budget_bytes = if fixed_and_scatter_bytes >= DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES {
        0
    } else {
        DEFAULT_STREAMED_SH_POOL_FLOOR_BYTES
            .checked_sub(fixed_and_scatter_bytes)
            .ok_or(ShResidencyDrainError::SlotOverflow)?
    };

    let dense_weight_bytes = u64::from(whole_dense_slots)
        .checked_mul(dense_slot_bytes)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let sparse_weights = sparse
        .iter()
        .map(|(&section_id, plan)| Ok((section_id, plan.weight_bytes()?)))
        .collect::<Result<Vec<_>, ShResidencyDrainError>>()?;
    let mut family_limits = Vec::with_capacity(1 + sparse_weights.len());
    family_limits.push((dense_weight_bytes, dense_weight_bytes));
    family_limits.extend(
        sparse_weights
            .iter()
            .map(|&(_, weight_bytes)| (weight_bytes, weight_bytes)),
    );
    let family_shares = capped_proportional_shares(pool_budget_bytes, &family_limits)?;
    let dense_slots = dense_slots_for_share(
        minimum_dense_slots,
        whole_dense_slots,
        dense_slot_bytes,
        family_shares[0],
    )?;
    for ((section_id, _), share) in sparse_weights.iter().zip(family_shares.into_iter().skip(1)) {
        sparse_capacity_for_share(
            sparse
                .get_mut(section_id)
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
            share,
        )?;
    }

    validate_sparse_caps(&sparse, limits)?;
    let dense_physical_slots = atlas_capacity_slots(dense_slots, limits)?;
    let active_capacity_bytes = active_capacity_bytes(
        dense_physical_slots,
        dense_slot_bytes,
        sparse.values().copied(),
    )?;
    let effective_floor_bytes = fixed_bytes
        .checked_add(scatter_bytes)
        .and_then(|bytes| bytes.checked_add(active_capacity_bytes))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;

    Ok(InitialPoolFloor {
        dense_slots,
        sparse_capacities: sparse
            .into_iter()
            .map(|(section_id, plan)| (section_id, plan.capacity))
            .collect(),
        dense_group_minimum_bytes,
        sparse_group_minimum_bytes,
        effective_floor_bytes,
    })
}

fn normalized_dense_minimum(
    minimum_dense_slots: u32,
    whole_dense_slots: u32,
) -> Result<u32, ShResidencyDrainError> {
    if whole_dense_slots == 0 {
        // A valid all-invalid map still needs a bound texture for the existing
        // sampler contract, but has no resident atlas payload to weight.
        if minimum_dense_slots > 1 {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed dense minimum exceeds an empty stored-slot source",
            });
        }
        return Ok(1);
    }
    let minimum_dense_slots = minimum_dense_slots.max(1);
    if minimum_dense_slots > whole_dense_slots {
        return Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed dense minimum exceeds its whole stored-slot count",
        });
    }
    Ok(minimum_dense_slots)
}

fn dense_slot_bytes(
    base: &ShStreamBaseMetadata,
    sources: &ShStreamSourceMetadata,
) -> Result<u64, ShResidencyDrainError> {
    let mut bytes = atlas_cell_bytes(base.irradiance_format)?
        .checked_add(DENSE_COMPOSED_CELL_BYTES)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    if let Some(direct) = &sources.direct {
        bytes = bytes
            .checked_add(atlas_cell_bytes(direct.irradiance_format)?)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
    }
    if sources.direct_delta.is_some() || sources.animated_direct_delta.is_some() {
        bytes = bytes
            .checked_add(DENSE_COMPOSED_CELL_BYTES)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
    }
    if sources.animated_direct_delta.is_some() {
        bytes = bytes
            .checked_add(DENSE_COMPOSED_CELL_BYTES)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
    }
    Ok(bytes)
}

fn atlas_cell_bytes(format: u32) -> Result<u64, ShResidencyDrainError> {
    match format {
        IRRADIANCE_FORMAT_BC6H => Ok(64),
        IRRADIANCE_FORMAT_RGBA16F => Ok(DENSE_COMPOSED_CELL_BYTES),
        _ => Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed SH metadata has an unsupported atlas format",
        }),
    }
}

fn sparse_plans(
    sources: &ShStreamSourceMetadata,
    minimum_sparse: &BTreeMap<u32, (u32, u32)>,
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
        let cell_count = checked_cell_count(source.affinity_dims)?;
        let (whole_entries, whole_tiles) = whole_sparse_payload(source)?;
        let maximum = whole_sparse_capacity(whole_entries, whole_tiles)?;
        let requested = minimum_sparse.get(&section_id).copied().unwrap_or((1, 2));
        let capacity = normalized_sparse_capacity(requested)?;
        if capacity.0 > maximum.0 || capacity.1 > maximum.1 {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed sparse minimum exceeds its whole source capacity",
            });
        }
        let prior = plans.insert(
            section_id,
            SparsePlan {
                minimum: capacity,
                capacity,
                maximum,
                cell_count,
                has_descriptor_indices,
                descriptor_index_count: source.animation_descriptor_indices.len(),
                entry_weight_bytes: u64::from(whole_entries)
                    .checked_mul(SPARSE_ENTRY_BYTES)
                    .ok_or(ShResidencyDrainError::SlotOverflow)?,
                tile_weight_bytes: u64::from(whole_tiles)
                    .checked_mul(2)
                    .ok_or(ShResidencyDrainError::SlotOverflow)?,
            },
        );
        if prior.is_some() {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed sparse metadata duplicates a section id",
            });
        }
    }
    if minimum_sparse
        .keys()
        .any(|section_id| !plans.contains_key(section_id))
    {
        return Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed sparse minimum names an absent source",
        });
    }
    Ok(plans)
}

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
        tiles = tiles
            .checked_add(
                entries_in_row
                    .checked_mul(per_entry)
                    .ok_or(ShResidencyDrainError::SlotOverflow)?,
            )
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
    }
    Ok((entries, tiles))
}

fn whole_sparse_capacity(
    whole_entries: u32,
    whole_tiles: u32,
) -> Result<(u32, u32), ShResidencyDrainError> {
    let entries = whole_entries
        .checked_add(1)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let tiles = whole_tiles
        .checked_add(whole_tiles & 1)
        .and_then(|tiles| tiles.checked_add(2))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    Ok((entries, tiles))
}

fn normalized_sparse_capacity(
    (entries, tiles): (u32, u32),
) -> Result<(u32, u32), ShResidencyDrainError> {
    let entries = entries.max(1);
    let tiles = tiles.max(2);
    let tiles = tiles
        .checked_add(tiles & 1)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    Ok((entries, tiles))
}

fn dense_slots_for_share(
    minimum_slots: u32,
    whole_slots: u32,
    slot_bytes: u64,
    share_bytes: u64,
) -> Result<u32, ShResidencyDrainError> {
    if whole_slots == 0 {
        return Ok(minimum_slots);
    }
    let proportional_slots = ceil_div_u64(share_bytes, slot_bytes)?;
    let proportional_slots =
        u32::try_from(proportional_slots).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
    Ok(minimum_slots.max(proportional_slots.min(whole_slots)))
}

fn sparse_capacity_for_share(
    plan: &mut SparsePlan,
    family_share_bytes: u64,
) -> Result<(), ShResidencyDrainError> {
    let weight_bytes = plan.weight_bytes()?;
    let entry_share =
        proportional_share_ceil(family_share_bytes, plan.entry_weight_bytes, weight_bytes)?;
    let tile_share =
        proportional_share_ceil(family_share_bytes, plan.tile_weight_bytes, weight_bytes)?;
    let requested_entries = ceil_div_u64(entry_share, SPARSE_ENTRY_BYTES)?
        .checked_add(1)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let requested_tiles = ceil_div_u64(tile_share, SPARSE_F16_WORD_BYTES)?
        .checked_mul(2)
        .and_then(|tiles| tiles.checked_add(2))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let requested_entries =
        u32::try_from(requested_entries).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
    let requested_tiles =
        u32::try_from(requested_tiles).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
    plan.capacity.0 = plan.minimum.0.max(requested_entries.min(plan.maximum.0));
    plan.capacity.1 = plan.minimum.1.max(requested_tiles.min(plan.maximum.1));
    Ok(())
}

fn active_capacity_bytes(
    dense_physical_slots: u32,
    dense_slot_bytes: u64,
    mut sparse: impl Iterator<Item = SparsePlan>,
) -> Result<u64, ShResidencyDrainError> {
    let dense = u64::from(dense_physical_slots)
        .checked_mul(dense_slot_bytes)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    sparse.try_fold(dense, |total, plan| {
        total
            .checked_add(plan.capacity_bytes()?)
            .ok_or(ShResidencyDrainError::SlotOverflow)
    })
}

/// Mirrors `AtlasShape::for_slots` without exposing renderer GPU shape state
/// to this pure planner. It returns final-layer-padded physical slots.
fn atlas_capacity_slots(slots: u32, limits: &wgpu::Limits) -> Result<u32, ShResidencyDrainError> {
    let maximum = limits.max_texture_dimension_2d / PHYSICAL_TILE_DIMENSION;
    if maximum == 0 || slots == 0 {
        return Err(ShResidencyDrainError::GpuCapacity {
            reason: "adapter cannot allocate an 8x8 streamed SH cell",
        });
    }
    let tiles_per_row = ceil_sqrt(slots).min(maximum).max(1);
    let rows = slots.div_ceil(tiles_per_row).min(maximum).max(1);
    let tiles_per_layer = tiles_per_row
        .checked_mul(rows)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let layers = slots.div_ceil(tiles_per_layer);
    if layers == 0 || layers > limits.max_texture_array_layers {
        return Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed SH pool needs unsupported array layers",
        });
    }
    tiles_per_layer
        .checked_mul(layers)
        .ok_or(ShResidencyDrainError::SlotOverflow)
}

fn ceil_sqrt(value: u32) -> u32 {
    let mut low = 1_u32;
    let mut high = 65_536_u32;
    while low < high {
        let middle = low + (high - low) / 2;
        if u64::from(middle) * u64::from(middle) >= u64::from(value) {
            high = middle;
        } else {
            low = middle + 1;
        }
    }
    low
}

fn checked_cell_count(dimensions: [u32; 3]) -> Result<u32, ShResidencyDrainError> {
    dimensions.into_iter().try_fold(1_u32, |count, dimension| {
        count
            .checked_mul(dimension)
            .ok_or(ShResidencyDrainError::SlotOverflow)
    })
}

fn proportional_share_ceil(
    bytes: u64,
    weight: u64,
    total_weight: u64,
) -> Result<u64, ShResidencyDrainError> {
    if weight == 0 || bytes == 0 {
        return Ok(0);
    }
    if total_weight == 0 {
        return Err(ShResidencyDrainError::SlotOverflow);
    }
    let numerator = u128::from(bytes)
        .checked_mul(u128::from(weight))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let denominator = u128::from(total_weight);
    let quotient = numerator / denominator;
    let rounded = quotient
        .checked_add(u128::from(numerator % denominator != 0))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    u64::try_from(rounded).map_err(|_| ShResidencyDrainError::SlotOverflow)
}

/// Allocate a post-fixed/scatter family budget proportionally, never assigning
/// more than a family's whole-level source can hold. If a family reaches its
/// cap before the target is exhausted, it leaves the active set and its
/// remaining share is recomputed across the families that still have room.
fn capped_proportional_shares(
    target_bytes: u64,
    families: &[(u64, u64)],
) -> Result<Vec<u64>, ShResidencyDrainError> {
    let mut shares = vec![0; families.len()];
    let mut active = families
        .iter()
        .enumerate()
        .filter_map(|(index, &(weight, capacity))| {
            (weight != 0).then_some((index, weight, capacity))
        })
        .collect::<Vec<_>>();
    let mut remaining_target = target_bytes;
    let mut remaining_weight = active.iter().try_fold(0_u64, |total, &(_, weight, _)| {
        total
            .checked_add(weight)
            .ok_or(ShResidencyDrainError::SlotOverflow)
    })?;

    while remaining_target != 0 && !active.is_empty() && remaining_weight != 0 {
        let capped = active
            .iter()
            .filter_map(|&(index, weight, capacity)| {
                let desired = u128::from(remaining_target) * u128::from(weight);
                let cap = u128::from(capacity) * u128::from(remaining_weight);
                (desired >= cap).then_some((index, weight, capacity))
            })
            .collect::<Vec<_>>();
        if capped.is_empty() {
            for &(index, weight, capacity) in &active {
                shares[index] =
                    proportional_share_ceil(remaining_target, weight, remaining_weight)?
                        .min(capacity);
            }
            return Ok(shares);
        }

        for &(index, weight, capacity) in &capped {
            shares[index] = capacity;
            remaining_target = remaining_target
                .checked_sub(capacity)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            remaining_weight = remaining_weight
                .checked_sub(weight)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
        }
        active.retain(|(index, _, _)| {
            !capped
                .iter()
                .any(|(capped_index, _, _)| capped_index == index)
        });
    }
    Ok(shares)
}

fn ceil_div_u64(value: u64, divisor: u64) -> Result<u64, ShResidencyDrainError> {
    if divisor == 0 {
        return Err(ShResidencyDrainError::SlotOverflow);
    }
    value
        .checked_div(divisor)
        .and_then(|quotient| quotient.checked_add(u64::from(value % divisor != 0)))
        .ok_or(ShResidencyDrainError::SlotOverflow)
}

fn sparse_capacity_bytes((entries, tiles): (u32, u32)) -> Result<u64, ShResidencyDrainError> {
    u64::from(entries)
        .checked_mul(SPARSE_ENTRY_BYTES)
        .and_then(|bytes| {
            u64::from(tiles.div_ceil(2))
                .checked_mul(SPARSE_F16_WORD_BYTES)
                .and_then(|tile_bytes| bytes.checked_add(tile_bytes))
        })
        .ok_or(ShResidencyDrainError::SlotOverflow)
}

#[cfg(test)]
#[path = "floor/tests.rs"]
mod tests;
