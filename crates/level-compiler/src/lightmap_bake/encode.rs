// Lightmap section encode: BC6H/RGBA16F irradiance and reduced Rg8 direction, sliced per cell block.
// See: context/lib/build_pipeline.md §PRL section IDs

use glam::Vec3;
use postretro_level_format::lightmap::{
    DIRECTION_TEXEL_BYTES, IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F,
    IRRADIANCE_TEXEL_BYTES, LightmapBlock, LightmapMode, LightmapSection, encode_direction_oct,
    f32_to_f16_bits,
};

use super::block_layout::BlockLayout;
use super::{CompositedAtlas, MIN_ATLAS_DIMENSION};
use crate::bc6h;

/// BC texel block edge and bytes per BC6H/BC5 block.
const BC_EDGE: u32 = 4;
const BC_BLOCK_BYTES: usize = 16;

/// Assembles a v3 section one bake layer at a time. Each layer is encoded
/// whole, then every block on it is sliced out: BC6H is per 4×4 block and
/// block origins are aligned, so a slice equals encoding the block alone.
/// Only the current layer's encoded bytes and the finished blocks are held.
pub(crate) struct BlockSectionBuilder<'a> {
    layout: &'a BlockLayout,
    uncompressed_irradiance: bool,
    blocks: Vec<Option<LightmapBlock>>,
}

impl<'a> BlockSectionBuilder<'a> {
    pub(crate) fn new(layout: &'a BlockLayout, uncompressed_irradiance: bool) -> Self {
        Self {
            layout,
            uncompressed_irradiance,
            blocks: vec![None; layout.blocks.len()],
        }
    }

    /// Encode one dilated bake-layer plane and keep its blocks.
    pub(crate) fn push_layer(&mut self, layer: u32, plane: &CompositedAtlas) {
        let scale = self.layout.direction_texel_scale;
        let (irradiance, direction) =
            encode_atlas_layer(plane, self.uncompressed_irradiance, scale);
        let (irr_unit, irr_unit_bytes) = if self.uncompressed_irradiance {
            (1, IRRADIANCE_TEXEL_BYTES)
        } else {
            (BC_EDGE, BC_BLOCK_BYTES)
        };
        for (slot, block) in self.blocks.iter_mut().zip(&self.layout.blocks) {
            if block.layer != layer {
                continue;
            }
            let irradiance = copy_unit_rect(
                &irradiance,
                plane.atlas_width / irr_unit,
                irr_unit_bytes,
                [block.x, block.y, block.width, block.height].map(|v| v / irr_unit),
            );
            let direction = copy_unit_rect(
                &direction,
                plane.atlas_width / scale,
                DIRECTION_TEXEL_BYTES,
                [block.x, block.y, block.width, block.height].map(|v| v / scale),
            );
            *slot = Some(LightmapBlock {
                cell_id: block.cell_id,
                width: u16::try_from(block.width).expect("blocks fit a pool layer"),
                height: u16::try_from(block.height).expect("blocks fit a pool layer"),
                irradiance,
                direction,
            });
        }
    }

    pub(crate) fn finish(self) -> LightmapSection {
        LightmapSection {
            direction_texel_scale: self.layout.direction_texel_scale,
            irradiance_format: irradiance_format(self.uncompressed_irradiance),
            mode: LightmapMode::Shadowed,
            blocks: self
                .blocks
                .into_iter()
                .enumerate()
                .map(|(id, block)| {
                    block.unwrap_or_else(|| {
                        panic!("lightmap block {id}'s bake layer was never encoded")
                    })
                })
                .collect(),
        }
    }
}

pub(crate) fn irradiance_format(uncompressed_irradiance: bool) -> u32 {
    if uncompressed_irradiance {
        IRRADIANCE_FORMAT_RGBA16F
    } else {
        IRRADIANCE_FORMAT_BC6H
    }
}

/// Copy the `[x, y, width, height]` rect, in units, out of a row-major plane
/// `row_units` units wide at `unit_bytes` per unit. A unit is a texel for raw
/// formats and a 4×4 block for BC formats.
pub(crate) fn copy_unit_rect(
    plane: &[u8],
    row_units: u32,
    unit_bytes: usize,
    [x, y, width, height]: [u32; 4],
) -> Vec<u8> {
    let row_bytes = row_units as usize * unit_bytes;
    let rect_row_bytes = width as usize * unit_bytes;
    let mut out = Vec::with_capacity(rect_row_bytes * height as usize);
    for row in y as usize..(y + height) as usize {
        let start = row * row_bytes + x as usize * unit_bytes;
        out.extend_from_slice(&plane[start..start + rect_row_bytes]);
    }
    out
}

impl CompositedAtlas {
    /// Encode every layer of this atlas into the layout's blocks. The same
    /// per-layer encode and slice the bake uses, so an atlas equal to the
    /// bake's planes yields byte-identical sections.
    #[cfg(test)]
    pub fn encode_section(
        &self,
        layout: &BlockLayout,
        uncompressed_irradiance: bool,
    ) -> LightmapSection {
        let plane = self.atlas_width as usize * self.atlas_height as usize;
        let mut builder = BlockSectionBuilder::new(layout, uncompressed_irradiance);
        for layer in 0..self.layer_count {
            let offset = layer as usize * plane;
            let single = CompositedAtlas {
                irradiance: self.irradiance[offset * 4..(offset + plane) * 4].to_vec(),
                direction: self.direction[offset..offset + plane].to_vec(),
                coverage: self.coverage[offset..offset + plane].to_vec(),
                atlas_width: self.atlas_width,
                atlas_height: self.atlas_height,
                layer_count: 1,
            };
            builder.push_layer(layer, &single);
        }
        builder.finish()
    }
}

