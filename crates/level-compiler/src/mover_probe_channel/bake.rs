// The bounce-and-depth bake per (mover, terminus): posed bounce one, posed
// later bounces up to the mover's `light_bounces`, the SH base's own later
// bounces past them, and the posed gather, from the SH base's patches.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::time::Instant;

use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::bounce::{
    PosedLaterBounces, add_posed_later_bounces, light_terms, posed_bounce_one,
    posed_later_progress_units,
};
use super::depth::MoverAbsentDepth;
use super::gather::{PosedGather, PosedProbe, gather_posed_probes, trace_posed_depth};
use super::posed_scene::PosedScene;
use super::states::{ChangedProbe, MoverBounceStates, PoseStates, reduce_poses};
use crate::bake_control::BakeControl;
use crate::map_data::MapLight;
use crate::sh_bake::{ProbeDepthTiles, ProbeGridLayout, ShBakeCtx};
use crate::surface_radiance::{
    LATER_BOUNCE_COUNT, LayerPlan, PatchRadiance, PatchReachIndex, PatchSet,
};

/// The bounces id 34 bakes at `fill`: bounce one, and the later bounces
/// unless the fill is 0. A mover's `light_bounces` above it bakes it.
pub(crate) fn sh_base_bounce_count(fill: f32) -> u32 {
    1 + if fill == 0.0 { 0 } else { LATER_BOUNCE_COUNT }
}

/// One mover's poses to re-bake.
pub(crate) struct MoverBounceJob {
    pub(crate) mover_id: u32,
    pub(crate) label: String,
    /// Bounces re-baked posed, bounce one first, at most the SH base's
    /// count: later bounces past them are id 34's own. 0 bakes no bounce
    /// and traces only the poses' probe depth.
    pub(crate) bounces: u32,
    /// The mover posed at each terminus, in terminus order.
    pub(crate) scenes: Vec<PosedScene>,
}

impl MoverBounceJob {
    /// Index of the first of id 34's later bounces this mover keeps, when
    /// it keeps any: the later bounces past its posed ones.
    fn static_tail_from(&self, fill: f32) -> Option<u32> {
        let count = sh_base_bounce_count(fill);
        (self.bounces >= 1 && self.bounces < count).then(|| self.bounces - 1)
    }

    /// Progress units this job's bake advances over `valid_probes` valid
    /// probes and `patch_count` static patches at `fill`: per pose, one per
    /// valid probe (the posed gather, or the depth-only trace) and one per
    /// patch chunk of each posed later bounce. Posed bounce one counts none.
    /// A memo hit advances them all at once.
    pub(crate) fn progress_units(
        &self,
        valid_probes: usize,
        patch_count: usize,
        fill: f32,
    ) -> usize {
        self.scenes
            .iter()
            .map(|scene| {
                let later = if self.bounces == 0 || fill == 0.0 {
                    0
                } else {
                    posed_later_progress_units(self.bounces - 1, patch_count, scene.patches.len())
                };
                valid_probes + later
            })
            .sum()
    }
}

/// The movers one SH base bake re-bakes, and how.
#[derive(Default)]
pub(crate) struct MoverBounceRequest {
    pub(crate) jobs: Vec<MoverBounceJob>,
    /// The PRL stores id 34 as f16, so the bar is exact equality.
    pub(crate) uncompressed: bool,
    /// The full re-bake (test only; production leaves both off):
    /// keep every valid probe's posed tile, hand each pose back, and reduce
    /// no mover.
    pub(crate) keep_every_tile: bool,
    pub(crate) keep_poses: bool,
    /// The tile range mover depth may fill: once the movers baked so far
    /// carry depth at more probes, the bake stops, and the build fails by
    /// name when it reserves their tiles.
    pub(crate) depth_cap: u32,
}

impl MoverBounceRequest {
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// The first of id 34's later bounces any job keeps past its posed ones:
    /// the static later bounces the bake holds apart for them.
    pub(crate) fn static_tail_from<'j>(
        jobs: impl IntoIterator<Item = &'j MoverBounceJob>,
        fill: f32,
    ) -> Option<u32> {
        jobs.into_iter()
            .filter_map(|job| job.static_tail_from(fill))
            .min()
    }
}

