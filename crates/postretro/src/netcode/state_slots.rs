// Engine-side replicated state-slot schema: builds the deterministic
// `StateSlotId` map and 32-byte fingerprint from the slot table's replicated
// slots, lowers them to `postretro-net` wire descriptors, and projects
// descriptor-defined gameplay values (HealthComponent) into named replicated
// slots. The net crate is registry-blind and script-blind; this module owns the
// only mapping between `SlotTable` dotted names and wire `StateSlotId`s.
// See: context/lib/networking.md · context/lib/scripting.md §5
//
// Phase 3.5 Task 1 scope: the schema/fingerprint/lowering and the projection
// adapter *shape*. Task 2 builds the server tracker against the descriptors, and
// Task 3 wires production/apply into the frame loop. No production loop here yet.

use postretro_net::state_slots::{
    NumericRange as WireNumericRange, ReplicationScope as WireReplicationScope, SlotValueType,
    StateSchema, StateSlotDescriptor, StateSlotId,
};

use crate::scripting::components::health::pawn_with_health;
use crate::scripting::registry::EntityRegistry;
use crate::scripting::slot_table::{
    NumericRange, ReplicationScope, SlotTable, SlotType, SlotValue,
};

/// Version prefix folded into the schema fingerprint. Bump when the canonical byte
/// stream's *shape* changes (a new field, a reordered tag) so an old client's
/// fingerprint can never accidentally match a new server's.
const FINGERPRINT_STREAM_VERSION: u8 = 1;

/// Canonical type tags written into the fingerprint stream. Distinct from the wire
/// `VALUE_KIND_*` discriminants by design: this tags the *declared slot type*, not a
/// runtime value, and must stay stable independent of the wire codec.
const TYPE_TAG_NUMBER: u8 = 1;
const TYPE_TAG_BOOLEAN: u8 = 2;
const TYPE_TAG_STRING: u8 = 3;
const TYPE_TAG_ENUM: u8 = 4;
const TYPE_TAG_ARRAY: u8 = 5;

/// Canonical scope tags written into the fingerprint stream.
const SCOPE_TAG_SHARED_GLOBAL: u8 = 1;
const SCOPE_TAG_OWNER_PRIVATE: u8 = 2;

/// One replicated slot in the deterministic schema: its dotted name, assigned wire
/// id, declared type, validation shape, and replication scope. The engine keeps the
/// dotted `name` for the apply path (mapping a `StateSlotId` back to a slot table
/// write); the net descriptor it lowers to drops the name.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReplicatedSlotSchemaEntry {
    pub(crate) slot_id: StateSlotId,
    pub(crate) name: String,
    pub(crate) slot_type: SlotType,
    pub(crate) range: Option<NumericRange>,
    pub(crate) scope: ReplicationScope,
}

/// The deterministic replicated-slot schema, built from a `SlotTable`. Holds the
/// ordered entries, the `[u8; 32]` fingerprint, and the lowered `postretro-net`
/// `StateSchema`. Both peers build this identically from their own slot tables; a
/// fingerprint match is the cross-peer agreement gate.
#[derive(Clone, Debug)]
pub(crate) struct ReplicatedSlotSchema {
    entries: Vec<ReplicatedSlotSchemaEntry>,
    fingerprint: [u8; 32],
}

impl ReplicatedSlotSchema {
    /// Build the schema from the slot table. Includes only replicated slots
    /// (`SharedGlobal` / `OwnerPrivatePlayer`); `None`/local-only slots get no
    /// `StateSlotId` and do not affect the fingerprint. Entries are sorted by stable
    /// dotted name and assigned dense `StateSlotId`s starting at 0.
    pub(crate) fn build(slot_table: &SlotTable) -> Self {
        let mut replicated: Vec<(&str, &SlotType, Option<NumericRange>, ReplicationScope)> =
            slot_table
                .iter()
                .filter_map(|(name, record)| {
                    let scope = record.schema.network;
                    match scope {
                        ReplicationScope::None => None,
                        ReplicationScope::SharedGlobal | ReplicationScope::OwnerPrivatePlayer => {
                            Some((name, &record.schema.slot_type, record.schema.range, scope))
                        }
                    }
                })
                .collect();
        // Sort by stable dotted name so both peers assign identical ids.
        replicated.sort_by(|left, right| left.0.cmp(right.0));

        let entries: Vec<ReplicatedSlotSchemaEntry> = replicated
            .into_iter()
            .enumerate()
            .map(
                |(index, (name, slot_type, range, scope))| ReplicatedSlotSchemaEntry {
                    slot_id: StateSlotId(index as u16),
                    name: name.to_string(),
                    slot_type: slot_type.clone(),
                    range,
                    scope,
                },
            )
            .collect();

        let fingerprint = compute_fingerprint(&entries);
        Self {
            entries,
            fingerprint,
        }
    }

    pub(crate) fn entries(&self) -> &[ReplicatedSlotSchemaEntry] {
        &self.entries
    }

    pub(crate) fn fingerprint(&self) -> &[u8; 32] {
        &self.fingerprint
    }

    /// The dotted slot name for a wire id, or `None` if the id is not in this
    /// schema. The apply path uses this to map an incoming record to a slot write.
    pub(crate) fn name_for(&self, slot_id: StateSlotId) -> Option<&str> {
        self.entries
            .iter()
            .find(|entry| entry.slot_id == slot_id)
            .map(|entry| entry.name.as_str())
    }

    /// The wire id for a dotted slot name, or `None` if the slot is not replicated.
    /// The production path uses this to stamp a projected value with its id.
    pub(crate) fn id_for(&self, name: &str) -> Option<StateSlotId> {
        self.entries
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.slot_id)
    }

    /// Lower this schema to the registry-blind `postretro-net` `StateSchema`: the
    /// per-slot descriptors plus the opaque fingerprint. The net crate validates
    /// hostile bytes against this; it never sees the dotted names or scripting types.
    pub(crate) fn to_net_schema(&self) -> StateSchema {
        StateSchema::new(
            self.fingerprint,
            self.entries
                .iter()
                .map(ReplicatedSlotSchemaEntry::to_net_descriptor),
        )
    }
}

impl ReplicatedSlotSchemaEntry {
    fn to_net_descriptor(&self) -> StateSlotDescriptor {
        StateSlotDescriptor {
            slot_id: self.slot_id,
            value_type: slot_type_to_wire(&self.slot_type),
            range: self.range.map(numeric_range_to_wire),
            scope: scope_to_wire(self.scope),
        }
    }
}

fn slot_type_to_wire(slot_type: &SlotType) -> SlotValueType {
    match slot_type {
        SlotType::Number => SlotValueType::Number,
        SlotType::Boolean => SlotValueType::Boolean,
        SlotType::String => SlotValueType::String,
        SlotType::Enum { values } => SlotValueType::Enum {
            values: values.clone(),
        },
        SlotType::Array => SlotValueType::Array,
    }
}

fn numeric_range_to_wire(range: NumericRange) -> WireNumericRange {
    // An unbounded edge (e.g. `+inf` max on `player.maxHealth`) lowers with its
    // `*_finite` flag clear so the net crate never compares against a non-finite
    // bound. The numeric bytes still travel for fingerprint stability.
    WireNumericRange {
        min: range.min,
        max: range.max,
        min_finite: range.min.is_finite(),
        max_finite: range.max.is_finite(),
    }
}

fn scope_to_wire(scope: ReplicationScope) -> WireReplicationScope {
    match scope {
        ReplicationScope::SharedGlobal => WireReplicationScope::SharedGlobal,
        ReplicationScope::OwnerPrivatePlayer => WireReplicationScope::OwnerPrivatePlayer,
        // `None` slots are filtered out before lowering, so this is unreachable in
        // practice; map defensively to shared rather than panic.
        ReplicationScope::None => WireReplicationScope::SharedGlobal,
    }
}

