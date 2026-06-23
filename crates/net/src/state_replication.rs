// Server-side state-slot replication tracker: per-client acked-baseline state and
// per-client state-record encoding, keyed only by `StateSlotId`.
// See: context/lib/networking.md

use std::collections::{HashMap, HashSet};

use crate::state_slots::{
    RawStateSlotRecord, STATE_RECORD_KIND_DELTA, STATE_RECORD_KIND_FULL_BASELINE, StateSlotId,
    WireSlotValue,
};

/// Per-client state-replication state: how far this client has acked per slot, and
/// any pending baseline-refresh requests to satisfy in its next snapshot. Dropped
/// wholesale when the client disconnects, so none of its maps leak.
#[derive(Debug, Default)]
struct ClientStateReplication {
    /// Highest snapshot sequence this client has acked (diagnostic; the per-slot
    /// baseline map is the authoritative progress record).
    last_acked_sequence: u32,
    /// `slot_id -> acked baseline_id`. The baseline the client is known to hold for
    /// each slot. Advanced monotonically by acks. Id 0 is the "none yet" sentinel.
    acked_baselines: HashMap<StateSlotId, u32>,
    /// Pending baseline-refresh requests, keyed by `(slot_id, missing_baseline_ref)`
    /// so a duplicate request collapses to one queued `FullBaseline`. Cleared once
    /// the refresh is emitted.
    pending_refreshes: HashSet<(StateSlotId, u32)>,
}

/// Current server-tracked state for one replicated value: its latest complete wire
/// value and the `baseline_id` that names it. The baseline id advances only when the
/// value changes, so a client holding that id needs no update.
#[derive(Debug, Clone)]
struct SlotState {
    baseline_id: u32,
    value: WireSlotValue,
}

/// Server-side state-slot replication tracker. Owns the monotonic sequence /
/// baseline allocators, the current shared-global and owner-private slot values, and
/// per-client ack/refresh state. Keyed throughout by `StateSlotId` (and, for
/// owner-private values, the owning client id); never sees a slot's dotted name.
///
/// The engine glue ingests post-game-logic values each server frame, then encodes
/// each accepted client's records into the *same* snapshot envelope the entity
/// tracker produced (the engine merges the two record lists). Batching with the
/// entity tracker's sequence keeps one ack describing one server frame.
#[derive(Debug)]
pub struct ServerStateReplication {
    /// Monotonic snapshot sequence, for the single-client `produce_for_client` form.
    /// In the normal frame loop the engine shares the entity tracker's sequence via
    /// `produce_in_batch`, so this is only used when replicating one client alone.
    next_sequence: u32,
    /// Monotonic baseline-id allocator; a fresh id every time a value changes so a
    /// stale-baseline client is detectable. Starts at 1 — id 0 is the reserved "no
    /// acked baseline" sentinel in the per-client ack map, so a real baseline must
    /// never be 0 or it would read as "unacked".
    next_baseline_id: u32,
    /// Current shared-global slot values, one per `StateSlotId`, sent to every
    /// registered client.
    shared: HashMap<StateSlotId, SlotState>,
    /// Current owner-private slot values, one per `(StateSlotId, owner_client_id)`,
    /// sent only to the owning client.
    owner_private: HashMap<(StateSlotId, u64), SlotState>,
    /// Per-client ack/refresh state.
    clients: HashMap<u64, ClientStateReplication>,
}

impl Default for ServerStateReplication {
    fn default() -> Self {
        // Baseline allocator starts at 1: id 0 is the "none yet" sentinel for the
        // per-client ack map (a `.or_insert(0)` default reads as unacked).
        Self {
            next_sequence: 0,
            next_baseline_id: 1,
            shared: HashMap::new(),
            owner_private: HashMap::new(),
            clients: HashMap::new(),
        }
    }
}

impl ServerStateReplication {
    /// Fresh tracker with the baseline allocator reserving 0 as the unacked sentinel
    /// and no slots/clients.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an accepted client so it is replicated to. Idempotent —
    /// re-registering an existing client leaves its ack state intact. A newly
    /// registered client holds no acked baselines, so its first snapshot is all
    /// `FullBaseline` records (a late joiner thus gets a full state baseline without
    /// waiting for any value to change). Called from the engine accept path
    /// (`netcode::dispatch_accepted_client`).
    pub fn register_client(&mut self, client_id: u64) {
        self.clients.entry(client_id).or_default();
    }

