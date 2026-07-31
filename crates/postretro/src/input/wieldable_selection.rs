// Local cursor, dwell, and declaration state for wieldable selection.
// See: context/lib/input.md

use super::{Action, ActionSnapshot, ButtonState};

/// Frame-local policy resolved by the App before it reaches the selector.
/// Preferences and mod policy stop here; simulation receives only a slot index
/// after [`WieldableSelection::take_pending_commit`] accepts it.
#[derive(Debug, Clone, Copy)]
pub struct WieldableSelectionPolicy {
    pub commit_on_direct_select: bool,
    pub cycle_dwell_ms: f32,
}

#[derive(Debug, Clone, Copy)]
struct PendingCommit {
    slot: usize,
}

/// Input-layer state for local weapon selection. This is intentionally separate
/// from `Inventory`: occupancy is sampled into this state at frame rate, while
/// the inventory itself remains fixed-tick simulation state.
#[derive(Debug, Default)]
pub struct WieldableSelection {
    cursor_slot: Option<usize>,
    dwell_remaining_ms: Option<f32>,
    pending_commit: Option<PendingCommit>,
    last_weapon_slot: Option<usize>,
}

impl WieldableSelection {
    /// Clear every local selection latch. This is called wherever the gameplay
    /// input latch clears and on level unload so no local index crosses a level
    /// boundary or a capturing-modal transition.
    pub fn clear(&mut self) {
        self.cursor_slot = None;
        self.dwell_remaining_ms = None;
        self.pending_commit = None;
        self.last_weapon_slot = None;
    }

    /// Current local cursor for the HUD's pending-weapon projection.
    pub const fn cursor_slot(&self) -> Option<usize> {
        self.cursor_slot
    }

    /// Apply one rendered frame of direct-select, wheel, and last-weapon input.
    /// `occupied` is a frame-rate snapshot of the local pawn inventory, never a
    /// simulation input. A newly moved cursor does not spend this frame's delta:
    /// otherwise a low frame rate could expire a fresh dwell once per notch.
    pub fn advance_frame(
        &mut self,
        snapshot: &ActionSnapshot,
        occupied: &[bool],
        active_slot: Option<usize>,
        policy: WieldableSelectionPolicy,
        elapsed_ms: f32,
    ) {
        let Some(active_slot) = active_slot.filter(|&slot| is_occupied(occupied, slot)) else {
            self.clear();
            return;
        };

        self.reconcile_occupancy(occupied, active_slot);

        let mut cursor_moved = false;
        if let Some(slot) = direct_select_slot(snapshot)
            && slot != active_slot
            && is_occupied(occupied, slot)
        {
            self.cursor_slot = Some(slot);
            self.dwell_remaining_ms = None;
            cursor_moved = true;
            if policy.commit_on_direct_select {
                self.queue_commit(slot, occupied, active_slot);
            }
        }

        if matches!(
            snapshot.button(Action::ToggleLastWieldable),
            ButtonState::Pressed
        ) && let Some(slot) = self.last_weapon_slot
            && slot != active_slot
            && is_occupied(occupied, slot)
        {
            self.cursor_slot = Some(slot);
            self.dwell_remaining_ms = None;
            cursor_moved = true;
            self.queue_commit(slot, occupied, active_slot);
        }

        let previous_steps = snapshot.notch_count(Action::CycleWieldablePrevious);
        let next_steps = snapshot.notch_count(Action::CycleWieldableNext);
        if previous_steps != 0 || next_steps != 0 {
            let mut slot = self.cursor_slot.unwrap_or(active_slot);
            let mut cycled = false;
            for _ in 0..previous_steps {
                let next = previous_occupied_slot(occupied, slot);
                cycled |= next != slot;
                slot = next;
            }
            for _ in 0..next_steps {
                let next = next_occupied_slot(occupied, slot);
                cycled |= next != slot;
                slot = next;
            }
            if cycled {
                self.cursor_slot = Some(slot);
                cursor_moved = true;
                self.restart_cycle_dwell(slot, occupied, active_slot, policy.cycle_dwell_ms);
            }
        }

        if !cursor_moved {
            self.advance_dwell(occupied, active_slot, elapsed_ms);
        }
    }

