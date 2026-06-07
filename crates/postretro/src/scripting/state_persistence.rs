// State-store persistence encoding, restore lifecycle, and save gating.
// See: context/lib/scripting.md §5 "Durable State Store"

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::slot_table::{SlotOwnership, SlotRecord, SlotTable, SlotType, SlotValue};

/// Current on-disk state format. Increment only with a defined migration path.
pub(crate) const CURRENT_STATE_VERSION: u32 = 1;
pub(crate) const STATE_FILE_PATH: &str = "state.json";

/// Process-lifetime gate for the one-time restore and clean-exit save.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StateStoreLifecycle {
    restore_completed: bool,
}

impl StateStoreLifecycle {
    pub(crate) fn should_restore_after_mod_init(&self, has_manifest: bool) -> bool {
        has_manifest && !self.restore_completed
    }

    pub(crate) fn mark_restore_completed(&mut self) {
        self.restore_completed = true;
    }

    pub(crate) fn can_save(&self) -> bool {
        self.restore_completed
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct PersistedState {
    version: u32,
    slots: BTreeMap<String, PersistedValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
enum PersistedValue {
    Boolean(bool),
    Number(f64),
    String(String),
    Array(Vec<f64>),
    Unsupported(Value),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PersistenceError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub(crate) struct CollectedState {
    pub(crate) state: PersistedState,
    pub(crate) warnings: Vec<String>,
}

/// Build the save document without performing filesystem I/O.
pub(crate) fn collect_persisted_state(table: &SlotTable) -> CollectedState {
    let mut slots = BTreeMap::new();
    let mut warnings = Vec::new();

    for (name, record) in table.iter() {
        if !is_persisted_mod_slot(record) {
            continue;
        }

        let Some(value) = record.value.as_ref() else {
            warnings.push(format!(
                "persistent state slot `{name}` has no current value; omitting it"
            ));
            continue;
        };

        match value_for_save(name, record, value) {
            Ok(value) => {
                slots.insert(name.to_string(), value);
            }
            Err(warning) => warnings.push(warning),
        }
    }

    CollectedState {
        state: PersistedState {
            version: CURRENT_STATE_VERSION,
            slots,
        },
        warnings,
    }
}

/// Overlay a decoded save document onto already-declared slots.
///
/// Invalid entries are left at their current declared/default value and
/// returned as warnings for the caller to log.
pub(crate) fn overlay_persisted_state(
    table: &mut SlotTable,
    persisted: &PersistedState,
) -> Vec<String> {
    if persisted.version != CURRENT_STATE_VERSION {
        return vec![format!(
            "state file version {} is not supported (current version is {}); ignoring file",
            persisted.version, CURRENT_STATE_VERSION
        )];
    }

    let mut warnings = Vec::new();
    for (name, persisted_value) in &persisted.slots {
        let Some(record) = table.get_mut(name) else {
            warnings.push(format!(
                "state file contains unknown slot `{name}`; ignoring it"
            ));
            continue;
        };

        if !record.schema.persist {
            warnings.push(format!(
                "state file targets non-persistent slot `{name}`; ignoring it"
            ));
            continue;
        }
        if record.schema.readonly || record.schema.ownership != SlotOwnership::Mod {
            warnings.push(format!(
                "state file targets readonly or engine-owned slot `{name}`; ignoring it"
            ));
            continue;
        }

        match restored_value(name, record, persisted_value) {
            Ok((value, warning)) => {
                record.value = Some(value);
                if let Some(warning) = warning {
                    warnings.push(warning);
                }
            }
            Err(warning) => warnings.push(warning),
        }
    }
    warnings
}

pub(crate) fn load_persisted_state(
    path: &Path,
) -> Result<Option<PersistedState>, PersistenceError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    Ok(Some(serde_json::from_slice(&bytes)?))
}

pub(crate) fn save_persisted_state(
    path: &Path,
    state: &PersistedState,
) -> Result<(), PersistenceError> {
    let bytes = serde_json::to_vec_pretty(state)?;
    fs::write(path, bytes)?;
    Ok(())
}

fn is_persisted_mod_slot(record: &SlotRecord) -> bool {
    record.schema.persist
        && !record.schema.readonly
        && record.schema.ownership == SlotOwnership::Mod
}

fn value_for_save(
    name: &str,
    record: &SlotRecord,
    value: &SlotValue,
) -> Result<PersistedValue, String> {
    match (&record.schema.slot_type, value) {
        (SlotType::Number, SlotValue::Number(number)) if number.is_finite() => {
            Ok(PersistedValue::Number(f64::from(*number)))
        }
        (SlotType::Boolean, SlotValue::Boolean(boolean)) => Ok(PersistedValue::Boolean(*boolean)),
        (SlotType::String, SlotValue::String(string)) => Ok(PersistedValue::String(string.clone())),
        (SlotType::Enum { values }, SlotValue::Enum(value)) if values.contains(value) => {
            Ok(PersistedValue::String(value.clone()))
        }
        (SlotType::Array, SlotValue::Array(values))
            if values.iter().all(|value| value.is_finite()) =>
        {
            Ok(PersistedValue::Array(
                values.iter().copied().map(f64::from).collect(),
            ))
        }
        _ => Err(format!(
            "persistent state slot `{name}` has an invalid current value; omitting it"
        )),
    }
}

fn restored_value(
    name: &str,
    record: &SlotRecord,
    persisted: &PersistedValue,
) -> Result<(SlotValue, Option<String>), String> {
    let mismatch = || {
        format!("state file value for slot `{name}` does not match its declared type; ignoring it")
    };

    match (&record.schema.slot_type, persisted) {
        (SlotType::Number, PersistedValue::Number(number)) => {
            if !number.is_finite() {
                return Err(format!(
                    "state file value for number slot `{name}` is non-finite; ignoring it"
                ));
            }
            let narrowed = *number as f32;
            if !narrowed.is_finite() {
                return Err(format!(
                    "state file value for number slot `{name}` is outside the supported numeric range; ignoring it"
                ));
            }

            if let Some(range) = record.schema.range {
                let clamped = narrowed.clamp(range.min, range.max);
                let warning = (clamped != narrowed).then(|| {
                    format!(
                        "state file value {narrowed} for slot `{name}` is outside [{}, {}]; clamped to {clamped}",
                        range.min, range.max
                    )
                });
                Ok((SlotValue::Number(clamped), warning))
            } else {
                Ok((SlotValue::Number(narrowed), None))
            }
        }
        (SlotType::Boolean, PersistedValue::Boolean(boolean)) => {
            Ok((SlotValue::Boolean(*boolean), None))
        }
        (SlotType::String, PersistedValue::String(string)) => {
            Ok((SlotValue::String(string.clone()), None))
        }
        (SlotType::Enum { values }, PersistedValue::String(value)) => {
            if values.contains(value) {
                Ok((SlotValue::Enum(value.clone()), None))
            } else {
                Err(format!(
                    "state file enum value `{value}` for slot `{name}` is not declared; ignoring it"
                ))
            }
        }
        (SlotType::Array, PersistedValue::Array(values)) => {
            let mut narrowed = Vec::with_capacity(values.len());
            for value in values {
                let element = *value as f32;
                if !value.is_finite() || !element.is_finite() {
                    return Err(format!(
                        "state file array for slot `{name}` contains a non-finite or unsupported number; ignoring it"
                    ));
                }
                narrowed.push(element);
            }
            Ok((SlotValue::Array(narrowed), None))
        }
        _ => Err(mismatch()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::slot_table::{NumericRange, SlotSchema};
    use tempfile::tempdir;

    #[test]
    fn lifecycle_requires_successful_manifest_and_one_restore_attempt_before_save() {
        let mut lifecycle = StateStoreLifecycle::default();
        assert!(!lifecycle.should_restore_after_mod_init(false));
        assert!(!lifecycle.can_save());

        assert!(lifecycle.should_restore_after_mod_init(true));
        assert!(!lifecycle.can_save());
        lifecycle.mark_restore_completed();

        assert!(lifecycle.can_save());
        assert!(!lifecycle.should_restore_after_mod_init(true));
        assert!(!lifecycle.should_restore_after_mod_init(false));
    }

    fn mod_slot(
        slot_type: SlotType,
        default: SlotValue,
        persist: bool,
        range: Option<NumericRange>,
    ) -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type,
            default: Some(default),
            range,
            persist,
            readonly: false,
            ownership: SlotOwnership::Mod,
        })
    }

    fn declare_fixture(table: &mut SlotTable) {
        table
            .insert_namespace(
                "game",
                vec![
                    (
                        "score".to_string(),
                        mod_slot(
                            SlotType::Number,
                            SlotValue::Number(10.0),
                            true,
                            Some(NumericRange {
                                min: 0.0,
                                max: 100.0,
                            }),
                        ),
                    ),
                    (
                        "mode".to_string(),
                        mod_slot(
                            SlotType::Enum {
                                values: vec!["normal".to_string(), "hard".to_string()],
                            },
                            SlotValue::Enum("normal".to_string()),
                            true,
                            None,
                        ),
                    ),
                    (
                        "enabled".to_string(),
                        mod_slot(SlotType::Boolean, SlotValue::Boolean(false), true, None),
                    ),
                    (
                        "label".to_string(),
                        mod_slot(
                            SlotType::String,
                            SlotValue::String("default".to_string()),
                            true,
                            None,
                        ),
                    ),
                    (
                        "curve".to_string(),
                        mod_slot(
                            SlotType::Array,
                            SlotValue::Array(vec![0.0, 1.0]),
                            true,
                            None,
                        ),
                    ),
                    (
                        "scratch".to_string(),
                        mod_slot(SlotType::Boolean, SlotValue::Boolean(false), false, None),
                    ),
                ],
            )
            .unwrap();
    }

    #[test]
    fn persisted_slots_roundtrip_over_fresh_declarations() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");

        let mut source = SlotTable::new();
        declare_fixture(&mut source);
        source.get_mut("game.score").unwrap().value = Some(SlotValue::Number(42.0));
        source.get_mut("game.mode").unwrap().value = Some(SlotValue::Enum("hard".to_string()));
        source.get_mut("game.enabled").unwrap().value = Some(SlotValue::Boolean(true));
        source.get_mut("game.label").unwrap().value =
            Some(SlotValue::String("continued".to_string()));
        source.get_mut("game.curve").unwrap().value = Some(SlotValue::Array(vec![0.25, 0.75, 1.0]));
        source.get_mut("game.scratch").unwrap().value = Some(SlotValue::Boolean(true));

        let collected = collect_persisted_state(&source);
        assert!(collected.warnings.is_empty());
        assert_eq!(
            collected.state.slots.get("game.mode"),
            Some(&PersistedValue::String("hard".to_string()))
        );
        assert!(!collected.state.slots.contains_key("game.scratch"));
        save_persisted_state(&path, &collected.state).unwrap();

        let mut fresh = SlotTable::new();
        declare_fixture(&mut fresh);
        let loaded = load_persisted_state(&path).unwrap().unwrap();
        assert!(overlay_persisted_state(&mut fresh, &loaded).is_empty());

        assert_eq!(
            fresh.get("game.score").unwrap().value,
            Some(SlotValue::Number(42.0))
        );
        assert_eq!(
            fresh.get("game.mode").unwrap().value,
            Some(SlotValue::Enum("hard".to_string()))
        );
        assert_eq!(
            fresh.get("game.enabled").unwrap().value,
            Some(SlotValue::Boolean(true))
        );
        assert_eq!(
            fresh.get("game.label").unwrap().value,
            Some(SlotValue::String("continued".to_string()))
        );
        assert_eq!(
            fresh.get("game.curve").unwrap().value,
            Some(SlotValue::Array(vec![0.25, 0.75, 1.0]))
        );
        assert_eq!(
            fresh.get("game.scratch").unwrap().value,
            Some(SlotValue::Boolean(false))
        );
    }

    #[test]
    fn overlay_ignores_unknown_mismatched_and_invalid_enum_entries() {
        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        let persisted = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([
                (
                    "game.score".to_string(),
                    PersistedValue::String("many".into()),
                ),
                (
                    "game.mode".to_string(),
                    PersistedValue::String("nightmare".into()),
                ),
                ("missing.value".to_string(), PersistedValue::Number(1.0)),
            ]),
        };

        let warnings = overlay_persisted_state(&mut table, &persisted);
        assert_eq!(warnings.len(), 3);
        assert_eq!(
            table.get("game.score").unwrap().value,
            Some(SlotValue::Number(10.0))
        );
        assert_eq!(
            table.get("game.mode").unwrap().value,
            Some(SlotValue::Enum("normal".to_string()))
        );
    }

