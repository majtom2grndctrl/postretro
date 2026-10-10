// Per-client snapshot encoding and recipient-specific authority metadata.
// This module reads the tracker state in its parent and preserves its wire behavior.

use std::collections::{HashMap, HashSet};

use crate::wire::{
    ComponentPayload, RECORD_KIND_DELTA, RECORD_KIND_DESPAWN, RECORD_KIND_FULL_BASELINE,
    RawEntityRecord, RawSnapshotMessage, SNAPSHOT_VERSION,
};

use super::{EntityState, ServerReplication, raw_from_payload};

/// Per-client replication state the server tracks: how far this client has acked,
/// which baseline it holds per entity, which despawn tombstones it has applied, and
/// any pending baseline-refresh requests to satisfy in its next snapshot.
#[derive(Debug, Default)]
pub(super) struct ClientReplicationState {
    /// Highest snapshot sequence this client has acked (diagnostic; the per-entity
    /// baseline map is the authoritative progress record).
    pub(super) last_acked_sequence: u32,
    /// `network_id -> acked baseline_id`. The baseline the client is known to
    /// hold for each entity. Advanced monotonically by acks.
    pub(super) acked_baselines: HashMap<u32, u32>,
    /// `network_id -> acked tombstone_id`. A despawn is resent until its tombstone
    /// appears here.
    pub(super) acked_tombstones: HashMap<u32, u32>,
    /// Pending baseline-refresh requests, keyed by `(network_id,
    /// missing_baseline_ref)` so a duplicate request collapses to one queued
    /// `FullBaseline`. Cleared once the refresh is emitted.
    pub(super) pending_refreshes: HashSet<(u32, u32)>,
    /// Entity ids this recipient must never receive. This is host-local
    /// presentation routing, not snapshot metadata: the omitted record has no
    /// owner field on the wire. Excluded ids are removed once their tombstone
    /// drains, so the session-monotonic network-id namespace cannot accumulate
    /// per-shot entries forever.
    pub(super) excluded_entities: HashSet<u32>,
}

