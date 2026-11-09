//! Target-generation transitions and the atomic drain boundary.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency).

use super::*;

impl ShResidencyState {
    pub(in crate::render) fn drain(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::uploads::UploadQueue,
        sh: &mut crate::render::sh_volume::ShVolumeResources,
        selection_weights: &wgpu::Buffer,
        batch: ShDrainBatch,
    ) -> Result<ShDrainOutcome, ShResidencyDrainError> {
        // Validate the complete public handoff before target transitions,
        // retirement, promotion, eviction, or installation can mutate state.
        self.validate_batch_contract(&batch)?;
        // The load's one tile-reference table: every texel this drain writes
        // packs G from it (`pack_moment_upload_spans`).
        let references = sh.depth_tiles.references();
        self.apply_targets(device, queue, &batch, references, &mut sh.mover_channel)?;
        self.release_retired_generations();
        // Promotion, eviction and queued tile-reference texels share one
        // upload batch, submitted before any install can reuse a released
        // range.
        let mut uploads = self.begin_uploads(0);
        let mut outcome = ShDrainOutcome::default();
        let released = self
            .promote_and_evict(&mut uploads, references, batch.evictions, &mut outcome)
            .and_then(|()| self.flush_mover_releases(&mut uploads, &mut sh.mover_channel));
        self.submit_uploads(uploads, device, queue);
        released?;
        let mut gpu = InstallGpu {
            device,
            queue,
            sh,
            selection_weights,
        };
        let admission = PoolAdmission::new(batch.mandatory, batch.victims);
        self.install_ready(Some(&mut gpu), batch.ready, admission, &mut outcome)?;
        #[cfg(feature = "dev-tools")]
        self.probe_readback
            .reconcile(self.grid_dimensions, &self.sampled_words);
        outcome.pool_holds_whole_map = self.pool_holds_whole_map();
        Ok(outcome)
    }

    pub(super) fn promote_and_evict(
        &mut self,
        uploads: &mut StagedUploads,
        references: &ProbeDepthTileReferences,
        evictions: Vec<u32>,
        outcome: &mut ShDrainOutcome,
    ) -> Result<(), ShResidencyDrainError> {
        self.promote_completed(uploads, references)?;
        // Clusters are independent, so requests release in any order. Sorting
        // keeps the confirmed list ascending. A cluster re-targeted since the
        // request stays; the app learns that from its absence here.
        let requested: BTreeSet<u32> = evictions.into_iter().collect();
        for cluster_id in requested {
            if self.targets.contains(&cluster_id) || !self.installed.contains_key(&cluster_id) {
                continue;
            }
            self.evict(uploads, references, cluster_id)?;
            outcome.evicted.push(cluster_id);
        }
        // After promotion and eviction, so each queued texel carries the word
        // this drain leaves it with; G is the table's either way.
        self.flush_depth_tile_texels(uploads, references)
    }

