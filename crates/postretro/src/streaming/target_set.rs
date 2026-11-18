//! Atomic per-resource target set with each target's live drain class.
//! See: context/lib/rendering_pipeline.md §4 (one issuer, one budget)

// The frame thread writes it; the issuer and the decode pool read it. The
// issuer checks membership before every physical read and reads a pending
// request's tier from its key's class; a queued decode reads its rank from
// the same class. A key that turns mandatory, or loses its class, while its
// read or decode waits is therefore lifted or lowered in place, with no
// message to the issuer and no state of its own to clear: every read of a
// key sees only that key's current class.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use super::drain_budget::DrainClass;

/// A targeted key published without a class. Only tests publish membership
/// alone; readers then fall back to the class the item was requested at.
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

    /// Replaces the whole set without allocating, and stores only the
    /// classes and membership words that change. Each target's class is
    /// stored before its membership bit, so a newly targeted key never reads
    /// an older class. A reader racing a publish sees each key with either
    /// its old or its new membership and class; the set as a whole is not
    /// swapped atomically, so it may see some keys already published and
    /// others not yet. Keys in ascending order store each word once; any
    /// order is accepted. Keys beyond the resource's key count are ignored.
    pub(crate) fn publish(&self, targets: impl IntoIterator<Item = (u32, DrainClass)>) {
        self.replace(targets.into_iter().map(|(key, class)| (key, encode(class))));
    }

    /// Replaces the whole set with keys whose class is unpublished: readers
    /// use each item's requested class.
    #[cfg(test)]
    pub(crate) fn publish_keys(&self, targets: &std::collections::BTreeSet<u32>) {
        self.replace(targets.iter().map(|&key| (key, CLASS_UNPUBLISHED)));
    }

    /// [`Self::publish`] over encoded classes. Builds the membership word in
    /// progress locally and stores it once the keys move past it; a key
    /// whose word was already stored joins it in place.
    fn replace(&self, targets: impl Iterator<Item = (u32, u8)>) {
        let mut word_index = 0;
        let mut word = 0u64;
        for (key, encoded) in targets {
            let Some(slot) = self.classes.get(key as usize) else {
                continue;
            };
            // Only the frame thread writes, so a relaxed load sees its own
            // last store.
            if slot.load(Ordering::Relaxed) != encoded {
                slot.store(encoded, Ordering::Release);
            }
            let index = key as usize / 64;
            let bit = 1u64 << (key % 64);
            if index < word_index {
                self.words[index].fetch_or(bit, Ordering::AcqRel);
                continue;
            }
            while word_index < index {
                self.store_word(word_index, std::mem::take(&mut word));
                word_index += 1;
            }
            word |= bit;
        }
        while word_index < self.words.len() {
            self.store_word(word_index, std::mem::take(&mut word));
            word_index += 1;
        }
    }

    fn store_word(&self, index: usize, value: u64) {
        let word = &self.words[index];
        if word.load(Ordering::Relaxed) != value {
            word.store(value, Ordering::Release);
        }
    }

    /// Targets `key` at `class`, or untargets it with `None`, without
    /// touching other keys, for a resource that tracks its target changes
    /// itself. The class is stored before the membership bit. Keys beyond
    /// the resource's key count are ignored.
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

    /// `key`'s live class: `None` when it is not a member. Every production
    /// publish stores a class with each member, so there `None` means only
    /// that; a member published by a test without a class also reads `None`.
    pub(crate) fn class(&self, key: u32) -> Option<DrainClass> {
        self.member_class(key, None).flatten()
    }

    /// `None` when `key` is not a member; otherwise its live class, or
    /// `requested` when its class is unpublished.
    pub(crate) fn class_or(&self, key: u32, requested: DrainClass) -> Option<DrainClass> {
        self.member_class(key, Some(requested)).flatten()
    }

    fn member_class(
        &self,
        key: u32,
        unpublished: Option<DrainClass>,
    ) -> Option<Option<DrainClass>> {
        if !self.contains(key) {
            return None;
        }
        Some(
            match self.classes.get(key as usize)?.load(Ordering::Acquire) {
                CLASS_UNPUBLISHED => unpublished,
                encoded => DrainClass::ALL.get(usize::from(encoded - 1)).copied(),
            },
        )
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

    // Keys out of order, and keys landing in a word already stored, still
    // publish exactly the given set.
    #[test]
    fn publish_accepts_keys_in_any_order() {
        let set = TargetSet::new(200);
        set.publish(keys_at(&BTreeSet::from([1, 2, 130]), DrainClass::Lead));
        set.publish([
            (129, DrainClass::Visible),
            (3, DrainClass::Pinned),
            (70, DrainClass::Prefetch),
            (2, DrainClass::Hysteresis),
        ]);
        let members: Vec<u32> = (0..200).filter(|&key| set.contains(key)).collect();
        assert_eq!(members, [2, 3, 70, 129]);
        assert_eq!(set.class(2), Some(DrainClass::Hysteresis));
        assert_eq!(set.class(3), Some(DrainClass::Pinned));
        assert_eq!(set.class(129), Some(DrainClass::Visible));
    }

    #[test]
    fn class_or_falls_back_only_for_a_member_without_a_class() {
        let set = TargetSet::new(8);
        set.publish_keys(&BTreeSet::from([1]));
        set.set(2, Some(DrainClass::Lead));
        assert_eq!(
            set.class_or(1, DrainClass::Prefetch),
            Some(DrainClass::Prefetch)
        );
        assert_eq!(
            set.class_or(2, DrainClass::Prefetch),
            Some(DrainClass::Lead)
        );
        assert_eq!(set.class_or(3, DrainClass::Prefetch), None, "not a member");
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
