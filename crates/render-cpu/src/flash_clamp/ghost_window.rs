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

/// One counted change: the edge trackers (or red) it counts for, whether it
/// spends the budget, and the presented time since something last joined it
/// or a member's swing last reached further.
#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    members: u64,
    class: SlotClass,
    age: f32,
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
/// presented-frame time, in the order they were counted.
///
/// A transition of an edge tracker joins the latest slot of its class that
/// holds no red if the tracker is not in it and neither the tracker nor any
/// member of that slot has counted in a later slot: one change of what draws,
/// crossing the threshold at different edge points on different frames,
/// counts once. A join restarts the slot's age, so everything it holds stays
/// counted a full second, and so does a member's swing reaching further: a
/// point between edge samples may cross the flash threshold only on that
/// later frame. A red transition always spends a slot of its own.
///
/// A tracker's transitions therefore sit in slots of their own, so no edge
/// point counts more transitions in a second than the window holds slots
/// without red. The budget keeps that at six: a budgeted slot opens only with
/// room for it and one more, and over budget only the exempt ray (monotone at
/// every edge point) or a change joining existing slots moves the look. The
/// slot that one monotone run opens is the one every later transition of that
/// run joins. A swing reaching further after its slot aged out counts like a
/// transition; over budget that is the exempt ray's, and it joins the latest
/// edge slot, which is then exempt.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct GhostWindow {
    slots: [Slot; GHOST_WINDOW_SLOTS],
    count: usize,
}

impl GhostWindow {
    /// Age the window by a frame's presented time.
    pub(super) fn age(&mut self, dt: f32) {
        let mut kept = 0;
        for i in 0..self.count {
            let age = self.slots[i].age + dt;
            if age < FLASH_WINDOW_SECONDS {
                self.slots[kept] = Slot {
                    age,
                    ..self.slots[i]
                };
                kept += 1;
            }
        }
        self.count = kept;
    }

    pub(super) fn is_empty(&self) -> bool {
        self.count == 0
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
        self.slots[..self.count]
            .iter()
            .rev()
            .find(|slot| slot.members & RED == 0)
            .filter(|slot| slot.class == SlotClass::Budgeted)
            .map_or(0, |slot| slot.members)
    }

    /// Count edge tracker `edge`'s transition as `class`: it joins a slot (see
    /// [`GhostWindow`]), or else opens one if `opens` allows and, for a
    /// budgeted transition, the budget has room. Returns whether it counted;
    /// a transition that did not leaves the window unchanged.
    pub(super) fn count_edge(&mut self, edge: usize, class: SlotClass, opens: bool) -> bool {
        let bit = 1u64 << edge;
        if let Some(slot) = self.joinable(bit, class) {
            self.slots[slot].members |= bit;
            self.slots[slot].age = 0.0;
            return true;
        }
        if !opens || (class == SlotClass::Budgeted && !self.has_room()) {
            return false;
        }
        self.push(Slot {
            members: bit,
            class,
            age: 0.0,
        });
        true
    }

    /// Carry edge tracker `edge`'s counted swing on to a further extreme: the
    /// latest slot holding the tracker restarts its age, so the transition
    /// stays counted a full second from the last frame it reached further.
    /// With no slot holding it any more, it counts again as `class`, as
    /// [`Self::count_edge`] does. Returns whether it stayed counted.
    pub(super) fn extend(&mut self, edge: usize, class: SlotClass, opens: bool) -> bool {
        let bit = 1u64 << edge;
        if let Some(slot) = self.slots[..self.count]
            .iter_mut()
            .rev()
            .find(|slot| slot.members & bit != 0)
        {
            slot.age = 0.0;
            return true;
        }
        self.count_edge(edge, class, opens)
    }

    /// The slot a transition of the tracker at `bit` in `class` joins: the
    /// latest slot of that class holding no red, while the tracker is not in
    /// it and neither it nor that slot's members have counted since.
    fn joinable(&self, bit: u64, class: SlotClass) -> Option<usize> {
        let slot = (0..self.count).rev().find(|&slot| {
            let candidate = self.slots[slot];
            candidate.class == class && candidate.members & RED == 0
        })?;
        let members = self.slots[slot].members;
        let joined = members | bit;
        let open = members & bit == 0
            && self.slots[slot + 1..self.count]
                .iter()
                .all(|later| later.members & joined == 0);
        open.then_some(slot)
    }

    /// A red onset is admitted only while the window holds it, its own
    /// return, and the returns earlier red onsets still owe.
    pub(super) fn red_onset_fits(&self) -> bool {
        self.count + self.red_owed() + 2 <= FLASH_MAX_TRANSITIONS
    }

    /// Count a red onset or its return, each in a slot of its own.
    pub(super) fn count_red(&mut self, onset: bool) {
        self.push(Slot {
            members: RED,
            class: if onset {
                SlotClass::Budgeted
            } else {
                SlotClass::Exempt
            },
            age: 0.0,
        });
    }

    /// Red onsets in the window whose return it does not hold yet. A return
    /// with no onset before it owes nothing.
    fn red_owed(&self) -> usize {
        let mut owed = 0usize;
        for slot in &self.slots[..self.count] {
            if slot.members & RED == 0 {
                continue;
            }
            match slot.class {
                SlotClass::Budgeted => owed += 1,
                SlotClass::Exempt => owed = owed.saturating_sub(1),
            }
        }
        owed
    }

    /// A new budgeted slot leaves room for one more.
    fn has_room(&self) -> bool {
        self.count + 2 <= FLASH_MAX_TRANSITIONS
    }

    /// Slots holding edge transitions.
    pub(super) fn edge_slots(&self) -> usize {
        self.slots[..self.count]
            .iter()
            .filter(|slot| slot.members & RED == 0)
            .count()
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.count
    }

    fn push(&mut self, slot: Slot) {
        debug_assert!(
            self.count < GHOST_WINDOW_SLOTS,
            "a ghost's flash window never holds more than {GHOST_WINDOW_SLOTS} slots"
        );
        if self.count < GHOST_WINDOW_SLOTS {
            self.slots[self.count] = slot;
            self.count += 1;
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
