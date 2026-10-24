// Ghost flash window: the transitions one ghost's look counted in the last
// second, how its edge trackers' transitions share slots, and the budget.
// See: context/lib/rendering_pipeline.md §7.8 (Photosensitivity limiter), §7.9

use super::ghost_trackers::EDGE_POINTS;
use super::value::{FLASH_MAX_TRANSITIONS, FLASH_WINDOW_SECONDS};

/// The member bit of a red transition, past every edge tracker's.
const RED: u64 = 1 << EDGE_POINTS;

/// Room for every slot a ghost's window can hold: at most six counting edge
/// transitions (see [`GhostWindow`]) and at most six red ones, whose onsets
/// are admitted only with room for their returns.
const GHOST_WINDOW_SLOTS: usize = 2 * FLASH_MAX_TRANSITIONS;

/// Whether a transition spends the budget.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum SlotClass {
    /// Any change of what the ghost draws but the exempt ray's, and a red
    /// onset. A new budgeted slot is opened only while the budget has room.
    #[default]
    Budgeted,
    /// The exempt ray (the asked opacity falling at the held tint), and a red
    /// return. Always admitted.
    Exempt,
}

/// What a ghost's look may do this frame, from what its window holds as the
/// frame starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Budget {
    /// Under budget: anything presents that opens a budgeted slot only while
    /// there is room.
    Open,
    /// Over budget, the change that opened the latest budgeted slot may carry
    /// on: the look moves only where every transition joins a slot, and that
    /// slot's members move no way but the one they counted.
    Continue,
    /// Over budget: only the exempt ray moves.
    ExemptOnly,
}

/// The transitions one ghost's look counted in the last second, aged by
/// presented-frame time, in the order they were counted. Each slot holds its
/// members (the edge trackers it counts a transition of, or red), its class,
/// and the presented time since something last joined it or a member last
/// carried its count on.
///
/// A transition of an edge tracker joins the latest slot of its class that
/// holds no red if the tracker is not in it and neither the tracker nor any
/// member of that slot has counted in a later slot: one change of what draws,
/// crossing the threshold at different edge points on different frames,
/// counts once. A join restarts the slot's age, and so does a member carrying
/// its count on ([`Self::extend`]). A red transition always spends a slot of
/// its own.
///
/// Two properties bound what any point sees (the argument is on the ghost's
/// look clamp):
///
/// - **One tracker's transitions sit in slots of their own**, in the order it
///   counted them: it joins only a slot it is not in, and only one after every
///   slot it is in.
/// - **The window never holds more than six edge slots.** A budgeted slot
///   opens only with room for it and one more, so budgeted opens stop at five
///   slots, red included. A step opens at most one slot, since every later
///   transition of the step joins it. Over budget only an exempt step opens
///   one, and from six edge slots the budget is exempt-only: the exempt ray
///   at the held luminance is monotone at every edge point, so a tracker
///   counts on it at most once and joins its latest slot. A tracker whose
///   history is unknown and that has not moved yet draws at the presented
///   look what it drew with nothing drawn, so it draws that at every lower
///   opacity of the held luminance too, and the ray moves it by f32 rounding
///   at most. Such a move counts once and joins the latest slot like any
///   other, and a later one carries that count on as a swing stopped short
///   of its peak, since a still point's neighbours move both ways; or, once
///   its swing back has counted, as a pass of its peak, which on the ray
///   restarts a slot holding it, if any, and opens none.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct GhostWindow {
    // Slot fields side by side, not as an array of slots, so the window packs
    // without padding into the per-ghost budget.
    members: [u64; GHOST_WINDOW_SLOTS],
    classes: [SlotClass; GHOST_WINDOW_SLOTS],
    ages: [f32; GHOST_WINDOW_SLOTS],
    count: u8,
}

