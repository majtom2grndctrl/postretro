// Reflection-probe animated direct: each animated light's weight-map texels at a capture hit.
// See: context/lib/rendering_pipeline.md §7.1 step 4 · shaders animated_lightmap_compose.wgsl, lightmap_sample.wgsl

use glam::Vec3;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use postretro_level_format::lightmap::LightmapSection;

use super::shading::AnimatedTerms;

/// Unit-radiance animated direct from id 25 as the level ships it (unlit
/// chunks culled, survivors repacked into compact pages). The compose pass
/// writes `Σ_l scale_l × weight_l` per compact texel; the forward pass reads
/// it bilinearly (clamp to edge) at the hit's static block-local texel moved
/// by its animated block's offset (`animated_block_uv`). With the shading
/// normal at the face normal its bumped-Lambert factor is 1, so each light's
/// unit term is that bilinear read over its own weights. Texels outside every
/// chunk are the atlas's zero gutter.
pub(crate) struct AnimatedDirectSampler<'a> {
    section: &'a AnimatedLightWeightMapsSection,
    lightmap: &'a LightmapSection,
    /// Chunk indices of each animated block.
    block_chunks: Vec<Vec<u32>>,
}

impl<'a> AnimatedDirectSampler<'a> {
    pub(crate) fn new(
        section: &'a AnimatedLightWeightMapsSection,
        lightmap: &'a LightmapSection,
    ) -> Self {
        let mut block_chunks = vec![Vec::new(); section.blocks.len()];
        for (index, chunk) in section.chunk_rects.iter().enumerate() {
            if let Some(list) = block_chunks.get_mut(chunk.block as usize) {
                list.push(index as u32);
            }
        }
        Self {
            section,
            lightmap,
            block_chunks,
        }
    }

    /// Add each animated light's direct at block-local `lightmap_uv` of a
    /// face with animated block `animated_block` (vertex value: 0 none, n is
    /// block n − 1) and id-22 block `lightmap_block` (+1), times `scale`.
    pub(crate) fn accumulate(
        &self,
        animated_block: u16,
        lightmap_block: u16,
        lightmap_uv: [f32; 2],
        scale: Vec3,
        out: &mut AnimatedTerms,
    ) {
        let Some(block_index) = usize::from(animated_block).checked_sub(1) else {
            return;
        };
        let Some(block) = self.section.blocks.get(block_index) else {
            return;
        };
        // `resolve_lightmap_block`: block-local texel = uv × the id-22 extent.
        let Some(static_block) = usize::from(lightmap_block)
            .checked_sub(1)
            .and_then(|id| self.lightmap.blocks.get(id))
        else {
            return;
        };
        let texel = [
            lightmap_uv[0] * f32::from(static_block.width),
            lightmap_uv[1] * f32::from(static_block.height),
        ];
        // Compact − block-local, the block table's integer offset.
        let offset = [
            block.compact_x as f32 - f32::from(block.block_x),
            block.compact_y as f32 - f32::from(block.block_y),
        ];
        let last = self.section.page_size.saturating_sub(1) as i64;
        let mut taps = [[0i64; 2]; 2];
        let mut fraction = [0.0f32; 2];
        for axis in 0..2 {
            let p = texel[axis] + offset[axis] - 0.5;
            let lo = p.floor();
            taps[axis] = [(lo as i64).clamp(0, last), (lo as i64 + 1).clamp(0, last)];
            fraction[axis] = p - lo;
        }
        let [fx, fy] = fraction;
        for (x, y, weight) in [
            (taps[0][0], taps[1][0], (1.0 - fx) * (1.0 - fy)),
            (taps[0][1], taps[1][0], fx * (1.0 - fy)),
            (taps[0][0], taps[1][1], (1.0 - fx) * fy),
            (taps[0][1], taps[1][1], fx * fy),
        ] {
            if weight > 0.0 {
                self.add_texel(block_index, x as u32, y as u32, scale * weight, out);
            }
        }
    }

