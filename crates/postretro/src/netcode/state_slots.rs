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
    /// (all-`None`) engine slots. Only the two replicated slots should enter the
    /// schema.
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
        // Default engine slots are all `None` scope, so only the two mod slots
        // appear, sorted by dotted name.
        assert_eq!(names, vec!["net.alpha", "net.bravo"]);
        assert_eq!(schema.entries()[0].slot_id, StateSlotId(0));
        assert_eq!(schema.entries()[1].slot_id, StateSlotId(1));
    }

    #[test]
    fn id_and_name_round_trip() {
        let table = table_with_replicated();
        let schema = ReplicatedSlotSchema::build(&table);
        let id = schema.id_for("net.alpha").expect("alpha is replicated");
        assert_eq!(schema.name_for(id), Some("net.alpha"));
        assert_eq!(
            schema.id_for("player.health"),
            None,
            "None-scope slot has no id"
        );
    }

    #[test]
    fn default_table_has_empty_schema() {
        // Every built-in engine slot defaults to `None` scope in Phase 3.5 Task 1
        // (Task 4 flips player.health/maxHealth), so the default schema is empty and
        // its fingerprint is the empty-stream digest.
        let table = SlotTable::new();
        let schema = ReplicatedSlotSchema::build(&table);
        assert!(schema.entries().is_empty());
        assert!(schema.to_net_schema().is_empty());
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
        assert_eq!(net.len(), 2);
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
}
