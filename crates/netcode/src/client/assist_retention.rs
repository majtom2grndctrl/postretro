// Remote pawns' life-state id and `assistable` bool, retained from the movement
// payload a connected client otherwise drops for a pawn it does not predict.
// See: context/lib/networking.md §What gates, and what replicates instead

use std::collections::HashMap;

use postretro_entities::EntityId;
use postretro_net::wire::{NetworkId, WirePlayerMovementState};

use super::ClientReplication;

/// What a connected client knows of a remote pawn's assist candidacy. The id
/// is compared only for equality, never resolved through a name table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteAssistFacts {
    pub life_state: u8,
    pub assistable: bool,
}

/// Keyed by identity, beside `remote_player_locomotion`, but cleared only when
/// the identity ends: a meshless descriptor dropping locomotion keeps its facts.
#[derive(Debug, Default)]
pub(super) struct RemoteAssistRetention {
    facts: HashMap<NetworkId, RemoteAssistFacts>,
}

impl RemoteAssistRetention {
    /// Each accepted payload overwrites the entry. No component is built and no
    /// gate runs.
    pub(super) fn retain(&mut self, network_id: NetworkId, wire: &WirePlayerMovementState) {
        self.facts.insert(
            network_id,
            RemoteAssistFacts {
                life_state: wire.life_state,
                assistable: wire.assistable,
            },
        );
    }

    pub(super) fn forget(&mut self, network_id: NetworkId) {
        self.facts.remove(&network_id);
    }

    pub(super) fn clear(&mut self) {
        self.facts.clear();
    }

    pub(super) fn get(&self, network_id: NetworkId) -> Option<RemoteAssistFacts> {
        self.facts.get(&network_id).copied()
    }
}

impl ClientReplication {
    /// A remote pawn's retained life-state id and `assistable` bool, by its
    /// client-local entity. `None` for an unmapped entity or one whose movement
    /// payload has not arrived; the local pawn never has an entry.
    pub fn remote_assist_facts(&self, entity_id: EntityId) -> Option<RemoteAssistFacts> {
        self.remote_assist
            .get(self.network_id_for_entity(entity_id)?)
    }
}
