// Gating each animated baked light's lit texels from the finished id-25
// weights, at both termini of every mover it may meet.
// See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;
use rayon::prelude::*;

use super::pose::MoverPoses;
use super::rows::MoverTexels;
use super::texels::{TexelGate, light_candidates};
use crate::bake_control::BakeControl;
use crate::chart_raster::{CHART_PADDING_TEXELS, chart_texel_world_position};
use crate::light_namespaces::AnimatedBakedLights;
use crate::lightmap_bake::Chart;
use crate::lightmap_layer::chart_texel_bounds;
use crate::map_data::ShadowType;
use crate::ray_traversal::RAY_EPSILON;

/// The finished animated sections and the charts their chunks name.
pub(crate) struct AnimatedScene<'a> {
    /// Chunk `i` pairs with the weight maps' chunk rect `i`.
    pub(crate) chunks: &'a AnimatedLightChunksSection,
    pub(crate) weight_maps: &'a AnimatedLightWeightMapsSection,
    /// By face index, as `AnimatedLightChunk::face_index` names them.
    pub(crate) face_charts: &'a [Chart],
}

/// One light's gated texels, per mover with a gated texel.
#[derive(Debug, Clone)]
pub(crate) struct LightTexels {
    pub(crate) source_index: usize,
    pub(crate) movers: Vec<MoverTexels>,
}

/// Gate every animated baked light over its id-25 weights, one governed
/// work item per light. A texel is lit when its weight is above zero, and
/// is tested from the chart texel centre the weight bake anchored on,
/// lifted as that bake lifts it: the static walk's anchor and lift. Output
/// follows the namespace's order, which is source order, and holds only
/// lights with a gated texel. SDF-shadow lights own no weight, so none is
/// gated.
pub(crate) fn gate_animated(
    poses: &MoverPoses,
    lights: &AnimatedBakedLights<'_>,
    scene: &AnimatedScene<'_>,
    control: &BakeControl,
) -> Vec<LightTexels> {
    if poses.analysed.is_empty() || scene.weight_maps.chunk_rects.is_empty() {
        return Vec::new();
    }
    assert_eq!(
        scene.chunks.chunks.len(),
        scene.weight_maps.chunk_rects.len(),
        "weight-map rects pair 1:1 with animated chunks",
    );
    // Each light slot's chunks, ascending, so one chart's texels arrive
    // together.
    let mut light_chunks: Vec<Vec<usize>> = vec![Vec::new(); lights.len()];
    for (index, chunk) in scene.chunks.chunks.iter().enumerate() {
        let start = chunk.index_offset as usize;
        for &slot in &scene.chunks.light_indices[start..start + chunk.index_count as usize] {
            if let Some(chunks) = light_chunks.get_mut(slot as usize) {
                chunks.push(index);
            }
        }
    }
    lights
        .entries()
        .par_iter()
        .zip(light_chunks)
        .enumerate()
        .filter_map(|(slot, (entry, chunks))| {
            if entry.light.shadow_type == ShadowType::Sdf || chunks.is_empty() {
                return None;
            }
            let _permit = control.governor().enter();
            let candidates = light_candidates(entry.light, poses);
            if candidates.is_empty() {
                return None;
            }
            let mut gate = TexelGate::new(poses, entry.light, &candidates);
            for chunk in chunks {
                gate_chunk(&mut gate, scene, chunk, slot as u32);
            }
            let movers = gate.finish();
            (!movers.is_empty()).then_some(LightTexels {
                source_index: entry.source_index,
                movers,
            })
        })
        .collect()
}

/// Gate the texels of chunk `index` that light slot `slot` lit.
fn gate_chunk(gate: &mut TexelGate<'_>, scene: &AnimatedScene<'_>, index: usize, slot: u32) {
    let weight_maps = scene.weight_maps;
    let rect = &weight_maps.chunk_rects[index];
    let block = &weight_maps.blocks[rect.block as usize];
    let chart = &scene.face_charts[scene.chunks.chunks[index].face_index as usize];
    let Some(bounds) = chart_texel_bounds(chart) else {
        return;
    };
    if !gate.begin_chart(bounds) {
        return;
    }
    // A block spans its chart's padded placement and moves whole between
    // atlas spaces, so the rect's offset inside it, less the padding, is its
    // first texel in chart interior coordinates.
    let padding = CHART_PADDING_TEXELS as i64;
    let origin_x = (i64::from(rect.compact_x) - i64::from(block.compact_x) - padding) as i32;
    let origin_y = (i64::from(rect.compact_y) - i64::from(block.compact_y) - padding) as i32;
    let lift = chart.normal * RAY_EPSILON;
    for ry in 0..rect.height {
        for rx in 0..rect.width {
            let entry =
                &weight_maps.offset_counts[(rect.texel_offset + ry * rect.width + rx) as usize];
            let start = entry.offset as usize;
            let lit = weight_maps.texel_lights[start..start + entry.count as usize]
                .iter()
                .any(|texel| texel.light_index == slot && texel.weight > 0.0);
            if !lit {
                continue;
            }
            let centre =
                chart_texel_world_position(chart, origin_x + rx as i32, origin_y + ry as i32);
            gate.add_texel(centre, centre + lift, block.lightmap_block);
        }
    }
}
