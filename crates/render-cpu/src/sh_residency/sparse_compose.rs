// Sparse residency capacities derived from the stream manifest alone: the
// per-owner backing floors, the compose-planner row space, and each sparse
// compose family's row tables and minimum backing.
// See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

use std::collections::BTreeMap;

use postretro_level_format::delta_sh_volumes::delta_probe_f16_stride;
use postretro_level_format::sh_reconstruct::{Level, stored_delta_tiles};
use postretro_level_loader::{
    ShStreamBaseMetadata, ShStreamSourceMetadata, ShStreamSparseMetadata,
};

use super::{ANIMATED_DIRECT_DELTA_ID, DIRECT_DELTA_ID, INDIRECT_DELTA_ID, ShResidencyDrainError};

/// Physical entry/f16 backing needed to install every owned row in one
/// canonical cluster. Values include the allocator's missing-row sentinel
/// and f16 word alignment, but never reserve all legacy CSR payloads.
pub type SparseCapacityFloors = BTreeMap<u32, (u32, u32)>;

/// One sparse compose family's row tables and minimum backing, independent
/// of the pool floor a generation applies: [`Self::entry_capacity`] and
/// [`Self::tile_f16_capacity`] fold that floor in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SparseComposeLayout {
    /// The family's sparse source streams; `false` is the base-only layout
    /// derived from the id-34 projection.
    pub source_present: bool,
    pub affinity_dims: [u32; 3],
    pub valid_probe_masks: Vec<u64>,
    pub cell_levels: Vec<u8>,
    pub animation_descriptor_indices: Vec<u32>,
    entry_minimum: u32,
    tile_f16_minimum: u32,
}

impl SparseComposeLayout {
    /// Entry backing: the largest canonical row (plus the missing-row
    /// sentinel), raised to `floor`'s entries.
    pub fn entry_capacity(&self, floor: Option<(u32, u32)>) -> u32 {
        self.entry_minimum.max(floor.map_or(1, |floor| floor.0))
    }

    /// Packed f16 tile backing: the largest canonical row, word aligned with
    /// the sentinel word, raised to `floor`'s tiles.
    pub fn tile_f16_capacity(&self, floor: Option<(u32, u32)>) -> u32 {
        self.tile_f16_minimum.max(floor.map_or(2, |floor| floor.1))
    }
}

/// The sparse compose layouts a streamed level's passes build from: the
/// indirect family always (base-only maps still compose id 34 → total), the
/// direct promotion family when any direct compose runs, and the animated
/// direct family when id 45 streams.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SparseComposeLayouts {
    pub indirect: SparseComposeLayout,
    pub direct: Option<SparseComposeLayout>,
    pub animated_direct: Option<SparseComposeLayout>,
}

impl SparseComposeLayouts {
    pub fn derive(
        base: &ShStreamBaseMetadata,
        sources: &ShStreamSourceMetadata,
    ) -> Result<Self, ShResidencyDrainError> {
        let indirect = sparse_compose_capacity(base, sources.indirect_delta.as_ref())?;
        let direct = (sources.direct_delta.is_some() || sources.animated_direct_delta.is_some())
            .then(|| sparse_compose_capacity(base, sources.direct_delta.as_ref()))
            .transpose()?;
        let animated_direct = sources
            .animated_direct_delta
            .as_ref()
            .map(|source| sparse_compose_capacity(base, Some(source)))
            .transpose()?;
        Ok(Self {
            indirect,
            direct,
            animated_direct,
        })
    }
}

fn checked_cell_count(dimensions: [u32; 3]) -> Result<u32, ShResidencyDrainError> {
    dimensions.into_iter().try_fold(1u32, |count, dimension| {
        count
            .checked_mul(dimension)
            .ok_or(ShResidencyDrainError::SlotOverflow)
    })
}

