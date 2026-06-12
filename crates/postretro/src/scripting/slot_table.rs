// Engine-global state-store declarations, values, and reconciliation.
// See: context/lib/scripting.md §5 "Durable State Store"

use std::collections::{BTreeMap, HashMap, HashSet};

/// Runtime value stored in a state slot.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SlotValue {
    Number(f32),
    Boolean(bool),
    String(String),
    Enum(String),
    Array(Vec<f32>),
}

/// Declared value type and any type-specific schema metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SlotType {
    Number,
    Boolean,
    String,
    Enum { values: Vec<String> },
    Array,
}

/// Inclusive bounds for a numeric slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NumericRange {
    pub(crate) min: f32,
    pub(crate) max: f32,
}

/// Identifies which side declared and owns a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotOwnership {
    Engine,
    Mod,
}

/// Immutable declaration metadata for a state slot.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SlotSchema {
    pub(crate) slot_type: SlotType,
    pub(crate) default: Option<SlotValue>,
    pub(crate) range: Option<NumericRange>,
    pub(crate) persist: bool,
    pub(crate) readonly: bool,
    pub(crate) ownership: SlotOwnership,
}

/// A declared slot and its current runtime value.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SlotRecord {
    pub(crate) schema: SlotSchema,
    pub(crate) value: Option<SlotValue>,
}

impl SlotRecord {
    pub(crate) fn new(schema: SlotSchema) -> Self {
        let value = schema.default.clone();
        Self { schema, value }
    }
}

/// One validated namespace declaration, owned independently of a script VM.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoreDeclaration {
    pub(crate) namespace: String,
    pub(crate) records: Vec<(String, SlotRecord)>,
}

/// Declarations collected during one mod-init or staged-build attempt.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StoreDeclarationSet {
    declarations: BTreeMap<String, StoreDeclaration>,
}

impl StoreDeclarationSet {
    pub(crate) fn add(
        &mut self,
        declaration: StoreDeclaration,
    ) -> Result<(), NamespaceInsertError> {
        validate_namespace_records(&declaration.namespace, &declaration.records)?;

        if let Some(existing) = self
            .declarations
            .keys()
            .find(|existing| namespaces_overlap(&declaration.namespace, existing))
        {
            return Err(NamespaceInsertError::NamespaceCollision {
                namespace: declaration.namespace,
                existing: existing.clone(),
            });
        }

        self.declarations
            .insert(declaration.namespace.clone(), declaration);
        Ok(())
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &StoreDeclaration> {
        self.declarations.values()
    }

    pub(crate) fn len(&self) -> usize {
        self.declarations.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.declarations.is_empty()
    }
}

/// A prevalidated live-table update. Applying it cannot encounter collisions.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct StoreReconcilePlan {
    new_declarations: Vec<StoreDeclaration>,
}

impl StoreReconcilePlan {
    pub(crate) fn added_namespace_count(&self) -> usize {
        self.new_declarations.len()
    }
}

/// Engine-global state slots keyed by stable dotted names.
///
/// This table intentionally has no clear or teardown API. It lives on
/// `ScriptCtx` for the process lifetime.
#[derive(Debug)]
pub(crate) struct SlotTable {
    slots: HashMap<String, SlotRecord>,
    namespaces: HashSet<String>,
}

impl Default for SlotTable {
    fn default() -> Self {
        let mut table = Self {
            slots: HashMap::new(),
            namespaces: HashSet::new(),
        };
        table
            .insert_namespace("player", engine_player_slots())
            .expect("built-in player store schema must be valid");
        table
    }
}

impl SlotTable {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Atomically inserts every slot in a namespace.
    ///
    /// Namespace equality and dotted-prefix overlap both count as collisions:
    /// registering `player.stats` cannot partially extend the engine-owned
    /// `player` namespace, and registering `player` cannot absorb an existing
    /// `player.stats` namespace.
    pub(crate) fn insert_namespace(
        &mut self,
        namespace: &str,
        records: Vec<(String, SlotRecord)>,
    ) -> Result<(), NamespaceInsertError> {
        validate_namespace_records(namespace, &records)?;

        if let Some(existing) = self
            .namespaces
            .iter()
            .find(|existing| namespaces_overlap(namespace, existing))
        {
            return Err(NamespaceInsertError::NamespaceCollision {
                namespace: namespace.to_string(),
                existing: existing.clone(),
            });
        }

        let full_names = records
            .iter()
            .map(|(slot_name, _)| format!("{namespace}.{slot_name}"))
            .collect::<Vec<_>>();
        for full_name in &full_names {
            if self.slots.contains_key(full_name) {
                return Err(NamespaceInsertError::SlotCollision {
                    name: full_name.clone(),
                });
            }
        }

        self.namespaces.insert(namespace.to_string());
        for ((_, record), full_name) in records.into_iter().zip(full_names) {
            self.slots.insert(full_name, record);
        }
        Ok(())
    }

