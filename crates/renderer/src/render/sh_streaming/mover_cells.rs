//! Per-cell residency of a streamed MoverProbeChannel (id 57) payload.
//! See: context/lib/rendering_pipeline.md §4 (Cluster SH residency, Mover
//! state lighting)
//!
//! Every cluster covering a listed cell carries that cell's id-57 row. Each
//! carrying cluster holds one `(57, cell)` storage reference, as a sparse row
//! does; the first places the cell's payload in the channel's pooled tables
//! and the last frees it. A streamed payload is the same data placed with
//! id 50's cells: residency, never a light- or mover-driven switch.

use super::install::StagedInstall;
use super::*;
use crate::render::mover_channel::{
    ClusterChannelWrites, ClusterSparseRow, ClusterTermRows, MoverChannelResources,
};

/// The storage-reference key family of a pooled channel cell.
pub(super) const MOVER_CHANNEL_ID: u32 = SectionId::MoverProbeChannel as u32;

impl ShResidencyState {
    /// Reference every pooled channel cell `prepared` carries and stage the
    /// payloads it brings first, plus each whole direct cell still waiting
    /// for its borrowed terms. Runs under the install journal: a malformed
    /// row refuses the cluster before any upload, and rollback releases the
    /// references. A pool that grows rebinds every compose pass binding it.
    pub(super) fn stage_mover_channel(
        &mut self,
        journal: &mut InstallJournal,
        gpu: &mut InstallGpu<'_>,
        prepared: &PreparedShCluster,
        staged: &mut StagedInstall,
    ) -> Result<ClusterChannelWrites, ShResidencyDrainError> {
        let cluster_id = prepared.chunk.cluster_id;
        if !gpu.sh.mover_channel.has_streamed_cells() {
            return Ok(ClusterChannelWrites::default());
        }
        let refuse = |error: &dyn std::fmt::Display| {
            log::error!("[Renderer] streamed cluster {cluster_id}: {error}");
            malformed(
                cluster_id,
                "MoverProbeChannel (id 57) cell is malformed or incomplete",
            )
        };
        let channel_rows = match prepared.chunk.mover_channel_rows() {
            Some(block) => postretro_level_format::mover_probe_channel::parse_streamed_rows(block)
                .map_err(|error| refuse(&error))?,
            None => Vec::new(),
        };
        // Every cluster covering a pooled cell carries its id-57 row. A dense
        // row without it would leave the cell's payload unplaced, and the
        // cell would compose ungated with nothing said; refuse the cluster,
        // as a chunk omitting a required dense member is refused.
        let mut dense_cells = staged
            .dense_probes()
            .map(|dense| self.affinity_row_for_dense(dense))
            .collect::<Result<Vec<_>, _>>()?;
        dense_cells.sort_unstable();
        dense_cells.dedup();
        if let Some(cell) = dense_cells.into_iter().find(|&cell| {
            gpu.sh.mover_channel.pools_cell(cell)
                && !channel_rows.iter().any(|row| row.cell == cell)
        }) {
            return Err(refuse(&format!(
                "dense row of affinity cell {cell} arrives without its MoverProbeChannel (id 57) row"
            )));
        }
        let mut first_cells = Vec::new();
        for row in &channel_rows {
            if !gpu.sh.mover_channel.pools_cell(row.cell) {
                continue;
            }
            let key = (MOVER_CHANNEL_ID, row.cell);
            staged.push_sparse_reference(key);
            if self.journal_reference_sparse_row(journal, key)? {
                first_cells.push(row.cell);
                // A newly resident cell recomposes in both passes.
                self.journal_insert_row(journal, RowSet::IndirectDirty, row.cell);
                if self.direct_compose_required {
                    self.journal_insert_row(journal, RowSet::DirectPromotionDirty, row.cell);
                    self.journal_insert_row(journal, RowSet::DirectAnimatedDirty, row.cell);
                }
            }
        }
        // Every id-41/45 row the cluster carries, not only the ones it writes
        // first: a gated cell's terms are copied from whichever row brings
        // them. Parsed again only when a direct cell can take them.
        let sparse_rows = if gpu.sh.mover_channel.wants_terms(&first_cells) {
            self.collect_sparse_rows(cluster_id, &prepared.chunk)?
        } else {
            Vec::new()
        };
        let mut rows = ClusterTermRows {
            channel_rows,
            ..ClusterTermRows::default()
        };
        for (section_id, row) in &sparse_rows {
            let term = ClusterSparseRow {
                cell: row.row,
                entry_offsets: &row.entry_tile_f16_offsets,
                tiles: &row.tile_f16,
            };
            match *section_id {
                DIRECT_DELTA_ID => rows.static_rows.push(term),
                ANIMATED_DIRECT_DELTA_ID => rows.animated_rows.push(term),
                _ => {}
            }
        }
        let writes = gpu
            .sh
            .mover_channel
            .stage_cluster(gpu.device, gpu.queue, &rows, &first_cells)
            .map_err(|error| refuse(&error))?;
        if writes.grew {
            // The grown table is live and holds every earlier word; the
            // passes bound to the old one rebind before anything composes.
            if let Some(pools) = self.gpu.as_mut() {
                pools.rebind_mover_channel(gpu.device, gpu.sh, gpu.selection_weights)?;
            }
        }
        Ok(writes)
    }

    /// Free every pooled cell whose last carrying cluster left since the
    /// last flush, into `uploads`, which is submitted before any later
    /// install can reuse a freed range.
    pub(super) fn flush_mover_releases(
        &mut self,
        uploads: &mut StagedUploads,
        channel: &mut MoverChannelResources,
    ) -> Result<(), ShResidencyDrainError> {
        for cell in std::mem::take(&mut self.released_mover_cells) {
            channel.release_cell(uploads, cell).map_err(|_| {
                ShResidencyDrainError::GpuCapacity {
                    reason: "mover channel cell release is out of bounds",
                }
            })?;
        }
        self.mover_pool_bytes = channel.pool_bytes();
        Ok(())
    }

    /// Refresh the channel's share of the streamed pool for the snapshot.
    pub(super) fn observe_mover_pool(&mut self, channel: &MoverChannelResources) {
        self.mover_pool_bytes = channel.pool_bytes();
    }
}