/// Derive an initial sparse backing floor from id-49 ownership, not from the
/// complete legacy CSR body.  Each owner total represents the exact worst
/// case for one ready cluster (multiple rows included), so a normal cluster
/// cannot stall after its first row merely because its second row needs a
/// separate allocation.  Later residency pressure grows the pool
/// append-preservingly; this floor is deliberately bounded and never turns
/// the streamed path into a whole-resident copy.
pub fn sparse_capacity_floors(
    sources: &ShStreamSourceMetadata,
    sparse_row_owner: &BTreeMap<(u32, u32), u32>,
) -> Result<SparseCapacityFloors, ShResidencyDrainError> {
    let mut by_owner = BTreeMap::<(u32, u32), (u32, u32)>::new();
    for (&(section_id, row), &owner) in sparse_row_owner {
        let source = match section_id {
            INDIRECT_DELTA_ID => sources.indirect_delta.as_ref(),
            DIRECT_DELTA_ID => sources.direct_delta.as_ref(),
            ANIMATED_DIRECT_DELTA_ID => sources.animated_direct_delta.as_ref(),
            _ => None,
        }
        .ok_or(ShResidencyDrainError::MalformedChunk {
            cluster_id: owner,
            reason: "id-49 names a sparse source omitted by stream metadata",
        })?;
        let index = usize::try_from(row).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        let start =
            *source
                .affinity_offsets
                .get(index)
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id: owner,
                    reason: "id-49 sparse row lacks a CSR start",
                })?;
        let end = *source.affinity_offsets.get(index + 1).ok_or(
            ShResidencyDrainError::MalformedChunk {
                cluster_id: owner,
                reason: "id-49 sparse row lacks a CSR end",
            },
        )?;
        let level = Level::from_u8(*source.cell_levels.get(index).ok_or(
            ShResidencyDrainError::MalformedChunk {
                cluster_id: owner,
                reason: "id-49 sparse row lacks its reconstruction level",
            },
        )?)
        .ok_or(ShResidencyDrainError::MalformedChunk {
            cluster_id: owner,
            reason: "id-49 sparse row has an invalid reconstruction level",
        })?;
        let mask =
            *source
                .valid_probe_masks
                .get(index)
                .ok_or(ShResidencyDrainError::MalformedChunk {
                    cluster_id: owner,
                    reason: "id-49 sparse row lacks its valid-probe mask",
                })?;
        let entries = end
            .checked_sub(start)
            .ok_or(ShResidencyDrainError::MalformedChunk {
                cluster_id: owner,
                reason: "streamed sparse CSR offsets are not monotonic",
            })?;
        let per_entry = u32::try_from(
            stored_delta_tiles(level, mask)
                .checked_mul(delta_probe_f16_stride(source.tile_dimension))
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
        )
        .map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        let tiles = entries
            .checked_mul(per_entry)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let total = by_owner.entry((section_id, owner)).or_insert((0, 0));
        total.0 = total
            .0
            .checked_add(entries)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        total.1 = total
            .1
            .checked_add(tiles)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
    }

    let mut floors = SparseCapacityFloors::new();
    for ((section_id, _), (entries, tiles)) in by_owner {
        let floor = floors.entry(section_id).or_insert((1, 2));
        floor.0 = floor.0.max(
            entries
                .checked_add(1)
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
        );
        let aligned_tiles = tiles
            .checked_add(tiles & 1)
            .and_then(|count| count.checked_add(2))
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        floor.1 = floor.1.max(aligned_tiles);
    }

    Ok(floors)
}

/// Affinity-row id space for dense compose-planner state: the 4×4×4 brick
/// grid over the base probes, widened to any sparse CSR row count (the loader
/// already requires those to match the brick grid).
pub fn compose_row_capacity(
    grid_dimensions: [u32; 3],
    sources: &ShStreamSourceMetadata,
) -> Result<usize, ShResidencyDrainError> {
    let brick_rows = grid_dimensions
        .iter()
        .try_fold(1usize, |rows, &dimension| {
            rows.checked_mul(usize::try_from(dimension.div_ceil(4)).ok()?)
        })
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    Ok([
        sources.indirect_delta.as_ref(),
        sources.direct_delta.as_ref(),
        sources.animated_direct_delta.as_ref(),
    ]
    .into_iter()
    .flatten()
    .map(|source| source.affinity_offsets.len().saturating_sub(1))
    .fold(brick_rows, usize::max))
}

