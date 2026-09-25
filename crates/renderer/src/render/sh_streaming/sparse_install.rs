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

    /// Allocate a complete already-validated batch. The install journal
    /// restores every CPU mirror if any later row cannot reserve its
    /// entry/tile ranges; queue writes have not started yet.
    pub(super) fn allocate_sparse_rows(
        &mut self,
        journal: &mut InstallJournal,
        cluster_id: u32,
        rows: Vec<(u32, ParsedSparseRow)>,
    ) -> Result<Vec<SparseInstallPlan>, ShResidencyDrainError> {
        let mut plans = Vec::with_capacity(rows.len());
        for (section_id, payload) in rows {
            if !self.sparse_pools.contains_key(&section_id) {
                return Err(malformed(
                    cluster_id,
                    "sparse source disappeared during install",
                ));
            }
            let (entries, tiles) = self
                .journal_install_sparse_row(
                    journal,
                    section_id,
                    payload.row,
                    payload.entry_count,
                    payload.tile_f16_count,
                )
                .map_err(|_| malformed(cluster_id, "sparse row cannot allocate pool range"))?;
            let row = payload.row;
            self.journal_dirty_row(journal, section_id, row);
            match section_id {
                INDIRECT_DELTA_ID => {
                    self.journal_insert_row(journal, RowSet::IndirectDirty, row);
                    self.journal_add_row_refs(journal, RowRefTable::IndirectDelta, row, 1)?;
                }
                DIRECT_DELTA_ID => {
                    self.journal_insert_row(journal, RowSet::DirectPromotionDirty, row);
                    // Pass B samples Pass A's result, so an id-41 change must
                    // dirty both unions when id-45 is present.
                    self.journal_insert_row(journal, RowSet::DirectAnimatedDirty, row);
                    self.journal_add_row_refs(journal, RowRefTable::DirectPromotion, row, 1)?;
                }
                ANIMATED_DIRECT_DELTA_ID => {
                    self.journal_insert_row(journal, RowSet::DirectAnimatedDirty, row);
                    self.journal_add_row_refs(journal, RowRefTable::DirectAnimated, row, 1)?;
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
    pub(super) fn ensure_sparse_capacity(
        &mut self,
        gpu: Option<&mut InstallGpu<'_>>,
    ) -> Result<(), ShResidencyDrainError> {
        let Some(pools) = self.gpu.as_mut() else {
            return Ok(());
        };
        let gpu = gpu.ok_or(ShResidencyDrainError::GpuCapacity {
            reason: "streamed sparse growth requires device handles",
        })?;
        let required = self
            .sparse_pools
            .iter()
            .map(|(&section_id, pool)| (section_id, (pool.entries.capacity, pool.tiles.capacity)))
            .collect::<SparseCapacityFloors>();
        pools.grow_sparse(
            gpu.device,
            gpu.queue,
            &self.base_metadata,
            &self.source_metadata,
            &required,
            gpu.sh,
            gpu.uniform_bind_group_layout,
            gpu.selection_weights,
        )
    }
}