    /// Validate a complete declaration attempt without mutating live values.
    ///
    /// Identical schemas are compatible and become no-ops. New namespaces are
    /// added by the returned plan. Changed schemas and namespace overlap are
    /// rejected before any live mutation.
    pub(crate) fn plan_reconcile(
        &self,
        declarations: &StoreDeclarationSet,
    ) -> Result<StoreReconcilePlan, NamespaceInsertError> {
        let mut new_declarations = Vec::new();

        for declaration in declarations.iter() {
            validate_namespace_records(&declaration.namespace, &declaration.records)?;

            if self.namespaces.contains(&declaration.namespace) {
                if !self.namespace_schema_matches(declaration) {
                    return Err(NamespaceInsertError::IncompatibleSchema {
                        namespace: declaration.namespace.clone(),
                    });
                }
                continue;
            }

            if let Some(existing) = self
                .namespaces
                .iter()
                .find(|existing| namespaces_overlap(&declaration.namespace, existing))
            {
                return Err(NamespaceInsertError::NamespaceCollision {
                    namespace: declaration.namespace.clone(),
                    existing: existing.clone(),
                });
            }

            for (slot_name, _) in &declaration.records {
                let full_name = format!("{}.{}", declaration.namespace, slot_name);
                if self.slots.contains_key(&full_name) {
                    return Err(NamespaceInsertError::SlotCollision { name: full_name });
                }
            }
            new_declarations.push(declaration.clone());
        }

        Ok(StoreReconcilePlan { new_declarations })
    }

    pub(crate) fn apply_reconcile_plan(&mut self, plan: StoreReconcilePlan) {
        for declaration in plan.new_declarations {
            self.namespaces.insert(declaration.namespace.clone());
            for (slot_name, record) in declaration.records {
                self.slots
                    .insert(format!("{}.{}", declaration.namespace, slot_name), record);
            }
        }
    }

    /// Inserts a new stable name without replacing an existing declaration.
    pub(crate) fn insert(
        &mut self,
        name: String,
        record: SlotRecord,
    ) -> Result<(), SlotInsertError> {
        if self.slots.contains_key(&name) {
            return Err(SlotInsertError { name });
        }
        self.slots.insert(name, record);
        Ok(())
    }

    /// Attach (or replace) the inclusive numeric range on an engine-owned
    /// number slot, re-clamping any current value into the new bounds.
    ///
    /// Engine-only by contract: the slot must be `SlotOwnership::Engine` and
    /// `SlotType::Number`. This exists because some engine ranges are mod data
    /// (e.g. `player.health`'s `[0, max]`, where `max` is an authored health
    /// descriptor) and so cannot be declared at `SlotTable` construction — they
    /// attach when the producing component materializes, and re-attach on hot
    /// reload. Subsequent `write_store_slot` calls enforce the range via the
    /// existing validation/clamp path; this mutation only installs it (and
    /// re-clamps the value already present, if any).
    pub(crate) fn set_engine_numeric_range(
        &mut self,
        name: &str,
        range: NumericRange,
    ) -> Result<(), SlotRangeError> {
        let record = self
            .slots
            .get_mut(name)
            .ok_or_else(|| SlotRangeError::UnknownSlot {
                name: name.to_string(),
            })?;
        if record.schema.ownership != SlotOwnership::Engine {
            return Err(SlotRangeError::NotEngineOwned {
                name: name.to_string(),
            });
        }
        if record.schema.slot_type != SlotType::Number {
            return Err(SlotRangeError::NotNumeric {
                name: name.to_string(),
            });
        }
        record.schema.range = Some(range);
        // Re-clamp an already-published value so the table never holds a value
        // outside the freshly-installed bounds (e.g. an authored `max`
        // reduction on hot reload that drops below the live HP read last frame).
        if let Some(SlotValue::Number(value)) = record.value {
            record.value = Some(SlotValue::Number(value.clamp(range.min, range.max)));
        }
        Ok(())
    }

    pub(crate) fn get(&self, name: &str) -> Option<&SlotRecord> {
        self.slots.get(name)
    }

