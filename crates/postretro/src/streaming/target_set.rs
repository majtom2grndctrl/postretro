//! Atomic per-resource target set with each target's live drain class.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency"

// The frame thread writes it; the issuer and the decode pool read it. The
// issuer checks membership before every physical read and reads a pending
// request's tier from its key's class; a queued decode reads its rank from
// the same class. A key that turns mandatory, or loses its class, while its
// read or decode waits is therefore lifted or lowered in place, with no
// message to the issuer and no state of its own to clear: every read of a
// key sees only that key's current class.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use super::drain_budget::DrainClass;

/// A targeted key whose class its resource has not published. Readers fall
/// back to the class the item was requested at.
const CLASS_UNPUBLISHED: u8 = 0;

#[derive(Debug)]
pub(crate) struct TargetSet {
    /// Membership, one bit per key.
    words: Box<[AtomicU64]>,
    /// Per key: [`CLASS_UNPUBLISHED`], or 1 + the class's index. Meaningful
    /// only while the key's membership bit is set.
    classes: Box<[AtomicU8]>,
}

fn encode(class: DrainClass) -> u8 {
    // Six classes: the index always fits.
    class.index() as u8 + 1
}

impl TargetSet {
    /// Starts empty: nothing is read until the session publishes targets.
    pub(crate) fn new(key_count: u32) -> Self {
        let words = (key_count as usize).div_ceil(64);
        Self {
            words: (0..words).map(|_| AtomicU64::new(0)).collect(),
            classes: (0..key_count).map(|_| AtomicU8::new(0)).collect(),
        }
    }

    /// Replaces the whole set. Each target's class is stored before any
    /// membership word, so a newly targeted key never reads an older class.
    /// A reader racing a publish sees every key as either its old or its new
    /// membership. Keys beyond the resource's key count are ignored.
    pub(crate) fn publish(&self, targets: impl IntoIterator<Item = (u32, DrainClass)>) {
        let mut next = vec![0u64; self.words.len()];
        for (key, class) in targets {
            if let Some(slot) = self.classes.get(key as usize) {
                slot.store(encode(class), Ordering::Release);
                next[key as usize / 64] |= 1 << (key % 64);
            }
        }
        for (word, value) in self.words.iter().zip(next) {
            word.store(value, Ordering::Release);
        }
    }

    /// Replaces the whole set with keys whose class is unpublished: readers
    /// use each item's requested class.
    #[cfg(test)]
    pub(crate) fn publish_keys(&self, targets: &std::collections::BTreeSet<u32>) {
        let mut next = vec![0u64; self.words.len()];
        for &key in targets {
            if let Some(slot) = self.classes.get(key as usize) {
                slot.store(CLASS_UNPUBLISHED, Ordering::Release);
                next[key as usize / 64] |= 1 << (key % 64);
            }
        }
        for (word, value) in self.words.iter().zip(next) {
            word.store(value, Ordering::Release);
        }
    }

    /// Targets `key` at `class`, or untargets it with `None`, without
    /// touching other keys, so a resource that tracks target changes
    /// incrementally publishes without allocating. Keys beyond the
    /// resource's key count are ignored.
    pub(crate) fn set(&self, key: u32, class: Option<DrainClass>) {
        let (Some(word), Some(slot)) = (
            self.words.get(key as usize / 64),
            self.classes.get(key as usize),
        ) else {
            return;
        };
        let bit = 1u64 << (key % 64);
        match class {
            Some(class) => {
                slot.store(encode(class), Ordering::Release);
                word.fetch_or(bit, Ordering::AcqRel);
            }
            None => {
                word.fetch_and(!bit, Ordering::AcqRel);
            }
        }
    }

    pub(crate) fn contains(&self, key: u32) -> bool {
        self.words
            .get(key as usize / 64)
            .is_some_and(|word| word.load(Ordering::Acquire) & (1 << (key % 64)) != 0)
    }

    /// `key`'s live class: `None` when it is untargeted or its class is
    /// unpublished.
    pub(crate) fn class(&self, key: u32) -> Option<DrainClass> {
        if !self.contains(key) {
            return None;
        }
        match self.classes.get(key as usize)?.load(Ordering::Acquire) {
            CLASS_UNPUBLISHED => None,
            encoded => DrainClass::ALL.get(usize::from(encoded - 1)).copied(),
        }
    }
}

/// Keys of a set, for callers that hold membership as a `BTreeSet`.
#[cfg(test)]
pub(crate) fn keys_at(
    targets: &std::collections::BTreeSet<u32>,
    class: DrainClass,
) -> impl Iterator<Item = (u32, DrainClass)> + '_ {
    targets.iter().map(move |&key| (key, class))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn publish_replaces_membership_across_word_boundaries() {
        let set = TargetSet::new(130);
        assert!(!set.contains(0), "starts empty");
        set.publish(keys_at(
            &BTreeSet::from([0, 63, 64, 129, 500]),
            DrainClass::Visible,
        ));
        for id in [0, 63, 64, 129] {
            assert!(set.contains(id), "{id}");
            assert_eq!(set.class(id), Some(DrainClass::Visible), "{id}");
        }
        assert!(!set.contains(1));
        assert!(!set.contains(500), "out of range ids are ignored");

        set.publish([(1, DrainClass::Lead)]);
        assert_eq!(set.class(1), Some(DrainClass::Lead));
        assert!(!set.contains(0) && !set.contains(129));
        assert_eq!(set.class(0), None, "an untargeted key has no class");
    }

    #[test]
    fn set_changes_one_key_and_leaves_its_word_neighbours() {
        let set = TargetSet::new(130);
        set.publish(keys_at(&BTreeSet::from([63, 64]), DrainClass::Prefetch));
        set.set(65, Some(DrainClass::Pinned));
        set.set(63, None);
        set.set(500, Some(DrainClass::Visible));
        assert!(!set.contains(63));
        assert!(set.contains(64) && set.contains(65));
        assert_eq!(set.class(65), Some(DrainClass::Pinned));
        assert_eq!(set.class(64), Some(DrainClass::Prefetch));
        assert!(!set.contains(500), "out of range ids are ignored");
    }

    // A key's class changes in place, in either direction, and a key
    // untargeted and targeted again reads only its new class.
    #[test]
    fn a_class_rises_and_falls_in_place_and_never_outlives_its_target() {
        let set = TargetSet::new(8);
        set.set(3, Some(DrainClass::Prefetch));
        set.set(3, Some(DrainClass::Visible));
        assert_eq!(set.class(3), Some(DrainClass::Visible));
        set.set(3, Some(DrainClass::Hysteresis));
        assert_eq!(set.class(3), Some(DrainClass::Hysteresis));
        set.set(3, Some(DrainClass::Visible));
        set.set(3, None);
        assert_eq!(set.class(3), None);
        set.set(3, Some(DrainClass::Prefetch));
        assert_eq!(set.class(3), Some(DrainClass::Prefetch));
    }

    #[test]
    fn keys_published_without_a_class_read_as_unpublished() {
        let set = TargetSet::new(8);
        set.set(2, Some(DrainClass::Visible));
        set.publish_keys(&BTreeSet::from([2, 5]));
        assert!(set.contains(2) && set.contains(5));
        assert_eq!(set.class(2), None, "the older class is not kept");
        assert_eq!(set.class(5), None);
    }
}
