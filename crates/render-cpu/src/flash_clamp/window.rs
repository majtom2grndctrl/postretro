// Flash window: the transitions one limited source counted in the last second,
// the budget an onset must fit, and how a ghost's luminance parts share a slot.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter)

use super::value::Tracker;

const FLASH_MAX_TRANSITIONS: usize = 6;
const FLASH_WINDOW_SECONDS: f32 = 1.0;
/// Room for every transition a window can hold: the six an admitted onset
/// reserves, plus one more per part (a joined onset, or a return whose onset
/// predates the window). See [`FlashWindow`].
const WINDOW_SLOTS: usize = FLASH_MAX_TRANSITIONS + PARTS;

/// Which tracked part of a value a window slot's transition belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Part {
    Level,
    Red,
    /// A ghost's drawn brightness over black: presented opacity times its
    /// tint's luminance.
    Brightness,
    /// How far a white ghost darkens a white background: presented opacity
    /// times one minus its tint's luminance.
    Darkening,
}

const PARTS: usize = 4;

impl Part {
    /// This part's bit in a slot's part set.
    pub(super) fn bit(self) -> u8 {
        1u8 << self as u8
    }

    /// Whether this part's transitions merge with another part's on a merging
    /// window. Opacity, drawn brightness and darkening are views of one drawn
    /// change. Redness is a separate criterion, so a red transition always
    /// spends a slot of its own, and no other part joins it.
    fn merges(self) -> bool {
        !matches!(self, Part::Red)
    }
}

/// One counted transition: the parts it counts for, and whether it was an
/// onset.
#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    parts: u8,
    onset: bool,
}

/// Transitions counted in the last second, aged by presented-frame time, in
/// the order they were counted. Each slot records the parts it counts for and
/// whether it was an onset. Every part limited on one window spends the same
/// budget of six transitions: an onset is admitted only with room for itself,
/// its return, and the returns every part's earlier onsets still owe.
///
/// A merging window (a ghost's look) lets a luminance part's transition join
/// the most recent slot of the same direction that holds no red: one change
/// in what draws, whose parts cross the threshold on different frames, counts
/// once. It joins only while that slot's parts have counted nothing since and
/// none of them is moving back this frame, and a part joins a slot once. A
/// join restarts the slot's age, so every transition it holds stays counted a
/// full second. A screen channel's window never merges.
///
/// Each part's transitions alternate and sit in slots of their own, so no part
/// counts four onsets in a second. The window holds at most six transitions
/// once an onset is admitted, and each part can add at most one more before
/// the next admission (an onset joining a slot, or a return whose onset
/// predates the window), so it never needs more than [`WINDOW_SLOTS`].
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FlashWindow {
    ages: [f32; WINDOW_SLOTS],
    kinds: [Slot; WINDOW_SLOTS],
    count: usize,
    /// Parts rising this frame, then parts falling: a slot holding one is not
    /// joined by a transition the other way.
    rising: u8,
    falling: u8,
    /// Whether luminance parts' transitions may share a slot.
    merges: bool,
}

impl FlashWindow {
    /// An empty window whose luminance parts' transitions merge: a ghost's.
    pub(super) fn merging() -> Self {
        Self {
            merges: true,
            ..Self::default()
        }
    }

    /// Age the window by a frame's time and start that frame, with no part
    /// moving yet.
    pub(super) fn age(&mut self, dt: f32) {
        let mut kept = 0;
        for i in 0..self.count {
            let age = self.ages[i] + dt;
            if age < FLASH_WINDOW_SECONDS {
                self.ages[kept] = age;
                self.kinds[kept] = self.kinds[i];
                kept += 1;
            }
        }
        self.count = kept;
        self.rising = 0;
        self.falling = 0;
    }

    /// Record which way `part` moves this frame: the sign of `change`.
    pub(super) fn moving(&mut self, part: Part, change: f32) {
        let bit = part.bit();
        self.rising &= !bit;
        self.falling &= !bit;
        if change > 0.0 {
            self.rising |= bit;
        } else if change < 0.0 {
            self.falling |= bit;
        }
    }

