// One mover's poses baked from the SH base's patches: posed bounce one,
// posed later bounces up to the mover's `light_bounces` (over every static
// patch at a terminus, over the near-field domain's at a mid pose, rays
// leaving it reading the mover-absent bake), the SH base's own later bounces
// past them, and the posed gather (every valid probe at a terminus, the
// domain's at a mid pose).
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use std::time::Instant;

use super::absent_bounces::AbsentLaterBounces;
use super::bake::{MoverBounceJob, MoverBounceRequest, PoseShared, StaticBounce};
use super::bounce::{
    LaterReach, PosedLaterBounces, PosedRadiance, add_posed_later_bounces, posed_bounce_one,
};
use super::gather::{
    DomainProbe, PosedGather, PosedProbe, compact_slots, gather_domain_probes, gather_posed_probes,
    trace_posed_depth,
};
use super::mid_pose::{classify_mid_pose, depth_only_mid_pose};
use super::posed_scene::PosedScene;
use super::states::{ChangedProbe, MoverPoseStates, PoseStates};
use crate::bake_control::BakeControl;
use crate::sh_bake::{RaytracingCtx, ShBakeCtx};

/// One pose's bounce and depth state, before its mover's poses reduce.
pub(crate) struct PosedBounce {
    /// Which pose this is, for the full re-bake.
    #[cfg(test)]
    pub(crate) mover_id: u32,
    #[cfg(test)]
    pub(crate) pose: usize,
    /// At a terminus, the valid probes that differ from id 34 or whose rays
    /// meet the pose, ascending by probe index.
    pub(crate) probes: Vec<PosedProbe>,
    /// Handed back by the full re-bake at a mid pose: every domain probe's
    /// full posed bounce, with its depth. Empty at a terminus.
    #[cfg(test)]
    pub(crate) domain: Vec<DomainProbe>,
    /// Handed back by the full re-bake at a mid pose: every domain probe's
    /// posed bounce one alone, gathered before the later bounces join.
    /// Empty at a terminus.
    #[cfg(test)]
    pub(crate) domain_bounce_one: Vec<DomainProbe>,
}

impl PosedBounce {
    /// The terminus's states: the probes past the round-trip bar with their
    /// tiles, and every probe's depth where a ray met the pose.
    pub(super) fn into_states(self) -> PoseStates {
        #[cfg(test)]
        let _probes = crate::live_bytes::hold(
            "mover posed probes",
            (self.probes.capacity() * size_of::<PosedProbe>()) as u64
                + self.probes.iter().map(PosedProbe::heap_bytes).sum::<u64>(),
        );
        let mut states = PoseStates::default();
        for probe in self.probes {
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
        states
    }
}

/// One baked job's poses, against the SH base bake's shared inputs.
pub(super) struct PoseBaker<'b, 'a> {
    pub(super) sh_ctx: &'b ShBakeCtx<'a>,
    pub(super) ctx: RaytracingCtx<'a>,
    pub(super) inputs: &'b StaticBounce<'b>,
    pub(super) shared: &'b PoseShared<'b>,
    pub(super) job: &'b MoverBounceJob,
    pub(super) request: &'b MoverBounceRequest,
    pub(super) control: &'b BakeControl,
    /// Every valid probe, ascending: a terminus's gather or depth trace.
    pub(super) valid: &'b [u32],
}

/// A mid pose's two near-field gathers: its full posed bounce, with depth,
/// and, for the full re-bake, its bounce one alone.
struct MidGather {
    full: Vec<DomainProbe>,
    #[cfg(test)]
    bounce_one: Vec<DomainProbe>,
}

impl PoseBaker<'_, '_> {
    /// Every pose of the job in path order. `None` when the request keeps
    /// each pose's re-bake (into `kept`) instead of reducing.
    pub(super) fn bake(&self, kept: &mut Vec<PosedBounce>) -> Option<MoverPoseStates> {
        let job = self.job;
        let last = job.scenes.len() - 1;
        let mut poses = MoverPoseStates::default();
        #[cfg(test)]
        let mut _held = Vec::new();
        for (k, pose) in [0, last].into_iter().enumerate() {
            let scene = &job.scenes[pose];
            if job.bounces == 0 {
                poses.termini[k] = PoseStates {
                    changed: Vec::new(),
                    depth: trace_posed_depth(
                        &self.ctx,
                        self.inputs.layout,
                        scene,
                        self.valid,
                        self.control,
                    ),
                };
            } else {
                let started = Instant::now();
                let posed = self.terminus(scene);
                log::debug!(
                    "[MoverLight] probe channel bounce: mover #{} terminus pose {pose}: {:.2} s",
                    job.mover_id,
                    started.elapsed().as_secs_f64()
                );
                if self.request.keep_poses {
                    kept.push(posed);
                    continue;
                }
                poses.termini[k] = posed.into_states();
            }
            #[cfg(test)]
            _held.push(crate::live_bytes::hold(
                "mover posed probes",
                poses.termini[k].heap_bytes(),
            ));
        }
        let bounce_mids = job.bounces > 0 && job.has_mid_poses();
        let domain_patches = if bounce_mids {
            job.domain.static_patches(self.inputs.patches)
        } else {
            Vec::new()
        };
        let slots = if bounce_mids {
            compact_slots(self.inputs.layout)
        } else {
            Vec::new()
        };
        #[cfg(test)]
        _held.push(crate::live_bytes::hold(
            "mover near-field patches and slots",
            ((domain_patches.capacity() + slots.capacity()) * size_of::<u32>()) as u64,
        ));
        for scene in job.scenes.iter().take(last).skip(1) {
            let mid = if job.bounces == 0 {
                let met = trace_posed_depth(
                    &self.ctx,
                    self.inputs.layout,
                    scene,
                    &job.domain_probes,
                    self.control,
                );
                depth_only_mid_pose(&job.domain_probes, met)
            } else {
                let gathered = self.mid_gather(scene, &domain_patches);
                if self.request.keep_poses {
                    kept.push(PosedBounce {
                        #[cfg(test)]
                        mover_id: job.mover_id,
                        #[cfg(test)]
                        pose: scene.pose,
                        probes: Vec::new(),
                        #[cfg(test)]
                        domain: gathered.full,
                        #[cfg(test)]
                        domain_bounce_one: gathered.bounce_one,
                    });
                    continue;
                }
                classify_mid_pose(
                    gathered.full,
                    self.inputs.base,
                    &slots,
                    self.request.uncompressed,
                    self.control,
                )
            };
            #[cfg(test)]
            _held.push(crate::live_bytes::hold("mover mid poses", mid.heap_bytes()));
            poses.mids.push(mid);
        }
        (!self.request.keep_poses || job.bounces == 0).then_some(poses)
    }

