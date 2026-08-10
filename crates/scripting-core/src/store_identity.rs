//! Author-owned durable identities for mod-declared state slots.
//!
//! The engine only reads this ledger. Authoring tools use this module's public
//! parser, serializer, and key generator so the on-disk contract has one owner.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use postretro_entities::slot_table::{
    ReplicationScope, SlotOwnership, SlotRecord, SlotTable, StoreDeclarationSet,
};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

/// Filename of a mod's author-owned slot-identity ledger.
pub const IDENTITY_FILE_NAME: &str = "identity.json";
/// The only supported ledger wire version.
pub const IDENTITY_VERSION: u32 = 1;

/// A validated mapping from authored dotted slot names to opaque durable keys.
///
/// The fields are public because the authoring-side mint binary needs to append
/// entries before serializing the result. Use [`StoreIdentityLedger::parse`] to
/// read untrusted JSON; direct construction is intentionally still validated by
/// [`StoreIdentityLedger::validate`] before it can enter an engine snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StoreIdentityLedger {
    pub version: u32,
    pub slots: BTreeMap<String, String>,
}

impl StoreIdentityLedger {
    /// Create an empty v1 ledger suitable for authoring tools to populate.
    pub fn empty() -> Self {
        Self {
            version: IDENTITY_VERSION,
            slots: BTreeMap::new(),
        }
    }

    /// Parse and validate a ledger JSON document.
    ///
    /// The custom deserializer observes every object pair while decoding, so a
    /// duplicate authored name cannot be silently collapsed by a map's
    /// last-write-wins behavior.
    pub fn parse(json: &str) -> Result<Self, StoreIdentityError> {
        serde_json::from_str(json).map_err(|error| StoreIdentityError::Parse {
            reason: error.to_string(),
        })
    }

    /// Serialize the canonical, human-editable ledger form.
    pub fn serialize_pretty(&self) -> Result<String, StoreIdentityError> {
        self.validate()
            .map_err(|reason| StoreIdentityError::Invalid { reason })?;
        serde_json::to_string_pretty(self).map_err(|error| StoreIdentityError::Parse {
            reason: error.to_string(),
        })
    }

    /// Validate version, injectivity, and every durable-key spelling.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != IDENTITY_VERSION {
            return Err(format!(
                "identity ledger version {} is unsupported (expected {})",
                self.version, IDENTITY_VERSION
            ));
        }

        let mut keys = BTreeSet::new();
        for (authored_name, durable_key) in &self.slots {
            if authored_name.is_empty() {
                return Err("identity ledger contains an empty authored slot name".to_string());
            }
            if !is_durable_key(durable_key) {
                return Err(format!(
                    "identity ledger durable key `{durable_key}` for slot `{authored_name}` must match `k[0-9a-f]{{16}}`"
                ));
            }
            if !keys.insert(durable_key) {
                return Err(format!(
                    "identity ledger durable key `{durable_key}` is assigned to more than one authored slot"
                ));
            }
        }
        Ok(())
    }

    /// Return the durable key recorded for an authored dotted slot name.
    pub fn durable_key(&self, authored_name: &str) -> Option<&str> {
        self.slots.get(authored_name).map(String::as_str)
    }

    /// Read and validate `<mod_root>/identity.json`. A missing file is distinct
    /// from an invalid file so the commit gate can allow it for non-durable
    /// declaration attempts only.
    pub fn read_from_mod_root(mod_root: &Path) -> Result<Option<Self>, StoreIdentityError> {
        let path = mod_root.join(IDENTITY_FILE_NAME);
        let json = match fs::read_to_string(&path) {
            Ok(json) => json,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(StoreIdentityError::Io { path, source }),
        };
        Self::parse(&json)
            .map(Some)
            .map_err(|error| StoreIdentityError::Invalid {
                reason: format!("failed to parse `{}`: {error}", path.display()),
            })
    }
}

impl<'de> Deserialize<'de> for StoreIdentityLedger {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct WireLedger {
            version: u32,
            slots: SlotPairs,
        }

        let wire = WireLedger::deserialize(deserializer)?;
        let ledger = Self {
            version: wire.version,
            slots: wire.slots.0.into_iter().collect(),
        };
        ledger.validate().map_err(de::Error::custom)?;
        Ok(ledger)
    }
}

/// Generates one opaque durable key using the operating system random source.
pub fn generate_durable_key() -> Result<String, StoreIdentityError> {
    let mut bytes = [0_u8; 8];
    getrandom::fill(&mut bytes).map_err(|error| StoreIdentityError::Random {
        reason: error.to_string(),
    })?;
    Ok(format!("k{}", hex_lower(&bytes)))
}