    /// The slot a transition of `part` in this direction joins, on a merging
    /// window: the most recent slot of that direction holding no red, if
    /// `part` is not in it, nothing in it or `part` has counted since, and
    /// nothing in it is moving the other way this frame.
    fn joinable(&self, part: Part, onset: bool) -> Option<usize> {
        if !self.merges || !part.merges() {
            return None;
        }
        let slot = (0..self.count).rev().find(|&slot| {
            let kind = self.kinds[slot];
            kind.onset == onset && kind.parts & Part::Red.bit() == 0
        })?;
        let parts = self.kinds[slot].parts;
        let moving_back = if onset { self.falling } else { self.rising };
        let joined = parts | part.bit();
        let open = parts & part.bit() == 0
            && parts & moving_back == 0
            && self.kinds[slot + 1..self.count]
                .iter()
                .all(|later| later.parts & joined == 0);
        open.then_some(slot)
    }

    fn push(&mut self, part: Part, onset: bool) {
        if let Some(slot) = self.joinable(part, onset) {
            self.kinds[slot].parts |= part.bit();
            self.ages[slot] = 0.0;
            return;
        }
        debug_assert!(
            self.count < WINDOW_SLOTS,
            "the flash window never holds more than {WINDOW_SLOTS} transitions"
        );
        if self.count < WINDOW_SLOTS {
            self.ages[self.count] = 0.0;
            self.kinds[self.count] = Slot {
                parts: part.bit(),
                onset,
            };
            self.count += 1;
        }
    }

    /// Returns the window still owes: onsets in it whose return it does not
    /// hold yet, counted per part across every part. A return with no onset
    /// of its part before it owes nothing. A slot counting several parts
    /// opens or settles each of them.
    fn owed_returns(&self) -> usize {
        let mut open = [0usize; PARTS];
        for kind in &self.kinds[..self.count] {
            for (part, owed) in open.iter_mut().enumerate() {
                if kind.parts & (1u8 << part) == 0 {
                    continue;
                }
                if kind.onset {
                    *owed += 1;
                } else {
                    *owed = owed.saturating_sub(1);
                }
            }
        }
        open.iter().sum()
    }

    /// An onset is admitted only while the window holds it, the returns
    /// earlier onsets of any part still owe, and its own return.
    fn onset_fits(&self) -> bool {
        self.count + self.owed_returns() + 2 <= FLASH_MAX_TRANSITIONS
    }

    pub(super) fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// Decide one tracked part's next presented level. Onsets (rises) are admitted
/// only while the window holds them, their return, and the returns earlier
/// onsets still owe; over budget the part holds. Returns always pass. On a
/// merging window, an onset joining another part's slot is that admitted
/// transition.
pub(super) fn decide(
    tracker: &mut Tracker,
    window: &mut FlashWindow,
    part: Part,
    target: f32,
    threshold: f32,
    deadband: f32,
    counts: bool,
) -> f32 {
    let (excursion, _) = tracker.excursion(target, threshold);
    let sign: i8 = if excursion > 0.0 { 1 } else { -1 };
    let onset = sign > 0;
    let fits = window.onset_fits() || (onset && window.joinable(part, true).is_some());
    if fits {
        tracker.suppressing = false;
    }
    if counts && sign != tracker.last_sign && excursion.abs() >= threshold {
        if !onset || fits {
            window.push(part, onset);
            tracker.last_sign = sign;
        } else {
            tracker.suppressing = true;
        }
    } else if counts && onset && !fits && sign != tracker.last_sign && excursion >= deadband {
        tracker.suppressing = true;
    }
    if tracker.suppressing && target > tracker.out {
        tracker.out
    } else {
        target
    }
}

#[cfg(test)]
#[path = "window_tests.rs"]
mod tests;
