//! The mover light gate's probe and animated passes, and the merge of all three tiers' rows.
//! See: context/lib/build_pipeline.md §Compiler pipeline (step 12)

use std::sync::Arc;
use std::time::Instant;

use postretro_level_format::animated_light_chunks::AnimatedLightChunksSection;
use postretro_level_format::animated_light_weight_maps::AnimatedLightWeightMapsSection;

use crate::bake_control::BakeControl;
use crate::governor::Governor;
use crate::light_namespaces::AnimatedBakedLights;
use crate::lightmap_bake::Chart;
use crate::mover_light_gate::{
    AnimatedScene, GateLights, GateTally, LightProbes, MoverPoses, ProbeScene, gate_animated,
    gate_probes,
};
use crate::reporter::StageProgress;
use crate::sh_bake::{self, ShBakeCtx, ShConfig};

/// Gate the analysed lights' probes over the SH probe grid, as the SH
/// family lays it out and traces it: before atlas preparation, on the
/// pre-cut world. `None` when no mover is analysed.
pub(super) fn probe_gate(
    poses: &MoverPoses,
    lights: &GateLights<'_>,
    sh_ctx: &ShBakeCtx<'_>,
    sh_config: &ShConfig,
    governor: &Arc<Governor>,
) -> Option<Vec<LightProbes>> {
    if poses.analysed.is_empty() {
        return None;
    }
    let started = Instant::now();
    let layout = sh_bake::probe_grid_layout(sh_ctx, sh_config);
    let control = BakeControl::new(Arc::clone(governor), &StageProgress::indeterminate());
    let probes = gate_probes(
        poses,
        &lights.analysed,
        &ProbeScene {
            layout: &layout,
            bvh: sh_ctx.bvh,
            primitives: sh_ctx.primitives,
            geometry: sh_ctx.geometry,
        },
        &control,
    );
    log::debug!(
        "[MoverLight] probe gate: {:.1} ms over {} probes",
        started.elapsed().as_secs_f64() * 1000.0,
        layout.total_probes(),
    );
    Some(probes)
}

/// The finished animated sections the animated gate reads.
pub(super) struct AnimatedInputs<'a> {
    pub(super) lights: &'a AnimatedBakedLights<'a>,
    pub(super) chunks: Option<&'a AnimatedLightChunksSection>,
    pub(super) weight_maps: Option<&'a AnimatedLightWeightMapsSection>,
    pub(super) face_charts: &'a [Chart],
}

/// Gate the animated lights from their finished id-25 weights, then fold
/// the static, animated and probe rows into one tally. The fold order is
/// immaterial: rows key on (mover, terminus, light) and their counts add.
pub(super) fn merge_rows(
    poses: &MoverPoses,
    static_rows: Option<GateTally>,
    probes: Option<Vec<LightProbes>>,
    animated: AnimatedInputs<'_>,
    governor: &Arc<Governor>,
) -> GateTally {
    let mut tally = static_rows.unwrap_or_default();
    if let (Some(chunks), Some(weight_maps)) = (animated.chunks, animated.weight_maps)
        && !poses.analysed.is_empty()
    {
        let started = Instant::now();
        let control = BakeControl::new(Arc::clone(governor), &StageProgress::indeterminate());
        let lights = gate_animated(
            poses,
            animated.lights,
            &AnimatedScene {
                chunks,
                weight_maps,
                face_charts: animated.face_charts,
            },
            &control,
        );
        log::debug!(
            "[MoverLight] animated gate: {:.1} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
        for light in &lights {
            tally.add_partition(poses, light.source_index, &light.movers);
        }
    }
    for light in probes.iter().flatten() {
        tally.add_probes(poses, light.source_index, &light.movers);
    }
    tally
}
