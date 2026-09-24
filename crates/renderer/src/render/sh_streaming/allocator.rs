//! Deterministic CPU mirrors for renderer-owned streamed SH pool ranges.

use std::collections::BTreeMap;

use super::ShResidencyDrainError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PoolRange {
    pub(super) start: u32,
    pub(super) len: u32,
}

/// First-fit allocation is deliberately deterministic: residency policy never
/// observes addresses, but deterministic reuse makes stale-address regressions
/// reproducible in CPU tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct FirstFitRanges {
    pub(super) capacity: u32,
    free: BTreeMap<u32, u32>,
}

/// The exact inverse of one [`FirstFitRanges::allocate_undoable`]. Unlike
/// `release`, it also restores the address high-water, so a rolled-back
/// allocation leaves no trailing free range behind.
#[derive(Debug, Clone, Copy)]
pub(super) struct AllocationUndo {
    previous_capacity: u32,
    consumed_free: Option<(u32, u32)>,
    remainder_start: Option<u32>,
}

impl FirstFitRanges {
    #[cfg(test)]
    pub(super) fn allocate(&mut self, len: u32) -> Result<PoolRange, ShResidencyDrainError> {
        self.allocate_undoable(len).map(|(range, _)| range)
    }

    pub(super) fn allocate_undoable(
        &mut self,
        len: u32,
    ) -> Result<(PoolRange, AllocationUndo), ShResidencyDrainError> {
        let mut undo = AllocationUndo {
            previous_capacity: self.capacity,
            consumed_free: None,
            remainder_start: None,
        };
        if len == 0 {
            return Ok((PoolRange { start: 0, len: 0 }, undo));
        }
        let candidate = self
            .free
            .iter()
            .find_map(|(&start, &available)| (available >= len).then_some((start, available)));
        if let Some((start, available)) = candidate {
            self.free.remove(&start);
            undo.consumed_free = Some((start, available));
            if available > len {
                self.free.insert(start + len, available - len);
                undo.remainder_start = Some(start + len);
            }
            return Ok((PoolRange { start, len }, undo));
        }

        let start = self.capacity;
        self.capacity = self
            .capacity
            .checked_add(len)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        Ok((PoolRange { start, len }, undo))
    }

    /// Reverse one allocation. Undos must be applied newest-first.
    pub(super) fn undo_allocation(&mut self, undo: AllocationUndo) {
        if let Some(start) = undo.remainder_start {
            self.free.remove(&start);
        }
        if let Some((start, available)) = undo.consumed_free {
            self.free.insert(start, available);
        }
        self.capacity = undo.previous_capacity;
    }

    pub(super) fn release(&mut self, range: PoolRange) -> Result<(), ShResidencyDrainError> {
        if range.len == 0 {
            return Ok(());
        }
        let mut start = range.start;
        let mut len = range.len;
        if let Some((&previous_start, &previous_len)) = self.free.range(..start).next_back()
            && previous_start.checked_add(previous_len) == Some(start)
        {
            self.free.remove(&previous_start);
            start = previous_start;
            len = len
                .checked_add(previous_len)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
        }
        if let Some((&next_start, &next_len)) = self.free.range(start..).next()
            && start.checked_add(len) == Some(next_start)
        {
            self.free.remove(&next_start);
            len = len
                .checked_add(next_len)
                .ok_or(ShResidencyDrainError::SlotOverflow)?;
        }
        self.free.insert(start, len);
        Ok(())
    }
}

/// Sparse compose rows hold a pair into entry metadata and a separate f16-tile
/// range. Pair zero remains the sole missing-row sentinel and is reset before
/// either allocation is returned for reuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SparsePool {
    pub(super) entries: FirstFitRanges,
    pub(super) tiles: FirstFitRanges,
    pub(super) row_pairs: Vec<[u32; 2]>,
    live: BTreeMap<u32, LiveSparseRanges>,
}

/// The f16 backing range is deliberately rounded to a full packed-u32 word.
/// Keep the source payload length separately so the renderer's logical
/// occupancy ledger stays in lockstep with the codec's byte accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LiveSparseRanges {
    entries: PoolRange,
    tiles: PoolRange,
    logical_tile_f16: u32,
}

/// The exact inverse of one [`SparsePool::install_undoable`].
#[derive(Debug, Clone, Copy)]
pub(super) struct SparseInstallUndo {
    row: u32,
    previous_pair: [u32; 2],
    entries: AllocationUndo,
    tiles: AllocationUndo,
}

impl SparsePool {
    pub(super) fn logical_occupancy(&self) -> (u32, u32) {
        self.live
            .values()
            .try_fold((0u32, 0u32), |(entries, tiles), ranges| {
                Some((
                    entries.checked_add(ranges.entries.len)?,
                    tiles.checked_add(ranges.logical_tile_f16)?,
                ))
            })
            .expect("validated streamed sparse ranges must fit the u32 logical ledger")
    }