    pub(crate) fn get_mut(&mut self, name: &str) -> Option<&mut SlotRecord> {
        self.slots.get_mut(name)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &SlotRecord)> {
        self.slots
            .iter()
            .map(|(name, record)| (name.as_str(), record))
    }

    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    fn namespace_schema_matches(&self, declaration: &StoreDeclaration) -> bool {
        let prefix = format!("{}.", declaration.namespace);
        let existing = self
            .slots
            .iter()
            .filter_map(|(name, record)| {
                name.strip_prefix(&prefix)
                    .map(|slot_name| (slot_name, &record.schema))
            })
            .collect::<HashMap<_, _>>();

        existing.len() == declaration.records.len()
            && declaration.records.iter().all(|(slot_name, record)| {
                existing.get(slot_name.as_str()) == Some(&&record.schema)
            })
    }
}

fn validate_namespace_records(
    namespace: &str,
    records: &[(String, SlotRecord)],
) -> Result<(), NamespaceInsertError> {
    if namespace.is_empty() {
        return Err(NamespaceInsertError::InvalidNamespace);
    }

    let mut pending = HashSet::with_capacity(records.len());
    for (slot_name, _) in records {
        if slot_name.is_empty() {
            return Err(NamespaceInsertError::InvalidSlotName);
        }
        let full_name = format!("{namespace}.{slot_name}");
        if !pending.insert(full_name.clone()) {
            return Err(NamespaceInsertError::SlotCollision { name: full_name });
        }
    }
    Ok(())
}

fn namespaces_overlap(left: &str, right: &str) -> bool {
    left == right
        || left
            .strip_prefix(right)
            .is_some_and(|suffix| suffix.starts_with('.'))
        || right
            .strip_prefix(left)
            .is_some_and(|suffix| suffix.starts_with('.'))
}

