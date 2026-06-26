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
// entity's next encoding. A newly dirty/unacked entity gets a `Delta` (or a
// `FullBaseline` when the client holds no acked baseline for it); an entity whose
// acked baseline already matches the current one is omitted. Despawn tombstones
// resend until the client acks them. Acks advance per-client state monotonically:
// a newer baseline-id advances, an older one is ignored, and an omitted entry
// leaves prior state unchanged. None of this path panics on a stale or malformed
// ack/refresh — a `NetworkId` the server does not know is simply ignored.

use std::collections::{HashMap, HashSet};

use crate::wire::{
    COMPONENT_KIND_MESH_ANIMATION_STATE, COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
    COMPONENT_KIND_TRANSFORM, ComponentPayload, EntityRecord, RECORD_KIND_DELTA,
    RECORD_KIND_DESPAWN, RECORD_KIND_FULL_BASELINE, RawComponentPayload, RawEntityRecord,
    RawSnapshotMessage, SNAPSHOT_VERSION,
};

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
        }
    }
}

/// Despawn reason carried in a tombstone record. A `u8` on the wire; the tracker
/// keeps it so a resent despawn carries the original reason.
pub type DespawnReason = u8;

/// Per-client replication state the server tracks: how far this client has acked,
/// which baseline it holds per entity, which despawn tombstones it has applied, and
/// any pending baseline-refresh requests to satisfy in its next snapshot.
#[derive(Debug, Default)]
struct ClientReplicationState {
    /// Highest snapshot sequence this client has acked (diagnostic; the per-entity
    /// baseline map is the authoritative progress record).
    last_acked_sequence: u32,
    /// `network_id -> acked baseline_id`. The baseline the client is known to
    /// hold for each entity. Advanced monotonically by acks.
    acked_baselines: HashMap<u32, u32>,
    /// `network_id -> acked tombstone_id`. A despawn is resent until its tombstone
    /// appears here.
    acked_tombstones: HashMap<u32, u32>,
    /// Pending baseline-refresh requests, keyed by `(network_id,
    /// missing_baseline_ref)` so a duplicate request collapses to one queued
    /// `FullBaseline`. Cleared once the refresh is emitted.
    pending_refreshes: HashSet<(u32, u32)>,
}

/// Current server-tracked state for one entity: its latest owned components, the
/// `baseline_id` that names them, and the movement-authority metadata. The baseline
/// id advances only when the wire mirrors change, so a client holding that id needs
/// no update — but the authority metadata (`owner_client_id`,
/// `last_processed_client_tick`) is refreshed on **every** ingest without bumping
/// the baseline, since the resolved cursor advances each tick a command resolves
/// while the pose may not have moved. Folding the cursor into the baseline would
/// resend an otherwise-unchanged pawn every tick.
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
}

/// A despawned entity awaiting per-client tombstone acks. Held until every client
/// that ever knew the entity has acked the tombstone; resent in snapshots
/// meanwhile.
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

    /// Register a client so it is replicated to. Idempotent — re-registering an
    /// existing client leaves its ack state intact. A newly registered client holds
    /// no acked baselines, so its first snapshot is all `FullBaseline` records.
    pub fn register_client(&mut self, client_id: u64) {
        self.clients.entry(client_id).or_default();
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
        self.tombstones.retain(|network_id, tombstone| {
            let acked_by_all = clients.values().all(|client| {
                client
                    .acked_tombstones
                    .get(network_id)
                    .copied()
                    .unwrap_or(0)
                    >= tombstone.tombstone_id
            });
            // Retain (keep) while NOT yet acked by all registered clients.
            !acked_by_all
        });
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
            match self.entities.get_mut(&snap.network_id) {
                Some(existing) if existing.components == snap.components => {
                    // Unchanged pose/movement mirrors: keep the baseline id so acked
                    // clients are omitted. The authority metadata still refreshes —
                    // the resolved cursor advances every tick a command resolves even
                    // when the pawn did not visibly move, and the metadata rides the
                    // record without bumping the baseline.
                    existing.owner_client_id = snap.owner_client_id;
                    existing.last_processed_client_tick = snap.last_processed_client_tick;
                    existing.entity_class = snap.entity_class;
                }
                Some(existing) => {
                    existing.baseline_id = self.next_baseline_id;
                    existing.components = snap.components;
                    existing.owner_client_id = snap.owner_client_id;
                    existing.last_processed_client_tick = snap.last_processed_client_tick;
                    existing.entity_class = snap.entity_class;
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
            // Default reason 0; the engine glue does not yet distinguish despawn
            // causes (Task 4/6 own slot/mover lifecycle). A reason field exists on
            // the wire so a future cause can ride along without a shape change.
            self.tombstones.insert(
                id,
                Tombstone {
                    tombstone_id,
                    reason: 0,
                },
            );
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
    /// one; emit a `FullBaseline` if the client holds no acked baseline (or a refresh
    /// was requested for it); otherwise emit a `Delta` from the client's acked
    /// baseline to the current one. Active despawn tombstones the client has not
    /// acked are appended as `Despawn` records and resend until acked.
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

    fn encode_for_client_with_sequence(
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
                        components: entity.components.iter().map(raw_from_payload).collect(),
                    });
                }
            }
        }

        // Despawn records: resend every active tombstone the client has not acked.
        for (&network_id, tombstone) in &self.tombstones {
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
                components: Vec::new(),
            });
        }

        Some(RawSnapshotMessage {
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
        });

        // `entity_class` is valid on any finite-Transform record; movement metadata is
        // movement-only.
        let attach_class = carries_finite_transform && entity.entity_class.is_some();
        Self {
            has_tick: carries_movement && entity.last_processed_client_tick.is_some(),
            tick: if carries_movement {
                entity.last_processed_client_tick.unwrap_or(0)
            } else {
                0
            },
            local_player: carries_movement && entity.owner_client_id == Some(recipient),
            has_entity_class: attach_class,
            entity_class: if attach_class {
                entity.entity_class.clone().unwrap_or_default()
            } else {
                String::new()
            },
        }
    }
}

