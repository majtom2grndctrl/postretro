//! Target selection, hysteresis, and request priority.
//! See: context/lib/rendering_pipeline.md §4

use std::collections::{BTreeMap, BTreeSet};

use postretro_visibility::VisibleCells;

use crate::streaming::cell_demand::DemandFrame;

use super::*;

/// The one bucket a visible miss lands in; see `ShMissBuckets`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MissCause {
    OutsideReach,
    TrimmedByPressure,
    ReadInFlight,
    HeldByDrainBudget,
    AwaitingCompose,
    Failed,
    DeviceLimited,
}

impl ShResidencyController {
    /// Uses monotonic render seconds, not a frame count, so the two-second
    /// retention window is identical at 30, 60, and 144 Hz. The horizon is
    /// `visible ∪ reach`. The reach is the cell-demand stage's id-51 set for
    /// the camera cell, `None` without a usable id 51: it follows the camera
    /// cell and L alone, so view rotation moves only visible-class targets.
    /// `visible` is what targeting may read; the frame's drawn set, which can
    /// hold more, rides on `reach`. A non-portal frame holds (see `hold.rs`).
    pub(crate) fn update_targets(
        &mut self,
        visible: &VisibleCells,
        reach: Option<DemandFrame<'_>>,
        monotonic_seconds: f64,
    ) -> Result<(), ShResidencyControllerError> {
        self.validate_time(monotonic_seconds)?;
        let hysteresis_seconds = self.begin_frame_hold(reach.as_ref(), monotonic_seconds);
        let visible = self.frame_visible_clusters(visible, reach.as_ref())?;
        self.reach
            .update(reach, &self.topology.hints.cell_to_cluster)?;
        let horizon: BTreeSet<u32> = visible
            .iter()
            .copied()
            .chain(self.reach.clusters().map(|(cluster, _)| cluster))
            .collect();
        let raw_departures: Vec<_> = self.last_horizon.difference(&horizon).copied().collect();
        let horizon_changed = horizon != self.last_horizon;
        if horizon_changed {
            self.horizon_revision = self.horizon_revision.checked_add(1).ok_or(
                ShResidencyControllerError::AccountingOverflow("horizon revision"),
            )?;
            self.last_horizon = horizon.clone();
        }
        if horizon_changed {
            self.clear_optional_suppression();
        }
        for cluster_id in raw_departures {
            self.states[cluster_id as usize].hysteresis_started_at = Some(hysteresis_seconds);
        }
        for &cluster_id in &horizon {
            self.states[cluster_id as usize].hysteresis_started_at = None;
        }

        let mut classes =
            self.unsuppressed_target_classes(&visible, &horizon, hysteresis_seconds)?;
        let seam_warm: BTreeSet<_> = classes
            .iter()
            .filter_map(|(&cluster_id, directive)| {
                (directive.class == TargetClass::SeamWarm).then_some(cluster_id)
            })
            .collect();
        for &cluster_id in seam_warm.difference(&self.last_seam_warm) {
            self.states[cluster_id as usize].suppressed = false;
        }
        self.last_seam_warm = seam_warm;

        classes.retain(|cluster_id, directive| {
            !directive.class.is_optional() || !self.states[*cluster_id as usize].suppressed
        });

        let targets: BTreeSet<u32> = classes.keys().copied().collect();
        self.transition_failed_retries(&targets, monotonic_seconds, horizon_changed)?;
        self.drop_departed_ready(&targets)?;
        for (&cluster_id, directive) in &classes {
            let state = &mut self.states[cluster_id as usize];
            if !directive.class.is_optional() {
                // A prior optional classification cannot leave newly visible,
                // pinned, or lead work suppressed: these classes never yield.
                state.suppressed = false;
            }
            state.class = Some(directive.class);
            state.effective_priority = directive.effective_priority;
            // Hysteresis preserves raw-horizon departure time separately;
            // target time remains the LRU tie-break input.
            if directive.class != TargetClass::Hysteresis {
                state.last_target_time = Some(monotonic_seconds);
            }
            if directive.class == TargetClass::Visible {
                state.last_visible_time = Some(monotonic_seconds);
            }
        }
        for &cluster_id in self.targets.difference(&targets) {
            self.states[cluster_id as usize].class = None;
            self.states[cluster_id as usize].effective_priority = 0;
            if let Some(failure) = &mut self.states[cluster_id as usize].failure {
                failure.left_target = true;
                failure.left_target_horizon_revision = Some(self.horizon_revision);
            }
        }
        self.targets = targets;
        self.last_time = Some(monotonic_seconds);
        self.demand_updated = true;
        Ok(())
    }

