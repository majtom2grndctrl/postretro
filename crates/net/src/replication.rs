// Server-side replication tracker: per-client acked-baseline state and per-client
// delta snapshot encoding, keyed only by `NetworkId`.
// See: context/lib/networking.md
//
// This module is the net-crate half of M15 Phase 2 server replication. It is
// `postretro`-free and registry-blind by construction: it never sees `EntityId`,
// the `EntityRegistry`, or glam. The engine glue (`crate::netcode`) borrows the
// registry once per tick, copies replicable state into owned wire mirrors keyed by
// `NetworkId`, and hands those owned snapshots here; this tracker owns *only*
// replication state and encoding.
//
// Delta granularity is **per entity**: an unacked or lost packet only affects that
// entity. A dirty entity with an acked baseline emits a `Delta`; one needing a
// `FullBaseline` waits for the pending-byte budget, then stays in flight until a
// qualifying snapshot ack. An entity whose acked baseline matches the current one
// is omitted, except an owner's movement pawn whose resolved cursor advanced: the
// owner gets it again at the held baseline, to reconcile a still pawn. Despawn tombstones resend until acked. Acks advance per-client state
// monotonically: newer baseline ids advance, older ones are ignored, and omitted
// entries leave prior state unchanged. Stale or malformed ack/refresh ids are
// ignored without panicking.

use std::collections::{HashMap, HashSet};

use crate::wire::{
    COMPONENT_KIND_KINEMATIC_MOVER_STATE, COMPONENT_KIND_MESH_ANIMATION_STATE,
    COMPONENT_KIND_PLAYER_MOVEMENT_STATE, COMPONENT_KIND_TRANSFORM, ComponentPayload, EntityRecord,
    RawComponentPayload, RawSnapshotMessage, WirePresentationBinding, WireProjectilePresentation,
};

#[cfg(test)]
mod presentation_budget_tests;

mod encode;
use encode::ClientReplicationState;

/// Pending FullBaseline bytes per snapshot beyond the first pending record.
/// Tombstones, held-entity deltas and forced refreshes are exempt.
pub const JOIN_BASELINE_BUDGET_BYTES: usize = 16 * 1024;

/// One replicable entity's owned, post-tick component state, keyed by its stable
/// `NetworkId`. Produced by the engine glue (`crate::netcode`) after it releases
/// the registry borrow, then handed to [`ServerReplication::ingest_tick`].
///
/// `components` carries the wire mirrors directly — dirty detection compares these
/// (`WireTransform`, `WirePlayerMovementState`) by value, never by serializing and
/// diffing bytes. The order is significant for equality: the producer must emit a
/// stable component order per entity.
///
/// `owner_client_id` and `last_processed_client_tick` are movement-authority
/// metadata the engine glue stamps for a networked movement pawn (M15 Phase 3). The
/// tracker stays registry-blind: it keeps them keyed by `NetworkId`, compares
/// `owner_client_id` against the recipient client id at encode time to derive the
/// per-recipient `local_player` flag, and echoes `last_processed_client_tick` into
/// the record. Both are `None` for the Transform-only fixtures and the demo mover —
/// a pawn with no movement authority. They are **excluded from dirty detection**:
/// the resolved cursor advances every tick a command resolves, but the wire
/// payload only changes when the pose/movement mirrors do, so folding the cursor
/// into equality would defeat the omit-unchanged optimization.
#[derive(Debug, Clone, PartialEq)]
pub struct EntitySnapshot {
    pub network_id: u32,
    pub components: Vec<ComponentPayload>,
    /// The client that owns this pawn (movement authority), or `None` for an
    /// unowned entity (fixture / demo mover / static). Compared against the
    /// recipient client id at encode time to set `local_player`. Never exposes an
    /// `EntityId` — it is an opaque `u64` client id, the same id the transport keys
    /// clients by.
    pub owner_client_id: Option<u64>,
    /// The latest client command tick the host resolved for this pawn before
    /// snapshotting, or `None` if no command has resolved yet (or the pawn carries
    /// no movement authority). Echoed into the per-recipient record's
    /// `last_processed_client_tick`.
    pub last_processed_client_tick: Option<u32>,
    /// The opaque descriptor-class identifier the engine glue materialized this
    /// entity from (e.g. `"player"`). When provided, it can ride any non-despawn
    /// record carrying a finite `Transform`; movement authority metadata remains
    /// movement-only. Echoed verbatim into the record's `entity_class` so the
    /// recipient can materialize the matching descriptor-backed component. A plain
    /// string id — the tracker stays registry-blind and never resolves it. Excluded
    /// from dirty detection because a class never changes for a live entity.
    pub entity_class: Option<String>,
    /// Opaque canonical name of this movement pawn's active weapon, or `None` when
    /// unarmed. Shared-visible presentation metadata; a change advances the entity
    /// baseline so recipients observe an equip/unequip within one snapshot interval.
    pub active_weapon_archetype: Option<String>,
    pub projectile_presentation: Option<WireProjectilePresentation>,
    pub presentation_binding: Option<WirePresentationBinding>,
}

impl EntitySnapshot {
    /// Construct a metadata-free snapshot (no movement authority). The Transform-only
    /// slot fixtures and the demo mover use this; it keeps their call sites terse and
    /// documents that an unowned entity never carries authority metadata.
    #[must_use]
    pub fn unowned(network_id: u32, components: Vec<ComponentPayload>) -> Self {
        Self {
            network_id,
            components,
            owner_client_id: None,
            last_processed_client_tick: None,
            entity_class: None,
            active_weapon_archetype: None,
            projectile_presentation: None,
            presentation_binding: None,
        }
    }
}

/// Despawn reason carried in a tombstone record. A `u8` on the wire; the tracker
/// keeps it so a resent despawn carries the original reason.
pub type DespawnReason = u8;

/// Current server-tracked state for one entity: its latest owned components, the
/// `baseline_id` that names them, and the movement-authority metadata. The baseline
/// id advances only when the wire mirrors change, so a client holding that id needs
/// no update — but the authority metadata (`owner_client_id`,
/// `last_processed_client_tick`) is refreshed on **every** ingest without bumping
/// the baseline, since the resolved cursor advances each tick a command resolves
/// while the pose may not have moved. The owner still needs each advanced cursor
/// to reconcile a still pawn, so the encoder resends the owner's pawn at its held
/// baseline when the cursor moves; every other recipient keeps the omission. The
/// resend is metadata-only to the client, which re-applies nothing, but it carries
/// the full record on the wire, as a moving pawn's delta does.
#[derive(Debug, Clone)]
struct EntityState {
    baseline_id: u32,
    components: Vec<ComponentPayload>,
    /// Owning client (movement authority), or `None` for an unowned entity.
    owner_client_id: Option<u64>,
    /// Latest resolved client command tick, or `None` until one resolves.
    last_processed_client_tick: Option<u32>,
    /// Descriptor-class identifier the pawn was materialized from, or `None`. Echoed
    /// into the per-recipient record's `entity_class`; refreshed every ingest like the
    /// rest of the authority metadata.
    entity_class: Option<String>,
    /// Active weapon canonical name for a movement pawn. Unlike immutable class and
    /// authority metadata, a change makes the snapshot dirty and advances its baseline.
    active_weapon_archetype: Option<String>,
    projectile_presentation: Option<WireProjectilePresentation>,
    presentation_binding: Option<WirePresentationBinding>,
}

/// A despawned entity awaiting per-client tombstone acks. Held until every client
/// that was present for the entity has acked the tombstone; late joiners are
/// pre-acked for already-active tombstones and receive only live baselines.
#[derive(Debug, Clone, Copy)]
struct Tombstone {
    tombstone_id: u32,
    reason: DespawnReason,
}

/// Server-side replication tracker. Owns the monotonic sequence / baseline /
/// tombstone allocators, the current per-entity state, active despawn tombstones,
/// and per-client ack state. Keyed throughout by `NetworkId` (`u32`); never sees
/// `EntityId`.
#[derive(Debug)]
pub struct ServerReplication {
    /// Monotonic snapshot sequence, stamped into each emitted snapshot.
    next_sequence: u32,
    /// Monotonic baseline-id allocator; a fresh id every time an entity's wire
    /// mirrors change so a stale-baseline client is detectable. Starts at 1 — id 0
    /// is the reserved "no acked baseline" sentinel in the per-client ack map, so a
    /// real baseline must never be 0 or it would read as "unacked".
    next_baseline_id: u32,
    /// Monotonic tombstone-id allocator; a fresh id per despawn. Starts at 1 for the
    /// same sentinel reason as `next_baseline_id`.
    next_tombstone_id: u32,
    /// Current per-entity state, keyed by `NetworkId`.
    entities: HashMap<u32, EntityState>,
    /// Active despawn tombstones, keyed by `NetworkId`, resent until acked.
    tombstones: HashMap<u32, Tombstone>,
    /// One-shot causes for entities expected to vanish on the next ingest.
    pending_despawn_reasons: HashMap<u32, DespawnReason>,
    /// Per-client ack/refresh state.
    clients: HashMap<u64, ClientReplicationState>,
}

impl Default for ServerReplication {
    fn default() -> Self {
        // Baseline/tombstone allocators start at 1: id 0 is the "none yet" sentinel
        // for the per-client ack maps (a `.or_insert(0)` default reads as unacked).
        Self {
            next_sequence: 0,
            next_baseline_id: 1,
            next_tombstone_id: 1,
            entities: HashMap::new(),
            tombstones: HashMap::new(),
            pending_despawn_reasons: HashMap::new(),
            clients: HashMap::new(),
        }
    }
}

impl ServerReplication {
    /// Fresh tracker with monotonic ids reserving 0 as the unacked sentinel and no
    /// entities/clients.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear level-owned replication state while preserving connection-scoped
    /// sequence and id allocators. Re-promoted clients must see a sequence newer
    /// than every snapshot accepted before the level transition.
    pub fn reset_for_level_unload(&mut self) {
        self.entities.clear();
        self.tombstones.clear();
        self.pending_despawn_reasons.clear();
        self.clients.clear();
    }

    /// Forget a bound map presentation identity while clients retain their own
    /// authored entity. No tombstone is created, including on the next ingest.
    pub fn forget_entity_without_tombstone(&mut self, network_id: u32) {
        self.entities.remove(&network_id);
        self.tombstones.remove(&network_id);
        self.pending_despawn_reasons.remove(&network_id);
        for client in self.clients.values_mut() {
            client.acked_baselines.remove(&network_id);
            client.acked_tombstones.remove(&network_id);
            client.pending_refreshes.retain(|(id, _)| *id != network_id);
            client.excluded_entities.remove(&network_id);
            client.in_flight_baselines.remove(&network_id);
            client.sent_cursors.remove(&network_id);
        }
    }

    /// Attach a cause to the next despawn tombstone emitted for `network_id`.
    /// The marker is consumed when the entity vanishes from an ingested tick; if
    /// the entity remains live, a later call may overwrite it.
    pub fn set_next_despawn_reason(&mut self, network_id: u32, reason: DespawnReason) {
        if reason == 0 {
            self.pending_despawn_reasons.remove(&network_id);
        } else {
            self.pending_despawn_reasons.insert(network_id, reason);
        }
    }

    /// Return the live baseline currently tracked for `network_id`.
    ///
    /// Presentation lifecycles use this as an ingest witness: a terminal pose is
    /// retained until its changed baseline appears here and every intended
    /// recipient acknowledges that exact-or-newer baseline.
    #[must_use]
    pub fn current_baseline_id(&self, network_id: u32) -> Option<u32> {
        self.entities
            .get(&network_id)
            .map(|entity| entity.baseline_id)
    }

    /// Whether every registered recipient of `network_id` has acknowledged at
    /// least `baseline_id`. Clients excluded from the entity never receive or map
    /// it, so they do not participate in this delivery gate.
    #[must_use]
    pub fn baseline_acked_by_all_recipients(&self, network_id: u32, baseline_id: u32) -> bool {
        self.clients
            .values()
            .filter(|client| !client.excluded_entities.contains(&network_id))
            .all(|client| {
                client
                    .acked_baselines
                    .get(&network_id)
                    .copied()
                    .unwrap_or(0)
                    >= baseline_id
            })
    }

