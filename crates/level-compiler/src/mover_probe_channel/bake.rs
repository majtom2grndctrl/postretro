// The bounce-and-depth bake over every mover the SH base bake re-bakes: its
// jobs, what their poses share, and each mover's poses reduced to its states
// in request order, against the tile range.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::time::Instant;

use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::absent_bounces::AbsentLaterBounces;
use super::bounce::{light_terms, posed_later_progress_units};
use super::depth::MoverAbsentDepth;
use super::gather::valid_probes;
use super::near_field::NearFieldDomain;
use super::pose_bake::{PoseBaker, PosedBounce};
use super::posed_scene::PosedScene;
use super::states::{MoverBounceStates, reduce_poses};
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
    /// The mover posed at each pose, in path order: the first and last are
    /// its termini.
    pub(crate) scenes: Vec<PosedScene>,
    /// Each pose's path fraction t, 0 first and 1 last.
    pub(crate) knots: Vec<f32>,
    /// What a mid pose gathers over: the swept brushes widened by the
    /// gather's lift and one probe spacing. Its probes take the mid pose's
    /// gather, its static patches the mid pose's posed later bounces.
    pub(crate) domain: NearFieldDomain,
    /// The valid probes inside `domain`, ascending by dense index.
    pub(crate) domain_probes: Vec<u32>,
}

impl MoverBounceJob {
    /// Whether pose `pose` is a terminus: the first or the last.
    fn is_terminus(&self, pose: usize) -> bool {
        pose == 0 || pose + 1 == self.scenes.len()
    }

    /// Whether the mover has a pose between its termini.
    pub(super) fn has_mid_poses(&self) -> bool {
        self.scenes.len() > 2
    }

    /// Progress units this job's bake advances over `valid_probes` valid
    /// probes and the static `patches` at `fill`: per terminus, one per
    /// valid probe (the posed gather, or the depth-only trace) and one per
    /// patch chunk of each posed later bounce over every static patch; per
    /// mid pose, one per domain probe (its gather, or its depth-only trace)
    /// and one per patch chunk of each posed later bounce over the static
    /// patches in its domain. Each posed later bounce also counts the pose's
    /// own patch chunks. Posed bounce one counts none. A memo hit advances
    /// them all at once.
    pub(crate) fn progress_units(
        &self,
        valid_probes: usize,
        patches: &PatchSet,
        fill: f32,
    ) -> usize {
        let domain = self.domain_probes.len();
        let later = self.bounces.saturating_sub(1);
        let domain_patches = if fill != 0.0 && later > 0 && self.has_mid_poses() {
            self.domain.static_patches(patches).len()
        } else {
            0
        };
        self.scenes
            .iter()
            .map(|scene| {
                let terminus = self.is_terminus(scene.pose);
                let gathered = if terminus { valid_probes } else { domain };
                if self.bounces == 0 || fill == 0.0 {
                    return gathered;
                }
                let world = if terminus {
                    patches.len()
                } else {
                    domain_patches
                };
                gathered + posed_later_progress_units(later, world, scene.patches.len())
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
pub(super) struct PoseShared<'a> {
    pub(super) reach: PatchReachIndex<'a>,
    pub(super) terms: Vec<super::bounce::LightTerm<'a>>,
    pub(super) absent: MoverAbsentDepth<'a>,
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

/// What every pose of one SH base bake reads: the gate's patches, their
/// static bounce one (before later bounces), the plan's light order, the
/// gathered id 34 the bar compares against and its depth tiles before mover
/// tiles reserve their range, and id 34's own later bounces, when a job
/// reads them ([`AbsentLaterBounces::needed_by`]).
pub(crate) struct StaticBounce<'a> {
    pub(crate) patches: &'a PatchSet,
    pub(crate) bounce_one: &'a PatchRadiance,
    pub(crate) lights: &'a [&'a MapLight],
    pub(crate) plan: &'a LayerPlan,
    pub(crate) bounce_fill: f32,
    pub(crate) layout: &'a ProbeGridLayout,
    pub(crate) base: &'a OctahedralShVolumeSection,
    pub(crate) depth_tiles: &'a ProbeDepthTiles,
    pub(crate) absent: Option<&'a AbsentLaterBounces>,
}

/// Re-bake id 34 per pose of each baked slot, each mover posed alone, and
/// reduce each mover's poses to its states. Slots run in request order, a
/// held slot counting its depth probes where a cold bake would bake it, so
/// warm and cold stop at the same slot past the tile range. A pose holds one
/// posed copy of the patch radiance and the later-bounce buffers while it
/// runs, and drops them before the next; a mover holds its termini's
/// gathered probes and its mid poses' states until it reduces them, and the
/// reduction drops its depth below the significance floor before the next
/// mover runs.
pub(crate) fn bake_mover_bounce(
    sh_ctx: &ShBakeCtx<'_>,
    inputs: &StaticBounce<'_>,
    slots: &[MoverSlot<'_>],
    request: &MoverBounceRequest,
    control: &BakeControl,
) -> MoverBounceBake {
    let valid = valid_probes(inputs.layout);
    let mut shared: Option<PoseShared<'_>> = None;
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
                            valid.len(),
                            inputs.patches,
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
        let shared = shared.get_or_insert_with(|| PoseShared::new(inputs));
        let started = Instant::now();
        let baker = PoseBaker {
            sh_ctx,
            ctx: sh_ctx.ray_ctx(),
            inputs,
            shared,
            job,
            request,
            control,
            valid: &valid,
        };
        let Some(poses) = baker.bake(&mut out.poses) else {
            continue;
        };
        let states = reduce_poses(job.mover_id, &poses, &shared.absent, request.depth_cap);
        drop(poses);
        log::info!(
            "[MoverLight] probe channel bounce: mover `{}` (#{}) re-baked {} posed bounce(s) at {} \
             pose(s), {} near-field probe(s): {} affected probe(s), {} probe(s) carrying \
             closed-pose depth, {:.1} s",
            job.label,
            job.mover_id,
            job.bounces,
            job.scenes.len(),
            job.domain_probes.len(),
            states.bounce.len(),
            states.depth_carried,
            started.elapsed().as_secs_f64(),
        );
        depth_probes.extend(states.depth.iter().map(|probe| probe.probe));
        out.movers.push(states);
    }
    out
}
