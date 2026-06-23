// Built-in durable engine state catalog.
// See: context/lib/scripting.md §5 "Engine State SDK"

use std::collections::{BTreeMap, BTreeSet};

use super::slot_table::{
    NumericRange, ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
};

/// Script-side write capability for a built-in engine-owned state slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EngineStateCapability {
    Readonly,
    Writable,
}

impl EngineStateCapability {
    pub(crate) fn is_writable(self) -> bool {
        matches!(self, Self::Writable)
    }
}

/// Declared SDK value type for a built-in engine-owned state slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EngineStateValueType<'a> {
    Number,
    Boolean,
    String,
    Enum { values: &'a [&'a str] },
    Array,
}

/// Default value metadata for a built-in engine-owned state slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum EngineStateDefault<'a> {
    None,
    Number(f32),
    Boolean(bool),
    String(&'a str),
    Enum(&'a str),
    Array(&'a [f32]),
}

/// One engine-owned durable state declaration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EngineStateCatalogEntry<'a> {
    /// Stable retained wire name used by descriptors, reactions, persistence,
    /// and runtime store access.
    pub(crate) wire_name: &'a str,
    /// Explicit generated SDK path segments under `getGameState()`.
    pub(crate) sdk_path: &'a [&'a str],
    pub(crate) value_type: EngineStateValueType<'a>,
    pub(crate) default: EngineStateDefault<'a>,
    pub(crate) range: Option<NumericRange>,
    pub(crate) persist: bool,
    pub(crate) capability: EngineStateCapability,
    /// Replication scope for this engine slot (M15 Phase 3.5). Defaults to `None`
    /// for every existing slot in this phase; Task 4 flips `player.health` /
    /// `player.maxHealth` to `OwnerPrivatePlayer`.
    pub(crate) network: ReplicationScope,
}

impl EngineStateCatalogEntry<'_> {
    pub(crate) fn slot_record(&self) -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: self.value_type.slot_type(),
            default: self.default.slot_value(),
            range: self.range,
            persist: self.persist,
            readonly: !self.capability.is_writable(),
            ownership: SlotOwnership::Engine,
            network: self.network,
        })
    }
}

impl EngineStateValueType<'_> {
    fn slot_type(self) -> SlotType {
        match self {
            Self::Number => SlotType::Number,
            Self::Boolean => SlotType::Boolean,
            Self::String => SlotType::String,
            Self::Enum { values } => SlotType::Enum {
                values: values.iter().map(|value| (*value).to_string()).collect(),
            },
            Self::Array => SlotType::Array,
        }
    }

    pub(crate) fn to_ts(self) -> String {
        match self {
            Self::Number => "number".to_string(),
            Self::Boolean => "boolean".to_string(),
            Self::String => "string".to_string(),
            Self::Enum { values } => values
                .iter()
                .map(|value| format!("\"{value}\""))
                .collect::<Vec<_>>()
                .join(" | "),
            Self::Array => "ReadonlyArray<number>".to_string(),
        }
    }

    pub(crate) fn to_luau(self) -> String {
        match self {
            Self::Number => "number".to_string(),
            Self::Boolean => "boolean".to_string(),
            Self::String => "string".to_string(),
            Self::Enum { values } => values
                .iter()
                .map(|value| format!("\"{value}\""))
                .collect::<Vec<_>>()
                .join(" | "),
            Self::Array => "{number}".to_string(),
        }
    }
}

