// Bounded marker cache identity across residency changes and slot reuse.
// See: context/lib/rendering_pipeline.md §Cluster SH residency

use std::ops::Range;

use super::*;

impl StreamedProbeReadback {
    // Overlay toggles and session resets invalidate every cached/pending sample.
    pub(in crate::render::sh_streaming) fn invalidate(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.colors.clear();
        self.slot_incarnations.clear();
        // An in-flight map still needs its range length until it is unmapped.
        if !self.copied && !self.mapping {
            self.identities.clear();
        }
    }

    pub(in crate::render::sh_streaming) fn color(
        &self,
        dense: usize,
        sampled_word: u32,
    ) -> Option<[f32; 3]> {
        let index = self
            .colors
            .binary_search_by_key(&dense, |color| color.identity.dense)
            .ok()?;
        let color = &self.colors[index];
        // Drains reconcile every real sampled-word/slot change before callers
        // can draw or publish a map; the per-marker hot path needs only the word.
        (sampled_word != 0 && color.identity.word == sampled_word).then_some(color.rgb)
    }

    pub(in crate::render::sh_streaming) fn retire_slots(&mut self, range: Range<u32>) {
        // allocate_owned_nodes/journal_allocate_node reserve a whole StoredNode;
        // evict releases that same range atomically. L1's word names its base,
        // so retiring the base rejects every corner read in that node.
        let retired: Vec<_> = self
            .slot_incarnations
            .range(range)
            .map(|(&slot, _)| slot)
            .collect();
        for slot in retired {
            self.slot_incarnations.remove(&slot);
        }
        self.sample_words_changed = true;
    }

    pub(in crate::render::sh_streaming) fn sample_words_changed(&mut self) {
        self.sample_words_changed = true;
    }

    pub(in crate::render::sh_streaming) fn reconcile(&mut self, dims: [u32; 3], words: &[u32]) {
        if !std::mem::take(&mut self.sample_words_changed) {
            return;
        }
        // One bounded sweep per drain, including adaptive corner changes.
        // No-op/deferred drains never discard colors or rescan the cache.
        self.validation_epoch = self
            .validation_epoch
            .checked_add(1)
            .expect("marker validation epoch overflow");
        let incarnations = &mut self.slot_incarnations;
        let validation_epoch = self.validation_epoch;
        self.colors.retain(|color| {
            identity_matches(color.identity, dims, words, incarnations, validation_epoch)
        });
        if self.wanted && self.copied_epoch == self.epoch && (self.copied || self.mapping) {
            for identity in &mut self.identities {
                identity.valid &=
                    identity_matches(*identity, dims, words, incarnations, validation_epoch);
            }
        }
    }

    pub(in crate::render::sh_streaming) fn capture_incarnations(&mut self) {
        // Stamp the two bounded live sets in place instead of allocating a
        // second tree of cache references on every capture.
        for color in &self.colors {
            if let Some(current) = self.slot_incarnations.get_mut(&slot(color.identity.word)) {
                current.referenced = true;
            }
        }
        for identity in &mut self.identities {
            let current = self
                .slot_incarnations
                .entry(slot(identity.word))
                .or_insert_with(|| {
                    self.next_incarnation = self
                        .next_incarnation
                        .checked_add(1)
                        .expect("marker slot incarnation overflow");
                    SlotIncarnation::new(self.next_incarnation)
                });
            identity.incarnation = current.incarnation;
            current.referenced = true;
        }
        self.slot_incarnations
            .retain(|_, current| std::mem::take(&mut current.referenced));
        debug_assert!(self.slot_incarnations.len() <= 2 * MAX_PROBES);
    }
}