/// Re-encode a typed [`ComponentPayload`] into its raw wire envelope form. Mirror of
/// the engine glue's `payload_to_raw`; kept here so the tracker is self-contained.
fn raw_from_payload(payload: &ComponentPayload) -> RawComponentPayload {
    match payload {
        ComponentPayload::Transform(t) => RawComponentPayload {
            component_kind: COMPONENT_KIND_TRANSFORM,
            transform: Some(*t),
            player_movement: None,
            mesh_animation_state: None,
        },
        ComponentPayload::PlayerMovementState(m) => RawComponentPayload {
            component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
            transform: None,
            player_movement: Some(*m),
            mesh_animation_state: None,
        },
        ComponentPayload::MeshAnimationState(m) => RawComponentPayload {
            component_kind: COMPONENT_KIND_MESH_ANIMATION_STATE,
            transform: None,
            player_movement: None,
            mesh_animation_state: Some(m.clone()),
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
    use crate::wire::{WireMovementState, WirePlayerMovementState, WireTransform};

    const CLIENT_A: u64 = 1;
    const CLIENT_B: u64 = 2;

    fn transform(x: f32) -> ComponentPayload {
        ComponentPayload::Transform(WireTransform {
            position: [x, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
        })
    }

    fn movement(velocity_x: f32) -> ComponentPayload {
        ComponentPayload::PlayerMovementState(WirePlayerMovementState {
            velocity: [velocity_x, 0.0, 0.0],
            is_grounded: true,
            air_jumps_remaining: 1,
            air_dashes_remaining: 1,
            dash_cooldown_ms: 0.0,
            air_ticks: 0,
            movement_state: WireMovementState::Normal,
            coyote_timer_ms: 0.0,
            jump_buffer_timer_ms: 0.0,
            jump_spent: false,
            capsule_half_height: 0.8,
            capsule_eye_height: 1.5,
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

    // Two clients are independent: B acking does not omit A's records.
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

        // Only B acks. A must still receive the entity next tick.
        server.apply_ack(CLIENT_B, seq, &[(1, baseline_id)], &[]);
        server.ingest_tick(vec![entity(1, vec![transform(0.0)])]);
        assert!(record_for(&server.encode_for_client(CLIENT_A, 61).unwrap(), 1).is_some());
        assert!(record_for(&server.encode_for_client(CLIENT_B, 62).unwrap(), 1).is_none());
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
        // Client acks b0 so the next change is a Delta (an unacked client would get
        // another FullBaseline instead).
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
    // the baseline, so an acked recipient stays omitted — but the refreshed cursor is
    // still echoed when the pawn IS re-sent (here forced via a refresh request).
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

        // Cursor advances 5 -> 6 but the movement mirror is identical: the pawn is
        // omitted next tick (no baseline bump).
        server.ingest_tick(vec![owned_movement(1, CLIENT_A, Some(6), 1.0)]);
        let snap2 = server.encode_for_client(CLIENT_A, 61).unwrap();
        assert!(
            record_for(&snap2, 1).is_none(),
            "cursor-only advance must not resend the pawn"
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
}