    fn unsuppressed_target_classes(
        &self,
        visible: &BTreeSet<u32>,
        horizon: &BTreeSet<u32>,
        hysteresis_seconds: f64,
    ) -> Result<BTreeMap<u32, TargetDirective>, ShResidencyControllerError> {
        let mut classes = BTreeMap::new();
        for &cluster_id in visible {
            Self::merge_directive(
                &mut classes,
                cluster_id,
                TargetDirective::new(TargetClass::Visible, 0),
            );
        }
        for &cluster_id in &self.topology.hints.pinned {
            Self::merge_directive(
                &mut classes,
                cluster_id,
                TargetDirective::new(TargetClass::Pinned, 0),
            );
        }
        if self.is_held() {
            // No seam warms from what a held frame draws, or a frustum-culled
            // path would read every seam's far side. The warm set stands
            // instead: a resident outside the horizon has no hysteresis to
            // keep it.
            for &cluster_id in &self.last_seam_warm {
                if self.may_target_without_reading(cluster_id) {
                    self.merge_seam_warm(&mut classes, cluster_id)?;
                }
            }
        } else {
            for seam in &self.topology.seam_portals {
                let far = if visible.contains(&seam.front_cluster_id) {
                    Some(seam.back_cluster_id)
                } else if visible.contains(&seam.back_cluster_id) {
                    Some(seam.front_cluster_id)
                } else {
                    None
                };
                if let Some(cluster_id) = far {
                    self.merge_seam_warm(&mut classes, cluster_id)?;
                }
            }
        }
        for (cluster_id, lead) in self.reach.clusters() {
            let directive = if self.reach.is_lead(lead) {
                TargetDirective::new(TargetClass::Lead, 0)
            } else {
                TargetDirective::new(TargetClass::Band, self.authored_priority(cluster_id)?)
            };
            Self::merge_directive(&mut classes, cluster_id, directive);
        }
        for (cluster_id, state) in self.states.iter().enumerate() {
            let cluster_id = cluster_id as u32;
            if horizon.contains(&cluster_id) {
                continue;
            }
            if state
                .hysteresis_started_at
                .is_some_and(|started_at| hysteresis_seconds - started_at < HYSTERESIS_SECONDS)
                && self.may_target_without_reading(cluster_id)
            {
                Self::merge_directive(
                    &mut classes,
                    cluster_id,
                    TargetDirective::new(TargetClass::Hysteresis, 0),
                );
            }
        }
        Ok(classes)
    }

    /// Count a miss once for each continuous visible episode. Counting every
    /// render frame would turn refresh rate into a diagnostic input and would
    /// hide the useful question: how often did a drawn cluster lack sampleable
    /// residency?
    ///
    /// `drawn` is the frame's real drawn set, whatever targeting may read of
    /// it. Each miss lands in one bucket, read from the cluster's state before
    /// this frame retargets it.
    pub(super) fn record_visible_misses(
        &mut self,
        drawn: &BTreeSet<u32>,
    ) -> Result<(), ShResidencyControllerError> {
        self.prior_visible_misses.retain(|id| drawn.contains(id));
        for &cluster_id in drawn {
            if self.states[cluster_id as usize].state != ClusterResidencyState::Sampleable
                && self.prior_visible_misses.insert(cluster_id)
            {
                Self::increment_counter(&mut self.counters.misses, "visible misses")?;
                let bucket = self.miss_bucket(cluster_id);
                Self::increment_counter(bucket, "visible miss bucket")?;
            }
        }
        Ok(())
    }

