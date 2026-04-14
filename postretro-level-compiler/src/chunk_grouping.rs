// Per-cell chunk grouping: group faces by (cell, material bucket), reorder
// indices for contiguous draws, compute per-chunk AABBs, build cell->chunk-range
// index. Runs after geometry extraction and emits a CellChunksSection.
// See: context/lib/rendering_pipeline.md §5

use std::collections::BTreeMap;

use postretro_level_format::cell_chunks::{CellChunksSection, CellRange, DrawChunk};
use postretro_level_format::geometry::GeometrySectionV3;

/// Group geometry faces by (cell_id, material_bucket_id), reorder the index
/// buffer so each group is contiguous, and build the chunk table with AABBs
/// and cell->chunk-range index.
///
/// `cell_id` is the face's `leaf_index` (raw BSP leaf index).
/// `material_bucket_id` is the face's `texture_index`.
///
/// Returns the reordered index buffer and the CellChunksSection. The caller
/// must replace the geometry section's indices with the reordered buffer.
pub fn build_cell_chunks(geometry: &GeometrySectionV3) -> (Vec<u32>, CellChunksSection) {
    if geometry.faces.is_empty() {
        return (
            Vec::new(),
            CellChunksSection {
                cell_ranges: Vec::new(),
                chunks: Vec::new(),
            },
        );
    }

    // Group face indices by (cell_id, material_bucket_id). BTreeMap gives
    // sorted iteration over keys, which is what we want: chunks sorted by
    // cell_id then by material_bucket_id within each cell.
    let mut groups: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
    for (face_idx, face) in geometry.faces.iter().enumerate() {
        let key = (face.leaf_index, face.texture_index);
        groups.entry(key).or_default().push(face_idx);
    }

    // Build the reordered index buffer and chunk records.
    let mut reordered_indices: Vec<u32> = Vec::with_capacity(geometry.indices.len());
    let mut chunks: Vec<DrawChunk> = Vec::new();

    // Track chunks per cell for the cell-range index.
    // Use BTreeMap so cells come out sorted by cell_id.
    let mut cell_chunk_counts: BTreeMap<u32, u32> = BTreeMap::new();

    for (&(cell_id, material_bucket_id), face_indices) in &groups {
        let index_offset = reordered_indices.len() as u32;

        // AABB for this chunk, computed from vertex positions.
        let mut aabb_min = [f32::INFINITY; 3];
        let mut aabb_max = [f32::NEG_INFINITY; 3];

        for &face_idx in face_indices {
            let face = &geometry.faces[face_idx];
            let start = face.index_offset as usize;
            let end = start + face.index_count as usize;

            for &vertex_idx in &geometry.indices[start..end] {
                reordered_indices.push(vertex_idx);

                let pos = &geometry.vertices[vertex_idx as usize].position;
                for i in 0..3 {
                    aabb_min[i] = aabb_min[i].min(pos[i]);
                    aabb_max[i] = aabb_max[i].max(pos[i]);
                }
            }
        }

        let index_count = reordered_indices.len() as u32 - index_offset;

        chunks.push(DrawChunk {
            cell_id,
            aabb_min,
            aabb_max,
            index_offset,
            index_count,
            material_bucket_id,
        });

        *cell_chunk_counts.entry(cell_id).or_insert(0) += 1;
    }

    // Build cell->chunk-range index. Chunks are already sorted by cell_id
    // (BTreeMap iteration order), so ranges are contiguous.
    let mut cell_ranges: Vec<CellRange> = Vec::with_capacity(cell_chunk_counts.len());
    let mut chunk_start: u32 = 0;
    for (&cell_id, &chunk_count) in &cell_chunk_counts {
        cell_ranges.push(CellRange {
            cell_id,
            chunk_start,
            chunk_count,
        });
        chunk_start += chunk_count;
    }

    let section = CellChunksSection {
        cell_ranges,
        chunks,
    };

    (reordered_indices, section)
}