impl ServerReplication {
    pub(super) fn encode_for_client_with_sequence(
        &mut self,
        client_id: u64,
        server_tick: u32,
        sequence: u32,
    ) -> Option<RawSnapshotMessage> {
        // Pull the client's pending refreshes out up front (clearing them: a
        // refresh is satisfied by the FullBaseline this snapshot emits).
        let refreshes: HashSet<u32> = {
            let state = self.clients.get_mut(&client_id)?;
            let nids = state
                .pending_refreshes
                .iter()
                .map(|&(nid, _)| nid)
                .collect();
            state.pending_refreshes.clear();
            nids
        };

        let mut records = Vec::new();

        // Entity records. Iterate the current entity set; the client's acked map is
        // consulted read-only here, then mutated below would violate the borrow, so
        // read it through an immutable borrow scoped to this loop.
        for (&network_id, entity) in &self.entities {
            if self
                .clients
                .get(&client_id)
                .is_some_and(|client| client.excluded_entities.contains(&network_id))
            {
                continue;
            }
            let acked = self
                .clients
                .get(&client_id)
                .and_then(|c| c.acked_baselines.get(&network_id).copied());
            let force_full = refreshes.contains(&network_id);

            // Per-recipient movement-authority metadata (M15 Phase 3). Derived here
            // from id comparison only — the tracker never sees an `EntityId`, registry,
            // or descriptor. The metadata is meaningful only on a movement record
            // (`validate` rejects it elsewhere), so `movement_metadata` gates it on the
            // entity actually carrying a `PlayerMovementState` payload, and
            // `local_player` is `true` only in the snapshot sent to the owning client.
            let authority = MovementAuthority::for_recipient(entity, client_id);

            match acked {
                // Already holds the current baseline and no refresh forced: omit.
                Some(acked_id) if acked_id == entity.baseline_id && !force_full => continue,
                // Holds an older baseline and not forced: a per-entity delta.
                Some(acked_id) if !force_full => {
                    records.push(RawEntityRecord {
                        record_kind: RECORD_KIND_DELTA,
                        network_id,
                        baseline_id_or_ref: acked_id,
                        new_baseline_id_or_tombstone_id: entity.baseline_id,
                        reason: 0,
                        has_last_processed_client_tick: authority.has_tick,
                        last_processed_client_tick: authority.tick,
                        local_player: authority.local_player,
                        has_entity_class: authority.has_entity_class,
                        entity_class: authority.entity_class.clone(),
                        has_active_weapon_archetype: authority.has_active_weapon_archetype,
                        active_weapon_archetype: authority.active_weapon_archetype.clone(),
                        projectile_presentation: entity.projectile_presentation,
                        presentation_binding: entity.presentation_binding,
                        components: entity.components.iter().map(raw_from_payload).collect(),
                    });
                }
                // No acked baseline, or a forced refresh: a full baseline.
                _ => {
                    records.push(RawEntityRecord {
                        record_kind: RECORD_KIND_FULL_BASELINE,
                        network_id,
                        baseline_id_or_ref: entity.baseline_id,
                        new_baseline_id_or_tombstone_id: 0,
                        reason: 0,
                        has_last_processed_client_tick: authority.has_tick,
                        last_processed_client_tick: authority.tick,
                        local_player: authority.local_player,
                        has_entity_class: authority.has_entity_class,
                        entity_class: authority.entity_class.clone(),
                        has_active_weapon_archetype: authority.has_active_weapon_archetype,
                        active_weapon_archetype: authority.active_weapon_archetype.clone(),
                        projectile_presentation: entity.projectile_presentation,
                        presentation_binding: entity.presentation_binding,
                        components: entity.components.iter().map(raw_from_payload).collect(),
                    });
                }
            }
        }

        // Despawn records: resend every active tombstone the client has not acked.
        for (&network_id, tombstone) in &self.tombstones {
            if self
                .clients
                .get(&client_id)
                .is_some_and(|client| client.excluded_entities.contains(&network_id))
            {
                continue;
            }
            let acked = self
                .clients
                .get(&client_id)
                .and_then(|c| c.acked_tombstones.get(&network_id).copied())
                .unwrap_or(0);
            if acked >= tombstone.tombstone_id {
                continue;
            }
            records.push(RawEntityRecord {
                record_kind: RECORD_KIND_DESPAWN,
                network_id,
                baseline_id_or_ref: 0,
                new_baseline_id_or_tombstone_id: tombstone.tombstone_id,
                reason: tombstone.reason,
                // A despawn never carries movement-authority metadata or an
                // entity_class (rejected at validate); the tracker leaves all absent.
                has_last_processed_client_tick: false,
                last_processed_client_tick: 0,
                local_player: false,
                has_entity_class: false,
                entity_class: String::new(),
                has_active_weapon_archetype: false,
                active_weapon_archetype: String::new(),
                projectile_presentation: None,
                presentation_binding: None,
                components: Vec::new(),
            });
        }

        Some(RawSnapshotMessage {
            server_script_time: 0.0,
            version: SNAPSHOT_VERSION,
            sequence,
            server_tick,
            records,
            // Entity replication owns the entity-record half of the snapshot only.
            // The replicated state-slot half (fingerprint + records, M15 Phase 3.5)
            // is produced by the sibling state tracker and merged into this envelope
            // by the engine send path; the entity producer leaves it empty so a
            // state-less snapshot is still a valid carrier.
            state_schema_fingerprint: [0u8; 32],
            state_records: Vec::new(),
            faction_sentiment_record: None,
        })
    }
}

