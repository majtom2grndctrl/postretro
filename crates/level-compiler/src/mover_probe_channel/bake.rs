// The bounce-and-depth bake per (mover, terminus): posed bounce one, posed
// later bounces, posed gather, from the SH base's patches and static bounce.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::time::{Duration, Instant};

use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::bounce::{add_posed_later_bounces, light_terms, posed_bounce_one};
use super::gather::{PosedGather, PosedProbe, gather_posed_probes};
use super::posed_scene::PosedScene;
use crate::bake_control::BakeControl;
use crate::map_data::MapLight;
use crate::mover_light_gate::Terminus;
use crate::sh_bake::{ProbeGridLayout, ShBakeCtx};
use crate::surface_radiance::{LayerPlan, PatchRadiance, PatchReachIndex, PatchSet};

/// The (mover, terminus) poses one SH base bake re-bakes, and how.
#[derive(Default)]
pub(crate) struct MoverBounceRequest {
    pub(crate) scenes: Vec<PosedScene>,
    /// The PRL stores id 34 as f16, so the bar is exact equality.
    pub(crate) uncompressed: bool,
    /// Keep every valid probe's posed tile, not only affected ones.
    pub(crate) keep_every_tile: bool,
}

impl MoverBounceRequest {
    pub(crate) fn is_empty(&self) -> bool {
        self.scenes.is_empty()
    }
}

/// One pose's bounce and depth state, and what it cost.
pub(crate) struct PosedBounce {
    pub(crate) mover_id: u32,
    pub(crate) terminus: Terminus,
    pub(crate) label: String,
    /// Valid probes that differ from id 34 or whose rays meet the pose,
    /// ascending by probe index.
    pub(crate) probes: Vec<PosedProbe>,
    pub(crate) posed_patches: usize,
    pub(crate) reshaded_pairs: u64,
    /// Bounce one, later bounces, probe gather.
    pub(crate) timings: [Duration; 3],
}

impl PosedBounce {
    pub(crate) fn affected(&self) -> impl Iterator<Item = &PosedProbe> {
        self.probes.iter().filter(|probe| probe.affected())
    }

    pub(crate) fn depth(&self) -> impl Iterator<Item = (u32, &[[u16; 2]])> {
        self.probes
            .iter()
            .filter_map(|probe| probe.depth.as_deref().map(|tile| (probe.probe, tile)))
    }
}

/// What every pose of one SH base bake reads: the gate's patches, their
/// static bounce one (before later bounces), the plan's light order and
/// the gathered id 34 the bar compares against.
pub(crate) struct StaticBounce<'a> {
    pub(crate) patches: &'a PatchSet,
    pub(crate) bounce_one: &'a PatchRadiance,
    pub(crate) lights: &'a [&'a MapLight],
    pub(crate) plan: &'a LayerPlan,
    pub(crate) bounce_fill: f32,
    pub(crate) layout: &'a ProbeGridLayout,
    pub(crate) base: &'a OctahedralShVolumeSection,
}

/// Re-bake id 34 once per requested pose, each mover posed alone: posed
/// bounce one, the later bounces over every patch, and every valid probe's
/// gather. Each pose holds one posed copy of the patch radiance and the
/// later-bounce buffers while it runs, and drops them before the next.
pub(crate) fn bake_mover_bounce(
    sh_ctx: &ShBakeCtx<'_>,
    inputs: &StaticBounce<'_>,
    request: &MoverBounceRequest,
    control: &BakeControl,
) -> Vec<PosedBounce> {
    if request.is_empty() {
        return Vec::new();
    }
    let ctx = sh_ctx.ray_ctx();
    let reach = PatchReachIndex::new(inputs.patches.patches());
    let terms = light_terms(inputs.lights, inputs.plan);
    request
        .scenes
        .iter()
        .map(|scene| {
            let start = Instant::now();
            let mut radiance = posed_bounce_one(
                &ctx,
                inputs.patches,
                &reach,
                inputs.bounce_one,
                &terms,
                scene,
                control,
            );
            let bounce_one = start.elapsed();
            let start = Instant::now();
            add_posed_later_bounces(
                &ctx,
                inputs.patches,
                scene,
                &mut radiance,
                inputs.bounce_fill,
                control,
            );
            let later = start.elapsed();
            let start = Instant::now();
            let gather = PosedGather::new(inputs.patches, &radiance, scene);
            let probes = gather_posed_probes(
                sh_ctx,
                inputs.layout,
                &gather,
                scene,
                inputs.base,
                request.uncompressed,
                request.keep_every_tile,
                control,
            );
            let gathered = start.elapsed();
            PosedBounce {
                mover_id: scene.mover_id,
                terminus: scene.terminus,
                label: scene.label.clone(),
                probes,
                posed_patches: scene.patches.len(),
                reshaded_pairs: radiance.reshaded_pairs,
                timings: [bounce_one, later, gathered],
            }
        })
        .collect()
}