/// Compute the 32-byte schema fingerprint over a canonical byte stream:
/// version prefix, then for each replicated slot in id (== sorted-name) order:
/// length-prefixed UTF-8 name, explicit type tag, enum values in declared order
/// (count + length-prefixed UTF-8), range finite/min/max flags with stable
/// little-endian numeric bytes, and the scope tag. Computed in `postretro` with the
/// workspace `blake3`; `postretro-net` stores the result as opaque bytes.
fn compute_fingerprint(entries: &[ReplicatedSlotSchemaEntry]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&[FINGERPRINT_STREAM_VERSION]);
    hasher.update(&(entries.len() as u32).to_le_bytes());

    for entry in entries {
        hasher.update(&entry.slot_id.0.to_le_bytes());
        write_len_prefixed_str(&mut hasher, &entry.name);

        match &entry.slot_type {
            SlotType::Number => hasher.update(&[TYPE_TAG_NUMBER]),
            SlotType::Boolean => hasher.update(&[TYPE_TAG_BOOLEAN]),
            SlotType::String => hasher.update(&[TYPE_TAG_STRING]),
            SlotType::Enum { values } => {
                hasher.update(&[TYPE_TAG_ENUM]);
                hasher.update(&(values.len() as u32).to_le_bytes());
                for value in values {
                    write_len_prefixed_str(&mut hasher, value);
                }
                &hasher
            }
            SlotType::Array => hasher.update(&[TYPE_TAG_ARRAY]),
        };

        // Range: an explicit "has range" flag, then per-edge finite flag and the
        // stable LE numeric bytes (always written so a finite-flag flip alone still
        // changes the digest deterministically).
        match entry.range {
            Some(range) => {
                hasher.update(&[1u8]);
                hasher.update(&[u8::from(range.min.is_finite())]);
                hasher.update(&range.min.to_le_bytes());
                hasher.update(&[u8::from(range.max.is_finite())]);
                hasher.update(&range.max.to_le_bytes());
            }
            None => {
                hasher.update(&[0u8]);
            }
        }

        let scope_tag = match entry.scope {
            ReplicationScope::SharedGlobal => SCOPE_TAG_SHARED_GLOBAL,
            ReplicationScope::OwnerPrivatePlayer => SCOPE_TAG_OWNER_PRIVATE,
            // Filtered out before this point; map deterministically rather than panic.
            ReplicationScope::None => 0,
        };
        hasher.update(&[scope_tag]);
    }

    *hasher.finalize().as_bytes()
}

fn write_len_prefixed_str(hasher: &mut blake3::Hasher, value: &str) {
    let bytes = value.as_bytes();
    hasher.update(&(bytes.len() as u32).to_le_bytes());
    hasher.update(bytes);
}

// ---------------------------------------------------------------------------
// Descriptor-defined value projection
// ---------------------------------------------------------------------------

/// One projected `(dotted slot name, value)` pair, produced by a projection adapter
/// from descriptor-defined gameplay state. The production loop (Task 3) maps the
/// name to a `StateSlotId` via [`ReplicatedSlotSchema::id_for`] and the server
/// tracker (Task 2) tracks the wire value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProjectedSlotValue {
    pub(crate) name: String,
    pub(crate) value: SlotValue,
}

/// A source of descriptor-defined gameplay values that feed named replicated slots.
/// Phase 3.5 adds no descriptor authoring syntax: an adapter reads engine component
/// state and emits named-slot projections. The schema (type/scope/validation) lives
/// in the slot table; an adapter only supplies values for already-declared slots.
pub(crate) trait DescriptorSlotProjection {
    /// Project the current descriptor-defined values into named slots for one
    /// player pawn context. Returns the pairs to replicate; an empty vec means the
    /// source has no value this frame (e.g. no pawn materialized yet).
    fn project(&self, registry: &EntityRegistry) -> Vec<ProjectedSlotValue>;
}

/// The first projection adapter: reads `HealthComponent` current/max from the
/// descriptor-spawned player pawn and projects them to `player.health` /
/// `player.maxHealth`. No general descriptor-struct replication — only these two
/// named numeric slots. Task 4 extends this to per-owner extraction on the server;
/// Task 1 establishes the adapter shape and the single-pawn read.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct HealthSlotProjection;

impl DescriptorSlotProjection for HealthSlotProjection {
    fn project(&self, registry: &EntityRegistry) -> Vec<ProjectedSlotValue> {
        let Some((_pawn, health)) = pawn_with_health(registry) else {
            return Vec::new();
        };
        vec![
            ProjectedSlotValue {
                name: "player.health".to_string(),
                value: SlotValue::Number(health.current),
            },
            ProjectedSlotValue {
                name: "player.maxHealth".to_string(),
                value: SlotValue::Number(health.max),
            },
        ]
    }
}

// ---------------------------------------------------------------------------
// Server-side production glue
// ---------------------------------------------------------------------------

use postretro_net::state_replication::ServerStateReplication;
use postretro_net::state_slots::{RawStateSlotRecord, WireSlotValue};

use crate::netcode::command_queue::MovementOwners;
use crate::scripting::components::health::HealthComponent;
use crate::scripting::registry::EntityId;

/// Host-side replicated-state production: owns the deterministic replicated-slot
/// schema (built once, lazily, from the live `SlotTable` after mod stores commit)
/// and the registry-blind [`ServerStateReplication`] tracker. Lives on the
/// `NetEndpoint::Host` variant; the frame send path (`net_serialize_and_send` →
/// `host_replicate`) ingests this frame's projected values, then produces per-client
/// state records to splice into the entity snapshot envelope.
///
/// The schema is the only place the engine maps `StateSlotId <-> dotted name`; the
/// net tracker never sees a name. Both peers build the schema identically from the
/// same content, so a fingerprint match is the cross-peer agreement gate.
pub(crate) struct HostStateReplication {
    /// Built lazily on the first frame (mod init has committed the stores by then),
    /// then reused for the session. `None` until built.
    schema: Option<ReplicatedSlotSchema>,
    tracker: ServerStateReplication,
}

impl HostStateReplication {
    pub(crate) fn new() -> Self {
        Self {
            schema: None,
            tracker: ServerStateReplication::new(),
        }
    }

    /// Build the schema from the live slot table on first use, returning a reference.
    /// Idempotent — built once and cached. Called inside the frame send path, by which
    /// point mod stores have committed, so the schema reflects the final slot set.
    fn schema(&mut self, slot_table: &SlotTable) -> &ReplicatedSlotSchema {
        self.schema
            .get_or_insert_with(|| ReplicatedSlotSchema::build(slot_table))
    }

    /// The local schema fingerprint, building the schema if needed. Stamped into every
    /// snapshot carrying state records so the client gates on a match.
    pub(crate) fn fingerprint(&mut self, slot_table: &SlotTable) -> [u8; 32] {
        *self.schema(slot_table).fingerprint()
    }

    /// Register an accepted client so it is replicated to (accept lifecycle).
    pub(crate) fn register_client(&mut self, client_id: u64) {
        self.tracker.register_client(client_id);
    }

    /// Drop a closed client's per-client state and its owner-private values (close
    /// lifecycle).
    pub(crate) fn remove_client(&mut self, client_id: u64) {
        self.tracker.remove_client(client_id);
    }

    /// Apply a client's `AckMessage.slot_baselines` (inbound reliable path).
    pub(crate) fn apply_ack(
        &mut self,
        client_id: u64,
        latest_snapshot_sequence: u32,
        slot_baselines: &[(u16, u32)],
    ) {
        self.tracker
            .apply_ack(client_id, latest_snapshot_sequence, slot_baselines);
    }

    /// Apply a client's `StateBaselineRefresh` request keyed by `StateSlotId` (inbound
    /// reliable path). An unknown slot id is queued and simply produces nothing.
    pub(crate) fn request_refresh(
        &mut self,
        client_id: u64,
        slot_id: u16,
        missing_baseline_ref: u32,
    ) {
        self.tracker
            .request_refresh(client_id, StateSlotId(slot_id), missing_baseline_ref);
    }

