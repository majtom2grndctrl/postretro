// Per-cell residency for a streamed id-57 payload: each listed cell's payload
// takes a range of the table's pool while a resident cluster carries it, and
// gives it back when the last one leaves. Streaming is residency: the bytes a
// cell holds here are the bytes a resident placement holds, only placed while
// the cell is resident. wgpu-free; `gpu.rs` owns the buffer behind it.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting, Bounded SH pool)

use crate::render::sh_streaming::{FirstFitRanges, PoolRange};

/// Headroom over the first estimate: first-fit's address high-water runs
/// above what is live, as for the SH pools.
const POOL_HEADROOM_PERCENT: u64 = 25;

/// One table's pooled payload region, in 32-bit words.
#[derive(Debug, Clone)]
pub(super) struct CellPool {
    /// Words the GPU table holds for payload right now.
    capacity_words: u32,
    /// Every listed cell's payload at once: what a resident placement holds,
    /// and the most this pool is sized to before fragmentation.
    whole_words: u32,
    ranges: FirstFitRanges,
    /// Per table plan: its payload's words, and its range while resident.
    cell_words: Vec<u32>,
    cells: Vec<Option<PoolRange>>,
    live_words: u64,
}

impl CellPool {
    /// A pool for cells of `cell_words` each, with no capacity yet: the
    /// streamed level sizes it once its SH pool is planned.
    pub(super) fn new(cell_words: Vec<u32>) -> Self {
        let whole_words = cell_words
            .iter()
            .fold(0u32, |total, &words| total.saturating_add(words));
        Self {
            capacity_words: 0,
            whole_words,
            ranges: FirstFitRanges::default(),
            cells: vec![None; cell_words.len()],
            cell_words,
            live_words: 0,
        }
    }

    /// The first capacity for a level whose dense SH pool starts at
    /// `dense_fraction` of the map: the same share of the whole payload plus
    /// headroom, capped at the whole payload. Resident channel cells are
    /// resident SH cells, so the share follows the dense pool; growth is the
    /// logged net for the rest.
    pub(super) fn initial_words(&self, dense_fraction: f64) -> u32 {
        // NaN or a whole dense pool: the whole payload.
        if dense_fraction.is_nan() || dense_fraction >= 1.0 {
            return self.whole_words;
        }
        let share = (f64::from(self.whole_words) * dense_fraction.max(0.0)).ceil() as u64;
        let with_headroom = share + share * POOL_HEADROOM_PERCENT / 100;
        with_headroom.min(u64::from(self.whole_words)) as u32
    }

    pub(super) fn capacity_words(&self) -> u32 {
        self.capacity_words
    }

    pub(super) fn set_capacity_words(&mut self, words: u32) {
        debug_assert!(
            words >= self.ranges.capacity,
            "a pool never shrinks below its live high-water"
        );
        self.capacity_words = words;
    }

    pub(super) fn live_words(&self) -> u64 {
        self.live_words
    }

    pub(super) fn range(&self, plan: usize) -> Option<PoolRange> {
        self.cells.get(plan).copied().flatten()
    }

    /// Place cell `plan`'s payload. Returns its range; the caller grows the
    /// table first when [`Self::needed_words`] passes the capacity.
    pub(super) fn place(&mut self, plan: usize) -> Option<PoolRange> {
        if let Some(range) = self.range(plan) {
            return Some(range);
        }
        let words = *self.cell_words.get(plan)?;
        let (range, _) = self.ranges.allocate_undoable(words).ok()?;
        self.cells[plan] = Some(range);
        self.live_words += u64::from(words);
        Some(range)
    }

    /// The words the table must hold for every range placed so far.
    pub(super) fn needed_words(&self) -> u32 {
        self.ranges.capacity
    }

    /// The capacity to grow to when [`Self::needed_words`] passed it: half
    /// again, at least the need, and no more than the whole payload unless
    /// fragmentation needs it.
    pub(super) fn grown_words(&self) -> u32 {
        let need = self.needed_words();
        let half_again = self
            .capacity_words
            .saturating_add(self.capacity_words / 2)
            .min(self.whole_words);
        need.max(half_again)
    }

    /// Free cell `plan`'s range, returning it.
    pub(super) fn release(&mut self, plan: usize) -> Option<PoolRange> {
        let range = self.cells.get_mut(plan)?.take()?;
        self.ranges
            .release(range)
            .expect("a placed pool range releases once");
        self.live_words -= u64::from(range.len);
        Some(range)
    }

    /// Forget every placement: a new residency session starts empty.
    pub(super) fn clear(&mut self) {
        self.ranges = FirstFitRanges::default();
        self.cells.fill(None);
        self.live_words = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cell_places_once_and_its_release_frees_its_words() {
        let mut pool = CellPool::new(vec![4, 8, 2]);
        pool.set_capacity_words(pool.initial_words(1.0));
        assert_eq!(
            pool.capacity_words(),
            14,
            "a whole-map pool holds every cell"
        );
        let first = pool.place(1).unwrap();
        assert_eq!(
            pool.place(1),
            Some(first),
            "a resident cell keeps its range"
        );
        assert_eq!(pool.live_words(), 8);
        assert_eq!(pool.release(1), Some(first));
        assert_eq!(pool.live_words(), 0);
        assert_eq!(pool.release(1), None, "a cell releases once");
        assert_eq!(
            pool.place(1),
            Some(first),
            "first fit reuses the freed range"
        );
    }

    #[test]
    fn the_first_capacity_follows_the_dense_share_and_growth_covers_the_need() {
        let mut pool = CellPool::new(vec![100; 10]);
        assert_eq!(
            pool.initial_words(0.2),
            250,
            "a fifth of 1000 words, plus a quarter"
        );
        assert_eq!(
            pool.initial_words(0.95),
            1000,
            "capped at the whole payload"
        );
        pool.set_capacity_words(pool.initial_words(0.2));
        for plan in 0..3 {
            pool.place(plan).unwrap();
        }
        assert_eq!(pool.needed_words(), 300);
        assert!(pool.needed_words() > pool.capacity_words());
        assert_eq!(pool.grown_words(), 375, "half again over 250");
    }
}
