// Remote pawns' life-state id and `assistable` bool, retained from the movement
// payload a connected client otherwise drops for a pawn it does not predict,
// and the local pawn's assist facts from its own record.
// See: context/lib/networking.md §What gates, and what replicates instead

use std::collections::HashMap;

use postretro_entities::{EntityId, SlotTable};
use postretro_net::wire::{NetworkId, WireAssistFacts, WirePlayerMovementState};

use super::ClientReplication;
use crate::state_slots::{ClientStateApply, StateApplyOutcome};

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

/// One host sample of the local pawn's committed `player.assistProgress`,
/// paired with the input tick its snapshot acknowledged. The receive loop
/// appends one per accepted snapshot whose state batch applied, whose
/// local-pawn record carries an acknowledged tick, and whose slot was not held
/// for a baseline refresh; the app's assist prediction corrects against it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AssistProgressSample {
    pub acked_tick: u32,
    pub progress: f32,
}

impl AssistProgressSample {
    /// The sample one applied state batch contributes: the local pawn's
    /// acknowledged input tick paired with the committed `player.assistProgress`
    /// the batch left, when both exist.
    pub(crate) fn from_applied_batch(
        acked_tick: Option<u32>,
        state_slots: &ClientStateApply,
        slot_table: &SlotTable,
        state_outcome: &StateApplyOutcome,
    ) -> Option<Self> {
        let acked_tick = acked_tick?;
        let progress = state_slots.assist_progress_sample(slot_table, state_outcome)?;
        Some(Self {
            acked_tick,
            progress,
        })
    }
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

    /// Every remote pawn whose facts are retained and whose entity is mapped,
    /// in entity-id order, written into `out` (cleared first) so a per-tick
    /// caller reuses one buffer. The client's stand-in for `players()` order.
    pub fn remote_assist_pawns(&self, out: &mut Vec<(EntityId, RemoteAssistFacts)>) {
        out.clear();
        out.extend(
            self.remote_assist
                .facts
                .iter()
                .filter_map(|(network_id, facts)| {
                    Some((self.entity_for_network_id(*network_id)?, *facts))
                }),
        );
        out.sort_unstable_by_key(|(entity, _)| *entity);
    }

    /// A remote pawn's position as the interpolation buffer presents it at
    /// `render_server_tick`, the tick the last frame's remote sampling used;
    /// with none yet, the newest buffered position, which is what remotes
    /// present before time sync. The registry transform is no substitute: on
    /// a snapshot-apply frame it holds the raw newest pose until sampling runs
    /// after the tick loop. `None` for an unmapped entity or an empty buffer.
    pub fn remote_assist_position(
        &self,
        entity_id: EntityId,
        render_server_tick: Option<f64>,
    ) -> Option<glam::Vec3> {
        let network_id = self.network_id_for_entity(entity_id)?;
        match render_server_tick {
            Some(tick) => self
                .interp
                .presented_pose(network_id, tick)
                .map(|pose| pose.transform.position),
            None => self
                .interp
                .newest_transform(network_id)
                .map(|transform| transform.position),
        }
    }

    /// The local pawn's pair table as helper, from its newest accepted
    /// record, each entry resolved to its client-local entity. An entry whose
    /// pawn is not yet mapped is left out. Empty before any table arrives.
    pub fn assist_pair_table(&self) -> Vec<AssistPairEntry> {
        let mut out = Vec::new();
        self.assist_pair_table_into(&mut out);
        out
    }

    /// [`Self::assist_pair_table`] into a caller-owned buffer, cleared first,
    /// so a per-frame reader reuses its allocation.
    pub fn assist_pair_table_into(&self, out: &mut Vec<AssistPairEntry>) {
        out.clear();
        let Some((_, facts)) = &self.remote_assist.local else {
            return;
        };
        out.extend(facts.pairs.iter().filter_map(|pair| {
            Some(AssistPairEntry {
                target: self.entity_for_network_id(NetworkId(pair.network_id))?,
                radius: pair.radius,
                required_ticks: pair.required_ticks,
            })
        }));
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

#[cfg(feature = "test-support")]
impl ClientReplication {
    /// An empty client replication for another crate's tests.
    pub fn new_for_test() -> Self {
        Self::new()
    }

    /// Apply one snapshot from another crate's tests, as a receive with no
    /// presentation bindings would, so a test can stand up retained assist
    /// facts, entity mappings and interpolation buffers through the real path.
    pub fn apply_snapshot_for_test(
        &mut self,
        registry: &mut postretro_entities::EntityRegistry,
        snapshot: &postretro_net::wire::SnapshotMessage,
    ) {
        let _ = self.apply_snapshot_with_presentation(
            registry,
            snapshot,
            snapshot.server_tick,
            1.0 / 60.0,
            &mut NoBindings,
            0.0,
        );
    }
}

#[cfg(feature = "test-support")]
struct NoBindings;

#[cfg(feature = "test-support")]
impl postretro_entities::presentation_binding::PresentationBindingResolver for NoBindings {
    fn resolve(
        &self,
        _: &postretro_entities::EntityRegistry,
        _: postretro_entities::presentation_binding::PresentationBindingKey,
    ) -> Option<(EntityId, glam::Vec3)> {
        None
    }

    fn key_for_entity(
        &self,
        _: &postretro_entities::EntityRegistry,
        _: EntityId,
    ) -> Option<(
        postretro_entities::presentation_binding::PresentationBindingKey,
        glam::Vec3,
    )> {
        None
    }
}