    fn miss_bucket(&mut self, cluster_id: u32) -> &mut u64 {
        let cause = self.miss_cause(cluster_id);
        let buckets = &mut self.counters.miss_buckets;
        match cause {
            MissCause::OutsideReach => &mut buckets.outside_reach,
            MissCause::TrimmedByPressure => &mut buckets.trimmed_by_pressure,
            MissCause::ReadInFlight => &mut buckets.read_in_flight,
            MissCause::HeldByDrainBudget => &mut buckets.held_by_drain_budget,
            MissCause::AwaitingCompose => &mut buckets.awaiting_compose,
            MissCause::Failed => &mut buckets.failed,
            MissCause::DeviceLimited => &mut buckets.device_limited,
        }
    }

    /// The cluster's own state names the cause: no cluster waits on another.
    /// An absent drawn cluster a held frame will not read reads as outside
    /// the reach, because only the reach reads there.
    fn miss_cause(&self, cluster_id: u32) -> MissCause {
        let state = &self.states[cluster_id as usize];
        match state.state {
            ClusterResidencyState::Failed => self.failure_miss_cause(cluster_id),
            ClusterResidencyState::InstalledUncomposed => MissCause::AwaitingCompose,
            ClusterResidencyState::Queued => MissCause::ReadInFlight,
            ClusterResidencyState::Ready => MissCause::HeldByDrainBudget,
            ClusterResidencyState::Absent | ClusterResidencyState::Sampleable => {
                if state.suppressed {
                    MissCause::TrimmedByPressure
                } else if self.targets.contains(&cluster_id) {
                    // Targeted and waiting for a permit or the issuer.
                    MissCause::ReadInFlight
                } else {
                    MissCause::OutsideReach
                }
            }
        }
    }

    fn failure_miss_cause(&self, cluster_id: u32) -> MissCause {
        if self.states[cluster_id as usize]
            .failure
            .is_some_and(|failure| matches!(failure.cause, FailureCause::DeviceLimit { .. }))
        {
            MissCause::DeviceLimited
        } else {
            MissCause::Failed
        }
    }

    fn validate_time(&self, time: f64) -> Result<(), ShResidencyControllerError> {
        if !time.is_finite() || time < 0.0 || self.last_time.is_some_and(|last| time < last) {
            return Err(ShResidencyControllerError::InvalidMonotonicTime);
        }
        Ok(())
    }

    pub(super) fn visible_clusters(
        &self,
        visible: &VisibleCells,
    ) -> Result<BTreeSet<u32>, ShResidencyControllerError> {
        match visible {
            VisibleCells::DrawAll => Ok((0..self.topology.cluster_count() as u32).collect()),
            VisibleCells::Culled(cells) => cells
                .iter()
                .map(|&cell_id| {
                    self.topology
                        .hints
                        .cell_to_cluster
                        .get(cell_id as usize)
                        .copied()
                        .ok_or_else(|| {
                            ShResidencyControllerError::InvalidTopology(format!(
                                "visible cell {cell_id} is outside the id-49 cell map"
                            ))
                        })
                })
                .collect(),
        }
    }

    fn merge_seam_warm(
        &self,
        classes: &mut BTreeMap<u32, TargetDirective>,
        cluster_id: u32,
    ) -> Result<(), ShResidencyControllerError> {
        let priority = self.authored_priority(cluster_id)?;
        Self::merge_directive(
            classes,
            cluster_id,
            TargetDirective::new(TargetClass::SeamWarm, priority),
        );
        Ok(())
    }

    fn merge_directive(
        classes: &mut BTreeMap<u32, TargetDirective>,
        cluster_id: u32,
        candidate: TargetDirective,
    ) {
        match classes.get_mut(&cluster_id) {
            Some(current) if candidate.supersedes(*current) => *current = candidate,
            Some(_) => {}
            None => {
                classes.insert(cluster_id, candidate);
            }
        }
    }

