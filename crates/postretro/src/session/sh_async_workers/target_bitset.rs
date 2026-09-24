//! Shared atomic copy of the controller's target set, read by the I/O issuer
//! just before each physical read so departed work is never read.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub(super) struct ShTargetBitset {
    words: Box<[AtomicU64]>,
}

impl ShTargetBitset {
    /// Starts empty: nothing is read until the session publishes targets.
    pub(super) fn new(cluster_count: u32) -> Self {
        let words = (cluster_count as usize).div_ceil(64);
        Self {
            words: (0..words).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    /// Replaces the whole set. Each word is stored atomically; a reader racing
    /// a publish sees every cluster as either its old or its new membership.
    /// Ids beyond the level's cluster count are ignored.
    pub(super) fn publish(&self, targets: &BTreeSet<u32>) {
        let mut next = vec![0u64; self.words.len()];
        for &cluster_id in targets {
            if let Some(word) = next.get_mut(cluster_id as usize / 64) {
                *word |= 1 << (cluster_id % 64);
            }
        }
        for (word, value) in self.words.iter().zip(next) {
            word.store(value, Ordering::Release);
        }
    }

    pub(super) fn contains(&self, cluster_id: u32) -> bool {
        self.words
            .get(cluster_id as usize / 64)
            .is_some_and(|word| word.load(Ordering::Acquire) & (1 << (cluster_id % 64)) != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_replaces_membership_across_word_boundaries() {
        let bitset = ShTargetBitset::new(130);
        assert!(!bitset.contains(0), "starts empty");
        bitset.publish(&BTreeSet::from([0, 63, 64, 129, 500]));
        for id in [0, 63, 64, 129] {
            assert!(bitset.contains(id), "{id}");
        }
        assert!(!bitset.contains(1));
        assert!(!bitset.contains(500), "out of range ids are ignored");

        bitset.publish(&BTreeSet::from([1]));
        assert!(bitset.contains(1));
        assert!(!bitset.contains(0) && !bitset.contains(129));
    }
}
