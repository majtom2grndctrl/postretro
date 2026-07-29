// Per-client connection slot state and lifecycle transitions.
// See: context/lib/networking.md

use std::collections::HashMap;

use renet::ClientId;

use crate::wire::HoldingCause;

/// How a connection ended. This is distinct from `ClosingCause`, which explains
/// why immutable admission was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseCause {
    Disconnect,
    Timeout,
    /// Reserved for a future graceful host migration/departure path.
    HostInitiatedLeave,
}

/// The lifecycle state of one connection slot. A closed slot is terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    Pending,
    Admitted,
    Participating,
    Closed { cause: CloseCause },
}

/// Effects derived from a state transition. Any exit from `Participating` emits
/// cleanup, and any entry emits `Participating` for registration/pawn spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotEvent {
    Participating {
        client_id: ClientId,
    },
    Demoted {
        client_id: ClientId,
        cause: HoldingCause,
    },
    Closed {
        client_id: ClientId,
        cause: CloseCause,
    },
}

#[derive(Debug, Default)]
pub struct SlotTable {
    slots: HashMap<ClientId, SlotState>,
}

impl SlotTable {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed a pending slot exactly once. A closed id is never resurrected.
    pub fn on_connect(&mut self, client_id: ClientId) {
        self.slots.entry(client_id).or_insert(SlotState::Pending);
    }

    /// Owns every state mutation and derives the associated lifecycle edge.
    fn transition(
        &mut self,
        client_id: ClientId,
        next: SlotState,
        holding: Option<HoldingCause>,
    ) -> Option<SlotEvent> {
        let previous = self.slots.get(&client_id).copied();
        if matches!(previous, Some(SlotState::Closed { .. })) {
            return None;
        }
        if previous == Some(next) {
            return None;
        }

        // An unknown close must remain recorded so stale packets are refused, but
        // has no participating pawn to clean up.
        if previous.is_none() && matches!(next, SlotState::Closed { .. }) {
            self.slots.insert(client_id, next);
            return None;
        }

        self.slots.insert(client_id, next);
        match (previous, next) {
            (Some(SlotState::Participating), SlotState::Admitted) => Some(SlotEvent::Demoted {
                client_id,
                cause: holding.expect("participating -> admitted requires a holding cause"),
            }),
            (Some(SlotState::Participating), SlotState::Closed { cause }) => {
                Some(SlotEvent::Closed { client_id, cause })
            }
            (_, SlotState::Participating) => Some(SlotEvent::Participating { client_id }),
            _ => None,
        }
    }

    #[must_use]
    pub fn admit(&mut self, client_id: ClientId) -> Option<SlotEvent> {
        self.transition(client_id, SlotState::Admitted, None)
    }

    #[must_use]
    pub fn participate(&mut self, client_id: ClientId) -> Option<SlotEvent> {
        self.transition(client_id, SlotState::Participating, None)
    }

    #[must_use]
    pub fn demote(&mut self, client_id: ClientId, cause: HoldingCause) -> Option<SlotEvent> {
        self.transition(client_id, SlotState::Admitted, Some(cause))
    }

    #[must_use]
    pub fn close(&mut self, client_id: ClientId, cause: CloseCause) -> Option<SlotEvent> {
        self.transition(client_id, SlotState::Closed { cause }, None)
    }

    #[must_use]
    pub fn state(&self, client_id: ClientId) -> Option<SlotState> {
        self.slots.get(&client_id).copied()
    }

    #[must_use]
    pub fn is_participating(&self, client_id: ClientId) -> bool {
        matches!(self.state(client_id), Some(SlotState::Participating))
    }

    #[deprecated(note = "use is_participating")]
    #[must_use]
    pub fn is_accepted(&self, client_id: ClientId) -> bool {
        self.is_participating(client_id)
    }

    #[must_use]
    pub fn is_closed(&self, client_id: ClientId) -> bool {
        matches!(self.state(client_id), Some(SlotState::Closed { .. }))
    }

    #[must_use]
    pub fn participating_clients(&self) -> Vec<ClientId> {
        self.slots
            .iter()
            .filter_map(|(id, state)| matches!(state, SlotState::Participating).then_some(*id))
            .collect()
    }

    #[deprecated(note = "use participating_clients")]
    #[must_use]
    pub fn accepted_clients(&self) -> Vec<ClientId> {
        self.participating_clients()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT: ClientId = 1;

    fn holding() -> HoldingCause {
        HoldingCause::HostLevelAbsent
    }

    #[test]
    fn transitions_derive_entry_and_each_exit_once() {
        let mut slots = SlotTable::new();
        slots.on_connect(CLIENT);
        assert_eq!(slots.admit(CLIENT), None);
        assert_eq!(
            slots.participate(CLIENT),
            Some(SlotEvent::Participating { client_id: CLIENT })
        );
        assert_eq!(
            slots.demote(CLIENT, holding()),
            Some(SlotEvent::Demoted {
                client_id: CLIENT,
                cause: holding()
            })
        );
        assert_eq!(slots.close(CLIENT, CloseCause::Timeout), None);

        assert_eq!(slots.participate(CLIENT), None, "closed is terminal");
        assert_eq!(
            slots.close(9, CloseCause::Disconnect),
            None,
            "unknown close is recorded without cleanup"
        );
    }

    #[test]
    fn repromotion_emits_a_fresh_participating_event() {
        let mut slots = SlotTable::new();
        slots.on_connect(CLIENT);
        let _ = slots.admit(CLIENT);
        let _ = slots.participate(CLIENT);
        let _ = slots.demote(CLIENT, holding());
        assert_eq!(
            slots.participate(CLIENT),
            Some(SlotEvent::Participating { client_id: CLIENT })
        );
    }
}
