// Format-agnostic vertex and draw-chunk types shared by the PRL loader and renderer.
// See: context/lib/rendering_pipeline.md §5, §6

/// World-geometry vertex: position + UV + octahedral normal + octahedral tangent.
/// Matches the GeometryV3 on-disk layout. Normal and tangent are decoded in the
/// vertex shader; the fragment shader receives them as interpolants but does not
/// use them until Phase 4 adds lighting.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldVertex {
    pub position: [f32; 3],
    pub base_uv: [f32; 2],
    /// Octahedral-encoded unit normal (u16 x 2).
    pub normal_oct: [u16; 2],
    /// Packed tangent: u16 octahedral u-component, u16 v-component with
    /// bitangent sign in bit 15.
    pub tangent_packed: [u16; 2],
}

impl WorldVertex {
    /// Stride in bytes: 12 (pos) + 8 (uv) + 4 (normal) + 4 (tangent) = 28 bytes.
    pub const STRIDE: usize = 28;
}

/// A draw chunk referencing a contiguous index range for one (cell, material bucket)
/// pair. Loaded from the CellChunks PRL section. The renderer uses these for
/// per-cell, per-material draw call dispatch. AABB fields are consumed by GPU
/// frustum culling in the compute prepass.
#[derive(Debug, Clone)]
pub struct DrawChunk {
    /// Opaque cell identifier. Runtime never interprets this as a BSP leaf index.
    /// Read by GPU via storage buffer upload in compute_cull.rs.
    #[allow(dead_code)]
    pub cell_id: u32,
    /// World-space AABB minimum corner. Read by GPU for frustum culling.
    #[allow(dead_code)]
    pub aabb_min: [f32; 3],
    /// World-space AABB maximum corner. Read by GPU for frustum culling.
    #[allow(dead_code)]
    pub aabb_max: [f32; 3],
    /// Start of this chunk's indices in the shared index buffer.
    pub index_offset: u32,
    /// Number of indices in this chunk's range.
    pub index_count: u32,
    /// Material bucket (texture index) this chunk's indices reference.
    pub material_bucket_id: u32,
}

/// Cell-to-chunk-range index entry. Maps a cell_id to its contiguous
/// range of chunks in the chunk array.
#[derive(Debug, Clone)]
pub struct CellRange {
    pub cell_id: u32,
    pub chunk_start: u32,
    pub chunk_count: u32,
}

/// Per-cell draw chunk table loaded from the CellChunks PRL section.
/// Provides O(log n) lookup of all chunks for a given cell via binary search
/// on the sorted cell_ranges array.
#[derive(Debug, Clone)]
pub struct CellChunkTable {
    pub cell_ranges: Vec<CellRange>,
    pub chunks: Vec<DrawChunk>,
}

impl CellChunkTable {
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

    /// Return a list of all distinct cell IDs in the table.
    #[cfg(test)]
    fn cell_ids(&self) -> Vec<u32> {
        self.cell_ranges.iter().map(|cr| cr.cell_id).collect()
    }