/// Encode one already-dilated composited atlas plane.
///
/// The whole-atlas encoder applies these same operations independently to each
/// plane. Keeping this helper plane-only makes the cold path's append order
/// explicit while preserving BC6H blocks, direction reduction, and Rg8 output.
pub(crate) fn encode_atlas_layer(
    atlas: &CompositedAtlas,
    uncompressed_irradiance: bool,
    direction_texel_scale: u32,
) -> (Vec<u8>, Vec<u8>) {
    debug_assert_eq!(atlas.layer_count, 1);

    let irradiance = if uncompressed_irradiance {
        encode_irradiance_rgba16f(&atlas.irradiance)
    } else {
        bc6h::encode_bc6h_rgb_from_f32_rgba(
            &atlas.irradiance,
            atlas.atlas_width,
            atlas.atlas_height,
        )
    };

    let direction = if direction_texel_scale == 1 {
        encode_direction_rg8(&atlas.direction, &atlas.coverage)
    } else {
        let (direction, coverage) = reduce_direction_atlas(
            &atlas.direction,
            &atlas.coverage,
            atlas.atlas_width,
            atlas.atlas_height,
            1,
            direction_texel_scale,
        );
        encode_direction_rg8(&direction, &coverage)
    };

    (irradiance, direction)
}

fn encode_irradiance_rgba16f(data: &[f32]) -> Vec<u8> {
    let texel_count = data.len() / 4;
    let mut out = Vec::with_capacity(texel_count * 8);
    for t in 0..texel_count {
        let r = f32_to_f16_bits(data[t * 4]);
        let g = f32_to_f16_bits(data[t * 4 + 1]);
        let b = f32_to_f16_bits(data[t * 4 + 2]);
        let a = f32_to_f16_bits(data[t * 4 + 3]);
        out.extend_from_slice(&r.to_le_bytes());
        out.extend_from_slice(&g.to_le_bytes());
        out.extend_from_slice(&b.to_le_bytes());
        out.extend_from_slice(&a.to_le_bytes());
    }
    out
}

/// Normalize a programmatic direction scale to a supported power of two.
///
/// The CLI rejects malformed values. This defensive path keeps direct callers
/// from producing a zero-sized direction atlas if they construct a
/// `LightmapConfig` themselves.
pub(crate) fn normalized_direction_texel_scale(scale: u32) -> u32 {
    let bounded = scale.clamp(1, MIN_ATLAS_DIMENSION);
    if bounded.is_power_of_two() {
        bounded
    } else {
        bounded.next_power_of_two()
    }
}

/// Reduce full-resolution dominant directions for the static direction atlas.
///
/// Each output layer reads only its matching input plane. Within a block the
/// loop is fixed row-major, coverage is ORed, and covered cancelling vectors
/// resolve to neutral up rather than normalizing zero.
pub(super) fn reduce_direction_atlas(
    direction: &[Vec3],
    coverage: &[bool],
    atlas_width: u32,
    atlas_height: u32,
    layer_count: u32,
    factor: u32,
) -> (Vec<Vec3>, Vec<bool>) {
    debug_assert!(factor.is_power_of_two());
    debug_assert!(factor > 0);
    debug_assert_eq!(atlas_width % factor, 0);
    debug_assert_eq!(atlas_height % factor, 0);

    let reduced_width = atlas_width / factor;
    let reduced_height = atlas_height / factor;
    let source_plane = atlas_width as usize * atlas_height as usize;
    let reduced_plane = reduced_width as usize * reduced_height as usize;
    let mut reduced_direction = Vec::with_capacity(reduced_plane * layer_count as usize);
    let mut reduced_coverage = Vec::with_capacity(reduced_plane * layer_count as usize);

    for layer in 0..layer_count as usize {
        let layer_start = layer * source_plane;
        for y in 0..reduced_height {
            for x in 0..reduced_width {
                let mut sum = Vec3::ZERO;
                let mut covered = false;
                for block_y in 0..factor {
                    for block_x in 0..factor {
                        let source_x = x * factor + block_x;
                        let source_y = y * factor + block_y;
                        let index = layer_start
                            + source_y as usize * atlas_width as usize
                            + source_x as usize;
                        if coverage[index] {
                            covered = true;
                            sum += direction[index];
                        }
                    }
                }
                reduced_coverage.push(covered);
                reduced_direction.push(if covered && sum.length_squared() > 1.0e-8 {
                    sum.normalize()
                } else {
                    Vec3::Y
                });
            }
        }
    }

    (reduced_direction, reduced_coverage)
}

pub(super) fn encode_direction_rg8(direction: &[Vec3], coverage: &[bool]) -> Vec<u8> {
    let mut out = Vec::with_capacity(direction.len() * 2);
    for (i, d) in direction.iter().enumerate() {
        let bytes = if coverage[i] {
            encode_direction_oct([d.x, d.y, d.z])
        } else {
            // Neutral up-direction: stray bilinear samples return a Lambert-valid vector.
            [128u8, 255]
        };
        out.extend_from_slice(&bytes);
    }
    out
}
