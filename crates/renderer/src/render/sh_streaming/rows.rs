//! Dirty-row unions, deferred promotion, and eviction invalidation.

use super::*;

impl ShResidencyState {
    pub(super) fn mark_indirect_dirty_for_dense(
        &mut self,
        dense: u32,
    ) -> Result<(), ShResidencyDrainError> {
        let row = self.affinity_row_for_dense(dense)?;
        self.indirect_dirty_rows.insert(row);
        Ok(())
    }

    pub(super) fn remove_indirect_base_row_ref(
        &mut self,
        dense: u32,
    ) -> Result<(), ShResidencyDrainError> {
        let row = self.affinity_row_for_dense(dense)?;
        decrement_row_ref(&mut self.indirect_base_row_refs, row)
    }

    pub(super) fn refresh_indirect_resident_rows(&mut self) {
        self.indirect_resident_rows.clear();
        self.indirect_resident_rows
            .extend(self.indirect_base_row_refs.keys().copied());
        self.indirect_resident_rows
            .extend(self.indirect_delta_row_refs.keys().copied());
    }

    pub(super) fn mark_direct_dirty_for_dense(
        &mut self,
        dense: u32,
    ) -> Result<(), ShResidencyDrainError> {
        let row = self.affinity_row_for_dense(dense)?;
        self.direct_promotion_dirty_rows.insert(row);
        self.direct_animated_dirty_rows.insert(row);
        Ok(())
    }

    pub(super) fn remove_direct_base_row_ref(
        &mut self,
        dense: u32,
    ) -> Result<(), ShResidencyDrainError> {
        let row = self.affinity_row_for_dense(dense)?;
        decrement_row_ref(&mut self.direct_base_row_refs, row)
    }

    pub(super) fn refresh_direct_resident_rows(&mut self) {
        self.direct_promotion_resident_rows.clear();
        self.direct_promotion_resident_rows
            .extend(self.direct_base_row_refs.keys().copied());
        self.direct_promotion_resident_rows
            .extend(self.direct_promotion_row_refs.keys().copied());
        self.direct_animated_resident_rows = self.direct_promotion_resident_rows.clone();
        self.direct_animated_resident_rows
            .extend(self.direct_animated_row_refs.keys().copied());
    }

    pub(super) fn affinity_row_for_dense(&self, dense: u32) -> Result<u32, ShResidencyDrainError> {
        let [width, height, depth] = self.grid_dimensions();
        let xy = width
            .checked_mul(height)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        if dense
            >= xy
                .checked_mul(depth)
                .ok_or(ShResidencyDrainError::SlotOverflow)?
        {
            return Err(ShResidencyDrainError::SlotOverflow);
        }
        let x = dense % width;
        let y = (dense / width) % height;
        let z = dense / xy;
        let affinity = [x / 4, y / 4, z / 4];
        let affinity_width = width.div_ceil(4);
        let affinity_height = height.div_ceil(4);
        let row = affinity[0]
            .checked_add(
                affinity[1]
                    .checked_mul(affinity_width)
                    .ok_or(ShResidencyDrainError::SlotOverflow)?,
            )
            .and_then(|row| {
                row.checked_add(
                    affinity[2].checked_mul(affinity_width.checked_mul(affinity_height)?)?,
                )
            })
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        Ok(row)
    }

    pub(super) fn promote_completed(
        &mut self,
        queue: &wgpu::Queue,
    ) -> Result<(), ShResidencyDrainError> {
        let ready = promotion_sweep_candidates(&self.pending_promotion);
        for cluster_id in ready {
            if let Some(installed) = self.installed.get(&cluster_id)
                && installed_cluster_is_sampleable(
                    installed,
                    &self.owner_dependencies[cluster_id as usize],
                    &self.sampleable,
                    self.indirect_compose_epoch,
                    self.direct_compose_epoch,
                )
            {
                let updates: Vec<_> = installed
                    .patches
                    .iter()
                    .map(|patch| {
                        self.sampled_words[patch.dense as usize] =
                            self.compose_words[patch.dense as usize];
                        (patch.dense, patch.mean_distance, patch.mean_sq_distance)
                    })
                    .collect();
                if let Some(gpu) = self.gpu.as_ref() {
                    gpu.upload_sample_words_and_moments(
                        queue,
                        &self.sampled_words,
                        &updates,
                        self.grid_dimensions(),
                    )?;
                }
                self.pending_promotion.remove(&cluster_id);
                self.sampleable.insert(cluster_id);
            }
        }
        Ok(())
    }

