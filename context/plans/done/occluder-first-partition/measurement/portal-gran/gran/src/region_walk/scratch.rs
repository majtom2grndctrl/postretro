// Caller-owned walk state reused across walks: per-cell rects, stamps, queue.
// See: context/lib/rendering_pipeline.md §2

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use glam::Vec3;

use super::rect::ScreenRect;
use crate::visibility::FrustumPlane;

/// Reused by one caller across walks so a walk allocates nothing once the
/// buffers have grown to the world. Cell state is valid only where its stamp
/// equals the current generation, so no per-walk clear runs; a generation
/// wrap clears the stamps once. One scratch per thread: the state is not
/// shared.
///
/// It also lends `determine_visible_cells` its `VisibleCells::Culled` buffer;
/// [`Self::recycle_cells`] takes it back.
#[derive(Debug, Default)]
pub struct PortalWalkScratch {
    generation: u32,
    reached_at: Vec<u32>,
    queued_at: Vec<u32>,
    rects: Vec<ScreenRect>,
    reached: Vec<u32>,
    /// The draw-set buffer lent out per frame.
    cells: Vec<u32>,
    /// Front-to-back worklist: nearest clipped distance, ties by cell id, so
    /// the order never depends on how cells or seeds arrived.
    queue: BinaryHeap<Reverse<(u32, u32)>>,
    pub(super) clip_a: Vec<Vec3>,
    pub(super) clip_b: Vec<Vec3>,
    pub(super) planes: Vec<FrustumPlane>,
    expansions: Vec<u32>,
    /// The expansion that first reached each cell: (parent, parent's
    /// expansion number). Seeds record `(u32::MAX, 0)`.
    first_reached_by: Vec<(u32, u32)>,
    expanding: (u32, u32),
}

impl PortalWalkScratch {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a walk over `cell_count` cells.
    pub(super) fn begin(&mut self, cell_count: usize) {
        if self.reached_at.len() < cell_count {
            self.reached_at.resize(cell_count, 0);
            self.queued_at.resize(cell_count, 0);
            self.rects.resize(cell_count, ScreenRect::FULL);
            {
                self.expansions.resize(cell_count, 0);
                self.first_reached_by.resize(cell_count, (u32::MAX, 0));
            }
        }
        if self.generation == u32::MAX {
            self.reached_at.fill(0);
            self.queued_at.fill(0);
            self.generation = 0;
        }
        self.generation += 1;
        self.reached.clear();
        self.queue.clear();
        {
            self.expanding = (u32::MAX, 0);
        }
    }

    /// Union `rect` into `cell`'s region and queue the cell when it is new or
    /// its region grew while not already queued. `key` orders the worklist.
    pub(super) fn offer(&mut self, cell: usize, rect: &ScreenRect, key: f32) {
        let generation = self.generation;
        if self.reached_at[cell] != generation {
            self.reached_at[cell] = generation;
            self.rects[cell] = *rect;
            {
                self.expansions[cell] = 0;
                self.first_reached_by[cell] = self.expanding;
            }
            self.reached.push(cell as u32);
            self.push(cell, key);
        } else if self.rects[cell].grow(rect) && self.queued_at[cell] != generation {
            self.push(cell, key);
        }
    }

    fn push(&mut self, cell: usize, key: f32) {
        self.queued_at[cell] = self.generation;
        // Non-negative floats order like their bit patterns.
        self.queue
            .push(Reverse((key.max(0.0).to_bits(), cell as u32)));
    }

    /// Next cell to expand and its rect at the moment it leaves the queue.
    pub(super) fn pop(&mut self) -> Option<(usize, ScreenRect)> {
        let Reverse((_, cell)) = self.queue.pop()?;
        let cell = cell as usize;
        self.queued_at[cell] = 0;
        {
            self.expansions[cell] += 1;
            self.expanding = (cell as u32, self.expansions[cell]);
        }
        Some((cell, self.rects[cell]))
    }

    /// Whether the last walk run on this scratch reached `cell`.
    /// `determine_visible_cells` runs no walk on its non-portal paths (nor
    /// for a located cell past the cell list), so after one of those this
    /// still describes an earlier walk.
    pub fn is_reached(&self, cell: usize) -> bool {
        self.reached_at.get(cell) == Some(&self.generation) && self.generation != 0
    }

    /// Cells the last walk run on this scratch reached, in reach order.
    /// `determine_visible_cells` runs no walk on its non-portal paths (nor
    /// for a located cell past the cell list), so after one of those this
    /// still describes an earlier walk.
    pub fn reached(&self) -> &[u32] {
        &self.reached
    }

    /// Hand back a `VisibleCells::Culled` buffer once the frame has consumed
    /// it, so the next `determine_visible_cells` reuses its allocation.
    pub fn recycle_cells(&mut self, mut cells: Vec<u32>) {
        cells.clear();
        self.cells = cells;
    }

    /// The draw-set buffer for one frame, empty.
    pub(crate) fn take_cells(&mut self) -> Vec<u32> {
        let mut cells = std::mem::take(&mut self.cells);
        cells.clear();
        cells
    }

    pub(crate) fn rect(&self, cell: usize) -> Option<ScreenRect> {
        self.is_reached(cell).then(|| self.rects[cell])
    }

    /// Times `cell` expanded in the last walk.
    pub(crate) fn expansions(&self, cell: usize) -> u32 {
        if self.is_reached(cell) {
            self.expansions[cell]
        } else {
            0
        }
    }

    pub(crate) fn first_reached_by(&self, cell: usize) -> Option<(u32, u32)> {
        self.is_reached(cell).then(|| self.first_reached_by[cell])
    }

    pub(crate) fn set_generation_for_test(&mut self, generation: u32) {
        self.generation = generation;
    }
}
