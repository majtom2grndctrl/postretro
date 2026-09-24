//! Loader-to-renderer drain lifecycle and synchronous proof gate.
//! See: context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md

use std::cmp::Ordering;
use std::collections::BTreeSet;

use super::*;

impl ShResidencyController {
    /// Reads at most one target chunk synchronously. This is intentionally a
    /// proof-only entry point; Task 11 replaces production use with workers.
    pub(crate) fn read_one_sync_at_target_time(
        &mut self,
    ) -> Result<SyncReadResult, ShResidencyControllerError> {
        if self.manifest.is_none() {
            return Err(ShResidencyControllerError::MissingManifest);
        }
        let Some(request) = self.take_next_request()? else {
            return Ok(SyncReadResult::NoTargetReady);
        };
        let manifest = self
            .manifest
            .as_ref()
            .ok_or(ShResidencyControllerError::MissingManifest)?;
        let index = &manifest.payloads().index[request.cluster_id as usize];
        self.accounting
            .cpu
            .encoded
            .add(index.payload_len, "encoded bytes")?;
        let decoded = manifest.read_and_decode_cluster(request.cluster_id);
        self.accounting
            .cpu
            .encoded
            .remove(index.payload_len, "encoded bytes")?;
        match decoded {
            Ok(chunk) => {
                self.accounting
                    .cpu
                    .decoding
                    .add(index.decoded_bytes, "decoding bytes")?;
                self.accounting
                    .cpu
                    .decoding
                    .remove(index.decoded_bytes, "decoding bytes")?;
                let admission = self.admit_prepared(PreparedShCluster {
                    generation: request.generation,
                    content_tag: request.content_tag,
                    chunk,
                })?;
                debug_assert_eq!(admission, ShDrainAdmission::Ready);
                Ok(SyncReadResult::Prepared(request.cluster_id))
            }
            Err(error) => {
                let _ = self.mark_failed(request.cluster_id)?;
                Err(ShResidencyControllerError::SourceRead(error))
            }
        }
    }

    /// Reserves one of four lifecycle permits for the next worker or
    /// synchronous source read. Task 11 can feed the returned identity to a
    /// worker without introducing an app-owned type at the renderer boundary.
    pub(crate) fn take_next_request(
        &mut self,
    ) -> Result<Option<ShClusterRequest>, ShResidencyControllerError> {
        if self.permits_in_use >= MAX_STREAM_PERMITS {
            return Ok(None);
        }
        let Some(cluster_id) = self.next_request_cluster()? else {
            return Ok(None);
        };
        self.states[cluster_id as usize].state = ClusterResidencyState::Queued;
        self.permits_in_use = self.permits_in_use.checked_add(1).ok_or(
            ShResidencyControllerError::AccountingOverflow("stream permits"),
        )?;
        Ok(Some(ShClusterRequest {
            generation: self.generation,
            content_tag: self.content_tag,
            cluster_id,
            chunk_hash: self.topology.chunk_hashes[cluster_id as usize],
        }))
    }

    /// A failed worker completion releases exactly the request's permit and
    /// returns whether its identity is eligible for the one warning. A stale
    /// generation or changed chunk identity cannot fail a new request.
    pub(crate) fn admit_failed_request(
        &mut self,
        request: ShClusterRequest,
    ) -> Result<bool, ShResidencyControllerError> {
        if !self.matches_completion_identity(request)
            || self.states[request.cluster_id as usize].state != ClusterResidencyState::Queued
        {
            return Ok(false);
        }
        if !self.targets.contains(&request.cluster_id) {
            self.release_permit()?;
            self.states[request.cluster_id as usize].state = ClusterResidencyState::Absent;
            return Ok(false);
        }
        self.mark_failed(request.cluster_id)
    }

    pub(crate) fn matches_queued_request(&self, request: ShClusterRequest) -> bool {
        self.matches_completion_identity(request)
            && self.targets.contains(&request.cluster_id)
            && self.states[request.cluster_id as usize].state == ClusterResidencyState::Queued
    }