impl GhostWindow {
    /// Age the window by a frame's presented time.
    pub(super) fn age(&mut self, dt: f32) {
        let mut kept = 0;
        for i in 0..self.len() {
            let age = self.ages[i] + dt;
            if age < FLASH_WINDOW_SECONDS {
                self.members[kept] = self.members[i];
                self.classes[kept] = self.classes[i];
                self.ages[kept] = age;
                kept += 1;
            }
        }
        self.set_len(kept);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Slots held.
    pub(super) fn len(&self) -> usize {
        usize::from(self.count)
    }

    fn set_len(&mut self, len: usize) {
        self.count = len as u8;
    }

    /// What this frame's look may do. Read once a frame, after aging.
    pub(super) fn budget(&self) -> Budget {
        if self.has_room() {
            Budget::Open
        } else if self.continuing() != 0 && self.edge_slots() < FLASH_MAX_TRANSITIONS {
            Budget::Continue
        } else {
            Budget::ExemptOnly
        }
    }

    /// The members of the latest slot holding no red, if it is budgeted: the
    /// edge trackers whose last transition is the change [`Budget::Continue`]
    /// carries on. Empty otherwise.
    pub(super) fn continuing(&self) -> u64 {
        (0..self.len())
            .rev()
            .find(|&slot| self.members[slot] & RED == 0)
            .filter(|&slot| self.classes[slot] == SlotClass::Budgeted)
            .map_or(0, |slot| self.members[slot])
    }

    /// Count edge tracker `edge`'s transition as `class`: it joins a slot (see
    /// [`GhostWindow`]), or else opens one if `opens` allows and, for a
    /// budgeted transition, the budget has room. Returns whether it counted;
    /// a transition that did not leaves the window unchanged.
    pub(super) fn count_edge(&mut self, edge: usize, class: SlotClass, opens: bool) -> bool {
        let bit = 1u64 << edge;
        if let Some(slot) = self.joinable(bit, class) {
            self.members[slot] |= bit;
            self.ages[slot] = 0.0;
            return true;
        }
        if !opens || (class == SlotClass::Budgeted && !self.has_room()) {
            return false;
        }
        self.push(bit, class);
        true
    }

    /// Carry edge tracker `edge`'s counted swing on: the latest slot holding
    /// the tracker restarts its age, so the transition stays counted a full
    /// second from this frame. With no slot holding it any more, it counts
    /// again as `class`, as [`Self::count_edge`] does. Returns whether it
    /// stayed counted.
    ///
    /// A long ramp that keeps reaching further keeps its slot for as long as
    /// it runs, and a second after: that only ever limits more, never less.
    pub(super) fn extend(&mut self, edge: usize, class: SlotClass, opens: bool) -> bool {
        self.restart_held(edge, 0) || self.count_edge(edge, class, opens)
    }

    /// Carry edge tracker `edge`'s last transition on in place, opening
    /// nothing: the latest slot holding it restarts its age. Returns `false`,
    /// restarting nothing, when no slot holds the tracker.
    pub(super) fn restart_latest(&mut self, edge: usize) -> bool {
        self.restart_held(edge, 0)
    }

    /// Carry edge tracker `edge`'s transition before its last on: the slot
    /// holding it restarts its age, so it stays counted a full second from
    /// this frame. A tracker's transitions sit in slots of their own in the
    /// order it counted them, so that is the earlier of its two latest slots.
    /// Returns `false`, restarting nothing, unless two slots hold the tracker.
    pub(super) fn restart_earlier(&mut self, edge: usize) -> bool {
        self.restart_held(edge, 1)
    }

    /// Restart the age of the slot holding edge tracker `edge` with `back`
    /// later slots holding it, and whether there is one.
    fn restart_held(&mut self, edge: usize, back: usize) -> bool {
        let bit = 1u64 << edge;
        let Some(slot) = (0..self.len())
            .rev()
            .filter(|&slot| self.members[slot] & bit != 0)
            .nth(back)
        else {
            return false;
        };
        self.ages[slot] = 0.0;
        true
    }

    /// The slot a transition of the tracker at `bit` in `class` joins: the
    /// latest slot of that class holding no red, while the tracker is not in
    /// it and neither it nor that slot's members have counted since.
    fn joinable(&self, bit: u64, class: SlotClass) -> Option<usize> {
        let slot = (0..self.len())
            .rev()
            .find(|&slot| self.classes[slot] == class && self.members[slot] & RED == 0)?;
        let members = self.members[slot];
        let joined = members | bit;
        let open = members & bit == 0
            && self.members[slot + 1..self.len()]
                .iter()
                .all(|&later| later & joined == 0);
        open.then_some(slot)
    }

    /// A red onset is admitted only while the window holds it, its own
    /// return, and the returns earlier red onsets still owe. The screen
    /// channels' `FlashWindow::onset_fits` (`value.rs`) is the same rule over
    /// their own window; a change to one belongs in both.
    pub(super) fn red_onset_fits(&self) -> bool {
        self.len() + self.red_owed() + 2 <= FLASH_MAX_TRANSITIONS
    }

    /// Count a red onset or its return, each in a slot of its own.
    pub(super) fn count_red(&mut self, onset: bool) {
        let class = if onset {
            SlotClass::Budgeted
        } else {
            SlotClass::Exempt
        };
        self.push(RED, class);
    }

    /// Red onsets in the window whose return it does not hold yet. A return
    /// with no onset before it owes nothing, as the screen channels'
    /// `FlashWindow::owed_returns` counts.
    fn red_owed(&self) -> usize {
        let mut owed = 0usize;
        for slot in 0..self.len() {
            if self.members[slot] & RED == 0 {
                continue;
            }
            match self.classes[slot] {
                SlotClass::Budgeted => owed += 1,
                SlotClass::Exempt => owed = owed.saturating_sub(1),
            }
        }
        owed
    }

    /// A new budgeted slot leaves room for one more.
    fn has_room(&self) -> bool {
        self.len() + 2 <= FLASH_MAX_TRANSITIONS
    }

    /// Slots holding edge transitions.
    pub(super) fn edge_slots(&self) -> usize {
        self.members[..self.len()]
            .iter()
            .filter(|&&members| members & RED == 0)
            .count()
    }

    fn push(&mut self, members: u64, class: SlotClass) {
        let count = self.len();
        debug_assert!(
            count < GHOST_WINDOW_SLOTS,
            "a ghost's flash window never holds more than {GHOST_WINDOW_SLOTS} slots"
        );
        if count < GHOST_WINDOW_SLOTS {
            self.members[count] = members;
            self.classes[count] = class;
            self.ages[count] = 0.0;
            self.set_len(count + 1);
        }
        debug_assert!(
            self.edge_slots() <= FLASH_MAX_TRANSITIONS,
            "a ghost's flash window never holds more than {FLASH_MAX_TRANSITIONS} edge slots"
        );
    }
}

#[cfg(test)]
#[path = "ghost_window_tests.rs"]
mod tests;
