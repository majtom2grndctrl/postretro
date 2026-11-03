//! The host HUD publisher for the assist progress slots.
//! See: context/lib/scripting.md §5 (Per-player engine slots) · §12 (Assist)
//!
//! Host and single player only: `tick_for_role_and_report_sampled_weapon`
//! never reaches this on a connected client, whose component carries no
//! accumulator output, so a committed host sample survives the next publish.

use super::*;

impl PlayerHudStatePublisher {
    /// Publish the own pawn's `player.assistProgress` and
    /// `player.assistReceivedProgress` through the shared per-pawn lookup.
    /// With no own pawn both read 0, so no bar carries into the next level.
    pub(super) fn publish_assist_progress(&mut self) {
        let values = {
            let registry = self.ctx.registry.borrow();
            let pawn = registry.local_player_movement_pawn();
            [
                PlayerSlot::AssistProgress,
                PlayerSlot::AssistReceivedProgress,
            ]
            .map(|slot| {
                let value = pawn
                    .and_then(|pawn| player_slot_value(&registry, slot, pawn))
                    .unwrap_or(SlotValue::Number(0.0));
                (slot, value)
            })
        };
        for (slot, value) in values {
            self.write_hud_slot(slot.name(), value);
        }
    }
}

#[cfg(test)]
#[path = "ui_proxy_assist_tests.rs"]
mod tests;
