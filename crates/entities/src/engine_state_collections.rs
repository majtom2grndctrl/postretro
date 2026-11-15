// Engine-state families and lists: catalog entries keyed by data.
// See: context/lib/scripting.md §5 "Engine State SDK"

use crate::engine_state_catalog::{EngineStateDefault, EngineStateValueType};
use crate::slot_table::{ReplicationScope, SlotOwnership, SlotRecord, SlotSchema};

/// One field every family member or list row carries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineStateField<'a> {
    pub name: &'a str,
    pub value_type: EngineStateValueType<'a>,
    pub default: EngineStateDefault<'a>,
}

impl EngineStateField<'_> {
    /// Engine-written, script-readonly record for this field under `network`.
    pub fn slot_record(&self, network: ReplicationScope) -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: self.value_type.slot_type(),
            default: self.default.slot_value(),
            range: None,
            persist: false,
            readonly: true,
            ownership: SlotOwnership::Engine,
            network,
            per_owner: false,
            accumulate: None,
        })
    }
}

/// Entries keyed by a data key, reached through a generated accessor:
/// `getGameState().occupancy.tag("hill").occupants` reads the slot
/// `occupancy.hill.occupants`. Members exist only while the engine declares
/// them; the accessor builds refs for any key.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineStateFamily<'a> {
    /// SDK path of the node carrying the accessor; joined, also the wire
    /// prefix of every member slot.
    pub sdk_path: &'a [&'a str],
    /// Accessor method name on that node.
    pub accessor: &'a str,
    pub fields: &'a [EngineStateField<'a>],
    pub network: ReplicationScope,
}

impl EngineStateFamily<'_> {
    pub fn wire_prefix(&self) -> String {
        self.sdk_path.join(".")
    }

    /// The slot holding `field` of the member keyed `key`.
    pub fn slot_name(&self, key: &str, field: &str) -> String {
        format!("{}.{key}.{field}", self.wire_prefix())
    }

    /// Split a member slot name into its key and field. `None` when the name is
    /// not under this family or names no declared field. A key may itself hold
    /// dots, so callers that require a slot-safe key check it separately.
    pub fn split_slot_name<'n>(&self, slot: &'n str) -> Option<(&'n str, &'n str)> {
        let prefix = self.wire_prefix();
        let rest = slot.strip_prefix(prefix.as_str())?.strip_prefix('.')?;
        let (key, field) = rest.rsplit_once('.')?;
        (!key.is_empty() && self.fields.iter().any(|f| f.name == field)).then_some((key, field))
    }

    /// Whether `key` can form member slot names: non-empty, with no `.` (which
    /// would nest the slot) and no `:` (which slot names reject).
    pub fn is_slot_safe_key(key: &str) -> bool {
        !key.is_empty() && !key.contains(['.', ':'])
    }
}

/// A family keyed by row, `0` to a fixed capacity, each row carrying a
/// `present` field. Every row's slots exist for the process's life.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineStateList<'a> {
    /// SDK path of the list; joined, also the wire prefix of every row slot.
    pub sdk_path: &'a [&'a str],
    pub capacity: usize,
    pub fields: &'a [EngineStateField<'a>],
    pub network: ReplicationScope,
}

/// The field every list row carries and a row shows under.
pub const LIST_PRESENT_FIELD: &str = "present";

impl EngineStateList<'_> {
    pub fn wire_prefix(&self) -> String {
        self.sdk_path.join(".")
    }

    pub fn slot_name(&self, row: usize, field: &str) -> String {
        format!("{}.{row}.{field}", self.wire_prefix())
    }

    /// Every row slot as `(name within the namespace, record)`.
    pub fn namespace_slots(&self) -> Vec<(String, SlotRecord)> {
        let in_namespace = self.sdk_path[1..].join(".");
        let in_namespace = in_namespace.as_str();
        (0..self.capacity)
            .flat_map(|row| {
                self.fields.iter().map(move |field| {
                    let name = if in_namespace.is_empty() {
                        format!("{row}.{}", field.name)
                    } else {
                        format!("{in_namespace}.{row}.{}", field.name)
                    };
                    (name, field.slot_record(self.network))
                })
            })
            .collect()
    }
}