    /// The posed later bounces `reach` gathers over, with id 34's own past
    /// them, added to `radiance`.
    fn later_bounces(&self, scene: &PosedScene, radiance: &mut PosedRadiance, reach: LaterReach) {
        let empty = AbsentLaterBounces::default();
        let absent = self.inputs.absent.unwrap_or(&empty);
        debug_assert!(
            self.inputs.absent.is_some()
                || !AbsentLaterBounces::needed_by([self.job], self.inputs.bounce_fill),
            "a job reading id 34's later bounces has them"
        );
        add_posed_later_bounces(
            &self.ctx,
            self.inputs.patches,
            scene,
            radiance,
            self.inputs.bounce_fill,
            PosedLaterBounces {
                posed: self.job.bounces - 1,
                absent,
                reach,
            },
            self.control,
        );
    }

    /// One terminus's bounce: posed bounce one, the job's posed later
    /// bounces over every static patch with id 34's own past them, and every
    /// valid probe's posed gather.
    fn terminus(&self, scene: &PosedScene) -> PosedBounce {
        let inputs = self.inputs;
        let mut radiance = posed_bounce_one(
            &self.ctx,
            inputs.patches,
            &self.shared.reach,
            inputs.bounce_one,
            &self.shared.terms,
            scene,
            self.control,
        );
        self.later_bounces(scene, &mut radiance, LaterReach::Everywhere);
        let gather = PosedGather::new(inputs.patches, &radiance, scene);
        let probes = gather_posed_probes(
            self.sh_ctx,
            inputs.layout,
            &gather,
            inputs.base,
            self.request.uncompressed,
            self.request.keep_every_tile,
            self.control,
        );
        PosedBounce {
            #[cfg(test)]
            mover_id: self.job.mover_id,
            #[cfg(test)]
            pose: scene.pose,
            probes,
            #[cfg(test)]
            domain: Vec::new(),
            #[cfg(test)]
            domain_bounce_one: Vec::new(),
        }
    }

    /// A mid pose's posed bounce, gathered over the near-field domain's
    /// probes with its depth binned from the same rays: posed bounce one,
    /// the posed later bounces over `domain_patches` (the static patches in
    /// the domain) and the posed sides, rays leaving the domain reading the
    /// mover-absent bake, and id 34's own past them.
    fn mid_gather(&self, scene: &PosedScene, domain_patches: &[u32]) -> MidGather {
        let inputs = self.inputs;
        let started = Instant::now();
        let mut radiance = posed_bounce_one(
            &self.ctx,
            inputs.patches,
            &self.shared.reach,
            inputs.bounce_one,
            &self.shared.terms,
            scene,
            self.control,
        );
        let gather = |radiance: &PosedRadiance, bin_depth: bool| {
            gather_domain_probes(
                self.sh_ctx,
                inputs.layout,
                &PosedGather::new(inputs.patches, radiance, scene),
                &self.job.domain_probes,
                bin_depth,
                self.control,
            )
        };
        #[cfg(test)]
        let bounce_one = if self.request.keep_poses {
            gather(&radiance, false)
        } else {
            Vec::new()
        };
        let one_done = Instant::now();
        self.later_bounces(scene, &mut radiance, LaterReach::NearField(domain_patches));
        let later_done = Instant::now();
        let full = gather(&radiance, true);
        log::debug!(
            "[MoverLight] probe channel bounce: mover #{} mid pose {}: bounce one {:.2} s, {} \
             later bounce(s) over {} of {} static patch(es) and {} posed {:.2} s, gather of {} \
             probe(s) {:.2} s",
            self.job.mover_id,
            scene.pose,
            (one_done - started).as_secs_f64(),
            self.job.bounces.saturating_sub(1),
            domain_patches.len(),
            inputs.patches.len(),
            scene.patches.len(),
            (later_done - one_done).as_secs_f64(),
            self.job.domain_probes.len(),
            later_done.elapsed().as_secs_f64(),
        );
        #[cfg(test)]
        let _held = crate::live_bytes::hold(
            "mover mid pose gather",
            (full.capacity() * size_of::<DomainProbe>()) as u64
                + full.iter().map(DomainProbe::heap_bytes).sum::<u64>(),
        );
        MidGather {
            full,
            #[cfg(test)]
            bounce_one,
        }
    }
}