/// One pose's bounce and depth state, before its mover's two poses reduce.
pub(crate) struct PosedBounce {
    /// Which pose this is, for the full re-bake.
    #[cfg(test)]
    pub(crate) mover_id: u32,
    #[cfg(test)]
    pub(crate) terminus: crate::mover_light_gate::Terminus,
    /// Valid probes that differ from id 34 or whose rays meet the pose,
    /// ascending by probe index.
    pub(crate) probes: Vec<PosedProbe>,
}

/// Id 34's later bounces from some index on, one front side per static
/// patch each, held apart for movers whose `light_bounces` stops short.
pub(crate) struct StaticTail {
    /// Later-bounce index of `layers[0]`.
    pub(crate) first: u32,
    pub(crate) layers: Vec<Vec<[f32; 3]>>,
}

impl StaticTail {
    /// Id 34's later bounces from index `from` on, in bounce order; empty
    /// when none is kept there. The pose adds them one by one, as id 34 does:
    /// pre-summing them would reassociate the float sum.
    fn layers_from(&self, from: u32) -> &[Vec<[f32; 3]>] {
        from.checked_sub(self.first)
            .and_then(|skip| self.layers.get(skip as usize..))
            .unwrap_or(&[])
    }
}

/// What every pose of one SH base bake reads: the gate's patches, their
/// static bounce one (before later bounces), the plan's light order, the
/// gathered id 34 the bar compares against and its depth tiles before mover
/// tiles reserve their range, and id 34's own later bounces past the
/// shortest posed run.
pub(crate) struct StaticBounce<'a> {
    pub(crate) patches: &'a PatchSet,
    pub(crate) bounce_one: &'a PatchRadiance,
    pub(crate) lights: &'a [&'a MapLight],
    pub(crate) plan: &'a LayerPlan,
    pub(crate) bounce_fill: f32,
    pub(crate) layout: &'a ProbeGridLayout,
    pub(crate) base: &'a OctahedralShVolumeSection,
    pub(crate) depth_tiles: &'a ProbeDepthTiles,
    pub(crate) static_tail: Option<&'a StaticTail>,
}

