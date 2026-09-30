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

    /// Sets or clears one key without touching the others, so a resource
    /// that tracks target changes incrementally publishes without
    /// allocating. Keys beyond the resource's key count are ignored.
    pub(crate) fn set(&self, key: u32, targeted: bool) {
        let Some(word) = self.words.get(key as usize / 64) else {
            return;
        };
        let bit = 1u64 << (key % 64);
        if targeted {
            word.fetch_or(bit, Ordering::AcqRel);
        } else {
            word.fetch_and(!bit, Ordering::AcqRel);
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

    #[test]
    fn set_changes_one_key_and_leaves_its_word_neighbours() {
        let bitset = TargetBitset::new(130);
        bitset.publish(&BTreeSet::from([63, 64]));
        bitset.set(65, true);
        bitset.set(63, false);
        bitset.set(500, true);
        assert!(!bitset.contains(63));
        assert!(bitset.contains(64) && bitset.contains(65));
        assert!(!bitset.contains(500), "out of range ids are ignored");
    }
}
