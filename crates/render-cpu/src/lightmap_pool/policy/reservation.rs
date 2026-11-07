// Reserved room: pool rects held for mandatory and visible targets whose pairs are not ready.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// A reservation is a real allocation in one pool's allocator, so no band pair
// can land on it, but it is not a placement: the block stays a miss with no
// table entry until its whole pair uploads. Room is held per pool: a block can
// hold room in one pool and none in the other, and a pair lands only when
// every half has a place. Work here runs when targets change, or inside a
// repack, never on a drain that only lands pairs.

use postretro_level_format::lightmap_pool::PoolKind;

use super::super::{Slot, packing_order_key};
use super::{JournalOp, LightmapPoolModel};

/// Which demand a repack must seat before it counts as done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepackScope {
    /// Every mandatory and visible target. Any block not ready takes reserved
    /// room, so a demand arriving over several drains repacks once.
    WholeDemand,
    /// Only the drain's pairs. Other targets keep room as far as it lasts: a
    /// demand past the pool still places what is ready before it grows.
    DrainPairs,
}

/// One pool's reserved rects, and whether its demand has spent its compaction.
#[derive(Debug)]
pub(super) struct DemandRoom {
    /// Reserved rect by item.
    held: Vec<Option<Slot>>,
    /// Reserved texels per allocator layer, for band headroom.
    held_texels: Vec<u64>,
    /// A compaction ran for the demand as it stands and left a target without
    /// room, or found no layout. A demand here is past the pool, or jammed:
    /// compacting again cannot help, and costs a frame each drain. It stays
    /// spent until the targets change into a set that packs from scratch, or
    /// the pool grows. Either way what follows is a new demand, which may
    /// compact and grow again.
    pub(super) compaction_spent: bool,
}

impl DemandRoom {
    pub(super) fn new(items: usize) -> Self {
        Self {
            held: vec![None; items],
            held_texels: Vec::new(),
            compaction_spent: false,
        }
    }

    pub(super) fn slot(&self, item: u32) -> Option<Slot> {
        self.held[item as usize]
    }

    pub(super) fn held_texels_in(&self, layer: usize) -> u64 {
        self.held_texels.get(layer).copied().unwrap_or(0)
    }

    pub(super) fn hold(&mut self, item: u32, slot: Slot) {
        debug_assert!(self.held[item as usize].is_none(), "item {item} held twice");
        self.held[item as usize] = Some(slot);
        let layer = slot.layer as usize;
        if layer >= self.held_texels.len() {
            self.held_texels.resize(layer + 1, 0);
        }
        self.held_texels[layer] += u64::from(slot.width) * u64::from(slot.height);
    }

    pub(super) fn take(&mut self, item: u32) -> Option<Slot> {
        let slot = self.held[item as usize].take()?;
        self.held_texels[slot.layer as usize] -= u64::from(slot.width) * u64::from(slot.height);
        Some(slot)
    }

    #[cfg(test)]
    pub(super) fn capacities(&self) -> [usize; 2] {
        [self.held.capacity(), self.held_texels.capacity()]
    }
}

impl LightmapPoolModel {
    /// Hold `slot`, already allocated in `kind`'s pool, for `item`.
    pub(super) fn reserve(&mut self, kind: PoolKind, item: u32, slot: Slot) {
        self.touch(kind.block_of(item));
        self.pools[kind].room.hold(item, slot);
        self.journal.push(JournalOp::Reserve { kind, item, slot });
    }

    /// Turn `item`'s reserved room into its placement, leaving the allocator
    /// alone. False when it holds none.
    pub(super) fn land(&mut self, kind: PoolKind, item: u32) -> bool {
        if self.pools[kind].room.slot(item).is_none() {
            return false;
        }
        self.touch(kind.block_of(item));
        let pool = &mut self.pools[kind];
        let slot = pool.room.take(item).expect("checked above");
        pool.attach(item, slot);
        self.journal.push(JournalOp::Land { kind, item, slot });
        true
    }

    /// Give back the room every half of `block` holds.
    pub(super) fn return_room(&mut self, block: u32) {
        for (kind, item) in self.halves(block) {
            self.return_item_room(kind, item);
        }
    }

    /// Give `item`'s reserved room back to its pool, if it holds any.
    fn return_item_room(&mut self, kind: PoolKind, item: u32) {
        let Some(slot) = self.pools[kind].room.slot(item) else {
            return;
        };
        self.touch(kind.block_of(item));
        let pool = &mut self.pools[kind];
        pool.room.take(item);
        pool.allocator
            .free(slot)
            .expect("a reservation names its live allocation");
        self.journal
            .push(JournalOp::ReturnRoom { kind, item, slot });
    }

    pub(super) fn return_all_room(&mut self, kind: PoolKind) {
        for index in 0..self.pools[kind].items().len() {
            let item = self.pools[kind].items()[index];
            self.return_item_room(kind, item);
        }
    }

    /// Reserve what room `kind`'s pool has free, moving nothing and evicting
    /// nothing, for each mandatory and visible target with none, tallest first
    /// so a demand arriving shortest first does not shelve itself out of the
    /// pool. A target it cannot seat waits for its pair: only a ready pair
    /// evicts, repacks or grows. Runs when targets change or the pool grows.
    pub(super) fn hold_room_for_demand(&mut self, kind: PoolKind) {
        let mut pending = std::mem::take(&mut self.scratch.unseated);
        pending.clear();
        let pool = &self.pools[kind];
        pending.extend(pool.items().iter().copied().filter(|&item| {
            self.is_never_refused(kind.block_of(item))
                && pool.slot(item).is_none()
                && pool.room.slot(item).is_none()
        }));
        self.sort_tallest_first(kind, &mut pending);
        for &item in &pending {
            let pool = &mut self.pools[kind];
            let (width, height) = pool.alloc_extent(item);
            if let Some(slot) = pool.allocate_in_layers(width, height) {
                self.reserve(kind, item, slot);
            }
        }
        self.scratch.unseated = pending;
    }

    /// Mandatory and visible targets' items in `kind`'s pool, neither placed
    /// nor among this drain's pairs: a repack seats them in reserved room.
    pub(super) fn collect_waiting(&self, kind: PoolKind, waiting: &mut Vec<u32>) {
        waiting.clear();
        let pool = &self.pools[kind];
        waiting.extend(pool.items().iter().copied().filter(|&item| {
            let block = kind.block_of(item);
            self.is_never_refused(block) && pool.slot(item).is_none() && !self.is_pair(block)
        }));
    }

    pub(super) fn sort_tallest_first(&self, kind: PoolKind, items: &mut [u32]) {
        let pool = &self.pools[kind];
        items.sort_unstable_by_key(|&item| packing_order_key(item, pool.alloc_extent(item)));
    }

    pub(super) fn spend_compaction(&mut self, kind: PoolKind) {
        self.set_compaction_spent(kind, true);
    }

    /// A spent demand ends once the targets, changed, pack from scratch: that
    /// set is owed its repack.
    pub(super) fn reopen_compaction(&mut self, kind: PoolKind) {
        if self.pools[kind].room.compaction_spent
            && self.targeted_demand_layers(kind) <= self.pools[kind].layers
        {
            self.set_compaction_spent(kind, false);
        }
    }

    pub(super) fn set_compaction_spent(&mut self, kind: PoolKind, spent: bool) {
        let previous = self.pools[kind].room.compaction_spent;
        if previous != spent {
            self.pools[kind].room.compaction_spent = spent;
            self.journal
                .push(JournalOp::CompactionGate { kind, previous });
        }
    }
}
