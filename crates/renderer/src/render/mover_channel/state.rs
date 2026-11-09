// Mover probe channel per-frame state: each mover slot's t′ as the game
// handed it over, and which slots changed since the GPU blocks, the streamed
// planner, and each legacy compose pass last saw them. Sized once per level;
// a frame allocates nothing.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)

use super::layout::NO_STATE;
use crate::render::MoverLightState;

/// The whole-load compose passes a mover's t′ can recompose: the direct
/// passes (A and B, one trigger) and the indirect SH compose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::render) enum LegacyPass {
    Direct = 0,
    Indirect = 1,
}

#[derive(Debug, Clone)]
pub(super) struct MoverTBlock {
    /// Slot → mover id, ascending (the layout's).
    ids: Vec<u32>,
    /// This frame's t′ per slot; `NO_STATE` where the game handed over none.
    values: Vec<f32>,
    /// `values` as little-endian bytes, the GPU block's image.
    bytes: Vec<u8>,
    /// Bits the GPU block holds.
    uploaded: Vec<u32>,
    /// Bits the streamed planner last observed.
    observed: Vec<u32>,
    /// Bits each legacy compose pass last encoded, by [`LegacyPass`].
    composed: [Vec<u32>; 2],
    /// Slots whose entries feed each legacy pass: a mover with no entry in
    /// a pass never recomposes it.
    feeds: [Vec<bool>; 2],
    /// Scratch: slots whose bits moved since `observed`.
    changed: Vec<u32>,
}

impl MoverTBlock {
    pub(super) fn new(ids: &[u32]) -> Self {
        let none = vec![NO_STATE.to_bits(); ids.len()];
        Self {
            ids: ids.to_vec(),
            values: vec![NO_STATE; ids.len()],
            bytes: none.iter().flat_map(|bits| bits.to_le_bytes()).collect(),
            uploaded: none.clone(),
            observed: none.clone(),
            composed: [none.clone(), none],
            feeds: [vec![true; ids.len()], vec![true; ids.len()]],
            changed: Vec::with_capacity(ids.len()),
        }
    }

    /// Which slots carry entries in `pass`.
    pub(super) fn set_feeds(&mut self, pass: LegacyPass, feeds: impl Fn(u32) -> bool) {
        for (slot, fed) in self.feeds[pass as usize].iter_mut().enumerate() {
            *fed = feeds(slot as u32);
        }
    }

