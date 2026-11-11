// The bounce half's memo lookup: every pose of every mover keyed, the
// movers whose every pose hits reduced at once, and the poses left to bake.
// See: context/lib/build_pipeline.md §Build Cache

use super::bake::{MoverBounceBake, MoverBounceJob, MoverBounceRequest, MoverSlot, PoseSlot};
use super::depth::MoverAbsentDepth;
use super::pose_memo::{PoseKind, PoseMemo};
use super::states::{MoverBounceStates, MoverPoseStates, reduce_poses};
use crate::sh_bake::ProbeGridLayout;
use crate::surface_materials::stats::BounceMaterials;
use crate::surface_radiance::PatchSet;

/// What a lookup reduces a mover with when every pose of it hits: the
/// gathered id 34 and its depth tiles, which the depth floor reads, and
/// the tile range.
pub(crate) struct HitReduction<'a> {
    pub(crate) absent: MoverAbsentDepth<'a>,
    pub(crate) depth_cap: u32,
}

/// The production jobs' memo lookup: per job, its poses' keys and which
/// poses the memo holds, and the states of the jobs whose every pose hit.
pub(crate) struct PoseLookup<'r, 'c> {
    memo: Option<PoseMemo<'c>>,
    /// Per job, its states when every pose hit and the lookup reduced it.
    found: Vec<Option<MoverBounceStates>>,
    /// Per job, each pose's key and whether the memo held it.
    poses: Vec<Vec<PoseSlot>>,
    /// The jobs with some pose left to bake.
    pub(crate) pending: Vec<&'r MoverBounceJob>,
}

impl<'r, 'c> PoseLookup<'r, 'c> {
    /// Key every pose of every job of `request` under `memo`, at its
    /// effective bounce count, a mid pose over its domain's probes in
    /// `layout` and static patches in `patches`. With `reduce`, read each
    /// pose: a job whose every pose hits reduces at once, its raw poses
    /// dropped; a job with a miss keeps only which poses hit, and the bake
    /// reads those again when it reaches the job, so no raw pose is held
    /// across movers. Without it (id 34 itself is baking, so no state to
    /// reduce against exists yet, and no pose under its key was stored),
    /// every pose bakes. Logs one `[cache]` line and one line per mover.
    pub(crate) fn new(
        request: &'r MoverBounceRequest,
        memo: Option<PoseMemo<'c>>,
        materials: Option<&BounceMaterials>,
        layout: &ProbeGridLayout,
        patches: &PatchSet,
        reduce: Option<&HitReduction<'_>>,
    ) -> Self {
        let memo = memo.filter(|_| !request.keep_poses);
        let mut lookup = Self {
            memo,
            found: Vec::with_capacity(request.jobs.len()),
            poses: Vec::with_capacity(request.jobs.len()),
            pending: Vec::new(),
        };
        let (mut probed, mut missed) = (0, 0);
        for job in &request.jobs {
            let Some(memo) = lookup.memo.as_ref() else {
                lookup.found.push(None);
                lookup.poses.push(
                    (0..job.scenes.len())
                        .map(|_| PoseSlot {
                            key: None,
                            held: false,
                        })
                        .collect(),
                );
                lookup.pending.push(job);
                continue;
            };
            let count = job.scenes.len();
            let domain_patches = if job.has_mid_poses() {
                job.domain.static_patches(patches)
            } else {
                Vec::new()
            };
            let mut slots = Vec::with_capacity(count);
            let mut states = Vec::with_capacity(count);
            #[cfg(test)]
            let mut _held = Vec::new();
            for pose in 0..count {
                let key = memo.key(job, pose, job.bounces, materials, &domain_patches);
                let state = reduce.and_then(|_| {
                    memo.get(&key, PoseKind::of(pose, count), layout, &job.domain_probes)
                });
                #[cfg(test)]
                if let Some(state) = &state {
                    _held.push(crate::live_bytes::hold(
                        "mover posed probes",
                        state.heap_bytes(),
                    ));
                }
                slots.push(PoseSlot {
                    key: Some(key),
                    held: state.is_some(),
                });
                states.push(state);
            }
            let all_hit = states.iter().all(Option::is_some);
            let found = match reduce {
                Some(reduce) if all_hit => {
                    let poses = MoverPoseStates::from_poses(states.into_iter().flatten().collect());
                    Some(reduce_poses(
                        job.mover_id,
                        &poses,
                        &reduce.absent,
                        reduce.depth_cap,
                    ))
                }
                _ => {
                    drop(states);
                    None
                }
            };
            probed += count;
            missed += slots.iter().filter(|slot| !slot.held).count();
            log_mover(job, &slots);
            if found.is_none() {
                lookup.pending.push(job);
            }
            lookup.found.push(found);
            lookup.poses.push(slots);
        }
        if lookup.memo.is_some() {
            PoseMemo::log_outcome(probed, missed, request.jobs.len(), lookup.pending.len());
        }
        lookup
    }

