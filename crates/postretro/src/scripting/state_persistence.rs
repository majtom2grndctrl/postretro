// State-store persistence encoding, restore lifecycle, and save gating.
// See: context/lib/scripting.md §5 "Durable State Store"

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use postretro_entities::slot_table::{SlotOwnership, SlotRecord, SlotTable, SlotType, SlotValue};
use postretro_scripting_core::store_identity::StoreIdentityLedger;

/// Current on-disk state format. Increment only with an explicit migration or invalidation policy.
pub(crate) const CURRENT_STATE_VERSION: u32 = 2;
const STATE_FILENAME: &str = "state.json";

/// Process-lifetime gate for the one-time restore and clean-exit save.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StateStoreLifecycle {
    restore_completed: bool,
    persistence_disabled: bool,
}

impl StateStoreLifecycle {
    pub(crate) fn should_restore_after_mod_init(&self, has_manifest: bool) -> bool {
        has_manifest && !self.restore_completed && !self.persistence_disabled
    }

    pub(crate) fn mark_restore_completed(&mut self) {
        self.restore_completed = true;
    }

    pub(crate) fn can_save(&self) -> bool {
        self.restore_completed && !self.persistence_disabled
    }

    /// Disable persistence for the remainder of this process. Returns whether
    /// this call changed state so callers can emit the unavailable-path warning
    /// exactly once.
    pub(crate) fn disable_persistence(&mut self) -> bool {
        let was_enabled = !self.persistence_disabled;
        self.persistence_disabled = true;
        was_enabled
    }
}

/// Resolve one mod's state file under the platform data directory. The project
/// name is already the final `postretro` component of `data_dir`; do not add it
/// again here.
pub(crate) fn state_path(mod_id: &str) -> Option<PathBuf> {
    let project_dirs = ProjectDirs::from("", "", "postretro");
    state_path_from_data_dir(project_dirs.as_ref().map(|dirs| dirs.data_dir()), mod_id)
}

