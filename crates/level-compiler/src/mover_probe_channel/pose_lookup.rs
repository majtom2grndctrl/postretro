// The bounce half's memo lookup: every pose of every mover keyed, its `high`
// entry served before the build's own count, and the poses left to bake. It
// plans and holds no states: every mover reads its poses back at its slot.
// See: context/lib/build_pipeline.md §Build Cache (Bake fidelity)

use super::bake::{MoverBounceJob, MoverBounceRequest, MoverSlot, PoseSlot};
use super::pose_memo::{PoseKind, PoseMemo};
use crate::sh_bake::ProbeGridLayout;
use crate::surface_materials::stats::BounceMaterials;
use crate::surface_radiance::PatchSet;

/// How one mover's poses resolved, for the Build Summary: per pose, in
/// path order, the posed bounces its state holds, served from the memo or
/// baked. A pose short of `high_bounces` shows less than a `high` build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MoverPoseBounces {
    pub(crate) mover_id: u32,
    pub(crate) label: String,
    pub(crate) high_bounces: u32,
    pub(crate) poses: Vec<u32>,
    /// The static patches inside the mover's near-field domain, which each
    /// of its mid poses traces its later bounces over; 0 with no mid pose.
    pub(crate) domain_patches: usize,
    /// Every static patch: the share's denominator.
    pub(crate) static_patches: usize,
}

/// The production jobs' memo lookup: per job, its poses' keys and which
/// poses the memo holds.
pub(crate) struct PoseLookup<'r, 'c> {
    memo: Option<PoseMemo<'c>>,
    /// Per job, each pose's key and whether the memo held it.
    poses: Vec<Vec<PoseSlot>>,
    /// Per job, the static patches inside its domain
    /// ([`super::NearFieldDomain::static_patches`]), counted for a job with
    /// mid poses: the Build Summary's projection reads them.
    domain_patches: Vec<usize>,
    /// Every static patch.
    static_patches: usize,
    /// The jobs with some pose the memo does not hold, left to bake.
    pub(crate) pending: Vec<&'r MoverBounceJob>,
}