    pub(super) fn new(row_count: usize) -> Self {
        Self {
            entries: FirstFitRanges {
                capacity: 1,
                free: BTreeMap::new(),
            },
            tiles: FirstFitRanges {
                // Two f16 halves form one storage word. Reserve the dummy
                // word so every live row starts on an even half offset; its
                // entries may still have odd internal offsets.
                capacity: 2,
                free: BTreeMap::new(),
            },
            row_pairs: vec![[0, 0]; row_count],
            live: BTreeMap::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn install(
        &mut self,
        row: u32,
        entries: u32,
        tile_f16: u32,
    ) -> Result<(PoolRange, PoolRange), ShResidencyDrainError> {
        self.install_undoable(row, entries, tile_f16)
            .map(|(entries, tiles, _)| (entries, tiles))
    }

    /// Install one row and return the exact inverse alongside its ranges. A
    /// refused row leaves the pool untouched.
    pub(super) fn install_undoable(
        &mut self,
        row: u32,
        entries: u32,
        tile_f16: u32,
    ) -> Result<(PoolRange, PoolRange, SparseInstallUndo), ShResidencyDrainError> {
        let row_index =
            usize::try_from(row).map_err(|_| ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "sparse row does not fit platform index",
            })?;
        let Some(&previous_pair) = self.row_pairs.get(row_index) else {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "sparse row exceeds metadata",
            });
        };
        if self.live.contains_key(&row) {
            return Err(ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "sparse row already has a live owner",
            });
        }
        // The GPU carrier packs two f16 halves per u32.  Rows may contain an
        // odd number of halves, but their allocation must leave one padding
        // half so the following row can start on an even word boundary.
        let reserved_tile_f16 = tile_f16
            .checked_add(tile_f16 & 1)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let (entry_range, entries_undo) = self.entries.allocate_undoable(entries)?;
        let (tile_range, tiles_undo) = match self.tiles.allocate_undoable(reserved_tile_f16) {
            Ok(allocated) => allocated,
            Err(error) => {
                self.entries.undo_allocation(entries_undo);
                return Err(error);
            }
        };
        let Some(end) = entry_range.start.checked_add(entry_range.len) else {
            self.tiles.undo_allocation(tiles_undo);
            self.entries.undo_allocation(entries_undo);
            return Err(ShResidencyDrainError::SlotOverflow);
        };
        self.row_pairs[row_index] = [entry_range.start, end];
        self.live.insert(
            row,
            LiveSparseRanges {
                entries: entry_range,
                tiles: tile_range,
                logical_tile_f16: tile_f16,
            },
        );
        let undo = SparseInstallUndo {
            row,
            previous_pair,
            entries: entries_undo,
            tiles: tiles_undo,
        };
        Ok((entry_range, tile_range, undo))
    }

    /// Reverse one installed row. Undos must be applied newest-first.
    pub(super) fn undo_install(&mut self, undo: SparseInstallUndo) {
        self.live.remove(&undo.row);
        self.row_pairs[undo.row as usize] = undo.previous_pair;
        self.tiles.undo_allocation(undo.tiles);
        self.entries.undo_allocation(undo.entries);
    }

    pub(super) fn evict(
        &mut self,
        row: u32,
    ) -> Result<Option<(PoolRange, PoolRange)>, ShResidencyDrainError> {
        let Some(ranges) = self.live.remove(&row) else {
            return Ok(None);
        };
        if let Some(pair) = self.row_pairs.get_mut(row as usize) {
            *pair = [0, 0];
        }
        self.entries.release(ranges.entries)?;
        self.tiles.release(ranges.tiles)?;
        Ok(Some((ranges.entries, ranges.tiles)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dense_growth_and_eviction_reuse_a_stale_slot_without_relocating_live_slots() {
        let mut pool = FirstFitRanges::default();
        let a = pool.allocate(3).unwrap();
        let b = pool.allocate(2).unwrap();
        pool.release(a).unwrap();
        assert_eq!(pool.allocate(2).unwrap(), PoolRange { start: 0, len: 2 });
        pool.release(b).unwrap();
        pool.release(PoolRange { start: 0, len: 2 }).unwrap();
        assert_eq!(pool.allocate(5).unwrap(), PoolRange { start: 0, len: 5 });
    }

    #[test]
    fn allocation_undo_restores_the_free_list_and_high_water_exactly() {
        let mut pool = FirstFitRanges::default();
        let a = pool.allocate(4).unwrap();
        pool.allocate(2).unwrap();
        pool.release(a).unwrap();
        let before = pool.clone();

        // Newest-first undo of a split free range, an exact fit, and a tail
        // extension restores the original allocator, not a released copy.
        let (_, split) = pool.allocate_undoable(1).unwrap();
        let (_, exact) = pool.allocate_undoable(3).unwrap();
        let (tail, extend) = pool.allocate_undoable(5).unwrap();
        assert_eq!(tail, PoolRange { start: 6, len: 5 });
        pool.undo_allocation(extend);
        pool.undo_allocation(exact);
        pool.undo_allocation(split);
        assert_eq!(pool, before);
    }

    #[test]
    fn sparse_install_undo_restores_the_pool_exactly() {
        let mut pool = SparsePool::new(3);
        pool.install(0, 2, 3).unwrap();
        let before = pool.clone();
        let (_, _, first) = pool.install_undoable(1, 1, 5).unwrap();
        let (_, _, second) = pool.install_undoable(2, 4, 2).unwrap();
        pool.undo_install(second);
        pool.undo_install(first);
        assert_eq!(pool, before);
    }

    #[test]
    fn sparse_eviction_clears_a_reachable_pair_before_same_drain_reuse() {
        let mut pool = SparsePool::new(2);
        let (_, first_tiles) = pool.install(0, 1, 3).unwrap();
        assert_eq!(pool.row_pairs[0], [1, 2]);
        assert_eq!(first_tiles, PoolRange { start: 2, len: 4 });
        assert_eq!(pool.logical_occupancy(), (1, 3));
        pool.evict(0).unwrap();
        assert_eq!(pool.row_pairs[0], [0, 0]);
        let (_, second_tiles) = pool.install(1, 1, 1).unwrap();
        assert_eq!(pool.row_pairs[1], [1, 2]);
        assert_eq!(second_tiles.start % 2, 0);
        assert_eq!(pool.logical_occupancy(), (1, 1));
    }
}
