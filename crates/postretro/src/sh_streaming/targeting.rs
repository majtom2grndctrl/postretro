//! Target selection, owner closure, hysteresis, and request priority.
//! See: context/lib/rendering_pipeline.md §4

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use postretro_visibility::VisibleCells;

use super::*;

impl ShResidencyController {
    /// Uses monotonic render seconds, not a frame count, so the two-second
    /// retention window is identical at 30, 60, and 144 Hz. The horizon is
    /// `visible ∪ warm`; the warm set follows `camera_cell` alone, so view
    /// rotation moves only visible-class targets.
    pub(crate) fn update_targets(
        &mut self,
        visible: &VisibleCells,
        camera_cell: Option<usize>,
        monotonic_seconds: f64,
    ) -> Result<(), ShResidencyControllerError> {
        self.validate_time(monotonic_seconds)?;
        let visible = self.visible_clusters(visible)?;
        self.refresh_warm_set(camera_cell)?;
        self.record_visible_misses(&visible)?;
        let horizon: BTreeSet<u32> = visible
            .iter()
            .copied()
            .chain(self.warm.clusters())
            .collect();
        let raw_departures: Vec<_> = self.last_horizon.difference(&horizon).copied().collect();
        let horizon_changed = horizon != self.last_horizon;
        if horizon_changed {
            self.horizon_revision = self.horizon_revision.checked_add(1).ok_or(
                ShResidencyControllerError::AccountingOverflow("horizon revision"),
            )?;
            self.last_horizon = horizon.clone();
        }
        if horizon_changed
            || self.optional_pressure_has_cleared(&visible, &horizon, monotonic_seconds)?
        {
            self.clear_optional_suppression();
        }
        for cluster_id in raw_departures {
            self.states[cluster_id as usize].hysteresis_started_at = Some(monotonic_seconds);
        }
        for &cluster_id in &horizon {
            self.states[cluster_id as usize].hysteresis_started_at = None;
        }

        let mut classes =
            self.unsuppressed_target_classes(&visible, &horizon, monotonic_seconds)?;
        let seam_warm: BTreeSet<_> = classes
            .iter()
            .filter_map(|(&cluster_id, directive)| {
                (directive.class == TargetClass::SeamWarm).then_some(cluster_id)
            })
            .collect();
        let seam_activated: Vec<_> = seam_warm
            .difference(&self.last_seam_warm)
            .copied()
            .collect();
        for cluster_id in seam_activated {
            self.clear_suppression_for_owner_closure(cluster_id)?;
        }
        self.last_seam_warm = seam_warm;

        classes.retain(|cluster_id, directive| {
            !directive.class.is_pressure_eligible() || !self.states[*cluster_id as usize].suppressed
        });

        let mut targets: BTreeSet<u32> = classes.keys().copied().collect();
        self.close_owner_targets(&mut targets, &mut classes)?;
        self.transition_failed_retries(&targets)?;
        self.drop_departed_ready(&targets)?;
        for (&cluster_id, directive) in &classes {
            let state = &mut self.states[cluster_id as usize];
            if !directive.class.is_pressure_eligible() {
                // A prior optional classification cannot leave newly visible,
                // pinned, or retained work suppressed. These classes never
                // yield to pressure.
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
        Ok(())
    }

    /// Suppression survives ordinary frames so an over-budget doorway does not
    /// oscillate at the render cadence. It can clear without a horizon change
    /// only when the complete target set that clearing would restore, including
    /// live hysteresis and transitive owners, fits the nominal pool.
    fn optional_pressure_has_cleared(
        &self,
        visible: &BTreeSet<u32>,
        horizon: &BTreeSet<u32>,
        monotonic_seconds: f64,
    ) -> Result<bool, ShResidencyControllerError> {
        if !self.states.iter().any(|state| state.suppressed) {
            return Ok(false);
        }

        let mut classes = self.unsuppressed_target_classes(visible, horizon, monotonic_seconds)?;
        let mut targets: BTreeSet<_> = classes.keys().copied().collect();
        self.close_owner_targets(&mut targets, &mut classes)?;
        let demand = targets.iter().try_fold(0u64, |total, &cluster_id| {
            total
                .checked_add(self.topology.requested_resident_bytes[cluster_id as usize])
                .ok_or(ShResidencyControllerError::AccountingOverflow(
                    "optional pressure target demand",
                ))
        })?;
        Ok(demand <= self.accounting.nominal_cluster_bytes()?)
    }

    fn unsuppressed_target_classes(
        &self,
        visible: &BTreeSet<u32>,
        horizon: &BTreeSet<u32>,
        monotonic_seconds: f64,
    ) -> Result<BTreeMap<u32, TargetDirective>, ShResidencyControllerError> {
        let mut classes = BTreeMap::new();
        for &cluster_id in visible {
            Self::merge_directive(
                &mut classes,
                cluster_id,
                TargetDirective::new(TargetClass::Visible, 0),
            );
        }
        for &cluster_id in &self.topology.pinned_clusters {
            Self::merge_directive(
                &mut classes,
                cluster_id,
                TargetDirective::new(TargetClass::Pinned, 0),
            );
        }
        for seam in &self.topology.seam_portals {
            let far = if visible.contains(&seam.front_cluster_id) {
                Some(seam.back_cluster_id)
            } else if visible.contains(&seam.back_cluster_id) {
                Some(seam.front_cluster_id)
            } else {
                None
            };
            if let Some(cluster_id) = far {
                let priority = self.authored_priority(cluster_id)?;
                Self::merge_directive(
                    &mut classes,
                    cluster_id,
                    TargetDirective::new(TargetClass::SeamWarm, priority),
                );
            }
        }
        for &cluster_id in horizon {
            let priority = self.authored_priority(cluster_id)?;
            Self::merge_directive(
                &mut classes,
                cluster_id,
                TargetDirective::new(TargetClass::Prefetch, priority),
            );
        }
        for (cluster_id, state) in self.states.iter().enumerate() {
            let cluster_id = cluster_id as u32;
            if horizon.contains(&cluster_id) {
                continue;
            }
            if state
                .hysteresis_started_at
                .is_some_and(|started_at| monotonic_seconds - started_at < HYSTERESIS_SECONDS)
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
    /// hide the useful question: how often did a visible cluster lack a
    /// sampleable resident closure?
    fn record_visible_misses(
        &mut self,
        visible: &BTreeSet<u32>,
    ) -> Result<(), ShResidencyControllerError> {
        self.prior_visible_misses.retain(|id| visible.contains(id));
        for &cluster_id in visible {
            if self.states[cluster_id as usize].state != ClusterResidencyState::Sampleable
                && self.prior_visible_misses.insert(cluster_id)
            {
                Self::increment_counter(&mut self.counters.misses, "visible misses")?;
            }
        }
        Ok(())
    }

    fn validate_time(&self, time: f64) -> Result<(), ShResidencyControllerError> {
        if !time.is_finite() || time < 0.0 || self.last_time.is_some_and(|last| time < last) {
            return Err(ShResidencyControllerError::InvalidMonotonicTime);
        }
        Ok(())
    }

    fn visible_clusters(
        &self,
        visible: &VisibleCells,
    ) -> Result<BTreeSet<u32>, ShResidencyControllerError> {
        match visible {
            VisibleCells::DrawAll => Ok((0..self.topology.cluster_count() as u32).collect()),
            VisibleCells::Culled(cells) => cells
                .iter()
                .map(|&cell_id| {
                    self.topology
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

    fn refresh_warm_set(
        &mut self,
        camera_cell: Option<usize>,
    ) -> Result<(), ShResidencyControllerError> {
        if self.warm.camera_cell() != camera_cell {
            self.warm = self.warm_source.warm_set(&self.topology, camera_cell)?;
        }
        Ok(())
    }

    fn close_owner_targets(
        &self,
        targets: &mut BTreeSet<u32>,
        classes: &mut BTreeMap<u32, TargetDirective>,
    ) -> Result<(), ShResidencyControllerError> {
        let mut queue: VecDeque<_> = classes
            .iter()
            .map(|(&cluster_id, &directive)| (cluster_id, directive))
            .collect();
        while let Some((cluster_id, dependent_directive)) = queue.pop_front() {
            let owners = self
                .topology
                .owners
                .get(cluster_id as usize)
                .ok_or_else(|| {
                    ShResidencyControllerError::InvalidTopology("owner map is incomplete".into())
                })?;
            for &owner in owners {
                if owner as usize >= self.topology.cluster_count() {
                    return Err(ShResidencyControllerError::InvalidTopology(
                        "owner map names an out-of-range cluster".into(),
                    ));
                }
                targets.insert(owner);
                // An optional dependent carries its effective priority into
                // the owner closure, but the owner's own authored priority
                // can only strengthen that same optional class. Protected
                // classes deliberately remain priority zero.
                let owner_directive = TargetDirective::new(
                    dependent_directive.class,
                    if dependent_directive.class.is_pressure_eligible() {
                        dependent_directive
                            .effective_priority
                            .max(self.authored_priority(owner)?)
                    } else {
                        0
                    },
                );
                let should_visit = match classes.entry(owner) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(owner_directive);
                        true
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry)
                        if owner_directive.supersedes(*entry.get()) =>
                    {
                        entry.insert(owner_directive);
                        true
                    }
                    std::collections::btree_map::Entry::Occupied(_) => false,
                };
                if should_visit {
                    queue.push_back((owner, owner_directive));
                }
            }
        }
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
            .authored_priorities
            .get(cluster_id as usize)
            .copied()
            .ok_or_else(|| {
                ShResidencyControllerError::InvalidTopology(
                    "target cluster exceeds authored priority table".into(),
                )
            })
    }

    fn clear_suppression_for_owner_closure(
        &mut self,
        cluster_id: u32,
    ) -> Result<(), ShResidencyControllerError> {
        let mut pending = VecDeque::from([cluster_id]);
        let mut seen = BTreeSet::new();
        while let Some(cluster_id) = pending.pop_front() {
            if !seen.insert(cluster_id) {
                continue;
            }
            let state = self.states.get_mut(cluster_id as usize).ok_or_else(|| {
                ShResidencyControllerError::InvalidTopology(
                    "seam target exceeds controller state".into(),
                )
            })?;
            state.suppressed = false;
            let owners = self
                .topology
                .owners
                .get(cluster_id as usize)
                .ok_or_else(|| {
                    ShResidencyControllerError::InvalidTopology("owner map is incomplete".into())
                })?;
            pending.extend(owners.iter().copied());
        }
        Ok(())
    }

    fn transition_failed_retries(
        &mut self,
        targets: &BTreeSet<u32>,
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
            if failure.identity == expected_identity
                && failure.left_target
                && !failure.retry_spent
                && failure
                    .left_target_horizon_revision
                    .is_some_and(|left_revision| self.horizon_revision > left_revision)
            {
                failure.retry_spent = true;
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

    pub(super) fn next_request_cluster(&self) -> Result<Option<u32>, ShResidencyControllerError> {
        let mut candidates: Vec<_> = self
            .targets
            .iter()
            .copied()
            .filter(|&cluster_id| {
                self.states[cluster_id as usize].state == ClusterResidencyState::Absent
            })
            .collect();
        // Authored priority outranks warm distance.
        candidates.sort_by_key(|&cluster_id| {
            (
                self.states[cluster_id as usize]
                    .class
                    .unwrap_or(TargetClass::Hysteresis),
                std::cmp::Reverse(self.states[cluster_id as usize].effective_priority),
                self.warm_rank(cluster_id),
                cluster_id,
            )
        });
        for cluster_id in candidates {
            if let Some(next) = self.first_missing_owner(cluster_id, &mut BTreeSet::new())? {
                return Ok(Some(next));
            }
        }
        Ok(None)
    }

    /// `visiting` holds only the current path, so every return pops this
    /// cluster; an owner reached again through a sibling branch is a diamond,
    /// not a cycle.
    fn first_missing_owner(
        &self,
        cluster_id: u32,
        visiting: &mut BTreeSet<u32>,
    ) -> Result<Option<u32>, ShResidencyControllerError> {
        if !visiting.insert(cluster_id) {
            return Err(ShResidencyControllerError::OwnerCycle(cluster_id));
        }
        let missing = self.first_missing_owner_on_path(cluster_id, visiting);
        visiting.remove(&cluster_id);
        missing
    }

    fn first_missing_owner_on_path(
        &self,
        cluster_id: u32,
        visiting: &mut BTreeSet<u32>,
    ) -> Result<Option<u32>, ShResidencyControllerError> {
        for &owner in &self.topology.owners[cluster_id as usize] {
            match self.states[owner as usize].state {
                // An absent owner either yields the next cluster to request or
                // is itself blocked behind in-flight work; either way its
                // dependents wait rather than being requested ahead of it.
                ClusterResidencyState::Absent => return self.first_missing_owner(owner, visiting),
                ClusterResidencyState::Sampleable => {}
                ClusterResidencyState::Failed
                | ClusterResidencyState::Queued
                | ClusterResidencyState::Ready
                | ClusterResidencyState::InstalledUncomposed => return Ok(None),
            }
        }
        Ok(
            (self.states[cluster_id as usize].state == ClusterResidencyState::Absent)
                .then_some(cluster_id),
        )
    }
}
