// Undo journal for one drain's model mutations: install rollback and drain abort.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use super::plan::{EvictionReason, NotAPlannedUpload};
use super::{LightmapPoolModel, Slot, Target, placement_of};

/// The exact inverse of each mutation, replayed newest-first.
#[derive(Debug, Clone, Copy)]
pub(super) enum JournalOp {
    Target {
        block: u32,
        previous: Option<Target>,
    },
    Cap {
        previous: u32,
    },
    Place {
        block: u32,
        slot: Slot,
    },
    Free {
        block: u32,
        slot: Slot,
    },
    Grow {
        previous_layers: u32,
    },
}

/// A block's state when the drain first touched it, so the plan's outputs
/// are diffs against the drain's start whatever happened in between.
#[derive(Debug, Clone, Copy)]
pub(super) struct Touched {
    pub block: u32,
    pub start: Option<Slot>,
    /// The reason of its latest eviction this drain, if any.
    pub reason: Option<EvictionReason>,
    /// Freed by a residency reset. Its `start` stays, so the table write
    /// that turns its entry non-resident still diffs; but the new controller
    /// never saw it resident, so it is never reported evicted, and any slot
    /// it holds now came from an upload, never a repack move.
    pub forgotten: bool,
}

impl LightmapPoolModel {
    /// Roll back one install after the GPU layer failed it. The block's slot
    /// is freed and its entry stays non-resident; the drain's repack moves
    /// and growth stand, so every other entry keeps addressing its own
    /// block in the active pool. A retiring generation stays retiring until
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
        self.plan.report = self.report(self.plan.report.repacked, self.plan.report.grew);
        Ok(())
    }

    /// Undo the whole drain, for a failure before anything was submitted
    /// (for example growth could not allocate). The model returns to its
    /// state before `plan_drain`, targets and cap included, and the plan is
    /// cleared.
    pub fn abort_drain(&mut self) {
        self.rollback_to(0);
        self.plan.clear();
        self.plan.report = self.report(false, false);
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
            start: self.slots[b],
            reason: None,
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
        if target.is_none() && self.is_resident(block) {
            self.release(block, EvictionReason::Untargeted);
        }
    }

    pub(super) fn set_cap(&mut self, cap_layers: u32) {
        if cap_layers != self.cap_layers {
            self.journal.push(JournalOp::Cap {
                previous: self.cap_layers,
            });
            self.cap_layers = cap_layers;
        }
    }

    /// Record `slot` (already allocated in the pool) as `block`'s placement.
    pub(super) fn place(&mut self, block: u32, slot: Slot) {
        self.touch(block);
        self.attach(block, slot);
        self.journal.push(JournalOp::Place { block, slot });
    }

    /// Free `block`'s slot.
    pub(super) fn release(&mut self, block: u32, reason: EvictionReason) {
        self.touch(block);
        self.touched[self.touch_index[block as usize] as usize].reason = Some(reason);
        let slot = self.detach(block);
        self.pool
            .free(slot)
            .expect("a resident slot names its live allocation");
        self.journal.push(JournalOp::Free { block, slot });
    }

    /// Free every resident block for a new generation's first drain. The
    /// frees are journaled, so an aborted drain restores them, and their
    /// table writes land with the drain. Each is marked forgotten rather
    /// than having its `start` cleared: a cleared start would drop the table
    /// write for a block that stays non-resident.
    pub(super) fn forget_residency(&mut self) {
        while let Some(&block) = self.resident.last() {
            self.release(block, EvictionReason::Untargeted);
            self.touched[self.touch_index[block as usize] as usize].forgotten = true;
        }
    }

    pub(super) fn record_growth(&mut self, previous_layers: u32) {
        self.journal.push(JournalOp::Grow { previous_layers });
        self.retiring = true;
        self.texture_allocations += 1;
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
            JournalOp::Cap { previous } => self.cap_layers = previous,
            JournalOp::Place { block, slot } => {
                let placed = self.detach(block);
                debug_assert_eq!(placed, slot, "undo frees the slot it placed");
                self.pool
                    .free(slot)
                    .expect("undo replays newest-first, so the placed slot is live");
            }
            JournalOp::Free { block, slot } => {
                self.pool
                    .restore(slot)
                    .expect("undo replays newest-first, so the freed rect is free");
                self.attach(block, slot);
            }
            JournalOp::Grow { previous_layers } => {
                self.layers = previous_layers;
                self.retiring = false;
                self.texture_allocations -= 1;
                self.plan.growth = None;
            }
        }
    }

    fn attach(&mut self, block: u32, slot: Slot) {
        let b = block as usize;
        debug_assert!(self.slots[b].is_none(), "block {block} placed twice");
        self.slots[b] = Some(slot);
        self.resident_pos[b] = self.resident.len() as u32;
        self.resident.push(block);
        let layer = slot.layer as usize;
        if layer >= self.used_texels.len() {
            self.used_texels.resize(layer + 1, 0);
        }
        self.used_texels[layer] += u64::from(slot.width) * u64::from(slot.height);
    }

    fn detach(&mut self, block: u32) -> Slot {
        let b = block as usize;
        let slot = self.slots[b].take().expect("detach a resident block");
        let at = self.resident_pos[b] as usize;
        self.resident.swap_remove(at);
        if let Some(&moved) = self.resident.get(at) {
            self.resident_pos[moved as usize] = at as u32;
        }
        self.resident_pos[b] = u32::MAX;
        self.used_texels[slot.layer as usize] -= u64::from(slot.width) * u64::from(slot.height);
        slot
    }

    /// Changed entries only, diffed against each touched block's state at
    /// the drain's start, sorted by entry index.
    pub(super) fn rebuild_table_writes(&mut self) {
        self.plan.table_writes.clear();
        for touched in &self.touched {
            let now = self.slots[touched.block as usize].map(placement_of);
            if touched.start.map(placement_of) != now {
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
