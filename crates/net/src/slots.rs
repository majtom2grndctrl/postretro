// Per-client connection slot state and accepted->closed lifecycle transitions.
// See: context/lib/networking.md
//
// M15 Phase 2 Task 4 substrate. Phase 1 distinguished only accepted vs rejected
// at the app handshake gate (`transport.rs`); this module models the *full* slot
// lifecycle the connection-cleanup path needs: a slot moves
// `Pending -> Accepted -> Closed`, and `Closed` is terminal for the lifetime of
// that `ClientId`. A clean disconnect and a timeout both land in `Closed`, carrying
// the `CloseCause` so the engine glue can log/route them, and `closed` slots stop
// accepting input/snapshot traffic from that peer.
//
// Registry-blind by construction: this tracks connection slots keyed by the renet
// `ClientId` (`u64`) only. It never sees `EntityId`, the registry, or a pawn — the
// engine glue (`crate::netcode::lifecycle`) owns the slot -> remote-pawn mapping and
// the despawn through the game-logic-owned apply path.

use std::collections::HashMap;

use renet::ClientId;

/// Why a client slot closed. A clean disconnect and a timeout are distinguished so
/// the engine glue can log them differently and a future Phase 4 policy can branch
/// (this task runs one cleanup path for both — see `lifecycle.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseCause {
    /// The client sent a clean disconnect (renet `DisconnectedByClient`).
    Disconnect,
    /// The transport layer dropped the connection — a timeout or a lower-level
    /// transport error (renet `Transport` and the channel/serialization variants).
    /// Phase 2 folds every non-clean cause here; the engine glue treats it as a
    /// timeout for cleanup purposes.
    Timeout,
}

/// The lifecycle state of one connection slot. `Closed` is terminal: once a slot
/// closes it never re-opens for the same `ClientId` (a later connection is a fresh
/// `ClientId` and thus a fresh slot). This is what lets the post-close path refuse
/// stale traffic without a race on slot reuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    /// Connected at the transport layer; the app handshake has not yet validated.
    /// No entity state flows in this state (the Phase 1 pending invariant).
    Pending,
    /// Handshake validated — the slot may receive entity state and its messages are
    /// accepted.
    Accepted,
    /// The slot closed (clean disconnect or timeout). Terminal. Input/snapshot
    /// traffic from this peer is refused from here on.
    Closed { cause: CloseCause },
}

/// A slot lifecycle transition surfaced to the caller after a poll. The engine glue
/// reacts to these: `Accepted` triggers slot-pawn registration, `Closed` triggers
/// the immediate remote-pawn despawn cleanup. `Pending` is not surfaced — it carries
/// no engine-visible action (no pawn yet, no state sent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotEvent {
    /// The slot passed the app handshake and is now accepted.
    Accepted { client_id: ClientId },
    /// The slot closed. Carries the cause so the glue can distinguish a clean
    /// disconnect from a timeout (both run the same cleanup in Phase 2).
    Closed {
        client_id: ClientId,
        cause: CloseCause,
    },
}

/// Tracks per-client slot state and the accepted->closed transitions. Owned by
/// `NetServer`; the caller reads transitions out of the `update`/`poll` return and
/// queries slot state through the server's accessors.
///
/// Keyed by `ClientId` (`u64`). A `Closed` entry is retained so a stale packet that
/// arrives after close is recognized as "from a closed slot" and refused rather than
/// mistaken for an unknown (and thus newly-connecting) peer.
#[derive(Debug, Default)]
pub struct SlotTable {
    slots: HashMap<ClientId, SlotState>,
}

impl SlotTable {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark a slot `Pending` on transport connect. A slot that already exists is
    /// left as-is: a `Closed` slot must never be resurrected to `Pending` by a
    /// duplicate/stale connect event (slot reuse only ever happens under a fresh
    /// `ClientId`). Returns no event — `Pending` carries no engine action.
    pub fn on_connect(&mut self, client_id: ClientId) {
        self.slots.entry(client_id).or_insert(SlotState::Pending);
    }

    /// Mark a slot `Accepted` (app handshake passed). Idempotent. A `Closed` slot is
    /// never re-accepted. Returns the `Accepted` event the first time the slot
    /// transitions into `Accepted` so the glue registers its pawn exactly once;
    /// `None` if it was already accepted or is closed.
    #[must_use]
    pub fn on_accept(&mut self, client_id: ClientId) -> Option<SlotEvent> {
        match self.slots.get(&client_id) {
            Some(SlotState::Accepted) | Some(SlotState::Closed { .. }) => None,
            _ => {
                self.slots.insert(client_id, SlotState::Accepted);
                Some(SlotEvent::Accepted { client_id })
            }
        }
    }

