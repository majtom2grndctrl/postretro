// Per-cell draw chunk table with cell->chunk-range index.
// See: context/lib/rendering_pipeline.md §5

use crate::FormatError;

/// A single draw chunk: one (cell, material bucket) pair.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawChunk {
    /// Opaque cell identifier. Runtime never interprets this as a BSP leaf index.
    pub cell_id: u32,
    /// World-space AABB minimum corner.
    pub aabb_min: [f32; 3],
    /// World-space AABB maximum corner.
    pub aabb_max: [f32; 3],
    /// Start of this chunk's indices in the shared index buffer.
    pub index_offset: u32,
    /// Number of indices in this chunk's range.
    pub index_count: u32,
    /// Material bucket this chunk's indices reference.
    pub material_bucket_id: u32,
}

// On-disk size per DrawChunk: 4 + 24 + 4 + 4 + 4 = 40 bytes
const CHUNK_SIZE: usize = 40;

/// Cell-to-chunk-range index entry. Maps a cell_id to its contiguous
/// range of chunks in the chunk array.
#[derive(Debug, Clone, PartialEq)]
pub struct CellRange {
    /// Opaque cell identifier.
    pub cell_id: u32,
    /// Index of the first chunk for this cell in the chunks array.
    pub chunk_start: u32,
    /// Number of chunks belonging to this cell.
    pub chunk_count: u32,
}

// On-disk size per CellRange: 4 + 4 + 4 = 12 bytes
const CELL_RANGE_SIZE: usize = 12;

/// CellChunks section: per-cell draw chunk table with O(1) cell lookup.
///
/// On-disk layout (all little-endian):
///   u32  cell_count     (number of cell-range index entries)
///   u32  chunk_count    (number of draw chunks)
///   CellRange * cell_count   (12 bytes each)
///   DrawChunk * chunk_count  (40 bytes each)
///
/// Chunks are sorted by cell_id with the cell_ranges providing the index.
/// Within a cell, chunks are ordered by material_bucket_id.
#[derive(Debug, Clone, PartialEq)]
pub struct CellChunksSection {
    /// Cell-to-chunk-range index, sorted by cell_id.
    pub cell_ranges: Vec<CellRange>,
    /// Flat array of draw chunks, grouped contiguously by cell.
    pub chunks: Vec<DrawChunk>,
}

