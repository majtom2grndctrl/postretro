// The `player.assistProgress` display row for each role, applied at the frozen
// UI boundary in `App::build_ui_display_slot_snapshot`.
// See: context/lib/ui.md §3 · context/lib/networking.md §What gates, and what replicates instead

use std::collections::HashMap;

use postretro_entities::SlotValue;

use super::ClientAssist;
use crate::sim::assist::{Row6Record, increment, reported};

const PROGRESS_NAME: &str = "player.assistProgress";

/// The connected client's step state as the display table reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientDisplay {
    /// Abstaining: the authoritative slot, no alpha term.
    Abstaining,
    /// `assist` not held: 0. Host samples of a released hold reach only the
    /// store.
    Released,
    /// Following a drop: 0, no alpha term.
    Following,
    /// The last forward-predicted tick ran row 6 without reaching U: the
    /// prediction plus the alpha term.
    Advanced,
    /// Any other held state: the prediction.
    Held,
}

/// One frame's assist display: the client's prediction, step state and the
/// increment its last forward-predicted tick advanced by, or a listen host's
/// or single player's row-6 record; and the frame's render alpha. The alpha
/// term always uses the last tick's increment, never a pair table applied
/// since.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum UiProjection {
    Client {
        prediction: u64,
        state: ClientDisplay,
        increment: u64,
        alpha: f32,
    },
    Host {
        row6: Row6Record,
        alpha: f32,
    },
}

impl UiProjection {
    /// Replace `player.assistProgress` in the UI display map. Never touches
    /// the store.
    pub(crate) fn project(self, values: &mut HashMap<String, SlotValue>) {
        let value = match self {
            UiProjection::Client {
                prediction,
                state,
                increment,
                alpha,
            } => match state {
                ClientDisplay::Abstaining => return,
                ClientDisplay::Released | ClientDisplay::Following => 0.0,
                ClientDisplay::Held => reported(prediction),
                ClientDisplay::Advanced => {
                    (reported(prediction) + alpha.clamp(0.0, 1.0) * reported(increment)).min(1.0)
                }
            },
            UiProjection::Host { row6, alpha } => {
                if !row6.advanced_without_completing {
                    return;
                }
                let Some(SlotValue::Number(slot)) = values.get(PROGRESS_NAME) else {
                    return;
                };
                let term = alpha.clamp(0.0, 1.0) * reported(increment(row6.required_ticks));
                (*slot + term).min(1.0)
            }
        };
        let value = SlotValue::Number(value);
        if let Some(slot) = values.get_mut(PROGRESS_NAME) {
            *slot = value;
        } else {
            values.insert(PROGRESS_NAME.into(), value);
        }
    }
}

impl ClientAssist {
    /// The connected client's projection for this frame.
    pub(crate) fn client_projection(&self, alpha: f32) -> UiProjection {
        let state = if self.abstaining {
            ClientDisplay::Abstaining
        } else if !self.held {
            ClientDisplay::Released
        } else if self.following {
            ClientDisplay::Following
        } else if self.advanced {
            ClientDisplay::Advanced
        } else {
            ClientDisplay::Held
        };
        UiProjection::Client {
            prediction: self.progress,
            state,
            increment: self.last_increment,
            alpha,
        }
    }
}
