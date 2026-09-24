//! Budget-pressure suppression and departed-resident eviction order.
//! See: context/lib/rendering_pipeline.md §4

use std::cmp::Ordering;
use std::collections::BTreeSet;

use super::*;

impl ShResidencyController {
    /// Enforce the CPU-side policy budget without pretending it is a second
    /// GPU allocation. Active pool capacity is renderer-owned; this budget
    /// only ranks logical working-set demand to decide which cold optional work may
    /// be suppressed before the renderer has to grow a pool.
    pub(super) fn apply_budget_policy(&mut self) -> Result<(), ShResidencyControllerError> {
        let nominal = self.accounting.nominal_cluster_bytes()?;
        let mut projected = self.projected_logical_demand()?;

        // Only cold optional work is pressure-eligible. A target owner is
        // pinned while any other target still depends on it; owner closure is
        // then preserved before the renderer independently checks installed
        // dependencies at the release boundary.
        // Recompute eligibility after every suppression. A prefetch owner may
        // initially be pinned only by a colder prefetch dependent; once that
        // dependent leaves, the owner is eligible in this same bounded drain
        // rather than forcing an avoidable growth/extra-frame residency.
        while projected > nominal {
            let Some(cluster_id) = self
                .targets
                .iter()
                .copied()
                .filter(|&cluster_id| self.pressure_evictable_optional(cluster_id))
                .min_by(|left, right| self.compare_pressure_keys(*left, *right))
            else {
                break;
            };
            let state = self.states[cluster_id as usize].state;
            let released = match state {
                ClusterResidencyState::Sampleable => {
                    self.topology.requested_resident_bytes[cluster_id as usize]
                }
                ClusterResidencyState::Ready => self.ready.get(&cluster_id).map_or(0, |_ready| {
                    self.topology.requested_resident_bytes[cluster_id as usize]
                }),
                // A just-installed cluster is deliberately protected. A
                // queued/absent request has no logical occupancy yet.
                ClusterResidencyState::InstalledUncomposed
                | ClusterResidencyState::Queued
                | ClusterResidencyState::Absent
                | ClusterResidencyState::Failed => 0,
            };
            {
                let state = &mut self.states[cluster_id as usize];
                state.suppressed = true;
                state.class = None;
                if let Some(failure) = &mut state.failure {
                    failure.left_target = true;
                    failure.left_target_horizon_revision = Some(self.horizon_revision);
                }
            }
            self.targets.remove(&cluster_id);
            projected = projected.saturating_sub(released);
        }
        // Owner safety can change as optional dependents yield. Report only
        // the demand still protected after the final target set is selected.
        let protected = self.non_evictable_target_bytes()?;
        self.set_non_evictable_overshoot(protected.saturating_sub(nominal));
        Ok(())
    }

    fn non_evictable_target_bytes(&self) -> Result<u64, ShResidencyControllerError> {
        self.targets.iter().try_fold(0u64, |total, &cluster_id| {
            // Optional work normally yields to pressure, but it becomes
            // non-evictable when it was just installed or is an owner still
            // needed by another target. Count that pinned work with visible,
            // resident, and hysteresis targets so a real floor overage is
            // never hidden behind its current class.
            if self.pressure_evictable_optional(cluster_id) {
                return Ok(total);
            }
            total
                .checked_add(self.topology.requested_resident_bytes[cluster_id as usize])
                .ok_or(ShResidencyControllerError::AccountingOverflow(
                    "non-evictable logical demand",
                ))
        })
    }

    fn projected_logical_demand(&self) -> Result<u64, ShResidencyControllerError> {
        self.targets.iter().try_fold(0u64, |total, &cluster_id| {
            match self.states[cluster_id as usize].state {
                ClusterResidencyState::Sampleable | ClusterResidencyState::InstalledUncomposed => {
                    total
                        .checked_add(self.topology.requested_resident_bytes[cluster_id as usize])
                        .ok_or(ShResidencyControllerError::AccountingOverflow(
                            "projected logical demand",
                        ))
                }
                ClusterResidencyState::Ready => total
                    .checked_add(self.topology.requested_resident_bytes[cluster_id as usize])
                    .ok_or(ShResidencyControllerError::AccountingOverflow(
                        "projected logical demand",
                    )),
                _ => Ok(total),
            }
        })
    }

    fn pressure_evictable_optional(&self, cluster_id: u32) -> bool {
        let state = &self.states[cluster_id as usize];
        if !state.class.is_some_and(TargetClass::is_pressure_eligible)
            || state.state == ClusterResidencyState::InstalledUncomposed
        {
            return false;
        }
        !self.targets.iter().any(|&dependent| {
            dependent != cluster_id
                && self.topology.owners[dependent as usize].contains(&cluster_id)
        })
    }

