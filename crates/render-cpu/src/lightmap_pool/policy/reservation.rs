// Reserved room: pool rects held for mandatory and visible targets whose pairs are not ready.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)
//
// A reservation is a real allocation in the pool's allocator, so no band pair
// can land on it, but it is not a placement: the block stays a miss with no
// table entry until its pair uploads. Work here runs when targets change, or
// inside a repack, never on a drain that only lands pairs.

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
    /// Reserved rect by block id.
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
    pub(super) fn new(blocks: usize) -> Self {
        Self {
            held: vec![None; blocks],
            held_texels: Vec::new(),
            compaction_spent: false,
        }
    }

    pub(super) fn slot(&self, block: u32) -> Option<Slot> {
        self.held[block as usize]
    }

    pub(super) fn held_texels_in(&self, layer: usize) -> u64 {
        self.held_texels.get(layer).copied().unwrap_or(0)
    }

    pub(super) fn hold(&mut self, block: u32, slot: Slot) {
        debug_assert!(
            self.held[block as usize].is_none(),
            "block {block} held twice"
        );
        self.held[block as usize] = Some(slot);
        let layer = slot.layer as usize;
        if layer >= self.held_texels.len() {
            self.held_texels.resize(layer + 1, 0);
        }
        self.held_texels[layer] += u64::from(slot.width) * u64::from(slot.height);
    }

    pub(super) fn take(&mut self, block: u32) -> Option<Slot> {
        let slot = self.held[block as usize].take()?;
        self.held_texels[slot.layer as usize] -= u64::from(slot.width) * u64::from(slot.height);
        Some(slot)
    }

    #[cfg(test)]
    pub(super) fn capacities(&self) -> [usize; 2] {
        [self.held.capacity(), self.held_texels.capacity()]
    }
}

impl LightmapPoolModel {
    /// Hold `slot`, already allocated in the pool, for `block`.
    pub(super) fn reserve(&mut self, block: u32, slot: Slot) {
        self.touch(block);
        self.demand_room.hold(block, slot);
        self.journal.push(JournalOp::Reserve { block, slot });
    }

    /// Turn `block`'s reserved room into its placement, leaving the allocator
    /// alone. False when it holds none.
    pub(super) fn land(&mut self, block: u32) -> bool {
        if self.demand_room.slot(block).is_none() {
            return false;
        }
        self.touch(block);
        let slot = self.demand_room.take(block).expect("checked above");
        self.attach(block, slot);
        self.journal.push(JournalOp::Land { block, slot });
        true
    }

    /// Give `block`'s reserved room back to the pool, if it holds any.
    pub(super) fn return_room(&mut self, block: u32) {
        let Some(slot) = self.demand_room.slot(block) else {
            return;
        };
        self.touch(block);
        self.demand_room.take(block);
        self.pool
            .free(slot)
            .expect("a reservation names its live allocation");
        self.journal.push(JournalOp::ReturnRoom { block, slot });
    }

    pub(super) fn return_all_room(&mut self) {
        for block in 0..self.block_count() {
            self.return_room(block);
        }
    }

    /// Reserve what room the pool has free, moving nothing and evicting
    /// nothing, for each mandatory and visible target with none, tallest first
    /// so a demand arriving shortest first does not shelve itself out of the
    /// pool. A target it cannot seat waits for its pair: only a ready pair
    /// evicts, repacks or grows. Runs when targets change or the pool grows.
    pub(super) fn hold_room_for_demand(&mut self) {
        #[cfg(test)]
        {
            self.demand_passes += 1;
        }
        let mut pending = std::mem::take(&mut self.scratch.unseated);
        pending.clear();
        pending.extend((0..self.block_count()).filter(|&block| {
            self.is_never_refused(block)
                && !self.is_resident(block)
                && self.demand_room.slot(block).is_none()
        }));
        self.sort_tallest_first(&mut pending);
        for &block in &pending {
            let (width, height) = self.alloc_extent(block);
            if let Some(slot) = self.allocate_in_layers(width, height) {
                self.reserve(block, slot);
            }
        }
        self.scratch.unseated = pending;
    }

    /// Mandatory and visible targets neither resident nor among this drain's
    /// pairs: a repack seats them in reserved room.
    pub(super) fn collect_waiting(&self, waiting: &mut Vec<u32>) {
        waiting.clear();
        waiting.extend((0..self.block_count()).filter(|&block| {
            self.is_never_refused(block) && !self.is_resident(block) && !self.is_pair(block)
        }));
    }

    pub(super) fn sort_tallest_first(&self, blocks: &mut [u32]) {
        blocks.sort_unstable_by_key(|&block| packing_order_key(block, self.alloc_extent(block)));
    }

    pub(super) fn spend_compaction(&mut self) {
        self.set_compaction_spent(true);
    }

    /// A spent demand ends once the targets, changed, pack from scratch: that
    /// set is owed its repack.
    pub(super) fn reopen_compaction(&mut self) {
        if self.demand_room.compaction_spent && self.targeted_demand_layers() <= self.layers {
            self.set_compaction_spent(false);
        }
    }

    pub(super) fn set_compaction_spent(&mut self, spent: bool) {
        let previous = self.demand_room.compaction_spent;
        if previous != spent {
            self.demand_room.compaction_spent = spent;
            self.journal.push(JournalOp::CompactionGate { previous });
        }
    }
}