    /// Whether at least one client is registered for entity replication.
    #[must_use]
    pub fn has_registered_clients(&self) -> bool {
        !self.clients.is_empty()
    }

    /// Register a client so it is replicated to. Idempotent — re-registering an
    /// existing client leaves its ack state intact. A new client has no acked
    /// baselines, so live entities need `FullBaseline` records; the pending-byte
    /// budget may defer some. Tombstones active before it joined are pre-acked,
    /// since it never mapped those despawned entities.
    pub fn register_client(&mut self, client_id: u64) {
        if self.clients.contains_key(&client_id) {
            return;
        }

        let mut state = ClientReplicationState::default();
        state.acked_tombstones.extend(
            self.tombstones
                .iter()
                .map(|(&network_id, tombstone)| (network_id, tombstone.tombstone_id)),
        );
        self.clients.insert(client_id, state);
    }

    /// Drop a client (disconnect). Removing its [`ClientReplicationState`] discards
    /// *all* of that client's per-client maps (`acked_baselines`, `acked_tombstones`,
    /// `pending_refreshes`) in one move, so none of them leak. Remaining clients are
    /// unaffected — except that dropping a never-acking client may have been the last
    /// holdout keeping a tombstone alive, so re-evaluate the global tombstone map for
    /// anything now acked-by-all-remaining.
    pub fn remove_client(&mut self, client_id: u64) {
        self.clients.remove(&client_id);
        self.prune_acked_tombstones();
    }

    /// Omit one entity from this recipient's snapshots for its remaining lifetime.
    ///
    /// The tracker remains registry-blind: both ids are transport identities. This
    /// is intentionally stronger than per-recipient authority metadata because the
    /// recipient receives no baseline, delta, or later tombstone for the entity.
    /// It is used when a client already presents its own locally predicted visual.
    pub fn exclude_entity_for_client(&mut self, client_id: u64, network_id: u32) {
        if let Some(state) = self.clients.get_mut(&client_id) {
            state.excluded_entities.insert(network_id);
            state.in_flight_baselines.remove(&network_id);
            state.pending_refreshes.retain(|(id, _)| *id != network_id);
        }
    }

    /// Drop every global tombstone that all currently-registered clients have acked.
    ///
    /// A tombstone must keep resending to clients that have not yet acked it, so it is
    /// pruned only when *every* registered client's `acked_tombstones` covers it
    /// (`acked >= tombstone_id`, matching the encode gate). Pruning-after-all-acked is
    /// safe against future joins: a `NetworkId` is session-monotonic and never
    /// recycled, so a despawned id never reappears, and a client that joins after the
    /// prune gets a fresh full baseline of only the *live* entities — it never knew
    /// the despawned entity and so never needs its tombstone. With no clients
    /// registered, `all()` over the empty client set is vacuously true, so any
    /// tombstone this runs against is dropped — harmless, since a later joiner gets
    /// a live-only baseline. `ingest_tick` calls this after recording despawns, so
    /// entities that died while no clients were registered do not produce stale
    /// despawns for later joiners.
    ///
    /// This is the bound on cumulative-despawn state: without it `self.tombstones`
    /// (and the per-snapshot encode loop over it) grows forever across a session's
    /// join/leave/despawn churn, since ids never recycle to trigger the
    /// reappearance-based removal in `ingest_tick`.
    ///
    /// Cost is O(tombstones × clients) per call; both stay small in practice —
    /// tombstones drain as soon as all clients ack them, and acks arrive frequently
    /// on the reliable-ordered `Channel::Input`.
    fn prune_acked_tombstones(&mut self) {
        let clients = &self.clients;
        let drained: Vec<u32> = self
            .tombstones
            .iter()
            .filter_map(|(network_id, tombstone)| {
                let acked_by_all = clients.values().all(|client| {
                    client
                        .acked_tombstones
                        .get(network_id)
                        .copied()
                        .unwrap_or(0)
                        >= tombstone.tombstone_id
                });
                acked_by_all.then_some(*network_id)
            })
            .collect();
        for network_id in drained {
            self.tombstones.remove(&network_id);
            for client in self.clients.values_mut() {
                client.excluded_entities.remove(&network_id);
                client.sent_cursors.remove(&network_id);
            }
        }
    }

    /// Ingest the engine-produced owned snapshots for this server tick. Compares
    /// each entity's wire mirrors against the tracked state: an unchanged entity
    /// keeps its baseline id; a changed (or newly seen) entity is assigned a fresh
    /// baseline id. Entities present last tick but absent now are despawned (a fresh
    /// tombstone id is allocated) so the despawn replicates and resends until acked.
    ///
    /// Dirty detection is by **wire-mirror value equality**, not byte diffing.
    pub fn ingest_tick(&mut self, snapshots: Vec<EntitySnapshot>) {
        let mut seen: HashSet<u32> = HashSet::with_capacity(snapshots.len());
        for snap in snapshots {
            seen.insert(snap.network_id);
            // A re-appearing entity clears any pending tombstone for its id.
            self.tombstones.remove(&snap.network_id);
            self.pending_despawn_reasons.remove(&snap.network_id);
            match self.entities.get_mut(&snap.network_id) {
                Some(existing)
                    if existing.components == snap.components
                        && existing.active_weapon_archetype == snap.active_weapon_archetype
                        && existing.projectile_presentation == snap.projectile_presentation
                        && existing.presentation_binding == snap.presentation_binding =>
                {
                    // Unchanged pose/movement mirrors: keep the baseline id so acked
                    // clients are omitted (the owner's pawn is still resent when its
                    // cursor advances, `encode::own_cursor_advanced`). The authority
                    // metadata still refreshes —
                    // the resolved cursor advances every tick a command resolves even
                    // when the pawn did not visibly move, and the metadata rides the
                    // record without bumping the baseline. Active-weapon identity is
                    // deliberately part of the condition above: equip changes must
                    // reach remote presentation even when pose is unchanged.
                    existing.owner_client_id = snap.owner_client_id;
                    existing.last_processed_client_tick = snap.last_processed_client_tick;
                    existing.entity_class = snap.entity_class;
                    existing.active_weapon_archetype = snap.active_weapon_archetype;
                    existing.projectile_presentation = snap.projectile_presentation;
                    existing.presentation_binding = snap.presentation_binding;
                }
                Some(existing) => {
                    existing.baseline_id = self.next_baseline_id;
                    existing.components = snap.components;
                    existing.owner_client_id = snap.owner_client_id;
                    existing.last_processed_client_tick = snap.last_processed_client_tick;
                    existing.entity_class = snap.entity_class;
                    existing.active_weapon_archetype = snap.active_weapon_archetype;
                    existing.projectile_presentation = snap.projectile_presentation;
                    existing.presentation_binding = snap.presentation_binding;
                    self.next_baseline_id = self.next_baseline_id.wrapping_add(1);
                }
                None => {
                    let baseline_id = self.next_baseline_id;
                    self.next_baseline_id = self.next_baseline_id.wrapping_add(1);
                    self.entities.insert(
                        snap.network_id,
                        EntityState {
                            baseline_id,
                            components: snap.components,
                            owner_client_id: snap.owner_client_id,
                            last_processed_client_tick: snap.last_processed_client_tick,
                            entity_class: snap.entity_class,
                            active_weapon_archetype: snap.active_weapon_archetype,
                            projectile_presentation: snap.projectile_presentation,
                            presentation_binding: snap.presentation_binding,
                        },
                    );
                }
            }
        }

        // Entities that vanished this tick become despawns. Collect first to avoid
        // mutating `entities` while iterating it.
        let vanished: Vec<u32> = self
            .entities
            .keys()
            .copied()
            .filter(|id| !seen.contains(id))
            .collect();
        for id in vanished {
            self.entities.remove(&id);
            let tombstone_id = self.next_tombstone_id;
            self.next_tombstone_id = self.next_tombstone_id.wrapping_add(1);
            let reason = self.pending_despawn_reasons.remove(&id).unwrap_or(0);
            self.tombstones.insert(
                id,
                Tombstone {
                    tombstone_id,
                    reason,
                },
            );
            // A recipient excluded before the entity's first baseline has no
            // mapping to tear down. Mark this tombstone as already acknowledged
            // for that client instead of sending an unknown-id despawn forever.
            for client in self.clients.values_mut() {
                client.in_flight_baselines.remove(&id);
                if client.excluded_entities.contains(&id) {
                    client.acked_tombstones.insert(id, tombstone_id);
                }
            }
        }
        self.prune_acked_tombstones();
    }

    /// Apply a client ack. Advances each named entity's per-client baseline only if
    /// the acked id is **newer** than the recorded one; retires each named
    /// tombstone. Omitted entries leave prior state unchanged. A `network_id` the
    /// server does not know, or a stale (older/equal) id, is ignored — never a
    /// panic. Unknown clients are ignored.
    pub fn apply_ack(
        &mut self,
        client_id: u64,
        latest_snapshot_sequence: u32,
        entity_baselines: &[(u32, u32)],
        despawn_tombstones: &[(u32, u32)],
    ) {
        let Some(state) = self.clients.get_mut(&client_id) else {
            return;
        };
        // Sequence advances monotonically; an out-of-order/old ack does not regress
        // it (the per-entity baselines below carry the real progress regardless).
        if latest_snapshot_sequence > state.last_acked_sequence {
            state.last_acked_sequence = latest_snapshot_sequence;
        }
        // A client cannot legitimately ack an id the server never issued. The
        // allocators name the *next* id to hand out, so the highest issued id is
        // `next_* - 1`; anything strictly above that is a forged or buggy ack.
        // Accepting it would poison this client's per-entity gate permanently: a
        // `tombstone_id = u32::MAX` ack makes the encode gate
        // `acked >= tombstone.tombstone_id` skip every later real despawn, and an
        // out-of-range `baseline_id` ack forces a refresh round-trip on every change
        // until the real counter climbs past it. Clamp by ignoring out-of-range ids
        // while keeping monotonic-advance-only for in-range ones.
        let highest_baseline = self.next_baseline_id.wrapping_sub(1);
        let highest_tombstone = self.next_tombstone_id.wrapping_sub(1);
        for &(network_id, baseline_id) in entity_baselines {
            if baseline_id > highest_baseline {
                // Forged/buggy: server never issued this baseline id. Ignore it
                // entirely — no state advance, no refresh satisfaction.
                continue;
            }
            let entry = state.acked_baselines.entry(network_id).or_insert(0);
            // Monotonic: only a newer baseline advances. An older/equal ack is a
            // stale or duplicate packet and is ignored.
            if baseline_id > *entry {
                *entry = baseline_id;
            }
            // A baseline ack for this entity also satisfies any pending refresh that
            // named an older-or-equal missing ref: the client now holds a baseline.
            state
                .pending_refreshes
                .retain(|&(nid, missing_ref)| !(nid == network_id && missing_ref <= baseline_id));
        }
        for &(network_id, tombstone_id) in despawn_tombstones {
            if tombstone_id > highest_tombstone {
                // Forged/buggy: server never issued this tombstone id. Ignore it so a
                // later real despawn still reaches this client.
                continue;
            }
            let entry = state.acked_tombstones.entry(network_id).or_insert(0);
            if tombstone_id > *entry {
                *entry = tombstone_id;
            }
        }

        // A qualifying snapshot ack closes the window even when the packet or
        // this record was lost. Preserve additive older baseline progress: it can
        // support a delta; otherwise the newest state becomes a pending full.
        state.in_flight_baselines.retain(|network_id, flight| {
            if latest_snapshot_sequence < flight.sent_sequence {
                return true;
            }
            let covered = state.acked_baselines.get(network_id).copied().unwrap_or(0)
                >= flight.sent_baseline_id;
            if !covered {
                // No flight state may suppress the next repair. The ack map alone
                // decides whether that repair is a full baseline or a delta.
                if state.acked_baselines.get(network_id) == Some(&0) {
                    state.acked_baselines.remove(network_id);
                }
            }
            false
        });

        // This client may have just acked the last outstanding tombstone; once every
        // registered client has acked a tombstone it can leave the global map.
        self.prune_acked_tombstones();
    }