/// Returns whether `value` has the durable-key wire grammar.
pub fn is_durable_key(value: &str) -> bool {
    value.len() == 17
        && value.starts_with('k')
        && value
            .as_bytes()
            .iter()
            .skip(1)
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

/// Read failures are path-carrying; syntax and contract failures remain clear
/// enough to surface directly in mod-init diagnostics.
#[derive(Debug, thiserror::Error)]
pub enum StoreIdentityError {
    #[error("failed to read identity ledger `{path}`: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid identity ledger: {reason}")]
    Parse { reason: String },
    #[error("invalid identity ledger: {reason}")]
    Invalid { reason: String },
    #[error("failed to generate a durable identity key: {reason}")]
    Random { reason: String },
}

/// The accepted ledger snapshot and non-fatal authoring diagnostics for one
/// declaration attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedStoreIdentity {
    pub ledger: Option<StoreIdentityLedger>,
    pub warnings: Vec<String>,
}

/// Whether a mod slot has a persistence or replication consumer that survives
/// the process and therefore requires a durable ledger key.
pub fn requires_durable_key(record: &SlotRecord) -> bool {
    record.schema.ownership == SlotOwnership::Mod
        && ((record.schema.persist && !record.schema.readonly)
            || record.schema.network != ReplicationScope::None)
}

/// Authored dotted names belonging to one successfully committed declaration
/// snapshot. The live slot table is add-only, so consumers need this membership
/// separately from the durable ledger to distinguish current declarations from
/// retained in-memory slots.
pub fn declaration_slot_names(declarations: &StoreDeclarationSet) -> BTreeSet<String> {
    declarations
        .iter()
        .flat_map(|declaration| {
            declaration
                .records
                .iter()
                .map(|(slot_name, _)| format!("{}.{}", declaration.namespace, slot_name))
        })
        .collect()
}

/// Read the ledger once and validate it against this declaration attempt.
/// Missing-entry requirements are attempt-scoped. Durable-key changes compare
/// every fresh mapping that still names a live slot against the prior snapshot;
/// omitting the fresh mapping remains the explicit discard gesture.
pub fn read_and_validate_attempt(
    mod_root: &Path,
    declarations: &StoreDeclarationSet,
    live_table: &SlotTable,
    previous_snapshot: Option<&StoreIdentityLedger>,
) -> Result<ValidatedStoreIdentity, StoreIdentityError> {
    let ledger = StoreIdentityLedger::read_from_mod_root(mod_root)?;
    validate_attempt(ledger, declarations, live_table, previous_snapshot)
}

/// Validate a previously read ledger against one declaration attempt.
pub fn validate_attempt(
    ledger: Option<StoreIdentityLedger>,
    declarations: &StoreDeclarationSet,
    live_table: &SlotTable,
    previous_snapshot: Option<&StoreIdentityLedger>,
) -> Result<ValidatedStoreIdentity, StoreIdentityError> {
    let declared_names = declaration_slot_names(declarations);
    let mut durable_names = Vec::new();
    let mut durable_name_set = BTreeSet::new();
    for declaration in declarations.iter() {
        for (slot_name, record) in &declaration.records {
            let name = format!("{}.{}", declaration.namespace, slot_name);
            if requires_durable_key(record) {
                durable_names.push(name.clone());
                durable_name_set.insert(name.clone());
            }
        }
    }

    if ledger.is_none() && !durable_names.is_empty() {
        let name = &durable_names[0];
        let fresh_key = generate_durable_key()?;
        return Err(StoreIdentityError::Invalid {
            reason: format!(
                "identity ledger is missing durable entry for state slot `{name}`; add `\"{name}\": \"{fresh_key}\"` to `{IDENTITY_FILE_NAME}`"
            ),
        });
    }

    let mut warnings = Vec::new();
    if let Some(ledger) = ledger.as_ref() {
        if let Some(previous_snapshot) = previous_snapshot {
            for (name, old_key) in &previous_snapshot.slots {
                let Some(new_key) = ledger.durable_key(name) else {
                    continue;
                };
                if live_table.get(name).is_some() && old_key != new_key {
                    return Err(StoreIdentityError::Invalid {
                        reason: format!(
                            "identity ledger changes durable key for already committed state slot `{name}`"
                        ),
                    });
                }
            }
        }

        for name in &durable_names {
            if ledger.durable_key(name).is_none() {
                let fresh_key = generate_durable_key()?;
                return Err(StoreIdentityError::Invalid {
                    reason: format!(
                        "identity ledger is missing durable entry for state slot `{name}`; add `\"{name}\": \"{fresh_key}\"` to `{IDENTITY_FILE_NAME}`"
                    ),
                });
            }
        }

        for name in ledger.slots.keys() {
            if durable_name_set.contains(name) {
                continue;
            }

            if declared_names.contains(name) {
                warnings.push(format!(
                    "identity ledger entry for non-durable state slot `{name}` is retained"
                ));
            } else {
                warnings.push(format!(
                    "identity ledger entry for undeclared state slot `{name}` is retained"
                ));
            }
        }
    }

    Ok(ValidatedStoreIdentity { ledger, warnings })
}

