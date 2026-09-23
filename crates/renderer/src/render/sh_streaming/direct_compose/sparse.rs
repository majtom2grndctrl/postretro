//! Existing-binding sparse pools for streamed direct composition.

use postretro_level_loader::ShStreamBaseMetadata;
use postretro_render_cpu::sh_compose::{ComposeGridParams, DYNAMIC_COMPOSE_GRID_DIMS_SIZE};
use wgpu::util::DeviceExt;

use super::super::{
    AtlasShape, ShResidencyDrainError, buffer_with_zeroes, checked_cell_count,
    sparse_compose_capacity, u32_bytes,
};

/// One row after the residency allocator has assigned its entry and f16 ranges.
/// `role == 2` is a halo and never becomes a reachable compose row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in super::super) struct DirectSparseRowUpload {
    pub(in super::super) row: u32,
    pub(in super::super) role: u32,
    pub(in super::super) entry_start: u32,
    pub(in super::super) entry_end: u32,
    /// Row starts are word-aligned; entries may still have odd f16 offsets.
    pub(in super::super) tile_f16_start: u32,
    pub(in super::super) lights: Vec<u32>,
    pub(in super::super) entry_tile_f16_offsets: Vec<u32>,
    pub(in super::super) tile_f16: Vec<u16>,
}

pub(super) struct StreamingSparseBuffers {
    row_pairs: wgpu::Buffer,
    lights: wgpu::Buffer,
    tile_words: wgpu::Buffer,
    compaction_metadata: wgpu::Buffer,
    entry_capacity: u32,
    tile_f16_capacity: u32,
    compaction_entry_offset_words: u32,
    row_count: u32,
}

impl StreamingSparseBuffers {
    pub(super) fn row_pairs(&self) -> &wgpu::Buffer {
        &self.row_pairs
    }

    pub(super) fn lights(&self) -> &wgpu::Buffer {
        &self.lights
    }

    pub(super) fn tile_words(&self) -> &wgpu::Buffer {
        &self.tile_words
    }

    pub(super) fn compaction_metadata(&self) -> &wgpu::Buffer {
        &self.compaction_metadata
    }

    /// Validate the complete batch before queuing any writes. A later invalid
    /// row must not leave an earlier CSR pair reachable.
    pub(super) fn upload_rows(
        &self,
        queue: &wgpu::Queue,
        rows: &[DirectSparseRowUpload],
    ) -> Result<(), ShResidencyDrainError> {
        self.validate_rows(rows)?;
        for row in rows {
            if row.role == 2 || row.lights.is_empty() {
                continue;
            }
            queue.write_buffer(
                &self.tile_words,
                u64::from(row.tile_f16_start / 2) * 4,
                &u16_words(&row.tile_f16),
            );
            queue.write_buffer(
                &self.lights,
                u64::from(row.entry_start) * 4,
                &u32_bytes(&row.lights),
            );
            let compaction_byte_offset = self
                .compaction_entry_offset_words
                .checked_add(row.entry_start)
                .and_then(|offset| offset.checked_mul(4))
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
            queue.write_buffer(
                &self.compaction_metadata,
                u64::from(compaction_byte_offset),
                &u32_bytes(&row.entry_tile_f16_offsets),
            );
            // Queue order publishes the pair only after its entry/tile data.
            queue.write_buffer(
                &self.row_pairs,
                u64::from(row.row) * 8,
                &u32_bytes(&[row.entry_start, row.entry_end]),
            );
        }
        Ok(())
    }

    pub(super) fn validate_rows(
        &self,
        rows: &[DirectSparseRowUpload],
    ) -> Result<(), ShResidencyDrainError> {
        for row in rows {
            self.validate_row(row)?;
        }
        Ok(())
    }

