// Compaction: the repack's escalation, a from-scratch layout reached through the spare.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// Works on one pool's placements and spare, knowing nothing of any other pool.
// It rebuilds the few layers with the most room first and the whole pool last:
// a copy costs by the command, and a small move graph almost always has an
// order. Only a whole-pool round can place every set that packs from scratch,
// and the search does not always reach it: `compaction_fixtures` holds a
// captured graph it cannot order. Each target is packed and its copy order
// planned before the model changes, so a target with no order leaves nothing
// to undo and no partial copy list.

use std::cmp::Reverse;

use super::copy_order::{BlockMove, CopyOrderScratch, CopyOrderStats, plan_copy_order};
use super::plan::EvictionReason;
use super::target_layout::TargetScratch;
use super::{LightmapPoolModel, placement_of};

/// Targets tried for the whole pool before a compaction gives up: the
/// from-scratch pack first, then orders near it that
/// group blocks differently. Which blocks share a layer can decide whether an
/// order exists, and a graph of a few large blocks is where that bites.
const MAX_TARGETS: usize = 16;

/// Bounds the targets a compaction tries, as `copy_order` bounds its orders.
const TARGET_BLOCK_BUDGET: usize = 512;

/// Reused compaction buffers.
#[derive(Debug, Default)]
pub(super) struct CompactionScratch {
    /// Mandatory and visible blocks, resident and pending, tallest first.
    keep: Vec<u32>,
    /// Resident band blocks, nearest lead first.
    band: Vec<u32>,
    /// `(mandatory and visible texels, layer)`, the fewest first.
    layers_by_room: Vec<(u64, u32)>,
    /// The pool layers this round rebuilds, ascending.
    chosen_layers: Vec<u32>,
    target: TargetScratch,
    moves: Vec<BlockMove>,
    copy_order: CopyOrderScratch,
}

impl CompactionScratch {
    #[cfg(test)]
    pub(super) fn capacities(&self) -> Vec<usize> {
        let mut capacities = vec![
            self.keep.capacity(),
            self.band.capacity(),
            self.layers_by_room.capacity(),
            self.chosen_layers.capacity(),
            self.moves.capacity(),
        ];
        capacities.extend(self.target.capacities());
        capacities.extend(self.copy_order.capacities());
        capacities
    }
}

/// How one round of a compaction ended.
enum Round {
    Compacted(CopyOrderStats),
    DoesNotPack,
    NoCopyOrder,
}