    pub(crate) fn matches_completion_identity(&self, request: ShClusterRequest) -> bool {
        request.generation == self.generation
            && request.content_tag == self.content_tag
            && self.topology.chunk_hashes.get(request.cluster_id as usize)
                == Some(&request.chunk_hash)
    }

    /// Accepts a loader-decoded payload. Stale or departed work is dropped on
    /// the app side before renderer admission, releasing its permit exactly
    /// once. A checked loader decode has already verified the chunk hash.
    pub(crate) fn admit_prepared(
        &mut self,
        prepared: PreparedShCluster,
    ) -> Result<ShDrainAdmission, ShResidencyControllerError> {
        let cluster_id = prepared.chunk.cluster_id;
        let Some(state) = self.states.get(cluster_id as usize) else {
            return Ok(ShDrainAdmission::DroppedStale);
        };
        if prepared.generation != self.generation || prepared.content_tag != self.content_tag {
            // This controller may already have a fresh request for the same
            // cluster. A mismatched completion belongs to another session and
            // must not consume that request's permit or change its state.
            return Ok(ShDrainAdmission::DroppedStale);
        }
        if !self.targets.contains(&cluster_id) {
            if state.state == ClusterResidencyState::Queued {
                self.release_permit()?;
                self.states[cluster_id as usize].state = ClusterResidencyState::Absent;
            }
            return Ok(ShDrainAdmission::DroppedNotTargeted);
        }
        if state.state != ClusterResidencyState::Queued || self.ready.contains_key(&cluster_id) {
            return Ok(ShDrainAdmission::DroppedDuplicate);
        }
        let byte_charge = u64::try_from(prepared.chunk.bytes.len()).map_err(|_| {
            ShResidencyControllerError::AccountingOverflow("ready chunk byte length")
        })?;
        self.accounting.cpu.ready.add(byte_charge, "ready bytes")?;
        self.ready.insert(
            cluster_id,
            ReadyCluster {
                prepared,
                byte_charge,
            },
        );
        self.states[cluster_id as usize].state = ClusterResidencyState::Ready;
        Ok(ShDrainAdmission::Ready)
    }

    /// Emits no more than two renderer-ready chunks plus an initial reset or
    /// sorted target deltas for the deterministic sync-proof path. This path
    /// deliberately never emits evictions or changes targets for budget
    /// pressure: Task 10's proof gate remains a stable no-eviction baseline.
    pub(crate) fn take_drain_batch(&mut self) -> Result<ShDrainBatch, ShResidencyControllerError> {
        self.take_drain_batch_with_eviction(false)
    }

    /// Async-only drain policy. Departed residents leave before pressure can
    /// suppress cold prefetch, and every eviction remains only a request until
    /// the renderer confirms that no installed dependent pins its owner.
    pub(crate) fn take_async_drain_batch(
        &mut self,
    ) -> Result<ShDrainBatch, ShResidencyControllerError> {
        self.take_drain_batch_with_eviction(true)
    }

