// The `occupancy` family members the scripts reference, collected from the
// descriptors they evaluated to, never from source text.
// See: context/lib/scripting.md §5 (Occupancy)

use std::collections::BTreeSet;

use postretro_entities::engine_state_collections::{EngineStateFamily, OCCUPANCY_FAMILY};
use postretro_foundation::IrNode;

use crate::data_descriptors::{CrossingCondition, CrossingDescriptor, ReactionDescriptor};
use crate::slot_table::SlotTable;
use crate::ui::descriptor::AnchoredTree;

/// Referenced `occupancy` tags, split into the ones that can form member slot
/// names and the ones that cannot (`.` or `:` in the tag).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FamilyReferences {
    pub keys: BTreeSet<String>,
    pub unformable: BTreeSet<String>,
}

impl FamilyReferences {
    fn family() -> &'static EngineStateFamily<'static> {
        &OCCUPANCY_FAMILY
    }

    /// Note one slot name; anything outside the family is ignored.
    pub fn note_slot(&mut self, name: &str) {
        let Some((key, _)) = Self::family().split_slot_name(name) else {
            return;
        };
        if EngineStateFamily::is_slot_safe_key(key) {
            self.keys.insert(key.to_string());
        } else {
            self.unformable.insert(key.to_string());
        }
    }

    pub fn note_ir(&mut self, ir: &IrNode) {
        ir.for_each_input(&mut |name, _| self.note_slot(name));
    }

    /// Every string in a lowered JSON value: reaction args carry slot names
    /// as IR input leaves, `updateState` targets and the like.
    pub fn note_json(&mut self, value: &serde_json::Value) {
        match value {
            serde_json::Value::String(text) => self.note_slot(text),
            serde_json::Value::Array(items) => items.iter().for_each(|item| self.note_json(item)),
            serde_json::Value::Object(map) => map.values().for_each(|item| self.note_json(item)),
            _ => {}
        }
    }

    pub fn note_crossing(&mut self, crossing: &CrossingDescriptor) {
        if let Some(slot) = &crossing.slot {
            self.note_slot(slot);
        }
        if let CrossingCondition::Ir(ir) = &crossing.condition {
            self.note_ir(ir);
        }
    }

    pub fn note_reaction(&mut self, reaction: &ReactionDescriptor) {
        match reaction {
            ReactionDescriptor::Progress(_) => {}
            ReactionDescriptor::Primitive(primitive) => self.note_json(&primitive.args),
            ReactionDescriptor::Sequence(steps) => {
                steps.iter().for_each(|step| self.note_json(&step.args));
            }
        }
    }

    /// A UI tree's binds and predicates, read from its wire form.
    pub fn note_ui_tree(&mut self, tree: &AnchoredTree) {
        if let Ok(value) = serde_json::to_value(tree) {
            self.note_json(&value);
        }
    }

    /// Every store accumulator in the table. Stores are declared only by the
    /// mod, so these are mod-scope references.
    pub fn note_accumulators(&mut self, slot_table: &SlotTable) {
        for (_, record) in slot_table.iter() {
            if let Some(ir) = &record.schema.accumulate {
                self.note_ir(ir);
            }
        }
    }

    /// Warn once for each tag that cannot form a slot name. It gets no family.
    pub fn warn_unformable(&self, scope: &str) {
        for tag in &self.unformable {
            log::warn!(
                "[Scripting] {scope} references occupancy tag `{tag}`, which cannot form a state slot name (it is empty or contains `.` or `:`); it gets no occupancy family"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_descriptors::{NamedReaction, PrimitiveDescriptor};

    #[test]
    fn references_collect_from_crossings_reactions_ir_and_ui_binds() {
        let mut refs = FamilyReferences::default();
        refs.note_crossing(&CrossingDescriptor {
            slot: Some("occupancy.vault-plates.occupiedCount".to_string()),
            condition: CrossingCondition::Above { threshold: 1.0 },
            max: 1.0,
            edge: None,
            fire: Vec::new(),
        });
        refs.note_crossing(&CrossingDescriptor {
            slot: None,
            condition: CrossingCondition::Ir(IrNode::Input {
                name: "occupancy.airlock.occupants".to_string(),
                owner: None,
            }),
            max: 1.0,
            edge: None,
            fire: Vec::new(),
        });
        let reaction = NamedReaction {
            name: "r".to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "setState".to_string(),
                target: None,
                kind: None,
                tag: None,
                on_complete: None,
                args: serde_json::json!({
                    "value": { "op": "input", "name": "occupancy.hill.occupants" },
                    "note": "occupancy.a.b.occupants",
                }),
            }),
        };
        refs.note_reaction(&reaction.descriptor);
        refs.note_slot("occupancy.bad:tag.occupants");
        refs.note_slot("occupancy..occupants");
        refs.note_slot("occupancyBoard.x.occupants");
        refs.note_slot("occupancy.hill.held");
        assert_eq!(
            refs.keys,
            ["airlock", "hill", "vault-plates"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
        assert_eq!(
            refs.unformable,
            ["", "a.b", "bad:tag"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
    }
}
