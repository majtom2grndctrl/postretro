//! Target-generation transitions and the atomic drain boundary.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency).

use super::*;

impl ShResidencyState {
    pub(in crate::render) fn drain(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sh: &mut crate::render::sh_volume::ShVolumeResources,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        selection_weights: &wgpu::Buffer,
        batch: ShDrainBatch,
    ) -> Result<ShDrainOutcome, ShResidencyDrainError> {
        // Validate the complete public handoff before target transitions,
        // retirement, promotion, eviction, or installation can mutate state.
        self.validate_batch_contract(&batch)?;
        self.apply_targets(queue, &batch)?;
        self.release_retired_generations();
        self.promote_completed(queue)?;
        let mut outcome = ShDrainOutcome::default();
        // A dependent and its canonical owner may depart in the same batch.
        // Compute the release sequence against the installed graph first, so
        // numeric cluster order cannot evict an owner before its halo. A
        // target-dependent owner is absent from this sequence and remains
        // pinned without making the app-side logical ledger guess.
        let requested_evictions: BTreeSet<u32> = batch.evictions.into_iter().collect();
        let installed: BTreeSet<u32> = self.installed.keys().copied().collect();
        for cluster_id in dependent_first_release_order(
            &requested_evictions,
            &self.targets,
            &installed,
            &self.owner_dependencies,
        ) {
            self.evict(queue, cluster_id)?;
            // `evict` rechecks live ownership while mutating rows. Report a
            // confirmed release only after its installed record is actually
            // gone; the app uses this outcome to retire its logical ledger.
            if !self.installed.contains_key(&cluster_id) {
                outcome.evicted.push(cluster_id);
            }
        }
        outcome.evicted.sort_unstable();
        for prepared in batch.ready {
            let cluster_id = prepared.chunk.cluster_id;
            if prepared.generation != self.generation
                || prepared.content_tag != self.content_tag
                || !self.targets.contains(&cluster_id)
                || self.installed.contains_key(&cluster_id)
            {
                outcome.dropped.push(cluster_id);
                continue;
            }
            if self.missing_owner(cluster_id) {
                outcome.deferred.push(prepared);
                continue;
            }
            match self.install(
                device,
                queue,
                sh,
                uniform_bind_group_layout,
                selection_weights,
                &prepared,
            ) {
                Ok(()) => outcome.accepted.push(cluster_id),
                // A growth transaction may be blocked behind its one retiring
                // generation. Keep the ready payload/permit in the outcome
                // rather than treating capacity pressure as a malformed
                // completion or silently losing visible lighting.
                Err(error) if error.is_retryable_retirement_pressure() => {
                    outcome.deferred.push(prepared);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(outcome)
    }

    pub(super) fn validate_batch_contract(
        &self,
        batch: &ShDrainBatch,
    ) -> Result<(), ShResidencyDrainError> {
        batch
            .validate_contract(self.cluster_count, self.content_tag)
            .map_err(|error| ShResidencyDrainError::InvalidBatch(error.to_string()))
    }

    fn apply_targets(
        &mut self,
        queue: &wgpu::Queue,
        batch: &ShDrainBatch,
    ) -> Result<(), ShResidencyDrainError> {
        let (new_generation, next_targets) = self.plan_target_transition(batch)?;
        if new_generation {
            self.clear_session(queue)?;
            self.generation = batch.generation;
            self.generation_has_reset = false;
        }
        self.targets = next_targets;
        // This is a generation-scoped latch, not a property of each delta.
        // Ordinary subsequent target deltas must remain valid after the one
        // accepted reset established the generation.
        if batch.target_reset.is_some() {
            self.generation_has_reset = true;
        }
        Ok(())
    }

    /// Validate an entire controller target transition without mutating
    /// renderer CPU/GPU state. Keeping this pure planning seam makes a bad
    /// reset/delta batch incapable of tearing down a live session.
    pub(super) fn plan_target_transition(
        &self,
        batch: &ShDrainBatch,
    ) -> Result<(bool, BTreeSet<u32>), ShResidencyDrainError> {
        let new_generation = validate_generation_transition(
            self.generation,
            batch.generation,
            batch.target_reset.is_some(),
        )?;
        if !new_generation && !self.generation_has_reset {
            return Err(ShResidencyDrainError::InvalidTargetResetLifecycle);
        }
        // Validate and derive the complete next target set first. A malformed
        // reset or delta must never clear a valid live generation or apply a
        // prefix of a same-generation target update.
        let mut next_targets = self.targets.clone();
        if let Some(words) = &batch.target_reset {
            let expected_words = self.cluster_count.div_ceil(64) as usize;
            if words.len() != expected_words {
                return Err(ShResidencyDrainError::InvalidTargetBitset);
            }
            let mut reset = BTreeSet::new();
            for cluster_id in 0..self.cluster_count {
                if words[cluster_id as usize / 64] & (1u64 << (cluster_id % 64)) != 0 {
                    reset.insert(cluster_id);
                }
            }
            if self.cluster_count % 64 != 0
                && words
                    .last()
                    .is_some_and(|word| *word >> (self.cluster_count % 64) != 0)
            {
                return Err(ShResidencyDrainError::InvalidTargetBitset);
            }
            next_targets = reset;
        }
        for &cluster_id in &batch.target_remove {
            self.validate_cluster_id(cluster_id)?;
            if batch.target_add.binary_search(&cluster_id).is_ok() {
                return Err(ShResidencyDrainError::DuplicateTargetDelta(cluster_id));
            }
            next_targets.remove(&cluster_id);
        }
        for &cluster_id in &batch.target_add {
            self.validate_cluster_id(cluster_id)?;
            next_targets.insert(cluster_id);
        }
        Ok((new_generation, next_targets))
    }

    pub(super) fn validate_cluster_id(&self, cluster_id: u32) -> Result<(), ShResidencyDrainError> {
        if cluster_id >= self.cluster_count {
            return Err(ShResidencyDrainError::TargetOutOfRange(cluster_id));
        }
        Ok(())
    }

    fn clear_session(&mut self, queue: &wgpu::Queue) -> Result<(), ShResidencyDrainError> {
        self.targets.clear();
        self.node_slots.clear();
        self.dense_slots = FirstFitRanges::default();
        self.compose_words.fill(0);
        self.sampled_words.fill(0);
        self.installed.clear();
        self.sampleable.clear();
        self.pending_promotion.clear();
        self.dirty_rows.clear();
        self.indirect_dirty_rows.clear();
        self.indirect_resident_rows.clear();
        self.indirect_base_row_refs.clear();
        self.indirect_delta_row_refs.clear();
        self.direct_promotion_dirty_rows.clear();
        self.direct_animated_dirty_rows.clear();
        self.direct_promotion_resident_rows.clear();
        self.direct_animated_resident_rows.clear();
        self.direct_base_row_refs.clear();
        self.direct_promotion_row_refs.clear();
        self.direct_animated_row_refs.clear();
        self.indirect_was_active = false;
        self.last_indirect_mask = LightTermMask::ALL;
        self.direct_was_active = false;
        self.last_direct_mask = LightTermMask::ALL;
        self.indirect_compose_epoch = 0;
        self.direct_compose_epoch = 0;
        for pool in self.sparse_pools.values_mut() {
            *pool = SparsePool::new(pool.row_pairs.len());
        }
        if let Some(gpu) = self.gpu.as_ref() {
            gpu.clear_all_indirect_sparse_rows(queue);
            gpu.clear_all_direct_sparse_rows(queue)?;
            let updates = (0..self.sampled_words.len())
                .map(|dense| {
                    u32::try_from(dense)
                        .map(|dense| (dense, 0u16, 0u16))
                        .map_err(|_| ShResidencyDrainError::SlotOverflow)
                })
                .collect::<Result<Vec<_>, _>>()?;
            gpu.upload_compose_words(queue, &self.compose_words);
            gpu.upload_sample_words_and_moments(
                queue,
                &self.sampled_words,
                &updates,
                self.grid_dimensions(),
            )?;
        }
        Ok(())
    }
}

/// Return the subset that can actually leave this drain, in dependency order.
/// A renderer does not manufacture ownership transfers: an owner stays alive
/// whenever any installed dependent still names it, including a dependent that
/// remains targeted and was never itself requested for eviction.
fn dependent_first_release_order(
    requested: &BTreeSet<u32>,
    targets: &BTreeSet<u32>,
    installed: &BTreeSet<u32>,
    owner_dependencies: &[BTreeSet<u32>],
) -> Vec<u32> {
    let mut remaining: BTreeSet<_> = requested
        .iter()
        .copied()
        .filter(|cluster_id| !targets.contains(cluster_id) && installed.contains(cluster_id))
        .collect();
    let mut live = installed.clone();
    let mut ordered = Vec::with_capacity(remaining.len());
    loop {
        let releasable: Vec<_> = remaining
            .iter()
            .copied()
            .filter(|&candidate| {
                !live.iter().any(|&dependent| {
                    dependent != candidate
                        && owner_dependencies
                            .get(dependent as usize)
                            .is_some_and(|owners| owners.contains(&candidate))
                })
            })
            .collect();
        if releasable.is_empty() {
            return ordered;
        }
        for cluster_id in releasable {
            remaining.remove(&cluster_id);
            live.remove(&cluster_id);
            ordered.push(cluster_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_order_keeps_an_owner_pinned_until_its_departed_halo_leaves() {
        let order = dependent_first_release_order(
            &BTreeSet::from([0, 1]),
            &BTreeSet::new(),
            &BTreeSet::from([0, 1]),
            &[BTreeSet::new(), BTreeSet::from([0])],
        );
        assert_eq!(order, vec![1, 0]);
    }

    #[test]
    fn release_order_refuses_an_owner_needed_by_a_targeted_halo() {
        let order = dependent_first_release_order(
            &BTreeSet::from([0]),
            &BTreeSet::from([1]),
            &BTreeSet::from([0, 1]),
            &[BTreeSet::new(), BTreeSet::from([0])],
        );
        assert!(order.is_empty());
    }
}