    /// One compact texel's per-light weights. Chunks lie inside their block
    /// and the block rect includes its chart's padding gutter, so a hit's
    /// bilinear footprint stays on its own block's chunks or zero gutter.
    fn add_texel(&self, block: usize, x: u32, y: u32, scale: Vec3, out: &mut AnimatedTerms) {
        let section = self.section;
        for &chunk_index in &self.block_chunks[block] {
            let chunk = &section.chunk_rects[chunk_index as usize];
            if x < chunk.compact_x
                || y < chunk.compact_y
                || x >= chunk.compact_x + chunk.width
                || y >= chunk.compact_y + chunk.height
            {
                continue;
            }
            let record =
                chunk.texel_offset + (y - chunk.compact_y) * chunk.width + (x - chunk.compact_x);
            let Some(entry) = section.offset_counts.get(record as usize) else {
                return;
            };
            let range = entry.offset as usize..(entry.offset + entry.count) as usize;
            for light in section.texel_lights.get(range).unwrap_or_default() {
                out.add(light.light_index, scale * light.weight);
            }
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::animated_light_weight_maps::{
        AnimatedBlock, ChunkAtlasRect, TexelLight, TexelLightEntry,
    };
    use postretro_level_format::lightmap::{
        IRRADIANCE_FORMAT_RGBA16F, LightmapBlock, LightmapMode,
    };

    /// One 8×8 static block whose chart (4×4 at block texel (2, 2)) packs to
    /// compact (10, 20) on a 32² page. Light 5 weighs `x + 1` at chart column
    /// x; light 2 lights only the chart's left column at 1.
    fn fixture() -> (AnimatedLightWeightMapsSection, LightmapSection) {
        let mut offset_counts = Vec::new();
        let mut texel_lights = Vec::new();
        for _y in 0..4 {
            for x in 0..4 {
                let start = texel_lights.len() as u32;
                texel_lights.push(TexelLight {
                    light_index: 5,
                    weight: x as f32 + 1.0,
                    direction_oct: [0, 0],
                });
                if x == 0 {
                    texel_lights.push(TexelLight {
                        light_index: 2,
                        weight: 1.0,
                        direction_oct: [0, 0],
                    });
                }
                offset_counts.push(TexelLightEntry {
                    offset: start,
                    count: texel_lights.len() as u32 - start,
                });
            }
        }
        let section = AnimatedLightWeightMapsSection {
            page_size: 32,
            compact_layers: 1,
            blocks: vec![AnimatedBlock {
                lightmap_block: 0,
                block_x: 1,
                block_y: 1,
                compact_x: 9,
                compact_y: 19,
                compact_layer: 0,
                width: 6,
                height: 6,
            }],
            chunk_rects: vec![ChunkAtlasRect {
                compact_x: 10,
                compact_y: 20,
                width: 4,
                height: 4,
                texel_offset: 0,
                block: 0,
            }],
            offset_counts,
            texel_lights,
        };
        let lightmap = LightmapSection {
            direction_texel_scale: 1,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            mode: LightmapMode::Shadowed,
            blocks: vec![LightmapBlock {
                cell_id: 0,
                width: 8,
                height: 8,
                irradiance: vec![0; 8 * 8 * 8],
                direction: Vec::new(),
            }],
        };
        (section, lightmap)
    }

    fn read(sampler: &AnimatedDirectSampler<'_>, texel: [f32; 2]) -> Vec<(u32, Vec3)> {
        let mut out = AnimatedTerms::default();
        sampler.accumulate(1, 1, [texel[0] / 8.0, texel[1] / 8.0], Vec3::ONE, &mut out);
        let mut lights = out.lights().to_vec();
        lights.sort_by_key(|l| l.0);
        lights
    }

    #[test]
    fn weights_read_bilinearly_at_the_compact_texel_of_the_hit() {
        let (section, lightmap) = fixture();
        let sampler = AnimatedDirectSampler::new(&section, &lightmap);
        // Chart column 1's centre (block texel 3.5): light 5 weighs 2.
        let centre = read(&sampler, [3.5, 3.5]);
        assert_eq!(centre.len(), 1);
        assert_eq!(centre[0].0, 5);
        assert!(
            (centre[0].1 - Vec3::splat(2.0)).length() < 1e-5,
            "{centre:?}"
        );
        // Halfway between columns 0 and 1: light 5 at 1.5, light 2 at 0.5.
        let between = read(&sampler, [3.0, 3.5]);
        assert_eq!(between[0].0, 2);
        assert!((between[0].1.x - 0.5).abs() < 1e-5, "{between:?}");
        assert!((between[1].1.x - 1.5).abs() < 1e-5, "{between:?}");
        // On the chart's left edge the gutter column reads zero: half weight.
        let edge = read(&sampler, [2.0, 3.5]);
        assert!((edge[0].1.x - 0.5).abs() < 1e-5, "{edge:?}");
        assert!((edge[1].1.x - 0.5).abs() < 1e-5, "{edge:?}");
    }

    #[test]
    fn a_face_without_an_animated_block_reads_nothing() {
        let (section, lightmap) = fixture();
        let sampler = AnimatedDirectSampler::new(&section, &lightmap);
        let mut out = AnimatedTerms::default();
        sampler.accumulate(0, 1, [0.4, 0.4], Vec3::ONE, &mut out);
        sampler.accumulate(2, 1, [0.4, 0.4], Vec3::ONE, &mut out);
        assert!(out.lights().is_empty());
    }
}
