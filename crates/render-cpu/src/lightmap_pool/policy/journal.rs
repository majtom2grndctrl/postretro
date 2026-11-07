// Undo journal for one drain's model mutations: install rollback and drain abort.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use postretro_level_format::lightmap_pool::{PerPool, PoolKind, mask_item};

use super::plan::{EvictionReason, NotAPlannedUpload};
use super::{LightmapPoolModel, Slot, Target, placement_of};

/// A block's places, one per pool half: its lightmap item, then each plane.
/// Absent planes stay empty.
pub(super) const HALVES: usize = 3;

/// The pool and item of half `half` of `block`.
pub(super) fn half_at(block: u32, half: usize) -> (PoolKind, u32) {
    match half {
        0 => (PoolKind::Lightmap, block),
        plane => (PoolKind::Mask, mask_item(block, plane - 1)),
    }
}

/// The halves of a block that sit in `kind`.
pub(super) fn halves_of(kind: PoolKind) -> std::ops::Range<usize> {
    match kind {
        PoolKind::Lightmap => 0..1,
        PoolKind::Mask => 1..HALVES,
    }
}

/// The exact inverse of each mutation, replayed newest-first.
#[derive(Debug, Clone, Copy)]
pub(super) enum JournalOp {
    Target {
        block: u32,
        previous: Option<Target>,
    },
    Place {
        kind: PoolKind,
        item: u32,
        slot: Slot,
    },
    Free {
        kind: PoolKind,
        item: u32,
        slot: Slot,
    },
    Grow {
        kind: PoolKind,
        previous_layers: u32,
    },
    Reserve {
        kind: PoolKind,
        item: u32,
        slot: Slot,
    },
    ReturnRoom {
        kind: PoolKind,
        item: u32,
        slot: Slot,
    },
    Land {
        kind: PoolKind,
        item: u32,
        slot: Slot,
    },
    CompactionGate {
        kind: PoolKind,
        previous: bool,
    },
}

/// A block's state when the drain first touched it, so the plan's outputs
/// are diffs against the drain's start whatever happened in between.
#[derive(Debug, Clone, Copy)]
pub(super) struct Touched {
    pub block: u32,
    /// Where each half sat.
    pub start: [Option<Slot>; HALVES],
    /// The reason of its latest eviction this drain, if any.
    pub reason: Option<EvictionReason>,
    /// Whether each half held reserved room, as the controller last heard.
    pub held_at_start: [bool; HALVES],
    /// Freed by a residency reset. Its `start` stays, so the table write
    /// that turns its entry non-resident still diffs; but the new controller
    /// never saw it resident, so it is never reported evicted, and any slot
    /// it holds now came from an upload, never a repack move.
    pub forgotten: bool,
}

impl LightmapPoolModel {
    /// Roll back one install after the GPU layer failed it. Both halves are
    /// freed and its entry stays non-resident; the drain's repack moves and
    /// growth stand, so every other entry keeps addressing its own block in
    /// the active pools. A retiring generation stays retiring until
    /// `release_retirement`.
    pub fn fail_install(&mut self, block: u32) -> Result<(), NotAPlannedUpload> {
        let at = self
            .plan
            .uploads
            .iter()
            .position(|upload| upload.block == block)
            .ok_or(NotAPlannedUpload)?;
        self.plan.uploads.remove(at);
        self.plan.installed.retain(|&installed| installed != block);
        self.release(block, EvictionReason::Pressure);
        self.plan.failed.push(block);
        self.rebuild_table_writes();
        self.plan.report = self.report(self.plan.repacked(), self.plan.grew());
        Ok(())
    }

    /// Undo the whole drain, for a failure before anything was submitted
    /// (for example growth could not allocate). Both pools return to their
    /// state before `plan_drain`, targets, reserved room and the compaction
    /// gates included, and the plan is cleared.
    pub fn abort_drain(&mut self) {
        self.rollback_to(0);
        // Nothing this drain deferred stands.
        self.drain += 1;
        self.plan.clear();
        self.plan.report = self.report(PerPool::default(), PerPool::default());
    }

    fn half_slots(&self, block: u32) -> [Option<Slot>; HALVES] {
        std::array::from_fn(|half| {
            let (kind, item) = half_at(block, half);
            self.pools[kind].slot(item)
        })
    }

    /// Whether each half of `block` holds reserved room.
    pub(super) fn held_halves(&self, block: u32) -> [bool; HALVES] {
        std::array::from_fn(|half| {
            let (kind, item) = half_at(block, half);
            self.pools[kind].room.slot(item).is_some()
        })
    }

    pub(super) fn touch(&mut self, block: u32) {
        let b = block as usize;
        if self.touch_stamp[b] == self.drain {
            return;
        }
        self.touch_stamp[b] = self.drain;
        self.touch_index[b] = self.touched.len() as u32;
        self.touched.push(Touched {
            block,
            start: self.half_slots(block),
            reason: None,
            held_at_start: self.held_halves(block),
            forgotten: false,
        });
    }

    pub(super) fn set_target(&mut self, block: u32, target: Option<Target>) {
        let previous = self.targets[block as usize];
        if previous == target {
            return;
        }
        self.journal.push(JournalOp::Target { block, previous });
        self.targets[block as usize] = target;
        let never_refused = |t: Option<Target>| t.is_some_and(|t| !t.is_band());
        if never_refused(previous) != never_refused(target) {
            self.scratch.demand_changed = true;
        }
        if !never_refused(target) {
            self.return_room(block);
        }
        if target.is_none() && self.is_resident(block) {
            // Its room may seat a target that found none.
            self.scratch.demand_changed = true;
            self.release(block, EvictionReason::Untargeted);
        }
    }

