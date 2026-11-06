//! Sparse-row reference planning.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency)

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
                collected.push((block.section_id, row));
            }
        }
        Ok(collected)
    }

    /// Reference every row of an already-validated chunk and allocate the
    /// ones it writes first. Returns the rows to upload; a row already live
    /// keeps its one copy. The install journal restores every CPU mirror if
    /// any later row cannot reserve its entry/tile ranges; queue writes have
    /// not started yet.
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
            if !self.journal_reference_sparse_row(journal, (section_id, payload.row))? {
                continue;
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
}
