//! Loader-to-renderer drain lifecycle and synchronous proof gate.
//! See: context/plans/in-progress/sh-probe-streaming--cluster-residency/index.md

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
                self.mark_failed(request.cluster_id)?;
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

    /// A failed worker completion releases exactly the request's permit. A
    /// stale generation or changed chunk identity cannot fail a new request.
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
        self.mark_failed(request.cluster_id)?;
        Ok(true)
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
    /// sorted target deltas. It never evicts in the synchronous proof stage.
    pub(crate) fn take_drain_batch(&mut self) -> Result<ShDrainBatch, ShResidencyControllerError> {
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

    /// Applies renderer ownership transfer after one drain. Deferred chunks
    /// are returned intact by the renderer and continue retaining their permit.
    pub(crate) fn apply_drain_outcome(
        &mut self,
        outcome: ShDrainOutcome,
    ) -> Result<(), ShResidencyControllerError> {
        validate_outcome_lists(&outcome)?;
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
        let accepted_logical = outcome
            .accepted
            .iter()
            .try_fold(0u64, |total, &cluster_id| {
                total
                    .checked_add(self.topology.requested_resident_bytes[cluster_id as usize])
                    .ok_or(ShResidencyControllerError::AccountingOverflow(
                        "logical occupancy",
                    ))
            })?;
        self.accounting.can_add_logical(accepted_logical)?;
        for cluster_id in outcome.accepted {
            let byte_charge = self
                .in_drain
                .remove(&cluster_id)
                .expect("outcome ids were checked");
            self.release_ready_charge(byte_charge)?;
            self.release_permit()?;
            self.accounting
                .add_logical(self.topology.requested_resident_bytes[cluster_id as usize])?;
            self.states[cluster_id as usize].state = ClusterResidencyState::InstalledUncomposed;
        }
        for cluster_id in outcome.dropped {
            let byte_charge = self
                .in_drain
                .remove(&cluster_id)
                .expect("outcome ids were checked");
            self.release_ready_charge(byte_charge)?;
            self.release_permit()?;
            self.states[cluster_id as usize].state = ClusterResidencyState::Absent;
        }
        for prepared in outcome.deferred {
            let cluster_id = prepared.chunk.cluster_id;
            let tracked = self
                .in_drain
                .remove(&cluster_id)
                .expect("outcome ids were checked");
            let byte_charge = u64::try_from(prepared.chunk.bytes.len()).map_err(|_| {
                ShResidencyControllerError::AccountingOverflow("deferred chunk byte length")
            })?;
            if byte_charge != tracked {
                return Err(ShResidencyControllerError::InvalidDrainOutcome(
                    "deferred chunk changed its accounted byte length".into(),
                ));
            }
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

    /// Task 12 uses this after pressure eviction. The suppression is only for
    /// prefetch: a newly visible cluster immediately clears it on horizon
    /// change, while the controller never suppresses visible work itself.
    pub(crate) fn suppress_prefetch(&mut self, cluster_id: u32) {
        if let Some(state) = self.states.get_mut(cluster_id as usize)
            && state.class == Some(TargetClass::Prefetch)
        {
            state.suppressed = true;
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