    /// Record `slot` (already allocated in `kind`'s pool) as `item`'s placement.
    pub(super) fn place(&mut self, kind: PoolKind, item: u32, slot: Slot) {
        self.touch(kind.block_of(item));
        self.pools[kind].attach(item, slot);
        self.journal.push(JournalOp::Place { kind, item, slot });
    }

    /// Free whichever halves of `block` are placed: a block leaves both pools.
    pub(super) fn release(&mut self, block: u32, reason: EvictionReason) {
        for (kind, item) in self.halves(block) {
            if self.pools[kind].slot(item).is_some() {
                self.release_item(kind, item, reason);
            }
        }
    }

    /// Free one half. Only a repack does this on its own, and it either seats
    /// the half again or drops the whole block.
    pub(super) fn release_item(&mut self, kind: PoolKind, item: u32, reason: EvictionReason) {
        let block = kind.block_of(item);
        self.touch(block);
        self.touched[self.touch_index[block as usize] as usize].reason = Some(reason);
        let slot = self.pools[kind].detach(item);
        self.pools[kind]
            .allocator
            .free(slot)
            .expect("a resident slot names its live allocation");
        self.journal.push(JournalOp::Free { kind, item, slot });
    }

    /// Free every resident block for a new generation's first drain. The
    /// frees are journaled, so an aborted drain restores them, and their
    /// table writes land with the drain. Each is marked forgotten rather
    /// than having its `start` cleared: a cleared start would drop the table
    /// write for a block that stays non-resident. Reserved room goes too, and
    /// counts as never held: the new controller has heard of none, so what
    /// this drain seats again it reports as new.
    pub(super) fn forget_residency(&mut self) {
        while let Some(&block) = self.pools.lightmap.resident.last() {
            self.release(block, EvictionReason::Untargeted);
            self.touched[self.touch_index[block as usize] as usize].forgotten = true;
        }
        for kind in PoolKind::ALL {
            self.return_all_room(kind);
        }
        for touched in &mut self.touched {
            touched.held_at_start = [false; HALVES];
        }
        self.scratch.demand_changed = true;
    }

    pub(super) fn record_growth(&mut self, kind: PoolKind, previous_layers: u32) {
        self.journal.push(JournalOp::Grow {
            kind,
            previous_layers,
        });
        self.pools[kind].retiring = true;
        self.pools[kind].texture_allocations += 1;
        self.set_compaction_spent(kind, false);
    }

    pub(super) fn rollback_to(&mut self, len: usize) {
        while self.journal.len() > len {
            let op = self.journal.pop().expect("journal is longer than len");
            self.undo(op);
        }
        self.scratch.victims_built = false;
    }

    fn undo(&mut self, op: JournalOp) {
        match op {
            JournalOp::Target { block, previous } => self.targets[block as usize] = previous,
            JournalOp::Place { kind, item, slot } => {
                let pool = &mut self.pools[kind];
                let placed = pool.detach(item);
                debug_assert_eq!(placed, slot, "undo frees the slot it placed");
                pool.allocator
                    .free(slot)
                    .expect("undo replays newest-first, so the placed slot is live");
            }
            JournalOp::Free { kind, item, slot } => {
                let pool = &mut self.pools[kind];
                pool.allocator
                    .restore(slot)
                    .expect("undo replays newest-first, so the freed rect is free");
                pool.attach(item, slot);
            }
            JournalOp::Grow {
                kind,
                previous_layers,
            } => {
                let pool = &mut self.pools[kind];
                pool.layers = previous_layers;
                pool.retiring = false;
                pool.texture_allocations -= 1;
                self.plan.pools[kind].growth = None;
            }
            JournalOp::Reserve { kind, item, slot } => {
                let pool = &mut self.pools[kind];
                pool.room.take(item);
                pool.allocator
                    .free(slot)
                    .expect("undo replays newest-first, so the reserved slot is live");
            }
            JournalOp::ReturnRoom { kind, item, slot } => {
                let pool = &mut self.pools[kind];
                pool.allocator
                    .restore(slot)
                    .expect("undo replays newest-first, so the returned rect is free");
                pool.room.hold(item, slot);
            }
            JournalOp::Land { kind, item, slot } => {
                let pool = &mut self.pools[kind];
                let placed = pool.detach(item);
                debug_assert_eq!(placed, slot, "undo unlands the slot it landed");
                pool.room.hold(item, slot);
            }
            JournalOp::CompactionGate { kind, previous } => {
                self.pools[kind].room.compaction_spent = previous;
            }
        }
    }

    /// Changed entries only, diffed against each touched block's state at
    /// the drain's start, sorted by entry index. A block moved in both pools
    /// is one entry, so one write carries both addresses.
    pub(super) fn rebuild_table_writes(&mut self) {
        self.plan.table_writes.clear();
        for touched in &self.touched {
            let places = |slots: [Option<Slot>; HALVES]| slots.map(|s| s.map(placement_of));
            if places(touched.start) != places(self.half_slots(touched.block)) {
                self.plan.table_writes.push(super::TableWrite {
                    index: touched.block + 1,
                    entry: self.table_entry(touched.block),
                });
            }
        }
        self.plan
            .table_writes
            .sort_unstable_by_key(|write| write.index);
    }
}
