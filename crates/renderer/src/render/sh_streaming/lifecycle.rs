//! Target-generation transitions and the atomic drain boundary.

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
        if batch.content_tag != self.content_tag {
            return Ok(ShDrainOutcome {
                dropped: batch
                    .ready
                    .into_iter()
                    .map(|ready| ready.chunk.cluster_id)
                    .collect(),
                ..ShDrainOutcome::default()
            });
        }
        self.apply_targets(queue, &batch)?;
        self.release_retired_generations();
        self.promote_completed(queue)?;
        for cluster_id in &batch.evictions {
            // Target membership is authoritative for the active generation.
            // A delayed eviction that still names a current target must not
            // tear down lighting that a newer target-reset/delta retained.
            if !self.targets.contains(cluster_id) {
                self.evict(queue, *cluster_id)?;
            }
        }

        let mut outcome = ShDrainOutcome::default();
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
