// Authoritative state-slot replication projects each owner's component state and applies validated client slot updates.
// See: context/lib/networking.md §Game-logic-owned apply invariant · context/lib/scripting.md §5

use std::borrow::Cow;
use std::collections::BTreeSet;

use postretro_net::state_slots::{
    NumericRange as WireNumericRange, ReplicationScope as WireReplicationScope, SlotValueType,
    StateSchema, StateSlotDescriptor, StateSlotId,
};

use postretro_entities::slot_table::SlotOwnership;
use postretro_entities::{
    AmmoReserve, EntityRegistry, NumericRange, ReplicationScope, SlotTable, SlotType, SlotValue,
};
use postretro_scripting_core::StoreIdentityLedger;

/// Version prefix folded into the schema fingerprint. Bump when the canonical byte
/// stream's *shape* changes (a new field, a reordered tag) so an old client's
/// fingerprint can never accidentally match a new server's.
const FINGERPRINT_STREAM_VERSION: u8 = 3;

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

const WIRE_SHAPE_PLAIN: u8 = 0;
const WIRE_SHAPE_WIELDABLE_SLOT_NUMBER: u8 = 1;
const WEAPON_COOLDOWN_SLOT: &str = "player.weaponCooldownMs";

/// Some engine slots need source identity to make their value meaningful. The
/// script-facing slot keeps its ordinary type; only its replicated wire sample is
/// widened. The schema fingerprint prevents a stale peer from interpreting the
/// correlated sample as an ordinary number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReplicatedWireShape {
    Plain,
    WieldableSlotNumber,
}

impl ReplicatedWireShape {
    fn for_name(name: &str) -> Self {
        if name == WEAPON_COOLDOWN_SLOT {
            Self::WieldableSlotNumber
        } else {
            Self::Plain
        }
    }

    fn fingerprint_tag(self) -> u8 {
        match self {
            Self::Plain => WIRE_SHAPE_PLAIN,
            Self::WieldableSlotNumber => WIRE_SHAPE_WIELDABLE_SLOT_NUMBER,
        }
    }
}

/// The committed runtime inputs that establish mod-slot schema membership and
/// replication identity.
///
/// This is deliberately a snapshot passed from `ScriptRuntime` callers, never a
/// path that reads `identity.json` while building a network schema.
#[derive(Clone, Debug, Default)]
pub(crate) struct ReplicatedSlotIdentity<'a> {
    mod_id: Option<Cow<'a, str>>,
    ledger: Option<Cow<'a, StoreIdentityLedger>>,
    committed_store_slots: Cow<'a, BTreeSet<String>>,
}

impl<'a> ReplicatedSlotIdentity<'a> {
    #[cfg(test)]
    pub(crate) fn new(
        mod_id: Option<String>,
        ledger: Option<StoreIdentityLedger>,
        committed_store_slots: BTreeSet<String>,
    ) -> Self {
        Self {
            mod_id: mod_id.map(Cow::Owned),
            ledger: ledger.map(Cow::Owned),
            committed_store_slots: Cow::Owned(committed_store_slots),
        }
    }

    pub(crate) fn borrowed(
        mod_id: Option<&'a str>,
        ledger: Option<&'a StoreIdentityLedger>,
        committed_store_slots: &'a BTreeSet<String>,
    ) -> Self {
        Self {
            mod_id: mod_id.map(Cow::Borrowed),
            ledger: ledger.map(Cow::Borrowed),
            committed_store_slots: Cow::Borrowed(committed_store_slots),
        }
    }

    fn mod_id(&self) -> Option<&str> {
        self.mod_id.as_deref()
    }

    fn durable_key(&self, authored_name: &str) -> Option<&str> {
        self.ledger
            .as_deref()
            .and_then(|ledger| ledger.durable_key(authored_name))
    }

    fn is_committed(&self, authored_name: &str) -> bool {
        self.committed_store_slots.contains(authored_name)
    }
}

