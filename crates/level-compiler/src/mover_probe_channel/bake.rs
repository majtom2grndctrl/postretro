// The bounce-and-depth bake over every mover the SH base bake re-bakes: its
// jobs, what their poses share, and each mover's poses reduced to its states
// in request order, against the tile range.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::time::Instant;

use postretro_level_format::mover_probe_channel::Knot;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::absent_bounces::AbsentLaterBounces;
use super::bounce::{light_terms, posed_later_progress_units};
use super::depth::MoverAbsentDepth;
use super::edge_copy::copy_job_bounce_edges;
use super::gather::valid_probes;
use super::near_field::NearFieldDomain;
use super::pose_bake::{PoseBaker, PosedBounce};
use super::pose_memo::{PoseKind, PoseMemo};
use super::posed_scene::PosedScene;
use super::states::{MoverBounceStates, MoverPoseStates, PoseState, reduce_poses};
use crate::bake_control::BakeControl;
use crate::cache::CacheKey;
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
    /// Bounces a pose this build bakes re-bakes posed, bounce one first:
    /// `high_bounces` at the build's fidelity
    /// ([`crate::bake_fidelity::BakeFidelity::bounces`]). Later bounces past
    /// them are id 34's own. 0 bakes no bounce and traces only the poses'
    /// probe depth.
    pub(crate) bounces: u32,
    /// The mover's own count, its `light_bounces` clamped to the SH base's:
    /// what a `high` build bakes, and the memo entry a `low` build serves
    /// before baking. Equals `bounces` at high, and wherever it is at most 1.
    pub(crate) high_bounces: u32,
    /// The mover posed at each pose, in path order: the first and last are
    /// its termini.
    pub(crate) scenes: Vec<PosedScene>,
    /// The knots compose reads, as the posed mover holds them
    /// ([`super::posed_solid::PosedMover::read_knots`]): where enclosure
    /// runs and their clear neighbours are found
    /// ([`super::edge_copy::value_sources`]).
    pub(crate) read_knots: Vec<Knot>,
    /// What a mid pose gathers over: the swept brushes widened by the
    /// gather's lift and one probe spacing. Its probes take the mid pose's
    /// gather, its static patches the mid pose's posed later bounces.
    pub(crate) domain: NearFieldDomain,
    /// The valid probes inside `domain`, ascending by dense index.
    pub(crate) domain_probes: Vec<u32>,
}

impl MoverBounceJob {
    /// Whether pose `pose` is a terminus: the first or the last.
    pub(super) fn is_terminus(&self, pose: usize) -> bool {
        pose == 0 || pose + 1 == self.scenes.len()
    }

    /// Whether the mover has a pose between its termini.
    pub(super) fn has_mid_poses(&self) -> bool {
        self.scenes.len() > 2
    }

    /// Progress units the bake of pose `pose` at `bounces` posed bounces
    /// advances over `valid_probes` valid probes, `patches` static patches
    /// and `domain_patches` of them inside the near-field domain
    /// ([`Self::domain_patch_count`]) at `fill`: at a terminus, one per
    /// valid probe (the posed gather, or the depth-only trace) and one per
    /// patch chunk of each posed later bounce over every static patch; at a
    /// mid pose, one per domain probe (its gather, or its depth-only trace)
    /// and one per patch chunk of each posed later bounce over the static
    /// patches in its domain. Each posed later bounce also counts the pose's
    /// own patch chunks. Posed bounce one counts none. A memo hit advances
    /// them at once.
    pub(crate) fn pose_progress_units(
        &self,
        pose: usize,
        bounces: u32,
        valid_probes: usize,
        patches: usize,
        domain_patches: usize,
        fill: f32,
    ) -> usize {
        let terminus = self.is_terminus(pose);
        let gathered = if terminus {
            valid_probes
        } else {
            self.domain_probes.len()
        };
        if bounces == 0 || fill == 0.0 {
            return gathered;
        }
        let world = if terminus { patches } else { domain_patches };
        gathered + posed_later_progress_units(bounces - 1, world, self.scenes[pose].patches.len())
    }