    /// Ingest this server frame's authoritative values into the tracker, then produce
    /// the per-client state records to splice into `client_id`'s snapshot. Returns
    /// `None` for an unregistered (pending/rejected/closed) client, so such a client
    /// receives no state records.
    ///
    /// The collect+ingest step is intrinsically per-frame, but the tracker dedups by
    /// value (an unchanged value keeps its baseline id), so calling it once per client
    /// in a batch only re-ingests already-tracked values cheaply. To keep one ack per
    /// frame, the caller passes the shared `sequence` from the entity tracker's batch.
    pub(crate) fn produce_for_client(
        &mut self,
        slot_table: &SlotTable,
        registry: &EntityRegistry,
        owners: &MovementOwners,
        client_id: u64,
        sequence: u32,
    ) -> Option<Vec<RawStateSlotRecord>> {
        // Build the schema once, then ingest this frame's projected values. The schema
        // borrow is dropped before the tracker call (it borrows `self.schema`; the
        // tracker borrows `self.tracker`), so clone the small per-entry projection list
        // out first.
        self.ingest_frame(slot_table, registry, owners);
        self.tracker.produce_in_batch(client_id, sequence)
    }

    /// Collect and ingest this frame's authoritative source values. Shared slots take
    /// the slot table's current value; owner-private slots take a per-owner value
    /// (descriptor-fed health from each owned pawn's `HealthComponent`, else the slot's
    /// table value keyed to each owner). A slot with no source value this frame is
    /// simply not ingested (it keeps its prior tracked value, or stays absent).
    fn ingest_frame(
        &mut self,
        slot_table: &SlotTable,
        registry: &EntityRegistry,
        owners: &MovementOwners,
    ) {
        // Snapshot the schema entries we need (id, name, scope) so the schema borrow is
        // released before the `&mut self.tracker` calls below.
        let entries: Vec<(StateSlotId, String, ReplicationScope)> = self
            .schema(slot_table)
            .entries()
            .iter()
            .map(|e| (e.slot_id, e.name.clone(), e.scope))
            .collect();

        for (slot_id, name, scope) in entries {
            match scope {
                ReplicationScope::None => {}
                ReplicationScope::SharedGlobal => {
                    if let Some(value) = shared_source_value(slot_table, &name) {
                        self.tracker.ingest_shared(slot_id, value);
                    }
                }
                ReplicationScope::OwnerPrivatePlayer => {
                    for (pawn, client_id) in owners.iter() {
                        if let Some(value) =
                            owner_private_source_value(slot_table, registry, &name, pawn)
                        {
                            self.tracker.ingest_owner_private(slot_id, client_id, value);
                        }
                    }
                }
            }
        }
    }
}

impl Default for HostStateReplication {
    fn default() -> Self {
        Self::new()
    }
}

/// The current shared-slot source value: the slot table's current value, lowered to
/// the wire mirror. `None` when the slot has no value yet or a non-finite value (kept
/// off the wire). Shared slots are global — they have one value regardless of owner.
fn shared_source_value(slot_table: &SlotTable, name: &str) -> Option<WireSlotValue> {
    let record = slot_table.get(name)?;
    let value = record.value.as_ref()?;
    slot_value_to_wire(value)
}

/// The per-owner source value for an owner-private slot. Descriptor-fed health slots
/// (`player.health` / `player.maxHealth`) read the owning pawn's live
/// `HealthComponent` directly — the descriptor projection path, per-owner. Any other
/// owner-private slot falls back to the slot table's current value keyed to this owner
/// (a single global value replicated privately). `None` when no source value exists.
fn owner_private_source_value(
    slot_table: &SlotTable,
    registry: &EntityRegistry,
    name: &str,
    pawn: EntityId,
) -> Option<WireSlotValue> {
    if let Some(value) = descriptor_health_for_pawn(registry, name, pawn) {
        return slot_value_to_wire(&value);
    }
    let record = slot_table.get(name)?;
    let value = record.value.as_ref()?;
    slot_value_to_wire(value)
}

/// Read the descriptor-fed health value for `name` from `pawn`'s live
/// `HealthComponent`, the first descriptor-defined replicated source (M15 Phase 3.5).
/// `player.health` → current HP, `player.maxHealth` → max HP. `None` for any other
/// name or a pawn carrying no `HealthComponent`. This is the per-owner generalization
/// of [`HealthSlotProjection`]: the production path reads each owned pawn's component
/// rather than a single context, so each client's snapshot carries its own health.
fn descriptor_health_for_pawn(
    registry: &EntityRegistry,
    name: &str,
    pawn: EntityId,
) -> Option<SlotValue> {
    let field = match name {
        "player.health" => HealthField::Current,
        "player.maxHealth" => HealthField::Max,
        _ => return None,
    };
    let health = registry.get_component::<HealthComponent>(pawn).ok()?;
    let value = match field {
        HealthField::Current => health.current,
        HealthField::Max => health.max,
    };
    Some(SlotValue::Number(value))
}

#[derive(Clone, Copy)]
enum HealthField {
    Current,
    Max,
}

// ---------------------------------------------------------------------------
// Client-side apply glue
// ---------------------------------------------------------------------------

use std::collections::HashMap;

use postretro_net::state_slots::{StateSlotRecord, StateValidationError, validate_state_records};
use postretro_net::wire::StateBaselineRefreshRequest;

use crate::scripting::primitives::store::apply_store_slot_batch;

/// Reason code carried in a `StateBaselineRefresh` request. Diagnostic only — the
/// server repair path keys on slot + missing ref, not the reason.
const STATE_REFRESH_REASON_UNKNOWN_BASELINE: u8 = 0;

/// What a client state-apply pass produced for the caller to send back on the reliable
/// input channel: the `(slot_id, baseline_id)` acks for applied records, and the
/// baseline-refresh requests for deltas referencing a baseline the client does not
/// hold. Both empty when the snapshot carried no state records or was rejected whole.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct StateApplyOutcome {
    pub(crate) slot_baselines: Vec<(u16, u32)>,
    pub(crate) refresh_requests: Vec<StateBaselineRefreshRequest>,
}

/// Client-side replicated-state apply: owns the deterministic schema (built lazily
/// from the live `SlotTable`, identical to the server's) and the per-slot held
/// baseline. Lives on the `NetEndpoint::Client` variant; the snapshot receive path
/// validates the whole state batch against the schema, applies all-or-nothing through
/// the engine store-write path, and returns the acks + refresh requests to send back.
pub(crate) struct ClientStateApply {
    schema: Option<ReplicatedSlotSchema>,
    /// The lowered net schema (fingerprint + per-slot descriptors), built once
    /// alongside `schema` and reused so per-snapshot validation does not re-lower it.
    net_schema: Option<StateSchema>,
    /// `StateSlotId -> held baseline_id`. A delta's `baseline_ref` must match this to
    /// apply; a successful apply advances it. `FullBaseline` sets it outright.
    held_baselines: HashMap<StateSlotId, u32>,
}

impl ClientStateApply {
    pub(crate) fn new() -> Self {
        Self {
            schema: None,
            net_schema: None,
            held_baselines: HashMap::new(),
        }
    }

    /// Build the schema (and its lowered net form) once from the live slot table,
    /// returning a reference to the replicated-slot schema for name lookups.
    fn schema(&mut self, slot_table: &SlotTable) -> &ReplicatedSlotSchema {
        self.ensure_built(slot_table);
        self.schema.as_ref().expect("schema built above")
    }

    /// The lowered net schema, building both forms once if needed.
    fn net_schema(&mut self, slot_table: &SlotTable) -> &StateSchema {
        self.ensure_built(slot_table);
        self.net_schema.as_ref().expect("net schema built above")
    }

    fn ensure_built(&mut self, slot_table: &SlotTable) {
        if self.schema.is_none() {
            let schema = ReplicatedSlotSchema::build(slot_table);
            self.net_schema = Some(schema.to_net_schema());
            self.schema = Some(schema);
        }
    }