    /// Drop a client (disconnect / close). Removing its [`ClientStateReplication`]
    /// discards all of that client's per-client maps in one move, so none of them
    /// leak. Also drops every owner-private value owned by this client — a private
    /// value's lifetime is bounded by its owner's connection. Shared-global values
    /// and other clients are unaffected. Called from the engine close path
    /// (`netcode::dispatch_closed_clients` / `on_slot_closed`).
    pub fn remove_client(&mut self, client_id: u64) {
        self.clients.remove(&client_id);
        // A private value's lifetime is bounded by its owner's connection.
        self.owner_private
            .retain(|&(_, owner), _| owner != client_id);
    }

    /// Ingest the current value of a shared-global slot for this server frame.
    /// Compares against the tracked value: an unchanged value keeps its baseline id
    /// (so acked clients stay omitted); a changed (or newly seen) value is assigned a
    /// fresh baseline id. Clearing a value rides as `WireSlotValue::Unset` — there is
    /// no tombstone; an `Unset` is just another value that replicates.
    pub fn ingest_shared(&mut self, slot_id: StateSlotId, value: WireSlotValue) {
        let next = self.next_baseline_id;
        if Self::upsert(&mut self.shared, slot_id, value, next) {
            self.next_baseline_id = self.next_baseline_id.wrapping_add(1);
        }
    }

    /// Ingest the current value of an owner-private slot for `owner_client_id` this
    /// server frame. Same change/baseline semantics as [`Self::ingest_shared`], but
    /// keyed by `(slot_id, owner_client_id)`: the value replicates only to the owning
    /// client. The recipient implies the owner on the wire, so the owner id never
    /// crosses to other clients.
    pub fn ingest_owner_private(
        &mut self,
        slot_id: StateSlotId,
        owner_client_id: u64,
        value: WireSlotValue,
    ) {
        let next = self.next_baseline_id;
        if Self::upsert(
            &mut self.owner_private,
            (slot_id, owner_client_id),
            value,
            next,
        ) {
            self.next_baseline_id = self.next_baseline_id.wrapping_add(1);
        }
    }

    /// Upsert a value into a slot-state map, allocating `fresh_baseline_id` only when
    /// the value actually changed (or is newly seen). Returns `true` iff a fresh
    /// baseline id was consumed, so the caller advances its allocator exactly then —
    /// an unchanged value keeps its prior id and an acked client stays omitted.
    fn upsert<K: std::hash::Hash + Eq>(
        map: &mut HashMap<K, SlotState>,
        key: K,
        value: WireSlotValue,
        fresh_baseline_id: u32,
    ) -> bool {
        match map.get_mut(&key) {
            Some(existing) if existing.value == value => false,
            Some(existing) => {
                existing.value = value;
                existing.baseline_id = fresh_baseline_id;
                true
            }
            None => {
                map.insert(
                    key,
                    SlotState {
                        baseline_id: fresh_baseline_id,
                        value,
                    },
                );
                true
            }
        }
    }

    /// Apply a client state ack. Advances each named slot's per-client baseline only
    /// if the acked id is **newer** than the recorded one, and satisfies any pending
    /// refresh that named an older-or-equal missing ref. Omitted entries leave prior
    /// state unchanged. An unknown client, or a baseline id the server never issued,
    /// is ignored — never a panic. `slot_baselines` is the `AckMessage` field of the
    /// same name (the `u16` is the `StateSlotId` inner value).
    pub fn apply_ack(
        &mut self,
        client_id: u64,
        latest_snapshot_sequence: u32,
        slot_baselines: &[(u16, u32)],
    ) {
        let highest_baseline = self.next_baseline_id.wrapping_sub(1);
        let Some(state) = self.clients.get_mut(&client_id) else {
            return;
        };
        // Sequence advances monotonically; an out-of-order/old ack does not regress
        // it (the per-slot baselines below carry the real progress regardless).
        if latest_snapshot_sequence > state.last_acked_sequence {
            state.last_acked_sequence = latest_snapshot_sequence;
        }
        for &(slot_id_raw, baseline_id) in slot_baselines {
            // A client cannot legitimately ack a baseline id the server never issued;
            // accepting an out-of-range id would wedge this client's per-slot gate
            // (every later real change would force a refresh round-trip until the real
            // counter climbs past the forged id). Ignore it entirely.
            if baseline_id > highest_baseline {
                continue;
            }
            let slot_id = StateSlotId(slot_id_raw);
            let entry = state.acked_baselines.entry(slot_id).or_insert(0);
            // Monotonic: only a newer baseline advances. An older/equal ack is a stale
            // or duplicate packet and is ignored.
            if baseline_id > *entry {
                *entry = baseline_id;
            }
            // A baseline ack for this slot also satisfies any pending refresh that
            // named an older-or-equal missing ref: the client now holds a baseline.
            state
                .pending_refreshes
                .retain(|&(sid, missing_ref)| !(sid == slot_id && missing_ref <= baseline_id));
        }
    }