    /// Install every admissible ready cluster, timing the whole per-drain
    /// install pass (including queue writes) into the renderer counters. A
    /// drain with no ready clusters records nothing, so the last-drain figure
    /// always describes real install work.
    pub(super) fn install_ready(
        &mut self,
        mut gpu: Option<&mut InstallGpu<'_>>,
        ready: Vec<PreparedShCluster>,
        mut admission: PoolAdmission,
        outcome: &mut ShDrainOutcome,
    ) -> Result<(), ShResidencyDrainError> {
        if ready.is_empty() {
            return Ok(());
        }
        let started = std::time::Instant::now();
        let growth_events_before = self.pool_growth_events();
        let result = (|| {
            for prepared in ready {
                let cluster_id = prepared.chunk.cluster_id;
                if prepared.generation != self.generation
                    || prepared.content_tag != self.content_tag
                    || !self.targets.contains(&cluster_id)
                    || self.installed.contains_key(&cluster_id)
                {
                    outcome.dropped.push(cluster_id);
                    continue;
                }
                match self.install_admitted(gpu.as_deref_mut(), &prepared, &mut admission, outcome)
                {
                    Ok(()) => outcome.accepted.push(cluster_id),
                    // A growth transaction may be blocked behind its one
                    // retiring generation. Keep the ready payload/permit in
                    // the outcome rather than treating capacity pressure as a
                    // malformed completion or silently losing visible
                    // lighting.
                    Err(error) if error.is_retryable_retirement_pressure() => {
                        outcome.deferred.push(prepared);
                    }
                    Err(ShResidencyDrainError::GpuDeviceLimit { .. }) => {
                        outcome.refused.push(cluster_id);
                    }
                    Err(ShResidencyDrainError::PoolAtWholeMap { family }) => {
                        self.warn_whole_map_refusal(family, cluster_id);
                        outcome.whole_map_refused.push(cluster_id);
                    }
                    Err(ShResidencyDrainError::PoolShortfall) => {
                        outcome.pool_refused.push(cluster_id);
                    }
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        })();
        let grew_pool = self.pool_growth_events() != growth_events_before;
        self.install_cpu.record_drain(started.elapsed(), grew_pool);
        result
    }

    fn pool_growth_events(&self) -> u64 {
        self.gpu.as_ref().map_or(0, |gpu| gpu.growth.events)
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
        device: &wgpu::Device,
        queue: &crate::render::uploads::UploadQueue,
        batch: &ShDrainBatch,
        references: &ProbeDepthTileReferences,
        mover_channel: &mut crate::render::mover_channel::MoverChannelResources,
    ) -> Result<(), ShResidencyDrainError> {
        let (new_generation, next_targets) = self.plan_target_transition(batch)?;
        if new_generation {
            self.clear_session(device, queue, references)?;
            if self.gpu.is_some() {
                mover_channel.clear_pools(queue);
                self.observe_mover_pool(mover_channel);
            }
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
            if !self.cluster_count.is_multiple_of(64)
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

    /// Every texel goes back to zero moments under the invalid word. G still
    /// packs through the table, so a tiled probe's cleared texel carries its
    /// reference; no reader looks past the invalid word, and the table (not
    /// the session) owns the reference, so it survives the clear.
    fn clear_session(
        &mut self,
        device: &wgpu::Device,
        queue: &crate::render::uploads::UploadQueue,
        references: &ProbeDepthTileReferences,
    ) -> Result<(), ShResidencyDrainError> {
        self.clear_session_mirrors();
        let grid = self.grid_dimensions();
        if let Some(gpu) = self.gpu.as_mut() {
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
            let mut uploads = gpu.begin_uploads(0);
            let staged = gpu.upload_sample_words_and_moments(
                &mut uploads,
                &self.sampled_words,
                &updates,
                grid,
                references,
            );
            gpu.submit_uploads(uploads, device, queue);
            staged?;
        }
        Ok(())
    }

    /// The CPU half of a session clear: every residency mirror, the compose
    /// planner, and the frame scratch return to their post-load state.
    pub(super) fn clear_session_mirrors(&mut self) {
        #[cfg(feature = "dev-tools")]
        self.probe_readback.invalidate();
        self.targets.clear();
        self.node_slots.clear();
        self.storage_refs.clear();
        self.dense_slots = FirstFitRanges::default();
        self.compose_words.fill(0);
        self.sampled_words.fill(0);
        self.installed.clear();
        self.sampleable.clear();
        self.pending_promotion.clear();
        // Nothing is sampleable after a clear; a later promotion packs the
        // table's reference anyway.
        self.pending_depth_tile_texels.clear();
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
        self.released_mover_cells.clear();
        self.compose_planner.reset_generation();
        // The ref tables were cleared wholesale, not row by row.
        self.compose_planner.clear_membership();
        self.compose_membership_touched.clear();
        self.compose_membership_changes.clear();
        self.compose_frame_plan = None;
        self.compose_input_regions.clear();
        self.compose_region_rows.clear();
        self.compose_residency_rows.clear();
        self.compose_animated_weights.clear();
        self.indirect_compose_diagnostics = ShComposePassDiagnostics::default();
        self.static_direct_compose_diagnostics = ShComposePassDiagnostics::default();
        self.animated_direct_compose_diagnostics = ShComposePassDiagnostics::default();
        self.compose_planning_cpu_micros = 0;
        self.indirect_compose_epoch = 0;
        self.direct_compose_epoch = 0;
        for pool in self.sparse_pools.values_mut() {
            *pool = SparsePool::new(pool.row_pairs.len());
        }
    }
}