impl EngineStateDefault<'_> {
    fn slot_value(self) -> Option<SlotValue> {
        match self {
            Self::None => None,
            Self::Number(value) => Some(SlotValue::Number(value)),
            Self::Boolean(value) => Some(SlotValue::Boolean(value)),
            Self::String(value) => Some(SlotValue::String(value.to_string())),
            Self::Enum(value) => Some(SlotValue::Enum(value.to_string())),
            Self::Array(values) => Some(SlotValue::Array(values.to_vec())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum EngineStateCatalogError {
    #[error("engine-state catalog has duplicate wire slot `{wire_name}`")]
    DuplicateWireName { wire_name: String },
    #[error("engine-state catalog has duplicate SDK path `{sdk_path}`")]
    DuplicateSdkPath { sdk_path: String },
    #[error("engine-state catalog uses SDK path `{sdk_path}` as both a leaf and an object")]
    LeafObjectPathCollision { sdk_path: String },
    #[error("engine-state catalog entry `{wire_name}` has invalid SDK path segment `{segment}`")]
    InvalidSdkPathSegment { wire_name: String, segment: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EngineStateTreeNode {
    Object(BTreeMap<String, EngineStateTreeNode>),
    Leaf { entry_index: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EngineStateTree {
    root: BTreeMap<String, EngineStateTreeNode>,
}

impl EngineStateTree {
    pub(crate) fn root(&self) -> &BTreeMap<String, EngineStateTreeNode> {
        &self.root
    }

    #[cfg(test)]
    fn leaf_paths(&self) -> Vec<String> {
        fn collect(prefix: &mut Vec<String>, node: &EngineStateTreeNode, out: &mut Vec<String>) {
            match node {
                EngineStateTreeNode::Leaf { .. } => out.push(prefix.join(".")),
                EngineStateTreeNode::Object(children) => {
                    for (segment, child) in children {
                        prefix.push(segment.clone());
                        collect(prefix, child, out);
                        prefix.pop();
                    }
                }
            }
        }

        let mut out = Vec::new();
        let mut prefix = Vec::new();
        for (segment, child) in &self.root {
            prefix.push(segment.clone());
            collect(&mut prefix, child, &mut out);
            prefix.pop();
        }
        out
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EngineStateCatalog {
    entries: Vec<EngineStateCatalogEntry<'static>>,
    tree: EngineStateTree,
}

impl EngineStateCatalog {
    pub(crate) fn entries(&self) -> &[EngineStateCatalogEntry<'static>] {
        &self.entries
    }

    pub(crate) fn tree(&self) -> &EngineStateTree {
        &self.tree
    }

    pub(crate) fn store_declarations(&self) -> Vec<(String, Vec<(String, SlotRecord)>)> {
        let mut namespaces: BTreeMap<String, Vec<(String, SlotRecord)>> = BTreeMap::new();
        for entry in &self.entries {
            let (namespace, slot_name) = entry
                .wire_name
                .split_once('.')
                .expect("built-in wire names must be dotted");
            namespaces
                .entry(namespace.to_string())
                .or_default()
                .push((slot_name.to_string(), entry.slot_record()));
        }
        namespaces.into_iter().collect()
    }
}

pub(crate) fn engine_state_catalog() -> Result<EngineStateCatalog, EngineStateCatalogError> {
    EngineStateCatalog::from_entries(BUILTIN_ENGINE_STATE)
}

impl EngineStateCatalog {
    pub(crate) fn from_entries(
        entries: &[EngineStateCatalogEntry<'static>],
    ) -> Result<Self, EngineStateCatalogError> {
        let mut sorted = entries.to_vec();
        sorted.sort_by(|left, right| {
            left.sdk_path
                .cmp(right.sdk_path)
                .then_with(|| left.wire_name.cmp(right.wire_name))
        });
        let tree = validate_entries(&sorted)?;
        Ok(Self {
            entries: sorted,
            tree,
        })
    }
}

fn validate_entries(
    entries: &[EngineStateCatalogEntry<'_>],
) -> Result<EngineStateTree, EngineStateCatalogError> {
    let mut wire_names = BTreeSet::new();
    let mut sdk_paths = BTreeSet::new();
    let mut root = BTreeMap::new();

    for (entry_index, entry) in entries.iter().enumerate() {
        if !wire_names.insert(entry.wire_name) {
            return Err(EngineStateCatalogError::DuplicateWireName {
                wire_name: entry.wire_name.to_string(),
            });
        }

        validate_sdk_path(entry)?;
        let sdk_path = sdk_path_string(entry.sdk_path);
        if !sdk_paths.insert(sdk_path.clone()) {
            return Err(EngineStateCatalogError::DuplicateSdkPath { sdk_path });
        }
        insert_tree_path(&mut root, entry.sdk_path, entry_index)?;
    }

    Ok(EngineStateTree { root })
}

fn validate_sdk_path(entry: &EngineStateCatalogEntry<'_>) -> Result<(), EngineStateCatalogError> {
    if entry.sdk_path.is_empty() {
        return Err(EngineStateCatalogError::InvalidSdkPathSegment {
            wire_name: entry.wire_name.to_string(),
            segment: String::new(),
        });
    }

    for segment in entry.sdk_path {
        if !is_valid_sdk_segment(segment) {
            return Err(EngineStateCatalogError::InvalidSdkPathSegment {
                wire_name: entry.wire_name.to_string(),
                segment: (*segment).to_string(),
            });
        }
    }
    Ok(())
}

fn is_valid_sdk_segment(segment: &str) -> bool {
    if matches!(segment, "__proto__" | "prototype" | "constructor") {
        return false;
    }

    let mut chars = segment.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn insert_tree_path(
    root: &mut BTreeMap<String, EngineStateTreeNode>,
    path: &[&str],
    entry_index: usize,
) -> Result<(), EngineStateCatalogError> {
    let mut children = root;
    let mut prefix = Vec::new();
    for (index, segment) in path.iter().enumerate() {
        let is_leaf = index == path.len() - 1;
        prefix.push((*segment).to_string());
        if is_leaf {
            match children.get_mut(*segment) {
                Some(EngineStateTreeNode::Object(_)) => {
                    return Err(EngineStateCatalogError::LeafObjectPathCollision {
                        sdk_path: prefix.join("."),
                    });
                }
                Some(EngineStateTreeNode::Leaf { .. }) => {
                    return Err(EngineStateCatalogError::DuplicateSdkPath {
                        sdk_path: prefix.join("."),
                    });
                }
                None => {
                    children.insert(
                        segment.to_string(),
                        EngineStateTreeNode::Leaf { entry_index },
                    );
                }
            }
        } else {
            let node = children
                .entry(segment.to_string())
                .or_insert_with(|| EngineStateTreeNode::Object(BTreeMap::new()));
            match node {
                EngineStateTreeNode::Object(next) => children = next,
                EngineStateTreeNode::Leaf { .. } => {
                    return Err(EngineStateCatalogError::LeafObjectPathCollision {
                        sdk_path: prefix.join("."),
                    });
                }
            }
        }
    }
    Ok(())
}

fn sdk_path_string(path: &[&str]) -> String {
    path.join(".")
}

const INPUT_MODE_VALUES: &[&str] = &["pointer", "focus"];

const BUILTIN_ENGINE_STATE: &[EngineStateCatalogEntry<'static>] = &[
    EngineStateCatalogEntry {
        wire_name: "player.health",
        sdk_path: &["player", "health"],
        value_type: EngineStateValueType::Number,
        default: EngineStateDefault::None,
        range: None,
        persist: false,
        capability: EngineStateCapability::Readonly,
        // M15 Phase 3.5 Task 4: server-authoritative, sent only to the owning
        // accepted client. The Task 3 production path already projects each owned
        // pawn's `HealthComponent` per `(StateSlotId, owner_client_id)`; this flip
        // makes the slot replicate.
        network: ReplicationScope::OwnerPrivatePlayer,
    },
    EngineStateCatalogEntry {
        wire_name: "player.maxHealth",
        sdk_path: &["player", "maxHealth"],
        value_type: EngineStateValueType::Number,
        default: EngineStateDefault::None,
        range: Some(NumericRange {
            min: 1.0,
            max: f32::INFINITY,
        }),
        persist: false,
        capability: EngineStateCapability::Readonly,
        // M15 Phase 3.5 Task 4: owner-private, paired with `player.health`.
        network: ReplicationScope::OwnerPrivatePlayer,
    },
    EngineStateCatalogEntry {
        wire_name: "screen.flash",
        sdk_path: &["screen", "flash"],
        value_type: EngineStateValueType::Array,
        default: EngineStateDefault::Array(&[0.0, 0.0, 0.0, 0.0]),
        range: None,
        persist: false,
        capability: EngineStateCapability::Readonly,
        network: ReplicationScope::None,
    },
    EngineStateCatalogEntry {
        wire_name: "screen.vignette",
        sdk_path: &["screen", "vignette"],
        value_type: EngineStateValueType::Array,
        default: EngineStateDefault::Array(&[0.0, 0.0, 0.0, 0.0]),
        range: None,
        persist: false,
        capability: EngineStateCapability::Readonly,
        network: ReplicationScope::None,
    },
    EngineStateCatalogEntry {
        wire_name: "screen.shake",
        sdk_path: &["screen", "shake"],
        value_type: EngineStateValueType::Array,
        default: EngineStateDefault::Array(&[0.0, 0.0]),
        range: None,
        persist: false,
        capability: EngineStateCapability::Readonly,
        network: ReplicationScope::None,
    },
    EngineStateCatalogEntry {
        wire_name: "input.mode",
        sdk_path: &["input", "mode"],
        value_type: EngineStateValueType::Enum {
            values: INPUT_MODE_VALUES,
        },
        default: EngineStateDefault::Enum("focus"),
        range: None,
        persist: false,
        capability: EngineStateCapability::Readonly,
        network: ReplicationScope::None,
    },
    EngineStateCatalogEntry {
        wire_name: "ui.textEntry",
        sdk_path: &["ui", "textEntry"],
        value_type: EngineStateValueType::String,
        default: EngineStateDefault::String(""),
        range: None,
        persist: false,
        capability: EngineStateCapability::Writable,
        network: ReplicationScope::None,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: EngineStateCatalogEntry<'static> = EngineStateCatalogEntry {
        wire_name: "alpha.value",
        sdk_path: &["alpha", "value"],
        value_type: EngineStateValueType::Number,
        default: EngineStateDefault::None,
        range: None,
        persist: false,
        capability: EngineStateCapability::Readonly,
        network: ReplicationScope::None,
    };

    fn entry(
        wire_name: &'static str,
        sdk_path: &'static [&'static str],
    ) -> EngineStateCatalogEntry<'static> {
        EngineStateCatalogEntry {
            wire_name,
            sdk_path,
            ..BASE
        }
    }

    #[test]
    fn catalog_entries_are_sorted_by_sdk_path() {
        let catalog = EngineStateCatalog::from_entries(&[
            entry("zeta.value", &["zeta", "value"]),
            entry("alpha.value", &["alpha", "value"]),
            entry("alpha.other", &["alpha", "other"]),
        ])
        .unwrap();

        let paths = catalog
            .entries()
            .iter()
            .map(|entry| entry.sdk_path.join("."))
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["alpha.other", "alpha.value", "zeta.value"]);
        assert_eq!(
            catalog.tree().leaf_paths(),
            vec!["alpha.other", "alpha.value", "zeta.value"]
        );
    }

    #[test]
    fn catalog_rejects_duplicate_wire_names() {
        let err = EngineStateCatalog::from_entries(&[
            entry("alpha.value", &["alpha", "value"]),
            entry("alpha.value", &["alpha", "other"]),
        ])
        .unwrap_err();

        assert_eq!(
            err,
            EngineStateCatalogError::DuplicateWireName {
                wire_name: "alpha.value".to_string()
            }
        );
    }

    #[test]
    fn catalog_rejects_duplicate_sdk_paths() {
        let err = EngineStateCatalog::from_entries(&[
            entry("alpha.value", &["alpha", "value"]),
            entry("beta.value", &["alpha", "value"]),
        ])
        .unwrap_err();

        assert_eq!(
            err,
            EngineStateCatalogError::DuplicateSdkPath {
                sdk_path: "alpha.value".to_string()
            }
        );
    }

    #[test]
    fn catalog_rejects_leaf_object_path_collisions() {
        let err = EngineStateCatalog::from_entries(&[
            entry("alpha.value", &["alpha", "value"]),
            entry("alpha.value.current", &["alpha", "value", "current"]),
        ])
        .unwrap_err();

        assert_eq!(
            err,
            EngineStateCatalogError::LeafObjectPathCollision {
                sdk_path: "alpha.value".to_string()
            }
        );
    }

    #[test]
    fn catalog_rejects_empty_sdk_paths_and_segments() {
        let empty_path =
            EngineStateCatalog::from_entries(&[entry("alpha.value", &[])]).unwrap_err();
        assert_eq!(
            empty_path,
            EngineStateCatalogError::InvalidSdkPathSegment {
                wire_name: "alpha.value".to_string(),
                segment: String::new()
            }
        );

        let empty_segment =
            EngineStateCatalog::from_entries(&[entry("alpha.value", &["alpha", ""])]).unwrap_err();
        assert_eq!(
            empty_segment,
            EngineStateCatalogError::InvalidSdkPathSegment {
                wire_name: "alpha.value".to_string(),
                segment: String::new()
            }
        );
    }

    #[test]
    fn catalog_rejects_invalid_sdk_path_segments() {
        let err = EngineStateCatalog::from_entries(&[entry("alpha.value", &["alpha", "bad-name"])])
            .unwrap_err();

        assert_eq!(
            err,
            EngineStateCatalogError::InvalidSdkPathSegment {
                wire_name: "alpha.value".to_string(),
                segment: "bad-name".to_string()
            }
        );
    }

    #[test]
    fn catalog_rejects_js_magic_sdk_path_segments() {
        const CASES: &[(&str, &[&str])] = &[
            ("__proto__", &["alpha", "__proto__"]),
            ("prototype", &["alpha", "prototype"]),
            ("constructor", &["alpha", "constructor"]),
        ];

        for (segment, path) in CASES {
            let err = EngineStateCatalog::from_entries(&[entry("alpha.value", path)]).unwrap_err();

            assert_eq!(
                err,
                EngineStateCatalogError::InvalidSdkPathSegment {
                    wire_name: "alpha.value".to_string(),
                    segment: (*segment).to_string()
                }
            );
        }
    }

    #[test]
    fn built_in_catalog_preserves_wire_names_and_capabilities() {
        let catalog = engine_state_catalog().unwrap();
        let entries = catalog.entries();
        let wire_names = entries
            .iter()
            .map(|entry| entry.wire_name)
            .collect::<Vec<_>>();

        assert_eq!(
            wire_names,
            vec![
                "input.mode",
                "player.health",
                "player.maxHealth",
                "screen.flash",
                "screen.shake",
                "screen.vignette",
                "ui.textEntry",
            ]
        );

        let ui_text_entry = entries
            .iter()
            .find(|entry| entry.wire_name == "ui.textEntry")
            .unwrap();
        assert_eq!(ui_text_entry.sdk_path, &["ui", "textEntry"]);
        assert_eq!(ui_text_entry.capability, EngineStateCapability::Writable);

        let player_max_health = entries
            .iter()
            .find(|entry| entry.wire_name == "player.maxHealth")
            .unwrap();
        assert_eq!(player_max_health.sdk_path, &["player", "maxHealth"]);
        assert_eq!(player_max_health.value_type, EngineStateValueType::Number);
        assert_eq!(player_max_health.default, EngineStateDefault::None);
        assert_eq!(
            player_max_health.range,
            Some(NumericRange {
                min: 1.0,
                max: f32::INFINITY,
            })
        );
        assert_eq!(
            player_max_health.capability,
            EngineStateCapability::Readonly
        );
    }

    #[test]
    fn player_health_slots_are_owner_private_replicated() {
        // M15 Phase 3.5 Task 4: the two engine player slots replicate owner-private
        // (server sends each only to the owning client); every other built-in slot
        // stays local-only (`None`).
        let catalog = engine_state_catalog().unwrap();
        let entries = catalog.entries();

        for wire_name in ["player.health", "player.maxHealth"] {
            let entry = entries
                .iter()
                .find(|entry| entry.wire_name == wire_name)
                .unwrap();
            assert_eq!(
                entry.network,
                ReplicationScope::OwnerPrivatePlayer,
                "{wire_name} must be owner-private replicated"
            );
        }

        for entry in entries {
            if entry.wire_name != "player.health" && entry.wire_name != "player.maxHealth" {
                assert_eq!(
                    entry.network,
                    ReplicationScope::None,
                    "{} must stay local-only in Phase 3.5",
                    entry.wire_name
                );
            }
        }
    }
}