/// Log chunk grouping statistics.
pub fn log_stats(section: &CellChunksSection) {
    log::info!(
        "[Compiler] CellChunks: {} cells, {} chunks",
        section.cell_ranges.len(),
        section.chunks.len()
    );
    if !section.cell_ranges.is_empty() {
        let avg = section.chunks.len() as f64 / section.cell_ranges.len() as f64;
        log::info!(
            "[Compiler]   Average chunks per cell: {avg:.1}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::geometry::{FaceMetaV3, VertexV3};

    /// Build a minimal geometry section with controlled face/leaf/texture layout.
    fn make_geometry(
        positions: &[[f32; 3]],
        faces: &[(u32, u32, u32, u32)], // (index_offset, index_count, leaf_index, texture_index)
        indices: &[u32],
    ) -> GeometrySectionV3 {
        let vertices: Vec<VertexV3> = positions
            .iter()
            .map(|&pos| {
                VertexV3::new(pos, [0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], true)
            })
            .collect();
        let face_metas: Vec<FaceMetaV3> = faces
            .iter()
            .map(|&(index_offset, index_count, leaf_index, texture_index)| FaceMetaV3 {
                index_offset,
                index_count,
                leaf_index,
                texture_index,
            })
            .collect();
        GeometrySectionV3 {
            vertices,
            indices: indices.to_vec(),
            faces: face_metas,
        }
    }

    #[test]
    fn single_face_single_chunk() {
        // One face in leaf 0, texture 0, triangle (0,1,2)
        let geo = make_geometry(
            &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            &[(0, 3, 0, 0)],
            &[0, 1, 2],
        );

        let (reordered, section) = build_cell_chunks(&geo);

        assert_eq!(section.chunks.len(), 1, "one face -> one chunk");
        assert_eq!(section.cell_ranges.len(), 1, "one cell");

        let chunk = &section.chunks[0];
        assert_eq!(chunk.cell_id, 0);
        assert_eq!(chunk.material_bucket_id, 0);
        assert_eq!(chunk.index_offset, 0);
        assert_eq!(chunk.index_count, 3);
        assert_eq!(reordered, vec![0, 1, 2]);
    }

    #[test]
    fn two_faces_same_cell_same_material_merge_into_one_chunk() {
        // Two faces in leaf 0, same texture 0
        let geo = make_geometry(
            &[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [2.0, 0.0, 0.0],
                [3.0, 0.0, 0.0],
                [2.0, 1.0, 0.0],
            ],
            &[(0, 3, 0, 0), (3, 3, 0, 0)],
            &[0, 1, 2, 3, 4, 5],
        );

        let (reordered, section) = build_cell_chunks(&geo);

        assert_eq!(section.chunks.len(), 1, "same cell+material -> one chunk");
        assert_eq!(section.chunks[0].index_count, 6);
        assert_eq!(reordered.len(), 6);
    }

    #[test]
    fn two_faces_same_cell_different_materials_produce_two_chunks() {
        // Two faces in leaf 0, different textures
        let geo = make_geometry(
            &[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [2.0, 0.0, 0.0],
                [3.0, 0.0, 0.0],
                [2.0, 1.0, 0.0],
            ],
            &[(0, 3, 0, 0), (3, 3, 0, 1)],
            &[0, 1, 2, 3, 4, 5],
        );

        let (reordered, section) = build_cell_chunks(&geo);

        assert_eq!(section.chunks.len(), 2, "different materials -> two chunks");
        assert_eq!(section.cell_ranges.len(), 1, "same cell -> one range");

        // Chunks should be sorted by material within the cell
        assert_eq!(section.chunks[0].material_bucket_id, 0);
        assert_eq!(section.chunks[1].material_bucket_id, 1);
        assert_eq!(section.chunks[0].index_count, 3);
        assert_eq!(section.chunks[1].index_count, 3);

        // Index ranges are contiguous and non-overlapping
        assert_eq!(section.chunks[0].index_offset, 0);
        assert_eq!(section.chunks[1].index_offset, 3);
        assert_eq!(reordered.len(), 6);
    }

    #[test]
    fn two_cells_produce_separate_ranges() {
        // Face in leaf 0 texture 0, face in leaf 1 texture 0
        let geo = make_geometry(
            &[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [5.0, 0.0, 0.0],
                [6.0, 0.0, 0.0],
                [5.0, 1.0, 0.0],
            ],
            &[(0, 3, 0, 0), (3, 3, 1, 0)],
            &[0, 1, 2, 3, 4, 5],
        );

        let (_, section) = build_cell_chunks(&geo);

        assert_eq!(section.cell_ranges.len(), 2, "two cells -> two ranges");
        assert_eq!(section.chunks.len(), 2, "two chunks total");

        // Cell range index correctness
        assert_eq!(section.cell_ranges[0].cell_id, 0);
        assert_eq!(section.cell_ranges[0].chunk_start, 0);
        assert_eq!(section.cell_ranges[0].chunk_count, 1);
        assert_eq!(section.cell_ranges[1].cell_id, 1);
        assert_eq!(section.cell_ranges[1].chunk_start, 1);
        assert_eq!(section.cell_ranges[1].chunk_count, 1);

        // chunks_for_cell lookup works
        let c0 = section.chunks_for_cell(0);
        assert_eq!(c0.len(), 1);
        assert_eq!(c0[0].cell_id, 0);
        let c1 = section.chunks_for_cell(1);
        assert_eq!(c1.len(), 1);
        assert_eq!(c1[0].cell_id, 1);
    }

    #[test]
    fn aabb_tightly_bounds_chunk_geometry() {
        // Triangle with known bounds
        let geo = make_geometry(
            &[[-1.0, 2.0, -3.0], [4.0, -5.0, 6.0], [0.0, 0.0, 0.0]],
            &[(0, 3, 0, 0)],
            &[0, 1, 2],
        );

        let (_, section) = build_cell_chunks(&geo);

        let chunk = &section.chunks[0];
        assert_eq!(chunk.aabb_min, [-1.0, -5.0, -3.0]);
        assert_eq!(chunk.aabb_max, [4.0, 2.0, 6.0]);
    }

    #[test]
    fn index_ranges_contiguous_and_non_overlapping() {
        // Multiple cells and materials
        let geo = make_geometry(
            &[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [2.0, 0.0, 0.0],
                [3.0, 0.0, 0.0],
                [2.0, 1.0, 0.0],
                [5.0, 0.0, 0.0],
                [6.0, 0.0, 0.0],
                [5.0, 1.0, 0.0],
            ],
            &[
                (0, 3, 0, 0), // cell 0, tex 0
                (3, 3, 0, 1), // cell 0, tex 1
                (6, 3, 1, 0), // cell 1, tex 0
            ],
            &[0, 1, 2, 3, 4, 5, 6, 7, 8],
        );

        let (reordered, section) = build_cell_chunks(&geo);

        // All index ranges contiguous
        let mut end = 0u32;
        for chunk in &section.chunks {
            assert_eq!(
                chunk.index_offset, end,
                "chunk at offset {} != expected {}",
                chunk.index_offset, end
            );
            end = chunk.index_offset + chunk.index_count;
        }
        assert_eq!(end as usize, reordered.len());
    }

    #[test]
    fn empty_geometry_produces_empty_chunks() {
        let geo = GeometrySectionV3 {
            vertices: Vec::new(),
            indices: Vec::new(),
            faces: Vec::new(),
        };

        let (reordered, section) = build_cell_chunks(&geo);

        assert!(section.chunks.is_empty());
        assert!(section.cell_ranges.is_empty());
        assert!(reordered.is_empty());
    }

    #[test]
    fn reordered_indices_reference_same_vertices() {
        // Two faces in different order: leaf 1 before leaf 0 in the original
        // index buffer. Chunk grouping should reorder so leaf 0 comes first.
        let geo = make_geometry(
            &[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [5.0, 0.0, 0.0],
                [6.0, 0.0, 0.0],
                [5.0, 1.0, 0.0],
            ],
            &[
                (0, 3, 1, 0), // leaf 1 first in original
                (3, 3, 0, 0), // leaf 0 second in original
            ],
            &[0, 1, 2, 3, 4, 5],
        );

        let (reordered, section) = build_cell_chunks(&geo);

        // After grouping, cell 0 should come before cell 1
        assert_eq!(section.chunks[0].cell_id, 0);
        assert_eq!(section.chunks[1].cell_id, 1);

        // Reordered indices should have leaf 0's indices first
        assert_eq!(reordered[0..3], [3, 4, 5]);
        assert_eq!(reordered[3..6], [0, 1, 2]);

        // All indices still valid
        for &idx in &reordered {
            assert!(
                (idx as usize) < geo.vertices.len(),
                "index {idx} out of bounds"
            );
        }
    }

    #[test]
    fn multiple_materials_per_cell_sorted_by_bucket() {
        // Cell 0 with 3 different materials, inserted in reverse order
        let geo = make_geometry(
            &[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [2.0, 0.0, 0.0],
                [3.0, 0.0, 0.0],
                [2.0, 1.0, 0.0],
                [4.0, 0.0, 0.0],
                [5.0, 0.0, 0.0],
                [4.0, 1.0, 0.0],
            ],
            &[
                (0, 3, 0, 2), // tex 2
                (3, 3, 0, 0), // tex 0
                (6, 3, 0, 1), // tex 1
            ],
            &[0, 1, 2, 3, 4, 5, 6, 7, 8],
        );

        let (_, section) = build_cell_chunks(&geo);

        assert_eq!(section.chunks.len(), 3);
        assert_eq!(section.cell_ranges.len(), 1);
        assert_eq!(section.cell_ranges[0].chunk_count, 3);

        // Sorted by material_bucket_id within cell
        assert_eq!(section.chunks[0].material_bucket_id, 0);
        assert_eq!(section.chunks[1].material_bucket_id, 1);
        assert_eq!(section.chunks[2].material_bucket_id, 2);
    }

    #[test]
    fn round_trip_through_format_crate() {
        // Build chunks, serialize, deserialize, verify equality
        let geo = make_geometry(
            &[
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [5.0, 0.0, 0.0],
                [6.0, 0.0, 0.0],
                [5.0, 1.0, 0.0],
            ],
            &[(0, 3, 0, 0), (3, 3, 1, 1)],
            &[0, 1, 2, 3, 4, 5],
        );

        let (_, section) = build_cell_chunks(&geo);
        let bytes = section.to_bytes();
        let restored = CellChunksSection::from_bytes(&bytes).expect("round-trip should succeed");
        assert_eq!(section, restored);
    }
}
