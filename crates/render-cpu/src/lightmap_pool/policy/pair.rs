// Atomic pairs: a block's halves take their places across both pools together, or not at all.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// Every path here works through the journal: a pair that cannot take every
// half rolls back to where it started, so no rect stays taken and any room it
// held is held again.

use postretro_level_format::lightmap_pool::{PerPool, PoolKind};
use postretro_level_loader::LightmapRefusal;

use super::plan::{EvictionReason, PoolGrowth, RepackKind};
use super::{JournalOp, LightmapPoolModel};

impl LightmapPoolModel {
    /// Land each half with reserved room and allocate the rest in their pools'
    /// current layers, as far as they go. `only` leaves the other pool's halves
    /// out. Returns the pools a half found no room in.
    fn seat_halves(&mut self, block: u32, only: Option<PoolKind>) -> PerPool<bool> {
        let mut short = PerPool::default();
        for (kind, item) in self.halves(block) {
            if only.is_some_and(|only| only != kind)
                || self.pools[kind].slot(item).is_some()
                || self.land(kind, item)
            {
                continue;
            }
            let pool = &mut self.pools[kind];
            let (width, height) = pool.alloc_extent(item);
            match pool.allocate_in_layers(width, height) {
                Some(slot) => self.place(kind, item, slot),
                None => short[kind] = true,
            }
        }
        short
    }

    /// Seat every half, evicting band blocks farthest lead first until they
    /// all fit. Evictions stand only when the pair ends up placed, and then
    /// only for victims that cannot go back whole: a failed attempt restores
    /// every victim and every half, so a pair that then grows or defers costs
    /// the band nothing. On failure `scratch.short` names the pools still
    /// short with every band block gone.
    pub(super) fn place_without_growth(&mut self, block: u32, only: Option<PoolKind>) -> bool {
        let checkpoint = self.journal.len();
        loop {
            let short = self.seat_halves(block, only);
            if !short.any() {
                self.reinstate_spare_victims(checkpoint);
                return true;
            }
            let Some(victim) = self.next_victim() else {
                self.scratch.short = short;
                self.rollback_to(checkpoint);
                return false;
            };
            self.release(victim, EvictionReason::Pressure);
        }
    }

    /// After a pair seats, put back each victim evicted before the last one
    /// all of whose rects are still free: farthest-lead-first order can evict
    /// blocks the pair never touched. A victim with any rect taken stays out
    /// whole, of both pools. The last victim is the one that made room. A
    /// reinstated victim returns to the top of the victim stack in lead order,
    /// so the next pair still evicts farthest first.
    fn reinstate_spare_victims(&mut self, checkpoint: usize) {
        let mut at = self.journal.len();
        let mut last_victim = true;
        while at > checkpoint {
            at -= 1;
            let JournalOp::Free { kind, item, .. } = self.journal[at] else {
                continue;
            };
            // A victim's frees sit together, one per half.
            let victim = kind.block_of(item);
            let mut first = at;
            while first > checkpoint
                && matches!(self.journal[first - 1],
                    JournalOp::Free { kind, item, .. } if kind.block_of(item) == victim)
            {
                first -= 1;
            }
            if !std::mem::take(&mut last_victim) {
                self.reinstate_whole(victim, first..at + 1);
            }
            at = first;
        }
    }

    /// Put `victim` back, when every rect its frees at `run` gave up is free.
    fn reinstate_whole(&mut self, victim: u32, run: std::ops::Range<usize>) {
        let rects_free = self.journal[run.clone()].iter().all(|op| match *op {
            JournalOp::Free { kind, slot, .. } => self.pools[kind].allocator.can_restore(slot),
            _ => false,
        });
        if !rects_free {
            return;
        }
        for at in run {
            let JournalOp::Free { kind, item, slot } = self.journal[at] else {
                continue;
            };
            // A rect free on its own can still lose to the victim's other
            // plane on a shared shelf; the victim then stays out whole.
            if self.pools[kind].allocator.restore(slot).is_err() {
                self.release(victim, EvictionReason::Pressure);
                return;
            }
            self.place(kind, item, slot);
        }
        self.scratch.victims.push(victim);
    }

    fn next_victim(&mut self) -> Option<u32> {
        if !self.scratch.victims_built {
            let mut victims = std::mem::take(&mut self.scratch.victims);
            victims.clear();
            victims.extend(
                self.pools
                    .lightmap
                    .resident
                    .iter()
                    .copied()
                    .filter(|&block| self.is_band(block)),
            );
            victims
                .sort_unstable_by_key(|&block| (self.target(block).map_or(0, |t| t.lead), block));
            self.scratch.victims = victims;
            self.scratch.victims_built = true;
        }
        while let Some(block) = self.scratch.victims.pop() {
            if self.is_resident(block) && self.is_band(block) {
                return Some(block);
            }
        }
        None
    }

