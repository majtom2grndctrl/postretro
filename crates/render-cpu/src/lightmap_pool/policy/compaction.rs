// Compaction: the repack's escalation, a from-scratch layout reached through the spare.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// Works on one pool's placements and spare, knowing nothing of any other pool.
// The target is packed and its copy order planned before the model changes, so
// a target with no order leaves nothing to undo and no partial copy list.

use super::super::{BlockPool, Slot};
use super::copy_order::{
    BlockMove, CopyOrderInput, CopyOrderScratch, CopyOrderStats, plan_copy_order,
};
use super::plan::EvictionReason;
use super::reservation::RepackScope;
use super::{LightmapPoolModel, placement_of};

/// How one attempt at a target layout ended.
enum Layout {
    /// The copies are in the plan.
    Planned(CopyOrderStats),
    DoesNotPack,
    NoOrder,
}

/// Reused compaction buffers.
#[derive(Debug, Default)]
pub(super) struct CompactionScratch {
    /// Resident mandatory and visible blocks and the drain's pairs, tallest
    /// first.
    keep: Vec<u32>,
    /// Mandatory and visible targets with no pair here, tallest first.
    waiting: Vec<u32>,
    /// `keep` and `waiting` together, tallest first.
    whole: Vec<u32>,
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
            self.waiting.capacity(),
            self.whole.capacity(),
            self.band.capacity(),
            self.target.capacity(),
            self.moves.capacity(),
        ];
        capacities.extend(self.copy_order.capacities());
        capacities
    }
}

impl LightmapPoolModel {
    /// Repack the whole targeted demand from scratch, tallest first, then every
    /// resident band block that still fits; a band block that does not is
    /// dropped. Blocks may land in any layer, including a higher one. Targets
    /// not ready take reserved room. A demand that does not pack still seats
    /// resident blocks and `pending`, then reserves room for the rest as far as
    /// it lasts. A pool past its cap drops its band outright: a target layer
    /// lands on whichever layer clears, which may be past the cap.
    ///
    /// Returns the copy order's figures, with the plan's copies written and
    /// the model moved to the target. Returns `None`, with the model and the
    /// plan's copies untouched, when `pending` does not pack, or when no
    /// order through the spare was found, even with every band block dropped;
    /// the plan then records the rollback and the demand falls through to
    /// growth. Any end but a whole demand seated spends the demand's one
    /// compaction.
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
        let mut layout = self.try_layout(RepackScope::WholeDemand, c);
        if matches!(layout, Layout::Planned(_)) {
            return self.commit_layout(layout, c);
        }
        self.spend_compaction();
        if matches!(layout, Layout::DoesNotPack) {
            layout = self.try_layout(RepackScope::DrainPairs, c);
        }
        if matches!(layout, Layout::NoOrder) {
            log::warn!(
                "[Lightmap] Compaction found no copy order through the spare for {} blocks in {} \
                 layers; demand falls through to growth",
                c.moves.len(),
                self.layers,
            );
            self.plan.compaction_rolled_back = true;
        }
        self.commit_layout(layout, c)
    }

    fn commit_layout(&mut self, layout: Layout, c: &CompactionScratch) -> Option<CopyOrderStats> {
        let Layout::Planned(stats) = layout else {
            return None;
        };
        self.commit_target(c);
        Some(stats)
    }

    /// Pack `scope`'s demand and plan its copies, with the band if it may stay
    /// and then without.
    fn try_layout(&mut self, scope: RepackScope, c: &mut CompactionScratch) -> Layout {
        let band_may_stay = !c.band.is_empty() && self.layers <= self.cap_layers;
        for keep_band in [band_may_stay, false] {
            let band: &[u32] = if keep_band { &c.band } else { &[] };
            let packed = match scope {
                RepackScope::WholeDemand => self.pack_target(&c.whole, &[], band, &mut c.target),
                RepackScope::DrainPairs => {
                    self.pack_target(&c.keep, &c.waiting, band, &mut c.target)
                }
            };
            if !packed {
                return Layout::DoesNotPack;
            }
            if let Some(stats) = self.plan_target_copies(c) {
                return Layout::Planned(stats);
            }
            if !band_may_stay {
                break;
            }
        }
        Layout::NoOrder
    }

    fn sort_demand(&self, pending: &[u32], c: &mut CompactionScratch) {
        c.keep.clear();
        c.band.clear();
        c.whole.clear();
        for &block in &self.resident {
            if self.is_band(block) {
                c.band.push(block);
            } else {
                c.keep.push(block);
            }
        }
        c.keep.extend_from_slice(pending);
        self.collect_waiting(&mut c.waiting);
        c.whole.extend_from_slice(&c.keep);
        c.whole.extend_from_slice(&c.waiting);
        self.sort_tallest_first(&mut c.keep);
        self.sort_tallest_first(&mut c.waiting);
        self.sort_tallest_first(&mut c.whole);
        c.band
            .sort_unstable_by_key(|&block| (self.target(block).map_or(0, |t| t.lead), block));
    }

    /// Pack `keep`, then `extra` and `band` as far as each fits, on a scratch
    /// allocator: a target that does not fit or has no copy order never
    /// touches the pool. False when a block of `keep` does not fit.
    fn pack_target(
        &self,
        keep: &[u32],
        extra: &[u32],
        band: &[u32],
        target: &mut Vec<(u32, Slot)>,
    ) -> bool {
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
        for &block in extra.iter().chain(band) {
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
    /// A band block left out of the target stays released, and a target left
    /// out holds no room. A target with no texels and no pair takes reserved
    /// room, never a placement: nothing is uploading into it.
    fn commit_target(&mut self, c: &CompactionScratch) {
        self.return_all_room();
        for &block in c.keep.iter().chain(&c.band) {
            if self.is_resident(block) {
                self.release(block, EvictionReason::Pressure);
            }
        }
        for (&(block, slot), planned) in c.target.iter().zip(&c.moves) {
            let home = c.copy_order.homes[slot.layer as usize];
            let placed = self
                .pool
                .allocate_at(home, slot.x, slot.y, slot.width, slot.height)
                .expect("the packing order replays onto layers it emptied");
            if planned.src.is_none() && !self.is_pair(block) {
                self.reserve(block, placed);
            } else {
                self.place(block, placed);
            }
        }
    }
}