impl LightmapPoolModel {
    /// Repack every mandatory and visible block of some layers, and `pending`,
    /// from scratch into those layers, tallest first, and every resident band
    /// block of them that still fits under the cap; a band block that does
    /// not is dropped. Rounds double the layers, those with the most room
    /// first, up to the whole pool. Blocks may land in any chosen layer,
    /// including a higher one.
    ///
    /// Returns the copy order's figures, with the plan's copies written and
    /// the model moved to the target. Returns `None`, with the model and the
    /// plan's copies untouched, when the demand does not pack, or when the
    /// whole pool has no order through the spare, even with every band block
    /// dropped; the plan then records the rollback and the demand falls
    /// through to growth.
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
        self.rank_layers_by_room(c);
        let layers = self.layers as usize;
        let mut round_layers = 1;
        loop {
            let whole_pool = round_layers >= layers;
            self.choose_layers(pending, round_layers.min(layers), c);
            match self.compact_round(c, whole_pool) {
                Round::Compacted(stats) => return Some(stats),
                Round::DoesNotPack | Round::NoCopyOrder if !whole_pool => round_layers *= 2,
                Round::DoesNotPack => return None,
                Round::NoCopyOrder => break,
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

    /// Layers by the texels a compaction must keep in them: a band block
    /// costs nothing to drop, so it does not count against a layer's room.
    fn rank_layers_by_room(&self, c: &mut CompactionScratch) {
        c.layers_by_room.clear();
        c.layers_by_room
            .extend((0..self.layers).map(|layer| (0, layer)));
        for &block in &self.resident {
            if !self.is_band(block) {
                let slot = self.slots[block as usize].expect("resident");
                let texels = u64::from(slot.width) * u64::from(slot.height);
                c.layers_by_room[slot.layer as usize].0 += texels;
            }
        }
        c.layers_by_room.sort_unstable();
    }

    fn choose_layers(&self, pending: &[u32], count: usize, c: &mut CompactionScratch) {
        c.chosen_layers.clear();
        c.chosen_layers
            .extend(c.layers_by_room[..count].iter().map(|&(_, layer)| layer));
        c.chosen_layers.sort_unstable();
        c.keep.clear();
        c.band.clear();
        for &block in &self.resident {
            let layer = self.slots[block as usize].expect("resident").layer;
            if c.chosen_layers.binary_search(&layer).is_err() {
                continue;
            }
            if self.is_band(block) {
                c.band.push(block);
            } else {
                c.keep.push(block);
            }
        }
        c.keep.extend_from_slice(pending);
        c.band
            .sort_unstable_by_key(|&block| (self.target(block).map_or(0, |t| t.lead), block));
    }

    fn compact_round(&mut self, c: &mut CompactionScratch, whole_pool: bool) -> Round {
        let targets = if whole_pool {
            (TARGET_BLOCK_BUDGET / c.keep.len().max(1)).clamp(1, MAX_TARGETS)
        } else {
            1
        };
        for target in 0..targets as u32 {
            self.sort_keep(&mut c.keep, target);
            // A band block costs copies and room, and a pool it fills to the
            // brim leaves the search nowhere to park. With none, the search
            // has the most free space it can get, so one order settles
            // whether the band may stay.
            for keep_band in [true, false] {
                if !keep_band && c.band.is_empty() {
                    continue;
                }
                let band: &[u32] = if keep_band { &c.band } else { &[] };
                if !self.pack_target(&c.keep, band, &c.chosen_layers, &mut c.target) {
                    if target == 0 {
                        return Round::DoesNotPack;
                    }
                    continue;
                }
                let band_can_go = keep_band && !c.band.is_empty();
                if let Some(stats) = self.plan_target_copies(c, band_can_go) {
                    self.commit_target(c);
                    return Round::Compacted(stats);
                }
            }
        }
        Round::NoCopyOrder
    }

    /// Tallest first for target 0, the from-scratch order; later
    /// targets let each block's height drift by up to a quarter layer.
    fn sort_keep(&self, keep: &mut [u32], target: u32) {
        keep.sort_unstable_by_key(|&block| {
            let (width, height) = self.alloc_extent(block);
            let drift = if target == 0 {
                0
            } else {
                drift_of(block, target) % (self.edge / 4 + 1)
            };
            (Reverse(height + drift), Reverse(width), block)
        });
    }

    fn plan_target_copies(
        &mut self,
        c: &mut CompactionScratch,
        first_order_only: bool,
    ) -> Option<CopyOrderStats> {
        c.moves.clear();
        c.moves
            .extend(c.target.slots.iter().filter_map(|&(block, slot)| {
                let at = self.slots[block as usize]?;
                Some(BlockMove {
                    src: placement_of(at),
                    dst: placement_of(slot),
                    width: slot.width,
                    height: slot.height,
                })
            }));
        // Every block outside the chosen layers holds its place.
        c.moves.extend(self.resident.iter().filter_map(|&block| {
            let at = self.slots[block as usize].expect("resident");
            c.chosen_layers.binary_search(&at.layer).is_err().then(|| {
                let place = placement_of(at);
                BlockMove {
                    src: place,
                    dst: place,
                    width: at.width,
                    height: at.height,
                }
            })
        }));
        plan_copy_order(
            &c.moves,
            self.spare_layer(),
            self.edge,
            self.staging_edge(),
            first_order_only,
            &mut c.copy_order,
            &mut self.plan.copies,
        )
        .ok()
    }

    /// Move the model to the planned target. Replaying the packing order
    /// rebuilds the packed shelves wherever the target put them. A band block
    /// of the chosen layers left out of the target stays released.
    fn commit_target(&mut self, c: &CompactionScratch) {
        for &block in c.keep.iter().chain(&c.band) {
            if self.is_resident(block) {
                self.release(block, EvictionReason::Pressure);
            }
        }
        for &(block, slot) in &c.target.slots {
            let placed = self
                .pool
                .allocate_at(slot.layer, slot.x, slot.y, slot.width, slot.height)
                .expect("the packing order replays onto layers it emptied");
            self.place(block, placed);
        }
    }

    /// How much of the spare a compaction may stage in, from its top left.
    fn staging_edge(&self) -> u32 {
        #[cfg(test)]
        if let Some(edge) = self.staging_edge {
            return edge;
        }
        self.edge
    }
}

/// A number that differs per block and per target, the same on every run.
fn drift_of(block: u32, target: u32) -> u32 {
    let mixed = (block ^ target.rotate_left(16)).wrapping_mul(0x9e37_79b1);
    (mixed ^ (mixed >> 15)).wrapping_mul(0x85eb_ca6b) >> 8
}