    #[test]
    fn overlay_ignores_bad_version_and_non_finite_rust_values() {
        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        let bad_version = PersistedState {
            version: CURRENT_STATE_VERSION + 1,
            slots: BTreeMap::from([("game.score".to_string(), PersistedValue::Number(99.0))]),
        };
        assert_eq!(overlay_persisted_state(&mut table, &bad_version).len(), 1);
        assert_eq!(
            table.get("game.score").unwrap().value,
            Some(SlotValue::Number(10.0))
        );

        let non_finite = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([("game.score".to_string(), PersistedValue::Number(f64::NAN))]),
        };
        assert_eq!(overlay_persisted_state(&mut table, &non_finite).len(), 1);
        assert_eq!(
            table.get("game.score").unwrap().value,
            Some(SlotValue::Number(10.0))
        );

        let non_finite_array = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([(
                "game.curve".to_string(),
                PersistedValue::Array(vec![0.0, f64::INFINITY]),
            )]),
        };
        assert_eq!(
            overlay_persisted_state(&mut table, &non_finite_array).len(),
            1
        );
        assert_eq!(
            table.get("game.curve").unwrap().value,
            Some(SlotValue::Array(vec![0.0, 1.0]))
        );
    }

    #[test]
    fn empty_persist_set_writes_empty_slots_map_and_missing_file_is_ok() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.json");
        let table = SlotTable::new();
        let collected = collect_persisted_state(&table);

        assert!(collected.state.slots.is_empty());
        assert!(load_persisted_state(&path).unwrap().is_none());

        save_persisted_state(&path, &collected.state).unwrap();
        let json: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(json["version"], CURRENT_STATE_VERSION);
        assert_eq!(json["slots"], serde_json::json!({}));
    }

    #[test]
    fn overlay_clamps_finite_out_of_range_numbers() {
        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        let persisted = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([("game.score".to_string(), PersistedValue::Number(500.0))]),
        };

        let warnings = overlay_persisted_state(&mut table, &persisted);
        assert_eq!(warnings.len(), 1);
        assert_eq!(
            table.get("game.score").unwrap().value,
            Some(SlotValue::Number(100.0))
        );
    }

    #[test]
    fn save_and_overlay_reject_readonly_or_non_persistent_targets() {
        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        table
            .insert(
                "game.locked".to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::String,
                    default: Some(SlotValue::String("default".to_string())),
                    range: None,
                    persist: true,
                    readonly: true,
                    ownership: SlotOwnership::Mod,
                }),
            )
            .unwrap();

        let collected = collect_persisted_state(&table);
        assert!(!collected.state.slots.contains_key("game.locked"));
        assert!(!collected.state.slots.contains_key("game.scratch"));

        let persisted = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([
                (
                    "game.locked".to_string(),
                    PersistedValue::String("changed".to_string()),
                ),
                ("game.scratch".to_string(), PersistedValue::Boolean(true)),
                ("player.health".to_string(), PersistedValue::Number(1.0)),
            ]),
        };
        let warnings = overlay_persisted_state(&mut table, &persisted);

        assert_eq!(warnings.len(), 3);
        assert_eq!(
            table.get("game.locked").unwrap().value,
            Some(SlotValue::String("default".to_string()))
        );
        assert_eq!(
            table.get("game.scratch").unwrap().value,
            Some(SlotValue::Boolean(false))
        );
        assert_eq!(table.get("player.health").unwrap().value, None);
    }
}