    /// Validate and apply one snapshot's replicated-state records. The whole batch is
    /// validated against the local schema FIRST (fingerprint, then every record); any
    /// invalid record rejects the WHOLE batch and leaves every slot unchanged — no
    /// partial apply. On a fingerprint mismatch a stable diagnostic is logged and the
    /// batch is dropped.
    ///
    /// On success, every applicable record's value is written through the atomic
    /// store-batch helper (which prevalidates all mapped values, then commits all or
    /// none), so the slot table's own type/range/enum/finite checks run too. A delta
    /// referencing a baseline the client does not hold yields a refresh request and is
    /// excluded from the applied set (the rest of the batch still applies). Returns the
    /// acks for applied records and any refresh requests to send.
    pub(crate) fn apply_snapshot_state(
        &mut self,
        slot_table: &mut SlotTable,
        snapshot_sequence: u32,
        snapshot_fingerprint: &[u8; 32],
        records: &[RawStateSlotRecord],
    ) -> StateApplyOutcome {
        if records.is_empty() {
            return StateApplyOutcome::default();
        }

        // Validate the whole batch against the local schema. The schema borrow is
        // released before the slot-table mutation below.
        let typed = {
            let net_schema = self.net_schema(slot_table);
            match validate_state_records(net_schema, snapshot_fingerprint, records) {
                Ok(typed) => typed,
                Err(err) => {
                    log_state_validation_rejection(&err);
                    return StateApplyOutcome::default();
                }
            }
        };

        // Partition the validated records: applicable (full baseline, or a delta whose
        // ref the client holds) vs refresh-needed (delta against a missing baseline).
        let mut writes: Vec<(String, SlotValue)> = Vec::new();
        let mut pending_baselines: Vec<(StateSlotId, u32)> = Vec::new();
        let mut outcome = StateApplyOutcome::default();

        for record in &typed {
            match record {
                StateSlotRecord::FullBaseline {
                    slot_id,
                    baseline_id,
                    value,
                } => {
                    if let Some(write) = self.write_for(slot_table, *slot_id, value) {
                        writes.push(write);
                    }
                    pending_baselines.push((*slot_id, *baseline_id));
                }
                StateSlotRecord::Delta {
                    slot_id,
                    baseline_ref,
                    new_baseline_id,
                    value,
                } => {
                    if self.held_baselines.get(slot_id).copied() == Some(*baseline_ref) {
                        if let Some(write) = self.write_for(slot_table, *slot_id, value) {
                            writes.push(write);
                        }
                        pending_baselines.push((*slot_id, *new_baseline_id));
                    } else {
                        // Missing baseline: request a full refresh keyed by StateSlotId.
                        // Leave the slot untouched; the rest of the batch still applies.
                        outcome.refresh_requests.push(StateBaselineRefreshRequest {
                            snapshot_sequence,
                            slot_id: slot_id.0,
                            missing_baseline_ref: *baseline_ref,
                            reason: STATE_REFRESH_REASON_UNKNOWN_BASELINE,
                        });
                    }
                }
            }
        }

        // Atomic commit: prevalidate ALL mapped values, then write all or none. A store
        // rejection (type/range/enum/finite) leaves every slot unchanged AND advances
        // no baseline — the batch is rejected whole.
        if !writes.is_empty() {
            if let Err(err) = apply_store_slot_batch(slot_table, &writes) {
                log::warn!(
                    "[Net] replicated state batch rejected by store validation; slots unchanged: {err}"
                );
                return StateApplyOutcome::default();
            }
        }

        // Applied: advance held baselines and ack them.
        for (slot_id, baseline_id) in pending_baselines {
            self.held_baselines.insert(slot_id, baseline_id);
            outcome.slot_baselines.push((slot_id.0, baseline_id));
        }
        outcome
    }

    /// Map a validated record's `StateSlotId` to its dotted slot name and engine value,
    /// or `None` to skip the slot write (an `Unset` clears no Phase 3.5 player slot, and
    /// an unmapped id never reaches here — the batch was schema-validated). The schema
    /// borrow is taken read-only.
    fn write_for(
        &mut self,
        slot_table: &SlotTable,
        slot_id: StateSlotId,
        value: &WireSlotValue,
    ) -> Option<(String, SlotValue)> {
        let name = self.schema(slot_table).name_for(slot_id)?.to_string();
        let value = wire_value_to_slot(value)?;
        Some((name, value))
    }
}

impl Default for ClientStateApply {
    fn default() -> Self {
        Self::new()
    }
}

/// Log a stable, greppable diagnostic for a rejected replicated-state batch. The
/// fingerprint-mismatch line is the one the AC names ("the client logs a stable
/// mismatch diagnostic"); the others share the `[Net]` tag and a stable prefix.
fn log_state_validation_rejection(err: &StateValidationError) {
    match err {
        StateValidationError::SchemaFingerprintMismatch => {
            log::warn!(
                "[Net] replicated state schema fingerprint mismatch; dropping state records and keeping existing slot values"
            );
        }
        other => {
            log::warn!("[Net] replicated state batch rejected before apply: {other}");
        }
    }
}

// ---------------------------------------------------------------------------
// Engine <-> wire value conversion
// ---------------------------------------------------------------------------

/// Lower an engine [`SlotValue`] to its wire mirror. A non-finite number or array
/// element yields `None`: the source value came from the validated slot table (so it
/// is finite by construction), but a defensive `None` keeps a poisoned value off the
/// wire rather than letting the client reject the whole batch. Enum/string/boolean
/// always convert.
fn slot_value_to_wire(value: &SlotValue) -> Option<WireSlotValue> {
    match value {
        SlotValue::Number(n) if n.is_finite() => Some(WireSlotValue::Number(*n)),
        SlotValue::Number(_) => None,
        SlotValue::Boolean(b) => Some(WireSlotValue::Boolean(*b)),
        SlotValue::String(s) => Some(WireSlotValue::String(s.clone())),
        SlotValue::Enum(s) => Some(WireSlotValue::Enum(s.clone())),
        SlotValue::Array(values) if values.iter().all(|v| v.is_finite()) => {
            Some(WireSlotValue::Array(values.clone()))
        }
        SlotValue::Array(_) => None,
    }
}

