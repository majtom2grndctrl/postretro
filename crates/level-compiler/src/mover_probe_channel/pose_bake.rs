// One mover's poses baked from the SH base's patches: at a terminus, posed
// bounce one, posed later bounces up to the mover's `light_bounces`, the SH
// base's own later bounces past them, and the posed gather; at a mid pose,
// posed bounce one gathered over the near-field domain, with the termini's
// later shares blended in.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Mover probe channel)

use super::bake::{MoverBounceJob, MoverBounceRequest, PoseShared, StaticBounce};
use super::bounce::{PosedLaterBounces, add_posed_later_bounces, posed_bounce_one};
use super::gather::{
    DomainProbe, PosedGather, PosedProbe, compact_slots, gather_domain_probes, gather_posed_probes,
    trace_posed_depth,
};
use super::mid_pose::{DomainShares, classify_mid_pose, depth_only_mid_pose};
use super::posed_scene::PosedScene;
use super::states::{ChangedProbe, MoverPoseStates, PoseStates, terminus_stores};
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
    /// Handed back by the full re-bake: every domain probe's posed bounce
    /// one, with its depth at a mid pose. Empty at a terminus of a mover with
    /// no mid pose.
    #[cfg(test)]
    pub(crate) domain: Vec<DomainProbe>,
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

impl PoseBaker<'_, '_> {
    /// Every pose of the job, termini first: a mid pose's bounce blends the
    /// termini's later shares, so they resolve before it. `None` when the
    /// request keeps each pose's re-bake (into `kept`) instead of reducing.
    pub(super) fn bake(&self, kept: &mut Vec<PosedBounce>) -> Option<MoverPoseStates> {
        let job = self.job;
        let last = job.scenes.len() - 1;
        let mut poses = MoverPoseStates::default();
        #[cfg(test)]
        let mut _held = Vec::new();
        // Each terminus's posed bounce one over the domain, for its shares.
        let mut bounce_one: [Vec<DomainProbe>; 2] = Default::default();
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
                #[cfg_attr(
                    not(test),
                    expect(unused_mut, reason = "only the full re-bake keeps it")
                )]
                let (mut posed, one) = self.terminus(scene);
                if self.request.keep_poses {
                    // The full re-bake hands the terminus's bounce one over
                    // the domain back beside its state.
                    #[cfg(test)]
                    {
                        posed.domain = one;
                    }
                    kept.push(posed);
                    continue;
                }
                bounce_one[k] = one;
                #[cfg(test)]
                _held.push(crate::live_bytes::hold(
                    "mover terminus bounce one",
                    (bounce_one[k].capacity() * size_of::<DomainProbe>()) as u64,
                ));
                poses.termini[k] = posed.into_states();
            }
            #[cfg(test)]
            _held.push(crate::live_bytes::hold(
                "mover posed probes",
                poses.termini[k].heap_bytes(),
            ));
        }
        let shares =
            (job.bounces > 0 && !self.request.keep_poses && job.has_mid_poses()).then(|| {
                DomainShares::new(
                    &job.domain_probes,
                    &poses.termini,
                    &terminus_stores(&poses.termini),
                    &bounce_one,
                    self.inputs.base,
                    &compact_slots(self.inputs.layout),
                    self.request.uncompressed,
                    self.control,
                )
            });
        drop(bounce_one);
        for (pose, scene) in job.scenes.iter().enumerate().take(last).skip(1) {
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
                let gathered = self.mid_bounce_one(scene);
                if self.request.keep_poses {
                    kept.push(PosedBounce {
                        #[cfg(test)]
                        mover_id: job.mover_id,
                        #[cfg(test)]
                        pose,
                        probes: Vec::new(),
                        #[cfg(test)]
                        domain: gathered,
                    });
                    continue;
                }
                let shares = shares.as_ref().expect("a bounce-baking mover's shares");
                classify_mid_pose(shares, gathered, job.knots[pose], self.control)
            };
            #[cfg(test)]
            _held.push(crate::live_bytes::hold("mover mid poses", mid.heap_bytes()));
            poses.mids.push(mid);
        }
        (!self.request.keep_poses || job.bounces == 0).then_some(poses)
    }

    /// One terminus's bounce: posed bounce one, gathered over the near-field
    /// domain for the terminus's later shares; then the job's posed later
    /// bounces with id 34's own past them, and every valid probe's posed
    /// gather.
    fn terminus(&self, scene: &PosedScene) -> (PosedBounce, Vec<DomainProbe>) {
        let (inputs, job) = (self.inputs, self.job);
        let mut radiance = posed_bounce_one(
            &self.ctx,
            inputs.patches,
            &self.shared.reach,
            inputs.bounce_one,
            &self.shared.terms,
            scene,
            self.control,
        );
        let bounce_one = if job.has_mid_poses() {
            gather_domain_probes(
                self.sh_ctx,
                inputs.layout,
                &PosedGather::new(inputs.patches, &radiance, scene),
                &job.domain_probes,
                false,
                self.control,
            )
        } else {
            Vec::new()
        };
        let static_tail = job
            .static_tail_from(inputs.bounce_fill)
            .zip(inputs.static_tail)
            .map_or(&[][..], |(from, tail)| tail.layers_from(from));
        add_posed_later_bounces(
            &self.ctx,
            inputs.patches,
            scene,
            &mut radiance,
            inputs.bounce_fill,
            PosedLaterBounces {
                posed: job.bounces - 1,
                static_tail,
            },
            self.control,
        );
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
        let posed = PosedBounce {
            #[cfg(test)]
            mover_id: job.mover_id,
            #[cfg(test)]
            pose: scene.pose,
            probes,
            #[cfg(test)]
            domain: Vec::new(),
        };
        (posed, bounce_one)
    }

    /// A mid pose's posed bounce one, gathered over the near-field domain
    /// with its depth binned from the same rays. Its later bounces come from
    /// the termini's shares, never re-traced.
    fn mid_bounce_one(&self, scene: &PosedScene) -> Vec<DomainProbe> {
        let inputs = self.inputs;
        let radiance = posed_bounce_one(
            &self.ctx,
            inputs.patches,
            &self.shared.reach,
            inputs.bounce_one,
            &self.shared.terms,
            scene,
            self.control,
        );
        let gathered = gather_domain_probes(
            self.sh_ctx,
            inputs.layout,
            &PosedGather::new(inputs.patches, &radiance, scene),
            &self.job.domain_probes,
            true,
            self.control,
        );
        #[cfg(test)]
        let _held = crate::live_bytes::hold(
            "mover mid pose gather",
            (gathered.capacity() * size_of::<DomainProbe>()) as u64
                + gathered.iter().map(DomainProbe::heap_bytes).sum::<u64>(),
        );
        gathered
    }
}