    pub(super) fn clear_row_pair(
        &self,
        queue: &wgpu::Queue,
        row: u32,
    ) -> Result<(), ShResidencyDrainError> {
        if row >= self.row_count {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed direct sparse row clear exceeds metadata",
            });
        }
        queue.write_buffer(&self.row_pairs, u64::from(row) * 8, &[0; 8]);
        Ok(())
    }

    pub(super) fn clear_all_row_pairs(
        &self,
        queue: &wgpu::Queue,
    ) -> Result<(), ShResidencyDrainError> {
        let byte_len = usize::try_from(self.row_pairs.size())
            .map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        queue.write_buffer(&self.row_pairs, 0, &vec![0; byte_len]);
        Ok(())
    }

    pub(super) fn fixed_metadata_bytes(&self) -> u64 {
        self.row_pairs.size()
            + self
                .compaction_metadata
                .size()
                .saturating_sub(u64::from(self.entry_capacity) * 4)
    }

    pub(super) fn active_capacity_bytes(&self) -> u64 {
        self.lights.size() + self.tile_words.size() + u64::from(self.entry_capacity) * 4
    }

    fn validate_row(&self, row: &DirectSparseRowUpload) -> Result<(), ShResidencyDrainError> {
        match row.role {
            2 => return Ok(()),
            1 => {}
            _ => {
                return Err(ShResidencyDrainError::MalformedChunk {
                    cluster_id: 0,
                    reason: "streamed direct sparse row has an invalid role",
                });
            }
        }
        if row.row >= self.row_count {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed direct sparse row exceeds metadata",
            });
        }
        // A owned zero-entry row has no delta; its missing pair stays zero but
        // the caller still includes its row in base copy-through dispatches.
        if row.lights.is_empty() {
            if !is_zero_entry_row(row) {
                return Err(ShResidencyDrainError::MalformedChunk {
                    cluster_id: 0,
                    reason: "streamed direct zero-entry row carries payload",
                });
            }
            return Ok(());
        }
        let light_count =
            u32::try_from(row.lights.len()).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
        if row.entry_start == 0
            || row.entry_end <= row.entry_start
            || row.entry_end - row.entry_start != light_count
            || row.lights.len() != row.entry_tile_f16_offsets.len()
        {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed direct sparse row entry allocation is inconsistent",
            });
        }
        // A queue write targets whole u32 words. Individual entry starts can
        // be odd, but allocator-owned row data must be even-length/aligned.
        if !is_word_aligned_tile_row(row) {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed direct sparse row is not word aligned",
            });
        }
        let tile_end = row
            .tile_f16_start
            .checked_add(
                u32::try_from(row.tile_f16.len())
                    .map_err(|_| ShResidencyDrainError::SlotOverflow)?,
            )
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        if row.entry_end > self.entry_capacity || tile_end > self.tile_f16_capacity {
            return Err(ShResidencyDrainError::GpuCapacity {
                reason: "streamed direct sparse row exceeds its GPU pool capacity",
            });
        }
        if row
            .entry_tile_f16_offsets
            .iter()
            .any(|&offset| offset < row.tile_f16_start || offset >= tile_end)
        {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "streamed direct sparse entry points outside its tile allocation",
            });
        }
        Ok(())
    }
}

fn is_zero_entry_row(row: &DirectSparseRowUpload) -> bool {
    row.entry_start == 0
        && row.entry_end == 0
        && row.entry_tile_f16_offsets.is_empty()
        && row.tile_f16.is_empty()
}

fn is_word_aligned_tile_row(row: &DirectSparseRowUpload) -> bool {
    row.tile_f16_start & 1 == 0 && row.tile_f16.len() & 1 == 0
}