    fn authored_priority(&self, cluster_id: u32) -> Result<u32, ShResidencyControllerError> {
        self.topology
            .hints
            .priority
            .get(cluster_id as usize)
            .map(|&priority| u32::from(priority))
            .ok_or_else(|| {
                ShResidencyControllerError::InvalidTopology(
                    "target cluster exceeds authored priority table".into(),
                )
            })
    }

    fn transition_failed_retries(
        &mut self,
        targets: &BTreeSet<u32>,
        monotonic_seconds: f64,
        horizon_changed: bool,
    ) -> Result<(), ShResidencyControllerError> {
        let mut retries = 0u64;
        for (cluster_id, state) in self.states.iter_mut().enumerate() {
            if state.state != ClusterResidencyState::Failed
                || !targets.contains(&(cluster_id as u32))
            {
                continue;
            }
            let Some(failure) = &mut state.failure else {
                continue;
            };
            let expected_identity = FailureIdentity {
                generation: self.generation,
                content_tag: self.content_tag,
                cluster_id: cluster_id as u32,
                chunk_hash: self.topology.chunk_hashes[cluster_id],
            };
            let retry_allowed = match failure.cause {
                FailureCause::Read => !failure.retry_spent,
                FailureCause::DeviceLimit { refused_at } => {
                    horizon_changed
                        && !self.targets.contains(&(cluster_id as u32))
                        && monotonic_seconds - refused_at >= HYSTERESIS_SECONDS
                }
            };
            if failure.identity == expected_identity
                && failure.left_target
                && retry_allowed
                && failure
                    .left_target_horizon_revision
                    .is_some_and(|left_revision| self.horizon_revision > left_revision)
            {
                if failure.cause == FailureCause::Read {
                    failure.retry_spent = true;
                }
                failure.left_target = false;
                failure.left_target_horizon_revision = None;
                state.state = ClusterResidencyState::Absent;
                retries = retries.checked_add(1).ok_or(
                    ShResidencyControllerError::AccountingOverflow("stream retries"),
                )?;
            }
        }
        self.counters.retries = self.counters.retries.checked_add(retries).ok_or(
            ShResidencyControllerError::AccountingOverflow("stream retries"),
        )?;
        Ok(())
    }

    pub(super) fn drop_departed_ready(
        &mut self,
        targets: &BTreeSet<u32>,
    ) -> Result<(), ShResidencyControllerError> {
        let departed: Vec<_> = self
            .ready
            .keys()
            .copied()
            .filter(|cluster_id| !targets.contains(cluster_id))
            .collect();
        for cluster_id in departed {
            let ready = self
                .ready
                .remove(&cluster_id)
                .expect("id came from ready map");
            self.release_ready_charge(ready.byte_charge)?;
            self.release_permit()?;
            self.states[cluster_id as usize].state = ClusterResidencyState::Absent;
        }
        Ok(())
    }

    /// The next read, or `None` when nothing may read now. Mandatory classes
    /// sort first and hold the larger permit limit, so if the best candidate
    /// cannot take a permit, nothing behind it can. Authored priority outranks
    /// reach lead.
    pub(super) fn next_request_cluster(&self) -> Option<u32> {
        let cluster_id = self
            .targets
            .iter()
            .copied()
            .filter(|&cluster_id| {
                let state = &self.states[cluster_id as usize];
                state.state == ClusterResidencyState::Absent && self.may_read(state.class)
            })
            .min_by_key(|&cluster_id| {
                let state = &self.states[cluster_id as usize];
                (
                    state.class.unwrap_or(TargetClass::Hysteresis),
                    std::cmp::Reverse(state.effective_priority),
                    self.reach_lead(cluster_id),
                    cluster_id,
                )
            })?;
        (self.permits_in_use < self.request_permit_limit(cluster_id)).then_some(cluster_id)
    }
}