    /// Total number of chunks across all cells.
    #[cfg(test)]
    fn total_chunks(&self) -> usize {
        self.chunks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_table() -> CellChunkTable {
        CellChunkTable {
            cell_ranges: vec![
                CellRange {
                    cell_id: 0,
                    chunk_start: 0,
                    chunk_count: 2,
                },
                CellRange {
                    cell_id: 5,
                    chunk_start: 2,
                    chunk_count: 1,
                },
            ],
            chunks: vec![
                DrawChunk {
                    cell_id: 0,
                    aabb_min: [-1.0, -1.0, -1.0],
                    aabb_max: [1.0, 1.0, 1.0],
                    index_offset: 0,
                    index_count: 6,
                    material_bucket_id: 0,
                },
                DrawChunk {
                    cell_id: 0,
                    aabb_min: [-1.0, -1.0, -1.0],
                    aabb_max: [1.0, 1.0, 1.0],
                    index_offset: 6,
                    index_count: 3,
                    material_bucket_id: 1,
                },
                DrawChunk {
                    cell_id: 5,
                    aabb_min: [10.0, 0.0, 10.0],
                    aabb_max: [20.0, 5.0, 20.0],
                    index_offset: 9,
                    index_count: 12,
                    material_bucket_id: 0,
                },
            ],
        }
    }

    #[test]
    fn chunks_for_cell_returns_correct_range() {
        let table = sample_table();
        let cell0 = table.chunks_for_cell(0);
        assert_eq!(cell0.len(), 2);
        assert_eq!(cell0[0].material_bucket_id, 0);
        assert_eq!(cell0[1].material_bucket_id, 1);

        let cell5 = table.chunks_for_cell(5);
        assert_eq!(cell5.len(), 1);
        assert_eq!(cell5[0].index_offset, 9);
    }

    #[test]
    fn chunks_for_absent_cell_returns_empty() {
        let table = sample_table();
        let empty = table.chunks_for_cell(999);
        assert!(empty.is_empty());
    }

    #[test]
    fn empty_table_returns_empty_for_any_cell() {
        let table = CellChunkTable {
            cell_ranges: vec![],
            chunks: vec![],
        };
        assert!(table.chunks_for_cell(0).is_empty());
        assert!(table.chunks_for_cell(u32::MAX).is_empty());
        assert_eq!(table.total_chunks(), 0);
        assert!(table.cell_ids().is_empty());
    }

    #[test]
    fn cell_ids_returns_all_ids_in_order() {
        let table = sample_table();
        assert_eq!(table.cell_ids(), vec![0, 5]);
    }

    #[test]
    fn total_chunks_counts_all_chunks() {
        let table = sample_table();
        assert_eq!(table.total_chunks(), 3);
    }

    #[test]
    fn single_cell_single_chunk_lookup() {
        let table = CellChunkTable {
            cell_ranges: vec![CellRange {
                cell_id: 42,
                chunk_start: 0,
                chunk_count: 1,
            }],
            chunks: vec![DrawChunk {
                cell_id: 42,
                aabb_min: [0.0; 3],
                aabb_max: [1.0; 3],
                index_offset: 0,
                index_count: 3,
                material_bucket_id: 0,
            }],
        };

        let chunks = table.chunks_for_cell(42);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].cell_id, 42);
        assert_eq!(chunks[0].index_count, 3);
    }

    #[test]
    fn many_cells_lookup_spot_check() {
        // 20+ cells to simulate a cell-heavy test map.
        let num_cells = 25u32;
        let mut cell_ranges = Vec::new();
        let mut chunks = Vec::new();
        for cell_id in 0..num_cells {
            cell_ranges.push(CellRange {
                cell_id,
                chunk_start: cell_id * 2,
                chunk_count: 2,
            });
            for mat in 0..2u32 {
                chunks.push(DrawChunk {
                    cell_id,
                    aabb_min: [cell_id as f32, 0.0, 0.0],
                    aabb_max: [cell_id as f32 + 1.0, 1.0, 1.0],
                    index_offset: (cell_id * 2 + mat) * 6,
                    index_count: 6,
                    material_bucket_id: mat,
                });
            }
        }
        let table = CellChunkTable {
            cell_ranges,
            chunks,
        };

        assert_eq!(table.total_chunks(), 50);
        assert_eq!(table.cell_ids().len(), 25);

        // Spot-check middle and edge cells.
        let c0 = table.chunks_for_cell(0);
        assert_eq!(c0.len(), 2);
        assert_eq!(c0[0].cell_id, 0);

        let c12 = table.chunks_for_cell(12);
        assert_eq!(c12.len(), 2);
        assert_eq!(c12[0].cell_id, 12);

        let c24 = table.chunks_for_cell(24);
        assert_eq!(c24.len(), 2);
        assert_eq!(c24[0].cell_id, 24);

        // Non-existent cell.
        assert!(table.chunks_for_cell(25).is_empty());
    }
}
