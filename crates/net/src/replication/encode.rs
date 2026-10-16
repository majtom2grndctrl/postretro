// Per-client snapshot encoding and recipient-specific authority metadata.
// See: context/lib/networking.md §Join baseline budget.

use std::collections::{HashMap, HashSet};

use crate::wire::{
    ComponentPayload, RECORD_KIND_DELTA, RECORD_KIND_DESPAWN, RECORD_KIND_FULL_BASELINE,
    RawEntityRecord, RawSnapshotMessage, SNAPSHOT_VERSION,
};

use super::{EntityState, JOIN_BASELINE_BUDGET_BYTES, ServerReplication, raw_from_payload};

/// Per-client replication state the server tracks: how far this client has acked,
/// which baseline it holds per entity, which despawn tombstones it has applied, and
/// any pending baseline-refresh requests to process during its next encoding pass.
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
    /// missing_baseline_ref)` so duplicates collapse to one forced `FullBaseline`.
    /// Consumed when encoding starts; only entities with an acknowledged baseline
    /// emit that refresh immediately. Initial-baseline flights use ack/repair flow.
    pub(super) pending_refreshes: HashSet<(u32, u32)>,
    /// Entity ids this recipient must never receive. This is host-local
    /// presentation routing, not snapshot metadata: the omitted record has no
    /// owner field on the wire. Excluded ids are removed once their tombstone
    /// drains, so the session-monotonic network-id namespace cannot accumulate
    /// per-shot entries forever.
    pub(super) excluded_entities: HashSet<u32>,
    /// Pending baselines already sent, held until a snapshot ack reaches their send.
    pub(super) in_flight_baselines: HashMap<u32, BaselineFlight>,
    /// Baselines left pending by the most recent encoding budget.
    pub(super) deferred_baselines: usize,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct BaselineFlight {
    pub(super) sent_sequence: u32,
    pub(super) sent_baseline_id: u32,
}

impl ServerReplication {
    pub(super) fn encode_for_client_with_sequence(
        &mut self,
        client_id: u64,
        server_tick: u32,
        sequence: u32,
    ) -> Option<RawSnapshotMessage> {
        let client = self.clients.get_mut(&client_id)?;
        let refreshes: HashSet<u32> = client
            .pending_refreshes
            .drain()
            .map(|(network_id, _)| network_id)
            .collect();
        let mut records = Vec::new();

        let mut tombstones: Vec<_> = self.tombstones.iter().collect();
        tombstones.sort_unstable_by_key(|(network_id, _)| **network_id);
        for (&network_id, tombstone) in tombstones {
            if client.excluded_entities.contains(&network_id)
                || client
                    .acked_tombstones
                    .get(&network_id)
                    .copied()
                    .unwrap_or(0)
                    >= tombstone.tombstone_id
            {
                continue;
            }
            records.push(RawEntityRecord {
                record_kind: RECORD_KIND_DESPAWN,
                network_id,
                baseline_id_or_ref: 0,
                new_baseline_id_or_tombstone_id: tombstone.tombstone_id,
                reason: tombstone.reason,
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

        // Sort references before constructing records: deferred entities retain only
        // their latest tracker state, rather than allocating unsent wire payloads.
        let mut held = Vec::new();
        let mut pending = Vec::new();
        for (&network_id, entity) in &self.entities {
            if client.excluded_entities.contains(&network_id)
                || client.in_flight_baselines.contains_key(&network_id)
            {
                continue;
            }
            match client
                .acked_baselines
                .get(&network_id)
                .copied()
                .filter(|id| *id != 0)
            {
                Some(acked) => {
                    let force_full = refreshes.contains(&network_id);
                    if acked != entity.baseline_id || force_full {
                        held.push((network_id, entity, acked, force_full));
                    }
                }
                None => pending.push((network_id, entity)),
            }
        }
        held.sort_unstable_by_key(|(network_id, ..)| *network_id);
        for (network_id, entity, acked, force_full) in held {
            records.push(entity_record(
                network_id,
                entity,
                client_id,
                (!force_full).then_some(acked),
            ));
        }
        pending.sort_unstable_by_key(|(network_id, entity)| {
            let own_pawn = entity.owner_client_id == Some(client_id)
                && entity
                    .components
                    .iter()
                    .any(|c| matches!(c, ComponentPayload::PlayerMovementState(_)));
            let group = if own_pawn {
                0
            } else if entity.presentation_binding.is_none() {
                1
            } else {
                2
            };
            (group, *network_id)
        });
        let pending_count = pending.len();
        let mut admitted = 0;
        let mut budget_bytes = 0;
        for (network_id, entity) in pending {
            let record = entity_record(network_id, entity, client_id, None);
            // The first pending baseline is exempt, even if it is oversized.
            // Measure RawEntityRecord with the production wire codec, not an
            // estimate from component sizes. Other traffic never draws this budget.
            if admitted != 0 {
                let bytes = crate::wire::encode(&record).len();
                if bytes > JOIN_BASELINE_BUDGET_BYTES - budget_bytes {
                    break;
                }
                budget_bytes += bytes;
            }
            client.in_flight_baselines.insert(
                network_id,
                BaselineFlight {
                    sent_sequence: sequence,
                    sent_baseline_id: entity.baseline_id,
                },
            );
            records.push(record);
            admitted += 1;
        }
        client.deferred_baselines = pending_count - admitted;

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

/// Construct both live record shapes through the same metadata and payload path.
fn entity_record(
    network_id: u32,
    entity: &EntityState,
    client_id: u64,
    baseline_ref: Option<u32>,
) -> RawEntityRecord {
    let authority = MovementAuthority::for_recipient(entity, client_id);
    RawEntityRecord {
        record_kind: if baseline_ref.is_some() {
            RECORD_KIND_DELTA
        } else {
            RECORD_KIND_FULL_BASELINE
        },
        network_id,
        baseline_id_or_ref: baseline_ref.unwrap_or(entity.baseline_id),
        new_baseline_id_or_tombstone_id: if baseline_ref.is_some() {
            entity.baseline_id
        } else {
            0
        },
        reason: 0,
        has_last_processed_client_tick: authority.has_tick,
        last_processed_client_tick: authority.tick,
        local_player: authority.local_player,
        has_entity_class: authority.has_entity_class,
        entity_class: authority.entity_class,
        has_active_weapon_archetype: authority.has_active_weapon_archetype,
        active_weapon_archetype: authority.active_weapon_archetype,
        projectile_presentation: entity.projectile_presentation,
        presentation_binding: entity.presentation_binding,
        components: entity.components.iter().map(raw_from_payload).collect(),
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
            ComponentPayload::PlayerMovementState(_) | ComponentPayload::TeleporterState(_) => false,
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
