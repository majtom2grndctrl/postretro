// Compaction: the repack's escalation, a from-scratch layout reached through the spare.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// Works on one pool's placements and spare, knowing nothing of any other pool.
// The target is packed and its copy order planned before the model changes, so
// a target with no order leaves nothing to undo and no partial copy list.

use std::cmp::Reverse;

use super::super::{BlockPool, Slot};
use super::copy_order::{
    BlockMove, CopyOrderInput, CopyOrderScratch, CopyOrderStats, plan_copy_order,
};
use super::plan::EvictionReason;
use super::{LightmapPoolModel, placement_of};

/// Reused compaction buffers.
#[derive(Debug, Default)]
pub(super) struct CompactionScratch {
    /// Mandatory and visible blocks, resident and pending, tallest first.
    keep: Vec<u32>,
    /// Resident band blocks, nearest lead first.
    band: Vec<u32>,
    /// Every placed block in packing order, the order that replays the layout
    /// into the pool. A slot's layer numbers the target's layers.
    target: Vec<(u32, Slot)>,
    moves: Vec<BlockMove>,
    copy_order: CopyOrderScratch,
}

impl CompactionScratch {
    #[cfg(test)]
    pub(super) fn capacities(&self) -> Vec<usize> {
        let mut capacities = vec![
            self.keep.capacity(),
            self.band.capacity(),
            self.target.capacity(),
            self.moves.capacity(),
        ];
        capacities.extend(self.copy_order.capacities());
        capacities
    }
}

impl LightmapPoolModel {
    /// Repack every mandatory and visible block, and `pending`, from scratch,
    /// tallest first, and every resident band block that still fits; a band
    /// block that does not is dropped. Blocks may land in any layer, including
    /// a higher one. A pool past its cap drops its band outright: a target
    /// layer lands on whichever layer clears, which may be past the cap.
    ///
    /// Returns the copy order's figures, with the plan's copies written and
    /// the model moved to the target. Returns `None`, with the model and the
    /// plan's copies untouched, when the demand does not pack, or when no
    /// order through the spare was found, even with every band block dropped;
    /// the plan then records the rollback and the demand falls through to
    /// growth.
    pub(super) fn compact(&mut self, pending: &[u32]) -> Option<CopyOrderStats> {
        let mut c = std::mem::take(&mut self.scratch.compaction);
        let compacted = self.compact_with(pending, &mut c);
        self.scratch.compaction = c;
        compacted
    }

    fn compact_with(
        &mut self,
        pending: &[u32],
        c: &mut CompactionScratch,
    ) -> Option<CopyOrderStats> {
        self.sort_demand(pending, c);
        let band_may_stay = !c.band.is_empty() && self.layers <= self.cap_layers;
        for keep_band in [band_may_stay, false] {
            let band: &[u32] = if keep_band { &c.band } else { &[] };
            if !self.pack_target(&c.keep, band, &mut c.target) {
                return None;
            }
            if let Some(stats) = self.plan_target_copies(c) {
                self.commit_target(c);
                return Some(stats);
            }
            if !band_may_stay {
                break;
            }
        }
        log::warn!(
            "[Lightmap] Compaction found no copy order through the spare for {} blocks in {} \
             layers; demand falls through to growth",
            c.moves.len(),
            self.layers,
        );
        self.plan.compaction_rolled_back = true;
        None
    }

    fn sort_demand(&self, pending: &[u32], c: &mut CompactionScratch) {
        c.keep.clear();
        c.band.clear();
        for &block in &self.resident {
            if self.is_band(block) {
                c.band.push(block);
            } else {
                c.keep.push(block);
            }
        }
        c.keep.extend_from_slice(pending);
        c.keep.sort_unstable_by_key(|&block| {
            let (width, height) = self.alloc_extent(block);
            (Reverse(height), Reverse(width), block)
        });
        c.band
            .sort_unstable_by_key(|&block| (self.target(block).map_or(0, |t| t.lead), block));
    }

    /// Pack `keep`, and `band` as far as it fits, on a scratch allocator: a
    /// target that does not fit or has no copy order never touches the pool.
    /// False when a block of `keep` does not fit.
    fn pack_target(&self, keep: &[u32], band: &[u32], target: &mut Vec<(u32, Slot)>) -> bool {
        let layers = Some(self.layers as usize);
        let mut pool = BlockPool::new(self.edge, None);
        target.clear();
        for &block in keep {
            let (width, height) = self.alloc_extent(block);
            match pool.allocate_within(width, height, layers) {
                Some(slot) => target.push((block, slot)),
                None => return false,
            }
        }
        for &block in band {
            let (width, height) = self.alloc_extent(block);
            if let Some(slot) = pool.allocate_within(width, height, layers) {
                target.push((block, slot));
            }
        }
        true
    }

    fn plan_target_copies(&mut self, c: &mut CompactionScratch) -> Option<CopyOrderStats> {
        c.moves.clear();
        c.moves
            .extend(c.target.iter().map(|&(block, slot)| BlockMove {
                src: self.slots[block as usize].map(placement_of),
                dst: placement_of(slot),
                width: slot.width,
                height: slot.height,
            }));
        plan_copy_order(
            CopyOrderInput {
                moves: &c.moves,
                layers: self.layers,
                edge: self.edge,
            },
            &mut c.copy_order,
            &mut self.plan.copies,
        )
        .ok()
    }

    /// Move the model to the planned target. Replaying the packing order
    /// rebuilds the packed shelves on the layers the planner landed them on.
    /// A band block left out of the target stays released.
    fn commit_target(&mut self, c: &CompactionScratch) {
        for &block in c.keep.iter().chain(&c.band) {
            if self.is_resident(block) {
                self.release(block, EvictionReason::Pressure);
            }
        }
        for &(block, slot) in &c.target {
            let home = c.copy_order.homes[slot.layer as usize];
            let placed = self
                .pool
                .allocate_at(home, slot.x, slot.y, slot.width, slot.height)
                .expect("the packing order replays onto layers it emptied");
            self.place(block, placed);
        }
    }
}
