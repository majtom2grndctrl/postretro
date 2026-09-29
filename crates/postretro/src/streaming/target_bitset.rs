//! Atomic per-resource target set the shared issuer checks before each read.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency"

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub(crate) struct TargetBitset {
    words: Box<[AtomicU64]>,
}

impl TargetBitset {
    /// Starts empty: nothing is read until the session publishes targets.
    pub(crate) fn new(key_count: u32) -> Self {
        let words = (key_count as usize).div_ceil(64);
        Self {
            words: (0..words).map(|_| AtomicU64::new(0)).collect(),
        }
    }

    /// Replaces the whole set. Each word is stored atomically; a reader racing
    /// a publish sees every key as either its old or its new membership.
    /// Keys beyond the resource's key count are ignored.
    pub(crate) fn publish(&self, targets: &BTreeSet<u32>) {
        let mut next = vec![0u64; self.words.len()];
        for &key in targets {
            if let Some(word) = next.get_mut(key as usize / 64) {
                *word |= 1 << (key % 64);
            }
        }
        for (word, value) in self.words.iter().zip(next) {
            word.store(value, Ordering::Release);
        }
    }

    pub(crate) fn contains(&self, key: u32) -> bool {
        self.words
            .get(key as usize / 64)
            .is_some_and(|word| word.load(Ordering::Acquire) & (1 << (key % 64)) != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_replaces_membership_across_word_boundaries() {
        let bitset = TargetBitset::new(130);
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