/// One sparse compose family's layout: its source's row tables (validated
/// for consistency) and largest canonical row, or with no source the
/// base-only tables derived from the id-34 projection, which the indirect
/// compose still runs for the id-34 → total copy.
pub fn sparse_compose_capacity(
    base: &ShStreamBaseMetadata,
    source: Option<&ShStreamSparseMetadata>,
) -> Result<SparseComposeLayout, ShResidencyDrainError> {
    if let Some(source) = source {
        let cells = checked_cell_count(source.affinity_dims)?;
        let cells_usize =
            usize::try_from(cells).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        if source.valid_probe_masks.len() != cells_usize
            || source.cell_levels.len() != cells_usize
            || source.affinity_offsets.len() != cells_usize.saturating_add(1)
        {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed indirect sparse metadata has inconsistent row tables",
            });
        }
        let entries = *source.affinity_offsets.last().unwrap_or(&0);
        if usize::try_from(entries).ok() != Some(source.affinity_lights.len()) {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed indirect sparse metadata has inconsistent entry table",
            });
        }
        if source
            .affinity_offsets
            .windows(2)
            .any(|pair| pair[0] > pair[1])
        {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed indirect sparse metadata offsets are not monotonic",
            });
        }
        let stride = u32::try_from(delta_probe_f16_stride(source.tile_dimension))
            .map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        let mut max_row_entries = 0u32;
        let mut max_row_tile_f16 = 0u32;
        for row in 0..cells_usize {
            let level = Level::from_u8(source.cell_levels[row]).ok_or(
                ShResidencyDrainError::MalformedChunk {
                    cluster_id: 0,
                    reason: "streamed indirect sparse metadata has invalid cell level",
                },
            )?;
            let tiles = u32::try_from(stored_delta_tiles(level, source.valid_probe_masks[row]))
                .map_err(|_| ShResidencyDrainError::SlotOverflow)?;
            let row_entries = source.affinity_offsets[row + 1] - source.affinity_offsets[row];
            let row_tiles = tiles
                .checked_mul(stride)
                .and_then(|count| count.checked_mul(row_entries))
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            max_row_entries = max_row_entries.max(row_entries);
            max_row_tile_f16 = max_row_tile_f16.max(row_tiles);
        }
        return Ok(SparseComposeLayout {
            source_present: true,
            affinity_dims: source.affinity_dims,
            valid_probe_masks: source.valid_probe_masks.clone(),
            cell_levels: source.cell_levels.clone(),
            animation_descriptor_indices: source.animation_descriptor_indices.clone(),
            // One largest canonical row is sufficient for the initial sparse
            // floor. Pool growth adds backing rather than retaining every
            // legacy CSR payload at level installation.
            entry_minimum: max_row_entries
                .checked_add(1)
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
            tile_f16_minimum: max_row_tile_f16
                .checked_add(max_row_tile_f16 & 1)
                .and_then(|count| count.checked_add(2))
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
        });
    }

    // Base-only streamed maps still use the indirect compose shader for the
    // id-34 → total copy. Derive its cell metadata from the retained id-34
    // projection instead of requiring a dummy id-27 body.
    let dims = base.grid_dimensions.map(|axis| axis.div_ceil(4));
    let cells = checked_cell_count(dims)?;
    let cells_usize = usize::try_from(cells).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
    let mut masks = vec![0u64; cells_usize];
    let mut levels = vec![0u8; cells_usize];
    let [width, height, _] = base.grid_dimensions;
    let xy = width
        .checked_mul(height)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    for (dense, probe) in base.probes.iter().enumerate() {
        let dense = u32::try_from(dense).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        let xyz = [dense % width, (dense / width) % height, dense / xy];
        let cell_xyz = xyz.map(|axis| axis / 4);
        let cell = cell_xyz[0]
            .checked_add(
                cell_xyz[1]
                    .checked_mul(dims[0])
                    .ok_or(ShResidencyDrainError::SlotOverflow)?,
            )
            .and_then(|index| {
                index.checked_add(cell_xyz[2].checked_mul(dims[0].checked_mul(dims[1])?)?)
            })
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let local = (xyz[0] % 4) + (xyz[1] % 4) * 4 + (xyz[2] % 4) * 16;
        let cell_index = usize::try_from(cell).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        if probe.validity != 0 {
            masks[cell_index] |= 1u64 << local;
        }
        levels[cell_index] = probe.density_level;
    }
    // Preserve the packed-u32 sentinel even when this base-only path never
    // allocates delta row payloads. It keeps the CPU and GPU sparse pools on
    // the same word-aligned address contract.
    Ok(SparseComposeLayout {
        source_present: false,
        affinity_dims: dims,
        valid_probe_masks: masks,
        cell_levels: levels,
        animation_descriptor_indices: vec![u32::MAX],
        entry_minimum: 1,
        tile_f16_minimum: 2,
    })
}