    fn take_drain_batch_with_eviction(
        &mut self,
        eviction_enabled: bool,
    ) -> Result<ShDrainBatch, ShResidencyControllerError> {
        if !self.in_drain.is_empty() || !self.in_drain_evictions.is_empty() {
            return Err(ShResidencyControllerError::InvalidDrainOutcome(
                "previous drain outcome was not applied before preparing another batch".into(),
            ));
        }
        if eviction_enabled {
            self.apply_budget_policy()?;
        }
        let mut batch = ShDrainBatch {
            generation: self.generation,
            content_tag: self.content_tag,
            ..ShDrainBatch::default()
        };
        if self.needs_target_reset {
            batch.target_reset = Some(target_bitset(self.topology.cluster_count(), &self.targets)?);
            self.needs_target_reset = false;
            self.sent_targets = self.targets.clone();
        } else {
            batch.target_add = self
                .targets
                .difference(&self.sent_targets)
                .copied()
                .collect();
            batch.target_remove = self
                .sent_targets
                .difference(&self.targets)
                .copied()
                .collect();
            self.sent_targets = self.targets.clone();
        }
        self.drop_departed_ready(&self.targets.clone())?;

        if eviction_enabled {
            batch.evictions = self.departed_eviction_order();
        }
        self.in_drain_evictions = batch.evictions.iter().copied().collect();
        let mut ready_ids: Vec<_> = self
            .ready
            .keys()
            .copied()
            .filter(|&cluster_id| self.ready_for_install(cluster_id))
            .collect();
        ready_ids.sort_by_key(|&cluster_id| {
            (
                self.states[cluster_id as usize]
                    .class
                    .unwrap_or(TargetClass::Hysteresis),
                cluster_id,
            )
        });
        for cluster_id in ready_ids.into_iter().take(MAX_INSTALLS_PER_DRAIN) {
            let ready = self
                .ready
                .remove(&cluster_id)
                .expect("ready id came from map");
            self.in_drain.insert(cluster_id, ready.byte_charge);
            batch.ready.push(ready.prepared);
        }
        Ok(batch)
    }

    /// Enforce the CPU-side policy budget without pretending it is a second
    /// GPU allocation. Active pool capacity is renderer-owned; this budget
    /// only ranks logical working-set demand to decide which cold prefetch may
    /// be suppressed before the renderer has to grow a pool.
    fn apply_budget_policy(&mut self) -> Result<(), ShResidencyControllerError> {
        let nominal = self.accounting.nominal_cluster_bytes()?;
        let protected = self.non_evictable_target_bytes()?;
        self.set_non_evictable_overshoot(protected.saturating_sub(nominal));

        let mut projected = self.projected_logical_demand()?;
        if projected <= nominal {
            return Ok(());
        }

        // Only non-visible prefetch is pressure-eligible. A target owner is
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
                .filter(|&cluster_id| self.pressure_evictable_prefetch(cluster_id))
                .min_by(|left, right| self.compare_eviction_keys(*left, *right))
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
        Ok(())
    }