    /// Queue a state baseline-refresh request from a client. Additive and idempotent
    /// — keyed by `(slot_id, missing_baseline_ref)`, so a duplicate request queues the
    /// same `FullBaseline` once. Unknown clients are ignored; an unknown `slot_id` is
    /// queued anyway and simply produces nothing at encode time if the slot has no
    /// value for this client (never a panic). Maps directly from
    /// `ClientMessage::StateBaselineRefresh`.
    pub fn request_refresh(
        &mut self,
        client_id: u64,
        slot_id: StateSlotId,
        missing_baseline_ref: u32,
    ) {
        if let Some(state) = self.clients.get_mut(&client_id) {
            state
                .pending_refreshes
                .insert((slot_id, missing_baseline_ref));
        }
    }

    /// Begin a server-frame batch: allocate the one `sequence` every client in this
    /// batch shares. In the normal frame loop the engine instead shares the entity
    /// tracker's sequence directly with [`Self::produce_in_batch`] so one ack
    /// describes one server frame; this helper exists for the state-only batch path.
    #[must_use]
    pub fn begin_batch(&mut self) -> u32 {
        self.next_sequence()
    }

    /// Allocate the next monotonic snapshot sequence for the single-client form.
    fn next_sequence(&mut self) -> u32 {
        let seq = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        seq
    }

    /// Produce this client's state records for one server frame, sharing `sequence`
    /// with the rest of the batch (so one ack covers one frame). Returns the records
    /// to merge into the snapshot envelope; `None` for an unregistered (pending /
    /// rejected / closed) client, so such a client receives no state records.
    ///
    /// The `sequence` is accepted but not stamped into a record here — state records
    /// ride inside the snapshot envelope, which already carries the sequence. It is
    /// part of the signature so the engine threads the entity-tracker sequence
    /// through, documenting that state and entity records share one server frame.
    #[must_use]
    pub fn produce_in_batch(
        &mut self,
        client_id: u64,
        _sequence: u32,
    ) -> Option<Vec<RawStateSlotRecord>> {
        self.produce_for_client(client_id)
    }

    /// Produce this client's state records for one server frame: a `FullBaseline` for
    /// every slot the client holds no acked baseline for (or that a refresh forced),
    /// a `Delta` against the acked baseline otherwise, and nothing for a slot the
    /// client already holds at the current baseline. `None` for an unregistered
    /// client (pending/rejected/closed clients get no records).
    ///
    /// Scope filtering is intrinsic to the storage: shared-global values are sent to
    /// every client; owner-private values are keyed by `(slot, owner)`, so only the
    /// owning client's records include them — client B never even iterates client A's
    /// private values.
    #[must_use]
    pub fn produce_for_client(&mut self, client_id: u64) -> Option<Vec<RawStateSlotRecord>> {
        // Pull and clear this client's pending refreshes up front: a refresh is
        // satisfied by the FullBaseline this frame emits. Keyed by slot id only here
        // (the missing ref was just dedup context).
        let forced: HashSet<StateSlotId> = {
            let state = self.clients.get_mut(&client_id)?;
            let slots = state
                .pending_refreshes
                .iter()
                .map(|&(slot_id, _)| slot_id)
                .collect();
            state.pending_refreshes.clear();
            slots
        };

        let mut records = Vec::new();

        // Shared-global: every registered client receives these.
        for (&slot_id, slot) in &self.shared {
            let acked = self
                .clients
                .get(&client_id)
                .and_then(|c| c.acked_baselines.get(&slot_id).copied());
            if let Some(record) = Self::encode_slot(slot_id, slot, acked, forced.contains(&slot_id))
            {
                records.push(record);
            }
        }

        // Owner-private: only the values keyed to THIS client. A non-owner never sees
        // another client's private slot — the filter is the map key, not a per-record
        // check.
        for (&(slot_id, owner), slot) in &self.owner_private {
            if owner != client_id {
                continue;
            }
            let acked = self
                .clients
                .get(&client_id)
                .and_then(|c| c.acked_baselines.get(&slot_id).copied());
            if let Some(record) = Self::encode_slot(slot_id, slot, acked, forced.contains(&slot_id))
            {
                records.push(record);
            }
        }

        Some(records)
    }