/// `slots` needs a custom map visitor: deserializing straight into a map would
/// discard duplicate JSON object keys before validation sees them.
struct SlotPairs(Vec<(String, String)>);

impl<'de> Deserialize<'de> for SlotPairs {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct SlotPairsVisitor;

        impl<'de> Visitor<'de> for SlotPairsVisitor {
            type Value = SlotPairs;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object mapping authored slot names to durable keys")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut entries = Vec::new();
                let mut names = BTreeSet::new();
                while let Some((authored_name, durable_key)) = map.next_entry::<String, String>()? {
                    if !names.insert(authored_name.clone()) {
                        return Err(de::Error::custom(format!(
                            "identity ledger contains duplicate authored slot `{authored_name}`"
                        )));
                    }
                    entries.push((authored_name, durable_key));
                }
                Ok(SlotPairs(entries))
            }
        }

        deserializer.deserialize_map(SlotPairsVisitor)
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::slot_table::{SlotSchema, SlotType, SlotValue, StoreDeclaration};

    fn durable_record() -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: SlotType::Number,
            default: Some(SlotValue::Number(0.0)),
            range: None,
            persist: true,
            readonly: false,
            ownership: SlotOwnership::Mod,
            network: ReplicationScope::None,
            accumulate: None,
        })
    }

    fn durable_declaration(name: &str) -> StoreDeclarationSet {
        let mut declarations = StoreDeclarationSet::default();
        declarations
            .add(StoreDeclaration {
                namespace: "story".to_string(),
                records: vec![(name.to_string(), durable_record())],
            })
            .unwrap();
        declarations
    }

    #[test]
    fn parser_rejects_duplicate_authored_slot_before_map_collapse() {
        let error = StoreIdentityLedger::parse(
            r#"{"version":1,"slots":{"story.score":"k0123456789abcdef","story.score":"kfedcba9876543210"}}"#,
        )
        .expect_err("duplicate JSON object keys must reject");

        assert!(
            error
                .to_string()
                .contains("duplicate authored slot `story.score`")
        );
    }

    #[test]
    fn parser_rejects_duplicate_keys_bad_versions_and_bad_key_grammar() {
        for json in [
            r#"{"version":1,"slots":{"story.score":"k0123456789abcdef","story.mode":"k0123456789abcdef"}}"#,
            r#"{"version":2,"slots":{}}"#,
            r#"{"version":1,"slots":{"story.score":"K0123456789abcdef"}}"#,
        ] {
            assert!(StoreIdentityLedger::parse(json).is_err(), "{json}");
        }
    }

    #[test]
    fn serializer_roundtrips_canonical_ledger() {
        let ledger = StoreIdentityLedger {
            version: IDENTITY_VERSION,
            slots: BTreeMap::from([("story.score".to_string(), "k0123456789abcdef".to_string())]),
        };

        assert_eq!(
            StoreIdentityLedger::parse(&ledger.serialize_pretty().unwrap()).unwrap(),
            ledger
        );
    }

    #[test]
    fn generated_key_uses_the_pinned_wire_grammar() {
        assert!(is_durable_key(&generate_durable_key().unwrap()));
    }

    #[test]
    fn absent_ledger_is_legal_only_without_durable_declarations() {
        let table = SlotTable::new();
        let empty = StoreDeclarationSet::default();
        assert!(validate_attempt(None, &empty, &table, None).is_ok());

        let error = validate_attempt(None, &durable_declaration("score"), &table, None)
            .expect_err("a durable declaration requires a ledger entry");
        assert!(error.to_string().contains("story.score"));
        assert!(error.to_string().contains("add `\"story.score\": \"k"));
    }

    #[test]
    fn gate_is_attempt_scoped_and_retains_orphan_entries_as_warnings() {
        let mut table = SlotTable::new();
        table
            .insert_namespace("old", vec![("score".to_string(), durable_record())])
            .unwrap();
        let empty = StoreDeclarationSet::default();
        let ledger = StoreIdentityLedger {
            version: IDENTITY_VERSION,
            slots: BTreeMap::from([("old.score".to_string(), "k0123456789abcdef".to_string())]),
        };

        let validated = validate_attempt(Some(ledger), &empty, &table, None)
            .expect("a live-but-undeclared durable slot must not gate a new attempt");
        assert_eq!(validated.warnings.len(), 1);
        assert!(validated.warnings[0].contains("old.score"));
    }

    #[test]
    fn gate_warns_for_declared_slots_that_no_longer_need_durable_identity() {
        let mut declarations = StoreDeclarationSet::default();
        let mut non_durable = durable_record();
        non_durable.schema.persist = false;
        declarations
            .add(StoreDeclaration {
                namespace: "story".to_string(),
                records: vec![("temporary".to_string(), non_durable)],
            })
            .unwrap();
        let ledger = StoreIdentityLedger {
            version: IDENTITY_VERSION,
            slots: BTreeMap::from([
                (
                    "story.temporary".to_string(),
                    "k0123456789abcdef".to_string(),
                ),
                ("old.score".to_string(), "kfedcba9876543210".to_string()),
            ]),
        };

        let validated = validate_attempt(Some(ledger), &declarations, &SlotTable::new(), None)
            .expect("orphan and stale durable entries must be warnings, not rejection");
        assert_eq!(validated.warnings.len(), 2);
        assert!(
            validated
                .warnings
                .iter()
                .any(|warning| warning.contains("non-durable state slot `story.temporary`"))
        );
        assert!(
            validated
                .warnings
                .iter()
                .any(|warning| warning.contains("undeclared state slot `old.score`"))
        );
    }

    #[test]
    fn durable_key_changes_for_live_attempt_slots_are_rejected() {
        let declarations = durable_declaration("score");
        let mut table = SlotTable::new();
        table
            .insert_namespace("story", vec![("score".to_string(), durable_record())])
            .unwrap();
        let previous = StoreIdentityLedger {
            version: IDENTITY_VERSION,
            slots: BTreeMap::from([("story.score".to_string(), "k0123456789abcdef".to_string())]),
        };
        let changed = StoreIdentityLedger {
            version: IDENTITY_VERSION,
            slots: BTreeMap::from([("story.score".to_string(), "kfedcba9876543210".to_string())]),
        };

        assert!(validate_attempt(Some(changed), &declarations, &table, Some(&previous)).is_err());
    }

    // Regression: removing a declaration let a changed ledger key replace the
    // snapshot while the add-only table still held data under that authored name.
    #[test]
    fn durable_key_changes_for_live_undeclared_slots_are_rejected() {
        let mut table = SlotTable::new();
        table
            .insert_namespace("story", vec![("score".to_string(), durable_record())])
            .unwrap();
        let previous = StoreIdentityLedger {
            version: IDENTITY_VERSION,
            slots: BTreeMap::from([("story.score".to_string(), "k0123456789abcdef".to_string())]),
        };
        let changed = StoreIdentityLedger {
            version: IDENTITY_VERSION,
            slots: BTreeMap::from([("story.score".to_string(), "kfedcba9876543210".to_string())]),
        };

        let error = validate_attempt(
            Some(changed),
            &StoreDeclarationSet::default(),
            &table,
            Some(&previous),
        )
        .expect_err("a fresh mapping cannot re-key a live slot omitted by the attempt");
        assert!(
            error
                .to_string()
                .contains("changes durable key for already committed state slot `story.score`")
        );
    }

    #[test]
    fn omitted_fresh_mapping_explicitly_discards_live_slot_identity() {
        let mut table = SlotTable::new();
        table
            .insert_namespace("story", vec![("score".to_string(), durable_record())])
            .unwrap();
        let previous = StoreIdentityLedger {
            version: IDENTITY_VERSION,
            slots: BTreeMap::from([("story.score".to_string(), "k0123456789abcdef".to_string())]),
        };

        let validated = validate_attempt(
            None,
            &StoreDeclarationSet::default(),
            &table,
            Some(&previous),
        )
        .expect("an absent fresh mapping is the explicit discard gesture");
        assert!(validated.ledger.is_none());
    }
}
