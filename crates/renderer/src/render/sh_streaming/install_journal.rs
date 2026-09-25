//! Undo journal for one in-flight cluster install.
//!
//! An install mutates the residency mirrors in place and records the exact
//! inverse of each mutation. A failed install replays those inverses
//! newest-first, so rollback costs time proportional to the work the install
//! did, never to the map or to how much of it is resident. A successful
//! install drops the journal.

use super::allocator::{AllocationUndo, SparseInstallUndo};
use super::*;

#[derive(Debug)]
enum Undo {
    DenseAllocation(AllocationUndo),
    NodeSlot(StoredNode),
    ComposeWord {
        dense: u32,
        previous: u32,
    },
    DirtyRow((u32, u32)),
    RowInserted {
        set: RowSet,
        row: u32,
    },
    RowRef {
        table: RowRefTable,
        row: u32,
        count: u32,
    },
    SparseRow {
        section_id: u32,
        undo: SparseInstallUndo,
    },
}

#[derive(Debug, Default)]
pub(super) struct InstallJournal {
    undo: Vec<Undo>,
}

impl ShResidencyState {
    /// Allocate a canonical node's dense range and publish it in the node map.
    pub(super) fn journal_allocate_node(
        &mut self,
        journal: &mut InstallJournal,
        node: StoredNode,
        tile_count: u32,
    ) -> Result<(), ShResidencyDrainError> {
        let (range, undo) = self.dense_slots.allocate_undoable(tile_count)?;
        journal.undo.push(Undo::DenseAllocation(undo));
        let previous = self.node_slots.insert(node, range);
        debug_assert!(previous.is_none(), "install allocates only absent nodes");
        journal.undo.push(Undo::NodeSlot(node));
        Ok(())
    }

    pub(super) fn journal_compose_word(
        &mut self,
        journal: &mut InstallJournal,
        cluster_id: u32,
        dense: u32,
        word: u32,
    ) -> Result<(), ShResidencyDrainError> {
        let destination = self.compose_words.get_mut(dense as usize).ok_or(
            ShResidencyDrainError::MissingDenseOwner {
                cluster_id,
                dense_index: dense,
            },
        )?;
        let previous = std::mem::replace(destination, word);
        journal.undo.push(Undo::ComposeWord { dense, previous });
        Ok(())
    }

    pub(super) fn journal_dirty_row(
        &mut self,
        journal: &mut InstallJournal,
        section_id: u32,
        row: u32,
    ) {
        if self.dirty_rows.insert((section_id, row)) {
            journal.undo.push(Undo::DirtyRow((section_id, row)));
        }
    }

    pub(super) fn journal_insert_row(
        &mut self,
        journal: &mut InstallJournal,
        set: RowSet,
        row: u32,
    ) {
        if self.row_set_mut(set).insert(row) {
            journal.undo.push(Undo::RowInserted { set, row });
        }
    }

    /// Count `count` more contributors to `row` and keep the resident unions
    /// that table feeds in step, without rebuilding them.
    pub(super) fn journal_add_row_refs(
        &mut self,
        journal: &mut InstallJournal,
        table: RowRefTable,
        row: u32,
        count: u32,
    ) -> Result<(), ShResidencyDrainError> {
        increment_row_ref(self.row_ref_table_mut(table), row, count)?;
        journal.undo.push(Undo::RowRef { table, row, count });
        for &set in table.resident_sets() {
            self.journal_insert_row(journal, set, row);
        }
        Ok(())
    }

    pub(super) fn journal_install_sparse_row(
        &mut self,
        journal: &mut InstallJournal,
        section_id: u32,
        row: u32,
        entries: u32,
        tile_f16: u32,
    ) -> Result<(PoolRange, PoolRange), ShResidencyDrainError> {
        let pool = self.sparse_pools.get_mut(&section_id).ok_or(
            ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "sparse source disappeared during install",
            },
        )?;
        let (entry_range, tile_range, undo) = pool.install_undoable(row, entries, tile_f16)?;
        journal.undo.push(Undo::SparseRow { section_id, undo });
        Ok((entry_range, tile_range))
    }

    /// Restore every mirror the journal touched to its pre-install value.
    pub(super) fn roll_back(&mut self, journal: InstallJournal) {
        for undo in journal.undo.into_iter().rev() {
            match undo {
                Undo::DenseAllocation(undo) => self.dense_slots.undo_allocation(undo),
                Undo::NodeSlot(node) => {
                    self.node_slots.remove(&node);
                }
                Undo::ComposeWord { dense, previous } => {
                    self.compose_words[dense as usize] = previous;
                }
                Undo::DirtyRow(key) => {
                    self.dirty_rows.remove(&key);
                }
                Undo::RowInserted { set, row } => {
                    self.row_set_mut(set).remove(&row);
                }
                Undo::RowRef { table, row, count } => {
                    decrement_row_ref(self.row_ref_table_mut(table), row, count)
                        .expect("an install journal only releases row refs it added");
                }
                Undo::SparseRow { section_id, undo } => self
                    .sparse_pools
                    .get_mut(&section_id)
                    .expect("an install journal only names pools it allocated from")
                    .undo_install(undo),
            }
        }
    }
}