    /// Consume the one pending declaration for the first fixed tick of a frame.
    /// A stale/empty target is discarded rather than exposed to simulation. The
    /// next holder replaces the prior one; selection never queues declarations.
    pub fn take_pending_commit(
        &mut self,
        occupied: &[bool],
        active_slot: Option<usize>,
    ) -> Option<usize> {
        let pending = self.pending_commit.take()?;
        let active_slot = active_slot.filter(|&slot| is_occupied(occupied, slot))?;
        if pending.slot == active_slot || !is_occupied(occupied, pending.slot) {
            self.reconcile_occupancy(occupied, active_slot);
            return None;
        }
        // The previous weapon is the slot actually held when this declaration is
        // accepted, not a transient cursor target overwritten before the tick.
        self.last_weapon_slot = Some(active_slot);
        Some(pending.slot)
    }

    fn reconcile_occupancy(&mut self, occupied: &[bool], active_slot: usize) {
        if !self
            .cursor_slot
            .is_some_and(|slot| is_occupied(occupied, slot))
        {
            self.cursor_slot = Some(active_slot);
            self.dwell_remaining_ms = None;
        }
        if self
            .last_weapon_slot
            .is_some_and(|slot| !is_occupied(occupied, slot))
        {
            self.last_weapon_slot = None;
        }
    }

    fn restart_cycle_dwell(
        &mut self,
        slot: usize,
        occupied: &[bool],
        active_slot: usize,
        dwell_ms: f32,
    ) {
        if !(dwell_ms.is_finite() && dwell_ms > 0.0) {
            self.dwell_remaining_ms = None;
            self.queue_commit(slot, occupied, active_slot);
        } else {
            self.dwell_remaining_ms = Some(dwell_ms);
        }
    }

    fn advance_dwell(&mut self, occupied: &[bool], active_slot: usize, elapsed_ms: f32) {
        let Some(remaining) = self.dwell_remaining_ms else {
            return;
        };
        let elapsed_ms = elapsed_ms.is_finite().then_some(elapsed_ms).unwrap_or(0.0);
        let remaining = remaining - elapsed_ms.max(0.0);
        if remaining > 0.0 {
            self.dwell_remaining_ms = Some(remaining);
            return;
        }
        self.dwell_remaining_ms = None;
        if let Some(slot) = self.cursor_slot {
            self.queue_commit(slot, occupied, active_slot);
        }
    }

    fn queue_commit(&mut self, slot: usize, occupied: &[bool], active_slot: usize) {
        if slot != active_slot && is_occupied(occupied, slot) {
            self.pending_commit = Some(PendingCommit { slot });
        }
    }
}

fn direct_select_slot(snapshot: &ActionSnapshot) -> Option<usize> {
    [
        Action::SelectWieldable1,
        Action::SelectWieldable2,
        Action::SelectWieldable3,
        Action::SelectWieldable4,
        Action::SelectWieldable5,
        Action::SelectWieldable6,
        Action::SelectWieldable7,
        Action::SelectWieldable8,
        Action::SelectWieldable9,
        Action::SelectWieldable10,
    ]
    .into_iter()
    .position(|action| matches!(snapshot.button(action), ButtonState::Pressed))
}

fn is_occupied(occupied: &[bool], slot: usize) -> bool {
    occupied.get(slot).copied().unwrap_or(false)
}

fn next_occupied_slot(occupied: &[bool], slot: usize) -> usize {
    if occupied.is_empty() {
        return slot;
    }
    for offset in 1..=occupied.len() {
        let candidate = (slot + offset) % occupied.len();
        if is_occupied(occupied, candidate) {
            return candidate;
        }
    }
    slot
}