pub(super) fn build_grid_and_sparse(
    device: &wgpu::Device,
    base: &ShStreamBaseMetadata,
    source: Option<&postretro_level_loader::ShStreamSparseMetadata>,
    shape: AtlasShape,
    label: &'static str,
) -> Result<(ComposeGridParams, StreamingSparseBuffers, wgpu::Buffer, u64), ShResidencyDrainError> {
    let (affinity_dims, masks, levels, _indices, entry_capacity, tile_f16_capacity) =
        sparse_compose_capacity(base, source)?;
    let cells = checked_cell_count(affinity_dims)?;
    let cells_usize = usize::try_from(cells).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
    let entry_len =
        usize::try_from(entry_capacity).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
    let mut compaction = Vec::with_capacity(
        cells_usize
            .checked_mul(3)
            .and_then(|count| count.checked_add(entry_len))
            .ok_or(ShResidencyDrainError::SlotOverflow)?,
    );
    for mask in masks {
        compaction.push(mask as u32);
        compaction.push((mask >> 32) as u32);
    }
    compaction.extend(levels.into_iter().map(u32::from));
    compaction.resize(
        compaction
            .len()
            .checked_add(entry_len)
            .ok_or(ShResidencyDrainError::SlotOverflow)?,
        0,
    );
    let row_pairs = buffer_with_zeroes(
        device,
        "Streamed Direct SH CSR Row Pairs",
        usize::try_from(cells)
            .ok()
            .and_then(|count| count.checked_mul(8))
            .ok_or(ShResidencyDrainError::SlotOverflow)?,
    );
    let lights = buffer_with_zeroes(
        device,
        "Streamed Direct SH Entry Pool",
        entry_len
            .checked_mul(4)
            .ok_or(ShResidencyDrainError::SlotOverflow)?,
    );
    let tile_words = buffer_with_zeroes(
        device,
        "Streamed Direct SH Delta Tile Pool",
        usize::try_from(tile_f16_capacity.div_ceil(2))
            .ok()
            .and_then(|count| count.checked_mul(4))
            .ok_or(ShResidencyDrainError::SlotOverflow)?,
    );
    let compaction_metadata = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Streamed Direct SH Compaction Metadata"),
        contents: &u32_bytes(&compaction),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });
    let width = shape
        .tiles_per_row
        .checked_mul(8)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let height = shape
        .tiles_per_layer
        .checked_div(shape.tiles_per_row)
        .and_then(|rows| rows.checked_mul(8))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let grid = ComposeGridParams {
        grid_dimensions: base.grid_dimensions,
        atlas_dimensions: [width, height],
        tile_dimension: base.tile_dimension,
        tile_border: base.tile_border,
        atlas_tiles_per_row: shape.tiles_per_row,
        tiles_per_layer: shape.tiles_per_layer,
        atlas_layer_count: shape.layers,
        affinity_dims,
        compact_atlas_tiles_per_row: shape.tiles_per_row,
        compact_atlas_tiles_per_layer: shape.tiles_per_layer,
    };
    let limits = device.limits();
    let record_size = u64::try_from(DYNAMIC_COMPOSE_GRID_DIMS_SIZE)
        .map_err(|_| ShResidencyDrainError::SlotOverflow)?;
    let alignment = u64::from(limits.min_uniform_buffer_offset_alignment.max(1));
    let stride = record_size
        .checked_add(alignment - 1)
        .and_then(|value| value.checked_div(alignment))
        .and_then(|value| value.checked_mul(alignment))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let grid_capacity = stride
        .checked_mul(u64::from(cells.max(1)))
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    if grid_capacity > limits.max_buffer_size {
        return Err(ShResidencyDrainError::GpuCapacity {
            reason: "streamed direct dirty compose records exceed adapter buffer limit",
        });
    }
    let grid_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: grid_capacity.max(record_size),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    Ok((
        grid,
        StreamingSparseBuffers {
            row_pairs,
            lights,
            tile_words,
            compaction_metadata,
            entry_capacity,
            tile_f16_capacity,
            compaction_entry_offset_words: cells
                .checked_mul(3)
                .ok_or(ShResidencyDrainError::SlotOverflow)?,
            row_count: cells,
        },
        grid_buffer,
        grid_capacity,
    ))
}

fn u16_words(values: &[u16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len().div_ceil(2) * size_of::<u32>());
    for pair in values.chunks(2) {
        bytes.extend_from_slice(
            &(u32::from(pair[0]) | (u32::from(pair.get(1).copied().unwrap_or(0)) << 16))
                .to_le_bytes(),
        );
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn odd_entry_offset_keeps_row_upload_word_aligned() {
        let row = DirectSparseRowUpload {
            row: 4,
            role: 1,
            entry_start: 1,
            entry_end: 2,
            tile_f16_start: 8,
            lights: vec![7],
            entry_tile_f16_offsets: vec![9],
            tile_f16: vec![1, 2, 3, 4],
        };
        assert_eq!(row.entry_tile_f16_offsets[0] & 1, 1);
        assert!(is_word_aligned_tile_row(&row));
        assert_eq!(u16_words(&row.tile_f16), vec![1, 0, 2, 0, 3, 0, 4, 0]);
    }

    #[test]
    fn owned_zero_entry_row_has_no_reachable_payload() {
        let row = DirectSparseRowUpload {
            row: 4,
            role: 1,
            entry_start: 0,
            entry_end: 0,
            tile_f16_start: 0,
            lights: vec![],
            entry_tile_f16_offsets: vec![],
            tile_f16: vec![],
        };
        assert!(is_zero_entry_row(&row));
        assert!(is_word_aligned_tile_row(&row));
    }
}
