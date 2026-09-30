// Animated-lightmap block ids on geometry vertices: the guards the compact
// repack relies on, and the stamp that writes each face's block id.
// See: context/lib/build_pipeline.md §PRL section IDs (AnimatedLightWeightMaps)

use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::{
    AnimatedBlock, AnimatedLightWeightMapsSection,
};
use postretro_level_format::animated_lightmap_atlas::ANIMATED_BLOCK_CAP;
use postretro_level_format::geometry::GeometrySection;
use thiserror::Error;

use crate::geometry::FaceIndexRange;
use crate::lightmap_bake::BlockLayout;

/// A compiled level that would break the repack's assumptions.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum AnimatedBlockGuardError {
    #[error(
        "animated block count {count} exceeds the block-table cap {cap}; the forward \
         shader's binding-7 table cannot name more blocks"
    )]
    BlockCountOverCap { count: usize, cap: u32 },
    #[error(
        "vertex {vertex} is shared by face {face_a} and face {face_b}, and face {block_face} \
         owns an animated block; per-vertex block ids need unshared vertices"
    )]
    SharedVertex {
        vertex: u32,
        face_a: usize,
        face_b: usize,
        block_face: usize,
    },
    #[error(
        "face {face} vertex {vertex}: bilinear footprint texels {footprint:?} in lightmap cell \
         block {lightmap_block} leave the face's chart placement {placement:?}; the compact \
         repack would sample another block"
    )]
    FootprintOutsidePlacement {
        face: usize,
        vertex: u32,
        lightmap_block: u32,
        /// `[x0, y0, x1, y1]`, inclusive block-local texels.
        footprint: [i64; 4],
        /// `[x, y, width, height]`, block-local.
        placement: [u32; 4],
    },
    #[error(
        "face {face} vertex {vertex} names lightmap block slot {vertex_block} (id + 1) but its \
         animated block is keyed to lightmap block {block}"
    )]
    VertexBlockMismatch {
        face: usize,
        vertex: u32,
        vertex_block: u32,
        block: u32,
    },
    #[error("face {face} has chunks in blocks {block_a} and {block_b}; one face owns one block")]
    FaceSpansBlocks {
        face: usize,
        block_a: u32,
        block_b: u32,
    },
}

/// Reject a block count the shared block table cannot hold.
pub(crate) fn check_block_cap(count: usize, cap: u32) -> Result<(), AnimatedBlockGuardError> {
    if count > cap as usize {
        Err(AnimatedBlockGuardError::BlockCountOverCap { count, cap })
    } else {
        Ok(())
    }
}

/// Block owned by each face (`None` for faces without an animated block),
/// from the chunk section's `face_index` and each rect's `block`.
pub(crate) fn face_blocks(
    chunk_section: &AnimatedLightChunksSection,
    weight_maps: &AnimatedLightWeightMapsSection,
    face_count: usize,
) -> Result<Vec<Option<u32>>, AnimatedBlockGuardError> {
    assert_eq!(
        chunk_section.chunks.len(),
        weight_maps.chunk_rects.len(),
        "weight-map rects must pair 1:1 with animated chunks",
    );
    let mut blocks = vec![None; face_count];
    for (chunk, rect) in chunk_section.chunks.iter().zip(&weight_maps.chunk_rects) {
        let face = chunk.face_index as usize;
        match blocks[face] {
            None => blocks[face] = Some(rect.block),
            Some(existing) if existing != rect.block => {
                return Err(AnimatedBlockGuardError::FaceSpansBlocks {
                    face,
                    block_a: existing,
                    block_b: rect.block,
                });
            }
            Some(_) => {}
        }
    }
    Ok(blocks)
}