/// `session.players` row status names, in roster wire order.
pub const PLAYER_STATUS_VALUES: &[&str] = &["loading", "inPlay", "down", "reconnecting"];

const OCCUPANCY_FIELDS: &[EngineStateField<'static>] = &[
    EngineStateField {
        name: "occupiedCount",
        value_type: EngineStateValueType::Number,
        default: EngineStateDefault::Number(0.0),
    },
    EngineStateField {
        name: "occupants",
        value_type: EngineStateValueType::Number,
        default: EngineStateDefault::Number(0.0),
    },
];

const SESSION_PLAYER_FIELDS: &[EngineStateField<'static>] = &[
    EngineStateField {
        name: LIST_PRESENT_FIELD,
        value_type: EngineStateValueType::Boolean,
        default: EngineStateDefault::Boolean(false),
    },
    EngineStateField {
        name: "name",
        value_type: EngineStateValueType::String,
        default: EngineStateDefault::String(""),
    },
    EngineStateField {
        name: "status",
        value_type: EngineStateValueType::Enum {
            values: PLAYER_STATUS_VALUES,
        },
        default: EngineStateDefault::Enum("loading"),
    },
    EngineStateField {
        name: "isLocal",
        value_type: EngineStateValueType::Boolean,
        default: EngineStateDefault::Boolean(false),
    },
];

/// Per-tag trigger-volume occupancy, host-computed and shared.
pub const OCCUPANCY_FAMILY: EngineStateFamily<'static> = EngineStateFamily {
    sdk_path: &["occupancy"],
    accessor: "tag",
    fields: OCCUPANCY_FIELDS,
    network: ReplicationScope::SharedGlobal,
};

/// One display row per session seat. Each machine writes its own rows: the
/// host from its seat table, a client from the roster.
pub const SESSION_PLAYERS_LIST: EngineStateList<'static> = EngineStateList {
    sdk_path: &["session", "players"],
    capacity: postretro_foundation::seat::SESSION_PLAYER_CAPACITY,
    fields: SESSION_PLAYER_FIELDS,
    network: ReplicationScope::None,
};

pub const BUILTIN_ENGINE_STATE_FAMILIES: &[EngineStateFamily<'static>] = &[OCCUPANCY_FAMILY];
pub const BUILTIN_ENGINE_STATE_LISTS: &[EngineStateList<'static>] = &[SESSION_PLAYERS_LIST];

/// Engine namespaces declared at run time, which no mod store may declare or
/// overlap.
pub const RESERVED_ENGINE_NAMESPACES: &[&str] = &["occupancy"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_slot_names_split_back_into_key_and_field() {
        assert_eq!(
            OCCUPANCY_FAMILY.slot_name("hill", "occupants"),
            "occupancy.hill.occupants"
        );
        assert_eq!(
            OCCUPANCY_FAMILY.split_slot_name("occupancy.vault-plates.occupiedCount"),
            Some(("vault-plates", "occupiedCount"))
        );
        assert_eq!(
            OCCUPANCY_FAMILY.split_slot_name("occupancy.a.b.occupants"),
            Some(("a.b", "occupants")),
            "a dotted key splits off its field; the caller rejects it"
        );
        assert_eq!(OCCUPANCY_FAMILY.split_slot_name("occupancy.hill.held"), None);
        assert_eq!(OCCUPANCY_FAMILY.split_slot_name("occupancyBoard.x.occupants"), None);
        assert!(EngineStateFamily::is_slot_safe_key("vault-plates"));
        assert!(!EngineStateFamily::is_slot_safe_key("a.b"));
        assert!(!EngineStateFamily::is_slot_safe_key("a:b"));
        assert!(!EngineStateFamily::is_slot_safe_key(""));
    }

    #[test]
    fn session_player_rows_span_the_session_capacity() {
        let slots = SESSION_PLAYERS_LIST.namespace_slots();
        assert_eq!(
            slots.len(),
            postretro_foundation::seat::SESSION_PLAYER_CAPACITY * SESSION_PLAYER_FIELDS.len()
        );
        assert_eq!(slots[0].0, "players.0.present");
        assert!(slots.iter().all(|(_, record)| record.schema.readonly));
        assert_eq!(
            SESSION_PLAYERS_LIST.slot_name(8, "isLocal"),
            "session.players.8.isLocal"
        );
    }
}
