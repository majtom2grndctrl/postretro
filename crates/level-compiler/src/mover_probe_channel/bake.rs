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
    /// The tile range mover depth may fill: once the movers met so far,
    /// baked or held from the memo, carry depth at more probes, the bake
    /// stops, and the build fails by name when it reserves their tiles. No
    /// mover keeps more than one depth record past it
    /// ([`super::states::reduce_poses`]), and a mover past it is never
    /// memoized.
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

/// One production job in request order, as the bake meets it.
#[derive(Clone, Copy)]
pub(crate) enum MoverSlot<'a> {
    /// Its memo missed: the bake re-bakes it.
    Bake(&'a MoverBounceJob),
    /// Its memo hit: its depth probes count toward the tile range where a
    /// cold bake would have baked it.
    Held(&'a MoverBounceStates),
}

/// What [`bake_mover_bounce`] hands back.
#[derive(Default)]
pub(crate) struct MoverBounceBake {
    /// The baked slots' states, in slot order.
    pub(crate) movers: Vec<MoverBounceStates>,
    /// Each pose's re-bake, when the request keeps them.
    pub(crate) poses: Vec<PosedBounce>,
    /// The slot the bake stopped at past the tile range, when it did: no
    /// slot from it on, baked or held, joins the states.
    pub(crate) stopped_at: Option<usize>,
}

/// What every baked pose shares, built at the first slot that bakes.
struct PoseShared<'a> {
    reach: PatchReachIndex<'a>,
    terms: Vec<super::bounce::LightTerm<'a>>,
    absent: MoverAbsentDepth<'a>,
    #[cfg(test)]
    _reach: crate::live_bytes::Held,
}

impl<'a> PoseShared<'a> {
    fn new(inputs: &StaticBounce<'a>) -> Self {
        Self {
            reach: PatchReachIndex::new(inputs.patches.patches()),
            #[cfg(test)]
            _reach: crate::live_bytes::hold(
                "mover bounce reach index",
                inputs.patches.len() as u64 * crate::surface_radiance::REACH_INDEX_BYTES_PER_PATCH,
            ),
            terms: light_terms(inputs.lights, inputs.plan),
            absent: MoverAbsentDepth::new(inputs.base, inputs.depth_tiles),
        }
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

/// Re-bake id 34 per pose of each baked slot, each mover posed alone, and
/// reduce each mover's two poses to its states. Slots run in request order,
/// a held slot counting its depth probes where a cold bake would bake it,
/// so warm and cold stop at the same slot past the tile range. A pose holds
/// one posed copy of the patch radiance and the later-bounce buffers while
/// it runs, and drops them before the next; a mover holds its two poses'
/// gathered probes until it reduces them, and the reduction drops its depth
/// below the significance floor before the next mover runs.
pub(crate) fn bake_mover_bounce(
    sh_ctx: &ShBakeCtx<'_>,
    inputs: &StaticBounce<'_>,
    slots: &[MoverSlot<'_>],
    request: &MoverBounceRequest,
    control: &BakeControl,
) -> MoverBounceBake {
    let ctx = sh_ctx.ray_ctx();
    let mut shared: Option<PoseShared<'_>> = None;
    let valid_probes = inputs
        .layout
        .validity
        .iter()
        .filter(|&&valid| valid != 0)
        .count();
    let mut out = MoverBounceBake::default();
    let mut depth_probes = std::collections::BTreeSet::new();
    for (at, slot) in slots.iter().enumerate() {
        if depth_probes.len() > request.depth_cap as usize {
            // The build fails by name when it reserves these tiles; the
            // jobs left never run, so their progress lands at once. A held
            // slot's progress landed with its memo hit.
            control.advance(
                slots[at..]
                    .iter()
                    .filter_map(|slot| match slot {
                        MoverSlot::Bake(job) => Some(job.progress_units(
                            valid_probes,
                            inputs.patches.len(),
                            inputs.bounce_fill,
                        )),
                        MoverSlot::Held(_) => None,
                    })
                    .sum(),
            );
            out.stopped_at = Some(at);
            break;
        }
        let job = match slot {
            MoverSlot::Bake(job) => job,
            MoverSlot::Held(states) => {
                depth_probes.extend(states.depth.iter().map(|probe| probe.probe));
                continue;
            }
        };
        let PoseShared {
            reach,
            terms,
            absent,
            ..
        } = shared.get_or_insert_with(|| PoseShared::new(inputs));
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
                sh_ctx, &ctx, inputs, reach, terms, job, scene, request, control,
            );
            if request.keep_poses {
                out.poses.push(posed);
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
        let states = reduce_poses(job.mover_id, &poses, absent, request.depth_cap);
        drop(poses);
        log::info!(
            "[MoverLight] probe channel bounce: mover `{}` (#{}) re-baked {} posed bounce(s): \
             {} affected probe(s), {} probe(s) carrying closed-pose depth, {:.1} s",
            job.label,
            job.mover_id,
            job.bounces,
            states.bounce.len(),
            states.depth_carried,
            started.elapsed().as_secs_f64(),
        );
        depth_probes.extend(states.depth.iter().map(|probe| probe.probe));
        out.movers.push(states);
    }
    out
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
