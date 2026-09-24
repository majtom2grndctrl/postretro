//! Sparse-row ownership planning and bounded pool growth.

use super::*;

impl ShResidencyState {
    pub(super) fn collect_sparse_rows(
        &self,
        cluster_id: u32,
        chunk: &DecodedClusterShPayload,
    ) -> Result<Vec<(u32, ParsedSparseRow)>, ShResidencyDrainError> {
        let mut collected = Vec::new();
        let mut seen = BTreeSet::new();
        for block in chunk
            .blocks
            .iter()
            .filter(|block| block.kind == SPARSE_ROWS_BLOCK)
        {
            if !matches!(
                block.section_id,
                INDIRECT_DELTA_ID | DIRECT_DELTA_ID | ANIMATED_DIRECT_DELTA_ID
            ) {
                return Err(malformed(
                    cluster_id,
                    "chunk carries an unsupported sparse source",
                ));
            }
            if !self.sparse_pools.contains_key(&block.section_id) {
                return Err(malformed(
                    cluster_id,
                    "chunk carries an absent sparse source",
                ));
            }
            for row in parse_sparse_rows(cluster_id, chunk.block_bytes(block))? {
                if !seen.insert((block.section_id, row.row)) {
                    return Err(malformed(cluster_id, "chunk repeats a sparse row"));
                }
                let expected_owner = self
                    .sparse_row_owner
                    .get(&(block.section_id, row.row))
                    .copied()
                    .ok_or(malformed(cluster_id, "sparse row has no id-49 owner"))?;
                match row.role {
                    1 if expected_owner == cluster_id => {
                        collected.push((block.section_id, row));
                    }
                    2 if expected_owner != cluster_id => {}
                    _ => {
                        return Err(malformed(
                            cluster_id,
                            "sparse row role disagrees with id-49 owner",
                        ));
                    }
                }
            }
        }
        Ok(collected)
    }

    /// Allocate a complete already-validated batch. The caller's rollback
    /// snapshot restores all CPU mirrors if any later row cannot reserve its
    /// entry/tile ranges; queue writes have not started yet.
    pub(super) fn allocate_sparse_rows(
        &mut self,
        cluster_id: u32,
        rows: Vec<(u32, ParsedSparseRow)>,
    ) -> Result<Vec<SparseInstallPlan>, ShResidencyDrainError> {
        let mut plans = Vec::with_capacity(rows.len());
        for (section_id, payload) in rows {
            let (entries, tiles) = self
                .sparse_pools
                .get_mut(&section_id)
                .ok_or(malformed(
                    cluster_id,
                    "sparse source disappeared during install",
                ))?
                .install(payload.row, payload.entry_count, payload.tile_f16_count)
                .map_err(|_| malformed(cluster_id, "sparse row cannot allocate pool range"))?;
            self.dirty_rows.insert((section_id, payload.row));
            match section_id {
                INDIRECT_DELTA_ID => {
                    self.indirect_dirty_rows.insert(payload.row);
                    increment_row_ref(&mut self.indirect_delta_row_refs, payload.row)?;
                    self.refresh_indirect_resident_rows();
                }
                DIRECT_DELTA_ID => {
                    self.direct_promotion_dirty_rows.insert(payload.row);
                    // Pass B samples Pass A's result, so an id-41 change must
                    // dirty both unions when id-45 is present.
                    self.direct_animated_dirty_rows.insert(payload.row);
                    increment_row_ref(&mut self.direct_promotion_row_refs, payload.row)?;
                    self.refresh_direct_resident_rows();
                }
                ANIMATED_DIRECT_DELTA_ID => {
                    self.direct_animated_dirty_rows.insert(payload.row);
                    increment_row_ref(&mut self.direct_animated_row_refs, payload.row)?;
                    self.refresh_direct_resident_rows();
                }
                _ => unreachable!("collect_sparse_rows validates streamed families"),
            }
            plans.push(SparseInstallPlan {
                section_id,
                payload,
                entries,
                tiles,
            });
        }
        Ok(plans)
    }

    /// Grow all sparse families needed by the current provisional allocator
    /// state in one replacement.  This runs before any GPU upload is queued;
    /// if the sole retirement slot is still occupied, the caller rolls the
    /// CPU mirrors back and retains the prepared payload for the next drain.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn ensure_sparse_capacity(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sh: &mut crate::render::sh_volume::ShVolumeResources,
        uniform_bind_group_layout: &wgpu::BindGroupLayout,
        selection_weights: &wgpu::Buffer,
    ) -> Result<(), ShResidencyDrainError> {
        let required = self
            .sparse_pools
            .iter()
            .map(|(&section_id, pool)| (section_id, (pool.entries.capacity, pool.tiles.capacity)))
            .collect::<SparseCapacityFloors>();
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.grow_sparse(
                device,
                queue,
                &self.base_metadata,
                &self.source_metadata,
                &required,
                sh,
                uniform_bind_group_layout,
                selection_weights,
            )?;
        }
        Ok(())
    }
}
