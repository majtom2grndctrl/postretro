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
#[derive(Debug, Clone, Default)]
pub(super) struct FirstFitRanges {
    pub(super) capacity: u32,
    free: BTreeMap<u32, u32>,
}

impl FirstFitRanges {
    pub(super) fn free_capacity(&self) -> u32 {
        self.free.values().copied().fold(0u32, u32::saturating_add)
    }

    pub(super) fn allocate(&mut self, len: u32) -> Result<PoolRange, ShResidencyDrainError> {
        if len == 0 {
            return Ok(PoolRange { start: 0, len: 0 });
        }
        let candidate = self
            .free
            .iter()
            .find_map(|(&start, &available)| (available >= len).then_some((start, available)));
        if let Some((start, available)) = candidate {
            self.free.remove(&start);
            if available > len {
                self.free.insert(start + len, available - len);
            }
            return Ok(PoolRange { start, len });
        }

        let start = self.capacity;
        self.capacity = self
            .capacity
            .checked_add(len)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        Ok(PoolRange { start, len })
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
#[derive(Debug, Clone)]
pub(super) struct SparsePool {
    pub(super) entries: FirstFitRanges,
    pub(super) tiles: FirstFitRanges,
    pub(super) row_pairs: Vec<[u32; 2]>,
    live: BTreeMap<u32, LiveSparseRanges>,
}

/// The f16 backing range is deliberately rounded to a full packed-u32 word.
/// Keep the source payload length separately so the renderer's logical
/// occupancy ledger stays in lockstep with the codec's byte accounting.
#[derive(Debug, Clone, Copy)]
struct LiveSparseRanges {
    entries: PoolRange,
    tiles: PoolRange,
    logical_tile_f16: u32,
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

    pub(super) fn install(
        &mut self,
        row: u32,
        entries: u32,
        tile_f16: u32,
    ) -> Result<(PoolRange, PoolRange), ShResidencyDrainError> {
        let row_index =
            usize::try_from(row).map_err(|_| ShResidencyDrainError::MalformedChunk {
                cluster_id: 0,
                reason: "sparse row does not fit platform index",
            })?;
        let Some(pair) = self.row_pairs.get_mut(row_index) else {
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
        let entry_range = self.entries.allocate(entries)?;
        // The GPU carrier packs two f16 halves per u32.  Rows may contain an
        // odd number of halves, but their allocation must leave one padding
        // half so the following row can start on an even word boundary.
        let reserved_tile_f16 = tile_f16
            .checked_add(tile_f16 & 1)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        let tile_range = match self.tiles.allocate(reserved_tile_f16) {
            Ok(range) => range,
            Err(error) => {
                self.entries.release(entry_range)?;
                return Err(error);
            }
        };
        let end = entry_range
            .start
            .checked_add(entry_range.len)
            .ok_or(ShResidencyDrainError::SlotOverflow)?;
        *pair = [entry_range.start, end];
        self.live.insert(
            row,
            LiveSparseRanges {
                entries: entry_range,
                tiles: tile_range,
                logical_tile_f16: tile_f16,
            },
        );
        Ok((entry_range, tile_range))
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