/// Enforce what the compact repack relies on, failing the build otherwise:
/// the block count fits the shared cap, no vertex of a block-owning face is
/// shared with any other face, and every bilinear footprint of a block-owning
/// face stays inside that face's chart placement, in its lightmap cell block's
/// texels.
pub(crate) fn validate_block_guards(
    geometry: &GeometrySection,
    face_index_ranges: &[FaceIndexRange],
    weight_maps: &AnimatedLightWeightMapsSection,
    face_blocks: &[Option<u32>],
    layout: &BlockLayout,
) -> Result<(), AnimatedBlockGuardError> {
    check_block_cap(weight_maps.blocks.len(), ANIMATED_BLOCK_CAP)?;

    // A vertex referenced by a block-owning face must be referenced by no
    // other face: the flat per-vertex block id would leak into it.
    let mut owner = vec![u32::MAX; geometry.vertices.len()];
    for (face, range) in face_index_ranges.iter().enumerate() {
        for &vertex in face_indices(geometry, range) {
            let slot = &mut owner[vertex as usize];
            if *slot == u32::MAX {
                *slot = face as u32;
            } else if *slot != face as u32 {
                let other = *slot as usize;
                if face_blocks[face].is_some() || face_blocks[other].is_some() {
                    let block_face = if face_blocks[face].is_some() {
                        face
                    } else {
                        other
                    };
                    return Err(AnimatedBlockGuardError::SharedVertex {
                        vertex,
                        face_a: other,
                        face_b: face,
                        block_face,
                    });
                }
            }
        }
    }

    for (face, block) in face_blocks.iter().enumerate() {
        let Some(block) = block else { continue };
        let block = &weight_maps.blocks[*block as usize];
        for &vertex in face_indices(geometry, &face_index_ranges[face]) {
            let cell_block = &layout.blocks[block.lightmap_block as usize];
            check_vertex_footprint(
                geometry,
                face,
                vertex,
                block,
                (cell_block.width, cell_block.height),
            )?;
        }
    }
    Ok(())
}

fn face_indices<'a>(geometry: &'a GeometrySection, range: &FaceIndexRange) -> &'a [u32] {
    let start = range.index_offset as usize;
    &geometry.indices[start..start + range.index_count as usize]
}

/// The runtime's bilinear footprint at a vertex: the forward shader samples
/// at `q / 65535 × extent` in the vertex's cell block, reading texels
/// `floor(t − 0.5)` and the next one. Interior fragments interpolate between
/// vertices, so checking every vertex bounds the whole face.
fn check_vertex_footprint(
    geometry: &GeometrySection,
    face: usize,
    vertex: u32,
    block: &AnimatedBlock,
    (block_width, block_height): (u32, u32),
) -> Result<(), AnimatedBlockGuardError> {
    let v = &geometry.vertices[vertex as usize];
    if u32::from(v.lightmap_block) != block.lightmap_block + 1 {
        return Err(AnimatedBlockGuardError::VertexBlockMismatch {
            face,
            vertex,
            vertex_block: u32::from(v.lightmap_block),
            block: block.lightmap_block,
        });
    }
    let first_texel = |quantized: u16, extent: u32| -> i64 {
        let t = f32::from(quantized) / 65535.0 * extent as f32;
        (t - 0.5).floor() as i64
    };
    let (x0, y0) = (
        first_texel(v.lightmap_uv[0], block_width),
        first_texel(v.lightmap_uv[1], block_height),
    );
    let (x1, y1) = (x0 + 1, y0 + 1);
    let (bx, by) = (i64::from(block.block_x), i64::from(block.block_y));
    let inside = x0 >= bx
        && y0 >= by
        && x1 < bx + i64::from(block.width)
        && y1 < by + i64::from(block.height);
    if inside {
        Ok(())
    } else {
        Err(AnimatedBlockGuardError::FootprintOutsidePlacement {
            face,
            vertex,
            lightmap_block: block.lightmap_block,
            footprint: [x0, y0, x1, y1],
            placement: [
                u32::from(block.block_x),
                u32::from(block.block_y),
                block.width,
                block.height,
            ],
        })
    }
}

