// Mover probe channel per-frame state: each mover slot's t′ as the game
// handed it over with its knot bracket, and which slots changed since the GPU
// blocks, the streamed planner, and each legacy compose pass last saw them.
// Sized once per level; a frame allocates nothing.
// See: context/lib/rendering_pipeline.md §4 (Mover state lighting)
//
// The bracket is a per-mover lookup acceleration, a function of t′ and the
// mover's knots alone: it never flags, zeroes or excludes a probe.

use super::knots::MoverKnots;
use super::layout::NO_STATE;
use crate::render::MoverTravel;

/// Words per mover slot in both tables' t′ block: its t′ bits, then its
/// knot bracket ([`MoverKnots::bracket`]), which compose starts each probe's
/// interval walk from instead of scanning every knot.
pub(super) const T_SLOT_WORDS: usize = 2;

/// Fill a table's t′ block (`words`, [`T_SLOT_WORDS`] per slot) as it reads
/// before any hand-over: no state, bracket 0, as [`MoverTBlock::set_states`]
/// writes a slot with no state.
pub(super) fn write_no_state(words: &mut [u32]) {
    let (slots, rest) = words.as_chunks_mut::<T_SLOT_WORDS>();
    debug_assert!(rest.is_empty(), "whole slots");
    slots.fill([NO_STATE.to_bits(), 0]);
}

/// `depth_written` for a slot whose last planned write did not land: a NaN
/// pattern, which [`MoverTBlock::set_states`] never stores (a non-finite t′
/// reads as `NO_STATE`), so no frame's bits equal it.
const DEPTH_UNWRITTEN: u32 = u32::MAX;

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
    /// Each slot's knots through its ramp, for its bracket.
    knots: MoverKnots,
    /// This frame's t′ per slot; `NO_STATE` where the game handed over none.
    values: Vec<f32>,
    /// `values` and each slot's bracket as little-endian bytes,
    /// [`T_SLOT_WORDS`] words per slot: the GPU block's image.
    bytes: Vec<u8>,
    /// Bits the GPU block holds.
    uploaded: Vec<u32>,
    /// Bits the streamed planner last observed.
    observed: Vec<u32>,
    /// Bits each legacy compose pass last encoded, by [`LegacyPass`].
    composed: [Vec<u32>; 2],
    /// Bits the depth live tiles were last written from.
    depth_written: Vec<u32>,
    /// Slots whose entries feed each legacy pass: a mover with no entry in
    /// a pass never recomposes it.
    feeds: [Vec<bool>; 2],
    /// Scratch: slots whose bits moved since `observed`.
    changed: Vec<u32>,
    /// Scratch: slots whose bits moved since `depth_written`.
    depth_changed: Vec<u32>,
}

impl MoverTBlock {
    /// The block of the movers `ids` (slot order), whose knots are `knots`.
    pub(super) fn new(ids: &[u32], knots: &MoverKnots) -> Self {
        let none = vec![NO_STATE.to_bits(); ids.len()];
        let mut words = vec![0; ids.len() * T_SLOT_WORDS];
        write_no_state(&mut words);
        Self {
            ids: ids.to_vec(),
            knots: knots.clone(),
            values: vec![NO_STATE; ids.len()],
            bytes: words.iter().flat_map(|word| word.to_le_bytes()).collect(),
            uploaded: none.clone(),
            observed: none.clone(),
            composed: [none.clone(), none.clone()],
            depth_written: none,
            feeds: [vec![true; ids.len()], vec![true; ids.len()]],
            changed: Vec::with_capacity(ids.len()),
            depth_changed: Vec::with_capacity(ids.len()),
        }
    }

    /// Which slots carry entries in `pass`.
    pub(super) fn set_feeds(&mut self, pass: LegacyPass, feeds: impl Fn(u32) -> bool) {
        for (slot, fed) in self.feeds[pass as usize].iter_mut().enumerate() {
            *fed = feeds(slot as u32);
        }
    }

