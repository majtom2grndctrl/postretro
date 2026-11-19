// Engine family members in the slot table: declared and removed at run time,
// keeping the values of members that stay.
// See: context/lib/scripting.md §5 (Durable State Store, Occupancy)

use std::collections::BTreeSet;

use super::{SlotTable, SlotValue};
use crate::engine_state_collections::EngineStateFamily;

/// What a family replace changed. A peer that adds or removes members must
/// rebuild its replicated schema before the next batch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FamilyMembersChange {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

impl FamilyMembersChange {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

impl SlotTable {
    /// Make `keys` exactly `family`'s members. A kept member keeps its slots
    /// and values; a new one is declared at its fields' defaults; a dropped one
    /// loses its slots. Members register no namespace: mod declarations are
    /// kept off the family's namespace by its reservation instead.
    pub fn replace_family_members(
        &mut self,
        family: &EngineStateFamily<'_>,
        keys: &BTreeSet<String>,
    ) -> FamilyMembersChange {
        let current = self.family_members(family);
        let mut change = FamilyMembersChange::default();
        for key in current.difference(keys) {
            for field in family.fields {
                self.slots.remove(&family.slot_name(key, field.name));
            }
            change.removed.push(key.clone());
        }
        for key in keys.difference(&current) {
            for field in family.fields {
                self.slots.insert(
                    family.slot_name(key, field.name),
                    field.slot_record(family.network),
                );
            }
            change.added.push(key.clone());
        }
        change
    }

    /// Every declared member key of `family`, sorted.
    pub fn family_members(&self, family: &EngineStateFamily<'_>) -> BTreeSet<String> {
        self.slots
            .keys()
            .filter_map(|name| family.split_slot_name(name).map(|(key, _)| key.to_string()))
            .collect()
    }

    /// Write every member field back to its declared default, as a level ends
    /// or installs: no member carries a value across levels.
    pub fn reset_family_values(&mut self, family: &EngineStateFamily<'_>) {
        for key in self.family_members(family) {
            for field in family.fields {
                let Some(slot) = self.slots.get_mut(&family.slot_name(&key, field.name)) else {
                    continue;
                };
                let default: Option<SlotValue> = slot.schema.default.clone();
                if slot.value != default {
                    slot.write_value(default);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_state_collections::OCCUPANCY_FAMILY;
    use crate::slot_table::ReplicationScope;

    fn keys(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| (*name).to_string()).collect()
    }

    fn value(table: &SlotTable, name: &str) -> Option<SlotValue> {
        table.get(name).and_then(|slot| slot.value.clone())
    }

    // P22: a replace keeps the members that stay, with their values.
    #[test]
    fn family_replace_keeps_kept_members_and_their_values() {
        let mut table = SlotTable::new();
        let change = table.replace_family_members(&OCCUPANCY_FAMILY, &keys(&["airlock", "hill"]));
        assert_eq!(change.added, vec!["airlock", "hill"]);
        let slot = table.get("occupancy.hill.occupants").unwrap();
        assert!(slot.schema.readonly);
        assert_eq!(slot.schema.network, ReplicationScope::SharedGlobal);
        assert_eq!(
            value(&table, "occupancy.hill.occupants"),
            Some(SlotValue::Number(0.0))
        );

        table
            .get_mut("occupancy.hill.occupants")
            .unwrap()
            .write_value(Some(SlotValue::Number(2.0)));
        let change = table.replace_family_members(&OCCUPANCY_FAMILY, &keys(&["hill", "arena"]));
        assert_eq!(change.added, vec!["arena"]);
        assert_eq!(change.removed, vec!["airlock"]);
        assert_eq!(
            value(&table, "occupancy.hill.occupants"),
            Some(SlotValue::Number(2.0))
        );
        assert!(table.get("occupancy.airlock.occupiedCount").is_none());
        assert_eq!(
            table.family_members(&OCCUPANCY_FAMILY),
            keys(&["arena", "hill"])
        );

        assert!(
            table
                .replace_family_members(&OCCUPANCY_FAMILY, &keys(&["arena", "hill"]))
                .is_empty()
        );
        table.reset_family_values(&OCCUPANCY_FAMILY);
        assert_eq!(
            value(&table, "occupancy.hill.occupants"),
            Some(SlotValue::Number(0.0))
        );
    }

    fn mod_declaration(namespace: &str) -> crate::slot_table::StoreDeclaration {
        use crate::slot_table::{SlotOwnership, SlotRecord, SlotSchema, SlotType};
        crate::slot_table::StoreDeclaration {
            namespace: namespace.to_string(),
            records: vec![(
                "held".to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(0.0)),
                    range: None,
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: ReplicationScope::None,
                    per_owner: false,
                    accumulate: None,
                }),
            )],
            run_scoped: Default::default(),
        }
    }

    // P5: the mod declaration paths reject `occupancy` and any namespace
    // overlapping it, with or without a level's families declared, while a
    // namespace that merely shares the prefix loads and the engine's own
    // family insert succeeds.
    #[test]
    fn occupancy_namespace_is_reserved_for_the_engine() {
        use crate::slot_table::{NamespaceInsertError, StoreDeclarationSet};
        for with_level_families in [false, true] {
            let mut table = SlotTable::new();
            if with_level_families {
                table.replace_family_members(&OCCUPANCY_FAMILY, &keys(&["hill"]));
            }
            for namespace in ["occupancy", "occupancy.x"] {
                let mut set = StoreDeclarationSet::default();
                assert!(matches!(
                    set.add(mod_declaration(namespace)),
                    Err(NamespaceInsertError::EngineReservedNamespace { .. })
                ));
            }
            let mut set = StoreDeclarationSet::default();
            set.add(mod_declaration("occupancyBoard")).unwrap();
            let plan = table.plan_reconcile(&set).unwrap();
            table.apply_reconcile_plan(plan);
            assert!(table.get("occupancyBoard.held").is_some());
        }
        let mut table = SlotTable::new();
        assert!(
            !table
                .replace_family_members(&OCCUPANCY_FAMILY, &keys(&["hill"]))
                .is_empty(),
            "the engine's own family insert succeeds"
        );
    }
}
