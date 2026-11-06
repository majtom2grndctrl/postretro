//! Capacity-victim order, what happens to a victim, and departed-resident order.
//! The renderer owns capacity truth; this file owns only who goes first.
//! See: context/lib/rendering_pipeline.md §4 (Bounded SH pool)

use std::cmp::Ordering;

use super::*;

impl ShResidencyController {
    pub(super) fn mandatory_work_ready(&self) -> bool {
        self.ready
            .keys()
            .any(|&cluster_id| self.is_mandatory_target(cluster_id))
    }

    /// Every mandatory target, sorted: what growth sizes to, and which ready
    /// items may evict or grow.
    pub(super) fn mandatory_targets(&self) -> Vec<u32> {
        self.targets
            .iter()
            .copied()
            .filter(|&cluster_id| self.is_mandatory_target(cluster_id))
            .collect()
    }

    fn is_mandatory_target(&self, cluster_id: u32) -> bool {
        self.states[cluster_id as usize]
            .class
            .is_some_and(TargetClass::is_mandatory)
    }

    /// Optional residents in eviction order: SeamWarm and Band by the pressure
    /// comparator, then Hysteresis, paused or not, then Held. A just-installed
    /// cluster is protected until it composes, and so is a resident only the
    /// mirror draws. Held residents go last of all: they are what a held frame
    /// draws, but a read the hold still owes matters more.
    pub(super) fn capacity_victims(&self) -> Vec<u32> {
        let mut victims: Vec<u32> = self
            .targets
            .iter()
            .copied()
            .filter(|&cluster_id| {
                let state = &self.states[cluster_id as usize];
                state.state == ClusterResidencyState::Sampleable
                    && state.class.is_some_and(TargetClass::is_optional)
                    && !self.drawn_outside_targets.contains(&cluster_id)
            })
            .collect();
        victims.sort_by(|&left, &right| self.compare_pressure_keys(left, right));
        victims
    }

    pub(super) fn may_evict_this_drain(&self, cluster_id: u32) -> bool {
        self.in_drain_evictions.contains(&cluster_id) || self.in_drain_victims.contains(&cluster_id)
    }

    /// A cluster that lost its place to capacity leaves the targets until the
    /// horizon changes, so it cannot thrash at the render cadence. Its
    /// hysteresis ends here too: the room it held is what was wanted.
    pub(super) fn yield_capacity_victim(&mut self, cluster_id: u32) {
        let state = &mut self.states[cluster_id as usize];
        state.suppressed = true;
        state.class = None;
        state.effective_priority = 0;
        state.hysteresis_started_at = None;
        if let Some(failure) = &mut state.failure {
            failure.left_target = true;
            failure.left_target_horizon_revision = Some(self.horizon_revision);
        }
        self.targets.remove(&cluster_id);
    }

    /// Expired hysteresis departures, by the eviction key, whose cluster-id
    /// tie-break keeps the order deterministic.
    pub(super) fn departed_eviction_order(&self) -> Vec<u32> {
        let mut ordered: Vec<u32> = self
            .states
            .iter()
            .enumerate()
            .filter_map(|(cluster_id, state)| {
                (state.state == ClusterResidencyState::Sampleable
                    && !self.targets.contains(&(cluster_id as u32))
                    && !self.topology.hints.pinned.contains(&(cluster_id as u32))
                    && !state.class.is_some_and(TargetClass::is_mandatory))
                .then_some(cluster_id as u32)
            })
            .collect();
        ordered.sort_by(|&left, &right| self.compare_eviction_keys(left, right));
        ordered
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

    /// Yields by class, then lower authored priority, then the farthest reach
    /// lead (outside the reach counts as farthest), then LRU. Expired
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
            .then_with(|| self.reach_lead(right).cmp(&self.reach_lead(left)))
            .then_with(|| self.compare_eviction_keys(left, right))
    }

    fn pressure_class_rank(&self, cluster_id: u32) -> u8 {
        match self.states[cluster_id as usize].class {
            Some(TargetClass::Band) => 0,
            Some(TargetClass::SeamWarm) => 1,
            // What a held frame draws goes after everything else.
            Some(TargetClass::Held) => 3,
            // Hysteresis goes late, because it stops doorway thrash. Callers
            // rank only optional residents; the key stays total regardless.
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
