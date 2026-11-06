// Downward repack: the first pass, its layout and its copy schedule through the spare layer.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// WebGPU copies within one texture only between distinct subresources, so a
// block cannot move within its own layer directly. This pass never moves a
// block to a higher layer than it sits in, which is what makes its schedule
// this simple; `compaction` is the escalation that may. Each destination
// layer d, in ascending order, is rebuilt in three passes: stage every move
// out of layer d into the spare, copy in every move from a higher layer, then
// copy the staged moves back from the spare. Layers above d are untouched
// until their own turn, so every source is intact when it is read.

use std::cmp::Reverse;

use super::super::BlockPlacement;
use super::plan::{EvictionReason, PoolCopy};
use super::reservation::RepackScope;
use super::{LightmapPoolModel, placement_of};

/// A block a repack must seat.
#[derive(Debug, Clone, Copy)]
pub(super) struct RepackEntry {
    block: u32,
    /// Highest layer + 1 it may land in.
    bound: u32,
    /// Its pair is not here, so it takes reserved room, not a placement.
    waiting: bool,
}

impl LightmapPoolModel {
    /// The downward pass, under the cap and then, in a pool already grown past
    /// it, within its layers: compacting what is already allocated beats
    /// growing again.
    pub(super) fn repack_downward(&mut self, pending: &[u32], scope: RepackScope) -> bool {
        let cap = self.cap_eff();
        self.repack(pending, cap, scope)
            || (self.layers > cap && self.repack(pending, self.layers, scope))
    }

    /// Repack every resident block plus the ready never-refused `pending`
    /// without reads. Mandatory and visible blocks go below `limit` layers
    /// first, each resident one no higher than the layer it sits in (most
    /// constrained first, then tallest); then resident band blocks, nearest
    /// lead first, below the cap. A band block that no longer fits is
    /// dropped. Returns false, with the model untouched, when `scope`'s demand
    /// does not fit.
    ///
    /// Targets not ready take reserved room below `limit`. A whole-demand
    /// repack must seat them with the rest; a drain-pairs repack seats them
    /// after, as far as room lasts.
    fn repack(&mut self, pending: &[u32], limit: u32, scope: RepackScope) -> bool {
        let band_limit = self.cap_eff();
        let checkpoint = self.journal.len();
        let mut keep = std::mem::take(&mut self.scratch.repack_keep);
        let mut band = std::mem::take(&mut self.scratch.repack_band);
        let mut waiting = std::mem::take(&mut self.scratch.repack_waiting);
        let mut residents = std::mem::take(&mut self.scratch.repack_residents);
        keep.clear();
        band.clear();
        self.collect_waiting(&mut waiting);
        residents.clear();
        residents.extend_from_slice(&self.resident);
        residents.sort_unstable();
        for &block in &residents {
            let layer = self.slots[block as usize].expect("resident").layer;
            if self.is_band(block) {
                band.push((block, (layer + 1).min(band_limit)));
            } else {
                keep.push(RepackEntry {
                    block,
                    bound: (layer + 1).min(limit),
                    waiting: false,
                });
            }
        }
        let entry = |waiting| {
            move |&block: &u32| RepackEntry {
                block,
                bound: limit,
                waiting,
            }
        };
        keep.extend(pending.iter().map(entry(false)));
        if scope == RepackScope::WholeDemand {
            keep.extend(waiting.iter().map(entry(true)));
            waiting.clear();
        }
        keep.sort_unstable_by_key(|entry| {
            let (width, height) = self.alloc_extent(entry.block);
            (entry.bound, Reverse(height), Reverse(width), entry.block)
        });
        self.sort_tallest_first(&mut waiting);
        band.sort_unstable_by_key(|&(block, _)| (self.target(block).map_or(0, |t| t.lead), block));

        self.return_all_room();
        for &block in &residents {
            self.release(block, EvictionReason::Pressure);
        }
        let fits = keep.iter().all(|entry| {
            let (width, height) = self.alloc_extent(entry.block);
            let bound = Some(entry.bound as usize);
            match self.pool.allocate_within(width, height, bound) {
                Some(slot) if entry.waiting => {
                    self.reserve(entry.block, slot);
                    true
                }
                Some(slot) => {
                    self.place(entry.block, slot);
                    true
                }
                None => false,
            }
        });
        if fits {
            for &block in &waiting {
                let (width, height) = self.alloc_extent(block);
                if let Some(slot) = self
                    .pool
                    .allocate_within(width, height, Some(limit as usize))
                {
                    self.reserve(block, slot);
                }
            }
            for &(block, bound) in &band {
                let (width, height) = self.alloc_extent(block);
                if let Some(slot) = self
                    .pool
                    .allocate_within(width, height, Some(bound as usize))
                {
                    self.place(block, slot);
                }
            }
        } else {
            self.rollback_to(checkpoint);
        }
        self.scratch.repack_keep = keep;
        self.scratch.repack_band = band;
        self.scratch.repack_waiting = waiting;
        self.scratch.repack_residents = residents;
        fits
    }

    /// Turn this drain's moves (resident at the start, resident elsewhere
    /// now) into the ordered copy list.
    pub(super) fn schedule_repack_copies(&mut self) {
        let mut moves = std::mem::take(&mut self.scratch.moves);
        moves.clear();
        for touched in &self.touched {
            // A forgotten block resident again was re-placed from a ready
            // pair, so its upload fills the new slot. Copying its old texels
            // would be wasted work, and the copy could run upward.
            if touched.forgotten {
                continue;
            }
            let (Some(start), Some(now)) = (touched.start, self.slots[touched.block as usize])
            else {
                continue;
            };
            let (src, dst) = (placement_of(start), placement_of(now));
            if src != dst {
                debug_assert!(dst.layer <= src.layer, "a downward repack moved a block up");
                moves.push(PoolCopy {
                    src,
                    dst,
                    width: now.width,
                    height: now.height,
                });
            }
        }
        moves.sort_unstable_by_key(|m| (m.dst.layer, m.src.layer, m.dst.y, m.dst.x));
        let spare = self.spare_layer();
        let staged = |at: BlockPlacement| BlockPlacement { layer: spare, ..at };
        let copies = &mut self.plan.copies;
        for group in moves.chunk_by(|a, b| a.dst.layer == b.dst.layer) {
            let layer = group[0].dst.layer;
            let within = || group.iter().filter(move |m| m.src.layer == layer);
            copies.extend(within().map(|m| PoolCopy {
                dst: staged(m.dst),
                ..*m
            }));
            copies.extend(group.iter().filter(|m| m.src.layer != layer).copied());
            copies.extend(within().map(|m| PoolCopy {
                src: staged(m.dst),
                ..*m
            }));
        }
        self.scratch.moves = moves;
    }
}