    /// The memo the bake reads held poses from and writes baked ones to.
    pub(crate) fn memo(&self) -> Option<&PoseMemo<'c>> {
        self.memo.as_ref()
    }

    /// Whether a pending job re-bakes radiance, which needs the patches'
    /// bounce one; a job at `light_bounces` 0 traces depth alone.
    pub(crate) fn needs_radiance(&self) -> bool {
        self.pending.iter().any(|job| job.bounces > 0)
    }

    /// Every job in request order: held, reduced from the memo, or baked
    /// pose by pose.
    pub(crate) fn slots<'s>(&'s self, request: &'s MoverBounceRequest) -> Vec<MoverSlot<'s>> {
        request
            .jobs
            .iter()
            .zip(&self.found)
            .zip(&self.poses)
            .map(|((job, found), poses)| match found {
                Some(states) => MoverSlot::Held(states),
                None => MoverSlot::Bake { job, poses },
            })
            .collect()
    }

    /// Progress units the poses the memo held advance at once.
    pub(crate) fn held_units(
        &self,
        request: &MoverBounceRequest,
        units: impl Fn(&MoverBounceJob, &[PoseSlot]) -> usize,
    ) -> usize {
        request
            .jobs
            .iter()
            .zip(&self.poses)
            .map(|(job, poses)| units(job, poses))
            .sum()
    }

    /// Merge the baked jobs' states with the held ones, by mover id, short
    /// of the jobs from the slot the bake stopped at.
    pub(crate) fn finish(
        mut self,
        request: &MoverBounceRequest,
        baked: MoverBounceBake,
    ) -> (Vec<MoverBounceStates>, MoverBounceBake) {
        if let Some(stopped_at) = baked.stopped_at {
            self.found[stopped_at..].fill(None);
        }
        let mut baked = baked;
        for states in std::mem::take(&mut baked.movers) {
            if let Some(at) = request
                .jobs
                .iter()
                .position(|job| job.mover_id == states.mover_id)
            {
                self.found[at] = Some(states);
            }
        }
        let mut states: Vec<MoverBounceStates> = self.found.into_iter().flatten().collect();
        states.sort_by_key(|states| states.mover_id);
        (states, baked)
    }
}

/// One line per mover: `hit` when every pose hit, else the poses that
/// re-bake and what the termini did.
fn log_mover(job: &MoverBounceJob, slots: &[PoseSlot]) {
    let count = slots.len();
    let missed: Vec<String> = slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| !slot.held)
        .map(|(pose, _)| pose.to_string())
        .collect();
    if missed.is_empty() {
        log::info!(
            "[MoverLight] probe channel bounce memo: mover `{}` (#{}) hit ({count} pose(s))",
            job.label,
            job.mover_id,
        );
        return;
    }
    let termini = match (slots[0].held, slots[count - 1].held) {
        (true, true) => "termini hit",
        (false, false) => "termini miss",
        (true, false) => "first terminus hit, last miss",
        (false, true) => "first terminus miss, last hit",
    };
    log::info!(
        "[MoverLight] probe channel bounce memo: mover `{}` (#{}) miss: {termini}; re-bakes \
         pose(s) {} of {count}",
        job.label,
        job.mover_id,
        missed.join(", "),
    );
}
