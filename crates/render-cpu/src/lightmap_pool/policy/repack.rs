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

use postretro_level_format::lightmap_pool::PoolKind;

use super::super::{BlockPlacement, packing_order_key};
use super::journal::halves_of;
use super::plan::{EvictionReason, PoolCopy};
use super::reservation::RepackScope;
use super::{LightmapPoolModel, placement_of};

/// An item a repack must seat.
#[derive(Debug, Clone, Copy)]
pub(super) struct RepackEntry {
    item: u32,
    /// Highest layer + 1 it may land in.
    bound: u32,
    /// It holds no texels here, so it takes reserved room, not a placement.
    room: bool,
}

impl LightmapPoolModel {
    /// The downward pass: repack every resident item of `kind`'s pool plus the
    /// ready never-refused `pending` items without reads, knowing nothing of
    /// the other pool. Mandatory and visible items go first, each resident one
    /// no higher than the layer it sits in (most constrained first, then
    /// tallest); then resident band items, nearest lead first. A band item
    /// that no longer fits drops its block from both pools. Returns false,
    /// with the model untouched, when `scope`'s demand does not fit.
    ///
    /// `pending` items and targets not ready take reserved room: a pair lands
    /// whole, once every pool has a place for it. A whole-demand repack must
    /// seat targets with the rest; a drain-pairs repack seats them after, as
    /// far as room lasts.
    pub(super) fn repack_downward(
        &mut self,
        kind: PoolKind,
        pending: &[u32],
        scope: RepackScope,
    ) -> bool {
        let limit = self.pools[kind].layers;
        let checkpoint = self.journal.len();
        let mut keep = std::mem::take(&mut self.scratch.repack_keep);
        let mut band = std::mem::take(&mut self.scratch.repack_band);
        let mut waiting = std::mem::take(&mut self.scratch.repack_waiting);
        let mut residents = std::mem::take(&mut self.scratch.repack_residents);
        keep.clear();
        band.clear();
        self.collect_waiting(kind, &mut waiting);
        residents.clear();
        residents.extend_from_slice(&self.pools[kind].resident);
        residents.sort_unstable();
        for &item in &residents {
            let layer = self.pools[kind].slot(item).expect("resident").layer;
            if self.is_band(kind.block_of(item)) {
                band.push((item, layer + 1));
            } else {
                keep.push(RepackEntry {
                    item,
                    bound: layer + 1,
                    room: false,
                });
            }
        }
        let in_room = |&item: &u32| RepackEntry {
            item,
            bound: limit,
            room: true,
        };
        keep.extend(pending.iter().map(in_room));
        if scope == RepackScope::WholeDemand {
            keep.extend(waiting.iter().map(in_room));
            waiting.clear();
        }
        keep.sort_unstable_by_key(|entry| {
            let order = packing_order_key(entry.item, self.pools[kind].alloc_extent(entry.item));
            (entry.bound, order)
        });
        self.sort_tallest_first(kind, &mut waiting);
        band.sort_unstable_by_key(|&(item, _)| {
            let lead = self.target(kind.block_of(item)).map_or(0, |t| t.lead);
            (lead, item)
        });

        self.return_all_room(kind);
        for &item in &residents {
            self.release_item(kind, item, EvictionReason::Pressure);
        }
        let fits = keep.iter().all(|entry| {
            let pool = &mut self.pools[kind];
            let (width, height) = pool.alloc_extent(entry.item);
            let bound = Some(entry.bound as usize);
            match pool.allocator.allocate_within(width, height, bound) {
                Some(slot) if entry.room => {
                    self.reserve(kind, entry.item, slot);
                    true
                }
                Some(slot) => {
                    self.place(kind, entry.item, slot);
                    true
                }
                None => false,
            }
        });
        if fits {
            for &item in &waiting {
                let pool = &mut self.pools[kind];
                let (width, height) = pool.alloc_extent(item);
                let bound = Some(limit as usize);
                if let Some(slot) = pool.allocator.allocate_within(width, height, bound) {
                    self.reserve(kind, item, slot);
                }
            }
            for &(item, bound) in &band {
                let pool = &mut self.pools[kind];
                let (width, height) = pool.alloc_extent(item);
                let bound = Some(bound as usize);
                if let Some(slot) = pool.allocator.allocate_within(width, height, bound) {
                    self.place(kind, item, slot);
                }
            }
            for &(item, _) in &band {
                self.drop_unseated_band(kind, item);
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

    /// A band item a repack left unseated takes its whole block out, of both
    /// pools: a pair never stays half resident.
    pub(super) fn drop_unseated_band(&mut self, kind: PoolKind, item: u32) {
        if self.pools[kind].slot(item).is_none() {
            self.release(kind.block_of(item), EvictionReason::Pressure);
        }
    }

    /// Turn this drain's moves in `kind`'s pool (resident at the start,
    /// resident elsewhere now) into the ordered copy list.
    pub(super) fn schedule_repack_copies(&mut self, kind: PoolKind) {
        let mut moves = std::mem::take(&mut self.scratch.moves);
        moves.clear();
        for touched in &self.touched {
            // A forgotten block resident again was re-placed from a ready
            // pair, so its upload fills the new slot. Copying its old texels
            // would be wasted work, and the copy could run upward.
            if touched.forgotten {
                continue;
            }
            for half in halves_of(kind) {
                let (_, item) = super::journal::half_at(touched.block, half);
                let (Some(start), Some(now)) = (touched.start[half], self.pools[kind].slot(item))
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
        }
        moves.sort_unstable_by_key(|m| (m.dst.layer, m.src.layer, m.dst.y, m.dst.x));
        let spare = self.pools[kind].spare_layer();
        let staged = |at: BlockPlacement| BlockPlacement { layer: spare, ..at };
        let copies = &mut self.plan.pools[kind].copies;
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