    /// Queue a baseline-refresh request from a client. Additive and idempotent —
    /// keyed by `(network_id, missing_baseline_ref)`, so a duplicate request queues
    /// the same `FullBaseline` once. Unknown clients are ignored; an unknown
    /// `network_id` is queued anyway and simply produces nothing at encode time if
    /// the entity is gone (never a panic).
    pub fn request_refresh(&mut self, client_id: u64, network_id: u32, missing_baseline_ref: u32) {
        if let Some(state) = self.clients.get_mut(&client_id) {
            state
                .pending_refreshes
                .insert((network_id, missing_baseline_ref));
        }
    }

    /// Baselines deferred by the last encoding for this client, excluding flights.
    #[must_use]
    pub fn deferred_baseline_count(&self, client_id: u64) -> usize {
        self.clients
            .get(&client_id)
            .map_or(0, |state| state.deferred_baselines)
    }

    /// Allocate the next monotonic snapshot sequence. Called once per emitted
    /// 20 Hz snapshot batch so every client's snapshot in that batch shares a
    /// sequence.
    fn next_sequence(&mut self) -> u32 {
        let seq = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        seq
    }

    /// Encode the per-client delta snapshot for `client_id` at `server_tick`, or
    /// `None` for an unregistered client. Allocates and stamps a fresh `sequence`.
    ///
    /// Per entity: omit if the client's acked baseline already matches the current
    /// one, unless it is this client's movement pawn and its resolved cursor moved
    /// past the last one stamped for it (then resend at the held baseline); emit a `FullBaseline` if the client holds no acked baseline (or a refresh
    /// was requested for it); otherwise emit a `Delta` from the client's acked
    /// baseline to the current one. Active despawn tombstones the client has not
    /// acked go first as `Despawn` records and resend until acked. Pending full
    /// baselines follow held updates in stable priority order, within the byte
    /// budget, and remain in flight until a qualifying snapshot acknowledgement.
    ///
    /// Returns the raw envelope ready for `wire::encode`. The same `sequence` should
    /// be shared across all clients in one 20 Hz batch by allocating it once via the
    /// batch helper [`ServerReplication::begin_batch`]; this single-client form
    /// allocates its own sequence for callers replicating one client at a time.
    #[must_use]
    pub fn encode_for_client(
        &mut self,
        client_id: u64,
        server_tick: u32,
    ) -> Option<RawSnapshotMessage> {
        let sequence = self.next_sequence();
        self.encode_for_client_with_sequence(client_id, server_tick, sequence)
    }

    /// Begin a 20 Hz batch: allocate the one `sequence` every client in this batch
    /// shares, then encode each client with [`ServerReplication::encode_in_batch`].
    /// Monotonic per server.
    #[must_use]
    pub fn begin_batch(&mut self) -> u32 {
        self.next_sequence()
    }

    /// Encode one client's snapshot within a batch whose `sequence` was allocated by
    /// [`ServerReplication::begin_batch`]. `None` for an unregistered client.
    #[must_use]
    pub fn encode_in_batch(
        &mut self,
        client_id: u64,
        server_tick: u32,
        sequence: u32,
    ) -> Option<RawSnapshotMessage> {
        self.encode_for_client_with_sequence(client_id, server_tick, sequence)
    }
}

/// Re-encode a typed [`ComponentPayload`] into its raw wire envelope form. Mirror of
/// the engine glue's `payload_to_raw`; kept here so the tracker is self-contained.
fn raw_from_payload(payload: &ComponentPayload) -> RawComponentPayload {
    match payload {
        ComponentPayload::TeleporterState(state) => RawComponentPayload {
            component_kind: crate::wire::COMPONENT_KIND_TELEPORTER,
            teleporter: Some(*state),
            transform: None,
            player_movement: None,
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
        },
        ComponentPayload::Transform(t) => RawComponentPayload {
            teleporter: None,
            component_kind: COMPONENT_KIND_TRANSFORM,
            transform: Some(*t),
            player_movement: None,
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
        },
        ComponentPayload::PlayerMovementState(m) => RawComponentPayload {
            teleporter: None,
            component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
            transform: None,
            player_movement: Some(m.clone()),
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
        },
        ComponentPayload::MeshAnimationState(m) => RawComponentPayload {
            teleporter: None,
            component_kind: COMPONENT_KIND_MESH_ANIMATION_STATE,
            transform: None,
            player_movement: None,
            mesh_animation_state: Some(m.clone()),
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: None,
        },
        ComponentPayload::KinematicMoverState(m) => RawComponentPayload {
            teleporter: None,
            component_kind: COMPONENT_KIND_KINEMATIC_MOVER_STATE,
            transform: None,
            player_movement: None,
            mesh_animation_state: None,
            kinematic_mover: Some(*m),
            light: None,
            emitter: None,
            fog: None,
        },
        ComponentPayload::LightState(state) => RawComponentPayload {
            teleporter: None,
            component_kind: crate::wire::COMPONENT_KIND_LIGHT,
            transform: None,
            player_movement: None,
            mesh_animation_state: None,
            kinematic_mover: None,
            light: Some(state.clone()),
            emitter: None,
            fog: None,
        },
        ComponentPayload::EmitterState(state) => RawComponentPayload {
            teleporter: None,
            component_kind: crate::wire::COMPONENT_KIND_BILLBOARD_EMITTER,
            transform: None,
            player_movement: None,
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: Some(state.clone()),
            fog: None,
        },
        ComponentPayload::FogState(state) => RawComponentPayload {
            teleporter: None,
            component_kind: crate::wire::COMPONENT_KIND_FOG_VOLUME,
            transform: None,
            player_movement: None,
            mesh_animation_state: None,
            kinematic_mover: None,
            light: None,
            emitter: None,
            fog: Some(state.clone()),
        },
    }
}

