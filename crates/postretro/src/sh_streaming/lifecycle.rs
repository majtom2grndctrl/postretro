//! Loader-to-renderer drain lifecycle and synchronous proof gate.
//! See: context/lib/rendering_pipeline.md §4

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

    /// Reserves one lifecycle permit for the next worker or synchronous source
    /// read. Workers receive the returned identity without introducing an
    /// app-owned type at the renderer boundary.
    pub(crate) fn take_next_request(
        &mut self,
    ) -> Result<Option<ShClusterRequest>, ShResidencyControllerError> {
        if self.permits_in_use >= MAX_STREAM_PERMITS {
            return Ok(None);
        }
        let Some(cluster_id) = self.next_request_cluster()? else {
            return Ok(None);
        };
        self.permits_in_use = self.permits_in_use.checked_add(1).ok_or(
            ShResidencyControllerError::AccountingOverflow("stream permits"),
        )?;
        let state = &mut self.states[cluster_id as usize];
        state.state = ClusterResidencyState::Queued;
        let mandatory = matches!(
            state.class,
            Some(TargetClass::Visible | TargetClass::Pinned)
        );
        Ok(Some(ShClusterRequest {
            generation: self.generation,
            content_tag: self.content_tag,
            cluster_id,
            chunk_hash: self.topology.chunk_hashes[cluster_id as usize],
            mandatory,
        }))
    }

    /// A request the I/O issuer skipped because its cluster left the target
    /// set before the read. It returns to `Absent` with its permit released
    /// even if the cluster is targeted again by now: no bytes were read, so
    /// the next request simply re-issues it. Never a failure, never a warning.
    pub(crate) fn admit_cancelled_request(
        &mut self,
        request: ShClusterRequest,
    ) -> Result<(), ShResidencyControllerError> {
        if !self.matches_completion_identity(request)
            || self.states[request.cluster_id as usize].state != ClusterResidencyState::Queued
        {
            return Ok(());
        }
        let mut cancelled_requests = self.counters.cancelled_requests;
        Self::increment_counter(&mut cancelled_requests, "cancelled requests")?;
        self.release_permit()?;
        self.states[request.cluster_id as usize].state = ClusterResidencyState::Absent;
        self.counters.cancelled_requests = cancelled_requests;
        Ok(())
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

    #[cfg(test)]
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
        // Counted in encoded bytes so the figure shares a unit with the
        // workers' read counters; a foreign cluster id counts no bytes.
        let read_bytes = self
            .topology
            .encoded_chunk_bytes
            .get(prepared.chunk.cluster_id as usize)
            .copied()
            .unwrap_or(0);
        let admission = self.admit_prepared_payload(prepared)?;
        if admission != ShDrainAdmission::Ready {
            let mut counters = self.counters;
            Self::increment_counter(&mut counters.discarded_reads, "discarded reads")?;
            Self::add_to_counter(
                &mut counters.discarded_read_bytes,
                read_bytes,
                "discarded read bytes",
            )?;
            self.counters = counters;
        }
        Ok(admission)
    }

    fn admit_prepared_payload(
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

    /// Emits a decoded-byte-bounded set of renderer-ready chunks plus an
    /// initial reset or sorted target deltas for the sync-proof path. This path
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
                std::cmp::Reverse(self.states[cluster_id as usize].effective_priority),
                cluster_id,
            )
        });
        let (admitted, drain_bytes) = self.select_install_budget(&ready_ids)?;
        let mut counters = self.counters;
        Self::add_to_counter(
            &mut counters.decoded_bytes_installed,
            drain_bytes,
            "decoded bytes installed",
        )?;
        if admitted > 0 {
            counters.last_drain_decoded_bytes = drain_bytes;
            counters.max_drain_decoded_bytes = counters.max_drain_decoded_bytes.max(drain_bytes);
        }
        if admitted < ready_ids.len() {
            Self::increment_counter(&mut counters.budget_limited_drains, "budget-limited drains")?;
        }
        self.counters = counters;
        for cluster_id in ready_ids.into_iter().take(admitted) {
            let ready = self
                .ready
                .remove(&cluster_id)
                .expect("ready id came from map");
            self.in_drain.insert(cluster_id, ready.byte_charge);
            batch.ready.push(ready.prepared);
        }
        Ok(batch)
    }

    /// Returns how many leading clusters of `ordered` fit the decoded-byte
    /// budget, and their byte sum. The first always fits whatever its size, so
    /// an oversized chunk cannot stall residency. Selection stops at the first
    /// cluster over budget rather than skipping ahead to smaller, lower-priority
    /// work.
    fn select_install_budget(
        &self,
        ordered: &[u32],
    ) -> Result<(usize, u64), ShResidencyControllerError> {
        let mut total = 0u64;
        for (admitted, cluster_id) in ordered.iter().enumerate() {
            let next = total
                .checked_add(self.ready[cluster_id].byte_charge)
                .ok_or(ShResidencyControllerError::AccountingOverflow(
                    "drain decoded bytes",
                ))?;
            if admitted > 0 && next > MAX_INSTALL_DECODED_BYTES_PER_DRAIN {
                return Ok((admitted, total));
            }
            total = next;
        }
        Ok((ordered.len(), total))
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
            if self.topology.pinned_clusters.contains(&cluster_id)
                || matches!(
                    self.states[cluster_id as usize].class,
                    Some(TargetClass::Visible | TargetClass::Pinned)
                )
            {
                return Err(ShResidencyControllerError::InvalidDrainOutcome(
                    "renderer attempted to evict a protected SH target".into(),
                ));
            }
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

    /// Pressure recovery makes optional work eligible again without changing
    /// the raw visibility horizon, once its complete target set fits.
    pub(crate) fn clear_optional_suppression(&mut self) {
        for state in &mut self.states {
            state.suppressed = false;
        }
    }
}