    pub(super) fn evict(
        &mut self,
        queue: &wgpu::Queue,
        cluster_id: u32,
    ) -> Result<(), ShResidencyDrainError> {
        self.validate_cluster_id(cluster_id)?;
        // A baked owner stays pinned while any dependent is installed. The
        // renderer never transfers sparse ownership to a halo copy.
        if self.installed.iter().any(|(&dependent, _)| {
            dependent != cluster_id
                && self.owner_dependencies[dependent as usize].contains(&cluster_id)
        }) {
            return Ok(());
        }
        let Some(installed) = self.installed.remove(&cluster_id) else {
            return Ok(());
        };
        let mut sample_updates = Vec::with_capacity(installed.patches.len());
        for patch in installed.patches {
            invalidate_dense_words(
                &mut self.compose_words,
                &mut self.sampled_words,
                patch.dense,
            )?;
            self.mark_indirect_dirty_for_dense(patch.dense)?;
            self.remove_indirect_base_row_ref(patch.dense)?;
            if self.direct_required {
                self.mark_direct_dirty_for_dense(patch.dense)?;
                self.remove_direct_base_row_ref(patch.dense)?;
            }
            sample_updates.push((patch.dense, patch.mean_distance, patch.mean_sq_distance));
        }
        self.refresh_indirect_resident_rows();
        for node in installed.owned_nodes {
            if let Some(range) = self.node_slots.remove(&node) {
                self.dense_slots.release(range)?;
            }
        }
        for (section_id, row) in installed.sparse_rows {
            let gpu = self
                .gpu
                .as_ref()
                .ok_or(ShResidencyDrainError::GpuCapacity {
                    reason: "streamed sparse eviction requested before GPU initialization",
                })?;
            if section_id == INDIRECT_DELTA_ID {
                gpu.clear_indirect_sparse_row(queue, row)?;
            } else {
                gpu.clear_direct_sparse_row(queue, section_id, row)?;
            }
            if let Some(pool) = self.sparse_pools.get_mut(&section_id) {
                pool.evict(row)?;
            }
            self.dirty_rows.insert((section_id, row));
            if section_id == INDIRECT_DELTA_ID {
                self.indirect_dirty_rows.insert(row);
                decrement_row_ref(&mut self.indirect_delta_row_refs, row)?;
                self.refresh_indirect_resident_rows();
            } else if section_id == DIRECT_DELTA_ID {
                self.direct_promotion_dirty_rows.insert(row);
                self.direct_animated_dirty_rows.insert(row);
                decrement_row_ref(&mut self.direct_promotion_row_refs, row)?;
                self.refresh_direct_resident_rows();
            } else if section_id == ANIMATED_DIRECT_DELTA_ID {
                self.direct_animated_dirty_rows.insert(row);
                decrement_row_ref(&mut self.direct_animated_row_refs, row)?;
                self.refresh_direct_resident_rows();
            }
        }
        if self.direct_required {
            self.refresh_direct_resident_rows();
        }
        self.pending_promotion.remove(&cluster_id);
        self.sampleable.remove(&cluster_id);
        if let Some(gpu) = self.gpu.as_ref() {
            // Publish zeros before freed ranges can be reused by a later ready
            // item in the same drain. The texture moments retain their baked
            // values, but their indirection word is invalid so sampling uses
            // the miss policy.
            gpu.upload_changed_compose_words(
                queue,
                &self.compose_words,
                sample_updates.iter().map(|&(dense, _, _)| dense),
            )?;
            gpu.upload_sample_words_and_moments(
                queue,
                &self.sampled_words,
                &sample_updates,
                self.grid_dimensions(),
            )?;
        }
        Ok(())
    }
}

/// Promotion deliberately observes compose completion from a later drain.
/// Keeping this predicate pure makes the epoch/dependency contract testable
/// without a renderer device and prevents a new canonical writer from being
/// published before both its tile owner and required compose passes are ready.
pub(super) fn installed_cluster_is_sampleable(
    installed: &InstalledCluster,
    owner_dependencies: &BTreeSet<u32>,
    sampleable: &BTreeSet<u32>,
    indirect_compose_epoch: u64,
    direct_compose_epoch: u64,
) -> bool {
    owner_dependencies
        .iter()
        .all(|owner| sampleable.contains(owner))
        && installed.required_indirect_epoch <= indirect_compose_epoch
        && installed.required_direct_epoch <= direct_compose_epoch
}

/// Freeze the sweep candidate set at drain entry. Clusters installed later in
/// the same drain wait for the next sweep, after their compute work has had a
/// chance to encode.
pub(super) fn promotion_sweep_candidates(pending: &BTreeSet<u32>) -> Vec<u32> {
    pending.iter().copied().collect()
}

/// Invalidate both publicly reachable indirection mirrors before returning a
/// dense slot to the first-fit allocator. The physical tile can retain stale
/// texels, but neither sampling nor compose may address it after this step.
pub(super) fn invalidate_dense_words(
    compose_words: &mut [u32],
    sampled_words: &mut [u32],
    dense: u32,
) -> Result<(), ShResidencyDrainError> {
    let index = usize::try_from(dense).map_err(|_| ShResidencyDrainError::SlotOverflow)?;
    let compose = compose_words
        .get_mut(index)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    let sampled = sampled_words
        .get_mut(index)
        .ok_or(ShResidencyDrainError::SlotOverflow)?;
    *compose = 0;
    *sampled = 0;
    Ok(())
}