fn state_path_from_data_dir(data_dir: Option<&Path>, mod_id: &str) -> Option<PathBuf> {
    data_dir.map(|data_dir| data_dir.join(mod_id).join(STATE_FILENAME))
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

/// Build the save document without performing filesystem I/O. Retained live mod
/// slots absent from the latest committed declarations are not save members.
pub(crate) fn collect_persisted_state(
    table: &SlotTable,
    identity: Option<&StoreIdentityLedger>,
    committed_store_slots: &BTreeSet<String>,
) -> CollectedState {
    let mut slots = BTreeMap::new();
    let mut warnings = Vec::new();

    for (name, record) in table.iter() {
        if !is_persisted_mod_slot(record) {
            continue;
        }
        if !committed_store_slots.contains(name) {
            continue;
        }

        let Some(value) = record.value.as_ref() else {
            warnings.push(format!(
                "persistent state slot `{name}` has no current value; omitting it"
            ));
            continue;
        };

        let Some(durable_key) = identity.and_then(|ledger| ledger.durable_key(name)) else {
            warnings.push(format!(
                "persistent state slot `{name}` has no durable identity; omitting it"
            ));
            continue;
        };

        match value_for_save(name, record, value) {
            Ok(value) => {
                slots.insert(durable_key.to_string(), value);
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
/// returned as warnings for the caller to log. Ledger rows outside current
/// declaration membership cannot target the add-only live table.
pub(crate) fn overlay_persisted_state(
    table: &mut SlotTable,
    persisted: &PersistedState,
    identity: Option<&StoreIdentityLedger>,
    committed_store_slots: &BTreeSet<String>,
) -> Vec<String> {
    if persisted.version != CURRENT_STATE_VERSION {
        return vec![format!(
            "state file version {} is not supported (current version is {}); ignoring file",
            persisted.version, CURRENT_STATE_VERSION
        )];
    }

    let authored_by_key = identity
        .map(|ledger| {
            ledger
                .slots
                .iter()
                .filter(|(name, _)| committed_store_slots.contains(name.as_str()))
                .map(|(name, key)| (key.as_str(), name.as_str()))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();

    let mut warnings = Vec::new();
    for (durable_key, persisted_value) in &persisted.slots {
        let Some(name) = authored_by_key.get(durable_key.as_str()).copied() else {
            warnings.push(format!(
                "state file contains unknown durable key `{durable_key}`; ignoring it"
            ));
            continue;
        };
        let Some(record) = table.get_mut(name) else {
            warnings.push(format!(
                "state file durable key `{durable_key}` targets unavailable slot `{name}`; ignoring it"
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
                record.write_value(Some(value));
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
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = tmp_path_for(path);
    if let Err(error) = fs::write(&tmp_path, bytes) {
        let _ = fs::remove_file(&tmp_path);
        return Err(error.into());
    }
    if let Err(error) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(error.into());
    }
    Ok(())
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
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
    use postretro_entities::slot_table::{NumericRange, ReplicationScope, SlotSchema};
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
            network: ReplicationScope::None,
            accumulate: None,
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

    fn fixture_identity() -> StoreIdentityLedger {
        StoreIdentityLedger {
            version: 1,
            slots: BTreeMap::from([
                ("game.score".to_string(), "k0000000000000001".to_string()),
                ("game.mode".to_string(), "k0000000000000002".to_string()),
                ("game.enabled".to_string(), "k0000000000000003".to_string()),
                ("game.label".to_string(), "k0000000000000004".to_string()),
                ("game.curve".to_string(), "k0000000000000005".to_string()),
            ]),
        }
    }

    fn identity_membership(identity: Option<&StoreIdentityLedger>) -> BTreeSet<String> {
        identity
            .into_iter()
            .flat_map(|identity| identity.slots.keys().cloned())
            .collect()
    }

    #[test]
    fn persisted_slots_roundtrip_over_fresh_declarations() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");

        let mut source = SlotTable::new();
        declare_fixture(&mut source);
        let identity = fixture_identity();
        source.get_mut("game.score").unwrap().value = Some(SlotValue::Number(42.0));
        source.get_mut("game.mode").unwrap().value = Some(SlotValue::Enum("hard".to_string()));
        source.get_mut("game.enabled").unwrap().value = Some(SlotValue::Boolean(true));
        source.get_mut("game.label").unwrap().value =
            Some(SlotValue::String("continued".to_string()));
        source.get_mut("game.curve").unwrap().value = Some(SlotValue::Array(vec![0.25, 0.75, 1.0]));
        source.get_mut("game.scratch").unwrap().value = Some(SlotValue::Boolean(true));

        let membership = identity_membership(Some(&identity));
        let collected = collect_persisted_state(&source, Some(&identity), &membership);
        assert!(collected.warnings.is_empty());
        assert_eq!(
            collected.state.slots.get("k0000000000000002"),
            Some(&PersistedValue::String("hard".to_string()))
        );
        assert!(!collected.state.slots.contains_key("game.scratch"));
        save_persisted_state(&path, &collected.state).unwrap();

        let mut fresh = SlotTable::new();
        declare_fixture(&mut fresh);
        let loaded = load_persisted_state(&path).unwrap().unwrap();
        assert!(
            overlay_persisted_state(&mut fresh, &loaded, Some(&identity), &membership).is_empty()
        );

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
        let identity = fixture_identity();
        let persisted = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([
                (
                    "k0000000000000001".to_string(),
                    PersistedValue::String("many".into()),
                ),
                (
                    "k0000000000000002".to_string(),
                    PersistedValue::String("nightmare".into()),
                ),
                ("kffffffffffffffff".to_string(), PersistedValue::Number(1.0)),
            ]),
        };

        let warnings = overlay_persisted_state(
            &mut table,
            &persisted,
            Some(&identity),
            &identity_membership(Some(&identity)),
        );
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
        let identity = fixture_identity();
        let bad_version = PersistedState {
            version: CURRENT_STATE_VERSION + 1,
            slots: BTreeMap::from([(
                "k0000000000000001".to_string(),
                PersistedValue::Number(99.0),
            )]),
        };
        assert_eq!(
            overlay_persisted_state(
                &mut table,
                &bad_version,
                Some(&identity),
                &identity_membership(Some(&identity)),
            )
            .len(),
            1
        );
        assert_eq!(
            table.get("game.score").unwrap().value,
            Some(SlotValue::Number(10.0))
        );

        let non_finite = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([(
                "k0000000000000001".to_string(),
                PersistedValue::Number(f64::NAN),
            )]),
        };
        assert_eq!(
            overlay_persisted_state(
                &mut table,
                &non_finite,
                Some(&identity),
                &identity_membership(Some(&identity)),
            )
            .len(),
            1
        );
        assert_eq!(
            table.get("game.score").unwrap().value,
            Some(SlotValue::Number(10.0))
        );

        let non_finite_array = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([(
                "k0000000000000005".to_string(),
                PersistedValue::Array(vec![0.0, f64::INFINITY]),
            )]),
        };
        assert_eq!(
            overlay_persisted_state(
                &mut table,
                &non_finite_array,
                Some(&identity),
                &identity_membership(Some(&identity)),
            )
            .len(),
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
        let collected = collect_persisted_state(&table, None, &BTreeSet::new());

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
        let identity = fixture_identity();
        let persisted = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([(
                "k0000000000000001".to_string(),
                PersistedValue::Number(500.0),
            )]),
        };

        let warnings = overlay_persisted_state(
            &mut table,
            &persisted,
            Some(&identity),
            &identity_membership(Some(&identity)),
        );
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
        let mut identity = fixture_identity();
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
                    network: ReplicationScope::None,
                    accumulate: None,
                }),
            )
            .unwrap();

        identity
            .slots
            .insert("game.locked".to_string(), "k0000000000000006".to_string());
        identity
            .slots
            .insert("game.scratch".to_string(), "k0000000000000007".to_string());
        identity
            .slots
            .insert("player.health".to_string(), "k0000000000000008".to_string());
        let membership = identity_membership(Some(&identity));
        let collected = collect_persisted_state(&table, Some(&identity), &membership);
        assert!(!collected.state.slots.contains_key("game.locked"));
        assert!(!collected.state.slots.contains_key("game.scratch"));

        let persisted = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([
                (
                    "k0000000000000006".to_string(),
                    PersistedValue::String("changed".to_string()),
                ),
                (
                    "k0000000000000007".to_string(),
                    PersistedValue::Boolean(true),
                ),
                ("k0000000000000008".to_string(), PersistedValue::Number(1.0)),
            ]),
        };
        let warnings =
            overlay_persisted_state(&mut table, &persisted, Some(&identity), &membership);

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

    #[test]
    fn preserved_durable_key_restores_after_an_authored_slot_rename() {
        let mut before_rename = SlotTable::new();
        before_rename
            .insert_namespace(
                "story",
                vec![(
                    "old_score".to_string(),
                    mod_slot(SlotType::Number, SlotValue::Number(0.0), true, None),
                )],
            )
            .unwrap();
        before_rename.get_mut("story.old_score").unwrap().value = Some(SlotValue::Number(42.0));
        let before_identity = StoreIdentityLedger {
            version: 1,
            slots: BTreeMap::from([(
                "story.old_score".to_string(),
                "k0123456789abcdef".to_string(),
            )]),
        };
        let persisted = collect_persisted_state(
            &before_rename,
            Some(&before_identity),
            &identity_membership(Some(&before_identity)),
        )
        .state;

        let mut after_rename = SlotTable::new();
        after_rename
            .insert_namespace(
                "story",
                vec![(
                    "score".to_string(),
                    mod_slot(SlotType::Number, SlotValue::Number(0.0), true, None),
                )],
            )
            .unwrap();
        let renamed_identity = StoreIdentityLedger {
            version: 1,
            slots: BTreeMap::from([("story.score".to_string(), "k0123456789abcdef".to_string())]),
        };

        assert!(
            overlay_persisted_state(
                &mut after_rename,
                &persisted,
                Some(&renamed_identity),
                &identity_membership(Some(&renamed_identity)),
            )
            .is_empty()
        );
        assert_eq!(
            after_rename.get("story.score").unwrap().value,
            Some(SlotValue::Number(42.0))
        );
    }

    #[test]
    fn deleted_ledger_entry_discards_saved_value_by_its_durable_key() {
        let mut table = SlotTable::new();
        table
            .insert_namespace(
                "story",
                vec![(
                    "score".to_string(),
                    mod_slot(SlotType::Number, SlotValue::Number(0.0), true, None),
                )],
            )
            .unwrap();
        let persisted = PersistedState {
            version: CURRENT_STATE_VERSION,
            slots: BTreeMap::from([(
                "k0123456789abcdef".to_string(),
                PersistedValue::Number(42.0),
            )]),
        };

        let warnings = overlay_persisted_state(
            &mut table,
            &persisted,
            None,
            &BTreeSet::from(["story.score".to_string()]),
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("k0123456789abcdef"));
        assert_eq!(
            table.get("story.score").unwrap().value,
            Some(SlotValue::Number(0.0))
        );
    }

    #[test]
    fn v1_document_is_ignored_after_the_durable_key_format_bump() {
        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        let warnings = overlay_persisted_state(
            &mut table,
            &PersistedState {
                version: 1,
                slots: BTreeMap::from([("game.score".to_string(), PersistedValue::Number(42.0))]),
            },
            Some(&fixture_identity()),
            &identity_membership(Some(&fixture_identity())),
        );

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("version 1"));
        assert_eq!(
            table.get("game.score").unwrap().value,
            Some(SlotValue::Number(10.0))
        );
    }

    #[test]
    fn platform_state_paths_are_per_mod_without_double_nesting_postretro() {
        let data_dir = Path::new("/tmp/postretro-test-data/postretro");
        let first = state_path_from_data_dir(Some(data_dir), "first.mod").unwrap();
        let second = state_path_from_data_dir(Some(data_dir), "second.mod").unwrap();

        assert_ne!(first, second);
        assert_eq!(first, data_dir.join("first.mod/state.json"));
        assert_eq!(
            first
                .components()
                .filter(|component| component.as_os_str() == "postretro")
                .count(),
            1
        );
        assert!(
            state_path_from_data_dir(None, "first.mod").is_none(),
            "an unavailable platform data directory must not fall back to cwd"
        );
    }

    // Regression: the add-only table and retained ledger kept saving a slot
    // removed by the latest successful manifest commit.
    #[test]
    fn committed_declaration_membership_filters_retained_live_slots() {
        let mut table = SlotTable::new();
        table
            .insert_namespace(
                "old",
                vec![(
                    "score".to_string(),
                    mod_slot(SlotType::Number, SlotValue::Number(7.0), true, None),
                )],
            )
            .unwrap();
        table
            .insert_namespace(
                "current",
                vec![(
                    "score".to_string(),
                    mod_slot(SlotType::Number, SlotValue::Number(11.0), true, None),
                )],
            )
            .unwrap();
        let identity = StoreIdentityLedger {
            version: 1,
            slots: BTreeMap::from([
                ("old.score".to_string(), "k0123456789abcdef".to_string()),
                ("current.score".to_string(), "kfedcba9876543210".to_string()),
            ]),
        };
        let current_membership = BTreeSet::from(["current.score".to_string()]);

        let collected = collect_persisted_state(&table, Some(&identity), &current_membership).state;
        assert_eq!(collected.slots.len(), 1);
        assert_eq!(
            collected.slots.get("kfedcba9876543210"),
            Some(&PersistedValue::Number(11.0))
        );
        assert!(!collected.slots.contains_key("k0123456789abcdef"));

        let after_no_start = collect_persisted_state(&table, Some(&identity), &BTreeSet::new());
        assert!(
            after_no_start.state.slots.is_empty(),
            "an empty successful commit must not save through stale identity entries"
        );
    }

    #[test]
    fn save_uses_a_sibling_temp_file_and_retained_snapshot_key() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("state.json");
        fs::write(&path, b"old-state").unwrap();
        let mut table = SlotTable::new();
        declare_fixture(&mut table);
        table.get_mut("game.score").unwrap().value = Some(SlotValue::Number(7.0));
        let snapshot = fixture_identity();
        let mut hand_edited = snapshot.clone();
        hand_edited
            .slots
            .insert("game.score".to_string(), "kffffffffffffffff".to_string());

        let collected = collect_persisted_state(
            &table,
            Some(&snapshot),
            &identity_membership(Some(&snapshot)),
        );
        assert!(collected.state.slots.contains_key("k0000000000000001"));
        assert!(
            !collected
                .state
                .slots
                .contains_key(hand_edited.durable_key("game.score").unwrap())
        );
        save_persisted_state(&path, &collected.state).unwrap();

        assert!(!tmp_path_for(&path).exists());
        let document: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(document["slots"]["k0000000000000001"], 7.0);
    }
}