/// The per-recipient movement-authority metadata for one entity record, resolved
/// from id comparison only. Registry-blind: it compares the entity's tracked
/// `owner_client_id` against the recipient client id and never sees an `EntityId`,
/// the registry, or a movement descriptor.
struct MovementAuthority {
    /// Whether `tick` carries a real resolved cursor (mirrors the typed `Option`).
    has_tick: bool,
    /// The resolved cursor value; meaningful only when `has_tick` is `true`.
    tick: u32,
    /// `true` only in the snapshot sent to this pawn's owning client.
    local_player: bool,
    /// Whether `entity_class` carries a real value (mirrors the typed `Option`).
    has_entity_class: bool,
    /// The descriptor-class identifier; meaningful only when `has_entity_class` is
    /// `true`. Cloned into the record (the only allocation on the encode path here).
    entity_class: String,
    /// Whether the active-weapon metadata carries a canonical name. Unlike
    /// `entity_class`, it is valid only on a movement record.
    has_active_weapon_archetype: bool,
    /// Active weapon canonical name, empty when the flag is false.
    active_weapon_archetype: String,
}

impl MovementAuthority {
    /// Resolve the authority metadata for `recipient`. Two metadata classes ride a
    /// record under DIFFERENT rules, mirroring the two `validate` gates (E10 Task 3/4):
    ///
    /// - **Movement authority** (`has_tick`/`last_processed_client_tick`, `local_player`)
    ///   rides ONLY a movement record: an entity with no `PlayerMovementState` payload
    ///   emits all-absent movement metadata regardless of its `owner_client_id`, because
    ///   `validate` rejects ack/local-player metadata on a non-movement record.
    ///   `local_player` is `true` only when the recipient is the tracked owner.
    /// - **`entity_class`** rides any NON-DESPAWN record carrying a finite `Transform`
    ///   (it no longer needs a `PlayerMovementState`): a host-authoritative map enemy
    ///   replicates Transform-only, so its descriptor class must travel on that record.
    ///   It is echoed to EVERY recipient (it is entity state, not owner-private — a
    ///   remote viewer materializes the same descriptor for its presentation). Gated on
    ///   the finite-`Transform` check `validate` enforces on receipt so production never
    ///   stamps a class the validator would reject. (A despawn record never reaches
    ///   here — this resolves only entity records the encode loop emits as
    ///   baseline/delta.)
    fn for_recipient(entity: &EntityState, recipient: u64) -> Self {
        let carries_movement = entity
            .components
            .iter()
            .any(|c| matches!(c, ComponentPayload::PlayerMovementState(_)));
        let carries_finite_transform = entity.components.iter().any(|c| match c {
            ComponentPayload::Transform(t) => t.all_finite(),
            ComponentPayload::PlayerMovementState(_) => false,
            ComponentPayload::MeshAnimationState(_) => false,
            ComponentPayload::KinematicMoverState(_)
            | ComponentPayload::LightState(_)
            | ComponentPayload::EmitterState(_)
            | ComponentPayload::FogState(_) => false,
        });

        // `entity_class` is valid on any finite-Transform record; movement metadata is
        // movement-only. Empty class ids are equivalent to absent metadata on the
        // producer side so the raw envelope stays valid.
        let entity_class = if carries_finite_transform {
            entity
                .entity_class
                .as_deref()
                .filter(|class| !class.is_empty())
        } else {
            None
        };
        let active_weapon_archetype = if carries_movement {
            entity
                .active_weapon_archetype
                .as_deref()
                .filter(|archetype| !archetype.is_empty())
        } else {
            None
        };
        Self {
            has_tick: carries_movement && entity.last_processed_client_tick.is_some(),
            tick: if carries_movement {
                entity.last_processed_client_tick.unwrap_or(0)
            } else {
                0
            },
            local_player: carries_movement && entity.owner_client_id == Some(recipient),
            has_entity_class: entity_class.is_some(),
            entity_class: entity_class.unwrap_or_default().to_string(),
            has_active_weapon_archetype: active_weapon_archetype.is_some(),
            active_weapon_archetype: active_weapon_archetype.unwrap_or_default().to_string(),
        }
    }
}