    fn non_evictable_target_bytes(&self) -> Result<u64, ShResidencyControllerError> {
        self.targets.iter().try_fold(0u64, |total, &cluster_id| {
            // A prefetch target normally yields to pressure, but it becomes
            // non-evictable when it was just installed or is an owner still
            // needed by another target. Count that pinned work with visible
            // and hysteresis targets so a real floor overage is never hidden
            // behind its prefetch classification.
            if self.pressure_evictable_prefetch(cluster_id) {
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

    fn pressure_evictable_prefetch(&self, cluster_id: u32) -> bool {
        let state = &self.states[cluster_id as usize];
        if state.class != Some(TargetClass::Prefetch)
            || state.state == ClusterResidencyState::InstalledUncomposed
        {
            return false;
        }
        !self.targets.iter().any(|&dependent| {
            dependent != cluster_id
                && self.topology.owners[dependent as usize].contains(&cluster_id)
        })
    }

    fn departed_eviction_order(&self) -> Vec<u32> {
        let eligible: BTreeSet<_> = self
            .states
            .iter()
            .enumerate()
            .filter_map(|(cluster_id, state)| {
                (state.state == ClusterResidencyState::Sampleable
                    && !self.targets.contains(&(cluster_id as u32)))
                .then_some(cluster_id as u32)
            })
            .collect();
        // Expired hysteresis departures are released first. Pressure-only
        // evictions are the remaining prefetch LRU pass, so a capacity event
        // cannot jump ahead of lighting that has already completed retention.
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

    /// Applies renderer ownership transfer after one drain. Deferred chunks
    /// are returned intact by the renderer and continue retaining their permit.
    pub(crate) fn apply_drain_outcome(
        &mut self,
        outcome: ShDrainOutcome,
    ) -> Result<(), ShResidencyControllerError> {
        validate_outcome_lists(&outcome)?;
        if outcome
            .evicted
            .iter()
            .any(|cluster_id| !self.in_drain_evictions.contains(cluster_id))
        {
            return Err(ShResidencyControllerError::InvalidDrainOutcome(
                "renderer released a cluster that this drain did not request for eviction".into(),
            ));
        }
        let expected: BTreeSet<_> = self.in_drain.keys().copied().collect();
        let returned: BTreeSet<_> = outcome
            .accepted
            .iter()
            .chain(&outcome.dropped)
            .copied()
            .chain(
                outcome
                    .deferred
                    .iter()
                    .map(|prepared| prepared.chunk.cluster_id),
            )
            .collect();
        if expected != returned {
            return Err(ShResidencyControllerError::InvalidDrainOutcome(
                "outcome does not account for every drained cluster exactly once".into(),
            ));
        }
        if outcome
            .accepted
            .iter()
            .any(|cluster_id| !self.targets.contains(cluster_id))
        {
            return Err(ShResidencyControllerError::InvalidDrainOutcome(
                "renderer accepted a cluster that is no longer targeted".into(),
            ));
        }
        for prepared in &outcome.deferred {
            let cluster_id = prepared.chunk.cluster_id;
            let byte_charge = u64::try_from(prepared.chunk.bytes.len()).map_err(|_| {
                ShResidencyControllerError::AccountingOverflow("deferred chunk byte length")
            })?;
            if prepared.generation != self.generation
                || prepared.content_tag != self.content_tag
                || !self.targets.contains(&cluster_id)
                || self.in_drain.get(&cluster_id) != Some(&byte_charge)
            {
                return Err(ShResidencyControllerError::InvalidDrainOutcome(
                    "deferred chunk does not match the current drain".into(),
                ));
            }
        }
        self.preflight_drain_outcome(&outcome)?;

        // The renderer reports only actual releases. A pinned owner stays in
        // this controller's logical ledger and is retried later, so releasing
        // a dependent can never underflow the owner charge.
        for cluster_id in &outcome.evicted {
            self.accounting
                .remove_logical(self.topology.requested_resident_bytes[*cluster_id as usize])
                .expect("preflight validated confirmed eviction accounting");
            self.states[*cluster_id as usize].state = ClusterResidencyState::Absent;
            Self::increment_counter(&mut self.counters.evictions, "stream evictions")
                .expect("preflight validated eviction counter");
        }
        self.in_drain_evictions.clear();

        for cluster_id in outcome.accepted {
            let byte_charge = self
                .in_drain
                .remove(&cluster_id)
                .expect("outcome ids were checked");
            self.release_ready_charge(byte_charge)
                .expect("preflight validated accepted ready bytes");
            self.release_permit()
                .expect("preflight validated accepted permit");
            self.accounting
                .add_logical(self.topology.requested_resident_bytes[cluster_id as usize])
                .expect("preflight validated accepted logical accounting");
            self.states[cluster_id as usize].state = ClusterResidencyState::InstalledUncomposed;
            Self::increment_counter(&mut self.counters.installs, "stream installs")
                .expect("preflight validated install counter");
        }
        for cluster_id in outcome.dropped {
            let byte_charge = self
                .in_drain
                .remove(&cluster_id)
                .expect("outcome ids were checked");
            self.release_ready_charge(byte_charge)
                .expect("preflight validated dropped ready bytes");
            self.release_permit()
                .expect("preflight validated dropped permit");
            self.states[cluster_id as usize].state = ClusterResidencyState::Absent;
        }
        for prepared in outcome.deferred {
            let cluster_id = prepared.chunk.cluster_id;
            let tracked = self
                .in_drain
                .remove(&cluster_id)
                .expect("outcome ids were checked");
            let byte_charge = u64::try_from(prepared.chunk.bytes.len())
                .expect("preflight validated deferred chunk byte length");
            debug_assert_eq!(
                byte_charge, tracked,
                "preflight validated deferred byte charge"
            );
            self.ready.insert(
                cluster_id,
                ReadyCluster {
                    prepared,
                    byte_charge,
                },
            );
            self.states[cluster_id as usize].state = ClusterResidencyState::Ready;
        }
        Ok(())
    }

    /// Validate every arithmetic transition before mutating controller state.
    /// Renderer outcomes cross a subsystem boundary: an overflow must reject
    /// the entire outcome, not release an eviction and then fail an install.
    fn preflight_drain_outcome(
        &self,
        outcome: &ShDrainOutcome,
    ) -> Result<(), ShResidencyControllerError> {
        let mut accounting = self.accounting;
        let mut permits = self.permits_in_use;
        let mut counters = self.counters;

        for &cluster_id in &outcome.evicted {
            if self.states[cluster_id as usize].state != ClusterResidencyState::Sampleable {
                return Err(ShResidencyControllerError::InvalidDrainOutcome(
                    "renderer released a controller-nonresident cluster".into(),
                ));
            }
            accounting
                .remove_logical(self.topology.requested_resident_bytes[cluster_id as usize])?;
            Self::increment_counter(&mut counters.evictions, "stream evictions")?;
        }
        for &cluster_id in &outcome.accepted {
            self.preflight_drained_ready_state(cluster_id)?;
            let byte_charge = self.in_drain[&cluster_id];
            accounting.cpu.ready.remove(byte_charge, "ready bytes")?;
            permits =
                permits
                    .checked_sub(1)
                    .ok_or(ShResidencyControllerError::AccountingUnderflow(
                        "stream permits",
                    ))?;
            accounting.add_logical(self.topology.requested_resident_bytes[cluster_id as usize])?;
            Self::increment_counter(&mut counters.installs, "stream installs")?;
        }
        for &cluster_id in &outcome.dropped {
            self.preflight_drained_ready_state(cluster_id)?;
            let byte_charge = self.in_drain[&cluster_id];
            accounting.cpu.ready.remove(byte_charge, "ready bytes")?;
            permits =
                permits
                    .checked_sub(1)
                    .ok_or(ShResidencyControllerError::AccountingUnderflow(
                        "stream permits",
                    ))?;
        }
        for prepared in &outcome.deferred {
            let cluster_id = prepared.chunk.cluster_id;
            self.preflight_drained_ready_state(cluster_id)?;
            let byte_charge = u64::try_from(prepared.chunk.bytes.len()).map_err(|_| {
                ShResidencyControllerError::AccountingOverflow("deferred chunk byte length")
            })?;
            if self.in_drain.get(&cluster_id) != Some(&byte_charge) {
                return Err(ShResidencyControllerError::InvalidDrainOutcome(
                    "deferred chunk changed its accounted byte length".into(),
                ));
            }
        }
        Ok(())
    }

    fn preflight_drained_ready_state(
        &self,
        cluster_id: u32,
    ) -> Result<(), ShResidencyControllerError> {
        if self.states[cluster_id as usize].state != ClusterResidencyState::Ready {
            return Err(ShResidencyControllerError::InvalidDrainOutcome(
                "renderer outcome names a controller-nonready drained cluster".into(),
            ));
        }
        Ok(())
    }

    /// The renderer calls this only at the next pre-compose drain after the
    /// prior frame's compose submission. That one-frame delay is what keeps a
    /// newly installed cluster's sample word invalid during its compose work.
    pub(crate) fn promote_composed_clusters(&mut self) {
        for state in &mut self.states {
            if state.state == ClusterResidencyState::InstalledUncomposed {
                state.state = ClusterResidencyState::Sampleable;
            }
        }
    }

    /// Pressure recovery makes prefetch eligible again without changing the
    /// raw visibility horizon. Task 12 owns when that recovery is safe.
    pub(crate) fn clear_prefetch_suppression(&mut self) {
        for state in &mut self.states {
            state.suppressed = false;
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