/// Convenience for tests / callers that want the typed view of an entity's records
/// in a freshly-encoded snapshot. Validates the raw envelope and returns its typed
/// records; a malformed envelope (never produced by [`ServerReplication`]) returns
/// an empty vec.
#[must_use]
pub fn typed_records(snapshot: &RawSnapshotMessage) -> Vec<EntityRecord> {
    snapshot
        .validate()
        .map(|typed| typed.records)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{WireGroundRef, WireMovementState, WirePlayerMovementState, WireTransform};

    const CLIENT_A: u64 = 1;
    const CLIENT_B: u64 = 2;

    #[test]
    fn forgetting_bound_entity_clears_all_delivery_state_without_tombstone() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.register_client(CLIENT_B);
        server.ingest_tick(vec![
            entity(7, vec![transform(0.0)]),
            entity(8, vec![transform(1.0)]),
        ]);
        let baseline = server.current_baseline_id(7).unwrap();
        server.encode_for_client(CLIENT_A, 1).unwrap();
        server.set_next_despawn_reason(7, 1);
        // Create a tombstone before populating every delivery-state seam.
        server.ingest_tick(vec![entity(8, vec![transform(1.0)])]);
        assert!(server.tombstones.contains_key(&7));
        server.set_next_despawn_reason(7, 1);
        for state in server.clients.values_mut() {
            state.acked_baselines.insert(7, baseline);
            state.acked_tombstones.insert(7, 12);
            state.pending_refreshes.insert((7, baseline));
            state.excluded_entities.insert(7);
            state.in_flight_baselines.insert(
                7,
                encode::BaselineFlight {
                    sent_sequence: 1,
                    sent_baseline_id: baseline,
                },
            );
        }
        server.forget_entity_without_tombstone(7);
        server.ingest_tick(vec![entity(8, vec![transform(1.0)])]);
        assert!(!server.entities.contains_key(&7));
        assert!(!server.tombstones.contains_key(&7));
        assert!(!server.pending_despawn_reasons.contains_key(&7));
        for state in server.clients.values() {
            assert!(!state.acked_baselines.contains_key(&7));
            assert!(!state.acked_tombstones.contains_key(&7));
            assert!(!state.pending_refreshes.iter().any(|(id, _)| *id == 7));
            assert!(!state.excluded_entities.contains(&7));
            assert!(!state.in_flight_baselines.contains_key(&7));
        }
        assert!(
            server
                .encode_for_client(CLIENT_A, 2)
                .unwrap()
                .records
                .iter()
                .all(|record| record.network_id != 7)
        );
        assert!(server.current_baseline_id(8).is_some());
    }

    fn transform(x: f32) -> ComponentPayload {
        ComponentPayload::Transform(WireTransform {
            position: [x, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        })
    }

    fn movement(velocity_x: f32) -> ComponentPayload {
        ComponentPayload::PlayerMovementState(WirePlayerMovementState {
            teleport: Default::default(),
            knockback_velocity: [0.0; 3],
            push_velocity: [0.0; 3],
            velocity: [velocity_x, 0.0, 0.0],
            ground: WireGroundRef::World,
            air_jumps_remaining: 1,
            air_dashes_remaining: 1,
            dash_cooldown_ms: 0.0,
            air_ticks: 0,
            movement_state: WireMovementState::Normal,
            coyote_timer_ms: 0.0,
            jump_buffer_timer_ms: 0.0,
            jump_spent: false,
            swim_reentry_latched: false,
            capsule_half_height: 0.8,
            capsule_eye_height: 1.5,
            aim_pitch: 0.0,
        })
    }

    fn entity(network_id: u32, components: Vec<ComponentPayload>) -> EntitySnapshot {
        EntitySnapshot::unowned(network_id, components)
    }

    /// An owned movement pawn snapshot: a `PlayerMovementState`-carrying entity with
    /// an `owner_client_id` and resolved cursor, the shape the engine host glue
    /// produces for a networked movement pawn.
    fn owned_movement(
        network_id: u32,
        owner: u64,
        last_tick: Option<u32>,
        velocity_x: f32,
    ) -> EntitySnapshot {
        EntitySnapshot {
            network_id,
            components: vec![transform(0.0), movement(velocity_x)],
            owner_client_id: Some(owner),
            last_processed_client_tick: last_tick,
            entity_class: Some("player".to_string()),
            active_weapon_archetype: None,
            projectile_presentation: None,
            presentation_binding: None,
        }
    }

    // Helper: find a typed record for a network_id in an encoded snapshot.
    fn record_for(snapshot: &RawSnapshotMessage, network_id: u32) -> Option<EntityRecord> {
        typed_records(snapshot).into_iter().find(|r| match r {
            EntityRecord::FullBaseline { network_id: n, .. } => *n == network_id,
            EntityRecord::Delta { network_id: n, .. } => *n == network_id,
            EntityRecord::Despawn { network_id: n, .. } => *n == network_id,
        })
    }

    #[test]
    fn presentation_binding_survives_baseline_delta_and_refresh() {
        let binding = WirePresentationBinding {
            kind: 2,
            index: 8,
            origin: [1.0, 2.0, 3.0],
        };
        let mut current = entity(
            1,
            vec![ComponentPayload::EmitterState(
                crate::wire::WireEmitterState {
                    rate: 1.0,
                    spin_rate: 0.0,
                    spin_animation: None,
                    anim_start: None,
                },
            )],
        );
        current.presentation_binding = Some(binding);
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![current.clone()]);
        let first = server.encode_for_client(CLIENT_A, 1).unwrap();
        let EntityRecord::FullBaseline {
            baseline_id,
            presentation_binding: Some(received),
            ..
        } = record_for(&first, 1).unwrap()
        else {
            panic!("bound baseline");
        };
        assert_eq!(received, binding);
        server.apply_ack(CLIENT_A, first.sequence, &[(1, baseline_id)], &[]);
        current.components[0] = ComponentPayload::EmitterState(crate::wire::WireEmitterState {
            rate: 2.0,
            spin_rate: 0.0,
            spin_animation: None,
            anim_start: None,
        });
        server.ingest_tick(vec![current]);
        let delta = server.encode_for_client(CLIENT_A, 2).unwrap();
        assert!(
            matches!(record_for(&delta, 1), Some(EntityRecord::Delta { presentation_binding: Some(received), .. }) if received == binding)
        );
        server.request_refresh(CLIENT_A, 1, baseline_id);
        let refresh = server.encode_for_client(CLIENT_A, 3).unwrap();
        assert!(
            matches!(record_for(&refresh, 1), Some(EntityRecord::FullBaseline { presentation_binding: Some(received), .. }) if received == binding)
        );
    }

    #[test]
    fn projectile_presentation_facts_survive_late_seed_refresh_and_dirty_delta() {
        let facts = WireProjectilePresentation {
            sprite_size: Some(0.4),
            model_scale: 2.0,
            shot_id: Some(crate::wire::WireShotId {
                pawn: 41,
                start_tick: 12,
                lane: 1,
                ordinal: 2,
            }),
        };
        let mut current = entity(1, vec![transform(0.0)]);
        current.entity_class = Some("projectile:plasma".into());
        current.projectile_presentation = Some(facts);
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![current.clone()]);
        let first = server.encode_for_client(CLIENT_A, 60).unwrap();
        let EntityRecord::FullBaseline {
            baseline_id,
            projectile_presentation: Some(received),
            presentation_binding: None,
            ..
        } = record_for(&first, 1).unwrap()
        else {
            panic!("frozen seed facts");
        };
        assert!((received.sprite_size.unwrap() - 0.4).abs() < 1.0e-6);
        assert_eq!(received.shot_id, facts.shot_id);
        server.apply_ack(CLIENT_A, first.sequence, &[(1, baseline_id)], &[]);
        current
            .projectile_presentation
            .as_mut()
            .unwrap()
            .sprite_size = Some(0.6);
        server.ingest_tick(vec![current]);
        let changed = server.encode_for_client(CLIENT_A, 61).unwrap();
        let EntityRecord::Delta {
            new_baseline_id,
            projectile_presentation: Some(received),
            presentation_binding: None,
            ..
        } = record_for(&changed, 1).unwrap()
        else {
            panic!("scalar change dirties entity");
        };
        assert!((received.sprite_size.unwrap() - 0.6).abs() < 1.0e-6);
        server.apply_ack(CLIENT_A, changed.sequence, &[(1, new_baseline_id)], &[]);
        server.register_client(CLIENT_B);
        let late = server.encode_for_client(CLIENT_B, 62).unwrap();
        let EntityRecord::FullBaseline {
            projectile_presentation: Some(late),
            presentation_binding: None,
            ..
        } = record_for(&late, 1).unwrap()
        else {
            panic!("late full seed");
        };
        assert!((late.sprite_size.unwrap() - 0.6).abs() < 1.0e-6);
        assert_eq!(late.shot_id, facts.shot_id);
        server.request_refresh(CLIENT_A, 1, 0);
        let refresh = server.encode_for_client(CLIENT_A, 63).unwrap();
        assert!(matches!(
            record_for(&refresh, 1),
            Some(EntityRecord::FullBaseline {
                projectile_presentation: Some(_),
                presentation_binding: None,
                ..
            })
        ));
    }

    // First snapshot for a fresh client is a FullBaseline for every entity.
    #[test]
    fn first_snapshot_is_full_baseline_for_each_entity() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![
            entity(1, vec![transform(0.0)]),
            entity(2, vec![transform(5.0)]),
        ]);

        let snap = server
            .encode_for_client(CLIENT_A, 60)
            .expect("registered client");
        let records = typed_records(&snap);
        assert_eq!(records.len(), 2);
        for record in records {
            assert!(matches!(record, EntityRecord::FullBaseline { .. }));
        }
    }

    // An acked entity whose baseline is unchanged is omitted from the next snapshot.
    #[test]
    fn acked_unchanged_entity_is_omitted() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);

        let snap1 = server.encode_for_client(CLIENT_A, 60).unwrap();
        // Pull the baseline id the client must ack from the FullBaseline.
        let EntityRecord::FullBaseline { baseline_id, .. } =
            record_for(&snap1, 1).expect("entity 1 present")
        else {
            panic!("expected full baseline");
        };
        server.apply_ack(CLIENT_A, snap1.sequence, &[(1, baseline_id)], &[]);

        // Same state next tick: nothing changed, so the entity is omitted.
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);
        let snap2 = server.encode_for_client(CLIENT_A, 61).unwrap();
        assert!(
            record_for(&snap2, 1).is_none(),
            "acked unchanged entity omitted"
        );
    }

    // A dirty entity (changed wire mirror) re-sends as a Delta against the acked
    // baseline.
    #[test]
    fn dirty_entity_resends_as_delta() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);
        let snap1 = server.encode_for_client(CLIENT_A, 60).unwrap();
        let EntityRecord::FullBaseline { baseline_id, .. } = record_for(&snap1, 1).unwrap() else {
            panic!("expected full baseline");
        };
        server.apply_ack(CLIENT_A, snap1.sequence, &[(1, baseline_id)], &[]);

        // Entity moves: new baseline, must re-send as a Delta from the acked id.
        server.ingest_tick(vec![entity(1, vec![transform(9.0)])]);
        let snap2 = server.encode_for_client(CLIENT_A, 61).unwrap();
        match record_for(&snap2, 1).expect("dirty entity present") {
            EntityRecord::Delta {
                baseline_ref,
                new_baseline_id,
                ..
            } => {
                assert_eq!(baseline_ref, baseline_id, "delta refs the acked baseline");
                assert_ne!(
                    new_baseline_id, baseline_id,
                    "delta carries a fresh baseline"
                );
            }
            other => panic!("expected Delta, got {other:?}"),
        }
    }

    // Wire-mirror dirty detection: a no-op re-ingest of equal mirrors does not
    // allocate a new baseline (so an acked client stays omitted), but a changed
    // movement field does.
    #[test]
    fn dirty_is_defined_by_wire_mirror_equality() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![entity(1, vec![movement(1.0)])]);
        let snap1 = server.encode_for_client(CLIENT_A, 60).unwrap();
        let EntityRecord::FullBaseline { baseline_id, .. } = record_for(&snap1, 1).unwrap() else {
            panic!("full baseline");
        };
        server.apply_ack(CLIENT_A, snap1.sequence, &[(1, baseline_id)], &[]);

        // Re-ingest equal mirrors: omitted.
        server.ingest_tick(vec![entity(1, vec![movement(1.0)])]);
        let snap2 = server.encode_for_client(CLIENT_A, 61).unwrap();
        assert!(record_for(&snap2, 1).is_none(), "equal mirrors -> omitted");

        // Change a movement field: dirty -> delta.
        server.ingest_tick(vec![entity(1, vec![movement(2.0)])]);
        let snap3 = server.encode_for_client(CLIENT_A, 62).unwrap();
        assert!(
            matches!(record_for(&snap3, 1), Some(EntityRecord::Delta { .. })),
            "changed mirror -> delta"
        );
    }

    // Lost packet only affects that entity: dropping client A's snapshot (A never
    // acks the new baseline) re-sends only the moved entity, not the whole world.
    #[test]
    fn lost_packet_only_affects_unacked_entity() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![
            entity(1, vec![transform(0.0)]),
            entity(2, vec![transform(0.0)]),
        ]);
        let snap1 = server.encode_for_client(CLIENT_A, 60).unwrap();
        let EntityRecord::FullBaseline {
            baseline_id: b1, ..
        } = record_for(&snap1, 1).unwrap()
        else {
            panic!("fb");
        };
        let EntityRecord::FullBaseline {
            baseline_id: b2, ..
        } = record_for(&snap1, 2).unwrap()
        else {
            panic!("fb");
        };
        // Client acks both baselines.
        server.apply_ack(CLIENT_A, snap1.sequence, &[(1, b1), (2, b2)], &[]);

        // Only entity 1 moves; entity 2 unchanged.
        server.ingest_tick(vec![
            entity(1, vec![transform(3.0)]),
            entity(2, vec![transform(0.0)]),
        ]);
        let snap2 = server.encode_for_client(CLIENT_A, 61).unwrap();
        // Entity 1 re-sends; entity 2 (acked + unchanged) is omitted.
        assert!(matches!(
            record_for(&snap2, 1),
            Some(EntityRecord::Delta { .. })
        ));
        assert!(
            record_for(&snap2, 2).is_none(),
            "unchanged acked entity stays omitted"
        );

        // Simulate snap2 dropped: client A does NOT ack entity 1's new baseline.
        // Entity 1 moves again. Only entity 1 is affected; entity 2 still omitted.
        server.ingest_tick(vec![
            entity(1, vec![transform(7.0)]),
            entity(2, vec![transform(0.0)]),
        ]);
        let snap3 = server.encode_for_client(CLIENT_A, 62).unwrap();
        match record_for(&snap3, 1).expect("entity 1 still unacked -> re-sent") {
            // Still a Delta from the last *acked* baseline (b1), not a global resend.
            EntityRecord::Delta { baseline_ref, .. } => assert_eq!(baseline_ref, b1),
            other => panic!("expected Delta from acked baseline, got {other:?}"),
        }
        assert!(
            record_for(&snap3, 2).is_none(),
            "lost packet did not disturb entity 2"
        );
    }

    // Two clients are independent: B acking does not complete A's flight.
    #[test]
    fn per_client_baselines_are_independent() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.register_client(CLIENT_B);
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);

        let seq = server.begin_batch();
        let snap_a = server.encode_in_batch(CLIENT_A, 60, seq).unwrap();
        let snap_b = server.encode_in_batch(CLIENT_B, 60, seq).unwrap();
        assert_eq!(
            snap_a.sequence, snap_b.sequence,
            "batch shares one sequence"
        );
        let EntityRecord::FullBaseline { baseline_id, .. } = record_for(&snap_a, 1).unwrap() else {
            panic!("fb");
        };

        // Only B acks. A still has an independent flight, so neither repeats it.
        server.apply_ack(CLIENT_B, seq, &[(1, baseline_id)], &[]);
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);
        let waiting_a = server.encode_for_client(CLIENT_A, 61).unwrap();
        assert!(record_for(&waiting_a, 1).is_none());
        assert!(record_for(&server.encode_for_client(CLIENT_B, 62).unwrap(), 1).is_none());
        assert!(!server.baseline_acked_by_all_recipients(1, baseline_id));

        // A acknowledges a later snapshot without coverage: its lost record re-pends,
        // while B's acknowledged entity remains omitted.
        server.apply_ack(CLIENT_A, waiting_a.sequence, &[], &[]);
        assert!(matches!(
            record_for(&server.encode_for_client(CLIENT_A, 63).unwrap(), 1),
            Some(EntityRecord::FullBaseline { .. })
        ));
        assert!(record_for(&server.encode_for_client(CLIENT_B, 64).unwrap(), 1).is_none());
    }

    #[test]
    fn baseline_delivery_gate_waits_for_every_nonexcluded_recipient() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.register_client(CLIENT_B);
        server.ingest_tick(vec![entity(1, vec![transform(3.0)])]);
        let baseline_id = server.current_baseline_id(1).expect("entity is ingested");

        assert!(!server.baseline_acked_by_all_recipients(1, baseline_id));
        server.apply_ack(CLIENT_A, 1, &[(1, baseline_id)], &[]);
        assert!(
            !server.baseline_acked_by_all_recipients(1, baseline_id),
            "one recipient ack cannot retire state still owed to another"
        );

        server.exclude_entity_for_client(CLIENT_B, 1);
        assert!(
            server.baseline_acked_by_all_recipients(1, baseline_id),
            "a recipient excluded before materialization does not hold delivery open"
        );
    }

    // Monotonic ack: a newer baseline advances; an older/equal one is ignored;
    // an omitted entry leaves prior state unchanged.
    #[test]
    fn ack_advances_monotonically() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        // Tick 1, baseline 0.
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);
        let snap1 = server.encode_for_client(CLIENT_A, 60).unwrap();
        let EntityRecord::FullBaseline {
            baseline_id: b0, ..
        } = record_for(&snap1, 1).unwrap()
        else {
            panic!("fb");
        };
        // Ack b0 before changing state so the next record can be a Delta.
        server.apply_ack(CLIENT_A, snap1.sequence, &[(1, b0)], &[]);
        // Move -> baseline 1.
        server.ingest_tick(vec![entity(1, vec![transform(1.0)])]);
        let snap2 = server.encode_for_client(CLIENT_A, 61).unwrap();
        let EntityRecord::Delta {
            new_baseline_id: b1,
            ..
        } = record_for(&snap2, 1).unwrap()
        else {
            panic!("delta");
        };
        assert!(b1 > b0);

        // Ack the newer baseline b1: entity now omitted on unchanged state.
        server.apply_ack(CLIENT_A, snap2.sequence, &[(1, b1)], &[]);
        server.ingest_tick(vec![entity(1, vec![transform(1.0)])]);
        assert!(record_for(&server.encode_for_client(CLIENT_A, 62).unwrap(), 1).is_none());

        // A stale ack for the older baseline b0 must NOT regress the client: the
        // entity stays omitted (still considered to hold b1).
        server.apply_ack(CLIENT_A, snap2.sequence, &[(1, b0)], &[]);
        server.ingest_tick(vec![entity(1, vec![transform(1.0)])]);
        assert!(
            record_for(&server.encode_for_client(CLIENT_A, 63).unwrap(), 1).is_none(),
            "stale older-baseline ack did not regress the client"
        );

        // An ack that omits entity 1 entirely also leaves its state unchanged.
        server.apply_ack(CLIENT_A, snap2.sequence, &[], &[]);
        server.ingest_tick(vec![entity(1, vec![transform(1.0)])]);
        assert!(record_for(&server.encode_for_client(CLIENT_A, 64).unwrap(), 1).is_none());
    }

    // Despawn resends until the client acks the tombstone.
    #[test]
    fn despawn_resends_until_tombstone_acked() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);
        let snap1 = server.encode_for_client(CLIENT_A, 60).unwrap();
        let EntityRecord::FullBaseline { baseline_id, .. } = record_for(&snap1, 1).unwrap() else {
            panic!("fb");
        };
        server.apply_ack(CLIENT_A, snap1.sequence, &[(1, baseline_id)], &[]);

        // Entity vanishes -> despawn.
        server.ingest_tick(vec![]);
        let snap2 = server.encode_for_client(CLIENT_A, 61).unwrap();
        let tombstone_id = match record_for(&snap2, 1).expect("despawn present") {
            EntityRecord::Despawn { tombstone_id, .. } => tombstone_id,
            other => panic!("expected Despawn, got {other:?}"),
        };

        // Not acked: resends next tick (despawn is durable until acked).
        server.ingest_tick(vec![]);
        let snap3 = server.encode_for_client(CLIENT_A, 62).unwrap();
        assert!(
            matches!(record_for(&snap3, 1), Some(EntityRecord::Despawn { .. })),
            "despawn resends until acked"
        );

        // Ack the tombstone: stops resending.
        server.apply_ack(CLIENT_A, snap3.sequence, &[], &[(1, tombstone_id)]);
        server.ingest_tick(vec![]);
        let snap4 = server.encode_for_client(CLIENT_A, 63).unwrap();
        assert!(
            record_for(&snap4, 1).is_none(),
            "acked tombstone stops resending"
        );
    }

    // Regression: an enemy that dies before any client connects must not leave a
    // stale despawn tombstone for the first later joiner.
    #[test]
    fn despawn_recorded_with_zero_clients_is_pruned_before_late_join() {
        let mut server = ServerReplication::new();
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);
        server.ingest_tick(vec![]);
        assert!(
            server.tombstones.is_empty(),
            "zero-client despawn tombstone pruned immediately"
        );

        server.register_client(CLIENT_A);
        let snap = server.encode_for_client(CLIENT_A, 60).unwrap();
        assert!(
            record_for(&snap, 1).is_none(),
            "late joiner does not receive stale despawn"
        );
        assert!(
            typed_records(&snap).is_empty(),
            "late join baseline contains only live entities"
        );
    }

    // Regression: a client joining after a despawn must not receive a tombstone for
    // an entity it never mapped, even while an older client still needs that despawn.
    #[test]
    fn late_joiner_skips_tombstone_pending_for_existing_client() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);

        let snap1 = server.encode_for_client(CLIENT_A, 60).unwrap();
        let EntityRecord::FullBaseline { baseline_id, .. } = record_for(&snap1, 1).unwrap() else {
            panic!("fb");
        };
        server.apply_ack(CLIENT_A, snap1.sequence, &[(1, baseline_id)], &[]);

        server.ingest_tick(vec![]);
        let snap2 = server.encode_for_client(CLIENT_A, 61).unwrap();
        let tombstone_id = match record_for(&snap2, 1).expect("A sees despawn") {
            EntityRecord::Despawn { tombstone_id, .. } => tombstone_id,
            other => panic!("expected Despawn, got {other:?}"),
        };

        server.register_client(CLIENT_B);
        let snap_b = server.encode_for_client(CLIENT_B, 62).unwrap();
        assert!(
            record_for(&snap_b, 1).is_none(),
            "late client B never knew entity 1 and gets no stale despawn"
        );
        assert!(
            typed_records(&snap_b).is_empty(),
            "late client B receives the current live set only"
        );

        let snap_a = server.encode_for_client(CLIENT_A, 63).unwrap();
        assert!(
            matches!(record_for(&snap_a, 1), Some(EntityRecord::Despawn { .. })),
            "existing client A still receives the unacked despawn"
        );

        server.apply_ack(CLIENT_A, snap_a.sequence, &[], &[(1, tombstone_id)]);
        assert!(
            server.tombstones.is_empty(),
            "tombstone prunes once the present-at-despawn client acks"
        );
    }

    // A refresh request queues a FullBaseline for that entity, even when the client
    // already holds (and acked) a baseline — the client lost the data and needs it
    // re-sent in full.
    #[test]
    fn refresh_request_queues_full_baseline() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);
        let snap1 = server.encode_for_client(CLIENT_A, 60).unwrap();
        let EntityRecord::FullBaseline { baseline_id, .. } = record_for(&snap1, 1).unwrap() else {
            panic!("fb");
        };
        server.apply_ack(CLIENT_A, snap1.sequence, &[(1, baseline_id)], &[]);

        // Without a refresh, the unchanged acked entity would be omitted. Request a
        // refresh: it must be re-sent as a FullBaseline.
        server.request_refresh(CLIENT_A, 1, baseline_id);
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);
        let snap2 = server.encode_for_client(CLIENT_A, 61).unwrap();
        assert!(
            matches!(
                record_for(&snap2, 1),
                Some(EntityRecord::FullBaseline { .. })
            ),
            "refresh request forces a FullBaseline"
        );

        // The refresh is one-shot: next snapshot reverts to omitting the acked
        // entity (the FullBaseline carried baseline_id, which the client still holds).
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);
        let snap3 = server.encode_for_client(CLIENT_A, 62).unwrap();
        assert!(record_for(&snap3, 1).is_none(), "refresh is one-shot");
    }

    // A malformed/stale ack never panics: unknown client, unknown entity, and an
    // ack naming ids the server never issued are all absorbed silently.
    #[test]
    fn malformed_and_stale_ack_never_panics() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);

        // Unknown client: ignored, no panic.
        server.apply_ack(999, 5, &[(1, 7)], &[(1, 3)]);
        // Unknown entity ids and absurd baseline/tombstone ids: absorbed.
        server.apply_ack(CLIENT_A, 0, &[(424242, 999999)], &[(424242, 88)]);
        // Refresh for an entity that does not exist: queued, produces nothing.
        server.request_refresh(CLIENT_A, 424242, 0);
        // Encoding still works and does not panic.
        let snap = server
            .encode_for_client(CLIENT_A, 60)
            .expect("client still encodes");
        // The real entity 1 is still a FullBaseline; the bogus acks did not corrupt
        // its state.
        assert!(matches!(
            record_for(&snap, 1),
            Some(EntityRecord::FullBaseline { .. })
        ));
        // The non-existent refreshed entity produces no record.
        assert!(record_for(&snap, 424242).is_none());

        // Encoding an unregistered client returns None, not a panic.
        assert!(server.encode_for_client(12345, 60).is_none());
    }

    // Drive an entity to a despawn tombstone for the given network id, returning the
    // allocated tombstone id. Both clients must already be registered and have acked
    // the entity's baseline so the despawn is the only outstanding work.
    fn spawn_then_despawn(server: &mut ServerReplication, network_id: u32) -> u32 {
        server.ingest_tick(vec![entity(network_id, vec![transform(0.0)])]);
        let snap = server.encode_for_client(CLIENT_A, 60).unwrap();
        let EntityRecord::FullBaseline { baseline_id, .. } = record_for(&snap, network_id).unwrap()
        else {
            panic!("fb");
        };
        // Ack the baseline for every registered client so only the despawn remains.
        let client_ids: Vec<u64> = server.clients.keys().copied().collect();
        for cid in client_ids {
            server.apply_ack(cid, snap.sequence, &[(network_id, baseline_id)], &[]);
        }
        // Entity vanishes -> despawn tombstone.
        server.ingest_tick(vec![]);
        let snap = server.encode_for_client(CLIENT_A, 61).unwrap();
        match record_for(&snap, network_id).expect("despawn present") {
            EntityRecord::Despawn { tombstone_id, .. } => tombstone_id,
            other => panic!("expected Despawn, got {other:?}"),
        }
    }

    // #3: a tombstone leaves the global map only once *every* registered client has
    // acked it. A client that never acks keeps the tombstone alive (and resending).
    #[test]
    fn tombstone_pruned_only_after_all_clients_ack() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.register_client(CLIENT_B);
        let tombstone_id = spawn_then_despawn(&mut server, 1);

        // Two clients, neither acked the tombstone yet: it must stay in the global map
        // and keep resending to both.
        assert_eq!(server.tombstones.len(), 1, "tombstone retained pre-ack");
        assert!(matches!(
            record_for(&server.encode_for_client(CLIENT_A, 62).unwrap(), 1),
            Some(EntityRecord::Despawn { .. })
        ));
        assert!(matches!(
            record_for(&server.encode_for_client(CLIENT_B, 62).unwrap(), 1),
            Some(EntityRecord::Despawn { .. })
        ));

        // Only A acks: B still needs it, so it is NOT pruned and still resends to B.
        server.apply_ack(CLIENT_A, 62, &[], &[(1, tombstone_id)]);
        assert_eq!(
            server.tombstones.len(),
            1,
            "one un-acking client keeps the tombstone alive"
        );
        assert!(
            record_for(&server.encode_for_client(CLIENT_A, 63).unwrap(), 1).is_none(),
            "acked client no longer receives the despawn"
        );
        assert!(
            matches!(
                record_for(&server.encode_for_client(CLIENT_B, 63).unwrap(), 1),
                Some(EntityRecord::Despawn { .. })
            ),
            "un-acked client still receives the despawn"
        );

        // Now B acks too: every registered client has acked -> pruned from the global
        // map. No future client could need it (a join gets a fresh live-only baseline).
        server.apply_ack(CLIENT_B, 63, &[], &[(1, tombstone_id)]);
        assert!(
            server.tombstones.is_empty(),
            "tombstone pruned once all registered clients acked"
        );
    }

    // #3: removing the last un-acking client re-evaluates the global map — a tombstone
    // every *remaining* client has acked is pruned even though the departed client
    // never acked it.
    #[test]
    fn removing_holdout_client_prunes_acked_by_remaining() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.register_client(CLIENT_B);
        let tombstone_id = spawn_then_despawn(&mut server, 1);

        // A acks; B never will. Tombstone stays alive for B.
        server.apply_ack(CLIENT_A, 62, &[], &[(1, tombstone_id)]);
        assert_eq!(server.tombstones.len(), 1, "B still holds it open");

        // B disconnects. Its per-client maps go with its ClientReplicationState, and
        // the global map is re-evaluated: A (the only remaining client) acked it.
        server.remove_client(CLIENT_B);
        assert!(
            !server.clients.contains_key(&CLIENT_B),
            "removed client entry fully dropped"
        );
        assert!(
            server.tombstones.is_empty(),
            "tombstone pruned once acked by all *remaining* clients"
        );
    }

    // #4: a forged ack naming a tombstone id above the server's issued range is
    // ignored — it does not advance per-client state, so a later real despawn still
    // reaches that client.
    #[test]
    fn forged_future_tombstone_ack_is_ignored() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);
        let snap1 = server.encode_for_client(CLIENT_A, 60).unwrap();
        let EntityRecord::FullBaseline { baseline_id, .. } = record_for(&snap1, 1).unwrap() else {
            panic!("fb");
        };
        server.apply_ack(CLIENT_A, snap1.sequence, &[(1, baseline_id)], &[]);

        // Forged ack: a tombstone id for entity 1 far above anything issued. If this
        // advanced acked_tombstones[1], the encode gate would permanently suppress the
        // real despawn below.
        server.apply_ack(CLIENT_A, snap1.sequence, &[], &[(1, u32::MAX)]);

        // Real despawn now happens. It must still reach the client.
        server.ingest_tick(vec![]);
        let snap2 = server.encode_for_client(CLIENT_A, 61).unwrap();
        assert!(
            matches!(record_for(&snap2, 1), Some(EntityRecord::Despawn { .. })),
            "forged future-id tombstone ack did not suppress the real despawn"
        );
    }

    // Regression: rebuilding the tracker on level unload reset snapshot sequence
    // to zero, so a connected client rejected every post-transition snapshot.
    #[test]
    fn level_unload_reset_preserves_connection_scoped_allocators() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);
        let before = server
            .encode_for_client(CLIENT_A, 60)
            .expect("first snapshot");

        server.reset_for_level_unload();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![entity(2, vec![transform(1.0)])]);
        let after = server
            .encode_for_client(CLIENT_A, 61)
            .expect("post-transition snapshot");

        assert!(
            after.sequence > before.sequence,
            "snapshot sequence remains monotonic across level lifetime"
        );
        let EntityRecord::FullBaseline { baseline_id, .. } =
            record_for(&after, 2).expect("new level baseline")
        else {
            panic!("expected full baseline");
        };
        let EntityRecord::FullBaseline {
            baseline_id: old_baseline,
            ..
        } = record_for(&before, 1).expect("old level baseline")
        else {
            panic!("expected full baseline");
        };
        assert!(
            baseline_id > old_baseline,
            "baseline ids remain monotonic across level lifetime"
        );
    }

    // #4: a forged ack naming a baseline id above the issued range is ignored — it
    // does not advance per-client baseline state, so a later real delta still reaches
    // that client (it is not stuck refreshing).
    #[test]
    fn forged_future_baseline_ack_is_ignored() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);
        let snap1 = server.encode_for_client(CLIENT_A, 60).unwrap();
        let EntityRecord::FullBaseline { baseline_id, .. } = record_for(&snap1, 1).unwrap() else {
            panic!("fb");
        };
        server.apply_ack(CLIENT_A, snap1.sequence, &[(1, baseline_id)], &[]);

        // Forged ack: a baseline id for entity 1 far above anything issued. It must be
        // ignored so the client is still recorded as holding the real `baseline_id`.
        server.apply_ack(CLIENT_A, snap1.sequence, &[(1, u32::MAX)], &[]);

        // Entity moves: the client must receive a Delta *from the real acked
        // baseline*, not be wedged or forced into a refresh by the bogus future id.
        server.ingest_tick(vec![entity(1, vec![transform(9.0)])]);
        let snap2 = server.encode_for_client(CLIENT_A, 61).unwrap();
        match record_for(&snap2, 1).expect("entity present") {
            EntityRecord::Delta { baseline_ref, .. } => {
                assert_eq!(
                    baseline_ref, baseline_id,
                    "delta refs the real acked baseline, not the forged id"
                );
            }
            other => panic!("expected Delta, got {other:?}"),
        }
    }

    // #3 soak: after N spawn/despawn cycles where the single client acks every
    // tombstone, the global tombstone map and the per-client maps stay bounded
    // (empty) rather than growing with cumulative despawns.
    #[test]
    fn steady_state_tombstone_map_does_not_grow_unbounded() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);

        // Each cycle uses a fresh, never-recycled network id (session-monotonic, as on
        // the real host), spawns it, despawns it, then acks the despawn.
        for nid in 1..=64u32 {
            server.ingest_tick(vec![entity(nid, vec![transform(0.0)])]);
            let snap = server.encode_for_client(CLIENT_A, 60).unwrap();
            let EntityRecord::FullBaseline { baseline_id, .. } = record_for(&snap, nid).unwrap()
            else {
                panic!("fb");
            };
            server.apply_ack(CLIENT_A, snap.sequence, &[(nid, baseline_id)], &[]);

            // Despawn it.
            server.ingest_tick(vec![]);
            let snap = server.encode_for_client(CLIENT_A, 61).unwrap();
            let EntityRecord::Despawn { tombstone_id, .. } =
                record_for(&snap, nid).expect("despawn")
            else {
                panic!("despawn");
            };
            // Ack the despawn: the sole client now covers this tombstone.
            server.apply_ack(CLIENT_A, snap.sequence, &[], &[(nid, tombstone_id)]);
        }

        // The global tombstone map is empty: every despawn was acked-by-all and
        // pruned. Without the fix it would hold all 64 entries forever.
        assert!(
            server.tombstones.is_empty(),
            "tombstone map bounded once all acked, got {}",
            server.tombstones.len()
        );
        // The entity set is empty too (all despawned).
        assert!(server.entities.is_empty(), "no live entities remain");
        // The per-client acked maps are bounded by the number of distinct ids the
        // client ever acked; they do not grow without bound relative to live state,
        // and crucially the global tombstone map (the per-snapshot encode cost) does
        // not. The acked maps are swept wholesale when the client disconnects.
        server.remove_client(CLIENT_A);
        assert!(server.clients.is_empty(), "client maps swept on disconnect");
    }

    // Per-recipient `local_player` (M15 Phase 3 Task 4): a movement pawn owned by
    // CLIENT_A is encoded `local_player = true` only in CLIENT_A's snapshot and
    // `false` in CLIENT_B's. Derived from id comparison alone — the tracker never
    // sees an EntityId/registry. The owned record also carries the movement payload
    // and the resolved `last_processed_client_tick`.
    #[test]
    fn local_player_true_only_for_owning_recipient() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.register_client(CLIENT_B);
        // CLIENT_A owns network id 1; its resolved cursor is at tick 12.
        server.ingest_tick(vec![owned_movement(1, CLIENT_A, Some(12), 3.0)]);

        let seq = server.begin_batch();
        let snap_a = server.encode_in_batch(CLIENT_A, 60, seq).unwrap();
        let snap_b = server.encode_in_batch(CLIENT_B, 60, seq).unwrap();

        let rec_a = record_for(&snap_a, 1).expect("owner sees its pawn");
        let rec_b = record_for(&snap_b, 1).expect("non-owner sees the pawn too");

        match rec_a {
            EntityRecord::FullBaseline {
                local_player,
                last_processed_client_tick,
                entity_class,
                components,
                ..
            } => {
                assert!(local_player, "owner's snapshot marks the pawn local_player");
                assert_eq!(
                    last_processed_client_tick,
                    Some(12),
                    "owner record carries the resolved cursor"
                );
                assert_eq!(
                    entity_class,
                    Some("player".to_string()),
                    "owner record carries the descriptor class to materialize"
                );
                assert!(
                    components
                        .iter()
                        .any(|c| matches!(c, ComponentPayload::PlayerMovementState(_))),
                    "owner record carries the movement payload"
                );
            }
            other => panic!("expected FullBaseline, got {other:?}"),
        }

        match rec_b {
            EntityRecord::FullBaseline {
                local_player,
                last_processed_client_tick,
                entity_class,
                ..
            } => {
                assert!(
                    !local_player,
                    "a non-owning recipient never sees local_player=true"
                );
                // The resolved cursor and entity_class are echoed to every recipient
                // (pawn state, not owner-private); only `local_player` is
                // recipient-specific.
                assert_eq!(last_processed_client_tick, Some(12));
                assert_eq!(entity_class, Some("player".to_string()));
            }
            other => panic!("expected FullBaseline, got {other:?}"),
        }
    }

    // A movement pawn with no resolved command yet (cursor `None`) encodes
    // `last_processed_client_tick = None` but still derives `local_player` for its
    // owner — the first baseline before any tick resolves.
    #[test]
    fn owned_movement_pawn_with_no_resolved_tick_encodes_none_cursor() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![owned_movement(1, CLIENT_A, None, 0.0)]);

        let snap = server.encode_for_client(CLIENT_A, 60).unwrap();
        match record_for(&snap, 1).expect("pawn present") {
            EntityRecord::FullBaseline {
                local_player,
                last_processed_client_tick,
                ..
            } => {
                assert!(local_player, "owner still marked local before any tick");
                assert_eq!(last_processed_client_tick, None);
            }
            other => panic!("expected FullBaseline, got {other:?}"),
        }
    }

    // The authority metadata is excluded from dirty detection: a tick that only
    // advances the resolved cursor (pose/movement mirrors unchanged) does NOT bump
    // the baseline. The owner gets a metadata-only resend at the held baseline
    // carrying the new cursor, and a forced refresh echoes it too.
    #[test]
    fn resolved_cursor_advance_does_not_bump_baseline() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![owned_movement(1, CLIENT_A, Some(5), 1.0)]);
        let snap1 = server.encode_for_client(CLIENT_A, 60).unwrap();
        let EntityRecord::FullBaseline { baseline_id, .. } = record_for(&snap1, 1).unwrap() else {
            panic!("fb");
        };
        server.apply_ack(CLIENT_A, snap1.sequence, &[(1, baseline_id)], &[]);

        // Cursor advances 5 -> 6 but the movement mirror is identical: no baseline
        // bump, but the owner gets a metadata-only resend at the held baseline
        // carrying the new cursor, so it can reconcile a still pawn.
        server.ingest_tick(vec![owned_movement(1, CLIENT_A, Some(6), 1.0)]);
        let snap2 = server.encode_for_client(CLIENT_A, 61).unwrap();
        match record_for(&snap2, 1).expect("a cursor advance resends the owner's pawn") {
            EntityRecord::Delta {
                baseline_ref,
                new_baseline_id,
                last_processed_client_tick,
                ..
            } => {
                assert_eq!(baseline_ref, baseline_id);
                assert_eq!(new_baseline_id, baseline_id, "no baseline bump");
                assert_eq!(last_processed_client_tick, Some(6));
            }
            other => panic!("expected a metadata-only Delta, got {other:?}"),
        }

        // The same cursor again: nothing new to stamp, so the pawn is omitted.
        server.ingest_tick(vec![owned_movement(1, CLIENT_A, Some(6), 1.0)]);
        let repeat = server.encode_for_client(CLIENT_A, 61).unwrap();
        assert!(
            record_for(&repeat, 1).is_none(),
            "an unchanged cursor and pose omit the pawn"
        );

        // Force a refresh: the re-sent FullBaseline carries the *latest* cursor (6),
        // proving the cursor was tracked even while the baseline held.
        server.request_refresh(CLIENT_A, 1, baseline_id);
        server.ingest_tick(vec![owned_movement(1, CLIENT_A, Some(6), 1.0)]);
        let snap3 = server.encode_for_client(CLIENT_A, 62).unwrap();
        match record_for(&snap3, 1).expect("refresh forces a baseline") {
            EntityRecord::FullBaseline {
                last_processed_client_tick,
                ..
            } => assert_eq!(last_processed_client_tick, Some(6)),
            other => panic!("expected FullBaseline, got {other:?}"),
        }
    }

    // A cursor advance resends a still pawn only to its owner: other recipients do
    // not reconcile it, so they keep the omission.
    #[test]
    fn cursor_advance_resends_a_still_pawn_only_to_its_owner() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.register_client(CLIENT_B);
        server.ingest_tick(vec![owned_movement(1, CLIENT_A, Some(5), 1.0)]);
        for client in [CLIENT_A, CLIENT_B] {
            let snap = server.encode_for_client(client, 60).unwrap();
            let EntityRecord::FullBaseline { baseline_id, .. } = record_for(&snap, 1).unwrap()
            else {
                panic!("fb");
            };
            server.apply_ack(client, snap.sequence, &[(1, baseline_id)], &[]);
        }

        server.ingest_tick(vec![owned_movement(1, CLIENT_A, Some(6), 1.0)]);
        let owner = server.encode_for_client(CLIENT_A, 61).unwrap();
        let other = server.encode_for_client(CLIENT_B, 61).unwrap();
        assert!(
            record_for(&owner, 1).is_some(),
            "the owner gets the new cursor"
        );
        assert!(
            record_for(&other, 1).is_none(),
            "another recipient keeps the omission"
        );
    }

    #[test]
    fn active_weapon_archetype_is_shared_visible_and_dirties_a_movement_baseline() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        let mut pistol = owned_movement(1, CLIENT_A, Some(1), 0.0);
        pistol.active_weapon_archetype = Some("reference_pistol".to_string());
        server.ingest_tick(vec![pistol]);

        let first = server.encode_for_client(CLIENT_A, 60).unwrap();
        let EntityRecord::FullBaseline {
            baseline_id,
            active_weapon_archetype,
            ..
        } = record_for(&first, 1).expect("initial pawn baseline")
        else {
            panic!("expected a full baseline");
        };
        assert_eq!(active_weapon_archetype.as_deref(), Some("reference_pistol"));
        server.apply_ack(CLIENT_A, first.sequence, &[(1, baseline_id)], &[]);

        let mut rifle = owned_movement(1, CLIENT_A, Some(2), 0.0);
        rifle.active_weapon_archetype = Some("pulse_rifle".to_string());
        server.ingest_tick(vec![rifle]);
        let changed = server.encode_for_client(CLIENT_A, 61).unwrap();
        let EntityRecord::Delta {
            active_weapon_archetype,
            ..
        } = record_for(&changed, 1).expect("weapon change produces delta")
        else {
            panic!("expected a delta");
        };
        assert_eq!(active_weapon_archetype.as_deref(), Some("pulse_rifle"));
    }

    /// A host-authoritative map enemy snapshot: a Transform-only entity carrying an
    /// `entity_class` but NO `PlayerMovementState`, unowned by any client — the shape the
    /// E10 host enemy-registration glue produces.
    fn enemy(network_id: u32, class: &str) -> EntitySnapshot {
        EntitySnapshot {
            network_id,
            components: vec![transform(0.0)],
            owner_client_id: None,
            last_processed_client_tick: None,
            entity_class: Some(class.to_string()),
            active_weapon_archetype: None,
            projectile_presentation: None,
            presentation_binding: None,
        }
    }

    // E10 Task 4: `entity_class` rides a non-movement finite-`Transform` record (a host
    // enemy), while `local_player`/resolved-tick stay movement-only — they are withheld on
    // a record without a `PlayerMovementState`. The encoded record validates (the wire
    // accepts `entity_class` on a finite-Transform record since Task 3).
    #[test]
    fn entity_class_rides_transform_record_but_movement_metadata_withheld() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![enemy(1, "grunt")]);

        let snap = server.encode_for_client(CLIENT_A, 60).unwrap();
        // The whole snapshot validates: an entity_class on a finite-Transform record is
        // accepted, so `typed_records` is non-empty.
        match record_for(&snap, 1).expect("the enemy record is present and valid") {
            EntityRecord::FullBaseline {
                entity_class,
                local_player,
                last_processed_client_tick,
                components,
                ..
            } => {
                assert_eq!(
                    entity_class,
                    Some("grunt".to_string()),
                    "the class rides the Transform-only enemy record"
                );
                assert!(
                    !local_player,
                    "a non-movement record never carries local_player"
                );
                assert_eq!(
                    last_processed_client_tick, None,
                    "a non-movement record carries no resolved-tick metadata"
                );
                assert!(
                    components
                        .iter()
                        .all(|c| matches!(c, ComponentPayload::Transform(_))),
                    "the enemy record is Transform-only"
                );
            }
            other => panic!("expected FullBaseline, got {other:?}"),
        }
    }

    // Regression: `Some("")` previously emitted `has_entity_class=true`, which raw
    // validation rejects as empty present metadata.
    #[test]
    fn empty_entity_class_is_withheld_from_finite_transform_record() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![EntitySnapshot {
            network_id: 1,
            components: vec![transform(0.0)],
            owner_client_id: None,
            last_processed_client_tick: None,
            entity_class: Some(String::new()),
            active_weapon_archetype: None,
            projectile_presentation: None,
            presentation_binding: None,
        }]);

        let snap = server.encode_for_client(CLIENT_A, 60).unwrap();
        let raw = snap
            .records
            .iter()
            .find(|record| record.network_id == 1)
            .expect("record present");
        assert!(
            !raw.has_entity_class,
            "empty class is normalized to absent metadata"
        );
        assert!(
            raw.entity_class.is_empty(),
            "absent class leaves the raw string empty"
        );

        let typed = snap
            .validate()
            .expect("producer emits an entity_class shape accepted by validation");
        match typed.records.as_slice() {
            [
                EntityRecord::FullBaseline {
                    network_id,
                    entity_class,
                    components,
                    ..
                },
            ] => {
                assert_eq!(*network_id, 1);
                assert_eq!(entity_class, &None);
                assert!(
                    matches!(components.as_slice(), [ComponentPayload::Transform(_)]),
                    "finite Transform record remains otherwise intact"
                );
            }
            other => panic!("expected one FullBaseline, got {other:?}"),
        }
    }

    #[test]
    fn recipient_entity_exclusion_hides_full_delta_and_despawn_records() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.register_client(CLIENT_B);
        server.ingest_tick(vec![entity(17, vec![transform(0.0)])]);
        server.exclude_entity_for_client(CLIENT_A, 17);

        let hidden = server.encode_for_client(CLIENT_A, 60).unwrap();
        assert!(
            record_for(&hidden, 17).is_none(),
            "the excluded recipient never receives an initial baseline"
        );
        let visible = server.encode_for_client(CLIENT_B, 60).unwrap();
        let EntityRecord::FullBaseline { baseline_id, .. } = record_for(&visible, 17).unwrap()
        else {
            panic!("visible recipient needs a full baseline");
        };
        server.apply_ack(CLIENT_B, visible.sequence, &[(17, baseline_id)], &[]);

        server.ingest_tick(vec![entity(17, vec![transform(1.0)])]);
        assert!(
            record_for(&server.encode_for_client(CLIENT_A, 61).unwrap(), 17).is_none(),
            "the exclusion holds after a dirty update as well"
        );
        assert!(matches!(
            record_for(&server.encode_for_client(CLIENT_B, 61).unwrap(), 17),
            Some(EntityRecord::Delta { .. })
        ));

        server.ingest_tick(vec![]);
        assert!(
            record_for(&server.encode_for_client(CLIENT_A, 62).unwrap(), 17).is_none(),
            "the excluded recipient receives no unknown-id despawn"
        );
        let visible_despawn = server.encode_for_client(CLIENT_B, 62).unwrap();
        let EntityRecord::Despawn { tombstone_id, .. } = record_for(&visible_despawn, 17).unwrap()
        else {
            panic!("visible recipient needs a despawn");
        };
        server.apply_ack(
            CLIENT_B,
            visible_despawn.sequence,
            &[],
            &[(17, tombstone_id)],
        );

        assert!(
            server.tombstones.is_empty(),
            "the exclusion acks its own tombstone"
        );
        assert!(
            !server.clients[&CLIENT_A].excluded_entities.contains(&17),
            "tombstone retirement clears the per-client exclusion entry"
        );
    }

    // E10 Task 4: an entity_class is withheld at production on a record that carries NO
    // finite `Transform` — here a movement-only record (`PlayerMovementState` alone, no
    // Transform). This mirrors the validate gate (`EntityClassWithoutTransform`) on the
    // production side: the host never stamps a class the validator would reject. The
    // record still encodes and validates because no class rides it.
    #[test]
    fn entity_class_withheld_when_no_finite_transform() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        // A movement-only record (no Transform component) that nonetheless carries an
        // entity_class in its tracked state: production must drop the class.
        server.ingest_tick(vec![EntitySnapshot {
            network_id: 1,
            components: vec![movement(1.0)],
            owner_client_id: Some(CLIENT_A),
            last_processed_client_tick: Some(3),
            entity_class: Some("grunt".to_string()),
            active_weapon_archetype: None,
            projectile_presentation: None,
            presentation_binding: None,
        }]);

        let snap = server.encode_for_client(CLIENT_A, 60).unwrap();
        match record_for(&snap, 1).expect("record present and valid") {
            EntityRecord::FullBaseline {
                entity_class,
                local_player,
                ..
            } => {
                assert_eq!(
                    entity_class, None,
                    "no class is stamped without a finite Transform to back it"
                );
                // Movement metadata still rides this movement record for its owner.
                assert!(
                    local_player,
                    "the owner still sees local_player on a movement record"
                );
            }
            other => panic!("expected FullBaseline, got {other:?}"),
        }
    }

    fn join_budget_sized_entity(network_id: u32, encoded_bytes: usize) -> EntitySnapshot {
        let mut snapshot = entity(network_id, vec![transform(network_id as f32)]);
        snapshot.entity_class = Some("x".repeat(encoded_bytes));
        for _ in 0..8 {
            let mut probe = ServerReplication::new();
            probe.register_client(CLIENT_A);
            probe.ingest_tick(vec![snapshot.clone()]);
            let raw = probe.encode_for_client(CLIENT_A, 0).unwrap();
            let actual = crate::wire::encode(&raw.records[0]).len();
            let class = snapshot.entity_class.as_mut().unwrap();
            match actual.cmp(&encoded_bytes) {
                std::cmp::Ordering::Equal => return snapshot,
                std::cmp::Ordering::Greater => {
                    class.truncate(class.len() - (actual - encoded_bytes))
                }
                std::cmp::Ordering::Less => class.push_str(&"x".repeat(encoded_bytes - actual)),
            }
        }
        panic!("could not construct exact encoded record size");
    }

    fn join_budget_bound(network_id: u32) -> EntitySnapshot {
        let mut snapshot = entity(
            network_id,
            vec![ComponentPayload::EmitterState(
                crate::wire::WireEmitterState {
                    rate: network_id as f32,
                    spin_rate: 0.0,
                    spin_animation: None,
                    anim_start: None,
                },
            )],
        );
        snapshot.presentation_binding = Some(WirePresentationBinding {
            kind: 2,
            index: network_id,
            origin: [0.0; 3],
        });
        snapshot
    }

    fn join_budget_ids(snapshot: &RawSnapshotMessage) -> Vec<u32> {
        snapshot
            .records
            .iter()
            .map(|record| record.network_id)
            .collect()
    }

    fn join_budget_ack(server: &mut ServerReplication, snapshot: &RawSnapshotMessage) {
        let baselines: Vec<_> = snapshot
            .records
            .iter()
            .filter_map(|record| match record.record_kind {
                crate::wire::RECORD_KIND_FULL_BASELINE => {
                    Some((record.network_id, record.baseline_id_or_ref))
                }
                crate::wire::RECORD_KIND_DELTA => {
                    Some((record.network_id, record.new_baseline_id_or_tombstone_id))
                }
                _ => None,
            })
            .collect();
        let tombstones: Vec<_> = snapshot
            .records
            .iter()
            .filter(|record| record.record_kind == crate::wire::RECORD_KIND_DESPAWN)
            .map(|record| (record.network_id, record.new_baseline_id_or_tombstone_id))
            .collect();
        server.apply_ack(CLIENT_A, snapshot.sequence, &baselines, &tombstones);
    }

    #[test]
    fn join_budget_first_oversized_is_exempt_and_exact_remaining_budget_is_admitted() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![
            join_budget_sized_entity(1, JOIN_BASELINE_BUDGET_BYTES + 100),
            join_budget_sized_entity(2, JOIN_BASELINE_BUDGET_BYTES),
            entity(3, vec![transform(3.0)]),
        ]);
        let first = server.encode_for_client(CLIENT_A, 0).unwrap();
        assert!(first.validate().is_ok());
        assert_eq!(join_budget_ids(&first), [1, 2]);
        assert_eq!(
            crate::wire::encode(&first.records[1]).len(),
            JOIN_BASELINE_BUDGET_BYTES
        );
        assert_eq!(server.deferred_baseline_count(CLIENT_A), 1);
        let second = server.encode_for_client(CLIENT_A, 1).unwrap();
        assert_eq!(
            join_budget_ids(&second),
            [3],
            "in-flight window does not repeat"
        );
        assert_eq!(server.deferred_baseline_count(CLIENT_A), 0);
    }

    #[test]
    fn join_budget_priority_is_own_pawn_then_unbound_then_bound_in_id_order() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![
            join_budget_bound(2),
            entity(9, vec![transform(9.0)]),
            owned_movement(20, CLIENT_A, Some(1), 0.0),
            join_budget_bound(1),
            owned_movement(8, CLIENT_B, Some(1), 0.0),
            entity(4, vec![transform(4.0)]),
        ]);
        let first = server.encode_for_client(CLIENT_A, 0).unwrap();
        assert_eq!(join_budget_ids(&first), [20, 4, 8, 9, 1, 2]);
        assert!(first.records[0].local_player);
        assert!(!first.records[2].local_player);
    }

    #[test]
    fn join_budget_pending_latest_state_and_despawn_precede_backlog() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        let a = join_budget_sized_entity(1, JOIN_BASELINE_BUDGET_BYTES + 100);
        let b = join_budget_sized_entity(2, JOIN_BASELINE_BUDGET_BYTES + 100);
        let c = join_budget_sized_entity(3, JOIN_BASELINE_BUDGET_BYTES + 100);
        server.ingest_tick(vec![a.clone(), b.clone(), c]);
        let first = server.encode_for_client(CLIENT_A, 0).unwrap();
        assert_eq!(join_budget_ids(&first), [1]);
        let mut newest = b;
        newest.components = vec![transform(42.0)];
        server.ingest_tick(vec![a, newest]);
        let second = server.encode_for_client(CLIENT_A, 1).unwrap();
        assert_eq!(join_budget_ids(&second), [3, 2]);
        assert_eq!(
            second.records[0].record_kind,
            crate::wire::RECORD_KIND_DESPAWN
        );
        assert_eq!(
            second.records[1].record_kind,
            crate::wire::RECORD_KIND_FULL_BASELINE
        );
        assert_eq!(
            second.records[1].components[0].transform.unwrap().position[0],
            42.0
        );
        assert_eq!(
            second
                .records
                .iter()
                .filter(|record| record.network_id == 3)
                .count(),
            1
        );
    }

    #[test]
    fn join_budget_held_deltas_and_forced_refresh_never_charge_pending_budget() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        let mut held_a = join_budget_sized_entity(1, JOIN_BASELINE_BUDGET_BYTES + 100);
        let held_b = join_budget_sized_entity(2, JOIN_BASELINE_BUDGET_BYTES + 100);
        // Seed the two held entities over two windows.
        server.ingest_tick(vec![held_a.clone(), held_b.clone()]);
        let seed_a = server.encode_for_client(CLIENT_A, 0).unwrap();
        join_budget_ack(&mut server, &seed_a);
        let seed_b = server.encode_for_client(CLIENT_A, 1).unwrap();
        join_budget_ack(&mut server, &seed_b);
        held_a.components = vec![transform(99.0)];
        server.ingest_tick(vec![
            held_a,
            held_b,
            join_budget_sized_entity(3, JOIN_BASELINE_BUDGET_BYTES + 100),
            join_budget_sized_entity(4, JOIN_BASELINE_BUDGET_BYTES),
            entity(5, vec![transform(5.0)]),
        ]);
        server.request_refresh(CLIENT_A, 2, 0);
        let raw = server.encode_for_client(CLIENT_A, 2).unwrap();
        assert_eq!(join_budget_ids(&raw), [1, 2, 3, 4]);
        assert_eq!(raw.records[0].record_kind, crate::wire::RECORD_KIND_DELTA);
        assert_eq!(
            raw.records[1].record_kind,
            crate::wire::RECORD_KIND_FULL_BASELINE
        );
        assert_eq!(
            crate::wire::encode(&raw.records[3]).len(),
            JOIN_BASELINE_BUDGET_BYTES
        );
        assert_eq!(server.deferred_baseline_count(CLIENT_A), 1);
    }

    #[test]
    fn join_budget_flight_waits_for_qualifying_sequence_then_repends_latest_after_loss() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        // Ensure an older sequence exists: the first real flight has sequence 1.
        let old = server.encode_for_client(CLIENT_A, 0).unwrap();
        server.ingest_tick(vec![entity(1, vec![transform(1.0)])]);
        let lost = server.encode_for_client(CLIENT_A, 1).unwrap();
        server.ingest_tick(vec![entity(1, vec![transform(2.0)])]);
        server.request_refresh(CLIENT_A, 1, 0);
        server.apply_ack(CLIENT_A, old.sequence, &[], &[]);
        let waiting = server.encode_for_client(CLIENT_A, 2).unwrap();
        assert!(waiting.records.is_empty());
        server.apply_ack(CLIENT_A, waiting.sequence, &[], &[]);
        let repair = server.encode_for_client(CLIENT_A, 3).unwrap();
        assert_eq!(join_budget_ids(&repair), [1]);
        assert_eq!(
            repair.records[0].record_kind,
            crate::wire::RECORD_KIND_FULL_BASELINE
        );
        assert_ne!(
            repair.records[0].baseline_id_or_ref,
            lost.records[0].baseline_id_or_ref
        );
        assert_eq!(
            repair.records[0].components[0].transform.unwrap().position[0],
            2.0
        );
        join_budget_ack(&mut server, &repair);
        assert!(
            server
                .encode_for_client(CLIENT_A, 4)
                .unwrap()
                .records
                .is_empty()
        );
    }

    #[test]
    fn join_budget_older_baseline_ack_cannot_hide_newer_state_or_release_newer_flight() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![entity(1, vec![transform(1.0)])]);
        let old = server.encode_for_client(CLIENT_A, 0).unwrap();
        let old_id = old.records[0].baseline_id_or_ref;
        server.ingest_tick(vec![entity(1, vec![transform(2.0)])]);
        assert!(
            server
                .encode_for_client(CLIENT_A, 1)
                .unwrap()
                .records
                .is_empty()
        );
        server.apply_ack(CLIENT_A, old.sequence, &[(1, old_id)], &[]);
        let newer = server.encode_for_client(CLIENT_A, 2).unwrap();
        assert_eq!(newer.records[0].record_kind, crate::wire::RECORD_KIND_DELTA);
        assert_eq!(newer.records[0].baseline_id_or_ref, old_id);
        assert_eq!(
            newer.records[0].components[0].transform.unwrap().position[0],
            2.0
        );
        join_budget_ack(&mut server, &newer);
        server.apply_ack(CLIENT_A, old.sequence, &[(1, old_id)], &[]);
        assert!(
            server
                .encode_for_client(CLIENT_A, 3)
                .unwrap()
                .records
                .is_empty()
        );
    }

    #[test]
    fn join_budget_late_additive_older_baseline_supports_delta_after_new_flight_loss() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_tick(vec![entity(1, vec![transform(1.0)])]);
        let old = server.encode_for_client(CLIENT_A, 0).unwrap();
        let old_id = old.records[0].baseline_id_or_ref;
        let empty = server.encode_for_client(CLIENT_A, 1).unwrap();
        server.apply_ack(CLIENT_A, empty.sequence, &[], &[]);
        server.ingest_tick(vec![entity(1, vec![transform(2.0)])]);
        let lost_new = server.encode_for_client(CLIENT_A, 2).unwrap();
        server.apply_ack(CLIENT_A, old.sequence, &[(1, old_id)], &[]);
        assert!(
            server
                .encode_for_client(CLIENT_A, 3)
                .unwrap()
                .records
                .is_empty(),
            "stale sequence cannot end newer flight"
        );
        server.apply_ack(CLIENT_A, lost_new.sequence, &[], &[]);
        let repair = server.encode_for_client(CLIENT_A, 4).unwrap();
        assert_eq!(
            repair.records[0].record_kind,
            crate::wire::RECORD_KIND_DELTA
        );
        assert_eq!(repair.records[0].baseline_id_or_ref, old_id);
        assert_eq!(
            repair.records[0].components[0].transform.unwrap().position[0],
            2.0
        );
        join_budget_ack(&mut server, &repair);
        assert!(
            server
                .encode_for_client(CLIENT_A, 5)
                .unwrap()
                .records
                .is_empty()
        );
    }

    #[test]
    fn join_budget_windows_converge_without_flight_repeats() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        let mut snapshots: Vec<_> = (1..=8)
            .map(|id| join_budget_sized_entity(id, JOIN_BASELINE_BUDGET_BYTES))
            .collect();
        server.ingest_tick(snapshots.clone());
        let first = server.encode_for_client(CLIENT_A, 0).unwrap();
        assert_eq!(join_budget_ids(&first), [1, 2]);
        snapshots.push(entity(20, vec![transform(20.0)]));
        server.ingest_tick(snapshots);
        let second = server.encode_for_client(CLIENT_A, 1).unwrap();
        assert_eq!(join_budget_ids(&second), [3, 4]);
        join_budget_ack(&mut server, &first);
        join_budget_ack(&mut server, &second);
        let mut observed: HashSet<_> = first
            .records
            .iter()
            .chain(&second.records)
            .map(|r| r.network_id)
            .collect();
        for tick in 2..8 {
            let raw = server.encode_for_client(CLIENT_A, tick).unwrap();
            if raw.records.is_empty() {
                break;
            }
            for record in &raw.records {
                assert!(
                    observed.insert(record.network_id),
                    "already-admitted baseline repeated"
                );
            }
            join_budget_ack(&mut server, &raw);
        }
        assert_eq!(observed.len(), 9);
        assert!(
            server
                .encode_for_client(CLIENT_A, 9)
                .unwrap()
                .records
                .is_empty()
        );
    }

    #[test]
    fn join_budget_later_unbound_entity_passes_earlier_bound_backlog() {
        let mut server = ServerReplication::new();
        server.register_client(CLIENT_A);
        let mut snapshots: Vec<_> = (1..=1200).map(join_budget_bound).collect();
        server.ingest_tick(snapshots.clone());
        let first = server.encode_for_client(CLIENT_A, 0).unwrap();
        assert!(server.deferred_baseline_count(CLIENT_A) > 0);
        let first_ids: HashSet<_> = join_budget_ids(&first).into_iter().collect();
        snapshots.push(entity(1201, vec![transform(1201.0)]));
        server.ingest_tick(snapshots);
        let second = server.encode_for_client(CLIENT_A, 1).unwrap();
        assert_eq!(second.records[0].network_id, 1201);
        assert!(second.records[0].presentation_binding.is_none());
        assert!(
            second.records[1..]
                .iter()
                .all(|r| r.presentation_binding.is_some() && !first_ids.contains(&r.network_id))
        );
        assert!(
            second.records[1..]
                .windows(2)
                .all(|pair| pair[0].network_id < pair[1].network_id)
        );
    }
}
