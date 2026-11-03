//! Reusable buffers for the host assist pass, cleared each pass rather than
//! reallocated, and the dense grid of evaluated pair values the pass indexes
//! by each pawn's place in its `players()` order.
//! See: context/lib/scripting.md §12 (Assist)

use glam::Vec3;
use postretro_entities::EntityId;

use super::assist::{AssistCandidate, AssistStep, HelperRecord, PawnLife};
use super::assist_pairs::PairValues;

/// One pawn as a pass sees it.
pub(super) struct PassPawn {
    pub(super) id: EntityId,
    pub(super) position: Vec3,
    pub(super) life: PawnLife,
    /// The pawn's `life_state_entry`, recorded with a completion on it.
    pub(super) entry: u32,
    pub(super) assistable: bool,
    pub(super) permits_use: bool,
    pub(super) held: bool,
    pub(super) real_command: bool,
}

/// One helper's step in a pass.
pub(super) struct PassStep {
    pub(super) helper: EntityId,
    pub(super) outcome: AssistStep,
    /// The step completed, but an earlier helper took the target's one
    /// completion this tick.
    pub(super) lost: bool,
}

/// The pass's working buffers. Every pass clears and refills them, so after
/// the first few ticks a pass allocates nothing.
#[derive(Default)]
pub(super) struct PassScratch {
    pub(super) group: Vec<EntityId>,
    pub(super) pawns: Vec<PassPawn>,
    pub(super) ids: Vec<EntityId>,
    pub(super) candidates: Vec<AssistCandidate>,
    pub(super) steps: Vec<PassStep>,
    /// The next cross-tick records, swapped with the state's once built.
    pub(super) helpers: Vec<(EntityId, HelperRecord)>,
    pub(super) lives: Vec<(EntityId, PawnLife)>,
}

/// Evaluated pair values for one pass, `width` pawns square, indexed by
/// (helper place, target place). A cell the pass did not evaluate is `None`.
#[derive(Default)]
pub(super) struct PairGrid {
    width: usize,
    cells: Vec<Option<PairValues>>,
}

impl PairGrid {
    /// Empty every cell for a pass over `width` pawns, keeping the allocation.
    pub(super) fn reset(&mut self, width: usize) {
        self.width = width;
        self.cells.clear();
        self.cells.resize(width * width, None);
    }

    pub(super) fn set(&mut self, helper: usize, target: usize, values: PairValues) {
        self.cells[helper * self.width + target] = Some(values);
    }

    pub(super) fn get(&self, helper: usize, target: usize) -> Option<PairValues> {
        if helper >= self.width || target >= self.width {
            return None;
        }
        self.cells[helper * self.width + target]
    }
}
