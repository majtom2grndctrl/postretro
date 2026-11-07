// Compaction: the repack's escalation, a from-scratch layout reached through the spare.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// Works on one pool's placements and spare, knowing nothing of any other pool.
// The target is packed and its copy order planned before the model changes, so
// a target with no order leaves nothing to undo and no partial copy list.

use postretro_level_format::lightmap_pool::PoolKind;

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
    /// The pool's resident mandatory and visible items and the drain's pairs'
    /// items, tallest first.
    keep: Vec<u32>,
    /// Mandatory and visible targets' items with no pair here, tallest first.
    waiting: Vec<u32>,
    /// `keep` and `waiting` together, tallest first.
    whole: Vec<u32>,
    /// Resident band items, nearest lead first.
    band: Vec<u32>,
    /// Every placed item in packing order, the order that replays the layout
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
    /// Repack `kind`'s pool from scratch for its whole targeted demand, tallest
    /// first, then every resident band item that still fits; a band item that
    /// does not drops its block from both pools. Items may land in any layer,
    /// including a higher one. Items with no texels here take reserved room. A
    /// demand that does not pack still seats resident items and `pending`, then
    /// reserves room for the rest as far as it lasts.
    ///
    /// Returns the copy order's figures, with the plan's copies written and
    /// the pool moved to the target. Returns `None`, with the pool and the
    /// plan's copies untouched, when `pending` does not pack, or when no
    /// order through the spare was found, even with every band item dropped;
    /// the plan then records the rollback, and the drain falls back to the
    /// downward pass, then growth. Any end but a whole demand seated spends
    /// the demand's one compaction.
    pub(super) fn compact(&mut self, kind: PoolKind, pending: &[u32]) -> Option<CopyOrderStats> {
        #[cfg(test)]
        if self.downward_only {
            return None;
        }
        let mut c = std::mem::take(&mut self.scratch.compaction);
        let compacted = self.compact_with(kind, pending, &mut c);
        self.scratch.compaction = c;
        compacted
    }

    fn compact_with(
        &mut self,
        kind: PoolKind,
        pending: &[u32],
        c: &mut CompactionScratch,
    ) -> Option<CopyOrderStats> {
        self.sort_demand(kind, pending, c);
        let mut layout = self.try_layout(kind, RepackScope::WholeDemand, c);
        if matches!(layout, Layout::Planned(_)) {
            return self.commit_layout(kind, layout, c);
        }
        self.spend_compaction(kind);
        if matches!(layout, Layout::DoesNotPack) {
            layout = self.try_layout(kind, RepackScope::DrainPairs, c);
        }
        if matches!(layout, Layout::NoOrder) {
            log::warn!(
                "[Renderer] Compaction of the {} pool found no copy order through the spare for \
                 {} items in {} layers; the drain falls back to the downward pass, then growth",
                kind.name(),
                c.moves.len(),
                self.pools[kind].layers,
            );
            self.plan.pools[kind].compaction_rolled_back = true;
        }
        self.commit_layout(kind, layout, c)
    }

    fn commit_layout(
        &mut self,
        kind: PoolKind,
        layout: Layout,
        c: &CompactionScratch,
    ) -> Option<CopyOrderStats> {
        let Layout::Planned(stats) = layout else {
            return None;
        };
        self.commit_target(kind, c);
        Some(stats)
    }

    /// Pack `scope`'s demand and plan its copies, with the band if it may stay
    /// and then without.
    fn try_layout(
        &mut self,
        kind: PoolKind,
        scope: RepackScope,
        c: &mut CompactionScratch,
    ) -> Layout {
        let band_may_stay = !c.band.is_empty();
        for keep_band in [band_may_stay, false] {
            let band: &[u32] = if keep_band { &c.band } else { &[] };
            let packed = match scope {
                RepackScope::WholeDemand => {
                    self.pack_target(kind, &c.whole, &[], band, &mut c.target)
                }
                RepackScope::DrainPairs => {
                    self.pack_target(kind, &c.keep, &c.waiting, band, &mut c.target)
                }
            };
            if !packed {
                return Layout::DoesNotPack;
            }
            if let Some(stats) = self.plan_target_copies(kind, c) {
                return Layout::Planned(stats);
            }
            if !band_may_stay {
                break;
            }
        }
        Layout::NoOrder
    }

    fn sort_demand(&self, kind: PoolKind, pending: &[u32], c: &mut CompactionScratch) {
        c.keep.clear();
        c.band.clear();
        c.whole.clear();
        for &item in &self.pools[kind].resident {
            if self.is_band(kind.block_of(item)) {
                c.band.push(item);
            } else {
                c.keep.push(item);
            }
        }
        c.keep.extend_from_slice(pending);
        self.collect_waiting(kind, &mut c.waiting);
        c.whole.extend_from_slice(&c.keep);
        c.whole.extend_from_slice(&c.waiting);
        self.sort_tallest_first(kind, &mut c.keep);
        self.sort_tallest_first(kind, &mut c.waiting);
        self.sort_tallest_first(kind, &mut c.whole);
        c.band.sort_unstable_by_key(|&item| {
            let lead = self.target(kind.block_of(item)).map_or(0, |t| t.lead);
            (lead, item)
        });
    }

    /// Pack `keep`, then `extra` and `band` as far as each fits, on a scratch
    /// allocator: a target that does not fit or has no copy order never
    /// touches the pool. False when an item of `keep` does not fit.
    fn pack_target(
        &self,
        kind: PoolKind,
        keep: &[u32],
        extra: &[u32],
        band: &[u32],
        target: &mut Vec<(u32, Slot)>,
    ) -> bool {
        let state = &self.pools[kind];
        let layers = Some(state.layers as usize);
        let mut pool = BlockPool::new(state.edge(), None);
        target.clear();
        for &item in keep {
            let (width, height) = state.alloc_extent(item);
            match pool.allocate_within(width, height, layers) {
                Some(slot) => target.push((item, slot)),
                None => return false,
            }
        }
        for &item in extra.iter().chain(band) {
            let (width, height) = state.alloc_extent(item);
            if let Some(slot) = pool.allocate_within(width, height, layers) {
                target.push((item, slot));
            }
        }
        true
    }

    fn plan_target_copies(
        &mut self,
        kind: PoolKind,
        c: &mut CompactionScratch,
    ) -> Option<CopyOrderStats> {
        let pool = &self.pools[kind];
        c.moves.clear();
        c.moves
            .extend(c.target.iter().map(|&(item, slot)| BlockMove {
                src: pool.slot(item).map(placement_of),
                dst: placement_of(slot),
                width: slot.width,
                height: slot.height,
            }));
        plan_copy_order(
            CopyOrderInput {
                moves: &c.moves,
                layers: pool.layers,
                edge: pool.edge(),
            },
            &mut c.copy_order,
            &mut self.plan.pools[kind].copies,
        )
        .ok()
    }

    /// Move the pool to the planned target. Replaying the packing order
    /// rebuilds the packed shelves on the layers the planner landed them on.
    /// A band item left out of the target drops its block from both pools, and
    /// a target left out holds no room. An item with no texels takes reserved
    /// room, never a placement: nothing is uploading into it until its whole
    /// pair lands.
    fn commit_target(&mut self, kind: PoolKind, c: &CompactionScratch) {
        self.return_all_room(kind);
        for &item in c.keep.iter().chain(&c.band) {
            if self.pools[kind].slot(item).is_some() {
                self.release_item(kind, item, EvictionReason::Pressure);
            }
        }
        for (&(item, slot), planned) in c.target.iter().zip(&c.moves) {
            let home = c.copy_order.homes[slot.layer as usize];
            let placed = self.pools[kind]
                .allocator
                .allocate_at(home, slot.x, slot.y, slot.width, slot.height)
                .expect("the packing order replays onto layers it emptied");
            if planned.src.is_none() {
                self.reserve(kind, item, placed);
            } else {
                self.place(kind, item, placed);
            }
        }
        for &item in &c.band {
            self.drop_unseated_band(kind, item);
        }
    }
}
