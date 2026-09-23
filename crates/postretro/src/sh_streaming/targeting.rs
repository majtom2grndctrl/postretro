//! Target selection, owner closure, hysteresis, and request priority.
//! See: context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use postretro_visibility::VisibleCells;

use super::*;

impl ShResidencyController {
    /// Uses monotonic render seconds, not a frame count, so the two-second
    /// retention window is identical at 30, 60, and 144 Hz.
    pub(crate) fn update_targets(
        &mut self,
        visible: &VisibleCells,
        monotonic_seconds: f64,
    ) -> Result<(), ShResidencyControllerError> {
        self.validate_time(monotonic_seconds)?;
        let visible = self.visible_clusters(visible)?;
        let horizon = self.two_hop_horizon(&visible)?;
        let raw_departures: Vec<_> = self.last_horizon.difference(&horizon).copied().collect();
        let horizon_changed = horizon != self.last_horizon;
        if horizon_changed {
            self.horizon_revision = self.horizon_revision.checked_add(1).ok_or(
                ShResidencyControllerError::AccountingOverflow("horizon revision"),
            )?;
            self.last_horizon = horizon.clone();
            for state in &mut self.states {
                state.suppressed = false;
            }
        }
        for cluster_id in raw_departures {
            self.states[cluster_id as usize].hysteresis_started_at = Some(monotonic_seconds);
        }
        for &cluster_id in &horizon {
            self.states[cluster_id as usize].hysteresis_started_at = None;
        }

        let mut classes = BTreeMap::new();
        for &cluster_id in &visible {
            classes.insert(cluster_id, TargetClass::Visible);
        }
        for &cluster_id in &horizon {
            classes.entry(cluster_id).or_insert(TargetClass::Prefetch);
        }
        for (cluster_id, state) in self.states.iter().enumerate() {
            let cluster_id = cluster_id as u32;
            if horizon.contains(&cluster_id) || state.suppressed {
                continue;
            }
            if state
                .hysteresis_started_at
                .is_some_and(|started_at| monotonic_seconds - started_at < HYSTERESIS_SECONDS)
            {
                classes.insert(cluster_id, TargetClass::Hysteresis);
            }
        }
        classes.retain(|cluster_id, class| {
            *class == TargetClass::Visible || !self.states[*cluster_id as usize].suppressed
        });

        let mut targets: BTreeSet<u32> = classes.keys().copied().collect();
        self.close_owner_targets(&mut targets, &mut classes)?;
        self.transition_failed_retries(&targets);
        self.drop_departed_ready(&targets)?;
        for (&cluster_id, class) in &classes {
            let state = &mut self.states[cluster_id as usize];
            state.class = Some(*class);
            // Hysteresis preserves raw-horizon departure time separately;
            // target time remains the LRU tie-break input.
            if *class != TargetClass::Hysteresis {
                state.last_target_time = Some(monotonic_seconds);
            }
            if *class == TargetClass::Visible {
                state.last_visible_time = Some(monotonic_seconds);
            }
        }
        for &cluster_id in self.targets.difference(&targets) {
            self.states[cluster_id as usize].class = None;
            if let Some(failure) = &mut self.states[cluster_id as usize].failure {
                failure.left_target = true;
                failure.left_target_horizon_revision = Some(self.horizon_revision);
            }
        }
        self.targets = targets;
        self.last_time = Some(monotonic_seconds);
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

    fn two_hop_horizon(
        &self,
        visible: &BTreeSet<u32>,
    ) -> Result<BTreeSet<u32>, ShResidencyControllerError> {
        let mut result = visible.clone();
        let mut queue: VecDeque<_> = visible
            .iter()
            .map(|&cluster_id| (cluster_id, 0u8))
            .collect();
        while let Some((cluster_id, depth)) = queue.pop_front() {
            if depth == PREFETCH_HOPS {
                continue;
            }
            let neighbors = self
                .topology
                .adjacency
                .get(cluster_id as usize)
                .ok_or_else(|| {
                    ShResidencyControllerError::InvalidTopology(
                        "visible cluster exceeds adjacency".into(),
                    )
                })?;
            for &neighbor in neighbors {
                if result.insert(neighbor) {
                    queue.push_back((neighbor, depth + 1));
                }
            }
        }
        Ok(result)
    }

    fn close_owner_targets(
        &self,
        targets: &mut BTreeSet<u32>,
        classes: &mut BTreeMap<u32, TargetClass>,
    ) -> Result<(), ShResidencyControllerError> {
        let mut queue: VecDeque<_> = classes
            .iter()
            .map(|(&cluster_id, &class)| (cluster_id, class))
            .collect();
        while let Some((cluster_id, dependent_class)) = queue.pop_front() {
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
                let should_visit = match classes.entry(owner) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(dependent_class);
                        true
                    }
                    std::collections::btree_map::Entry::Occupied(mut entry)
                        if dependent_class < *entry.get() =>
                    {
                        entry.insert(dependent_class);
                        true
                    }
                    std::collections::btree_map::Entry::Occupied(_) => false,
                };
                if should_visit {
                    queue.push_back((owner, dependent_class));
                }
            }
        }
        Ok(())
    }

    fn transition_failed_retries(&mut self, targets: &BTreeSet<u32>) {
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
            }
        }
    }

    fn drop_departed_ready(
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
        candidates.sort_by_key(|&cluster_id| {
            (
                self.states[cluster_id as usize]
                    .class
                    .unwrap_or(TargetClass::Hysteresis),
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

    fn first_missing_owner(
        &self,
        cluster_id: u32,
        visiting: &mut BTreeSet<u32>,
    ) -> Result<Option<u32>, ShResidencyControllerError> {
        if !visiting.insert(cluster_id) {
            return Err(ShResidencyControllerError::OwnerCycle(cluster_id));
        }
        for &owner in &self.topology.owners[cluster_id as usize] {
            match self.states[owner as usize].state {
                ClusterResidencyState::Absent => {
                    if let Some(missing) = self.first_missing_owner(owner, visiting)? {
                        return Ok(Some(missing));
                    }
                }
                ClusterResidencyState::Sampleable => {}
                ClusterResidencyState::Failed
                | ClusterResidencyState::Queued
                | ClusterResidencyState::Ready
                | ClusterResidencyState::InstalledUncomposed => return Ok(None),
            }
        }
        visiting.remove(&cluster_id);
        Ok(
            (self.states[cluster_id as usize].state == ClusterResidencyState::Absent)
                .then_some(cluster_id),
        )
    }
}
