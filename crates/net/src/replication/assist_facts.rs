// Assist facts as owner-private authority metadata on the local-pawn record.
// See: context/lib/networking.md §Version gates

use std::collections::{HashMap, VecDeque};

use crate::wire::{ComponentPayload, RawEntityRecord, WireAssistFacts};

use super::encode::ClientReplicationState;
use super::{EntityState, ServerReplication};

/// Sent values kept awaiting an ack. A client acks continuously, so this
/// bounds only a silent one; dropping the oldest never confirms a value early.
const MAX_UNACKED_SENDS: usize = 64;

/// One recipient's delivery state for its own assist facts. Unlike the
/// resolved cursor, these values can stay still for a whole hold, so a value
/// counts as delivered only once a snapshot ack reaches the snapshot that
/// carried it.
#[derive(Debug, Default)]
pub(super) struct AssistDelivery {
    /// Each value sent, with the sequence of the snapshot that carried it,
    /// oldest first.
    sent: VecDeque<(u32, WireAssistFacts)>,
    /// The value in the newest acknowledged snapshot that carried one.
    confirmed: Option<WireAssistFacts>,
}

impl AssistDelivery {
    /// Confirm every value sent at or before `acked_sequence`; the newest of
    /// them becomes the confirmed value.
    pub(super) fn confirm(&mut self, acked_sequence: u32) {
        while let Some((sequence, _)) = self.sent.front() {
            if *sequence > acked_sequence {
                break;
            }
            let (_, facts) = self.sent.pop_front().expect("front exists");
            self.confirmed = Some(facts);
        }
    }

    fn record_sent(&mut self, sequence: u32, facts: &WireAssistFacts) {
        if self.sent.len() == MAX_UNACKED_SENDS {
            self.sent.pop_front();
        }
        self.sent.push_back((sequence, facts.clone()));
    }
}

impl ServerReplication {
    /// Refresh each recipient's assist facts, keyed by owner client id. Like the
    /// resolved cursor, they are authority metadata: no baseline is minted, so
    /// a change sends nothing to any other recipient. A client absent from
    /// `facts` has none.
    pub fn ingest_assist_facts(&mut self, facts: HashMap<u64, WireAssistFacts>) {
        self.assist_facts = facts;
    }
}

/// Whether `client`'s own movement pawn must be resent at its held baseline
/// because its current assist facts differ from the newest acknowledged ones.
pub(super) fn own_assist_facts_changed(
    current: Option<&WireAssistFacts>,
    client: &ClientReplicationState,
    entity: &EntityState,
    client_id: u64,
) -> bool {
    current.is_some_and(|facts| {
        entity.owner_client_id == Some(client_id)
            && entity
                .components
                .iter()
                .any(|c| matches!(c, ComponentPayload::PlayerMovementState(_)))
            && client.assist.confirmed.as_ref() != Some(facts)
    })
}

/// Set the current facts on every local-pawn record of this snapshot and
/// remember them against its sequence.
pub(super) fn stamp_assist_facts(
    current: Option<&WireAssistFacts>,
    client: &mut ClientReplicationState,
    records: &mut [RawEntityRecord],
    sequence: u32,
) {
    let Some(facts) = current else {
        return;
    };
    let mut stamped = false;
    for record in records.iter_mut().filter(|record| record.local_player) {
        facts.stamp(record);
        stamped = true;
    }
    if stamped {
        client.assist.record_sent(sequence, facts);
    }
}

#[cfg(test)]
#[path = "assist_facts_tests.rs"]
mod tests;