    /// Take this frame's states, and each slot's knot bracket at its t′. A
    /// mover the section does not name is ignored; a slot no state names,
    /// or one whose t′ is not finite, reads as no state and composes
    /// ungated. Each mover appears at most once in one frame's list. Change
    /// detection compares bits, so −0.0 is stored as +0.0: the same
    /// position never reads as a move.
    pub(super) fn set_states(&mut self, states: &[MoverTravel]) {
        debug_assert!(
            states.iter().enumerate().all(|(at, state)| {
                states[..at]
                    .iter()
                    .all(|earlier| earlier.mover_id != state.mover_id)
            }),
            "one frame hands over each mover at most once"
        );
        self.values.fill(NO_STATE);
        for state in states {
            if let Ok(slot) = self.ids.binary_search(&state.mover_id)
                && state.t.is_finite()
            {
                // `+ 0.0` turns −0.0 into +0.0 and leaves every other value.
                self.values[slot] = state.t.clamp(0.0, 1.0) + 0.0;
            }
        }
        for (slot, value) in self.values.iter().enumerate() {
            // Once per mover per frame: every probe's interval walk starts
            // here. A pure function of t′, so the bits' change detection
            // covers it.
            let bracket = self.knots.bracket(slot, *value);
            let at = slot * T_SLOT_WORDS * 4;
            self.bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
            self.bytes[at + 4..at + 8].copy_from_slice(&bracket.to_le_bytes());
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

    /// Slots whose t′ moved since the streamed planner last committed an
    /// observation: its mover-state trigger. Peeking marks nothing; the
    /// caller commits with [`Self::commit_observed`] once the frame's plan
    /// has staled the rows, so a plan that fails keeps the change for the
    /// next frame. Planned once per frame, so a change on a frame that
    /// records no compose still stales its rows.
    pub(super) fn observe_changes(&mut self) -> &[u32] {
        self.changed.clear();
        for slot in 0..self.values.len() {
            if self.observed[slot] != self.bits(slot) {
                self.changed.push(slot as u32);
            }
        }
        &self.changed
    }

    /// The planner staled the rows of the slots [`Self::observe_changes`]
    /// last reported.
    pub(super) fn commit_observed(&mut self) {
        for at in 0..self.changed.len() {
            let slot = self.changed[at] as usize;
            self.observed[slot] = self.bits(slot);
        }
    }

    /// This frame's t′ per slot (`NO_STATE` where the game handed over
    /// none), and the slots whose t′ moved since the depth live tiles were
    /// last written: the live tiles' mover-state trigger. Peeking marks
    /// nothing; the caller commits with [`Self::commit_depth_changes`] once
    /// every write the change implies is staged and every reference move
    /// succeeded, so a failure keeps the change for the next frame. The
    /// tile writes go on the frame's upload batch; on a streamed level the
    /// reference texels flush on their own immediate submission, after the
    /// staged batch.
    pub(super) fn depth_changes(&mut self) -> (&[f32], &[u32]) {
        self.depth_changed.clear();
        for slot in 0..self.values.len() {
            if self.depth_written[slot] != self.bits(slot) {
                self.depth_changed.push(slot as u32);
            }
        }
        (&self.values, &self.depth_changed)
    }

    /// The live tiles were written from the slots [`Self::depth_changes`]
    /// last reported.
    pub(super) fn commit_depth_changes(&mut self) {
        for at in 0..self.depth_changed.len() {
            let slot = self.depth_changed[at] as usize;
            self.depth_written[slot] = self.bits(slot);
        }
    }

    /// Some reference move planned from the slots [`Self::depth_changes`]
    /// last reported failed, after others may have landed: mark those slots
    /// written from no t′ at all, so the next frame reports them changed
    /// whatever t′ it hands over, a return to the old t′ or to no state
    /// included, and replans their probes from the current t′.
    pub(super) fn forget_depth_changes(&mut self) {
        for at in 0..self.depth_changed.len() {
            let slot = self.depth_changed[at] as usize;
            self.depth_written[slot] = DEPTH_UNWRITTEN;
        }
    }

    #[cfg(test)]
    pub(super) fn values(&self) -> &[f32] {
        &self.values
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block of two-pose movers `ids`.
    fn block_of(ids: &[u32]) -> MoverTBlock {
        MoverTBlock::new(ids, &MoverKnots::termini(ids.len()))
    }

    fn state(mover_id: u32, t: f32) -> MoverTravel {
        MoverTravel { mover_id, t }
    }

    #[test]
    fn unnamed_movers_are_ignored_and_absent_ones_read_as_no_state() {
        let mut block = block_of(&[3, 9]);
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
        let mut block = block_of(&[1]);
        assert!(
            block.take_upload().is_none(),
            "the initial block is resident"
        );
        block.set_states(&[state(1, 0.5)]);
        let mut slot = 0.5f32.to_le_bytes().to_vec();
        slot.extend_from_slice(&0u32.to_le_bytes());
        assert_eq!(block.take_upload(), Some(&slot[..]), "t′, then its bracket");
        block.set_states(&[state(1, 0.5)]);
        assert!(block.take_upload().is_none());
    }

    /// Two frames between one pair of ticks hand over the same t; the
    /// second observes no change. An unplanned change is never lost,
    /// since observation and commit are separate from upload.
    #[test]
    fn observation_reports_each_change_once() {
        let mut block = block_of(&[1, 2]);
        block.set_states(&[state(1, 0.5), state(2, 0.0)]);
        assert_eq!(block.observe_changes(), &[0, 1]);
        block.commit_observed();
        block.set_states(&[state(1, 0.5), state(2, 0.0)]);
        assert!(block.observe_changes().is_empty());
        block.set_states(&[state(2, 0.0)]);
        assert_eq!(
            block.observe_changes(),
            &[0],
            "dropping a state is a change"
        );
    }

    /// A plan that fails after peeking keeps the change: the slots stay
    /// reported until a successful plan commits them, for the planner and
    /// the live tiles alike.
    #[test]
    fn a_change_is_kept_until_a_successful_plan_commits_it() {
        let mut block = block_of(&[1, 2]);
        block.set_states(&[state(1, 0.5)]);
        assert_eq!(block.observe_changes(), &[0]);
        assert_eq!(block.depth_changes().1, &[0]);
        // Both plans failed: nothing committed.
        block.set_states(&[state(1, 0.5)]);
        assert_eq!(
            block.observe_changes(),
            &[0],
            "the planner's change is kept"
        );
        assert_eq!(
            block.depth_changes().1,
            &[0],
            "the live tiles' change is kept"
        );
        block.commit_observed();
        block.commit_depth_changes();
        assert!(block.observe_changes().is_empty());
        assert!(block.depth_changes().1.is_empty());
    }

    /// A forgotten depth change is reported again whatever t′ the next frame
    /// hands over, the written one or no state included; only the slots that
    /// frame reported are forgotten.
    #[test]
    fn a_forgotten_depth_change_is_reported_whatever_t_comes_next() {
        let mut block = block_of(&[1, 2]);
        block.set_states(&[state(2, 1.0)]);
        let _ = block.depth_changes();
        block.commit_depth_changes();
        block.set_states(&[state(1, 0.5), state(2, 1.0)]);
        assert_eq!(block.depth_changes().1, &[0]);
        block.forget_depth_changes();
        // Back to no state, the bits the slot was last written from.
        block.set_states(&[state(2, 1.0)]);
        assert_eq!(block.depth_changes().1, &[0], "no state is a change now");
        block.commit_depth_changes();
        block.set_states(&[state(2, 1.0)]);
        assert!(block.depth_changes().1.is_empty());
    }

    /// −0.0 and +0.0 are the same position: handing over one after the
    /// other is no change, and the block holds +0.0.
    #[test]
    fn negative_zero_is_the_same_position_as_zero() {
        let mut block = block_of(&[1]);
        block.set_states(&[state(1, 0.0)]);
        let _ = block.observe_changes();
        block.commit_observed();
        let _ = block.depth_changes();
        block.commit_depth_changes();
        let _ = block.take_upload();
        block.set_states(&[state(1, -0.0)]);
        assert_eq!(block.values()[0].to_bits(), 0.0f32.to_bits());
        assert!(block.observe_changes().is_empty());
        assert!(block.depth_changes().1.is_empty());
        assert!(block.take_upload().is_none());
    }

    /// One frame's list names each mover at most once.
    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "each mover at most once")]
    fn a_mover_handed_over_twice_in_one_frame_is_a_bug() {
        let mut block = block_of(&[1]);
        block.set_states(&[state(1, 0.25), state(1, 0.5)]);
    }

    /// On the legacy compose, each pass's trigger holds until that
    /// pass encodes, whatever the other pass did.
    #[test]
    fn the_legacy_trigger_holds_until_an_encode_commits_it() {
        let mut block = block_of(&[7]);
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

    /// The live tiles' trigger is its own: each change reaches it once,
    /// whatever the planner and legacy passes observed, and a t′ the game
    /// hands over unchanged (a ramp holding it at 0 or 1) is no change.
    #[test]
    fn depth_changes_report_each_t_change_once_on_their_own_trigger() {
        let mut block = block_of(&[1, 2]);
        let take = |block: &mut MoverTBlock| {
            let (values, changed) = block.depth_changes();
            let taken = (values.to_vec(), changed.to_vec());
            block.commit_depth_changes();
            taken
        };
        assert!(take(&mut block).1.is_empty(), "no state yet");
        block.set_states(&[state(1, 0.0), state(2, 1.0)]);
        let _ = block.observe_changes();
        block.commit_observed();
        assert_eq!(take(&mut block).1, [0, 1], "the first hand-over");
        block.set_states(&[state(1, 0.0), state(2, 1.0)]);
        assert!(take(&mut block).1.is_empty());
        block.set_states(&[state(2, 0.5), state(1, 0.0)]);
        assert_eq!(take(&mut block), (vec![0.0, 0.5], vec![1]));
        block.set_states(&[state(2, 0.5)]);
        assert_eq!(take(&mut block).1, [0], "losing a state is a change");
    }

    /// On the legacy compose, a mover whose entries feed only one pass
    /// never recomposes the other.
    #[test]
    fn a_mover_recomposes_only_the_passes_its_entries_feed() {
        let mut block = block_of(&[3, 9]);
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
        let mut block = block_of(&[1, 2]);
        let capacities = |block: &MoverTBlock| {
            (
                block.values.capacity(),
                block.bytes.capacity(),
                block.changed.capacity(),
                block.depth_changed.capacity(),
            )
        };
        let before = capacities(&block);
        for frame in 0..8 {
            let t = frame as f32 / 8.0;
            block.set_states(&[state(1, t), state(2, 1.0 - t)]);
            let _ = block.take_upload();
            let _ = block.observe_changes();
            block.commit_observed();
            let _ = block.changed_since_composed(LegacyPass::Indirect);
            block.commit_composed(LegacyPass::Indirect);
            let _ = block.depth_changes();
            block.commit_depth_changes();
        }
        assert_eq!(capacities(&block), before);
    }
}
