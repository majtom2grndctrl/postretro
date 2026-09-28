//! Dense affinity-row membership set.
//!
//! Resident-row unions are read per gated row every frame and written once per
//! touched row on install and eviction. A bitset over the level's row space
//! makes both O(1); the storage is sized at level load and never grows, so
//! steady-state inserts never allocate.

use std::collections::BTreeSet;
use std::fmt;

#[derive(Clone, Default)]
pub(super) struct RowBitSet {
    words: Vec<u64>,
    capacity: usize,
    len: usize,
}

impl RowBitSet {
    /// Storage for row ids `0..rows`, fixed at level install to the same
    /// affinity-row space as the compose planner: the brick grid widened to
    /// every sparse CSR row count. Every inserted row comes from that
    /// installed data, so an out-of-range row is a caller bug. Debug builds
    /// assert on one; release builds ignore it rather than grow.
    pub(super) fn with_row_capacity(rows: usize) -> Self {
        Self {
            words: vec![0; rows.div_ceil(64)],
            capacity: rows,
            len: 0,
        }
    }

    pub(super) fn contains(&self, row: &u32) -> bool {
        let (word, bit) = split(*row);
        self.words.get(word).is_some_and(|bits| bits & bit != 0)
    }

    /// Returns whether `row` was absent.
    pub(super) fn insert(&mut self, row: u32) -> bool {
        if row as usize >= self.capacity {
            debug_assert!(false, "row {row} exceeds the row-set capacity");
            return false;
        }
        let (word, bit) = split(row);
        let absent = self.words[word] & bit == 0;
        self.words[word] |= bit;
        self.len += usize::from(absent);
        absent
    }

    /// Returns whether `row` was present.
    pub(super) fn remove(&mut self, row: &u32) -> bool {
        let (word, bit) = split(*row);
        let Some(bits) = self.words.get_mut(word) else {
            return false;
        };
        let present = *bits & bit != 0;
        *bits &= !bit;
        self.len -= usize::from(present);
        present
    }

    /// Keeps the storage for reuse.
    pub(super) fn clear(&mut self) {
        self.words.fill(0);
        self.len = 0;
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.len
    }

    /// Ascending row ids.
    pub(super) fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        iter_words(self.words.iter().copied())
    }

    /// Ascending row ids present in any of `sets`, each once.
    pub(super) fn iter_union<'a>(sets: [&'a RowBitSet; 3]) -> impl Iterator<Item = u32> + 'a {
        let words = sets.iter().map(|set| set.words.len()).max().unwrap_or(0);
        iter_words((0..words).map(move |index| {
            sets.iter().fold(0, |bits, set| {
                bits | set.words.get(index).copied().unwrap_or(0)
            })
        }))
    }
}

fn split(row: u32) -> (usize, u64) {
    ((row / 64) as usize, 1 << (row % 64))
}

fn iter_words(words: impl Iterator<Item = u64>) -> impl Iterator<Item = u32> {
    words.enumerate().flat_map(|(index, mut bits)| {
        std::iter::from_fn(move || {
            if bits == 0 {
                return None;
            }
            let bit = bits.trailing_zeros();
            bits &= bits - 1;
            Some(index as u32 * 64 + bit)
        })
    })
}

/// Row ids queued for one later drain, each listed at most once until the
/// queue is cleared. However many pushes land between drains, the list never
/// exceeds the row capacity; push and clear cost O(1) per row.
pub(super) struct RowQueue {
    rows: Vec<u32>,
    queued: RowBitSet,
}

impl RowQueue {
    pub(super) fn with_row_capacity(rows: usize) -> Self {
        Self {
            rows: Vec::new(),
            queued: RowBitSet::with_row_capacity(rows),
        }
    }

    /// Queue `row` unless it is already queued. Out-of-range rows follow
    /// `RowBitSet::insert`.
    pub(super) fn push(&mut self, row: u32) {
        if self.queued.insert(row) {
            self.rows.push(row);
        }
    }

    /// Queued rows in first-push order.
    pub(super) fn rows(&self) -> &[u32] {
        &self.rows
    }

    /// O(queued rows); keeps both allocations for reuse.
    pub(super) fn clear(&mut self) {
        for row in &self.rows {
            self.queued.remove(row);
        }
        self.rows.clear();
    }
}

impl PartialEq for RowBitSet {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.iter().eq(other.iter())
    }
}

impl Eq for RowBitSet {}

impl PartialEq<BTreeSet<u32>> for RowBitSet {
    fn eq(&self, other: &BTreeSet<u32>) -> bool {
        self.len == other.len() && self.iter().eq(other.iter().copied())
    }
}

impl fmt::Debug for RowBitSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

/// Sized to exactly the collected rows' span.
impl FromIterator<u32> for RowBitSet {
    fn from_iter<I: IntoIterator<Item = u32>>(rows: I) -> Self {
        let rows: Vec<u32> = rows.into_iter().collect();
        let capacity = rows.iter().max().map_or(0, |&row| row as usize + 1);
        let mut set = Self::with_row_capacity(capacity);
        for row in rows {
            set.insert(row);
        }
        set
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn row_bitset_matches_btreeset(
            ops in prop::collection::vec((any::<bool>(), 0u32..300), 0..200),
        ) {
            let mut bits = RowBitSet::with_row_capacity(300);
            let mut oracle = BTreeSet::new();
            for (insert, row) in ops {
                if insert {
                    prop_assert_eq!(bits.insert(row), oracle.insert(row));
                } else {
                    prop_assert_eq!(bits.remove(&row), oracle.remove(&row));
                }
                prop_assert_eq!(bits.len(), oracle.len());
            }
            prop_assert!(bits == oracle);
            for row in 0..300 {
                prop_assert_eq!(bits.contains(&row), oracle.contains(&row));
            }
        }
    }

    #[test]
    fn queue_lists_each_row_once_until_cleared() {
        let mut queue = RowQueue::with_row_capacity(8);
        for row in [5, 2, 5, 5, 2, 7] {
            queue.push(row);
        }
        assert_eq!(queue.rows(), &[5, 2, 7]);
        queue.clear();
        assert!(queue.rows().is_empty());
        queue.push(2);
        assert_eq!(queue.rows(), &[2]);
    }

    #[test]
    fn union_iterates_each_row_once_in_ascending_order() {
        let a: RowBitSet = [3, 64, 200].into_iter().collect();
        let b: RowBitSet = [3, 5].into_iter().collect();
        let c = RowBitSet::with_row_capacity(1000);
        assert_eq!(
            RowBitSet::iter_union([&a, &b, &c]).collect::<Vec<_>>(),
            [3, 5, 64, 200]
        );
    }
}
