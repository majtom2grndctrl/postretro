// One static light's patch direct: its sparse layer of bounced outgoing radiance.
// See: context/lib/build_pipeline.md §Compiler pipeline

use glam::Vec3;
use rayon::prelude::*;

use super::patches::{PatchSet, Side};
use crate::bake_control::BakeControl;
use crate::lightmap_bake::{DEFAULT_AREA_SAMPLE_COUNT, soft_visibility_early_out};
use crate::map_data::MapLight;
use crate::sh_bake::{
    RaytracingCtx, bounce_contribution, reflected_radiance, segment_clear, soft_visibility_seed,
};
use crate::texel_footprint::ProbeEarlyOut;

/// Patches one governed work item shades. Chunks split the patch list in a
/// fixed way, so the layer does not depend on which worker ran which chunk.
pub(super) const PATCHES_PER_CHUNK: usize = 256;

/// Progress units one light's layer advances: one per chunk.
pub(super) fn layer_progress_units(patch_count: usize) -> usize {
    patch_count.div_ceil(PATCHES_PER_CHUNK)
}

/// One lit side of one patch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct LayerEntry {
    pub(crate) patch: u32,
    pub(crate) side: Side,
    /// Outgoing radiance: the light's colour and intensity, its Lambert
    /// term and visibility at the patch, and the patch's albedo over π.
    pub(crate) radiance: [f32; 3],
}

/// One light's patch direct over the patches in its reach, ascending by
/// (patch, side). A side the light does not reach has no entry.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SparseLayer {
    pub(crate) entries: Vec<LayerEntry>,
}

/// Shade `light`'s layer: at each patch centroid, for each side whose
/// normal the light reaches, the bounce's own Lambert term
/// ([`bounce_contribution`]: range, cone and `n·l` early-outs) times soft
/// visibility times the patch's albedo over π, the same terms the per-hit
/// estimator takes at a gather hit. An `embedded` light (its emitter sphere
/// reaches into geometry) traces its full sample set with no early-out;
/// every other light takes the probe-and-escalate set. The visibility seed
/// mixes the patch's seed id, the side and `light_axis`. Each chunk takes a
/// governor permit and advances `control`'s progress by one.
pub(crate) fn shade_light_layer(
    ctx: &RaytracingCtx<'_>,
    patches: &PatchSet,
    light: &MapLight,
    light_axis: u64,
    embedded: bool,
    control: &BakeControl,
) -> SparseLayer {
    let early_out = ProbeEarlyOut::for_probe_point(embedded);
    let trace = |from, to| segment_clear(ctx, from, to);
    let chunks: Vec<Vec<LayerEntry>> = patches
        .patches()
        .par_chunks(PATCHES_PER_CHUNK)
        .enumerate()
        .map(|(chunk, slice)| {
            let _permit = control.governor().enter();
            let mut entries = Vec::new();
            for (offset, patch) in slice.iter().enumerate() {
                let index = (chunk * PATCHES_PER_CHUNK + offset) as u32;
                for side in Side::BOTH {
                    let normal = side.normal(patch.normal);
                    let Some(contribution) = bounce_contribution(light, patch.centroid, normal)
                    else {
                        continue;
                    };
                    let seed = soft_visibility_seed(patch.seed_id(), side as u64, light_axis);
                    let visibility = soft_visibility_early_out(
                        patch.centroid,
                        normal,
                        light,
                        seed,
                        DEFAULT_AREA_SAMPLE_COUNT,
                        early_out,
                        trace,
                    );
                    if visibility <= 0.0 {
                        continue;
                    }
                    let radiance: Vec3 = reflected_radiance(
                        ctx.bounce_materials,
                        patch.texture_index,
                        contribution * visibility,
                    );
                    entries.push(LayerEntry {
                        patch: index,
                        side,
                        radiance: radiance.to_array(),
                    });
                }
            }
            control.advance(1);
            entries
        })
        .collect();
    // Exact, so a layer holds no growth slack beside its priced entries.
    let mut entries = Vec::with_capacity(chunks.iter().map(Vec::len).sum());
    for chunk in chunks {
        entries.extend(chunk);
    }
    SparseLayer { entries }
}
