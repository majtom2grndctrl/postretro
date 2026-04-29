// Data-script registries: reactions and entity types collected from
// `registerLevelManifest()` at level load.
// See: context/lib/scripting.md §2 (Data context lifecycle)
//
// Lives separate from the behavior `HandlerTable` so a hot reload of the
// behavior surface (which clears handlers) does not invalidate descriptor
// state, and vice versa.

use super::data_descriptors::{EntityTypeDescriptor, LevelManifest, NamedReaction};

/// Registries populated from a level's `registerLevelManifest()` return
/// bundle. Both vectors preserve registration order.
#[derive(Debug, Default)]
pub(crate) struct DataRegistry {
    /// Reactions registered for this level. Each entry pairs an event name
    /// with the descriptor body the script supplied.
    pub(crate) reactions: Vec<NamedReaction>,
    /// Entity-type descriptors registered for this level, keyed informally
    /// by `classname`.
    pub(crate) entities: Vec<EntityTypeDescriptor>,
}

impl DataRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Append a manifest's reactions and entities. Existing entries are
    /// preserved — call [`Self::clear`] first for a fresh population.
    pub(crate) fn populate_from_manifest(&mut self, manifest: LevelManifest) {
        let LevelManifest {
            reactions,
            entities,
        } = manifest;
        self.reactions.extend(reactions);
        self.entities.extend(entities);
    }

    /// Drop every registered reaction and entity type. Called on level
    /// unload and during hot-reload of the data script.
    pub(crate) fn clear(&mut self) {
        self.reactions.clear();
        self.entities.clear();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.reactions.is_empty() && self.entities.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::data_descriptors::{
        EntityTypeDescriptor, NamedReaction, PrimitiveDescriptor, ReactionDescriptor,
    };

    fn sample_manifest() -> LevelManifest {
        LevelManifest {
            reactions: vec![NamedReaction {
                name: "wave1Complete".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "moveGeometry".to_string(),
                    tag: "reactorChambers".to_string(),
                    on_complete: None,
                    args: serde_json::Value::Object(Default::default()),
                }),
            }],
            entities: vec![EntityTypeDescriptor {
                classname: "grunt".to_string(),
            }],
        }
    }

    #[test]
    fn new_registry_is_empty() {
        let r = DataRegistry::new();
        assert!(r.is_empty());
    }

    #[test]
    fn populate_appends_manifest_entries() {
        let mut r = DataRegistry::new();
        r.populate_from_manifest(sample_manifest());
        assert_eq!(r.reactions.len(), 1);
        assert_eq!(r.entities.len(), 1);
        assert!(!r.is_empty());
    }

    #[test]
    fn clear_leaves_registry_empty() {
        let mut r = DataRegistry::new();
        r.populate_from_manifest(sample_manifest());
        r.clear();
        assert!(r.is_empty());
        assert_eq!(r.reactions.len(), 0);
        assert_eq!(r.entities.len(), 0);
    }
}