/// Lift a wire [`WireSlotValue`] back to an engine [`SlotValue`] for the client apply
/// path. `Unset` has no engine value (the slot is cleared, which Phase 3.5 never does
/// for the player slots) so it yields `None`; the apply path skips an `Unset` record's
/// slot write. All other variants convert directly; type/range/enum/finite validation
/// runs again at the store-write boundary.
fn wire_value_to_slot(value: &WireSlotValue) -> Option<SlotValue> {
    match value {
        WireSlotValue::Unset => None,
        WireSlotValue::Number(n) => Some(SlotValue::Number(*n)),
        WireSlotValue::Boolean(b) => Some(SlotValue::Boolean(*b)),
        WireSlotValue::String(s) => Some(SlotValue::String(s.clone())),
        WireSlotValue::Enum(s) => Some(SlotValue::Enum(s.clone())),
        WireSlotValue::Array(values) => Some(SlotValue::Array(values.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::slot_table::{SlotOwnership, SlotRecord, SlotSchema};

    fn replicated_number(name: &str, scope: ReplicationScope) -> (String, SlotRecord) {
        let (_ns, slot) = name.split_once('.').unwrap();
        (
            slot.to_string(),
            SlotRecord::new(SlotSchema {
                slot_type: SlotType::Number,
                default: Some(SlotValue::Number(0.0)),
                range: None,
                persist: false,
                readonly: false,
                ownership: SlotOwnership::Mod,
                network: scope,
            }),
        )
    }

    /// A table with two replicated mod slots under one namespace plus the default
    /// engine slots. After the Task 4 flip the engine `player.health` /
    /// `player.maxHealth` slots are also replicated (owner-private), so the schema
    /// carries those two engine slots alongside the two mod slots.
    fn table_with_replicated() -> SlotTable {
        let mut table = SlotTable::new();
        table
            .insert_namespace(
                "net",
                vec![
                    replicated_number("net.bravo", ReplicationScope::SharedGlobal),
                    replicated_number("net.alpha", ReplicationScope::OwnerPrivatePlayer),
                ],
            )
            .unwrap();
        table
    }

    #[test]
    fn build_includes_only_replicated_slots_sorted_by_name() {
        let table = table_with_replicated();
        let schema = ReplicatedSlotSchema::build(&table);
        let names: Vec<&str> = schema.entries().iter().map(|e| e.name.as_str()).collect();
        // After the Task 4 catalog flip the two engine player slots are also
        // replicated (owner-private), so they join the two mod slots, all sorted
        // by dotted name.
        assert_eq!(
            names,
            vec![
                "net.alpha",
                "net.bravo",
                "player.health",
                "player.maxHealth"
            ]
        );
        assert_eq!(schema.entries()[0].slot_id, StateSlotId(0));
        assert_eq!(schema.entries()[1].slot_id, StateSlotId(1));
    }

    #[test]
    fn id_and_name_round_trip() {
        let table = table_with_replicated();
        let schema = ReplicatedSlotSchema::build(&table);
        let id = schema.id_for("net.alpha").expect("alpha is replicated");
        assert_eq!(schema.name_for(id), Some("net.alpha"));
        // After the Task 4 flip `player.health` is owner-private replicated, so it
        // now carries a `StateSlotId`.
        let health_id = schema
            .id_for("player.health")
            .expect("player.health is replicated after the Task 4 flip");
        assert_eq!(schema.name_for(health_id), Some("player.health"));
    }

    #[test]
    fn default_table_has_only_player_health_slots() {
        // The Task 4 catalog flip makes `player.health` / `player.maxHealth`
        // owner-private; every other built-in slot stays `None`. So the default
        // table's schema is exactly these two engine player slots.
        let table = SlotTable::new();
        let schema = ReplicatedSlotSchema::build(&table);
        let names: Vec<&str> = schema.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["player.health", "player.maxHealth"]);
        assert!(!schema.to_net_schema().is_empty());
    }

    #[test]
    fn fingerprint_is_deterministic_and_order_independent() {
        // Two tables that declare the same replicated slots in different insertion
        // order must produce the same fingerprint (the builder sorts by name).
        let schema_a = ReplicatedSlotSchema::build(&table_with_replicated());

        let mut table_b = SlotTable::new();
        table_b
            .insert_namespace(
                "net",
                vec![
                    replicated_number("net.alpha", ReplicationScope::OwnerPrivatePlayer),
                    replicated_number("net.bravo", ReplicationScope::SharedGlobal),
                ],
            )
            .unwrap();
        let schema_b = ReplicatedSlotSchema::build(&table_b);

        assert_eq!(schema_a.fingerprint(), schema_b.fingerprint());
    }

    #[test]
    fn fingerprint_changes_with_scope() {
        let schema_a = ReplicatedSlotSchema::build(&table_with_replicated());

        let mut table_b = SlotTable::new();
        table_b
            .insert_namespace(
                "net",
                vec![
                    // Same names, but alpha's scope flipped.
                    replicated_number("net.bravo", ReplicationScope::SharedGlobal),
                    replicated_number("net.alpha", ReplicationScope::SharedGlobal),
                ],
            )
            .unwrap();
        let schema_b = ReplicatedSlotSchema::build(&table_b);

        assert_ne!(schema_a.fingerprint(), schema_b.fingerprint());
    }

    #[test]
    fn net_schema_carries_fingerprint_and_descriptors() {
        let table = table_with_replicated();
        let schema = ReplicatedSlotSchema::build(&table);
        let net = schema.to_net_schema();
        assert_eq!(net.fingerprint(), schema.fingerprint());
        // Two mod slots plus the two owner-private engine player slots (Task 4 flip).
        assert_eq!(net.len(), 4);
        let alpha = net
            .descriptor(StateSlotId(0))
            .expect("alpha descriptor exists");
        assert_eq!(alpha.value_type, SlotValueType::Number);
        assert_eq!(alpha.scope, WireReplicationScope::OwnerPrivatePlayer);
    }

    #[test]
    fn infinite_range_edge_lowers_as_non_finite() {
        let mut table = SlotTable::new();
        table
            .insert_namespace(
                "net",
                vec![(
                    "capped".to_string(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(1.0)),
                        range: Some(NumericRange {
                            min: 1.0,
                            max: f32::INFINITY,
                        }),
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: ReplicationScope::SharedGlobal,
                    }),
                )],
            )
            .unwrap();
        let schema = ReplicatedSlotSchema::build(&table);
        let net = schema.to_net_schema();
        let range = net
            .descriptor(StateSlotId(0))
            .and_then(|d| d.range)
            .expect("range lowered");
        assert!(range.min_finite);
        assert!(!range.max_finite, "inf max lowers as non-finite");
    }

    // -----------------------------------------------------------------------
    // Task 3: engine production + client apply glue
    // -----------------------------------------------------------------------

    use crate::scripting::data_descriptors::HealthDescriptor;
    use crate::scripting::registry::Transform;

    const CLIENT_A: u64 = 1;
    const CLIENT_B: u64 = 2;

    /// A host slot table with one `SharedGlobal` (`net.objective`) and one
    /// `OwnerPrivatePlayer` (`net.private`) mod number slot. Both peers build this
    /// identically, so their schema fingerprints match. The engine player slots are
    /// cleared back to `None` so these mod-slot round-trip tests stay focused on
    /// exactly the two declared mod slots (the Task 4 catalog flip is exercised by
    /// the dedicated player-health tests below).
    fn shared_and_private_table() -> SlotTable {
        let mut table = SlotTable::new();
        table.get_mut("player.health").unwrap().schema.network = ReplicationScope::None;
        table.get_mut("player.maxHealth").unwrap().schema.network = ReplicationScope::None;
        table
            .insert_namespace(
                "net",
                vec![
                    replicated_number("net.objective", ReplicationScope::SharedGlobal),
                    replicated_number("net.private", ReplicationScope::OwnerPrivatePlayer),
                ],
            )
            .unwrap();
        table
    }

    /// A slot table whose `player.health` / `player.maxHealth` slots are owner-private
    /// replicated. The Task 4 catalog flip already sets this scope, so a plain
    /// `SlotTable::new()` carries it; both peers build this identically.
    fn player_health_replicated_table() -> SlotTable {
        let table = SlotTable::new();
        debug_assert_eq!(
            table.get("player.health").unwrap().schema.network,
            ReplicationScope::OwnerPrivatePlayer,
            "Task 4 catalog flip makes player.health owner-private"
        );
        table
    }

    /// Spawn one owned pawn for `client_id` carrying a `HealthComponent`, returning the
    /// registry, the owner map, and the pawn id.
    fn registry_with_owned_health(
        client_id: u64,
        current: f32,
        max: f32,
    ) -> (EntityRegistry, MovementOwners, EntityId) {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let mut health = HealthComponent::from_descriptor(&HealthDescriptor {
            max,
            hitbox: None,
            zone_multipliers: std::collections::HashMap::new(),
        });
        health.current = current;
        registry.set_component(pawn, health).unwrap();
        let mut owners = MovementOwners::new();
        owners.set(pawn, client_id);
        (registry, owners, pawn)
    }

    // A shared slot and an owner-private slot round-trip from host production into the
    // client slot table through the real produce/apply glue, sharing one wire schema.
    #[test]
    fn shared_and_owner_private_round_trip_through_glue() {
        let mut host_table = shared_and_private_table();
        // The host sets the shared objective value and an owner-private value (via the
        // table fallback path keyed per owner).
        host_table.get_mut("net.objective").unwrap().value = Some(SlotValue::Number(3.0));
        host_table.get_mut("net.private").unwrap().value = Some(SlotValue::Number(42.0));

        let (registry, owners, _pawn) = registry_with_owned_health(CLIENT_A, 0.0, 0.0);

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        let fingerprint = host.fingerprint(&host_table);
        let records = host
            .produce_for_client(&host_table, &registry, &owners, CLIENT_A, 0)
            .expect("registered client produces records");
        assert_eq!(records.len(), 2, "shared + owner-private both produced");

        // Client side: a fresh table (no values) and the apply glue.
        let mut client_table = shared_and_private_table();
        let mut client = ClientStateApply::new();
        let outcome = client.apply_snapshot_state(&mut client_table, 0, &fingerprint, &records);
        assert_eq!(
            outcome.slot_baselines.len(),
            2,
            "both records acked after apply"
        );
        assert!(outcome.refresh_requests.is_empty());
        assert_eq!(
            client_table.get("net.objective").unwrap().value,
            Some(SlotValue::Number(3.0)),
            "shared slot applied through the store-write path"
        );
        assert_eq!(
            client_table.get("net.private").unwrap().value,
            Some(SlotValue::Number(42.0)),
            "owner-private slot applied through the store-write path"
        );
    }

    // A descriptor-defined source value (health) projects into a named owner-private
    // slot and replicates through the SAME wire schema/apply path as store slots.
    #[test]
    fn descriptor_health_projects_and_replicates_like_a_store_slot() {
        // A table whose player health slots are owner-private replicated (the Task 4
        // catalog flip, set directly here so Task 3 can prove the descriptor path).
        let host_table = player_health_replicated_table();

        // The descriptor-fed source: an owned pawn with a live HealthComponent. No slot
        // value is ever written on the host — the value comes straight from the
        // component through the projection.
        let (registry, owners, _pawn) = registry_with_owned_health(CLIENT_A, 75.0, 100.0);

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        let fingerprint = host.fingerprint(&host_table);
        let records = host
            .produce_for_client(&host_table, &registry, &owners, CLIENT_A, 0)
            .expect("registered client produces records");
        assert_eq!(records.len(), 2, "health + maxHealth projected");

        // Client applies through the store path; the engine-owned readonly player slots
        // receive the replicated values (engine bypass honors readonly).
        let mut client_table = player_health_replicated_table();
        let mut client = ClientStateApply::new();
        let outcome = client.apply_snapshot_state(&mut client_table, 0, &fingerprint, &records);
        assert_eq!(outcome.slot_baselines.len(), 2);
        assert_eq!(
            client_table.get("player.health").unwrap().value,
            Some(SlotValue::Number(75.0)),
            "descriptor-fed current HP reached the named slot"
        );
        assert_eq!(
            client_table.get("player.maxHealth").unwrap().value,
            Some(SlotValue::Number(100.0)),
            "descriptor-fed max HP reached the named slot"
        );
    }

    // Client apply validates ALL records before mutating any slot: a fingerprint
    // mismatch rejects the whole batch and leaves every slot unchanged.
    #[test]
    fn fingerprint_mismatch_rejects_whole_batch_and_keeps_values() {
        let mut host_table = shared_and_private_table();
        host_table.get_mut("net.objective").unwrap().value = Some(SlotValue::Number(9.0));
        let (registry, owners, _pawn) = registry_with_owned_health(CLIENT_A, 0.0, 0.0);

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        let _real_fingerprint = host.fingerprint(&host_table);
        let records = host
            .produce_for_client(&host_table, &registry, &owners, CLIENT_A, 0)
            .expect("records");

        // The client holds a prior value the apply must NOT overwrite.
        let mut client_table = shared_and_private_table();
        client_table.get_mut("net.objective").unwrap().value = Some(SlotValue::Number(1.0));
        let mut client = ClientStateApply::new();

        // A WRONG fingerprint must reject the whole batch before any mutation.
        let outcome = client.apply_snapshot_state(&mut client_table, 0, &[0xAB; 32], &records);
        assert!(
            outcome.slot_baselines.is_empty(),
            "rejected batch acks nothing"
        );
        assert!(outcome.refresh_requests.is_empty());
        assert_eq!(
            client_table.get("net.objective").unwrap().value,
            Some(SlotValue::Number(1.0)),
            "fingerprint mismatch left the prior value unchanged"
        );
    }

    // Any single invalid record rejects the WHOLE batch: a type-mismatched record in a
    // batch leaves EVERY slot (including the otherwise-valid ones) unchanged.
    #[test]
    fn one_invalid_record_rejects_whole_batch_no_partial_apply() {
        let host_table = shared_and_private_table();
        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        let fingerprint = host.fingerprint(&host_table);

        // Hand-build a batch: a valid number record for net.objective (slot 0, sorted by
        // name: net.objective < net.private) and a TYPE-MISMATCHED boolean for the
        // number slot net.private (slot 1). The whole batch must reject.
        let schema = ReplicatedSlotSchema::build(&host_table);
        let objective_id = schema.id_for("net.objective").unwrap().0;
        let private_id = schema.id_for("net.private").unwrap().0;
        let records = vec![
            RawStateSlotRecord {
                slot_id: objective_id,
                kind: postretro_net::state_slots::STATE_RECORD_KIND_FULL_BASELINE,
                has_baseline_ref: false,
                baseline_ref: 0,
                baseline_id: 1,
                value: WireSlotValue::Number(5.0),
            },
            RawStateSlotRecord {
                slot_id: private_id,
                kind: postretro_net::state_slots::STATE_RECORD_KIND_FULL_BASELINE,
                has_baseline_ref: false,
                baseline_ref: 0,
                baseline_id: 1,
                value: WireSlotValue::Boolean(true), // type mismatch: net.private is a number
            },
        ];

        let mut client_table = shared_and_private_table();
        // Both slots default to 0.0; assert they stay at the default after rejection.
        let mut client = ClientStateApply::new();
        let outcome = client.apply_snapshot_state(&mut client_table, 0, &fingerprint, &records);
        assert!(
            outcome.slot_baselines.is_empty(),
            "a type mismatch rejects the whole batch (no partial apply)"
        );
        assert_eq!(
            client_table.get("net.objective").unwrap().value,
            Some(SlotValue::Number(0.0)),
            "the valid record's slot is unchanged because the batch rejected whole"
        );
        assert_eq!(
            client_table.get("net.private").unwrap().value,
            Some(SlotValue::Number(0.0)),
            "the invalid record's slot is unchanged"
        );
    }

    // Owner-private filtering through the glue: client B never receives client A's
    // private slot, and each sees its own descriptor-fed health.
    #[test]
    fn owner_private_health_is_per_client_through_glue() {
        let host_table = player_health_replicated_table();

        // Two owned pawns with distinct health, owned by A and B.
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::new();
        for (client, current, max) in [(CLIENT_A, 80.0_f32, 100.0_f32), (CLIENT_B, 40.0, 50.0)] {
            let pawn = registry.spawn(Transform::default());
            let mut health = HealthComponent::from_descriptor(&HealthDescriptor {
                max,
                hitbox: None,
                zone_multipliers: std::collections::HashMap::new(),
            });
            health.current = current;
            registry.set_component(pawn, health).unwrap();
            owners.set(pawn, client);
        }

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        host.register_client(CLIENT_B);
        let fingerprint = host.fingerprint(&host_table);

        let records_a = host
            .produce_for_client(&host_table, &registry, &owners, CLIENT_A, 0)
            .unwrap();
        let records_b = host
            .produce_for_client(&host_table, &registry, &owners, CLIENT_B, 0)
            .unwrap();

        // Each client's batch carries only ITS pawn's health.
        let mut table_a = player_health_replicated_table();
        let mut table_b = player_health_replicated_table();
        let mut client_a = ClientStateApply::new();
        let mut client_b = ClientStateApply::new();
        client_a.apply_snapshot_state(&mut table_a, 0, &fingerprint, &records_a);
        client_b.apply_snapshot_state(&mut table_b, 0, &fingerprint, &records_b);

        assert_eq!(
            table_a.get("player.health").unwrap().value,
            Some(SlotValue::Number(80.0)),
            "client A sees its own health"
        );
        assert_eq!(
            table_b.get("player.health").unwrap().value,
            Some(SlotValue::Number(40.0)),
            "client B sees its own (different) health"
        );
    }

    // -----------------------------------------------------------------------
    // Task 5: shared/global mod-slot proof + descriptor parse/materialize fixture
    // -----------------------------------------------------------------------

    use crate::scripting::primitives::store::store_declaration;

    /// The Task 5 integration fixture store: a mod-authored `defineStore` slot opted
    /// into `network: "shared"` through the real `store_declaration` parse path. This
    /// proves the replication path is general, not health-hardcoded — the shared slot
    /// is declared exactly as a mod author would write it, then committed into the slot
    /// table. The engine player health slots are cleared back to `None` so this fixture
    /// table carries exactly the one shared mod slot.
    fn net_fixture_table() -> SlotTable {
        let mut table = SlotTable::new();
        table.get_mut("player.health").unwrap().schema.network = ReplicationScope::None;
        table.get_mut("player.maxHealth").unwrap().schema.network = ReplicationScope::None;

        // Authored through the same parse path as a real `defineStore("netFixture", ...)`
        // call, so the SharedGlobal scope comes from the mod-facing `network: "shared"`
        // opt-in, not a hand-set field.
        let declaration = store_declaration(
            "netFixture",
            serde_json::json!({
                "objectiveProgress": { "type": "number", "default": 0, "network": "shared" },
            }),
        )
        .expect("netFixture schema parses");
        assert_eq!(
            declaration.records[0].1.schema.network,
            ReplicationScope::SharedGlobal,
            "network: \"shared\" lowered to SharedGlobal through the parse path"
        );
        table
            .insert_namespace(&declaration.namespace, declaration.records)
            .expect("netFixture commits");
        table
    }

    // A `sharedGlobal` fixture slot (`netFixture.objectiveProgress`, authored via
    // `network: "shared"`) replicates to EVERY accepted client and to a LATE JOINER
    // through a full baseline — proving the shared path through the Task 3 shared-ingest
    // glue, not just the entity HUD slots.
    #[test]
    fn shared_fixture_objective_progress_reaches_every_client_and_late_joiner() {
        let mut host_table = net_fixture_table();
        // The host advances the shared objective. One value per StateSlotId regardless
        // of owner — every accepted client sees the same number.
        host_table
            .get_mut("netFixture.objectiveProgress")
            .unwrap()
            .value = Some(SlotValue::Number(7.0));

        // No owned pawns are needed: the shared slot's source is the table value.
        let registry = EntityRegistry::new();
        let owners = MovementOwners::new();

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        host.register_client(CLIENT_B);
        let fingerprint = host.fingerprint(&host_table);

        // Both originally-accepted clients receive the shared value on the first frame.
        for client in [CLIENT_A, CLIENT_B] {
            let records = host
                .produce_for_client(&host_table, &registry, &owners, client, 0)
                .expect("accepted client produces records");
            assert_eq!(records.len(), 1, "the one shared fixture slot is produced");

            let mut client_table = net_fixture_table();
            let mut apply = ClientStateApply::new();
            let outcome = apply.apply_snapshot_state(&mut client_table, 0, &fingerprint, &records);
            assert_eq!(
                outcome.slot_baselines.len(),
                1,
                "the shared record is acked"
            );
            assert!(outcome.refresh_requests.is_empty());
            assert_eq!(
                client_table
                    .get("netFixture.objectiveProgress")
                    .unwrap()
                    .value,
                Some(SlotValue::Number(7.0)),
                "client {client} sees the shared objective progress"
            );
        }

        // A LATE JOINER (client C) accepts after the value was set and without any
        // further value change, then must still receive the full baseline.
        const CLIENT_C: u64 = 3;
        host.register_client(CLIENT_C);
        let late_records = host
            .produce_for_client(&host_table, &registry, &owners, CLIENT_C, 1)
            .expect("late joiner produces records");
        assert_eq!(
            late_records.len(),
            1,
            "late joiner gets a full baseline for the shared slot without a value change"
        );

        let mut late_table = net_fixture_table();
        let mut late_apply = ClientStateApply::new();
        let outcome =
            late_apply.apply_snapshot_state(&mut late_table, 1, &fingerprint, &late_records);
        assert_eq!(outcome.slot_baselines.len(), 1);
        assert_eq!(
            late_table
                .get("netFixture.objectiveProgress")
                .unwrap()
                .value,
            Some(SlotValue::Number(7.0)),
            "the late joiner converges to the shared objective progress"
        );
    }

    /// Spawn an owned pawn whose `HealthComponent` is materialized through the SAME
    /// descriptor parse → materialize path the engine uses: a `HealthDescriptor` parsed
    /// from descriptor JSON (`serde_json::from_value`, exactly the engine's parse step),
    /// validated, then materialized via `HealthComponent::from_descriptor` (the engine's
    /// materialize step). This proves the descriptor-fed projection flows through the
    /// real descriptor path, not a hand-built component.
    fn registry_with_descriptor_health(
        client_id: u64,
        max: f32,
    ) -> (EntityRegistry, MovementOwners) {
        // Parse step: descriptor JSON → HealthDescriptor (the engine's `serde_json`
        // parse path for `components.health`).
        let descriptor: HealthDescriptor =
            serde_json::from_value(serde_json::json!({ "max": max }))
                .expect("health descriptor parses");
        let descriptor = descriptor.validate().expect("health descriptor validates");

        // Materialize step: HealthComponent::from_descriptor (current initializes to max).
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        registry
            .set_component(pawn, HealthComponent::from_descriptor(&descriptor))
            .unwrap();

        let mut owners = MovementOwners::new();
        owners.set(pawn, client_id);
        (registry, owners)
    }

    // A descriptor-defined source value (health), materialized through the descriptor
    // PARSE/MATERIALIZE path, projects into the named `player.health` / `player.maxHealth`
    // slots and replicates through the SAME wire schema/apply path as store slots — using
    // a `StateSlotId` from the same deterministic schema.
    #[test]
    fn descriptor_parsed_health_projects_through_named_slots() {
        let host_table = player_health_replicated_table();
        let (registry, owners) = registry_with_descriptor_health(CLIENT_A, 120.0);

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        let fingerprint = host.fingerprint(&host_table);
        let records = host
            .produce_for_client(&host_table, &registry, &owners, CLIENT_A, 0)
            .expect("registered client produces records");
        assert_eq!(records.len(), 2, "health + maxHealth projected");

        // The slot ids come from the same deterministic schema as store slots.
        let schema = ReplicatedSlotSchema::build(&host_table);
        let health_id = schema.id_for("player.health").expect("health id");
        let max_id = schema.id_for("player.maxHealth").expect("maxHealth id");
        let record_ids: std::collections::BTreeSet<u16> =
            records.iter().map(|r| r.slot_id).collect();
        assert!(record_ids.contains(&health_id.0));
        assert!(record_ids.contains(&max_id.0));

        let mut client_table = player_health_replicated_table();
        let mut client = ClientStateApply::new();
        let outcome = client.apply_snapshot_state(&mut client_table, 0, &fingerprint, &records);
        assert_eq!(outcome.slot_baselines.len(), 2);
        assert_eq!(
            client_table.get("player.health").unwrap().value,
            Some(SlotValue::Number(120.0)),
            "descriptor-parsed current HP (== max at spawn) reached the named slot"
        );
        assert_eq!(
            client_table.get("player.maxHealth").unwrap().value,
            Some(SlotValue::Number(120.0)),
            "descriptor-parsed max HP reached the named slot"
        );
    }

    // -----------------------------------------------------------------------
    // Task 6: schema-mismatch logging, UI read-snapshot AC, and the
    // refresh/repair-through-the-glue seam (the conditioned-loss harness lives
    // in `state_slot_loss_harness_test`).
    // -----------------------------------------------------------------------

    use crate::scripting::reactions::log_capture::capture;

    // A schema fingerprint mismatch logs a STABLE, greppable diagnostic before any
    // mutation. The AC names "the client logs a stable mismatch diagnostic"; this
    // captures it with the existing log-capture helper and asserts stable substrings
    // (not the full line), so the message can be reworded without breaking the gate
    // as long as the load-bearing tokens stay.
    #[test]
    fn fingerprint_mismatch_logs_stable_diagnostic() {
        let host_table = shared_and_private_table();
        let (registry, owners, _pawn) = registry_with_owned_health(CLIENT_A, 0.0, 0.0);
        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        let _real_fingerprint = host.fingerprint(&host_table);
        let records = host
            .produce_for_client(&host_table, &registry, &owners, CLIENT_A, 0)
            .expect("records");

        let mut client_table = shared_and_private_table();
        let mut client = ClientStateApply::new();

        // Run the apply under a captured-log scope so the warn! is recorded.
        let logs = capture(|| {
            let outcome = client.apply_snapshot_state(&mut client_table, 0, &[0xAB; 32], &records);
            assert!(
                outcome.slot_baselines.is_empty(),
                "the mismatched batch acks nothing"
            );
        });

        // Stable substrings: the `[Net]` subsystem tag and the load-bearing tokens of
        // the mismatch diagnostic. Asserted as `contains`, never a full-line match.
        let matched = logs.iter().any(|(level, message)| {
            *level == log::Level::Warn
                && message.contains("[Net]")
                && message.contains("fingerprint mismatch")
        });
        assert!(
            matched,
            "expected a [Net] warn naming the fingerprint mismatch; captured: {logs:?}"
        );
    }

    /// Mirror the UI read-snapshot slot projection contract documented on
    /// `App::build_ui_slot_snapshot`: the snapshot carries every slot whose `value`
    /// is `Some`. Deriving the assertion from that destination contract (not from the
    /// netcode apply path) is the seam this AC test guards — the value the apply path
    /// writes must surface as a present key in the UI read snapshot.
    fn ui_slot_snapshot(slot_table: &SlotTable) -> HashMap<String, SlotValue> {
        slot_table
            .iter()
            .filter_map(|(name, record)| {
                record.value.clone().map(|value| (name.to_string(), value))
            })
            .collect()
    }

    // Acceptance metric: after applying the first full state baseline, the UI read
    // snapshot contains both `player.health` and `player.maxHealth` (the connected
    // client no longer renders them as missing). Drives the real host production →
    // client apply glue, then projects the slot table exactly as the UI read path does.
    #[test]
    fn first_baseline_populates_ui_read_snapshot_player_health_slots() {
        let host_table = player_health_replicated_table();
        let (registry, owners, _pawn) = registry_with_owned_health(CLIENT_A, 75.0, 100.0);

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        let fingerprint = host.fingerprint(&host_table);
        let records = host
            .produce_for_client(&host_table, &registry, &owners, CLIENT_A, 0)
            .expect("registered client produces records");

        // A fresh client table whose player slots have NO value yet: the UI read
        // snapshot must not carry them before the baseline lands.
        let mut client_table = player_health_replicated_table();
        client_table.get_mut("player.health").unwrap().value = None;
        client_table.get_mut("player.maxHealth").unwrap().value = None;
        let before = ui_slot_snapshot(&client_table);
        assert!(
            !before.contains_key("player.health") && !before.contains_key("player.maxHealth"),
            "before the baseline the player health slots are missing from the UI snapshot"
        );

        let mut client = ClientStateApply::new();
        client.apply_snapshot_state(&mut client_table, 0, &fingerprint, &records);

        let after = ui_slot_snapshot(&client_table);
        assert_eq!(
            after.get("player.health"),
            Some(&SlotValue::Number(75.0)),
            "player.health is present in the UI read snapshot after the first baseline"
        );
        assert_eq!(
            after.get("player.maxHealth"),
            Some(&SlotValue::Number(100.0)),
            "player.maxHealth is present in the UI read snapshot after the first baseline"
        );
    }

    // Missing-baseline repair through the glue: when the client receives a DELTA that
    // references a baseline it never held (the FullBaseline carrying it was lost), the
    // apply path emits a `StateBaselineRefresh` keyed by `StateSlotId` and leaves the
    // slot untouched; the server then schedules a FullBaseline that converges the slot
    // — all without reconnect. This is the refresh/repair seam the conditioned-loss
    // harness exercises end to end; here it is pinned deterministically at the glue.
    #[test]
    fn missing_baseline_delta_requests_refresh_then_repairs() {
        let mut host_table = shared_and_private_table();
        host_table.get_mut("net.objective").unwrap().value = Some(SlotValue::Number(3.0));
        let registry = EntityRegistry::new();
        let owners = MovementOwners::new();

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        let fingerprint = host.fingerprint(&host_table);

        // Frame 1: the host produces the first FullBaseline — but it is LOST (the
        // client never applies it, so it holds no baseline for net.objective).
        let _lost = host
            .produce_for_client(&host_table, &registry, &owners, CLIENT_A, 0)
            .expect("first frame records");

        // Frame 2: the value changes. With an acked baseline (from the host's view the
        // client never acked, so this is actually a FullBaseline fallback). To force a
        // genuine DELTA-against-missing on the client we have the host believe the
        // client acked frame 1, then drop frame 1 on the client.
        let baseline_one = {
            // Re-produce frame 1 to learn its baseline id, ack it on the server so the
            // server will send a delta next, but the CLIENT never saw it.
            let records = host
                .produce_for_client(&host_table, &registry, &owners, CLIENT_A, 1)
                .expect("frame 1 reproduced");
            let objective = records
                .iter()
                .find(|r| r.kind == postretro_net::state_slots::STATE_RECORD_KIND_FULL_BASELINE)
                .expect("a full baseline for the unacked objective");
            objective.baseline_id
        };
        host.apply_ack(CLIENT_A, 1, &[(0, baseline_one)]);

        // Now the value changes: the server emits a DELTA referencing baseline_one.
        host_table.get_mut("net.objective").unwrap().value = Some(SlotValue::Number(4.0));
        let delta_records = host
            .produce_for_client(&host_table, &registry, &owners, CLIENT_A, 2)
            .expect("delta frame");
        assert!(
            delta_records
                .iter()
                .any(|r| r.kind == postretro_net::state_slots::STATE_RECORD_KIND_DELTA),
            "the server sends a delta against the (client-missing) baseline"
        );

        // The client never applied frame 1, so it holds no baseline: applying the delta
        // must request a refresh and leave the slot untouched.
        let mut client_table = shared_and_private_table();
        client_table.get_mut("net.objective").unwrap().value = None;
        let mut client = ClientStateApply::new();
        let outcome =
            client.apply_snapshot_state(&mut client_table, 2, &fingerprint, &delta_records);
        assert_eq!(
            outcome.refresh_requests.len(),
            1,
            "a delta against a missing baseline requests exactly one refresh"
        );
        assert_eq!(
            outcome.refresh_requests[0].slot_id, 0,
            "the refresh is keyed by the StateSlotId of net.objective"
        );
        assert_eq!(
            client_table.get("net.objective").unwrap().value,
            None,
            "the slot is left untouched until the refresh repairs it"
        );

        // Server handles the refresh and schedules a FullBaseline for that slot.
        let req = &outcome.refresh_requests[0];
        host.request_refresh(CLIENT_A, req.slot_id, req.missing_baseline_ref);
        let repair_records = host
            .produce_for_client(&host_table, &registry, &owners, CLIENT_A, 3)
            .expect("repair frame");
        assert!(
            repair_records
                .iter()
                .any(|r| r.kind == postretro_net::state_slots::STATE_RECORD_KIND_FULL_BASELINE),
            "the refresh forces a full baseline"
        );

        // The client applies the repair and converges — no reconnect needed.
        let repair_outcome =
            client.apply_snapshot_state(&mut client_table, 3, &fingerprint, &repair_records);
        assert!(repair_outcome.refresh_requests.is_empty(), "repaired");
        assert_eq!(
            client_table.get("net.objective").unwrap().value,
            Some(SlotValue::Number(4.0)),
            "the slot converges to the authoritative value after refresh repair"
        );
    }
}