/// One replicated slot in the deterministic schema: its authored dotted name,
/// replication-identity string, assigned wire id, declared type, validation shape,
/// and replication scope. The engine keeps the authored `name` for the apply path
/// (mapping a `StateSlotId` back to a slot-table write); `identity` alone drives sort
/// order and fingerprinting. The net descriptor drops both strings.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ReplicatedSlotSchemaEntry {
    pub(crate) slot_id: StateSlotId,
    pub(crate) name: String,
    pub(crate) identity: String,
    pub(crate) slot_type: SlotType,
    pub(crate) range: Option<NumericRange>,
    pub(crate) scope: ReplicationScope,
    wire_shape: ReplicatedWireShape,
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
    /// `StateSlotId` and do not affect the fingerprint. Engine-catalog slots retain
    /// their dotted name as identity. Currently declared mod slots sort by
    /// `<modId>:<durableKey>` and require an entry in the committed identity
    /// snapshot. Retained live slots absent from current declaration membership are
    /// excluded before the ledger is consulted.
    pub(crate) fn build(
        slot_table: &SlotTable,
        replication_identity: &ReplicatedSlotIdentity<'_>,
    ) -> Self {
        let mut replicated: Vec<(
            String,
            &str,
            &SlotType,
            Option<NumericRange>,
            ReplicationScope,
        )> = slot_table
            .iter()
            .filter_map(|(name, record)| {
                let scope = record.schema.network;
                if scope == ReplicationScope::None {
                    return None;
                }

                let identity = match record.schema.ownership {
                    SlotOwnership::Engine => name.to_string(),
                    SlotOwnership::Mod => {
                        if !replication_identity.is_committed(name) {
                            return None;
                        }
                        let Some(mod_id) = replication_identity.mod_id() else {
                            log::warn!(
                                "[Net] excluding replicated mod state slot `{name}` from schema: committed mod identity is unavailable"
                            );
                            return None;
                        };
                        let Some(durable_key) = replication_identity.durable_key(name) else {
                            log::warn!(
                                "[Net] excluding replicated mod state slot `{name}` from schema: no durable identity ledger entry"
                            );
                            return None;
                        };
                        format!("{mod_id}:{durable_key}")
                    }
                };
                Some((
                    identity,
                    name,
                    &record.schema.slot_type,
                    record.schema.range,
                    scope,
                ))
            })
            .collect();
        // Sort by replication identity so both peers assign identical dense ids.
        replicated.sort_by(|left, right| left.0.cmp(&right.0));

        let entries: Vec<ReplicatedSlotSchemaEntry> = replicated
            .into_iter()
            .enumerate()
            .map(
                |(index, (identity, name, slot_type, range, scope))| ReplicatedSlotSchemaEntry {
                    slot_id: StateSlotId(index as u16),
                    name: name.to_string(),
                    identity,
                    slot_type: slot_type.clone(),
                    range,
                    scope,
                    wire_shape: ReplicatedWireShape::for_name(name),
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

    fn entry_for(&self, slot_id: StateSlotId) -> Option<&ReplicatedSlotSchemaEntry> {
        self.entries.iter().find(|entry| entry.slot_id == slot_id)
    }

    /// The wire id for a dotted slot name, or `None` if the slot is not replicated.
    #[cfg(test)]
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
            value_type: match self.wire_shape {
                ReplicatedWireShape::Plain => slot_type_to_wire(&self.slot_type),
                ReplicatedWireShape::WieldableSlotNumber => SlotValueType::Array,
            },
            range: match self.wire_shape {
                ReplicatedWireShape::Plain => self.range.map(numeric_range_to_wire),
                ReplicatedWireShape::WieldableSlotNumber => None,
            },
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
        ReplicationScope::None => {
            unreachable!(
                "None-scoped slots are filtered out by ReplicatedSlotSchema::build before lowering"
            )
        }
    }
}

/// Compute the 32-byte schema fingerprint over a canonical byte stream:
/// version prefix, then for each replicated slot in id (== sorted-identity) order:
/// length-prefixed UTF-8 replication identity, explicit type tag, enum values in declared order
/// (count + length-prefixed UTF-8), range finite/min/max flags with stable
/// little-endian numeric bytes, and the scope tag. Computed in `postretro` with the
/// workspace `blake3`; `postretro-net` stores the result as opaque bytes.
fn compute_fingerprint(entries: &[ReplicatedSlotSchemaEntry]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&[FINGERPRINT_STREAM_VERSION]);
    hasher.update(&(entries.len() as u32).to_le_bytes());

    for entry in entries {
        hasher.update(&entry.slot_id.0.to_le_bytes());
        write_len_prefixed_str(&mut hasher, &entry.identity);

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
        hasher.update(&[entry.wire_shape.fingerprint_tag()]);

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
            ReplicationScope::None => {
                unreachable!(
                    "None-scoped slots are filtered out by ReplicatedSlotSchema::build before lowering"
                )
            }
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
// Server-side production glue
// ---------------------------------------------------------------------------

use postretro_net::state_replication::ServerStateReplication;
use postretro_net::state_slots::{RawStateSlotRecord, WireSlotValue};

use crate::netcode::command_queue::{MovementOwners, WeaponOwners};
use postretro_entities::EntityId;
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::inventory::{Inventory, WIELDABLE_SLOT_CAPACITY};
use postretro_entities::components::weapon::WeaponComponent;

/// Host-side replicated-state production: owns the deterministic replicated-slot
/// schema (rebuilt lazily from the live `SlotTable` for each committed staged-manifest
/// generation) and the registry-blind [`ServerStateReplication`] tracker. Lives on the
/// `NetEndpoint::Host` variant; the frame send path (`net_serialize_and_send` →
/// `host_replicate`) ingests this frame's projected values, then produces per-client
/// state records to splice into the entity snapshot envelope.
///
/// The schema is the only place the engine maps `StateSlotId <-> dotted name`; the
/// net tracker never sees a name. Both peers build the schema identically from the
/// same content, so a fingerprint match is the cross-peer agreement gate.
pub(crate) struct HostStateReplication {
    /// Built lazily after each committed staged-manifest generation, then reused until
    /// the next reset. `None` until built.
    schema: Option<ReplicatedSlotSchema>,
    tracker: ServerStateReplication,
    /// Set to `true` the first time frame ingest runs. Used only in debug builds to
    /// assert the ingest-before-produce ordering contract.
    #[cfg(debug_assertions)]
    ingested: bool,
}

impl HostStateReplication {
    pub(crate) fn new() -> Self {
        Self {
            schema: None,
            tracker: ServerStateReplication::new(),
            #[cfg(debug_assertions)]
            ingested: false,
        }
    }

    /// Build the schema from the live slot table after a reset, returning a reference.
    /// Idempotent within one committed staged-manifest generation. Called inside the
    /// frame send path, after mod stores commit, so it reflects that generation's slots.
    fn schema(
        &mut self,
        slot_table: &SlotTable,
        replication_identity: &ReplicatedSlotIdentity<'_>,
    ) -> &ReplicatedSlotSchema {
        self.schema
            .get_or_insert_with(|| ReplicatedSlotSchema::build(slot_table, replication_identity))
    }

    /// The local schema fingerprint, building the schema if needed. Stamped into every
    /// snapshot carrying state records so the client gates on a match.
    pub(crate) fn fingerprint(
        &mut self,
        slot_table: &SlotTable,
        replication_identity: &ReplicatedSlotIdentity<'_>,
    ) -> [u8; 32] {
        *self.schema(slot_table, replication_identity).fingerprint()
    }

    /// Register a participating client so it receives state records. This is idempotent
    /// and re-registers every current participant after a schema rebuild.
    pub(crate) fn register_client(&mut self, client_id: u64) {
        self.tracker.register_client(client_id);
    }

    /// Drop a client's per-client state and owner-private values on any participation
    /// exit, including demotion and close.
    pub(crate) fn remove_client(&mut self, client_id: u64) {
        self.tracker.remove_client(client_id);
    }

    /// The source declarations changed or the level lifetime ended. Rebuild lazily
    /// on the next send rather than comparing partial store-reconcile plans.
    pub(crate) fn reset_schema(&mut self) {
        self.schema = None;
        self.tracker.reset_schema_state();
        #[cfg(debug_assertions)]
        {
            self.ingested = false;
        }
    }

    /// Rebuild after a declaration commit without dropping live participants.
    /// Their old baseline state is invalid under the new schema, so each client is
    /// re-registered against a fresh tracker and receives full baselines next send.
    pub(crate) fn reset_schema_for_clients(
        &mut self,
        participating_clients: impl IntoIterator<Item = u64>,
    ) {
        self.reset_schema();
        for client_id in participating_clients {
            self.register_client(client_id);
        }
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

    /// Produce the per-client state records to splice into `client_id`'s snapshot.
    /// Returns `None` for an unregistered (pending/rejected/closed) client, so such a
    /// client receives no state records.
    ///
    /// PRODUCE ONLY — the caller must run frame ingest ONCE per frame BEFORE the
    /// per-client produce loop. Ingest is a frame-wide registry/table scan; running it
    /// once per client would repeat it O(clients) times. To keep one ack per frame, the
    /// caller passes the shared `sequence` from the entity tracker's batch.
    pub(crate) fn produce_for_client(
        &mut self,
        client_id: u64,
        sequence: u32,
    ) -> Option<Vec<RawStateSlotRecord>> {
        #[cfg(debug_assertions)]
        debug_assert!(
            self.ingested,
            "produce_for_client called before frame ingest; ingest must run once per frame before the per-client produce loop"
        );
        self.tracker.produce_in_batch(client_id, sequence)
    }

    /// Test-only convenience wrapper around frame ingest. Production consumes the
    /// sampled-weapon result to clear accepted reload feedback.
    /// Collect and ingest this frame's authoritative source values into the tracker.
    /// Shared slots take the slot table's current value; owner-private slots take a
    /// per-owner value (descriptor-fed health from each owned pawn's `HealthComponent`,
    /// else the slot's table value keyed to each owner). A slot with no source value
    /// this frame is simply not ingested (it keeps its prior tracked value, or stays
    /// absent).
    ///
    /// Run ONCE per frame before the per-client produce loop: the scan is frame-wide,
    /// not per-client.
    #[cfg(test)]
    pub(crate) fn ingest_frame(
        &mut self,
        slot_table: &SlotTable,
        replication_identity: &ReplicatedSlotIdentity<'_>,
        registry: &EntityRegistry,
        owners: &MovementOwners,
        weapon_owners: &WeaponOwners,
    ) {
        let _ = self.ingest_frame_and_collect_sampled_weapons(
            slot_table,
            replication_identity,
            registry,
            owners,
            weapon_owners,
        );
    }

    /// Ingest one frame and report only weapons whose owner-private reload
    /// values were actually sampled.
    pub(crate) fn ingest_frame_and_collect_sampled_weapons(
        &mut self,
        slot_table: &SlotTable,
        replication_identity: &ReplicatedSlotIdentity<'_>,
        registry: &EntityRegistry,
        owners: &MovementOwners,
        weapon_owners: &WeaponOwners,
    ) -> Vec<EntityId> {
        // Snapshot the schema entries we need (id, name, scope) so the schema borrow is
        // released before the `&mut self.tracker` calls below.
        let entries: Vec<(StateSlotId, String, ReplicationScope)> = self
            .schema(slot_table, replication_identity)
            .entries()
            .iter()
            .map(|e| (e.slot_id, e.name.clone(), e.scope))
            .collect();
        let owner_projections: Vec<(EntityId, u64, AmmoSlotProjection)> = owners
            .iter()
            .map(|(pawn, client_id)| {
                (
                    pawn,
                    client_id,
                    AmmoSlotProjection::for_pawn(registry, pawn),
                )
            })
            .collect();
        let sampled_weapons = owner_projections
            .iter()
            .filter_map(|(_, _, projection)| projection.weapon)
            .collect();

        #[cfg(debug_assertions)]
        {
            self.ingested = true;
        }

        for (slot_id, name, scope) in entries {
            match scope {
                ReplicationScope::None => {}
                ReplicationScope::SharedGlobal => {
                    if let Some(value) = shared_source_value(slot_table, &name) {
                        self.tracker.ingest_shared(slot_id, value);
                    }
                }
                ReplicationScope::OwnerPrivatePlayer => {
                    for (pawn, client_id, ammo_projection) in &owner_projections {
                        if let Some(value) = owner_private_source_value(
                            slot_table,
                            registry,
                            &name,
                            *pawn,
                            weapon_owners,
                            ammo_projection,
                        ) {
                            self.tracker
                                .ingest_owner_private(slot_id, *client_id, value);
                        }
                    }
                }
            }
        }
        sampled_weapons
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

/// The per-owner source value for an owner-private slot. Descriptor-fed player slots
/// read from owner-specific component state: `player.health` / `player.maxHealth`
/// from the owning pawn's live `HealthComponent`; weapon cooldown, ammo, and
/// reload state resolve through its `Inventory` to the sibling `WeaponComponent`;
/// ammo reserve reads the owning pawn's `AmmoReserve`. Per-owner mod slots read
/// the value for this pawn's seat before the global fallback. Any other
/// owner-private slot falls back to the slot table's current global value.
/// `None` when no source value exists.
fn owner_private_source_value(
    slot_table: &SlotTable,
    registry: &EntityRegistry,
    name: &str,
    pawn: EntityId,
    _weapon_owners: &WeaponOwners,
    ammo_projection: &AmmoSlotProjection,
) -> Option<WireSlotValue> {
    if let Some(value) = descriptor_health_for_pawn(registry, name, pawn) {
        return slot_value_to_wire(&value);
    }
    if let Some(value) = descriptor_weapon_cooldown_for_pawn(registry, name, pawn) {
        return Some(value);
    }
    if let Some(value) = ammo_projection.slot_value(name) {
        return value.as_ref().and_then(slot_value_to_wire);
    }
    if let Some(value) = per_owner_slot_value_for_pawn(slot_table, registry, name, pawn) {
        return value;
    }
    let record = slot_table.get(name)?;
    let value = record.value.as_ref()?;
    slot_value_to_wire(value)
}

/// Read a per-owner slot for one pawn's durable seat. The outer option says
/// whether this declaration is per-owner; the inner option is its source value.
/// A per-owner slot with no pawn-seat binding deliberately returns `Some(None)`,
/// preventing the global scalar fallback from leaking another owner's value.
fn per_owner_slot_value_for_pawn(
    slot_table: &SlotTable,
    registry: &EntityRegistry,
    name: &str,
    pawn: EntityId,
) -> Option<Option<WireSlotValue>> {
    let record = slot_table.get(name)?;
    if !record.schema.per_owner {
        return None;
    }
    let Some(seat) = registry.seat_for_pawn(pawn) else {
        return Some(None);
    };
    Some(record.per_seat_value(seat).and_then(slot_value_to_wire))
}

/// Project ammo/reload slots from one owner's pawn and sibling weapon.
/// The outer `Option` identifies names owned by this projection; the inner
/// option is absent when ammo has no valid source, preventing fallback to the
/// host's global HUD slots and cross-owner leakage.
#[cfg(test)]
fn descriptor_ammo_for_pawn(
    registry: &EntityRegistry,
    name: &str,
    pawn: EntityId,
    _weapon_owners: &WeaponOwners,
) -> Option<Option<SlotValue>> {
    AmmoSlotProjection::for_pawn(registry, pawn).slot_value(name)
}

struct AmmoSlotProjection {
    weapon: Option<EntityId>,
    magazine: Option<f32>,
    reserve: Option<f32>,
    reload_progress: f32,
    reload_active: bool,
}

impl AmmoSlotProjection {
    fn for_pawn(registry: &EntityRegistry, pawn: EntityId) -> Self {
        let weapon = if registry.exists(pawn) {
            super::active_wieldable_for_pawn(registry, pawn)
        } else {
            None
        }
        .filter(|weapon| registry.get_component::<WeaponComponent>(*weapon).is_ok());
        let component =
            weapon.and_then(|weapon| registry.get_component::<WeaponComponent>(weapon).ok());
        let (reload_progress, reload_active) = component
            .map(WeaponComponent::owner_reload_status)
            .unwrap_or((0.0, false));
        let mut magazine = None;
        let mut reserve = None;
        if let Some(weapon) = component {
            let effective = weapon.effective();
            if let Some(ammo) = effective.ammo {
                magazine = Some(weapon.magazine as f32);
                reserve = Some(
                    registry
                        .get_component::<AmmoReserve>(pawn)
                        .map_or(0, |reserve| reserve.available(ammo.ammo_type))
                        as f32,
                );
            }
        }

        Self {
            weapon,
            magazine,
            reserve,
            reload_progress,
            reload_active,
        }
    }

    fn slot_value(&self, name: &str) -> Option<Option<SlotValue>> {
        let value = match name {
            "player.ammo" => self.magazine.map(SlotValue::Number),
            "player.ammoReserve" => self.reserve.map(SlotValue::Number),
            "player.reloadProgress" => Some(SlotValue::Number(self.reload_progress)),
            "player.reloadActive" => Some(SlotValue::Boolean(self.reload_active)),
            _ => return None,
        };
        Some(value)
    }
}

/// Read the descriptor-fed health value for `name` from `pawn`'s live
/// `HealthComponent`, the first descriptor-defined replicated source (M15 Phase 3.5).
/// `player.health` → current HP, `player.maxHealth` → max HP. `None` for any other
/// name or a pawn carrying no `HealthComponent`. The production path reads each owned
/// pawn's component per-owner, so each client's snapshot carries its own health.
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

/// Read the owner-private active weapon cooldown for `pawn`. The value is not on
/// the pawn: its inventory identifies the active sibling weapon entity.
fn descriptor_weapon_cooldown_for_pawn(
    registry: &EntityRegistry,
    name: &str,
    pawn: EntityId,
) -> Option<WireSlotValue> {
    if name != WEAPON_COOLDOWN_SLOT {
        return None;
    }
    let inventory = registry.get_component::<Inventory>(pawn).ok()?;
    let weapon = inventory.active_wieldable()?;
    let component = registry.get_component::<WeaponComponent>(weapon).ok()?;
    Some(WireSlotValue::Array(vec![
        inventory.active_slot as f32,
        component.cooldown_remaining_ms,
    ]))
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
    pub(crate) fresh_slots: Vec<String>,
    /// Source slot carried atomically with a fresh owner-private cooldown sample.
    pub(crate) fresh_weapon_cooldown_slot: Option<usize>,
}

struct PendingSlotWrite {
    name: String,
    value: SlotValue,
    wieldable_slot: Option<usize>,
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

    /// Drop a schema derived from declarations no longer installed.
    pub(crate) fn reset_schema(&mut self) {
        self.schema = None;
        self.net_schema = None;
        self.held_baselines.clear();
    }

    #[cfg(test)]
    pub(crate) fn is_reset(&self) -> bool {
        self.schema.is_none() && self.net_schema.is_none() && self.held_baselines.is_empty()
    }

    /// Build the schema (and its lowered net form) once from the live slot table,
    /// returning a reference to the replicated-slot schema for name lookups.
    fn schema(
        &mut self,
        slot_table: &SlotTable,
        replication_identity: &ReplicatedSlotIdentity<'_>,
    ) -> &ReplicatedSlotSchema {
        self.ensure_built(slot_table, replication_identity);
        self.schema.as_ref().expect("schema built above")
    }

    /// The lowered net schema, building both forms once if needed.
    fn net_schema(
        &mut self,
        slot_table: &SlotTable,
        replication_identity: &ReplicatedSlotIdentity<'_>,
    ) -> &StateSchema {
        self.ensure_built(slot_table, replication_identity);
        self.net_schema.as_ref().expect("net schema built above")
    }

    fn ensure_built(
        &mut self,
        slot_table: &SlotTable,
        replication_identity: &ReplicatedSlotIdentity<'_>,
    ) {
        if self.schema.is_none() {
            let schema = ReplicatedSlotSchema::build(slot_table, replication_identity);
            self.net_schema = Some(schema.to_net_schema());
            self.schema = Some(schema);
        }
    }

    /// Validate and apply one snapshot's replicated-state records. Two rejection
    /// classes behave differently:
    ///
    /// - **Structural / schema rejection** — a fingerprint mismatch, or any record that
    ///   fails schema validation (unknown slot id, type mismatch, non-finite, over-cap)
    ///   or the store's own type/range/enum/finite check — rejects the WHOLE batch and
    ///   leaves every slot unchanged (no partial apply, no baseline advance). A
    ///   fingerprint mismatch logs a stable diagnostic; the batch is dropped.
    /// - **Delta against a missing baseline** — a single delta referencing a baseline
    ///   the client does not hold is EXCLUDED from this batch and triggers a refresh
    ///   request, while the rest of the batch still applies normally. It does not reject
    ///   the batch.
    ///
    /// On success, every applicable record's value is written through the atomic
    /// store-batch helper (which prevalidates all mapped values, then commits all or
    /// none), so the slot table's own validation runs too. Returns the acks for applied
    /// records and any refresh requests to send.
    pub(crate) fn apply_snapshot_state(
        &mut self,
        slot_table: &mut SlotTable,
        replication_identity: &ReplicatedSlotIdentity<'_>,
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
            let net_schema = self.net_schema(slot_table, replication_identity);
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
        let mut writes: Vec<PendingSlotWrite> = Vec::new();
        let mut pending_baselines: Vec<(StateSlotId, u32)> = Vec::new();
        let mut outcome = StateApplyOutcome::default();

        for record in &typed {
            match record {
                StateSlotRecord::FullBaseline {
                    slot_id,
                    baseline_id,
                    value,
                } => {
                    match self.write_for(slot_table, replication_identity, *slot_id, value) {
                        Ok(Some(write)) => writes.push(write),
                        Ok(None) => {}
                        Err(reason) => {
                            log::warn!(
                                "[Net] replicated state batch rejected before apply: {reason}"
                            );
                            return StateApplyOutcome::default();
                        }
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
                        match self.write_for(slot_table, replication_identity, *slot_id, value) {
                            Ok(Some(write)) => writes.push(write),
                            Ok(None) => {}
                            Err(reason) => {
                                log::warn!(
                                    "[Net] replicated state batch rejected before apply: {reason}"
                                );
                                return StateApplyOutcome::default();
                            }
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
            let store_writes = writes
                .iter()
                .map(|write| (write.name.clone(), write.value.clone()))
                .collect::<Vec<_>>();
            if let Err(err) = apply_store_slot_batch(slot_table, &store_writes) {
                log::warn!(
                    "[Net] replicated state batch rejected by store validation; slots unchanged: {err}"
                );
                return StateApplyOutcome::default();
            }
            outcome
                .fresh_slots
                .extend(writes.iter().map(|write| write.name.clone()));
            outcome.fresh_weapon_cooldown_slot =
                writes.iter().rev().find_map(|write| write.wieldable_slot);
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
        replication_identity: &ReplicatedSlotIdentity<'_>,
        slot_id: StateSlotId,
        value: &WireSlotValue,
    ) -> Result<Option<PendingSlotWrite>, String> {
        let Some(entry) = self
            .schema(slot_table, replication_identity)
            .entry_for(slot_id)
            .cloned()
        else {
            return Ok(None);
        };
        match entry.wire_shape {
            ReplicatedWireShape::Plain => {
                Ok(wire_value_to_slot(value).map(|value| PendingSlotWrite {
                    name: entry.name,
                    value,
                    wieldable_slot: None,
                }))
            }
            ReplicatedWireShape::WieldableSlotNumber => {
                let WireSlotValue::Array(sample) = value else {
                    return Err(format!(
                        "correlated cooldown slot {} did not carry an array",
                        slot_id.0
                    ));
                };
                let [slot, cooldown_ms] = sample.as_slice() else {
                    return Err(format!(
                        "correlated cooldown slot {} carried {} fields instead of 2",
                        slot_id.0,
                        sample.len()
                    ));
                };
                if slot.fract() != 0.0 || *slot < 0.0 || *slot >= WIELDABLE_SLOT_CAPACITY as f32 {
                    return Err(format!(
                        "correlated cooldown slot {} carried invalid wieldable slot {slot}",
                        slot_id.0
                    ));
                }
                Ok(Some(PendingSlotWrite {
                    name: entry.name,
                    value: SlotValue::Number(*cooldown_ms),
                    wieldable_slot: Some(*slot as usize),
                }))
            }
        }
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
    use postretro_entities::components::weapon::{ReloadFeedback, WeaponAmmoTuning};
    use postretro_entities::components::wieldable_state::WieldableState;
    use postretro_entities::data_descriptors::ReloadStyle;
    use postretro_entities::{SlotOwnership, SlotRecord, SlotSchema};
    use postretro_foundation::Seat;

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
                per_owner: false,
                accumulate: None,
            }),
        )
    }

    /// A table with two replicated mod slots under one namespace plus the default
    /// owner-private engine player slots.
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

    const TEST_MOD_ID: &str = "test.descriptor-identity";

    fn test_replication_identity() -> ReplicatedSlotIdentity<'static> {
        replication_identity(
            TEST_MOD_ID,
            &[
                ("net.alpha", "k0000000000000000"),
                ("net.bravo", "k0000000000000001"),
                ("net.capped", "k0000000000000002"),
                ("net.objective", "k0000000000000003"),
                ("net.private", "k0000000000000004"),
                ("netFixture.objectiveProgress", "k0000000000000005"),
                ("extra.extra", "k0000000000000006"),
                ("currency.xp", "k0000000000000007"),
                ("currency.killStreak", "k0000000000000008"),
            ],
        )
    }

    fn replication_identity(
        mod_id: &str,
        entries: &[(&str, &str)],
    ) -> ReplicatedSlotIdentity<'static> {
        let committed_store_slots = entries
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect();
        replication_identity_with_membership(mod_id, entries, committed_store_slots)
    }

    fn replication_identity_with_membership(
        mod_id: &str,
        entries: &[(&str, &str)],
        committed_store_slots: BTreeSet<String>,
    ) -> ReplicatedSlotIdentity<'static> {
        let mut slots = std::collections::BTreeMap::new();
        for (name, durable_key) in entries {
            slots.insert((*name).to_string(), (*durable_key).to_string());
        }
        ReplicatedSlotIdentity::new(
            Some(mod_id.to_string()),
            Some(StoreIdentityLedger { version: 1, slots }),
            committed_store_slots,
        )
    }

    fn build_test_schema(slot_table: &SlotTable) -> ReplicatedSlotSchema {
        let replication_identity = test_replication_identity();
        ReplicatedSlotSchema::build(slot_table, &replication_identity)
    }

    // Regression: frame replication cloned the full committed ledger and membership
    // map before it knew whether a snapshot would be sent or applied.
    #[test]
    fn borrowed_replication_identity_keeps_runtime_snapshots_borrowed() {
        let mod_id = String::from(TEST_MOD_ID);
        let ledger = StoreIdentityLedger {
            version: 1,
            slots: [("net.alpha".to_string(), "k0000000000000000".to_string())]
                .into_iter()
                .collect(),
        };
        let committed = ["net.alpha".to_string()].into_iter().collect();

        let identity =
            ReplicatedSlotIdentity::borrowed(Some(mod_id.as_str()), Some(&ledger), &committed);

        assert!(matches!(&identity.mod_id, Some(Cow::Borrowed(_))));
        assert!(matches!(&identity.ledger, Some(Cow::Borrowed(_))));
        assert!(matches!(&identity.committed_store_slots, Cow::Borrowed(_)));
        assert_eq!(identity.durable_key("net.alpha"), Some("k0000000000000000"));
    }

    #[test]
    fn build_includes_only_replicated_slots_sorted_by_name() {
        let table = table_with_replicated();
        let schema = build_test_schema(&table);
        let names: Vec<&str> = schema.entries().iter().map(|e| e.name.as_str()).collect();
        // Engine-catalog identities sort alongside mod-qualified durable identities.
        assert_eq!(
            names,
            vec![
                "player.ammo",
                "player.ammoReserve",
                "player.health",
                "player.maxHealth",
                "player.reloadActive",
                "player.reloadProgress",
                "player.weaponCooldownMs",
                "net.alpha",
                "net.bravo",
            ]
        );
        assert_eq!(schema.entries()[0].slot_id, StateSlotId(0));
        assert_eq!(schema.entries()[1].slot_id, StateSlotId(1));
    }

    #[test]
    fn default_table_has_player_owner_private_slots() {
        // The default table's schema is exactly the owner-private engine player
        // facts.
        let table = SlotTable::new();
        let schema = build_test_schema(&table);
        let names: Vec<&str> = schema.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "player.ammo",
                "player.ammoReserve",
                "player.health",
                "player.maxHealth",
                "player.reloadActive",
                "player.reloadProgress",
                "player.weaponCooldownMs"
            ]
        );
        assert!(!schema.to_net_schema().is_empty());
    }

    #[test]
    fn fingerprint_is_deterministic_and_order_independent() {
        // Two tables that declare the same replicated slots in different insertion
        // order must produce the same fingerprint (the builder sorts by name).
        let table_a = table_with_replicated();
        let schema_a = build_test_schema(&table_a);

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
        let schema_b = build_test_schema(&table_b);

        assert_eq!(schema_a.fingerprint(), schema_b.fingerprint());
    }

    #[test]
    fn fingerprint_changes_with_scope() {
        let table_a = table_with_replicated();
        let schema_a = build_test_schema(&table_a);

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
        let schema_b = build_test_schema(&table_b);

        assert_ne!(schema_a.fingerprint(), schema_b.fingerprint());
    }

    #[test]
    fn net_schema_carries_fingerprint_and_descriptors() {
        let table = table_with_replicated();
        let schema = build_test_schema(&table);
        let net = schema.to_net_schema();
        assert_eq!(net.fingerprint(), schema.fingerprint());
        // Two mod slots plus the owner-private engine player slots.
        assert_eq!(net.len(), 9);
        let alpha = net
            .descriptor(schema.id_for("net.alpha").expect("alpha descriptor exists"))
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
                        per_owner: false,
                        accumulate: None,
                    }),
                )],
            )
            .unwrap();
        let schema = build_test_schema(&table);
        let net = schema.to_net_schema();
        let range = net
            .descriptor(
                schema
                    .id_for("net.capped")
                    .expect("capped descriptor exists"),
            )
            .and_then(|d| d.range)
            .expect("range lowered");
        assert!(range.min_finite);
        assert!(!range.max_finite, "inf max lowers as non-finite");
    }

    fn one_mod_slot_table(namespace: &str, slot: &str, scope: ReplicationScope) -> SlotTable {
        let mut table = SlotTable::new();
        table
            .insert_namespace(
                namespace,
                vec![replicated_number(&format!("{namespace}.{slot}"), scope)],
            )
            .unwrap();
        table
    }

    #[test]
    fn fingerprint_is_stable_across_authored_rename_when_durable_key_is_preserved() {
        let before = one_mod_slot_table("story", "oldName", ReplicationScope::SharedGlobal);
        let after = one_mod_slot_table("story", "newName", ReplicationScope::SharedGlobal);
        let before_identity =
            replication_identity("test.rename", &[("story.oldName", "k0123456789abcdef")]);
        let after_identity =
            replication_identity("test.rename", &[("story.newName", "k0123456789abcdef")]);

        let before_schema = ReplicatedSlotSchema::build(&before, &before_identity);
        let after_schema = ReplicatedSlotSchema::build(&after, &after_identity);

        assert_eq!(before_schema.fingerprint(), after_schema.fingerprint());
        assert_eq!(
            before_schema
                .entries()
                .iter()
                .find(|entry| entry.name == "story.oldName")
                .expect("old authored slot is present")
                .identity,
            "test.rename:k0123456789abcdef"
        );
        assert_eq!(
            after_schema
                .entries()
                .iter()
                .find(|entry| entry.name == "story.newName")
                .expect("renamed authored slot is present")
                .identity,
            "test.rename:k0123456789abcdef"
        );
    }

    #[test]
    fn fingerprint_changes_when_a_durable_key_changes_under_the_same_authored_name() {
        let table = one_mod_slot_table("story", "objective", ReplicationScope::SharedGlobal);
        let first =
            replication_identity("test.rename", &[("story.objective", "k0123456789abcdef")]);
        let changed =
            replication_identity("test.rename", &[("story.objective", "kfedcba9876543210")]);

        assert_ne!(
            ReplicatedSlotSchema::build(&table, &first).fingerprint(),
            ReplicatedSlotSchema::build(&table, &changed).fingerprint()
        );
    }

    // Regression: a hot-reloaded host retained a removed live slot and built a
    // different schema than a fresh client running the same current content.
    #[test]
    fn hot_reload_host_and_fresh_client_match_when_ledger_retains_removed_slot() {
        let mut host_table = one_mod_slot_table("old", "objective", ReplicationScope::SharedGlobal);
        host_table
            .insert_namespace(
                "current",
                vec![replicated_number(
                    "current.objective",
                    ReplicationScope::SharedGlobal,
                )],
            )
            .unwrap();
        let fresh_client_table =
            one_mod_slot_table("current", "objective", ReplicationScope::SharedGlobal);
        let identity = replication_identity_with_membership(
            "test.reload",
            &[
                ("old.objective", "k0123456789abcdef"),
                ("current.objective", "kfedcba9876543210"),
            ],
            BTreeSet::from(["current.objective".to_string()]),
        );

        let host_schema = ReplicatedSlotSchema::build(&host_table, &identity);
        let client_schema = ReplicatedSlotSchema::build(&fresh_client_table, &identity);

        assert_eq!(host_schema.fingerprint(), client_schema.fingerprint());
        assert_eq!(host_schema.entries(), client_schema.entries());
        assert!(
            host_schema
                .entries()
                .iter()
                .all(|entry| entry.name != "old.objective")
        );
    }

    #[test]
    fn engine_catalog_slots_keep_dotted_schema_identity_without_a_ledger() {
        let table = SlotTable::new();
        let schema = ReplicatedSlotSchema::build(&table, &ReplicatedSlotIdentity::default());
        let health = schema
            .entries()
            .iter()
            .find(|entry| entry.name == "player.health")
            .expect("engine catalog health slot remains replicated");

        assert_eq!(health.identity, "player.health");
    }

    #[test]
    fn unkeyed_mod_slot_is_excluded_once_per_schema_build_and_changes_fingerprint() {
        let table = one_mod_slot_table("story", "objective", ReplicationScope::SharedGlobal);
        let keyed =
            replication_identity("test.unkeyed", &[("story.objective", "k0123456789abcdef")]);
        let keyed_schema = ReplicatedSlotSchema::build(&table, &keyed);
        let unkeyed_identity = replication_identity_with_membership(
            "test.unkeyed",
            &[("story.other", "k0123456789abcdef")],
            BTreeSet::from(["story.objective".to_string()]),
        );

        let logs = crate::scripting::reactions::log_capture::capture(|| {
            let unkeyed_schema = ReplicatedSlotSchema::build(&table, &unkeyed_identity);
            assert!(
                unkeyed_schema
                    .entries()
                    .iter()
                    .all(|entry| entry.name != "story.objective"),
                "a live mod slot with no snapshot key must never fall back to its authored name"
            );
            assert_ne!(unkeyed_schema.fingerprint(), keyed_schema.fingerprint());
        });
        assert_eq!(
            logs.iter()
                .filter(|(level, message)| {
                    *level == log::Level::Warn
                        && message.contains("story.objective")
                        && message.contains("no durable identity ledger entry")
                })
                .count(),
            1,
            "one rebuild emits one stable warning for the unkeyed slot"
        );
    }

    #[test]
    fn wire_shape_uses_the_authored_name_not_the_replication_identity() {
        let mut cooldown = SlotTable::new();
        for name in [
            "player.ammo",
            "player.ammoReserve",
            "player.health",
            "player.maxHealth",
            "player.reloadActive",
            "player.reloadProgress",
            WEAPON_COOLDOWN_SLOT,
        ] {
            cooldown.get_mut(name).unwrap().schema.network = ReplicationScope::None;
        }
        let cooldown_record = cooldown.get_mut(WEAPON_COOLDOWN_SLOT).unwrap();
        cooldown_record.schema.ownership = SlotOwnership::Mod;
        cooldown_record.schema.network = ReplicationScope::OwnerPrivatePlayer;

        let mut renamed = one_mod_slot_table(
            "story",
            "renamedCooldown",
            ReplicationScope::OwnerPrivatePlayer,
        );
        for name in [
            "player.ammo",
            "player.ammoReserve",
            "player.health",
            "player.maxHealth",
            "player.reloadActive",
            "player.reloadProgress",
            WEAPON_COOLDOWN_SLOT,
        ] {
            renamed.get_mut(name).unwrap().schema.network = ReplicationScope::None;
        }
        let cooldown_identity = replication_identity(
            "test.wire-shape",
            &[(WEAPON_COOLDOWN_SLOT, "k0123456789abcdef")],
        );
        let renamed_identity = replication_identity(
            "test.wire-shape",
            &[("story.renamedCooldown", "k0123456789abcdef")],
        );
        let cooldown_schema = ReplicatedSlotSchema::build(&cooldown, &cooldown_identity);
        let renamed_schema = ReplicatedSlotSchema::build(&renamed, &renamed_identity);
        let cooldown_entry = cooldown_schema
            .entries()
            .iter()
            .find(|entry| entry.name == WEAPON_COOLDOWN_SLOT)
            .unwrap();
        let renamed_entry = renamed_schema
            .entries()
            .iter()
            .find(|entry| entry.name == "story.renamedCooldown")
            .unwrap();

        assert_eq!(cooldown_entry.identity, renamed_entry.identity);
        assert_eq!(
            cooldown_entry.wire_shape,
            ReplicatedWireShape::for_name(cooldown_entry.name.as_str())
        );
        assert_eq!(
            renamed_entry.wire_shape,
            ReplicatedWireShape::for_name(renamed_entry.name.as_str())
        );
        assert_ne!(cooldown_entry.wire_shape, renamed_entry.wire_shape);
        assert_ne!(cooldown_schema.fingerprint(), renamed_schema.fingerprint());
    }

    // -----------------------------------------------------------------------
    // Task 3: engine production + client apply glue
    // -----------------------------------------------------------------------

    use postretro_entities::Transform;
    use postretro_foundation::HealthDescriptor;

    const CLIENT_A: u64 = 1;
    const CLIENT_B: u64 = 2;
    const CLIENT_C: u64 = 3;

    /// A host slot table with one `SharedGlobal` (`net.objective`) and one
    /// `OwnerPrivatePlayer` (`net.private`) mod number slot. Both peers build this
    /// identically, so their schema fingerprints match. Built-in owner-private player
    /// slots are disabled so these round-trip tests cover exactly the two mod slots.
    fn shared_and_private_table() -> SlotTable {
        let mut table = SlotTable::new();
        for name in [
            "player.ammo",
            "player.ammoReserve",
            "player.health",
            "player.maxHealth",
            "player.reloadActive",
            "player.reloadProgress",
            "player.weaponCooldownMs",
        ] {
            table.get_mut(name).unwrap().schema.network = ReplicationScope::None;
        }
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

    fn per_owner_number(name: &str, scope: ReplicationScope) -> (String, SlotRecord) {
        let (_namespace, slot) = name.split_once('.').unwrap();
        (
            slot.to_string(),
            SlotRecord::new(SlotSchema {
                slot_type: SlotType::Number,
                default: Some(SlotValue::Number(5.0)),
                range: None,
                persist: false,
                readonly: false,
                ownership: SlotOwnership::Mod,
                network: scope,
                per_owner: true,
                accumulate: None,
            }),
        )
    }

    /// A per-owner currency fixture with one owner-private replicated slot and
    /// one host-only per-owner slot. Disable built-in owner-private sources so
    /// the behavior harness observes exactly these mod-owned records.
    fn per_owner_currency_table() -> SlotTable {
        let mut table = SlotTable::new();
        for name in [
            "player.ammo",
            "player.ammoReserve",
            "player.health",
            "player.maxHealth",
            "player.reloadActive",
            "player.reloadProgress",
            "player.weaponCooldownMs",
        ] {
            table.get_mut(name).unwrap().schema.network = ReplicationScope::None;
        }
        table
            .insert_namespace(
                "currency",
                vec![
                    per_owner_number("currency.xp", ReplicationScope::OwnerPrivatePlayer),
                    per_owner_number("currency.killStreak", ReplicationScope::None),
                ],
            )
            .unwrap();
        table
    }

    fn add_owned_pawn(
        registry: &mut EntityRegistry,
        owners: &mut MovementOwners,
        client_id: u64,
        seat: Seat,
    ) -> EntityId {
        let pawn = registry.spawn(Transform::default());
        registry.bind_pawn_seat(pawn, seat);
        owners.set(pawn, client_id);
        pawn
    }

    fn record_for_slot(
        records: &[RawStateSlotRecord],
        slot_id: StateSlotId,
    ) -> &RawStateSlotRecord {
        records
            .iter()
            .find(|record| record.slot_id == slot_id.0)
            .expect("owner-private slot record exists")
    }

    /// A slot table whose player owner-private slots are replicated. The catalog
    /// sets this scope, so a plain `SlotTable::new()` carries it; both peers build
    /// this identically.
    fn owner_private_player_table() -> SlotTable {
        let table = SlotTable::new();
        debug_assert_eq!(
            table.get("player.health").unwrap().schema.network,
            ReplicationScope::OwnerPrivatePlayer,
            "player.health is owner-private by default"
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

    fn registry_with_owned_weapon_cooldown(
        client_id: u64,
        cooldown_remaining_ms: f32,
    ) -> (
        EntityRegistry,
        MovementOwners,
        WeaponOwners,
        EntityId,
        EntityId,
    ) {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        registry
            .set_component(
                weapon,
                WeaponComponent {
                    damage: 10.0,
                    pellet_count: 1,
                    spread_degrees: 0.0,
                    range: 100.0,
                    cooldown_ms: 250.0,
                    lower_ms: 0,
                    raise_ms: 0,
                    block_during_reload: None,
                    fire_mode: postretro_entities::data_descriptors::FireMode::Semi,
                    resolution: postretro_entities::data_descriptors::ResolutionMode::Hitscan,
                    projectile: None,
                    muzzle_offset: None,
                    cooldown_remaining_ms,
                    shoot_press_consumed: false,
                    reload_press_consumed: false,
                    credit_source: "weapon.test".to_string(),
                    ammo: None,
                    magazine: 0,
                    state: WieldableState::Idle,
                    state_remaining_ms: 0,
                    state_total_ms: 0,
                    state_elapsed_sub_ms: 0.0,
                    reload_credited: 0,
                    shells_fired: 0,
                    reload_feedback: Default::default(),
                },
            )
            .unwrap();
        let mut owners = MovementOwners::new();
        owners.set(pawn, client_id);
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(weapon);
        registry.set_component(pawn, inventory).unwrap();
        let weapon_owners = WeaponOwners::new();
        (registry, owners, weapon_owners, pawn, weapon)
    }

    struct OwnedAmmoPawnSpec<'a> {
        client: u64,
        ammo_type: &'a str,
        magazine: u32,
        reserve: Option<u32>,
        state_remaining_ms: u32,
        state_total_ms: u32,
    }

    fn add_owned_ammo_pawn(
        registry: &mut EntityRegistry,
        owners: &mut MovementOwners,
        _weapon_owners: &mut WeaponOwners,
        spec: OwnedAmmoPawnSpec<'_>,
    ) -> (EntityId, EntityId) {
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        registry
            .set_component(
                weapon,
                WeaponComponent {
                    damage: 10.0,
                    pellet_count: 1,
                    spread_degrees: 0.0,
                    range: 100.0,
                    cooldown_ms: 250.0,
                    lower_ms: 0,
                    raise_ms: 0,
                    block_during_reload: None,
                    fire_mode: postretro_entities::data_descriptors::FireMode::Semi,
                    resolution: postretro_entities::data_descriptors::ResolutionMode::Hitscan,
                    projectile: None,
                    muzzle_offset: None,
                    cooldown_remaining_ms: 0.0,
                    shoot_press_consumed: false,
                    reload_press_consumed: false,
                    credit_source: "weapon.test.ammo".to_string(),
                    ammo: Some(WeaponAmmoTuning {
                        ammo_type: spec.ammo_type.to_string(),
                        capacity: 12,
                        cost_per_shot: 1,
                        reload_ms: 500,
                        reload_style: ReloadStyle::Magazine,
                    }),
                    magazine: spec.magazine,
                    state: if spec.state_remaining_ms > 0 {
                        WieldableState::Reloading
                    } else {
                        WieldableState::Idle
                    },
                    state_remaining_ms: spec.state_remaining_ms,
                    state_total_ms: spec.state_total_ms,
                    state_elapsed_sub_ms: 0.0,
                    reload_credited: 0,
                    shells_fired: 0,
                    reload_feedback: Default::default(),
                },
            )
            .unwrap();
        if let Some(amount) = spec.reserve {
            let mut ammo_reserve = AmmoReserve::new();
            ammo_reserve.credit(spec.ammo_type, amount);
            registry.set_component(pawn, ammo_reserve).unwrap();
        }
        owners.set(pawn, spec.client);
        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(weapon);
        registry.set_component(pawn, inventory).unwrap();
        (pawn, weapon)
    }

    #[test]
    fn owner_private_ammo_reload_projection_is_per_pawn_and_typed() {
        let mut host_table = owner_private_player_table();
        // Host/global values must never leak into a valid remote pawn projection.
        host_table.get_mut("player.ammo").unwrap().value = Some(SlotValue::Number(999.0));
        host_table.get_mut("player.ammoReserve").unwrap().value = Some(SlotValue::Number(999.0));
        host_table.get_mut("player.reloadProgress").unwrap().value = Some(SlotValue::Number(0.9));
        host_table.get_mut("player.reloadActive").unwrap().value = Some(SlotValue::Boolean(false));

        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::new();
        let mut weapon_owners = WeaponOwners::new();
        let (pawn_idle, weapon_idle) = add_owned_ammo_pawn(
            &mut registry,
            &mut owners,
            &mut weapon_owners,
            OwnedAmmoPawnSpec {
                client: CLIENT_A,
                ammo_type: "cells",
                magazine: 3,
                reserve: Some(11),
                state_remaining_ms: 250,
                state_total_ms: 500,
            },
        );
        add_owned_ammo_pawn(
            &mut registry,
            &mut owners,
            &mut weapon_owners,
            OwnedAmmoPawnSpec {
                client: CLIENT_B,
                ammo_type: "shells",
                magazine: 8,
                reserve: None,
                state_remaining_ms: 10,
                state_total_ms: 0,
            },
        );

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        host.register_client(CLIENT_B);
        let fingerprint = host.fingerprint(&host_table, &test_replication_identity());
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &weapon_owners,
        );
        let records_a = host.produce_for_client(CLIENT_A, 0).unwrap();
        let records_b = host.produce_for_client(CLIENT_B, 0).unwrap();

        let mut table_a = owner_private_player_table();
        let mut table_b = owner_private_player_table();
        ClientStateApply::new().apply_snapshot_state(
            &mut table_a,
            &test_replication_identity(),
            0,
            &fingerprint,
            &records_a,
        );
        ClientStateApply::new().apply_snapshot_state(
            &mut table_b,
            &test_replication_identity(),
            0,
            &fingerprint,
            &records_b,
        );

        for (table, magazine, reserve, progress, active) in [
            (&table_a, 3.0, 11.0, 0.5, true),
            (&table_b, 8.0, 0.0, 0.0, true),
        ] {
            assert_eq!(
                table.get("player.ammo").unwrap().value,
                Some(SlotValue::Number(magazine))
            );
            assert_eq!(
                table.get("player.ammoReserve").unwrap().value,
                Some(SlotValue::Number(reserve))
            );
            assert_eq!(
                table.get("player.reloadProgress").unwrap().value,
                Some(SlotValue::Number(progress))
            );
            assert_eq!(
                table.get("player.reloadActive").unwrap().value,
                Some(SlotValue::Boolean(active)),
                "reloadActive remains Boolean on the wire/apply path"
            );
        }

        let mut idle_weapon = registry
            .get_component::<WeaponComponent>(weapon_idle)
            .unwrap()
            .clone();
        idle_weapon.state_remaining_ms = 0;
        idle_weapon.state_total_ms = 500;
        idle_weapon.state = WieldableState::Idle;
        registry.set_component(weapon_idle, idle_weapon).unwrap();
        assert_eq!(
            descriptor_ammo_for_pawn(
                &registry,
                "player.reloadProgress",
                pawn_idle,
                &weapon_owners
            ),
            Some(Some(SlotValue::Number(0.0)))
        );
        assert_eq!(
            descriptor_ammo_for_pawn(&registry, "player.reloadActive", pawn_idle, &weapon_owners),
            Some(Some(SlotValue::Boolean(false)))
        );
    }

    #[test]
    fn owner_private_reload_projection_publishes_catch_up_endpoints_in_order() {
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::new();
        let mut weapon_owners = WeaponOwners::new();
        let (pawn, weapon_id) = add_owned_ammo_pawn(
            &mut registry,
            &mut owners,
            &mut weapon_owners,
            OwnedAmmoPawnSpec {
                client: CLIENT_A,
                ammo_type: "cells",
                magazine: 3,
                reserve: Some(11),
                state_remaining_ms: 250,
                state_total_ms: 500,
            },
        );

        let mut weapon = registry
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        weapon.state = WieldableState::ShellLoading;
        let start_tick = weapon.begin_reload_feedback_tick();
        weapon.publish_reload_feedback(ReloadFeedback::Started, start_tick);
        let boundary_tick = weapon.begin_reload_feedback_tick();
        weapon.publish_reload_feedback(ReloadFeedback::Completed, boundary_tick);
        weapon.publish_reload_feedback(ReloadFeedback::Completed, boundary_tick);
        registry.set_component(weapon_id, weapon).unwrap();

        // Regression: a catch-up frame completed a short reload before the
        // owner-private projection sampled its Started endpoint.
        assert_eq!(
            descriptor_ammo_for_pawn(&registry, "player.reloadProgress", pawn, &weapon_owners),
            Some(Some(SlotValue::Number(0.0)))
        );
        assert_eq!(
            descriptor_ammo_for_pawn(&registry, "player.reloadActive", pawn, &weapon_owners),
            Some(Some(SlotValue::Boolean(true)))
        );

        // Each cadence-gated acknowledgement advances only the sampled owner.
        crate::sim::clear_owner_reload_feedback_for_weapons(&mut registry, &[weapon_id]);
        assert_eq!(
            descriptor_ammo_for_pawn(&registry, "player.reloadProgress", pawn, &weapon_owners),
            Some(Some(SlotValue::Number(1.0)))
        );
        let observation = registry
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .reload_feedback_sample(
                postretro_entities::components::weapon::ReloadFeedbackConsumer::OwnerProjection,
            )
            .endpoint
            .expect("coalesced boundary remains observable");
        assert_eq!(observation.occurrences, 2);
        assert!(observation.coalesced);
        crate::sim::clear_owner_reload_feedback_for_weapons(&mut registry, &[weapon_id]);
        assert_eq!(
            descriptor_ammo_for_pawn(&registry, "player.reloadProgress", pawn, &weapon_owners),
            Some(Some(SlotValue::Number(0.5)))
        );
        assert_eq!(
            descriptor_ammo_for_pawn(&registry, "player.reloadActive", pawn, &weapon_owners),
            Some(Some(SlotValue::Boolean(true)))
        );

        let mut weapon = registry
            .get_component::<WeaponComponent>(weapon_id)
            .unwrap()
            .clone();
        weapon.ammo = None;
        weapon.reload_feedback = Default::default();
        weapon.state = WieldableState::Reloading;
        weapon.state_remaining_ms = 250;
        registry.set_component(weapon_id, weapon).unwrap();
        assert_eq!(
            descriptor_ammo_for_pawn(&registry, "player.ammo", pawn, &weapon_owners),
            Some(None)
        );
        assert_eq!(
            descriptor_ammo_for_pawn(&registry, "player.reloadProgress", pawn, &weapon_owners),
            Some(Some(SlotValue::Number(0.5)))
        );
        assert_eq!(
            descriptor_ammo_for_pawn(&registry, "player.reloadActive", pawn, &weapon_owners),
            Some(Some(SlotValue::Boolean(true)))
        );
    }

    // Regression: cadence alone advanced owner feedback while pawn-to-weapon
    // projection had no live mapping.
    #[test]
    fn owner_feedback_advances_only_after_a_mapped_weapon_is_projected() {
        let table = owner_private_player_table();
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::new();
        let mut weapon_owners = WeaponOwners::new();
        let (pawn, weapon) = add_owned_ammo_pawn(
            &mut registry,
            &mut owners,
            &mut weapon_owners,
            OwnedAmmoPawnSpec {
                client: CLIENT_A,
                ammo_type: "cells",
                magazine: 3,
                reserve: Some(11),
                state_remaining_ms: 250,
                state_total_ms: 500,
            },
        );
        let mut component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        let tick = component.begin_reload_feedback_tick();
        component.publish_reload_feedback(ReloadFeedback::Started, tick);
        registry.set_component(weapon, component).unwrap();

        registry.set_component(pawn, Inventory::default()).unwrap();
        let mut host = HostStateReplication::new();
        let sampled = host.ingest_frame_and_collect_sampled_weapons(
            &table,
            &test_replication_identity(),
            &registry,
            &owners,
            &weapon_owners,
        );
        assert!(sampled.is_empty());
        crate::sim::clear_owner_reload_feedback_for_weapons(&mut registry, &sampled);

        let mut inventory = Inventory::default();
        inventory.wieldables[0] = Some(weapon);
        registry.set_component(pawn, inventory).unwrap();
        assert_eq!(
            descriptor_ammo_for_pawn(&registry, "player.reloadProgress", pawn, &weapon_owners),
            Some(Some(SlotValue::Number(0.0)))
        );
        let sampled = host.ingest_frame_and_collect_sampled_weapons(
            &table,
            &test_replication_identity(),
            &registry,
            &owners,
            &weapon_owners,
        );
        assert_eq!(sampled, vec![weapon]);
    }

    #[test]
    fn missing_ammo_source_does_not_fall_back_to_global_values() {
        let registry = EntityRegistry::new();
        let pawn = EntityId::from_raw(0);
        let owners = WeaponOwners::new();
        assert_eq!(
            descriptor_ammo_for_pawn(&registry, "player.ammo", pawn, &owners),
            Some(None)
        );
        assert_eq!(
            descriptor_ammo_for_pawn(&registry, "player.ammoReserve", pawn, &owners),
            Some(None)
        );
        assert_eq!(
            descriptor_ammo_for_pawn(&registry, "player.reloadProgress", pawn, &owners),
            Some(Some(SlotValue::Number(0.0)))
        );
        assert_eq!(
            descriptor_ammo_for_pawn(&registry, "player.reloadActive", pawn, &owners),
            Some(Some(SlotValue::Boolean(false)))
        );
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
        let fingerprint = host.fingerprint(&host_table, &test_replication_identity());
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let records = host
            .produce_for_client(CLIENT_A, 0)
            .expect("registered client produces records");
        assert_eq!(records.len(), 2, "shared + owner-private both produced");

        // Client side: a fresh table (no values) and the apply glue.
        let mut client_table = shared_and_private_table();
        let mut client = ClientStateApply::new();
        let outcome = client.apply_snapshot_state(
            &mut client_table,
            &test_replication_identity(),
            0,
            &fingerprint,
            &records,
        );
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

    // Regression: a delayed pre-rebuild ack aliased the rebuilt tracker's recycled
    // baseline id and suppressed that slot forever.
    #[test]
    fn schema_rebuild_retires_old_acks_and_sends_fresh_participant_baselines() {
        let mut host_table = shared_and_private_table();
        host_table.get_mut("net.objective").unwrap().value = Some(SlotValue::Number(3.0));
        host_table.get_mut("net.private").unwrap().value = Some(SlotValue::Number(42.0));
        let (registry, owners, _pawn) = registry_with_owned_health(CLIENT_A, 0.0, 0.0);

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        let _ = host.fingerprint(&host_table, &test_replication_identity());
        let objective_id = host
            .schema(&host_table, &test_replication_identity())
            .id_for("net.objective")
            .expect("objective is replicated");
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let before = host
            .produce_for_client(CLIENT_A, 0)
            .expect("participant produces before the staged rebuild");
        let old_baseline = before
            .iter()
            .find(|record| record.slot_id == objective_id.0)
            .expect("objective baseline before rebuild")
            .baseline_id;

        host.reset_schema_for_clients([CLIENT_A]);
        let _ = host.fingerprint(&host_table, &test_replication_identity());
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );

        // This reliable Input ack was queued before the staged manifest committed,
        // but reaches the unchanged participation epoch after the host rebuilt.
        host.apply_ack(CLIENT_A, 0, &[(objective_id.0, old_baseline)]);
        let rebuilt = host
            .produce_for_client(CLIENT_A, 1)
            .expect("participant remains registered after schema rebuild");
        let objective = rebuilt
            .iter()
            .find(|record| record.slot_id == objective_id.0)
            .expect("delayed old ack cannot suppress rebuilt objective baseline");
        assert_eq!(
            objective.kind,
            postretro_net::state_slots::STATE_RECORD_KIND_FULL_BASELINE,
            "retired pre-rebuild ack leaves the participant unbaselined"
        );
        assert_ne!(
            objective.baseline_id, old_baseline,
            "schema rebuild never recycles a server-lifetime baseline id"
        );
    }

    #[test]
    fn schema_rebuild_after_added_slot_reactivates_prior_baselines() {
        let mut host_table = shared_and_private_table();
        host_table.get_mut("net.objective").unwrap().value = Some(SlotValue::Number(3.0));
        let registry = EntityRegistry::new();
        let owners = MovementOwners::new();
        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);

        let before_fingerprint = host.fingerprint(&host_table, &test_replication_identity());
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let initial = host
            .produce_for_client(CLIENT_A, 0)
            .expect("initial schema produces a baseline");
        let objective_before = initial
            .iter()
            .find(|record| {
                record.slot_id
                    == host
                        .schema(&host_table, &test_replication_identity())
                        .id_for("net.objective")
                        .unwrap()
                        .0
            })
            .expect("objective baseline exists")
            .baseline_id;

        host_table
            .insert_namespace(
                "extra",
                vec![replicated_number(
                    "net.extra",
                    ReplicationScope::SharedGlobal,
                )],
            )
            .expect("added declaration is non-overlapping");
        host_table.get_mut("extra.extra").unwrap().value = Some(SlotValue::Number(7.0));
        host.reset_schema_for_clients([CLIENT_A]);
        let after_fingerprint = host.fingerprint(&host_table, &test_replication_identity());
        assert_ne!(before_fingerprint, after_fingerprint);

        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let rebuilt = host
            .produce_for_client(CLIENT_A, 1)
            .expect("rebuild reactivates the participant");
        let objective_id = host
            .schema(&host_table, &test_replication_identity())
            .id_for("net.objective")
            .unwrap()
            .0;
        let extra_id = host
            .schema(&host_table, &test_replication_identity())
            .id_for("extra.extra")
            .unwrap()
            .0;
        let objective_after = rebuilt
            .iter()
            .find(|record| record.slot_id == objective_id)
            .expect("prior objective receives a fresh baseline");
        assert_eq!(
            objective_after.kind,
            postretro_net::state_slots::STATE_RECORD_KIND_FULL_BASELINE
        );
        assert_ne!(objective_after.baseline_id, objective_before);
        assert!(rebuilt.iter().any(|record| {
            record.slot_id == extra_id
                && record.kind == postretro_net::state_slots::STATE_RECORD_KIND_FULL_BASELINE
        }));
    }

    // A descriptor-defined source value (health) projects into a named owner-private
    // slot and replicates through the SAME wire schema/apply path as store slots.
    #[test]
    fn descriptor_health_projects_and_replicates_like_a_store_slot() {
        // Default built-in membership makes player health owner-private.
        let host_table = owner_private_player_table();

        // The descriptor-fed source: an owned pawn with a live HealthComponent. No slot
        // value is ever written on the host — the value comes straight from the
        // component through the projection.
        let (registry, owners, _pawn) = registry_with_owned_health(CLIENT_A, 75.0, 100.0);

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        let fingerprint = host.fingerprint(&host_table, &test_replication_identity());
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let records = host
            .produce_for_client(CLIENT_A, 0)
            .expect("registered client produces records");
        assert_eq!(
            records.len(),
            4,
            "health pair plus reload defaults projected"
        );

        // Client applies through the store path; the engine-owned readonly player slots
        // receive the replicated values (engine bypass honors readonly).
        let mut client_table = owner_private_player_table();
        let mut client = ClientStateApply::new();
        let outcome = client.apply_snapshot_state(
            &mut client_table,
            &test_replication_identity(),
            0,
            &fingerprint,
            &records,
        );
        assert_eq!(outcome.slot_baselines.len(), 4);
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

    // Regression: the listen host used to derive `player.health`'s range from its
    // materialized boot pawn while a connected client suppressed that pawn. Their
    // otherwise-identical slot tables then produced different fingerprints and the
    // client dropped every replicated state record.
    #[test]
    fn descriptor_health_range_is_role_invariant_and_accepts_first_baseline() {
        use crate::scripting::map_entity::MapEntity;
        use crate::startup::lifecycle::install_descriptor_player_health_range;

        let mut descriptors =
            crate::netcode::predict_reconcile_harness_test_fixtures::entity_descriptors();
        descriptors[0].health = Some(HealthDescriptor {
            max: 137.0,
            hitbox: None,
            zone_multipliers: std::collections::HashMap::new(),
        });
        let spawn_points = [MapEntity {
            classname: "player_spawn".to_string(),
            origin: glam::Vec3::ZERO,
            angles: glam::Vec3::ZERO,
            key_values: std::collections::HashMap::new(),
            tags: Vec::new(),
        }];

        let mut host_table = owner_private_player_table();
        let mut client_table = owner_private_player_table();
        install_descriptor_player_health_range(&mut host_table, &spawn_points, &descriptors);
        install_descriptor_player_health_range(&mut client_table, &spawn_points, &descriptors);

        let host_schema = build_test_schema(&host_table);
        let client_schema = build_test_schema(&client_table);
        assert_eq!(host_schema.fingerprint(), client_schema.fingerprint());
        assert_eq!(
            host_table.get("player.health").unwrap().schema.range,
            Some(NumericRange {
                min: 0.0,
                max: 137.0,
            })
        );

        let (registry, owners, _pawn) = registry_with_owned_health(CLIENT_A, 75.0, 137.0);
        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        let fingerprint = host.fingerprint(&host_table, &test_replication_identity());
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let records = host
            .produce_for_client(CLIENT_A, 0)
            .expect("registered client produces descriptor state");

        let outcome = ClientStateApply::new().apply_snapshot_state(
            &mut client_table,
            &test_replication_identity(),
            0,
            &fingerprint,
            &records,
        );
        assert_eq!(outcome.slot_baselines.len(), records.len());
        assert_eq!(
            client_table.get("player.health").unwrap().value,
            Some(SlotValue::Number(75.0)),
            "matching descriptor-derived schemas accept the state baseline"
        );
    }

    #[test]
    fn weapon_cooldown_projects_through_owned_weapon_map() {
        let host_table = owner_private_player_table();
        let (mut registry, owners, weapon_owners, pawn, weapon) =
            registry_with_owned_weapon_cooldown(CLIENT_A, 123.0);

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        let fingerprint = host.fingerprint(&host_table, &test_replication_identity());
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &weapon_owners,
        );
        let records = host
            .produce_for_client(CLIENT_A, 0)
            .expect("registered client produces records");

        let schema = build_test_schema(&host_table);
        let cooldown_id = schema
            .id_for("player.weaponCooldownMs")
            .expect("cooldown id");
        assert!(
            records.iter().any(|record| record.slot_id == cooldown_id.0),
            "cooldown record is produced from pawn -> weapon projection"
        );

        let mut client_table = owner_private_player_table();
        let mut client = ClientStateApply::new();
        let outcome = client.apply_snapshot_state(
            &mut client_table,
            &test_replication_identity(),
            0,
            &fingerprint,
            &records,
        );
        assert!(
            outcome
                .slot_baselines
                .iter()
                .any(|(slot_id, _)| *slot_id == cooldown_id.0),
            "cooldown record is acked"
        );
        assert_eq!(
            client_table.get("player.weaponCooldownMs").unwrap().value,
            Some(SlotValue::Number(123.0)),
            "mapped sibling weapon cooldown reached the owner-private slot"
        );
        assert_eq!(outcome.fresh_weapon_cooldown_slot, Some(0));

        host.apply_ack(CLIENT_A, 0, &outcome.slot_baselines);
        let weapon_b = registry.spawn(postretro_entities::Transform::default());
        let weapon_b_component = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        registry
            .set_component(weapon_b, weapon_b_component)
            .unwrap();
        let mut inventory = registry.get_component::<Inventory>(pawn).unwrap().clone();
        inventory.wieldables[1] = Some(weapon_b);
        inventory.active_slot = 1;
        registry.set_component(pawn, inventory).unwrap();

        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &weapon_owners,
        );
        let switched_records = host.produce_for_client(CLIENT_A, 1).unwrap();
        let cooldown_record = switched_records
            .iter()
            .find(|record| record.slot_id == cooldown_id.0)
            .expect("slot identity changes the sample even when cooldown is equal");
        assert_eq!(
            cooldown_record.value,
            WireSlotValue::Array(vec![1.0, 123.0])
        );
        let switched = client.apply_snapshot_state(
            &mut client_table,
            &test_replication_identity(),
            1,
            &fingerprint,
            &switched_records,
        );
        assert_eq!(switched.fresh_weapon_cooldown_slot, Some(1));
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
        let _real_fingerprint = host.fingerprint(&host_table, &test_replication_identity());
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let records = host.produce_for_client(CLIENT_A, 0).expect("records");

        // The client holds a prior value the apply must NOT overwrite.
        let mut client_table = shared_and_private_table();
        client_table.get_mut("net.objective").unwrap().value = Some(SlotValue::Number(1.0));
        let mut client = ClientStateApply::new();

        // A WRONG fingerprint must reject the whole batch before any mutation.
        let outcome = client.apply_snapshot_state(
            &mut client_table,
            &test_replication_identity(),
            0,
            &[0xAB; 32],
            &records,
        );
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
        let fingerprint = host.fingerprint(&host_table, &test_replication_identity());

        // Hand-build a batch: a valid number record for net.objective (slot 0, sorted by
        // name: net.objective < net.private) and a TYPE-MISMATCHED boolean for the
        // number slot net.private (slot 1). The whole batch must reject.
        let schema = build_test_schema(&host_table);
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
        let outcome = client.apply_snapshot_state(
            &mut client_table,
            &test_replication_identity(),
            0,
            &fingerprint,
            &records,
        );
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
        let host_table = owner_private_player_table();

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
        let fingerprint = host.fingerprint(&host_table, &test_replication_identity());

        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let records_a = host.produce_for_client(CLIENT_A, 0).unwrap();
        let records_b = host.produce_for_client(CLIENT_B, 0).unwrap();

        // Each client's batch carries only ITS pawn's health.
        let mut table_a = owner_private_player_table();
        let mut table_b = owner_private_player_table();
        let mut client_a = ClientStateApply::new();
        let mut client_b = ClientStateApply::new();
        client_a.apply_snapshot_state(
            &mut table_a,
            &test_replication_identity(),
            0,
            &fingerprint,
            &records_a,
        );
        client_b.apply_snapshot_state(
            &mut table_b,
            &test_replication_identity(),
            0,
            &fingerprint,
            &records_b,
        );

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

    #[test]
    fn per_owner_mod_slot_isolates_owner_private_snapshots_and_skips_unbound_pawns() {
        let mut host_table = per_owner_currency_table();
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::new();
        add_owned_pawn(&mut registry, &mut owners, CLIENT_A, Seat(10));
        add_owned_pawn(&mut registry, &mut owners, CLIENT_B, Seat(11));
        let unbound_pawn = registry.spawn(Transform::default());
        owners.set(unbound_pawn, CLIENT_C);
        {
            let xp = host_table.get_mut("currency.xp").unwrap();
            xp.set_per_seat_value(Seat(10), SlotValue::Number(17.0));
            xp.set_per_seat_value(Seat(11), SlotValue::Number(31.0));
            // A poisoned scalar catches accidental global fallback for owner slots.
            xp.write_value(Some(SlotValue::Number(99.0)));
        }

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        host.register_client(CLIENT_B);
        host.register_client(CLIENT_C);
        let fingerprint = host.fingerprint(&host_table, &test_replication_identity());
        let xp_id = host
            .schema(&host_table, &test_replication_identity())
            .id_for("currency.xp")
            .unwrap();
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let records_a = host.produce_for_client(CLIENT_A, 0).unwrap();
        let records_b = host.produce_for_client(CLIENT_B, 0).unwrap();
        let records_c = host.produce_for_client(CLIENT_C, 0).unwrap();

        assert_eq!(
            record_for_slot(&records_a, xp_id).value,
            WireSlotValue::Number(17.0)
        );
        assert_eq!(
            record_for_slot(&records_b, xp_id).value,
            WireSlotValue::Number(31.0)
        );
        assert!(
            records_c.is_empty(),
            "a pawn with no seat skips its per-owner source instead of falling through to 99"
        );

        let mut table_a = per_owner_currency_table();
        let mut table_b = per_owner_currency_table();
        ClientStateApply::new().apply_snapshot_state(
            &mut table_a,
            &test_replication_identity(),
            0,
            &fingerprint,
            &records_a,
        );
        ClientStateApply::new().apply_snapshot_state(
            &mut table_b,
            &test_replication_identity(),
            0,
            &fingerprint,
            &records_b,
        );
        assert_eq!(
            table_a.get("currency.xp").unwrap().value,
            Some(SlotValue::Number(17.0)),
            "client A receives only its seat's value"
        );
        assert_eq!(
            table_b.get("currency.xp").unwrap().value,
            Some(SlotValue::Number(31.0)),
            "client B receives only its seat's value"
        );
    }

    #[test]
    fn per_owner_late_join_baseline_uses_that_seats_default_or_current_value() {
        let mut host_table = per_owner_currency_table();
        host_table
            .get_mut("currency.xp")
            .unwrap()
            .set_per_seat_value(Seat(10), SlotValue::Number(71.0));
        host_table
            .get_mut("currency.xp")
            .unwrap()
            .write_value(Some(SlotValue::Number(99.0)));
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::new();
        add_owned_pawn(&mut registry, &mut owners, CLIENT_A, Seat(10));

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        let _ = host.fingerprint(&host_table, &test_replication_identity());
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );

        // Admission mints and binds the seat before the first tracker ingest.
        add_owned_pawn(&mut registry, &mut owners, CLIENT_B, Seat(11));
        host.register_client(CLIENT_B);
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let xp_id = host
            .schema(&host_table, &test_replication_identity())
            .id_for("currency.xp")
            .unwrap();
        let default_records = host.produce_for_client(CLIENT_B, 1).unwrap();
        let default_record = record_for_slot(&default_records, xp_id);
        assert_eq!(
            default_record.value,
            WireSlotValue::Number(5.0),
            "an unwritten late-join seat receives the declaration default, never A's 71"
        );
        assert_eq!(
            default_record.kind,
            postretro_net::state_slots::STATE_RECORD_KIND_FULL_BASELINE
        );

        host_table
            .get_mut("currency.xp")
            .unwrap()
            .set_per_seat_value(Seat(12), SlotValue::Number(43.0));
        add_owned_pawn(&mut registry, &mut owners, CLIENT_C, Seat(12));
        host.register_client(CLIENT_C);
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let current_records = host.produce_for_client(CLIENT_C, 2).unwrap();
        let current_record = record_for_slot(&current_records, xp_id);
        assert_eq!(
            current_record.value,
            WireSlotValue::Number(43.0),
            "a late joiner with a current seat value receives that value, never another owner's"
        );
        assert_eq!(
            current_record.kind,
            postretro_net::state_slots::STATE_RECORD_KIND_FULL_BASELINE
        );
    }

    #[test]
    fn per_owner_reclaim_reseeds_a_full_baseline_from_the_seat_store() {
        let mut host_table = per_owner_currency_table();
        {
            let xp = host_table.get_mut("currency.xp").unwrap();
            xp.set_per_seat_value(Seat(10), SlotValue::Number(47.0));
            xp.write_value(Some(SlotValue::Number(99.0)));
        }
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::new();
        let departing_pawn = add_owned_pawn(&mut registry, &mut owners, CLIENT_A, Seat(10));

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        let fingerprint = host.fingerprint(&host_table, &test_replication_identity());
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        assert!(
            !host.produce_for_client(CLIENT_A, 0).unwrap().is_empty(),
            "the original participant received a baseline before disconnect"
        );

        // Participation exit clears tracker state, but not the held seat's store value.
        host.remove_client(CLIENT_A);
        owners.remove_pawn(departing_pawn);
        registry.clear_pawn_seat(departing_pawn);
        registry.despawn(departing_pawn).unwrap();
        add_owned_pawn(&mut registry, &mut owners, CLIENT_A, Seat(10));
        host.register_client(CLIENT_A);
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );

        let xp_id = host
            .schema(&host_table, &test_replication_identity())
            .id_for("currency.xp")
            .unwrap();
        let records = host.produce_for_client(CLIENT_A, 1).unwrap();
        let record = record_for_slot(&records, xp_id);
        assert_eq!(
            record.value,
            WireSlotValue::Number(47.0),
            "the first post-reclaim value comes from the held seat store, not the dropped tracker"
        );
        assert_eq!(
            record.kind,
            postretro_net::state_slots::STATE_RECORD_KIND_FULL_BASELINE,
            "a re-registered connection receives a fresh baseline"
        );

        let mut client_table = per_owner_currency_table();
        ClientStateApply::new().apply_snapshot_state(
            &mut client_table,
            &test_replication_identity(),
            1,
            &fingerprint,
            &records,
        );
        assert_eq!(
            client_table.get("currency.xp").unwrap().value,
            Some(SlotValue::Number(47.0))
        );
    }

    #[test]
    fn per_owner_slot_without_network_stays_host_only_and_unreplicated() {
        let mut host_table = per_owner_currency_table();
        {
            let kill_streak = host_table.get_mut("currency.killStreak").unwrap();
            kill_streak.set_per_seat_value(Seat(10), SlotValue::Number(3.0));
            kill_streak.set_per_seat_value(Seat(11), SlotValue::Number(9.0));
        }
        let mut registry = EntityRegistry::new();
        let mut owners = MovementOwners::new();
        add_owned_pawn(&mut registry, &mut owners, CLIENT_A, Seat(10));
        add_owned_pawn(&mut registry, &mut owners, CLIENT_B, Seat(11));

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        host.register_client(CLIENT_B);
        let schema = host
            .schema(&host_table, &test_replication_identity())
            .clone();
        assert_eq!(
            schema.id_for("currency.killStreak"),
            None,
            "a per-owner declaration with no network scope has no wire slot"
        );
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );

        let xp_id = schema.id_for("currency.xp").unwrap();
        for client_id in [CLIENT_A, CLIENT_B] {
            let records = host.produce_for_client(client_id, 0).unwrap();
            assert!(
                records.iter().all(|record| record.slot_id == xp_id.0),
                "host-only killStreak never enters the owner-private replication tracker"
            );
        }
        let kill_streak = host_table.get("currency.killStreak").unwrap();
        assert_eq!(
            kill_streak.per_seat_value(Seat(10)),
            Some(&SlotValue::Number(3.0))
        );
        assert_eq!(
            kill_streak.per_seat_value(Seat(11)),
            Some(&SlotValue::Number(9.0))
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
    /// table. Health projection is disabled; other owner-private built-ins have no
    /// owner source, so this fixture produces only the shared mod slot.
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
        let fingerprint = host.fingerprint(&host_table, &test_replication_identity());

        // Ingest the frame's shared value once; both clients (and the late joiner) read
        // the same ingested view.
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );

        // Both originally-accepted clients receive the shared value on the first frame.
        for client in [CLIENT_A, CLIENT_B] {
            let records = host
                .produce_for_client(client, 0)
                .expect("accepted client produces records");
            assert_eq!(records.len(), 1, "the one shared fixture slot is produced");

            let mut client_table = net_fixture_table();
            let mut apply = ClientStateApply::new();
            let outcome = apply.apply_snapshot_state(
                &mut client_table,
                &test_replication_identity(),
                0,
                &fingerprint,
                &records,
            );
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
            .produce_for_client(CLIENT_C, 1)
            .expect("late joiner produces records");
        assert_eq!(
            late_records.len(),
            1,
            "late joiner gets a full baseline for the shared slot without a value change"
        );

        let mut late_table = net_fixture_table();
        let mut late_apply = ClientStateApply::new();
        let outcome = late_apply.apply_snapshot_state(
            &mut late_table,
            &test_replication_identity(),
            1,
            &fingerprint,
            &late_records,
        );
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
        let host_table = owner_private_player_table();
        let (registry, owners) = registry_with_descriptor_health(CLIENT_A, 120.0);

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        let fingerprint = host.fingerprint(&host_table, &test_replication_identity());
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let records = host
            .produce_for_client(CLIENT_A, 0)
            .expect("registered client produces records");
        assert_eq!(
            records.len(),
            4,
            "health pair plus reload defaults projected"
        );

        // The slot ids come from the same deterministic schema as store slots.
        let schema = build_test_schema(&host_table);
        let health_id = schema.id_for("player.health").expect("health id");
        let max_id = schema.id_for("player.maxHealth").expect("maxHealth id");
        let record_ids: std::collections::BTreeSet<u16> =
            records.iter().map(|r| r.slot_id).collect();
        assert!(record_ids.contains(&health_id.0));
        assert!(record_ids.contains(&max_id.0));

        let mut client_table = owner_private_player_table();
        let mut client = ClientStateApply::new();
        let outcome = client.apply_snapshot_state(
            &mut client_table,
            &test_replication_identity(),
            0,
            &fingerprint,
            &records,
        );
        assert_eq!(outcome.slot_baselines.len(), 4);
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
        let _real_fingerprint = host.fingerprint(&host_table, &test_replication_identity());
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let records = host.produce_for_client(CLIENT_A, 0).expect("records");

        let mut client_table = shared_and_private_table();
        let mut client = ClientStateApply::new();

        // Run the apply under a captured-log scope so the warn! is recorded.
        let logs = capture(|| {
            let outcome = client.apply_snapshot_state(
                &mut client_table,
                &test_replication_identity(),
                0,
                &[0xAB; 32],
                &records,
            );
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

    /// Read a numeric UI slot value from the REAL UI read-snapshot projection, or
    /// `None` if absent. Pins the exact production destination contract by routing
    /// through `crate::App::build_ui_slot_snapshot` — the slot-table → `slot_values`
    /// projection that feeds `UiReadSnapshot` — rather than a hand-mirrored copy.
    fn ui_snapshot_number(slot_table: &SlotTable, name: &str) -> Option<f32> {
        crate::App::build_ui_slot_snapshot(slot_table)
            .get(name)
            .and_then(|value| match value {
                SlotValue::Number(n) => Some(*n),
                _ => None,
            })
    }

    // Acceptance metric (AC-1): after applying the first full state baseline through the
    // REAL host production → client apply glue, the REAL UI read snapshot
    // (`App::build_ui_slot_snapshot` → `slot_values`) carries both `player.health` and
    // `player.maxHealth` — the connected client no longer renders them as missing. This
    // is a true seam-crossing test: the apply path writes the slot table, the UI path
    // reads it, and the value must survive the crossing.
    #[test]
    fn first_baseline_populates_ui_read_snapshot_player_health_slots() {
        let host_table = owner_private_player_table();
        let (registry, owners, _pawn) = registry_with_owned_health(CLIENT_A, 75.0, 100.0);

        let mut host = HostStateReplication::new();
        host.register_client(CLIENT_A);
        let fingerprint = host.fingerprint(&host_table, &test_replication_identity());
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let records = host
            .produce_for_client(CLIENT_A, 0)
            .expect("registered client produces records");

        // A fresh client table whose player slots have NO value yet: the UI read
        // snapshot must not carry them before the baseline lands.
        let mut client_table = owner_private_player_table();
        client_table.get_mut("player.health").unwrap().value = None;
        client_table.get_mut("player.maxHealth").unwrap().value = None;
        assert!(
            ui_snapshot_number(&client_table, "player.health").is_none()
                && ui_snapshot_number(&client_table, "player.maxHealth").is_none(),
            "before the baseline the player health slots are missing from the UI snapshot"
        );

        let mut client = ClientStateApply::new();
        client.apply_snapshot_state(
            &mut client_table,
            &test_replication_identity(),
            0,
            &fingerprint,
            &records,
        );

        let health = ui_snapshot_number(&client_table, "player.health")
            .expect("player.health present in the UI read snapshot after the first baseline");
        let max_health = ui_snapshot_number(&client_table, "player.maxHealth")
            .expect("player.maxHealth present in the UI read snapshot after the first baseline");
        assert!(
            (health - 75.0).abs() < 1e-4,
            "player.health reached the UI snapshot with the replicated value, got {health}"
        );
        assert!(
            (max_health - 100.0).abs() < 1e-4,
            "player.maxHealth reached the UI snapshot with the replicated value, got {max_health}"
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
        let fingerprint = host.fingerprint(&host_table, &test_replication_identity());

        // Frame 1: the host produces the first FullBaseline — but it is LOST (the
        // client never applies it, so it holds no baseline for net.objective).
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let _lost = host
            .produce_for_client(CLIENT_A, 0)
            .expect("first frame records");

        // Frame 2: the value changes. With an acked baseline (from the host's view the
        // client never acked, so this is actually a FullBaseline fallback). To force a
        // genuine DELTA-against-missing on the client we have the host believe the
        // client acked frame 1, then drop frame 1 on the client.
        let baseline_one = {
            // Re-produce frame 1 to learn its baseline id, ack it on the server so the
            // server will send a delta next, but the CLIENT never saw it.
            host.ingest_frame(
                &host_table,
                &test_replication_identity(),
                &registry,
                &owners,
                &WeaponOwners::new(),
            );
            let records = host
                .produce_for_client(CLIENT_A, 1)
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
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let delta_records = host.produce_for_client(CLIENT_A, 2).expect("delta frame");
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
        let outcome = client.apply_snapshot_state(
            &mut client_table,
            &test_replication_identity(),
            2,
            &fingerprint,
            &delta_records,
        );
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
        host.ingest_frame(
            &host_table,
            &test_replication_identity(),
            &registry,
            &owners,
            &WeaponOwners::new(),
        );
        let repair_records = host.produce_for_client(CLIENT_A, 3).expect("repair frame");
        assert!(
            repair_records
                .iter()
                .any(|r| r.kind == postretro_net::state_slots::STATE_RECORD_KIND_FULL_BASELINE),
            "the refresh forces a full baseline"
        );

        // The client applies the repair and converges — no reconnect needed.
        let repair_outcome = client.apply_snapshot_state(
            &mut client_table,
            &test_replication_identity(),
            3,
            &fingerprint,
            &repair_records,
        );
        assert!(repair_outcome.refresh_requests.is_empty(), "repaired");
        assert_eq!(
            client_table.get("net.objective").unwrap().value,
            Some(SlotValue::Number(4.0)),
            "the slot converges to the authoritative value after refresh repair"
        );
    }
}