    /// Encode one slot's record for a recipient given its acked baseline and whether a
    /// refresh forces a full baseline. Returns `None` to omit a slot the client
    /// already holds at the current baseline.
    fn encode_slot(
        slot_id: StateSlotId,
        slot: &SlotState,
        acked: Option<u32>,
        force_full: bool,
    ) -> Option<RawStateSlotRecord> {
        match acked {
            // Holds the current baseline and not forced: omit.
            Some(acked_id) if acked_id == slot.baseline_id && !force_full => None,
            // Holds an older baseline and not forced: a delta carrying the new
            // complete value, referencing the acked baseline.
            Some(acked_id) if !force_full => Some(RawStateSlotRecord {
                slot_id: slot_id.0,
                kind: STATE_RECORD_KIND_DELTA,
                has_baseline_ref: true,
                baseline_ref: acked_id,
                baseline_id: slot.baseline_id,
                value: slot.value.clone(),
            }),
            // No acked baseline, or a forced refresh: a full baseline.
            _ => Some(RawStateSlotRecord {
                slot_id: slot_id.0,
                kind: STATE_RECORD_KIND_FULL_BASELINE,
                has_baseline_ref: false,
                baseline_ref: 0,
                baseline_id: slot.baseline_id,
                value: slot.value.clone(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT_A: u64 = 1;
    const CLIENT_B: u64 = 2;

    const HEALTH: StateSlotId = StateSlotId(10);
    const MAX_HEALTH: StateSlotId = StateSlotId(11);
    const OBJECTIVE: StateSlotId = StateSlotId(20);

    fn number(value: f32) -> WireSlotValue {
        WireSlotValue::Number(value)
    }

    // Find the record for a slot id in a produced batch.
    fn record_for(
        records: &[RawStateSlotRecord],
        slot_id: StateSlotId,
    ) -> Option<&RawStateSlotRecord> {
        records.iter().find(|r| r.slot_id == slot_id.0)
    }

    // A newly accepted client's first snapshot is a FullBaseline for each slot.
    #[test]
    fn newly_accepted_client_gets_full_baselines() {
        let mut server = ServerStateReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_owner_private(HEALTH, CLIENT_A, number(75.0));
        server.ingest_owner_private(MAX_HEALTH, CLIENT_A, number(100.0));

        let records = server.produce_for_client(CLIENT_A).expect("registered");
        assert_eq!(records.len(), 2);
        for r in &records {
            assert_eq!(r.kind, STATE_RECORD_KIND_FULL_BASELINE);
            assert!(!r.has_baseline_ref);
        }
    }

    // A changed value re-sends as a Delta against the client's acked baseline.
    #[test]
    fn delta_against_acked_baseline() {
        let mut server = ServerStateReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_owner_private(HEALTH, CLIENT_A, number(75.0));

        let snap1 = server.produce_for_client(CLIENT_A).unwrap();
        let baseline = record_for(&snap1, HEALTH).unwrap().baseline_id;
        server.apply_ack(CLIENT_A, 1, &[(HEALTH.0, baseline)]);

        // Value changes -> delta from the acked baseline to a fresh one.
        server.ingest_owner_private(HEALTH, CLIENT_A, number(50.0));
        let snap2 = server.produce_for_client(CLIENT_A).unwrap();
        let rec = record_for(&snap2, HEALTH).expect("changed slot present");
        assert_eq!(rec.kind, STATE_RECORD_KIND_DELTA);
        assert!(rec.has_baseline_ref);
        assert_eq!(rec.baseline_ref, baseline, "delta refs the acked baseline");
        assert_ne!(rec.baseline_id, baseline, "delta carries a fresh baseline");
        assert_eq!(rec.value, number(50.0), "delta carries the complete value");
    }

    // Acking the current baseline omits an unchanged slot next frame.
    #[test]
    fn ack_advances_baseline_and_omits_unchanged_slot() {
        let mut server = ServerStateReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_shared(OBJECTIVE, number(3.0));

        let snap1 = server.produce_for_client(CLIENT_A).unwrap();
        let baseline = record_for(&snap1, OBJECTIVE).unwrap().baseline_id;
        server.apply_ack(CLIENT_A, 1, &[(OBJECTIVE.0, baseline)]);

        // Same value next frame: acked and unchanged -> omitted.
        server.ingest_shared(OBJECTIVE, number(3.0));
        let snap2 = server.produce_for_client(CLIENT_A).unwrap();
        assert!(
            record_for(&snap2, OBJECTIVE).is_none(),
            "acked unchanged slot omitted"
        );
    }

    // A stale (older) ack does not regress the client.
    #[test]
    fn stale_ack_does_not_regress() {
        let mut server = ServerStateReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_shared(OBJECTIVE, number(1.0));
        let snap1 = server.produce_for_client(CLIENT_A).unwrap();
        let b0 = record_for(&snap1, OBJECTIVE).unwrap().baseline_id;
        server.apply_ack(CLIENT_A, 1, &[(OBJECTIVE.0, b0)]);

        server.ingest_shared(OBJECTIVE, number(2.0));
        let snap2 = server.produce_for_client(CLIENT_A).unwrap();
        let b1 = record_for(&snap2, OBJECTIVE).unwrap().baseline_id;
        server.apply_ack(CLIENT_A, 2, &[(OBJECTIVE.0, b1)]);

        // A stale ack for b0 must not regress: the unchanged slot stays omitted.
        server.apply_ack(CLIENT_A, 2, &[(OBJECTIVE.0, b0)]);
        server.ingest_shared(OBJECTIVE, number(2.0));
        let snap3 = server.produce_for_client(CLIENT_A).unwrap();
        assert!(
            record_for(&snap3, OBJECTIVE).is_none(),
            "stale ack did not regress"
        );
    }

    // A refresh request forces a FullBaseline for an already-acked slot.
    #[test]
    fn refresh_schedules_full_baseline() {
        let mut server = ServerStateReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_shared(OBJECTIVE, number(5.0));
        let snap1 = server.produce_for_client(CLIENT_A).unwrap();
        let baseline = record_for(&snap1, OBJECTIVE).unwrap().baseline_id;
        server.apply_ack(CLIENT_A, 1, &[(OBJECTIVE.0, baseline)]);

        // Without a refresh the unchanged acked slot would be omitted. Request a
        // refresh: it must re-send as a FullBaseline.
        server.request_refresh(CLIENT_A, OBJECTIVE, baseline);
        server.ingest_shared(OBJECTIVE, number(5.0));
        let snap2 = server.produce_for_client(CLIENT_A).unwrap();
        let rec = record_for(&snap2, OBJECTIVE).expect("refresh forces a record");
        assert_eq!(rec.kind, STATE_RECORD_KIND_FULL_BASELINE);

        // One-shot: next frame reverts to omitting the acked slot.
        server.ingest_shared(OBJECTIVE, number(5.0));
        let snap3 = server.produce_for_client(CLIENT_A).unwrap();
        assert!(
            record_for(&snap3, OBJECTIVE).is_none(),
            "refresh is one-shot"
        );
    }

    // A late joiner receives a full state baseline without waiting for a value change.
    #[test]
    fn late_joiner_gets_full_baseline_without_value_change() {
        let mut server = ServerStateReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_shared(OBJECTIVE, number(9.0));
        // A acks; the value never changes again.
        let snap1 = server.produce_for_client(CLIENT_A).unwrap();
        let baseline = record_for(&snap1, OBJECTIVE).unwrap().baseline_id;
        server.apply_ack(CLIENT_A, 1, &[(OBJECTIVE.0, baseline)]);

        // B joins late. Even with no value change, B must get the FullBaseline.
        server.register_client(CLIENT_B);
        let snap_b = server.produce_for_client(CLIENT_B).unwrap();
        let rec = record_for(&snap_b, OBJECTIVE).expect("late joiner sees the slot");
        assert_eq!(rec.kind, STATE_RECORD_KIND_FULL_BASELINE);
        assert_eq!(rec.value, number(9.0));
    }

    // Owner-private filtering: client B never receives client A's private slot.
    #[test]
    fn owner_private_slot_filtered_to_owner() {
        let mut server = ServerStateReplication::new();
        server.register_client(CLIENT_A);
        server.register_client(CLIENT_B);
        server.ingest_owner_private(HEALTH, CLIENT_A, number(80.0));
        server.ingest_owner_private(HEALTH, CLIENT_B, number(40.0));

        let snap_a = server.produce_for_client(CLIENT_A).unwrap();
        let snap_b = server.produce_for_client(CLIENT_B).unwrap();

        // Each client sees exactly one HEALTH record, with its own value.
        assert_eq!(snap_a.len(), 1);
        assert_eq!(record_for(&snap_a, HEALTH).unwrap().value, number(80.0));
        assert_eq!(snap_b.len(), 1);
        assert_eq!(record_for(&snap_b, HEALTH).unwrap().value, number(40.0));
    }

    // Shared/global delivery: a shared slot reaches every accepted client.
    #[test]
    fn shared_global_delivered_to_all_clients() {
        let mut server = ServerStateReplication::new();
        server.register_client(CLIENT_A);
        server.register_client(CLIENT_B);
        server.ingest_shared(OBJECTIVE, number(7.0));

        let seq = server.begin_batch();
        let snap_a = server.produce_in_batch(CLIENT_A, seq).unwrap();
        let snap_b = server.produce_in_batch(CLIENT_B, seq).unwrap();

        assert_eq!(record_for(&snap_a, OBJECTIVE).unwrap().value, number(7.0));
        assert_eq!(record_for(&snap_b, OBJECTIVE).unwrap().value, number(7.0));
    }

    // Fallback to full when the referenced baseline is missing: a refresh naming a
    // baseline the client never acked still yields a FullBaseline (the client holds
    // no acked baseline, so encode falls back to full rather than a dangling delta).
    #[test]
    fn fallback_to_full_when_referenced_baseline_missing() {
        let mut server = ServerStateReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_owner_private(HEALTH, CLIENT_A, number(100.0));
        let snap1 = server.produce_for_client(CLIENT_A).unwrap();
        let baseline = record_for(&snap1, HEALTH).unwrap().baseline_id;
        // Client never acks (the snapshot was lost). The value changes again.
        server.ingest_owner_private(HEALTH, CLIENT_A, number(90.0));

        // With no acked baseline, the client must get a FullBaseline, not a delta
        // against a baseline it does not hold.
        let snap2 = server.produce_for_client(CLIENT_A).unwrap();
        let rec = record_for(&snap2, HEALTH).expect("slot present");
        assert_eq!(rec.kind, STATE_RECORD_KIND_FULL_BASELINE);
        assert_ne!(rec.baseline_id, baseline, "carries the latest baseline");
        assert_eq!(rec.value, number(90.0));

        // A refresh that names the missing ref also resolves to a FullBaseline.
        server.request_refresh(CLIENT_A, HEALTH, baseline);
        server.ingest_owner_private(HEALTH, CLIENT_A, number(90.0));
        let snap3 = server.produce_for_client(CLIENT_A).unwrap();
        assert_eq!(
            record_for(&snap3, HEALTH).unwrap().kind,
            STATE_RECORD_KIND_FULL_BASELINE
        );
    }

    // Unchanged value keeps its baseline id (no allocator churn): two clients ack the
    // same id and both omit it next frame.
    #[test]
    fn unchanged_value_keeps_baseline_id() {
        let mut server = ServerStateReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_shared(OBJECTIVE, number(1.0));
        let b1 = record_for(&server.produce_for_client(CLIENT_A).unwrap(), OBJECTIVE)
            .unwrap()
            .baseline_id;
        // Re-ingest the same value: no fresh baseline.
        server.ingest_shared(OBJECTIVE, number(1.0));
        let b2 = record_for(&server.produce_for_client(CLIENT_A).unwrap(), OBJECTIVE)
            .unwrap()
            .baseline_id;
        assert_eq!(b1, b2, "unchanged value keeps its baseline id");
    }

    // Closing a client drops its per-client maps and its owner-private values, and
    // leaves other clients unaffected. An unregistered client produces no records.
    #[test]
    fn remove_client_drops_state_and_owner_private_values() {
        let mut server = ServerStateReplication::new();
        server.register_client(CLIENT_A);
        server.register_client(CLIENT_B);
        server.ingest_owner_private(HEALTH, CLIENT_A, number(80.0));
        server.ingest_owner_private(HEALTH, CLIENT_B, number(40.0));
        server.ingest_shared(OBJECTIVE, number(2.0));

        server.remove_client(CLIENT_A);
        assert!(!server.clients.contains_key(&CLIENT_A));
        assert!(
            !server.owner_private.contains_key(&(HEALTH, CLIENT_A)),
            "A's private value dropped"
        );
        assert!(
            server.owner_private.contains_key(&(HEALTH, CLIENT_B)),
            "B's private value retained"
        );
        // A removed/unregistered client produces no records.
        assert!(server.produce_for_client(CLIENT_A).is_none());
        // B still sees its private slot and the shared slot.
        let snap_b = server.produce_for_client(CLIENT_B).unwrap();
        assert!(record_for(&snap_b, HEALTH).is_some());
        assert!(record_for(&snap_b, OBJECTIVE).is_some());
    }

    // A forged ack naming a baseline id above the issued range is ignored, so a later
    // real change still reaches the client as a delta (not wedged into refresh).
    #[test]
    fn forged_future_baseline_ack_is_ignored() {
        let mut server = ServerStateReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_shared(OBJECTIVE, number(1.0));
        let snap1 = server.produce_for_client(CLIENT_A).unwrap();
        let baseline = record_for(&snap1, OBJECTIVE).unwrap().baseline_id;
        server.apply_ack(CLIENT_A, 1, &[(OBJECTIVE.0, baseline)]);

        // Forged: a baseline id far above anything issued. Must be ignored so the
        // client is still recorded as holding the real baseline.
        server.apply_ack(CLIENT_A, 1, &[(OBJECTIVE.0, u32::MAX)]);

        server.ingest_shared(OBJECTIVE, number(2.0));
        let snap2 = server.produce_for_client(CLIENT_A).unwrap();
        let rec = record_for(&snap2, OBJECTIVE).expect("slot present");
        assert_eq!(rec.kind, STATE_RECORD_KIND_DELTA);
        assert_eq!(
            rec.baseline_ref, baseline,
            "delta refs the real acked baseline"
        );
    }

    // An unset value replicates like any other (clearing rides as Unset, no tombstone).
    #[test]
    fn unset_value_replicates() {
        let mut server = ServerStateReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_shared(OBJECTIVE, number(3.0));
        let snap1 = server.produce_for_client(CLIENT_A).unwrap();
        let baseline = record_for(&snap1, OBJECTIVE).unwrap().baseline_id;
        server.apply_ack(CLIENT_A, 1, &[(OBJECTIVE.0, baseline)]);

        // Clear -> Unset rides as a fresh baseline / delta.
        server.ingest_shared(OBJECTIVE, WireSlotValue::Unset);
        let snap2 = server.produce_for_client(CLIENT_A).unwrap();
        let rec = record_for(&snap2, OBJECTIVE).expect("unset replicates");
        assert_eq!(rec.value, WireSlotValue::Unset);
    }

    // Malformed/stale acks and refreshes for unknown clients/slots never panic.
    #[test]
    fn malformed_ack_and_refresh_never_panic() {
        let mut server = ServerStateReplication::new();
        server.register_client(CLIENT_A);
        server.ingest_shared(OBJECTIVE, number(1.0));

        // Unknown client: ignored.
        server.apply_ack(999, 5, &[(OBJECTIVE.0, 7)]);
        server.request_refresh(999, OBJECTIVE, 0);
        // Unknown slot id for a known client: queued, produces nothing.
        server.request_refresh(CLIENT_A, StateSlotId(424), 0);
        // Absurd baseline id: absorbed.
        server.apply_ack(CLIENT_A, 0, &[(424, 999_999)]);

        let snap = server.produce_for_client(CLIENT_A).expect("still encodes");
        assert!(record_for(&snap, OBJECTIVE).is_some());
        assert!(record_for(&snap, StateSlotId(424)).is_none());
        // Unregistered client returns None, not a panic.
        assert!(server.produce_for_client(12345).is_none());
    }
}
