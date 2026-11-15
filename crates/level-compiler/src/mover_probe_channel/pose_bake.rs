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
use super::states::{ChangedProbe, PoseState, PoseStates, compact_rgb};
use crate::bake_control::BakeControl;
use crate::sh_bake::{RaytracingCtx, ShBakeCtx};

/// One pose's bounce and depth state, before its mover's poses reduce.
pub(crate) struct PosedBounce {
    /// The id of the mover this pose belongs to.
    #[cfg(test)]
    pub(crate) mover_id: u32,
    /// Which of the mover's poses this is, for the full re-bake.
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
                    tile: compact_rgb(&tile),
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

/// One job's poses, each resolved from its memo or baked, in path order.
pub(super) struct ResolvedPoses {
    pub(super) poses: Vec<PoseState>,
    /// Per pose, whether this bake baked it: only those are written back.
    pub(super) baked: Vec<bool>,
}

/// What every baked mid pose of one job shares: the domain's static patches
/// its later bounces trace, and the compact slots its classification reads.
struct NearField {
    patches: Vec<u32>,
    slots: Vec<u32>,
}

impl PoseBaker<'_, '_> {
    /// Every pose of the job in path order: `held[k]`, read from its memo,
    /// where it holds one, else baked at `bounces[k]` posed bounces. `None`
    /// when the request keeps each baked pose's re-bake (into `kept`)
    /// instead of reducing.
    pub(super) fn bake(
        &self,
        held: Vec<Option<PoseState>>,
        bounces: &[u32],
        kept: &mut Vec<PosedBounce>,
    ) -> Option<ResolvedPoses> {
        let job = self.job;
        debug_assert_eq!(held.len(), job.scenes.len());
        debug_assert_eq!(bounces.len(), job.scenes.len());
        let mut resolved = ResolvedPoses {
            poses: Vec::with_capacity(held.len()),
            baked: Vec::with_capacity(held.len()),
        };
        let mut near_field: Option<NearField> = None;
        #[cfg(test)]
        let mut _held = Vec::new();
        for (pose, held) in held.into_iter().enumerate() {
            resolved.baked.push(held.is_none());
            let state = match held {
                Some(state) => state,
                None => {
                    let scene = &job.scenes[pose];
                    let bounces = bounces[pose];
                    let baked = if job.is_terminus(pose) {
                        self.terminus_states(scene, bounces, kept)
                    } else {
                        let near_field = match &mut near_field {
                            Some(near_field) => near_field,
                            None => {
                                let built = self.near_field();
                                #[cfg(test)]
                                _held.push(crate::live_bytes::hold(
                                    "mover near-field patches and slots",
                                    ((built.patches.capacity() + built.slots.capacity())
                                        * size_of::<u32>())
                                        as u64,
                                ));
                                near_field.insert(built)
                            }
                        };
                        self.mid_states(scene, near_field, bounces, kept)
                    };
                    let Some(state) = baked else {
                        continue;
                    };
                    state
                }
            };
            #[cfg(test)]
            _held.push(crate::live_bytes::hold(
                "mover posed probes",
                state.heap_bytes(),
            ));
            resolved.poses.push(state);
        }
        (!self.request.keep_poses || job.bounces == 0).then_some(resolved)
    }

    /// The near-field inputs a mid pose re-baking bounce reads; none at
    /// `light_bounces` 0, where a mid pose traces depth alone.
    fn near_field(&self) -> NearField {
        if self.job.bounces == 0 {
            return NearField {
                patches: Vec::new(),
                slots: Vec::new(),
            };
        }
        NearField {
            patches: self.job.domain.static_patches(self.inputs.patches),
            slots: compact_slots(self.inputs.layout),
        }
    }

    /// One terminus's states at `bounces` posed bounces: its posed bounce
    /// gathered over every valid probe, or at `light_bounces` 0 its
    /// depth-only trace. `None` when the request keeps the re-bake instead
    /// (into `kept`).
    fn terminus_states(
        &self,
        scene: &PosedScene,
        bounces: u32,
        kept: &mut Vec<PosedBounce>,
    ) -> Option<PoseState> {
        let job = self.job;
        if bounces == 0 {
            return Some(PoseState::Terminus(PoseStates {
                changed: Vec::new(),
                depth: trace_posed_depth(
                    &self.ctx,
                    self.inputs.layout,
                    scene,
                    self.valid,
                    self.control,
                ),
            }));
        }
        let started = Instant::now();
        let posed = self.terminus(scene, bounces);
        log::debug!(
            "[MoverLight] probe channel bounce: mover #{} terminus pose {}: {:.2} s",
            job.mover_id,
            scene.pose,
            started.elapsed().as_secs_f64()
        );
        if self.request.keep_poses {
            kept.push(posed);
            return None;
        }
        Some(PoseState::Terminus(posed.into_states()))
    }

    /// One mid pose's states over the near-field domain at `bounces` posed
    /// bounces: its posed gather classified, or at `light_bounces` 0 its
    /// depth-only trace. `None` when the request keeps the re-bake instead
    /// (into `kept`).
    fn mid_states(
        &self,
        scene: &PosedScene,
        near_field: &NearField,
        bounces: u32,
        kept: &mut Vec<PosedBounce>,
    ) -> Option<PoseState> {
        let job = self.job;
        if bounces == 0 {
            let met = trace_posed_depth(
                &self.ctx,
                self.inputs.layout,
                scene,
                &job.domain_probes,
                self.control,
            );
            return Some(PoseState::Mid(depth_only_mid_pose(&job.domain_probes, met)));
        }
        let gathered = self.mid_gather(scene, &near_field.patches, bounces);
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
            return None;
        }
        Some(PoseState::Mid(classify_mid_pose(
            gathered.full,
            self.inputs.base,
            &near_field.slots,
            self.request.uncompressed,
            self.control,
        )))
    }

    /// The pose's `bounces - 1` posed later bounces `reach` gathers over,
    /// with id 34's own past them, added to `radiance`.
    fn later_bounces(
        &self,
        scene: &PosedScene,
        radiance: &mut PosedRadiance,
        reach: LaterReach,
        bounces: u32,
    ) {
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
                posed: bounces - 1,
                absent,
                reach,
            },
            self.control,
        );
    }

    /// One terminus's bounce: posed bounce one, its `bounces - 1` posed
    /// later bounces over every static patch with id 34's own past them, and
    /// every valid probe's posed gather.
    fn terminus(&self, scene: &PosedScene, bounces: u32) -> PosedBounce {
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
        self.later_bounces(scene, &mut radiance, LaterReach::Everywhere, bounces);
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
    /// its `bounces - 1` posed later bounces over `domain_patches` (the
    /// static patches in the domain) and the posed sides, rays leaving the
    /// domain reading the mover-absent bake, and id 34's own past them.
    fn mid_gather(&self, scene: &PosedScene, domain_patches: &[u32], bounces: u32) -> MidGather {
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
        self.later_bounces(
            scene,
            &mut radiance,
            LaterReach::NearField(domain_patches),
            bounces,
        );
        let later_done = Instant::now();
        let full = gather(&radiance, true);
        log::debug!(
            "[MoverLight] probe channel bounce: mover #{} mid pose {}: bounce one {:.2} s, {} \
             later bounce(s) over {} of {} static patch(es) and {} posed {:.2} s, gather of {} \
             probe(s) {:.2} s",
            self.job.mover_id,
            scene.pose,
            (one_done - started).as_secs_f64(),
            bounces.saturating_sub(1),
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
