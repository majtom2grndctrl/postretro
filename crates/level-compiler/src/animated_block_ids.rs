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
        "face {face} vertex {vertex}: bilinear footprint texels {footprint:?} on static layer \
         {layer} leave the face's chart placement {placement:?}; the compact repack would \
         sample another block"
    )]
    FootprintOutsidePlacement {
        face: usize,
        vertex: u32,
        layer: u32,
        /// `[x0, y0, x1, y1]`, inclusive texels.
        footprint: [i64; 4],
        /// `[x, y, width, height]`.
        placement: [u32; 4],
    },
    #[error(
        "face {face} vertex {vertex} samples static layer {vertex_layer} but its animated \
         block sits on layer {block_layer}"
    )]
    VertexLayerMismatch {
        face: usize,
        vertex: u32,
        vertex_layer: u32,
        block_layer: u32,
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
/// face stays inside that face's chart placement on the block's static layer.
pub(crate) fn validate_block_guards(
    geometry: &GeometrySection,
    face_index_ranges: &[FaceIndexRange],
    weight_maps: &AnimatedLightWeightMapsSection,
    face_blocks: &[Option<u32>],
    static_layer_size: u32,
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
            check_vertex_footprint(geometry, face, vertex, block, static_layer_size)?;
        }
    }
    Ok(())
}

fn face_indices<'a>(geometry: &'a GeometrySection, range: &FaceIndexRange) -> &'a [u32] {
    let start = range.index_offset as usize;
    &geometry.indices[start..start + range.index_count as usize]
}

/// The runtime's bilinear footprint at a vertex: the forward shader samples
/// at `q / 65535 × size`, reading texels `floor(t − 0.5)` and the next one.
/// Interior fragments interpolate between vertices, so checking every vertex
/// bounds the whole face.
fn check_vertex_footprint(
    geometry: &GeometrySection,
    face: usize,
    vertex: u32,
    block: &AnimatedBlock,
    static_layer_size: u32,
) -> Result<(), AnimatedBlockGuardError> {
    let v = &geometry.vertices[vertex as usize];
    if u32::from(v.lightmap_layer) != block.static_layer {
        return Err(AnimatedBlockGuardError::VertexLayerMismatch {
            face,
            vertex,
            vertex_layer: u32::from(v.lightmap_layer),
            block_layer: block.static_layer,
        });
    }
    let first_texel = |quantized: u16| -> i64 {
        let t = f32::from(quantized) / 65535.0 * static_layer_size as f32;
        (t - 0.5).floor() as i64
    };
    let (x0, y0) = (first_texel(v.lightmap_uv[0]), first_texel(v.lightmap_uv[1]));
    let (x1, y1) = (x0 + 1, y0 + 1);
    let inside = x0 >= i64::from(block.static_x)
        && y0 >= i64::from(block.static_y)
        && x1 < i64::from(block.static_x) + i64::from(block.width)
        && y1 < i64::from(block.static_y) + i64::from(block.height);
    if inside {
        Ok(())
    } else {
        Err(AnimatedBlockGuardError::FootprintOutsidePlacement {
            face,
            vertex,
            layer: block.static_layer,
            footprint: [x0, y0, x1, y1],
            placement: [block.static_x, block.static_y, block.width, block.height],
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

    const STATIC: u32 = 64;

    fn vertex(lm: [f32; 2], layer: u16) -> Vertex {
        Vertex::new(
            [0.0; 3],
            [0.0; 2],
            [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0],
            true,
            lm,
            layer,
        )
    }

    /// Two quads (faces 0 and 1), four unshared vertices each. Face 0's
    /// lightmap UVs cover texels [10, 20) × [10, 20) on layer 1.
    fn two_faces() -> (GeometrySection, Vec<FaceIndexRange>) {
        let uv = |t: f32| t / STATIC as f32;
        let mut vertices = Vec::new();
        for &(x0, x1, layer) in &[(10.0, 20.0, 1_u16), (40.0, 50.0, 0_u16)] {
            for &(u, v) in &[(x0, x0), (x1, x0), (x1, x1), (x0, x1)] {
                vertices.push(vertex([uv(u), uv(v)], layer));
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
    /// [10, 20)² plus a 2-texel gutter.
    fn one_block(x: u32, width: u32) -> AnimatedLightWeightMapsSection {
        AnimatedLightWeightMapsSection {
            page_size: STATIC,
            compact_layers: 1,
            blocks: vec![AnimatedBlock {
                static_layer: 1,
                static_x: x,
                static_y: 8,
                compact_x: x,
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
            validate_block_guards(&geometry, &ranges, &section, &[Some(0), None], STATIC),
            Ok(())
        );
    }

    #[test]
    fn footprint_past_the_placement_fails_the_build() {
        let (geometry, ranges) = two_faces();
        let section = one_block(10, 11);
        let error = validate_block_guards(&geometry, &ranges, &section, &[Some(0), None], STATIC)
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
    fn a_vertex_shared_with_a_block_face_fails_the_build() {
        let (mut geometry, ranges) = two_faces();
        // Face 1's first triangle reuses face 0's vertex 2.
        geometry.indices[6] = 2;
        let section = one_block(9, 12);
        let error = validate_block_guards(&geometry, &ranges, &section, &[Some(0), None], STATIC)
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
            validate_block_guards(&geometry, &ranges, &section, &[None, None], STATIC),
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