    /// Close a slot with `cause`. Terminal and idempotent: closing an
    /// already-closed slot returns `None` (no duplicate cleanup), and the first
    /// close wins its cause. A slot the table never saw is created directly in
    /// `Closed` (a disconnect for a never-accepted peer still records the closed
    /// state so later stale packets are refused), but it emits no event — there was
    /// no accepted pawn to clean up. Returns the `Closed` event only when an
    /// `Accepted` slot closes, since that is the transition the cleanup path acts on.
    #[must_use]
    pub fn on_close(&mut self, client_id: ClientId, cause: CloseCause) -> Option<SlotEvent> {
        match self.slots.get(&client_id) {
            Some(SlotState::Closed { .. }) => None,
            Some(SlotState::Accepted) => {
                self.slots.insert(client_id, SlotState::Closed { cause });
                Some(SlotEvent::Closed { client_id, cause })
            }
            // Pending (never accepted) or unknown: record Closed so stale traffic is
            // refused, but emit nothing — no slot-owned pawn was ever created.
            _ => {
                self.slots.insert(client_id, SlotState::Closed { cause });
                None
            }
        }
    }

    /// The current state of a slot, or `None` if the table never saw this client.
    #[must_use]
    pub fn state(&self, client_id: ClientId) -> Option<SlotState> {
        self.slots.get(&client_id).copied()
    }

    /// Is this slot accepted (and thus allowed to send/receive entity state)? A
    /// closed or pending slot is not. The post-close refusal gate reads this.
    #[must_use]
    pub fn is_accepted(&self, client_id: ClientId) -> bool {
        matches!(self.slots.get(&client_id), Some(SlotState::Accepted))
    }

    /// Has this slot closed? `true` for both clean-disconnect and timeout closes.
    #[must_use]
    pub fn is_closed(&self, client_id: ClientId) -> bool {
        matches!(self.slots.get(&client_id), Some(SlotState::Closed { .. }))
    }

    /// Every currently-accepted client id. Closed and pending slots are excluded —
    /// the host replicates only to accepted slots.
    #[must_use]
    pub fn accepted_clients(&self) -> Vec<ClientId> {
        self.slots
            .iter()
            .filter(|(_, state)| matches!(state, SlotState::Accepted))
            .map(|(id, _)| *id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT_A: ClientId = 1;

    #[test]
    fn connect_then_accept_emits_accepted_once() {
        let mut slots = SlotTable::new();
        slots.on_connect(CLIENT_A);
        assert_eq!(slots.state(CLIENT_A), Some(SlotState::Pending));

        let first = slots.on_accept(CLIENT_A);
        assert_eq!(
            first,
            Some(SlotEvent::Accepted {
                client_id: CLIENT_A
            })
        );
        assert!(slots.is_accepted(CLIENT_A));

        // Re-accepting is a no-op: no duplicate pawn registration.
        assert_eq!(slots.on_accept(CLIENT_A), None);
    }

    #[test]
    fn accepted_slot_close_emits_closed_with_cause() {
        for cause in [CloseCause::Disconnect, CloseCause::Timeout] {
            let mut slots = SlotTable::new();
            slots.on_connect(CLIENT_A);
            let _ = slots.on_accept(CLIENT_A);

            let event = slots.on_close(CLIENT_A, cause);
            assert_eq!(
                event,
                Some(SlotEvent::Closed {
                    client_id: CLIENT_A,
                    cause
                })
            );
            assert!(slots.is_closed(CLIENT_A));
            assert!(!slots.is_accepted(CLIENT_A));
        }
    }

    #[test]
    fn close_is_terminal_and_idempotent() {
        let mut slots = SlotTable::new();
        slots.on_connect(CLIENT_A);
        let _ = slots.on_accept(CLIENT_A);
        let _ = slots.on_close(CLIENT_A, CloseCause::Disconnect);

        // A second close (e.g. a duplicate transport event) emits nothing.
        assert_eq!(slots.on_close(CLIENT_A, CloseCause::Timeout), None);
        // The first cause wins and the slot stays closed.
        assert_eq!(
            slots.state(CLIENT_A),
            Some(SlotState::Closed {
                cause: CloseCause::Disconnect
            })
        );
    }

    #[test]
    fn closed_slot_is_not_resurrected_by_stale_connect_or_accept() {
        let mut slots = SlotTable::new();
        slots.on_connect(CLIENT_A);
        let _ = slots.on_accept(CLIENT_A);
        let _ = slots.on_close(CLIENT_A, CloseCause::Timeout);

        // A stray connect or accept for the closed client must not re-open it.
        slots.on_connect(CLIENT_A);
        assert!(slots.is_closed(CLIENT_A));
        assert_eq!(slots.on_accept(CLIENT_A), None);
        assert!(slots.is_closed(CLIENT_A));
    }

    #[test]
    fn close_of_never_accepted_slot_records_closed_without_event() {
        let mut slots = SlotTable::new();
        slots.on_connect(CLIENT_A); // Pending, never accepted.
        // No pawn exists, so closing emits no cleanup event, but the slot is closed
        // so later stale traffic is refused.
        assert_eq!(slots.on_close(CLIENT_A, CloseCause::Disconnect), None);
        assert!(slots.is_closed(CLIENT_A));
    }

    #[test]
    fn accepted_clients_excludes_pending_and_closed() {
        let mut slots = SlotTable::new();
        slots.on_connect(1);
        let _ = slots.on_accept(1);
        slots.on_connect(2); // pending
        slots.on_connect(3);
        let _ = slots.on_accept(3);
        let _ = slots.on_close(3, CloseCause::Disconnect);

        let accepted = slots.accepted_clients();
        assert_eq!(accepted, vec![1]);
    }
}