impl CellChunksSection {
    /// Look up all chunks for a given cell_id. Returns an empty slice if the
    /// cell_id is not present. Uses binary search on the sorted cell_ranges.
    pub fn chunks_for_cell(&self, cell_id: u32) -> &[DrawChunk] {
        match self
            .cell_ranges
            .binary_search_by_key(&cell_id, |cr| cr.cell_id)
        {
            Ok(idx) => {
                let range = &self.cell_ranges[idx];
                let start = range.chunk_start as usize;
                let end = start + range.chunk_count as usize;
                &self.chunks[start..end]
            }
            Err(_) => &[],
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let cell_count = self.cell_ranges.len() as u32;
        let chunk_count = self.chunks.len() as u32;

        let size = 8
            + (self.cell_ranges.len() * CELL_RANGE_SIZE)
            + (self.chunks.len() * CHUNK_SIZE);
        let mut buf = Vec::with_capacity(size);

        buf.extend_from_slice(&cell_count.to_le_bytes());
        buf.extend_from_slice(&chunk_count.to_le_bytes());

        for cr in &self.cell_ranges {
            buf.extend_from_slice(&cr.cell_id.to_le_bytes());
            buf.extend_from_slice(&cr.chunk_start.to_le_bytes());
            buf.extend_from_slice(&cr.chunk_count.to_le_bytes());
        }

        for chunk in &self.chunks {
            buf.extend_from_slice(&chunk.cell_id.to_le_bytes());
            buf.extend_from_slice(&chunk.aabb_min[0].to_le_bytes());
            buf.extend_from_slice(&chunk.aabb_min[1].to_le_bytes());
            buf.extend_from_slice(&chunk.aabb_min[2].to_le_bytes());
            buf.extend_from_slice(&chunk.aabb_max[0].to_le_bytes());
            buf.extend_from_slice(&chunk.aabb_max[1].to_le_bytes());
            buf.extend_from_slice(&chunk.aabb_max[2].to_le_bytes());
            buf.extend_from_slice(&chunk.index_offset.to_le_bytes());
            buf.extend_from_slice(&chunk.index_count.to_le_bytes());
            buf.extend_from_slice(&chunk.material_bucket_id.to_le_bytes());
        }

        buf
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        if data.len() < 8 {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "cell chunks section too short for header",
            )));
        }

        let cell_count = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let chunk_count = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;

        let expected_size =
            8 + (cell_count * CELL_RANGE_SIZE) + (chunk_count * CHUNK_SIZE);
        if data.len() < expected_size {
            return Err(FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "cell chunks section too short: need {expected_size} bytes, got {}",
                    data.len()
                ),
            )));
        }

        let mut offset = 8;

        let mut cell_ranges = Vec::with_capacity(cell_count);
        for _ in 0..cell_count {
            let cell_id = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            let chunk_start = u32::from_le_bytes([
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]);
            let chunk_count = u32::from_le_bytes([
                data[offset + 8],
                data[offset + 9],
                data[offset + 10],
                data[offset + 11],
            ]);
            cell_ranges.push(CellRange {
                cell_id,
                chunk_start,
                chunk_count,
            });
            offset += CELL_RANGE_SIZE;
        }

        let mut chunks = Vec::with_capacity(chunk_count);
        for _ in 0..chunk_count {
            let cell_id = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]);
            let aabb_min_x = f32::from_le_bytes([
                data[offset + 4],
                data[offset + 5],
                data[offset + 6],
                data[offset + 7],
            ]);
            let aabb_min_y = f32::from_le_bytes([
                data[offset + 8],
                data[offset + 9],
                data[offset + 10],
                data[offset + 11],
            ]);
            let aabb_min_z = f32::from_le_bytes([
                data[offset + 12],
                data[offset + 13],
                data[offset + 14],
                data[offset + 15],
            ]);
            let aabb_max_x = f32::from_le_bytes([
                data[offset + 16],
                data[offset + 17],
                data[offset + 18],
                data[offset + 19],
            ]);
            let aabb_max_y = f32::from_le_bytes([
                data[offset + 20],
                data[offset + 21],
                data[offset + 22],
                data[offset + 23],
            ]);
            let aabb_max_z = f32::from_le_bytes([
                data[offset + 24],
                data[offset + 25],
                data[offset + 26],
                data[offset + 27],
            ]);
            let index_offset = u32::from_le_bytes([
                data[offset + 28],
                data[offset + 29],
                data[offset + 30],
                data[offset + 31],
            ]);
            let index_count = u32::from_le_bytes([
                data[offset + 32],
                data[offset + 33],
                data[offset + 34],
                data[offset + 35],
            ]);
            let material_bucket_id = u32::from_le_bytes([
                data[offset + 36],
                data[offset + 37],
                data[offset + 38],
                data[offset + 39],
            ]);
            chunks.push(DrawChunk {
                cell_id,
                aabb_min: [aabb_min_x, aabb_min_y, aabb_min_z],
                aabb_max: [aabb_max_x, aabb_max_y, aabb_max_z],
                index_offset,
                index_count,
                material_bucket_id,
            });
            offset += CHUNK_SIZE;
        }

        Ok(Self {
            cell_ranges,
            chunks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_section() -> CellChunksSection {
        // Two cells: cell 0 has 2 chunks (2 materials), cell 3 has 1 chunk
        CellChunksSection {
            cell_ranges: vec![
                CellRange {
                    cell_id: 0,
                    chunk_start: 0,
                    chunk_count: 2,
                },
                CellRange {
                    cell_id: 3,
                    chunk_start: 2,
                    chunk_count: 1,
                },
            ],
            chunks: vec![
                DrawChunk {
                    cell_id: 0,
                    aabb_min: [-1.0, -2.0, -3.0],
                    aabb_max: [1.0, 2.0, 3.0],
                    index_offset: 0,
                    index_count: 12,
                    material_bucket_id: 0,
                },
                DrawChunk {
                    cell_id: 0,
                    aabb_min: [-0.5, 0.0, -0.5],
                    aabb_max: [0.5, 1.0, 0.5],
                    index_offset: 12,
                    index_count: 6,
                    material_bucket_id: 2,
                },
                DrawChunk {
                    cell_id: 3,
                    aabb_min: [10.0, 0.0, 10.0],
                    aabb_max: [20.0, 5.0, 20.0],
                    index_offset: 18,
                    index_count: 24,
                    material_bucket_id: 1,
                },
            ],
        }
    }

    #[test]
    fn round_trip_preserves_all_fields() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = CellChunksSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn cell_range_index_maps_correctly() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = CellChunksSection::from_bytes(&bytes).unwrap();

        // Cell 0 should have 2 chunks
        let cell0 = restored.chunks_for_cell(0);
        assert_eq!(cell0.len(), 2);
        assert_eq!(cell0[0].material_bucket_id, 0);
        assert_eq!(cell0[1].material_bucket_id, 2);

        // Cell 3 should have 1 chunk
        let cell3 = restored.chunks_for_cell(3);
        assert_eq!(cell3.len(), 1);
        assert_eq!(cell3[0].material_bucket_id, 1);
        assert_eq!(cell3[0].index_offset, 18);
    }

    #[test]
    fn absent_cell_returns_empty_slice() {
        let section = sample_section();
        let chunks = section.chunks_for_cell(999);
        assert!(chunks.is_empty());
    }

    #[test]
    fn aabb_values_preserved() {
        let section = sample_section();
        let bytes = section.to_bytes();
        let restored = CellChunksSection::from_bytes(&bytes).unwrap();

        assert_eq!(restored.chunks[0].aabb_min, [-1.0, -2.0, -3.0]);
        assert_eq!(restored.chunks[0].aabb_max, [1.0, 2.0, 3.0]);
        assert_eq!(restored.chunks[2].aabb_min, [10.0, 0.0, 10.0]);
        assert_eq!(restored.chunks[2].aabb_max, [20.0, 5.0, 20.0]);
    }

    #[test]
    fn index_ranges_non_overlapping() {
        let section = sample_section();
        // Verify chunks have contiguous, non-overlapping index ranges
        let mut end = 0u32;
        for chunk in &section.chunks {
            assert_eq!(
                chunk.index_offset, end,
                "chunk index_offset {} != expected {end}",
                chunk.index_offset,
            );
            end = chunk.index_offset + chunk.index_count;
        }
    }

    #[test]
    fn empty_section_round_trips() {
        let section = CellChunksSection {
            cell_ranges: vec![],
            chunks: vec![],
        };
        let bytes = section.to_bytes();
        let restored = CellChunksSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);
    }

    #[test]
    fn rejects_truncated_header() {
        let result = CellChunksSection::from_bytes(&[0; 4]);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_short_body() {
        // Header claims 1 cell range, 0 chunks, but body is missing
        let mut data = vec![0u8; 8];
        data[0] = 1; // cell_count = 1
        let result = CellChunksSection::from_bytes(&data);
        assert!(result.is_err());
    }

    #[test]
    fn byte_layout_header() {
        let section = sample_section();
        let bytes = section.to_bytes();

        let cell_count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let chunk_count = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);

        assert_eq!(cell_count, 2);
        assert_eq!(chunk_count, 3);
    }

    #[test]
    fn single_cell_single_chunk() {
        let section = CellChunksSection {
            cell_ranges: vec![CellRange {
                cell_id: 42,
                chunk_start: 0,
                chunk_count: 1,
            }],
            chunks: vec![DrawChunk {
                cell_id: 42,
                aabb_min: [0.0, 0.0, 0.0],
                aabb_max: [1.0, 1.0, 1.0],
                index_offset: 0,
                index_count: 3,
                material_bucket_id: 0,
            }],
        };
        let bytes = section.to_bytes();
        let restored = CellChunksSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);

        let chunks = restored.chunks_for_cell(42);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].cell_id, 42);
    }

    #[test]
    fn many_cells_round_trip() {
        // 10 cells, each with 3 chunks
        let mut cell_ranges = Vec::new();
        let mut chunks = Vec::new();
        for cell_id in 0..10u32 {
            cell_ranges.push(CellRange {
                cell_id,
                chunk_start: cell_id * 3,
                chunk_count: 3,
            });
            for mat in 0..3u32 {
                chunks.push(DrawChunk {
                    cell_id,
                    aabb_min: [cell_id as f32, 0.0, 0.0],
                    aabb_max: [cell_id as f32 + 1.0, 1.0, 1.0],
                    index_offset: (cell_id * 3 + mat) * 6,
                    index_count: 6,
                    material_bucket_id: mat,
                });
            }
        }
        let section = CellChunksSection {
            cell_ranges,
            chunks,
        };
        let bytes = section.to_bytes();
        let restored = CellChunksSection::from_bytes(&bytes).unwrap();
        assert_eq!(section, restored);

        // Spot-check a middle cell
        let cell5 = restored.chunks_for_cell(5);
        assert_eq!(cell5.len(), 3);
        assert_eq!(cell5[0].cell_id, 5);
    }
}