/// Re-bake id 34 per pose of each job, each mover posed alone, and reduce
/// each mover's two poses to its states. A pose holds one posed copy of the
/// patch radiance and the later-bounce buffers while it runs, and drops
/// them before the next; a mover holds its two poses' gathered probes until
/// it reduces them.
pub(crate) fn bake_mover_bounce(
    sh_ctx: &ShBakeCtx<'_>,
    inputs: &StaticBounce<'_>,
    jobs: &[&MoverBounceJob],
    request: &MoverBounceRequest,
    control: &BakeControl,
) -> (Vec<MoverBounceStates>, Vec<PosedBounce>) {
    if jobs.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let ctx = sh_ctx.ray_ctx();
    let reach = PatchReachIndex::new(inputs.patches.patches());
    #[cfg(test)]
    let _reach = crate::live_bytes::hold(
        "mover bounce reach index",
        inputs.patches.len() as u64 * crate::surface_radiance::REACH_INDEX_BYTES_PER_PATCH,
    );
    let terms = light_terms(inputs.lights, inputs.plan);
    let absent = MoverAbsentDepth::new(inputs.base, inputs.depth_tiles);
    let valid_probes = inputs
        .layout
        .validity
        .iter()
        .filter(|&&valid| valid != 0)
        .count();
    let mut movers: Vec<MoverBounceStates> = Vec::new();
    let mut kept_poses = Vec::new();
    let mut depth_probes = std::collections::BTreeSet::new();
    for (at, job) in jobs.iter().enumerate() {
        if depth_probes.len() > request.depth_cap as usize {
            // The build fails by name when it reserves these tiles; the
            // jobs left never run, so their progress lands at once.
            control.advance(
                jobs[at..]
                    .iter()
                    .map(|job| {
                        job.progress_units(valid_probes, inputs.patches.len(), inputs.bounce_fill)
                    })
                    .sum(),
            );
            break;
        }
        let started = Instant::now();
        let mut poses: Vec<PoseStates> = Vec::with_capacity(job.scenes.len());
        #[cfg(test)]
        let mut _held = Vec::new();
        for scene in &job.scenes {
            if job.bounces == 0 {
                let depth = trace_posed_depth(&ctx, inputs.layout, scene, control);
                poses.push(PoseStates {
                    changed: Vec::new(),
                    depth,
                });
                #[cfg(test)]
                _held.push(crate::live_bytes::hold(
                    "mover posed probes",
                    poses.last().map_or(0, PoseStates::heap_bytes),
                ));
                continue;
            }
            let posed = bake_pose(
                sh_ctx, &ctx, inputs, &reach, &terms, job, scene, request, control,
            );
            if request.keep_poses {
                kept_poses.push(posed);
                continue;
            }
            #[cfg(test)]
            let _probes = crate::live_bytes::hold(
                "mover posed probes",
                (posed.probes.capacity() * size_of::<PosedProbe>()) as u64
                    + posed.probes.iter().map(PosedProbe::heap_bytes).sum::<u64>(),
            );
            let mut states = PoseStates::default();
            for probe in posed.probes {
                let affected = probe.affected();
                if let (true, Some(tile)) = (probe.changed(), probe.tile) {
                    states.changed.push(ChangedProbe {
                        probe: probe.probe,
                        tile,
                        affected,
                    });
                }
                if let Some(depth) = probe.depth {
                    states.depth.push((probe.probe, depth));
                }
            }
            states.changed.sort_unstable_by_key(|probe| probe.probe);
            states.depth.sort_unstable_by_key(|(probe, _)| *probe);
            #[cfg(test)]
            _held.push(crate::live_bytes::hold(
                "mover posed probes",
                states.heap_bytes(),
            ));
            poses.push(states);
        }
        let Ok(poses) = <[PoseStates; 2]>::try_from(poses) else {
            continue;
        };
        let states = reduce_poses(job.mover_id, &poses, &absent);
        drop(poses);
        log::info!(
            "[MoverLight] probe channel bounce: mover `{}` (#{}) re-baked {} posed bounce(s): \
             {} affected probe(s), {} probe(s) carrying closed-pose depth, {:.1} s",
            job.label,
            job.mover_id,
            job.bounces,
            states.bounce.len(),
            states.depth.len(),
            started.elapsed().as_secs_f64(),
        );
        depth_probes.extend(states.depth.iter().map(|probe| probe.probe));
        movers.push(states);
    }
    (movers, kept_poses)
}

/// One pose's bounce: posed bounce one, the job's posed later bounces with
/// id 34's own past them, and every valid probe's posed gather.
#[allow(clippy::too_many_arguments)]
fn bake_pose(
    sh_ctx: &ShBakeCtx<'_>,
    ctx: &crate::sh_bake::RaytracingCtx<'_>,
    inputs: &StaticBounce<'_>,
    reach: &PatchReachIndex<'_>,
    terms: &[super::bounce::LightTerm<'_>],
    job: &MoverBounceJob,
    scene: &PosedScene,
    request: &MoverBounceRequest,
    control: &BakeControl,
) -> PosedBounce {
    let mut radiance = posed_bounce_one(
        ctx,
        inputs.patches,
        reach,
        inputs.bounce_one,
        terms,
        scene,
        control,
    );
    let static_tail = job
        .static_tail_from(inputs.bounce_fill)
        .zip(inputs.static_tail)
        .map_or(&[][..], |(from, tail)| tail.layers_from(from));
    add_posed_later_bounces(
        ctx,
        inputs.patches,
        scene,
        &mut radiance,
        inputs.bounce_fill,
        PosedLaterBounces {
            posed: job.bounces - 1,
            static_tail,
        },
        control,
    );
    let gather = PosedGather::new(inputs.patches, &radiance, scene);
    let probes = gather_posed_probes(
        sh_ctx,
        inputs.layout,
        &gather,
        inputs.base,
        request.uncompressed,
        request.keep_every_tile,
        control,
    );
    PosedBounce {
        #[cfg(test)]
        mover_id: job.mover_id,
        #[cfg(test)]
        terminus: scene.terminus,
        probes,
    }
}