    /// The static patches inside the near-field domain a mid pose's later
    /// bounces trace, counted only where some mid pose may trace one: at
    /// the mover's own count, which a pose a `high` entry serves holds.
    pub(crate) fn domain_patch_count(&self, patches: &PatchSet, fill: f32) -> usize {
        if fill != 0.0 && self.high_bounces > 1 && self.has_mid_poses() {
            self.domain.static_patches(patches).len()
        } else {
            0
        }
    }

    /// Progress units every pose of the job advances at the build's count
    /// ([`Self::pose_progress_units`]), before any memo lookup.
    pub(crate) fn progress_units(
        &self,
        valid_probes: usize,
        patches: &PatchSet,
        fill: f32,
    ) -> usize {
        let domain_patches = self.domain_patch_count(patches, fill);
        (0..self.scenes.len())
            .map(|pose| {
                self.pose_progress_units(
                    pose,
                    self.bounces,
                    valid_probes,
                    patches.len(),
                    domain_patches,
                    fill,
                )
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
    /// stops, and the build fails by name before any mid pose skips
    /// ([`super::depth::refuse_cut_short`]). No mover keeps more than one
    /// depth record past it ([`super::states::reduce_poses`]), and a mover
    /// past it has none of its poses memoized.
    pub(crate) depth_cap: u32,
}

impl MoverBounceRequest {
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }
}

/// How one pose of a job resolves.
pub(crate) struct PoseSlot {
    /// Its memo key, when a cache is open: the entry it was served from, or
    /// the one a baked pose is written to.
    pub(crate) key: Option<CacheKey>,
    /// The posed bounces its state holds: the job's `high_bounces` when a
    /// `high` entry served it, else the job's `bounces`, at which it bakes.
    pub(crate) bounces: u32,
    /// Its memo held it at lookup: the bake reads it there again, and bakes
    /// it only when that read fails. A held pose of a mover with a miss
    /// advances its progress when it reads back, so one that fails to read
    /// back advances only as it bakes.
    pub(crate) held: bool,
}

/// One production job in request order, as the bake meets it.
#[derive(Clone, Copy)]
pub(crate) enum MoverSlot<'a> {
    /// Some pose's memo missed: the bake resolves each pose, from the memo
    /// where `poses` holds it, else by baking it, then reduces the mover.
    Bake {
        job: &'a MoverBounceJob,
        poses: &'a [PoseSlot],
    },
    /// Every pose's memo hit and the mover reduced at lookup: its depth
    /// probes count toward the tile range where a cold bake would have
    /// baked it.
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
/// reduce each mover's poses to its states once every pose has resolved,
/// from `memo` or baked. Slots run in request order, a held slot counting
/// its depth probes where a cold bake would bake it, so warm and cold stop
/// at the same slot past the tile range. A pose holds one posed copy of the
/// patch radiance and the later-bounce buffers while it runs, and drops
/// them before the next; a mover holds its termini's gathered probes and
/// its mid poses' states until it reduces them, and the reduction drops its
/// depth below the significance floor before the next mover runs. Each
/// baked pose is then written to its memo, unless the mover's depth alone
/// passes the tile range: the build fails by name, so it is never memoized.
pub(crate) fn bake_mover_bounce(
    sh_ctx: &ShBakeCtx<'_>,
    inputs: &StaticBounce<'_>,
    slots: &[MoverSlot<'_>],
    request: &MoverBounceRequest,
    memo: Option<&PoseMemo<'_>>,
    control: &BakeControl,
) -> MoverBounceBake {
    let valid = valid_probes(inputs.layout);
    let mut shared: Option<PoseShared<'_>> = None;
    let mut out = MoverBounceBake::default();
    let mut depth_probes = std::collections::BTreeSet::new();
    for (at, slot) in slots.iter().enumerate() {
        if depth_probes.len() > request.depth_cap as usize {
            // The build fails by name before any mid pose skips; the jobs
            // left never run, so their progress lands at once, their held
            // poses' included. A held slot's progress landed with its memo
            // hit.
            control.advance(
                slots[at..]
                    .iter()
                    .filter_map(|slot| match slot {
                        MoverSlot::Bake { job, poses } => Some(poses_progress_units(
                            job,
                            poses,
                            valid.len(),
                            inputs.patches,
                            inputs.bounce_fill,
                            |_| true,
                        )),
                        MoverSlot::Held(_) => None,
                    })
                    .sum(),
            );
            out.stopped_at = Some(at);
            break;
        }
        let (job, pose_slots) = match slot {
            MoverSlot::Bake { job, poses } => (*job, *poses),
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
        let held = read_held_poses(job, pose_slots, memo, inputs.layout);
        // A pose read back advances now; one whose read fails bakes, and
        // advances as it bakes.
        control.advance(poses_progress_units(
            job,
            pose_slots,
            valid.len(),
            inputs.patches,
            inputs.bounce_fill,
            |pose| held[pose].is_some(),
        ));
        let bounces: Vec<u32> = pose_slots.iter().map(|slot| slot.bounces).collect();
        let Some(resolved) = baker.bake(held, &bounces, &mut out.poses) else {
            continue;
        };
        let baked = resolved.baked;
        let poses = MoverPoseStates::from_poses(resolved.poses);
        let mut states = reduce_poses(job.mover_id, &poses, &shared.absent, request.depth_cap);
        copy_job_bounce_edges(&mut states, job, inputs.layout);
        if let Some(memo) = memo
            && !states.depth_past_range(request.depth_cap)
        {
            write_baked_poses(memo, pose_slots, poses.into_poses(), &baked);
        } else {
            drop(poses);
        }
        log::info!(
            "[MoverLight] probe channel bounce: mover `{}` (#{}) re-baked {} posed bounce(s) at {} \
             of {} pose(s), {} near-field probe(s): {} affected probe(s), {} probe(s) carrying \
             closed-pose depth, {:.1} s",
            job.label,
            job.mover_id,
            baked_bounces(&bounces, &baked),
            baked.iter().filter(|&&baked| baked).count(),
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

/// The posed bounces the baked poses re-baked: one count, or each distinct
/// one where a pose whose `high` entry failed to read baked at it.
fn baked_bounces(bounces: &[u32], baked: &[bool]) -> String {
    let mut counts: Vec<u32> = bounces
        .iter()
        .zip(baked)
        .filter(|(_, baked)| **baked)
        .map(|(bounces, _)| *bounces)
        .collect();
    counts.sort_unstable();
    counts.dedup();
    if counts.is_empty() {
        return "no".to_owned();
    }
    counts
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join("/")
}

/// Progress units the poses of `job` that `counted` picks advance, each at
/// the count its slot in `poses` resolves at
/// ([`MoverBounceJob::pose_progress_units`]).
pub(crate) fn poses_progress_units(
    job: &MoverBounceJob,
    poses: &[PoseSlot],
    valid_probes: usize,
    patches: &PatchSet,
    fill: f32,
    counted: impl Fn(usize) -> bool,
) -> usize {
    if !(0..poses.len()).any(&counted) {
        return 0;
    }
    let domain_patches = job.domain_patch_count(patches, fill);
    poses
        .iter()
        .enumerate()
        .filter(|(pose, _)| counted(*pose))
        .map(|(pose, slot)| {
            job.pose_progress_units(
                pose,
                slot.bounces,
                valid_probes,
                patches.len(),
                domain_patches,
                fill,
            )
        })
        .sum()
}

/// Each pose the memo held at lookup, read again; `None` where it held
/// none, or where the read now fails, so the bake bakes that pose.
fn read_held_poses(
    job: &MoverBounceJob,
    poses: &[PoseSlot],
    memo: Option<&PoseMemo<'_>>,
    layout: &ProbeGridLayout,
) -> Vec<Option<PoseState>> {
    poses
        .iter()
        .enumerate()
        .map(|(pose, slot)| {
            let (memo, key) = memo.zip(slot.key.as_ref()).filter(|_| slot.held)?;
            memo.get(
                key,
                PoseKind::of(pose, poses.len()),
                layout,
                &job.domain_probes,
            )
        })
        .collect()
}

/// Write every pose `baked` marks to its memo key, dropping each pose's
/// states as it goes.
fn write_baked_poses(
    memo: &PoseMemo<'_>,
    slots: &[PoseSlot],
    poses: Vec<PoseState>,
    baked: &[bool],
) {
    for ((slot, state), &baked) in slots.iter().zip(poses).zip(baked) {
        if let Some(key) = slot.key.as_ref().filter(|_| baked) {
            memo.put(key, &state);
        }
    }
}
