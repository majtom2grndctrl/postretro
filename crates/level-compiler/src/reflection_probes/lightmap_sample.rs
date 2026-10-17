// Reflection-probe lightmap reads: the static direct term at a hit, from the encoded id-22 blocks.
// See: context/lib/rendering_pipeline.md §4 (Static direct) · build_pipeline.md §PRL section IDs

use glam::Vec3;
use postretro_level_format::lightmap::{
    IRRADIANCE_FORMAT_BC6H, IRRADIANCE_FORMAT_RGBA16F, LightmapBlock, LightmapSection,
};

use crate::bc6h::{Bc6hDecodeError, decode_bc6h_block, f16_bits_to_f32};

/// Static irradiance from id 22 as `lightmap_sample.wgsl` reads it: the
/// block-local texel clamped half a texel inside the block, snapped to 1/256
/// texel, then bilinear. Texels decode per read from the encoded blocks, so
/// no float atlas is ever held.
pub(crate) struct LightmapSampler<'a> {
    section: &'a LightmapSection,
}

impl<'a> LightmapSampler<'a> {
    pub(crate) fn new(section: &'a LightmapSection) -> Self {
        Self { section }
    }

    /// Irradiance at block-local `uv` of block `block_plus_one` (a vertex's
    /// `lightmap_block`). Block 0, "no lightmap", reads zero, as does an id
    /// past the section, which no compiled vertex carries.
    pub(crate) fn irradiance(
        &self,
        block_plus_one: u16,
        uv: [f32; 2],
    ) -> Result<Vec3, Bc6hDecodeError> {
        let Some(block) = block_plus_one
            .checked_sub(1)
            .and_then(|id| self.section.blocks.get(usize::from(id)))
        else {
            return Ok(Vec3::ZERO);
        };
        let extent = [f32::from(block.width), f32::from(block.height)];
        let mut taps = [[0usize; 2]; 2];
        let mut fraction = [0.0f32; 2];
        for axis in 0..2 {
            let texel = (uv[axis] * extent[axis]).clamp(0.5, extent[axis] - 0.5);
            let snapped = (texel * 256.0).round() / 256.0;
            let p = snapped - 0.5;
            let lo = p.floor();
            let last = extent[axis] as usize - 1;
            taps[axis] = [(lo as usize).min(last), (lo as usize + 1).min(last)];
            fraction[axis] = p - lo;
        }
        let mut reader = BlockReader::new(block, self.section.irradiance_format);
        let t00 = reader.texel(taps[0][0], taps[1][0])?;
        let t10 = reader.texel(taps[0][1], taps[1][0])?;
        let t01 = reader.texel(taps[0][0], taps[1][1])?;
        let t11 = reader.texel(taps[0][1], taps[1][1])?;
        let [fx, fy] = fraction;
        Ok(t00.lerp(t10, fx).lerp(t01.lerp(t11, fx), fy))
    }
}

/// Texel reads from one block, decoding each BC6H 4×4 block once per read set.
struct BlockReader<'b> {
    block: &'b LightmapBlock,
    format: u32,
    cached: Option<(usize, [[f32; 3]; 16])>,
}

impl<'b> BlockReader<'b> {
    fn new(block: &'b LightmapBlock, format: u32) -> Self {
        Self {
            block,
            format,
            cached: None,
        }
    }

    fn texel(&mut self, x: usize, y: usize) -> Result<Vec3, Bc6hDecodeError> {
        let width = usize::from(self.block.width);
        match self.format {
            IRRADIANCE_FORMAT_BC6H => {
                let index = (y / 4) * width.div_ceil(4) + x / 4;
                let texels = match self.cached {
                    Some((cached, texels)) if cached == index => texels,
                    _ => {
                        let bytes: &[u8; 16] = self.block.irradiance[index * 16..index * 16 + 16]
                            .try_into()
                            .expect("a 16-byte slice");
                        let texels = decode_bc6h_block(bytes)?;
                        self.cached = Some((index, texels));
                        texels
                    }
                };
                Ok(Vec3::from(texels[(y % 4) * 4 + x % 4]))
            }
            IRRADIANCE_FORMAT_RGBA16F => {
                let base = (y * width + x) * 8;
                let channel = |c: usize| {
                    let at = base + c * 2;
                    f16_bits_to_f32(u16::from_le_bytes([
                        self.block.irradiance[at],
                        self.block.irradiance[at + 1],
                    ]))
                };
                Ok(Vec3::new(channel(0), channel(1), channel(2)))
            }
            // The section decoder rejects every other tag.
            _ => Ok(Vec3::ZERO),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bc6h::encode_bc6h_rgb_from_f32_rgba;
    use postretro_level_format::lightmap::{LightmapMode, f32_to_f16_bits};

    fn block_from(rgba: &[f32], width: u16, height: u16, format: u32) -> LightmapBlock {
        let irradiance = match format {
            IRRADIANCE_FORMAT_BC6H => {
                encode_bc6h_rgb_from_f32_rgba(rgba, u32::from(width), u32::from(height))
            }
            _ => rgba
                .iter()
                .flat_map(|v| f32_to_f16_bits(*v).to_le_bytes())
                .collect(),
        };
        LightmapBlock {
            cell_id: 0,
            width,
            height,
            irradiance,
            direction: Vec::new(),
        }
    }

    fn section(blocks: Vec<LightmapBlock>, format: u32) -> LightmapSection {
        LightmapSection {
            direction_texel_scale: 1,
            irradiance_format: format,
            mode: LightmapMode::Shadowed,
            blocks,
        }
    }

    /// An 8×4 block whose left half is 1.0 and right half 3.0 (red channel).
    fn split_block(format: u32) -> LightmapBlock {
        let mut rgba = Vec::new();
        for _y in 0..4 {
            for x in 0..8 {
                let v = if x < 4 { 1.0 } else { 3.0 };
                rgba.extend_from_slice(&[v, 0.5, 0.25, 1.0]);
            }
        }
        block_from(&rgba, 8, 4, format)
    }

    #[test]
    fn lightmap_reads_block_texels_bilinearly_across_encoded_blocks() {
        for format in [IRRADIANCE_FORMAT_RGBA16F, IRRADIANCE_FORMAT_BC6H] {
            let lightmap = section(vec![split_block(format)], format);
            let sampler = LightmapSampler::new(&lightmap);
            // Texel centre 1.5 of 8: inside the left half.
            let left = sampler.irradiance(1, [1.5 / 8.0, 0.5]).unwrap();
            assert!((left.x - 1.0).abs() < 0.02, "{format}: {left}");
            assert!((left.y - 0.5).abs() < 0.02, "{format}: {left}");
            // On the seam between texels 3 and 4 (x = 4.0): an even blend of
            // the two encoded 4×4 blocks.
            let seam = sampler.irradiance(1, [4.0 / 8.0, 0.5]).unwrap();
            assert!((seam.x - 2.0).abs() < 0.05, "{format}: {seam}");
            // Past the block edge clamps to the last texel centre.
            let edge = sampler.irradiance(1, [1.0, 1.0]).unwrap();
            assert!((edge.x - 3.0).abs() < 0.05, "{format}: {edge}");
        }
    }

    #[test]
    fn lightmap_block_zero_reads_no_light() {
        let lightmap = section(
            vec![split_block(IRRADIANCE_FORMAT_BC6H)],
            IRRADIANCE_FORMAT_BC6H,
        );
        let sampler = LightmapSampler::new(&lightmap);
        assert_eq!(sampler.irradiance(0, [0.5, 0.5]).unwrap(), Vec3::ZERO);
    }
}