/// Write each face's block id into its vertices: 0 means none, `n` names
/// block `n − 1`. Runs after the SDF atlas key hashes the geometry, so an
/// animated-light edit that moves blocks never re-bakes the SDF atlas.
pub(crate) fn stamp_animated_block_ids(
    geometry: &mut GeometrySection,
    face_index_ranges: &[FaceIndexRange],
    face_blocks: &[Option<u32>],
) {
    for vertex in &mut geometry.vertices {
        vertex.animated_block = 0;
    }
    for (face, block) in face_blocks.iter().enumerate() {
        let Some(block) = block else { continue };
        let id = u16::try_from(block + 1).expect("block cap keeps ids within u16");
        let range = face_index_ranges[face];
        let start = range.index_offset as usize;
        for index in start..start + range.index_count as usize {
            let vertex = geometry.indices[index] as usize;
            geometry.vertices[vertex].animated_block = id;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::animated_light_weight_maps::{ChunkAtlasRect, TexelLightEntry};
    use postretro_level_format::geometry::{FaceMeta, Vertex};

    use crate::lightmap_bake::CellBlock;

    /// Edge of both lightmap cell blocks.
    const EDGE: u32 = 64;

    /// Two `EDGE²` cell blocks; face 0 lives in block 1, face 1 in block 0.
    fn cell_blocks() -> BlockLayout {
        let block = |cell_id, layer| CellBlock {
            cell_id,
            width: EDGE,
            height: EDGE,
            layer,
            x: 0,
            y: 0,
        };
        BlockLayout {
            direction_texel_scale: 2,
            blocks: vec![block(0, 0), block(1, 1)],
            chart_blocks: vec![1, 0],
        }
    }

    fn vertex(lm: [f32; 2], block_slot: u16) -> Vertex {
        Vertex::new(
            [0.0; 3],
            [0.0; 2],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            lm,
            block_slot,
        )
    }

    /// Two quads (faces 0 and 1), four unshared vertices each. Face 0's
    /// block-local lightmap UVs cover texels [10, 20) × [10, 20) of lightmap
    /// block 1 (vertex slot 2).
    fn two_faces() -> (GeometrySection, Vec<FaceIndexRange>) {
        let uv = |t: f32| t / EDGE as f32;
        let mut vertices = Vec::new();
        for &(x0, x1, slot) in &[(10.0, 20.0, 2_u16), (40.0, 50.0, 1_u16)] {
            for &(u, v) in &[(x0, x0), (x1, x0), (x1, x1), (x0, x1)] {
                vertices.push(vertex([uv(u), uv(v)], slot));
            }
        }
        let indices = vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7];
        let faces = vec![
            FaceMeta {
                leaf_index: 0,
                texture_index: 0,
            },
            FaceMeta {
                leaf_index: 1,
                texture_index: 0,
            },
        ];
        (
            GeometrySection {
                vertices,
                indices,
                faces,
            },
            vec![
                FaceIndexRange {
                    index_offset: 0,
                    index_count: 6,
                },
                FaceIndexRange {
                    index_offset: 6,
                    index_count: 6,
                },
            ],
        )
    }

    /// One block around face 0: its chart placement is the interior
    /// [10, 20)² plus a 2-texel gutter, in lightmap block 1's texels.
    fn one_block(x: u16, width: u32) -> AnimatedLightWeightMapsSection {
        AnimatedLightWeightMapsSection {
            page_size: EDGE,
            compact_layers: 1,
            blocks: vec![AnimatedBlock {
                lightmap_block: 1,
                block_x: x,
                block_y: 8,
                compact_x: u32::from(x),
                compact_y: 8,
                compact_layer: 0,
                width,
                height: 14,
            }],
            chunk_rects: vec![ChunkAtlasRect {
                compact_x: 10,
                compact_y: 10,
                width: 1,
                height: 1,
                texel_offset: 0,
                block: 0,
            }],
            offset_counts: vec![TexelLightEntry {
                offset: 0,
                count: 0,
            }],
            texel_lights: Vec::new(),
        }
    }

    #[test]
    fn footprint_just_inside_the_placement_passes() {
        let (geometry, ranges) = two_faces();
        // Vertex texel coords span [10, 20]; footprints reach texels 9..=20.
        let section = one_block(9, 12);
        assert_eq!(
            validate_block_guards(
                &geometry,
                &ranges,
                &section,
                &[Some(0), None],
                &cell_blocks()
            ),
            Ok(())
        );
    }

    #[test]
    fn footprint_past_the_placement_fails_the_build() {
        let (geometry, ranges) = two_faces();
        let section = one_block(10, 11);
        let error = validate_block_guards(
            &geometry,
            &ranges,
            &section,
            &[Some(0), None],
            &cell_blocks(),
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                AnimatedBlockGuardError::FootprintOutsidePlacement { face: 0, .. }
            ),
            "{error}"
        );
    }

    #[test]
    fn footprint_resolves_against_the_cell_block_extent_not_a_square_layer() {
        let (geometry, ranges) = two_faces();
        let mut layout = cell_blocks();
        // The same quantized UVs over a block twice as wide land at texels
        // [20, 40]; the placement at x 9..21 no longer holds them.
        layout.blocks[1].width = 2 * EDGE;
        let error = validate_block_guards(
            &geometry,
            &ranges,
            &one_block(9, 12),
            &[Some(0), None],
            &layout,
        )
        .unwrap_err();
        assert!(
            matches!(
                error,
                AnimatedBlockGuardError::FootprintOutsidePlacement {
                    face: 0,
                    lightmap_block: 1,
                    ..
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn vertex_naming_another_lightmap_block_fails_the_build() {
        let (mut geometry, ranges) = two_faces();
        geometry.vertices[0].lightmap_block = 1;
        let error = validate_block_guards(
            &geometry,
            &ranges,
            &one_block(9, 12),
            &[Some(0), None],
            &cell_blocks(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            AnimatedBlockGuardError::VertexBlockMismatch {
                face: 0,
                vertex: 0,
                vertex_block: 1,
                block: 1,
            }
        );
    }

    #[test]
    fn a_vertex_shared_with_a_block_face_fails_the_build() {
        let (mut geometry, ranges) = two_faces();
        // Face 1's first triangle reuses face 0's vertex 2.
        geometry.indices[6] = 2;
        let section = one_block(9, 12);
        let error = validate_block_guards(
            &geometry,
            &ranges,
            &section,
            &[Some(0), None],
            &cell_blocks(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            AnimatedBlockGuardError::SharedVertex {
                vertex: 2,
                face_a: 0,
                face_b: 1,
                block_face: 0,
            }
        );
    }

    #[test]
    fn vertices_shared_only_between_block_less_faces_pass() {
        let (mut geometry, ranges) = two_faces();
        geometry.indices[6] = 2;
        let section = AnimatedLightWeightMapsSection::empty();
        assert_eq!(
            validate_block_guards(&geometry, &ranges, &section, &[None, None], &cell_blocks()),
            Ok(())
        );
    }

    #[test]
    fn block_count_over_the_cap_fails_naming_cap_and_count_and_at_the_cap_passes() {
        assert_eq!(
            check_block_cap(ANIMATED_BLOCK_CAP as usize, ANIMATED_BLOCK_CAP),
            Ok(())
        );
        let error =
            check_block_cap(ANIMATED_BLOCK_CAP as usize + 1, ANIMATED_BLOCK_CAP).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains(&ANIMATED_BLOCK_CAP.to_string()),
            "{message}"
        );
        assert!(
            message.contains(&(ANIMATED_BLOCK_CAP + 1).to_string()),
            "{message}"
        );
    }

    #[test]
    fn stamping_names_block_n_as_n_plus_one_and_leaves_other_faces_zero() {
        let (mut geometry, ranges) = two_faces();
        geometry.vertices[5].animated_block = 9; // stale value is cleared
        stamp_animated_block_ids(&mut geometry, &ranges, &[Some(4), None]);
        let ids: Vec<u16> = geometry.vertices.iter().map(|v| v.animated_block).collect();
        assert_eq!(ids, [5, 5, 5, 5, 0, 0, 0, 0]);
    }
}