fn previous_occupied_slot(occupied: &[bool], slot: usize) -> usize {
    if occupied.is_empty() {
        return slot;
    }
    for offset in 1..=occupied.len() {
        let candidate = (slot + occupied.len() - (offset % occupied.len())) % occupied.len();
        if is_occupied(occupied, candidate) {
            return candidate;
        }
    }
    slot
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::ActionSnapshot;

    const THREE_SLOTS: [bool; 3] = [true, true, true];

    fn snapshot(action: Action) -> ActionSnapshot {
        ActionSnapshot::with_button_state(action, ButtonState::Pressed)
    }

    fn scroll_snapshot(next: u32, previous: u32) -> ActionSnapshot {
        ActionSnapshot::with_notch_counts([
            (Action::CycleWieldableNext, next),
            (Action::CycleWieldablePrevious, previous),
        ])
    }

    fn direct_policy() -> WieldableSelectionPolicy {
        WieldableSelectionPolicy {
            commit_on_direct_select: true,
            cycle_dwell_ms: 20.0,
        }
    }

    #[test]
    fn o29_two_direct_selects_before_consumption_keep_only_the_last_target() {
        let mut selection = WieldableSelection::default();
        selection.advance_frame(
            &snapshot(Action::SelectWieldable2),
            &THREE_SLOTS,
            Some(0),
            direct_policy(),
            0.0,
        );
        selection.advance_frame(
            &snapshot(Action::SelectWieldable3),
            &THREE_SLOTS,
            Some(0),
            direct_policy(),
            0.0,
        );

        assert_eq!(
            selection.take_pending_commit(&THREE_SLOTS, Some(0)),
            Some(2)
        );
        assert_eq!(selection.last_weapon_slot, Some(0));
    }

    #[test]
    fn o30_two_direct_selects_across_zero_tick_frames_discard_the_first() {
        let mut selection = WieldableSelection::default();
        selection.advance_frame(
            &snapshot(Action::SelectWieldable2),
            &THREE_SLOTS,
            Some(0),
            direct_policy(),
            16.0,
        );
        selection.advance_frame(
            &snapshot(Action::SelectWieldable3),
            &THREE_SLOTS,
            Some(0),
            direct_policy(),
            16.0,
        );

        assert_eq!(
            selection.take_pending_commit(&THREE_SLOTS, Some(0)),
            Some(2)
        );
        assert_eq!(selection.take_pending_commit(&THREE_SLOTS, Some(0)), None);
    }

    #[test]
    fn o31_multiple_scroll_notches_move_each_step_and_restart_dwell_once() {
        let mut selection = WieldableSelection::default();
        selection.advance_frame(
            &scroll_snapshot(2, 0),
            &THREE_SLOTS,
            Some(0),
            direct_policy(),
            100.0,
        );
        assert_eq!(selection.cursor_slot(), Some(2));
        assert_eq!(selection.take_pending_commit(&THREE_SLOTS, Some(0)), None);

        selection.advance_frame(
            &ActionSnapshot::neutral(),
            &THREE_SLOTS,
            Some(0),
            direct_policy(),
            20.0,
        );
        assert_eq!(
            selection.take_pending_commit(&THREE_SLOTS, Some(0)),
            Some(2)
        );
    }

    #[test]
    fn o32_long_frames_with_consecutive_scrolls_declare_only_the_last_cursor() {
        let mut selection = WieldableSelection::default();
        selection.advance_frame(
            &scroll_snapshot(1, 0),
            &THREE_SLOTS,
            Some(0),
            direct_policy(),
            100.0,
        );
        selection.advance_frame(
            &scroll_snapshot(1, 0),
            &THREE_SLOTS,
            Some(0),
            direct_policy(),
            100.0,
        );
        assert_eq!(selection.take_pending_commit(&THREE_SLOTS, Some(0)), None);

        selection.advance_frame(
            &ActionSnapshot::neutral(),
            &THREE_SLOTS,
            Some(0),
            direct_policy(),
            100.0,
        );
        assert_eq!(
            selection.take_pending_commit(&THREE_SLOTS, Some(0)),
            Some(2)
        );
    }

    #[test]
    fn o33_second_dwell_expiry_replaces_unconsumed_holder_on_zero_tick_frames() {
        let mut selection = WieldableSelection::default();
        selection.advance_frame(
            &scroll_snapshot(1, 0),
            &THREE_SLOTS,
            Some(0),
            direct_policy(),
            0.0,
        );
        selection.advance_frame(
            &ActionSnapshot::neutral(),
            &THREE_SLOTS,
            Some(0),
            direct_policy(),
            20.0,
        );
        selection.advance_frame(
            &scroll_snapshot(1, 0),
            &THREE_SLOTS,
            Some(0),
            direct_policy(),
            0.0,
        );
        selection.advance_frame(
            &ActionSnapshot::neutral(),
            &THREE_SLOTS,
            Some(0),
            direct_policy(),
            20.0,
        );

        assert_eq!(
            selection.take_pending_commit(&THREE_SLOTS, Some(0)),
            Some(2)
        );
    }

    #[test]
    fn o34_clear_discards_commit_when_a_modal_captures_gameplay() {
        let mut selection = WieldableSelection::default();
        selection.advance_frame(
            &snapshot(Action::SelectWieldable2),
            &THREE_SLOTS,
            Some(0),
            direct_policy(),
            0.0,
        );
        selection.clear();

        assert_eq!(selection.take_pending_commit(&THREE_SLOTS, Some(0)), None);
    }

    #[test]
    fn o35_clear_on_level_unload_removes_cursor_holder_and_last_weapon_memory() {
        let mut selection = WieldableSelection::default();
        selection.advance_frame(
            &snapshot(Action::SelectWieldable2),
            &THREE_SLOTS,
            Some(0),
            direct_policy(),
            0.0,
        );
        assert_eq!(
            selection.take_pending_commit(&THREE_SLOTS, Some(0)),
            Some(1)
        );
        selection.clear();

        assert_eq!(selection.cursor_slot(), None);
        assert_eq!(selection.last_weapon_slot, None);
        assert_eq!(selection.take_pending_commit(&THREE_SLOTS, Some(0)), None);
    }

    #[test]
    fn o36_single_entry_loadout_emits_no_declaration() {
        let mut selection = WieldableSelection::default();
        let occupied = [true];
        selection.advance_frame(
            &scroll_snapshot(4, 0),
            &occupied,
            Some(0),
            direct_policy(),
            100.0,
        );
        selection.advance_frame(
            &snapshot(Action::SelectWieldable1),
            &occupied,
            Some(0),
            direct_policy(),
            100.0,
        );
        selection.advance_frame(
            &snapshot(Action::ToggleLastWieldable),
            &occupied,
            Some(0),
            direct_policy(),
            100.0,
        );

        assert_eq!(selection.cursor_slot(), Some(0));
        assert_eq!(selection.take_pending_commit(&occupied, Some(0)), None);
    }

    #[test]
    fn direct_select_ignores_empty_slots_without_moving_the_cursor() {
        let mut selection = WieldableSelection::default();
        let occupied = [true, false, true];
        selection.advance_frame(
            &snapshot(Action::SelectWieldable2),
            &occupied,
            Some(0),
            direct_policy(),
            0.0,
        );

        assert_eq!(selection.cursor_slot(), Some(0));
        assert_eq!(selection.take_pending_commit(&occupied, Some(0)), None);
    }

    #[test]
    fn cycle_wraps_across_empty_slots_using_occupied_inventory_positions() {
        let mut selection = WieldableSelection::default();
        let occupied = [true, false, true];
        selection.advance_frame(
            &scroll_snapshot(2, 0),
            &occupied,
            Some(0),
            WieldableSelectionPolicy {
                commit_on_direct_select: true,
                cycle_dwell_ms: 0.0,
            },
            0.0,
        );

        assert_eq!(selection.cursor_slot(), Some(0));
        assert_eq!(selection.take_pending_commit(&occupied, Some(0)), None);
    }

    #[test]
    fn last_weapon_toggle_returns_to_the_last_held_slot_and_updates_its_memory() {
        let mut selection = WieldableSelection::default();
        selection.advance_frame(
            &snapshot(Action::SelectWieldable2),
            &THREE_SLOTS,
            Some(0),
            direct_policy(),
            0.0,
        );
        assert_eq!(
            selection.take_pending_commit(&THREE_SLOTS, Some(0)),
            Some(1)
        );

        selection.advance_frame(
            &snapshot(Action::ToggleLastWieldable),
            &THREE_SLOTS,
            Some(1),
            direct_policy(),
            0.0,
        );
        assert_eq!(
            selection.take_pending_commit(&THREE_SLOTS, Some(1)),
            Some(0)
        );
        assert_eq!(selection.last_weapon_slot, Some(1));
    }

    #[test]
    fn cursor_reads_the_input_frames_occupancy_snapshot() {
        let mut selection = WieldableSelection::default();
        let occupied = [true, false, true];
        selection.advance_frame(
            &scroll_snapshot(1, 0),
            &occupied,
            Some(0),
            direct_policy(),
            0.0,
        );

        assert_eq!(selection.cursor_slot(), Some(2));
    }
}