fn engine_player_slots() -> Vec<(String, SlotRecord)> {
    ["health", "ammo"]
        .into_iter()
        .map(|name| {
            (
                name.to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: None,
                    range: None,
                    persist: false,
                    readonly: true,
                    ownership: SlotOwnership::Engine,
                }),
            )
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("state slot `{name}` is already defined")]
pub(crate) struct SlotInsertError {
    pub(crate) name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SlotRangeError {
    #[error("state slot `{name}` is not declared")]
    UnknownSlot { name: String },
    #[error("state slot `{name}` is not engine-owned; range mutation is engine-only")]
    NotEngineOwned { name: String },
    #[error("state slot `{name}` is not a number slot; only numeric slots carry a range")]
    NotNumeric { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum NamespaceInsertError {
    #[error("state-store namespace must not be empty")]
    InvalidNamespace,
    #[error("state-store slot name must not be empty")]
    InvalidSlotName,
    #[error("state-store namespace `{namespace}` collides with registered namespace `{existing}`")]
    NamespaceCollision { namespace: String, existing: String },
    #[error("state slot `{name}` is already defined")]
    SlotCollision { name: String },
    #[error("state-store namespace `{namespace}` changes an already committed schema")]
    IncompatibleSchema { namespace: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn number_slot(value: f32) -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: SlotType::Number,
            default: Some(SlotValue::Number(value)),
            range: None,
            persist: false,
            readonly: false,
            ownership: SlotOwnership::Mod,
        })
    }

    #[test]
    fn new_registers_engine_player_namespace() {
        let table = SlotTable::new();
        for name in ["player.health", "player.ammo"] {
            let slot = table.get(name).expect("engine slot should exist");
            assert_eq!(slot.schema.slot_type, SlotType::Number);
            assert_eq!(slot.schema.ownership, SlotOwnership::Engine);
            assert!(slot.schema.readonly);
            assert_eq!(slot.schema.default, None);
            assert_eq!(slot.value, None);
        }
    }

    #[test]
    fn namespace_insert_is_atomic_on_slot_collision() {
        let mut table = SlotTable::new();
        table
            .insert("audio.music".to_string(), number_slot(0.5))
            .unwrap();

        let before = table.len();
        let err = table
            .insert_namespace(
                "audio",
                vec![
                    ("master".to_string(), number_slot(1.0)),
                    ("music".to_string(), number_slot(1.0)),
                ],
            )
            .unwrap_err();

        assert_eq!(
            err,
            NamespaceInsertError::SlotCollision {
                name: "audio.music".to_string()
            }
        );
        assert_eq!(table.len(), before);
        assert!(table.get("audio.master").is_none());
    }

    #[test]
    fn namespace_insert_rejects_dotted_prefix_collisions() {
        let mut table = SlotTable::new();
        for namespace in ["player", "player.stats"] {
            let err = table
                .insert_namespace(namespace, vec![("shield".to_string(), number_slot(100.0))])
                .unwrap_err();
            assert!(matches!(
                err,
                NamespaceInsertError::NamespaceCollision { .. }
            ));
        }
        assert!(table.get("player.shield").is_none());
        assert!(table.get("player.stats.shield").is_none());
    }

    #[test]
    fn reconcile_identical_schema_preserves_current_value() {
        let mut table = SlotTable::new();
        let declaration = StoreDeclaration {
            namespace: "audio".to_string(),
            records: vec![("master".to_string(), number_slot(1.0))],
        };
        let mut first = StoreDeclarationSet::default();
        first.add(declaration.clone()).unwrap();
        let plan = table.plan_reconcile(&first).unwrap();
        table.apply_reconcile_plan(plan);
        table.get_mut("audio.master").unwrap().value = Some(SlotValue::Number(0.25));

        let mut repeated = StoreDeclarationSet::default();
        repeated.add(declaration).unwrap();
        let plan = table.plan_reconcile(&repeated).unwrap();
        assert_eq!(plan.added_namespace_count(), 0);
        table.apply_reconcile_plan(plan);

        assert_eq!(
            table
                .get("audio.master")
                .and_then(|slot| slot.value.as_ref()),
            Some(&SlotValue::Number(0.25))
        );
    }

    #[test]
    fn set_engine_numeric_range_installs_range_and_reclamps_current_value() {
        // The `player.health` model: an engine-owned readonly number slot gains
        // its `[0, max]` range only when the producer materializes. Installing
        // the range must re-clamp a value already present (here, above the new
        // max) so the table never holds an out-of-range value.
        let mut table = SlotTable::new();
        table.get_mut("player.health").unwrap().value = Some(SlotValue::Number(150.0));

        table
            .set_engine_numeric_range(
                "player.health",
                NumericRange {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .unwrap();

        let slot = table.get("player.health").unwrap();
        assert_eq!(
            slot.schema.range,
            Some(NumericRange {
                min: 0.0,
                max: 100.0
            })
        );
        assert_eq!(
            slot.value,
            Some(SlotValue::Number(100.0)),
            "current value re-clamps into the freshly-installed range"
        );
    }

    #[test]
    fn set_engine_numeric_range_rejects_non_engine_and_non_numeric_slots() {
        let mut table = SlotTable::new();
        // Mod-owned slot: range mutation is engine-only.
        table
            .insert("audio.master".to_string(), number_slot(0.5))
            .unwrap();
        assert!(matches!(
            table
                .set_engine_numeric_range("audio.master", NumericRange { min: 0.0, max: 1.0 })
                .unwrap_err(),
            SlotRangeError::NotEngineOwned { .. }
        ));
        // Unknown slot.
        assert!(matches!(
            table
                .set_engine_numeric_range("player.missing", NumericRange { min: 0.0, max: 1.0 })
                .unwrap_err(),
            SlotRangeError::UnknownSlot { .. }
        ));
        // Engine-owned but non-numeric slot: range mutation requires a number type.
        table
            .insert(
                "engine.flag".to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Boolean,
                    default: Some(SlotValue::Boolean(false)),
                    range: None,
                    persist: false,
                    readonly: true,
                    ownership: SlotOwnership::Engine,
                }),
            )
            .unwrap();
        assert!(matches!(
            table
                .set_engine_numeric_range("engine.flag", NumericRange { min: 0.0, max: 1.0 })
                .unwrap_err(),
            SlotRangeError::NotNumeric { .. }
        ));
    }

    #[test]
    fn reconcile_rejects_changed_schema_without_partial_commit() {
        let mut table = SlotTable::new();
        table
            .insert_namespace("audio", vec![("master".to_string(), number_slot(1.0))])
            .unwrap();

        let mut declarations = StoreDeclarationSet::default();
        declarations
            .add(StoreDeclaration {
                namespace: "video".to_string(),
                records: vec![("gamma".to_string(), number_slot(1.0))],
            })
            .unwrap();
        declarations
            .add(StoreDeclaration {
                namespace: "audio".to_string(),
                records: vec![
                    ("master".to_string(), number_slot(1.0)),
                    ("music".to_string(), number_slot(0.5)),
                ],
            })
            .unwrap();

        let err = table.plan_reconcile(&declarations).unwrap_err();
        assert_eq!(
            err,
            NamespaceInsertError::IncompatibleSchema {
                namespace: "audio".to_string()
            }
        );
        assert!(table.get("video.gamma").is_none());
    }
}
