// Remote pawns' life-state id and `assistable` bool, retained from the movement
// payload a connected client otherwise drops for a pawn it does not predict,
// and the local pawn's assist facts from its own record.
// See: context/lib/networking.md §What gates, and what replicates instead

use std::collections::HashMap;

use postretro_entities::EntityId;
use postretro_net::wire::{NetworkId, WireAssistFacts, WirePlayerMovementState};

use super::ClientReplication;

/// What a connected client knows of a remote pawn's assist candidacy. The id
/// is compared only for equality, never resolved through a name table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteAssistFacts {
    pub life_state: u8,
    pub assistable: bool,
}

/// One entry of the local pawn's pair table, resolved to the client-local
/// entity. Radius 0 means this pawn cannot assist that one; `required_ticks`
/// 0 means held progress is kept, not advanced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AssistPairEntry {
    pub target: EntityId,
    pub radius: f32,
    pub required_ticks: u32,
}

/// Keyed by identity, beside `remote_player_locomotion`, but cleared only when
/// the identity ends: a meshless descriptor dropping locomotion keeps its facts.
/// The local pawn's assist facts live here too, keyed by the identity whose
/// record carried them, so they clear at the same sites.
#[derive(Debug, Default)]
pub(super) struct RemoteAssistRetention {
    facts: HashMap<NetworkId, RemoteAssistFacts>,
    local: Option<(NetworkId, WireAssistFacts)>,
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

    /// Keep the facts of an accepted local-pawn record, a held-baseline
    /// resend included. A record that omits them leaves the last values.
    pub(super) fn retain_local(
        &mut self,
        network_id: NetworkId,
        local_player: bool,
        facts: Option<&WireAssistFacts>,
    ) {
        if let (true, Some(facts)) = (local_player, facts) {
            self.local = Some((network_id, facts.clone()));
        }
    }

    /// Drop a pawn's remote facts only: arming it as the local pawn keeps
    /// the assist facts its own records carry.
    pub(super) fn forget_remote(&mut self, network_id: NetworkId) {
        self.facts.remove(&network_id);
    }

    pub(super) fn forget(&mut self, network_id: NetworkId) {
        self.facts.remove(&network_id);
        if self.local.as_ref().is_some_and(|(id, _)| *id == network_id) {
            self.local = None;
        }
    }

    pub(super) fn clear(&mut self) {
        self.facts.clear();
        self.local = None;
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

    /// The local pawn's pair table as helper, from its newest accepted
    /// record, each entry resolved to its client-local entity. An entry whose
    /// pawn is not yet mapped is left out. Empty before any table arrives.
    pub fn assist_pair_table(&self) -> Vec<AssistPairEntry> {
        let Some((_, facts)) = &self.remote_assist.local else {
            return Vec::new();
        };
        facts
            .pairs
            .iter()
            .filter_map(|pair| {
                Some(AssistPairEntry {
                    target: self.entity_for_network_id(NetworkId(pair.network_id))?,
                    radius: pair.radius,
                    required_ticks: pair.required_ticks,
                })
            })
            .collect()
    }

    /// The required ticks of the highest-progress helper on the local pawn,
    /// from its newest accepted record; 0 when none is received.
    pub fn received_required_ticks(&self) -> u32 {
        self.remote_assist
            .local
            .as_ref()
            .map_or(0, |(_, facts)| facts.received_required_ticks)
    }
}
