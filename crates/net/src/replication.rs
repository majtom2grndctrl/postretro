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
    ComponentPayload, EntityRecord, RawComponentPayload, RawEntityRecord, RawSnapshotMessage,
    COMPONENT_KIND_PLAYER_MOVEMENT_STATE, COMPONENT_KIND_TRANSFORM, RECORD_KIND_DELTA,
    RECORD_KIND_DESPAWN, RECORD_KIND_FULL_BASELINE, SNAPSHOT_VERSION,
};

/// One replicable entity's owned, post-tick component state, keyed by its stable
/// `NetworkId`. Produced by the engine glue (`crate::netcode`) after it releases
/// the registry borrow, then handed to [`ServerReplication::ingest_tick`].
///
/// `components` carries the wire mirrors directly — dirty detection compares these
/// (`WireTransform`, `WirePlayerMovementState`) by value, never by serializing and
/// diffing bytes. The order is significant for equality: the producer must emit a
/// stable component order per entity.
#[derive(Debug, Clone, PartialEq)]
pub struct EntitySnapshot {
    pub network_id: u32,
    pub components: Vec<ComponentPayload>,
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

/// Current server-tracked state for one entity: its latest owned components and the
/// `baseline_id` that names them. The baseline id advances only when the wire
/// mirrors change, so a client holding that id needs no update.
#[derive(Debug, Clone)]
struct EntityState {
    baseline_id: u32,
    components: Vec<ComponentPayload>,
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

    /// Drop a client (disconnect). Its ack state is discarded; remaining clients
    /// are unaffected.
    pub fn remove_client(&mut self, client_id: u64) {
        self.clients.remove(&client_id);
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
                    // Unchanged: keep the baseline id so acked clients are omitted.
                }
                Some(existing) => {
                    existing.baseline_id = self.next_baseline_id;
                    existing.components = snap.components;
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
        for &(network_id, baseline_id) in entity_baselines {
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
            let entry = state.acked_tombstones.entry(network_id).or_insert(0);
            if tombstone_id > *entry {
                *entry = tombstone_id;
            }
        }
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
    pub fn encode_for_client(&mut self, client_id: u64, server_tick: u32) -> Option<RawSnapshotMessage> {
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
                components: Vec::new(),
            });
        }

        Some(RawSnapshotMessage {
            version: SNAPSHOT_VERSION,
            sequence,
            server_tick,
            records,
        })
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
        },
        ComponentPayload::PlayerMovementState(m) => RawComponentPayload {
            component_kind: COMPONENT_KIND_PLAYER_MOVEMENT_STATE,
            transform: None,
            player_movement: Some(*m),
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
        EntitySnapshot {
            network_id,
            components,
        }
    }

    // Helper: find a typed record for a network_id in an encoded snapshot.
    fn record_for(snapshot: &RawSnapshotMessage, network_id: u32) -> Option<EntityRecord> {
        typed_records(snapshot)
            .into_iter()
            .find(|r| match r {
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
        server.ingest_tick(vec![entity(1, vec![transform(0.0)]), entity(2, vec![transform(5.0)])]);

        let snap = server.encode_for_client(CLIENT_A, 60).expect("registered client");
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
        assert!(record_for(&snap2, 1).is_none(), "acked unchanged entity omitted");
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
                assert_ne!(new_baseline_id, baseline_id, "delta carries a fresh baseline");
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
        server.ingest_tick(vec![entity(1, vec![transform(0.0)]), entity(2, vec![transform(0.0)])]);
        let snap1 = server.encode_for_client(CLIENT_A, 60).unwrap();
        let EntityRecord::FullBaseline { baseline_id: b1, .. } = record_for(&snap1, 1).unwrap()
        else {
            panic!("fb");
        };
        let EntityRecord::FullBaseline { baseline_id: b2, .. } = record_for(&snap1, 2).unwrap()
        else {
            panic!("fb");
        };
        // Client acks both baselines.
        server.apply_ack(CLIENT_A, snap1.sequence, &[(1, b1), (2, b2)], &[]);

        // Only entity 1 moves; entity 2 unchanged.
        server.ingest_tick(vec![entity(1, vec![transform(3.0)]), entity(2, vec![transform(0.0)])]);
        let snap2 = server.encode_for_client(CLIENT_A, 61).unwrap();
        // Entity 1 re-sends; entity 2 (acked + unchanged) is omitted.
        assert!(matches!(record_for(&snap2, 1), Some(EntityRecord::Delta { .. })));
        assert!(record_for(&snap2, 2).is_none(), "unchanged acked entity stays omitted");

        // Simulate snap2 dropped: client A does NOT ack entity 1's new baseline.
        // Entity 1 moves again. Only entity 1 is affected; entity 2 still omitted.
        server.ingest_tick(vec![entity(1, vec![transform(7.0)]), entity(2, vec![transform(0.0)])]);
        let snap3 = server.encode_for_client(CLIENT_A, 62).unwrap();
        match record_for(&snap3, 1).expect("entity 1 still unacked -> re-sent") {
            // Still a Delta from the last *acked* baseline (b1), not a global resend.
            EntityRecord::Delta { baseline_ref, .. } => assert_eq!(baseline_ref, b1),
            other => panic!("expected Delta from acked baseline, got {other:?}"),
        }
        assert!(record_for(&snap3, 2).is_none(), "lost packet did not disturb entity 2");
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
        assert_eq!(snap_a.sequence, snap_b.sequence, "batch shares one sequence");
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
        let EntityRecord::FullBaseline { baseline_id: b0, .. } = record_for(&snap1, 1).unwrap()
        else {
            panic!("fb");
        };
        // Client acks b0 so the next change is a Delta (an unacked client would get
        // another FullBaseline instead).
        server.apply_ack(CLIENT_A, snap1.sequence, &[(1, b0)], &[]);
        // Move -> baseline 1.
        server.ingest_tick(vec![entity(1, vec![transform(1.0)])]);
        let snap2 = server.encode_for_client(CLIENT_A, 61).unwrap();
        let EntityRecord::Delta { new_baseline_id: b1, .. } = record_for(&snap2, 1).unwrap() else {
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
        assert!(record_for(&snap4, 1).is_none(), "acked tombstone stops resending");
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
            matches!(record_for(&snap2, 1), Some(EntityRecord::FullBaseline { .. })),
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
        let snap = server.encode_for_client(CLIENT_A, 60).expect("client still encodes");
        // The real entity 1 is still a FullBaseline; the bogus acks did not corrupt
        // its state.
        assert!(matches!(record_for(&snap, 1), Some(EntityRecord::FullBaseline { .. })));
        // The non-existent refreshed entity produces no record.
        assert!(record_for(&snap, 424242).is_none());

        // Encoding an unregistered client returns None, not a panic.
        assert!(server.encode_for_client(12345, 60).is_none());
    }
}