    pub(super) fn departed_eviction_order(&self) -> Vec<u32> {
        let eligible: BTreeSet<_> = self
            .states
            .iter()
            .enumerate()
            .filter_map(|(cluster_id, state)| {
                (state.state == ClusterResidencyState::Sampleable
                    && !self.targets.contains(&(cluster_id as u32))
                    && !self.topology.pinned_clusters.contains(&(cluster_id as u32))
                    && !matches!(
                        state.class,
                        Some(TargetClass::Visible | TargetClass::Pinned)
                    ))
                .then_some(cluster_id as u32)
            })
            .collect();
        // Expired hysteresis departures are released first. Pressure-only
        // evictions are the remaining optional work; victim selection already
        // used the pressure comparator. Release IDs follow the canonical
        // dependency-safe drain ordering, not that selection sequence.
        let departed: BTreeSet<_> = eligible
            .iter()
            .copied()
            .filter(|&cluster_id| !self.states[cluster_id as usize].suppressed)
            .collect();
        let pressured: BTreeSet<_> = eligible.difference(&departed).copied().collect();
        let mut ordered = self.dependent_first_eviction_order(departed);
        ordered.extend(self.dependent_first_eviction_order(pressured));
        ordered
    }

    fn dependent_first_eviction_order(&self, mut remaining: BTreeSet<u32>) -> Vec<u32> {
        let mut ordered = Vec::with_capacity(remaining.len());
        while !remaining.is_empty() {
            let mut leaves: Vec<_> = remaining
                .iter()
                .copied()
                .filter(|&candidate| {
                    !remaining.iter().any(|&dependent| {
                        dependent != candidate
                            && self.topology.owners[dependent as usize].contains(&candidate)
                    })
                })
                .collect();
            // Planner topology rejects owner cycles. The deterministic fallback
            // still avoids an infinite loop if a malformed test fixture slips
            // through a future construction path.
            if leaves.is_empty() {
                leaves.extend(remaining.iter().copied());
            }
            leaves.sort_by(|left, right| self.compare_eviction_keys(*left, *right));
            for cluster_id in leaves {
                remaining.remove(&cluster_id);
                ordered.push(cluster_id);
            }
        }
        ordered
    }

    fn set_non_evictable_overshoot(&mut self, bytes: u64) {
        self.non_evictable_overshoot_bytes = bytes;
        if bytes == 0 {
            self.overshoot_reported = false;
        } else if !self.overshoot_reported {
            self.overshoot_reported = true;
            log::warn!(
                "[SH streaming] non-evictable logical demand exceeds the effective floor by {bytes} bytes"
            );
        }
    }

    fn compare_eviction_keys(&self, left: u32, right: u32) -> Ordering {
        match (self.eviction_key(left), self.eviction_key(right)) {
            (Some(left), Some(right)) => left
                .last_visible_time
                .total_cmp(&right.last_visible_time)
                .then_with(|| left.last_target_time.total_cmp(&right.last_target_time))
                .then_with(|| left.cluster_id.cmp(&right.cluster_id)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => left.cmp(&right),
        }
    }

    /// Pressure yields by class, then lower authored priority, then the
    /// farthest warm cluster (non-warm counts as farthest), then LRU. Expired
    /// departures deliberately keep `compare_eviction_keys` above, because
    /// they are no longer policy work.
    fn compare_pressure_keys(&self, left: u32, right: u32) -> Ordering {
        self.pressure_class_rank(left)
            .cmp(&self.pressure_class_rank(right))
            .then_with(|| {
                self.states[left as usize]
                    .effective_priority
                    .cmp(&self.states[right as usize].effective_priority)
            })
            .then_with(|| self.warm_rank(right).cmp(&self.warm_rank(left)))
            .then_with(|| self.compare_eviction_keys(left, right))
    }

    fn pressure_class_rank(&self, cluster_id: u32) -> u8 {
        match self.states[cluster_id as usize].class {
            Some(TargetClass::Prefetch) => 0,
            Some(TargetClass::SeamWarm) => 1,
            // Callers only request a rank for pressure-eligible candidates;
            // retain a total key for defensive test construction.
            _ => 2,
        }
    }

    pub(crate) fn eviction_key(&self, cluster_id: u32) -> Option<ShEvictionKey> {
        let state = self.states.get(cluster_id as usize)?;
        Some(ShEvictionKey {
            last_visible_time: state.last_visible_time.unwrap_or(f64::NEG_INFINITY),
            last_target_time: state.last_target_time.unwrap_or(f64::NEG_INFINITY),
            cluster_id,
        })
    }
}
