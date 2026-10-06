// Per-frame action-state snapshot: the input subsystem's one output.
// See: context/lib/input.md §3

use std::collections::HashMap;

use super::types::{Action, AxisValue, ButtonState};

/// Read-only snapshot of all action states for a single frame.
/// Game logic consumes this; nothing writes back to input mid-frame.
#[derive(Debug, Clone)]
pub struct ActionSnapshot {
    pub(super) button_states: HashMap<Action, ButtonState>,
    pub(super) axis_values: HashMap<Action, Vec<AxisValue>>,
    /// Discrete wheel notches are not button states: several can arrive in one
    /// frame, while a wheel event has no release edge.
    pub(super) notch_counts: HashMap<Action, u32>,
}

impl ActionSnapshot {
    /// Snapshot with every action inactive. Capturing UI uses this to keep fixed
    /// simulation ticks running while player controls are gated.
    pub fn neutral() -> Self {
        Self {
            button_states: HashMap::new(),
            axis_values: HashMap::new(),
            notch_counts: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_button_state(action: Action, state: ButtonState) -> Self {
        let mut snapshot = Self::neutral();
        snapshot.button_states.insert(action, state);
        snapshot
    }

    /// Query the button state for an action. Returns Inactive if unbound.
    pub fn button(&self, action: Action) -> ButtonState {
        self.button_states
            .get(&action)
            .copied()
            .unwrap_or(ButtonState::Inactive)
    }

    /// Query axis values for an action. Returns empty slice if no input active.
    /// Multiple values indicate additive sources (displacement + velocity).
    pub fn axis(&self, action: Action) -> &[AxisValue] {
        self.axis_values
            .get(&action)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Convenience: sum of all axis values for an action, regardless of source.
    /// Useful when you don't need to distinguish displacement from velocity.
    pub fn axis_value(&self, action: Action) -> f32 {
        self.axis(action).iter().map(|v| v.value).sum()
    }

    /// Number of discrete wheel notches bound to `action` during this frame.
    pub fn notch_count(&self, action: Action) -> u32 {
        self.notch_counts.get(&action).copied().unwrap_or(0)
    }

    #[cfg(test)]
    pub(crate) fn with_notch_counts<const N: usize>(counts: [(Action, u32); N]) -> Self {
        let mut snapshot = Self::neutral();
        snapshot.notch_counts.extend(counts);
        snapshot
    }
}