impl<'r, 'c> PoseLookup<'r, 'c> {
    /// Key every pose of every job of `request` under `memo`, a mid pose
    /// over its domain's probes in `layout` and static patches in
    /// `patches`. With `read`, read each pose: first its entry at the job's
    /// `high_bounces`, so the cache serves the best fidelity it holds, then,
    /// where the build bakes fewer, its entry at the job's `bounces`. A
    /// `high` build reads only `high`. Each read decodes, so a corrupt
    /// entry is a miss here, and drops: the lookup keeps only which poses
    /// hit and at what count, and every mover reads its hits again at its
    /// slot ([`super::bake_mover_bounce`]), so no raw or reduced state is
    /// held across movers. The stage cache cannot tell an entry is whole
    /// without reading it. Without `read` (an id-34 volume miss: id 34
    /// itself is baking, so no state to reduce against exists yet), no pose
    /// entry is read and every pose bakes at the job's `bounces`. Poses are
    /// written inside the mover bake, before the volume entry, so poses
    /// under this key may exist: a build killed or failed after writing
    /// them, or a volume entry evicted alone. Re-baking them costs time
    /// only; the bytes are the same. Logs one `[cache]` line and one line
    /// per mover.
    pub(crate) fn new(
        request: &'r MoverBounceRequest,
        memo: Option<PoseMemo<'c>>,
        materials: Option<&BounceMaterials>,
        layout: &ProbeGridLayout,
        patches: &PatchSet,
        read: bool,
    ) -> Self {
        let memo = memo.filter(|_| !request.keep_poses);
        let mut lookup = Self {
            memo,
            poses: Vec::with_capacity(request.jobs.len()),
            domain_patches: Vec::with_capacity(request.jobs.len()),
            static_patches: patches.len(),
            pending: Vec::new(),
        };
        let (mut probed, mut missed) = (0, 0);
        for job in &request.jobs {
            let Some(memo) = lookup.memo.as_ref() else {
                // No key reads the domain's patches here; count them alone,
                // a point-in-box test per patch and no ray.
                lookup.domain_patches.push(if job.has_mid_poses() {
                    job.domain.static_patches(patches).len()
                } else {
                    0
                });
                lookup.poses.push(
                    (0..job.scenes.len())
                        .map(|_| PoseSlot {
                            key: None,
                            bounces: job.bounces,
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
            lookup.domain_patches.push(domain_patches.len());
            let mut slots = Vec::with_capacity(count);
            for pose in 0..count {
                let kind = PoseKind::of(pose, count);
                // Read whole and dropped: the slot reads it again.
                let read_at = |bounces: u32| {
                    let key = memo.key(job, pose, bounces, materials, &domain_patches);
                    let held = memo.get(&key, kind, layout, &job.domain_probes).is_some();
                    (key, bounces, held)
                };
                // Best fidelity first: a `low` build serves a cached `high`
                // pose before its own count, and bakes at its own on a miss.
                let (key, bounces, held) = if read {
                    let high = read_at(job.high_bounces);
                    if high.2 || job.bounces == job.high_bounces {
                        high
                    } else {
                        read_at(job.bounces)
                    }
                } else {
                    (
                        memo.key(job, pose, job.bounces, materials, &domain_patches),
                        job.bounces,
                        false,
                    )
                };
                slots.push(PoseSlot {
                    key: Some(key),
                    bounces,
                    held,
                });
            }
            probed += count;
            missed += slots.iter().filter(|slot| !slot.held).count();
            log_mover(job, &slots);
            if slots.iter().any(|slot| !slot.held) {
                lookup.pending.push(job);
            }
            lookup.poses.push(slots);
        }
        if lookup.memo.is_some() {
            PoseMemo::log_outcome(probed, missed, request.jobs.len(), lookup.pending.len());
        }
        lookup
    }

    /// Every job's poses in request order, at the count each resolves at:
    /// the count it was served at or bakes at, with its domain's static
    /// patches against every static patch.
    pub(crate) fn pose_bounces(&self, request: &MoverBounceRequest) -> Vec<MoverPoseBounces> {
        request
            .jobs
            .iter()
            .zip(&self.poses)
            .zip(&self.domain_patches)
            .map(|((job, slots), &domain_patches)| MoverPoseBounces {
                mover_id: job.mover_id,
                label: job.label.clone(),
                high_bounces: job.high_bounces,
                poses: slots.iter().map(|slot| slot.bounces).collect(),
                domain_patches,
                static_patches: self.static_patches,
            })
            .collect()
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

    /// Every job in request order, with its poses' keys and which the memo
    /// held: each reads its hits back, bakes the rest and finishes at its
    /// slot.
    pub(crate) fn slots<'s>(&'s self, request: &'s MoverBounceRequest) -> Vec<MoverSlot<'s>> {
        request
            .jobs
            .iter()
            .zip(&self.poses)
            .map(|(job, poses)| MoverSlot { job, poses })
            .collect()
    }

    /// Progress units of every job's poses, each at the count its slot
    /// resolves at: what the stage publishes once the lookup has run.
    pub(crate) fn total_units(
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
}

/// One line per mover: `hit` when every pose hit, else the poses that
/// re-bake and what the termini did; and, in a build baking fewer bounces
/// than the mover's own, how many poses a `high` entry served.
fn log_mover(job: &MoverBounceJob, slots: &[PoseSlot]) {
    let count = slots.len();
    let served_high = if job.bounces == job.high_bounces {
        0
    } else {
        slots
            .iter()
            .filter(|slot| slot.held && slot.bounces == job.high_bounces)
            .count()
    };
    let served = if served_high == 0 {
        String::new()
    } else {
        format!(", {served_high} served from high")
    };
    let missed: Vec<String> = slots
        .iter()
        .enumerate()
        .filter(|(_, slot)| !slot.held)
        .map(|(pose, _)| pose.to_string())
        .collect();
    if missed.is_empty() {
        log::info!(
            "[MoverLight] probe channel bounce memo: mover `{}` (#{}) hit ({count} \
             pose(s){served})",
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
         pose(s) {} of {count}{served}",
        job.label,
        job.mover_id,
        missed.join(", "),
    );
}
