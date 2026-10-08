// Render-rate to fixed-tick carry of gameplay press edges.
// See: context/lib/input.md §2 (Weapon edges)

use std::collections::HashSet;

use super::activation::ActivationInputCapture;
use super::snapshot::ActionSnapshot;
use super::types::{Action, ButtonState};
use super::wieldable_selection::WieldableSelection;

/// Carries button press edges across render frames until fixed game logic runs.
///
/// `InputSystem::snapshot()` still advances at render rate so per-frame
/// consumers see fresh state, but fixed-tick gameplay must not miss a one-frame
/// `Pressed` edge on frames where the accumulator produces zero ticks.
#[derive(Debug, Default)]
pub struct GameplayInputLatch {
    pub activation: ActivationInputCapture,
    pressed_buttons: HashSet<Action>,
    wieldable_selection: WieldableSelection,
}

impl GameplayInputLatch {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.activation.suspend();
        self.pressed_buttons.clear();
        self.wieldable_selection.clear();
    }

    pub fn wieldable_selection_mut(&mut self) -> &mut WieldableSelection {
        &mut self.wieldable_selection
    }

    pub fn wieldable_selection(&self) -> &WieldableSelection {
        &self.wieldable_selection
    }

    pub fn snapshot_for_ticks(
        &mut self,
        frame_snapshot: &ActionSnapshot,
        ticks: u32,
    ) -> Option<ActionSnapshot> {
        self.activation.observe(frame_snapshot);
        for (&action, &state) in &frame_snapshot.button_states {
            if state == ButtonState::Pressed {
                self.pressed_buttons.insert(action);
            }
        }

        if ticks == 0 {
            return None;
        }

        let mut gameplay_snapshot = frame_snapshot.clone();
        for action in self.pressed_buttons.drain() {
            gameplay_snapshot
                .button_states
                .insert(action, ButtonState::Pressed);
        }

        Some(gameplay_snapshot)
    }
}