    /// Take this frame's states. A mover the section does not name is
    /// ignored; a slot no state names, or one whose t′ is not finite, reads
    /// as no state and composes ungated.
    pub(super) fn set_states(&mut self, states: &[MoverLightState]) {
        self.values.fill(NO_STATE);
        for state in states {
            if let Ok(slot) = self.ids.binary_search(&state.mover_id)
                && state.t.is_finite()
            {
                self.values[slot] = state.t.clamp(0.0, 1.0);
            }
        }
        for (slot, value) in self.values.iter().enumerate() {
            self.bytes[slot * 4..slot * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }

    fn bits(&self, slot: usize) -> u32 {
        self.values[slot].to_bits()
    }

    /// The block's bytes when they differ from what the GPU holds, marking
    /// them held.
    pub(super) fn take_upload(&mut self) -> Option<&[u8]> {
        let mut differs = false;
        for slot in 0..self.values.len() {
            let bits = self.bits(slot);
            differs |= self.uploaded[slot] != bits;
            self.uploaded[slot] = bits;
        }
        differs.then_some(&self.bytes[..])
    }

    /// Slots whose t′ moved since the last call: the streamed planner's
    /// mover-state trigger. Observed once per planned frame, so a change on
    /// a frame that records no compose still stales its rows.
    pub(super) fn observe_changes(&mut self) -> &[u32] {
        self.changed.clear();
        for slot in 0..self.values.len() {
            let bits = self.bits(slot);
            if self.observed[slot] != bits {
                self.observed[slot] = bits;
                self.changed.push(slot as u32);
            }
        }
        &self.changed
    }

    /// Whether a slot feeding `pass` moved since that legacy pass last
    /// encoded.
    pub(super) fn changed_since_composed(&self, pass: LegacyPass) -> bool {
        let (composed, feeds) = (&self.composed[pass as usize], &self.feeds[pass as usize]);
        (0..self.values.len()).any(|slot| feeds[slot] && composed[slot] != self.bits(slot))
    }

    /// Legacy pass `pass` encoded this frame's t′.
    pub(super) fn commit_composed(&mut self, pass: LegacyPass) {
        for slot in 0..self.values.len() {
            self.composed[pass as usize][slot] = self.bits(slot);
        }
    }

    #[cfg(test)]
    pub(super) fn values(&self) -> &[f32] {
        &self.values
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(mover_id: u32, t: f32) -> MoverLightState {
        MoverLightState { mover_id, t }
    }

    #[test]
    fn unnamed_movers_are_ignored_and_absent_ones_read_as_no_state() {
        let mut block = MoverTBlock::new(&[3, 9]);
        block.set_states(&[state(9, 0.25), state(4, 0.5)]);
        assert_eq!(block.values(), &[NO_STATE, 0.25]);
        block.set_states(&[state(3, f32::NAN), state(9, 2.0)]);
        assert_eq!(
            block.values(),
            &[NO_STATE, 1.0],
            "NaN is no state; t clamps"
        );
    }

    #[test]
    fn uploads_only_when_the_block_moved() {
        let mut block = MoverTBlock::new(&[1]);
        assert!(
            block.take_upload().is_none(),
            "the initial block is resident"
        );
        block.set_states(&[state(1, 0.5)]);
        assert_eq!(block.take_upload(), Some(&0.5f32.to_le_bytes()[..]));
        block.set_states(&[state(1, 0.5)]);
        assert!(block.take_upload().is_none());
    }

    /// P10: two frames between one pair of ticks hand over the same t; the
    /// second observes no change. P12: an unplanned change is never lost,
    /// since observation and commit are separate from upload.
    #[test]
    fn observation_reports_each_change_once() {
        let mut block = MoverTBlock::new(&[1, 2]);
        block.set_states(&[state(1, 0.5), state(2, 0.0)]);
        assert_eq!(block.observe_changes(), &[0, 1]);
        block.set_states(&[state(1, 0.5), state(2, 0.0)]);
        assert!(block.observe_changes().is_empty());
        block.set_states(&[state(2, 0.0)]);
        assert_eq!(
            block.observe_changes(),
            &[0],
            "dropping a state is a change"
        );
    }

    /// P12 on the legacy compose: each pass's trigger holds until that
    /// pass encodes, whatever the other pass did.
    #[test]
    fn the_legacy_trigger_holds_until_an_encode_commits_it() {
        let mut block = MoverTBlock::new(&[7]);
        for pass in [LegacyPass::Direct, LegacyPass::Indirect] {
            assert!(!block.changed_since_composed(pass));
        }
        block.set_states(&[state(7, 0.3)]);
        assert!(block.changed_since_composed(LegacyPass::Direct));
        assert!(block.changed_since_composed(LegacyPass::Indirect));
        // A frame that records nothing leaves it pending.
        block.set_states(&[state(7, 0.3)]);
        block.commit_composed(LegacyPass::Direct);
        assert!(!block.changed_since_composed(LegacyPass::Direct));
        assert!(
            block.changed_since_composed(LegacyPass::Indirect),
            "the indirect pass has not encoded the change yet"
        );
        block.commit_composed(LegacyPass::Indirect);
        assert!(!block.changed_since_composed(LegacyPass::Indirect));
    }

    /// C14 on the legacy compose: a mover whose entries feed only one pass
    /// never recomposes the other.
    #[test]
    fn a_mover_recomposes_only_the_passes_its_entries_feed() {
        let mut block = MoverTBlock::new(&[3, 9]);
        block.set_feeds(LegacyPass::Direct, |slot| slot == 0);
        block.set_feeds(LegacyPass::Indirect, |slot| slot == 1);
        block.set_states(&[state(9, 0.5)]);
        assert!(!block.changed_since_composed(LegacyPass::Direct));
        assert!(block.changed_since_composed(LegacyPass::Indirect));
        block.set_states(&[state(3, 0.5), state(9, 0.5)]);
        assert!(block.changed_since_composed(LegacyPass::Direct));
    }

    /// The per-frame path allocates nothing after warm-up.
    #[test]
    fn a_frame_keeps_every_buffer_at_its_capacity() {
        let mut block = MoverTBlock::new(&[1, 2]);
        let capacities = |block: &MoverTBlock| {
            (
                block.values.capacity(),
                block.bytes.capacity(),
                block.changed.capacity(),
            )
        };
        let before = capacities(&block);
        for frame in 0..8 {
            let t = frame as f32 / 8.0;
            block.set_states(&[state(1, t), state(2, 1.0 - t)]);
            let _ = block.take_upload();
            let _ = block.observe_changes();
            let _ = block.changed_since_composed(LegacyPass::Indirect);
            block.commit_composed(LegacyPass::Indirect);
        }
        assert_eq!(capacities(&block), before);
    }
}