    /// No repack could seat every mandatory and visible block: grow what is
    /// short. One generation per pool per drain, none while that pool's last
    /// one is still retiring, and never past the device's layer limit; a pair
    /// that needs growth then is deferred whole, and neither pool grows for it.
    /// A pool's first half to grow reserves the targeted demand's layers with
    /// the shared margin
    /// ([`growth_reserve_layers`](Self::growth_reserve_layers)), so later
    /// pairs, this drain or while the generation retires, land in layers
    /// already allocated.
    ///
    /// Once a pair is deferred at the device limit in a pool, it failed with
    /// every band victim evicted, so a later pair this drain at least as wide
    /// and as tall as that one, with a half in that pool, cannot fit either: it
    /// skips the victim walk, taking free space it fits as is or being
    /// deferred. Any other pair still walks, so an oversized pair that stays
    /// wanted never starves pairs that fit by evicting band blocks. Each
    /// deferred extent is compared on its own: a pair larger than a
    /// componentwise minimum of two deferred extents may still fit. The skip
    /// lasts one drain. Without it, a level past the device limit repeats a
    /// full walk per such pair on every drain, since its deferred pairs come
    /// back ready.
    pub(super) fn place_with_growth(&mut self, list: &[u32]) {
        let retiring = PerPool {
            lightmap: self.pools.lightmap.retiring,
            mask: self.pools.mask.retiring,
        };
        let start_layers = PerPool {
            lightmap: self.pools.lightmap.layers,
            mask: self.pools.mask.layers,
        };
        // Extents deferred at the device limit this drain; none contains
        // another.
        let mut limited = std::mem::take(&mut self.scratch.device_limited);
        limited.lightmap.clear();
        limited.mask.clear();
        for &block in list {
            if self.past_device_limit(block, &limited) {
                let checkpoint = self.journal.len();
                if self.seat_halves(block, None).any() {
                    self.rollback_to(checkpoint);
                    self.plan.deferred.push(block);
                }
                continue;
            }
            if self.place_without_growth(block, None) {
                continue;
            }
            let short = self.scratch.short;
            if PoolKind::ALL
                .iter()
                .any(|&kind| short[kind] && retiring[kind])
            {
                self.plan.deferred.push(block);
                continue;
            }
            self.place_past_layers(block, short, start_layers, &mut limited);
        }
        self.scratch.device_limited = limited;
        for kind in PoolKind::ALL {
            let (from_layers, to_layers) = (start_layers[kind], self.pools[kind].layers);
            // A compaction seats every pending half of its pool, so that pool
            // is never short here. Its copies took the spare's index from the
            // layers at plan time, where a downward repack's are scheduled
            // after growth, so a compacted pool that grew would stage through
            // a layer the new generation uses.
            debug_assert!(
                to_layers == from_layers
                    || !matches!(
                        self.plan.pools[kind].repack,
                        Some(RepackKind::Compaction(_))
                    ),
                "the {} pool grew in the drain it compacted",
                kind.name(),
            );
            if to_layers > from_layers {
                self.plan.pools[kind].growth = Some(PoolGrowth {
                    from_layers,
                    to_layers,
                });
            }
        }
    }

    fn past_device_limit(&self, block: u32, limited: &PerPool<Vec<(u32, u32)>>) -> bool {
        let (width, height) = self.slot_extent(block);
        self.halves(block).any(|(kind, _)| {
            limited[kind]
                .iter()
                .any(|&(w, h)| w <= width && h <= height)
        })
    }

    /// Seat `block` with each `short` pool past its current layers: the rest
    /// of the pair still evicts for its room, so only a pool with no other way
    /// grows. Nothing grows until every half has a place.
    fn place_past_layers(
        &mut self,
        block: u32,
        short: PerPool<bool>,
        start_layers: PerPool<u32>,
        limited: &mut PerPool<Vec<(u32, u32)>>,
    ) {
        let checkpoint = self.journal.len();
        let mut needed = PerPool::<u32>::default();
        for (kind, item) in self.halves(block) {
            if !short[kind] || self.land(kind, item) {
                continue;
            }
            let pool = &mut self.pools[kind];
            let (width, height) = pool.alloc_extent(item);
            let limit = Some(pool.max_layers as usize);
            let Some(slot) = pool.allocator.allocate_within(width, height, limit) else {
                self.rollback_to(checkpoint);
                self.plan.deferred.push(block);
                self.plan.pools[kind].device_limited = true;
                // This extent contains no entry, or it would have skipped.
                limited[kind].retain(|&(w, h)| width > w || height > h);
                limited[kind].push((width, height));
                return;
            };
            self.place(kind, item, slot);
            needed[kind] = needed[kind].max(slot.layer + 1);
        }
        if !self.place_without_growth(block, None) {
            self.rollback_to(checkpoint);
            self.plan.deferred.push(block);
            return;
        }
        for kind in PoolKind::ALL {
            if needed[kind] > self.pools[kind].layers {
                self.grow_to(kind, needed[kind], start_layers[kind]);
            }
        }
    }

    /// The drain's first growth of `kind` reserves for the demand; a later one
    /// lands exactly at need.
    fn grow_to(&mut self, kind: PoolKind, needed: u32, start_layers: u32) {
        self.pools[kind].layers = if self.pools[kind].layers == start_layers {
            self.record_growth(kind, start_layers);
            self.growth_reserve_layers(kind, needed)
        } else {
            needed
        };
    }

    /// Band pairs take what room is free in any layer, clear of reserved room,
    /// or are refused whole, naming the pools short of room.
    pub(super) fn place_band(&mut self) {
        let list = std::mem::take(&mut self.scratch.band_ready);
        for &block in &list {
            let checkpoint = self.journal.len();
            let short = self.seat_band(block);
            if short.any() {
                self.rollback_to(checkpoint);
                self.plan.refused.push(LightmapRefusal { block, short });
            }
        }
        self.scratch.band_ready = list;
    }

    fn seat_band(&mut self, block: u32) -> PerPool<bool> {
        let mut short = PerPool::default();
        for (kind, item) in self.halves(block) {
            let pool = &mut self.pools[kind];
            let (width, height) = pool.alloc_extent(item);
            match pool.allocate_in_layers(width, height) {
                Some(slot) => self.place(kind, item, slot),
                None => short[kind] = true,
            }
        }
        short
    }
}
