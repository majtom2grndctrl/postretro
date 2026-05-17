// Data-script registries: per-level reactions from setupLevel() and
// engine-global entity types drained from setupMod()'s `entities` return field.
// See: context/lib/scripting.md §2 (Data context lifecycle)
//
// Held inside `ScriptCtx` (not directly on `App`) so primitive closures can
// access it via the same captured handle they use for the entity registry.

use super::data_descriptors::{EntityTypeDescriptor, LevelManifest, NamedReaction};

/// Data registries collected from data-context script execution.
/// `reactions` are per-level and cleared on unload; `entities` are
/// engine-global (populated via the mod-init path) and survive level unload.
#[derive(Debug, Default)]
pub(crate) struct DataRegistry {
    /// Reactions registered for this level. Each entry pairs an event name
    /// with the descriptor body the script supplied.
    pub(crate) reactions: Vec<NamedReaction>,
    /// Entity-type descriptors. Engine-global — survive level unload.
    /// Populated by the boot caller after `run_mod_init`: it drains the
    /// `entities` field of the validated `setupMod()` return value into here
    /// via [`Self::upsert_entity_type`]. Read by the data-archetype spawn
    /// sweep. Not populated from `setupLevel()`.
    pub(crate) entities: Vec<EntityTypeDescriptor>,
}

impl DataRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Append a manifest's reactions. Existing reactions are preserved — call
    /// [`Self::clear`] first for a fresh population. Entity-type descriptors
    /// arrive separately via `setupMod()`'s `entities` return field (they
    /// outlive level unload).
    pub(crate) fn populate_from_manifest(&mut self, manifest: LevelManifest) {
        let LevelManifest { reactions } = manifest;
        self.reactions.extend(reactions);
    }

    /// Insert (or overwrite) an entity-type descriptor. Identical re-inserts
    /// keyed on `canonical_name` are silent no-ops; differing re-inserts
    /// overwrite and log at `debug!`. Descriptors with `canonical_name = None`
    /// are always appended — they have no addressable name to dedup against.
    /// Survives level unload — only invoke from the mod-init path (after
    /// `setupMod` returns), not during per-level data-script execution.
    pub(crate) fn upsert_entity_type(&mut self, descriptor: EntityTypeDescriptor) {
        let descriptor_name = descriptor.canonical_name.clone();
        if let Some(name) = descriptor_name.as_deref() {
            if let Some(existing) = self
                .entities
                .iter_mut()
                .find(|e| e.canonical_name.as_deref() == Some(name))
            {
                if *existing == descriptor {
                    return;
                }
                log::debug!(
                    "[Loader] upsert_entity_type: overwriting existing descriptor for `{}`",
                    name,
                );
                *existing = descriptor;
                return;
            }
        }
        self.entities.push(descriptor);
    }

    /// Drop every registered reaction. `entities` outlives the clear
    /// (engine-global; set via the mod-init path); only `reactions` are
    /// per-level and wiped here. Called on level unload.
    /// See [`Self::upsert_entity_type`].
    pub(crate) fn clear(&mut self) {
        self.reactions.clear();
    }

    /// Returns `true` only when both collections are empty. Note: `clear()` only
    /// clears reactions — a freshly-cleared registry with registered entity types
    /// will return `false`. This is intentional: entity types outlive levels.
    ///
    /// Marked `#[cfg(test)]` because production code should not gate logic on
    /// this — after a level unload `reactions` is empty but `entities` is not.
    /// Production code must not use this as a readiness gate — it correctly
    /// returns `false` after level unload while entity types remain.
    #[cfg(test)]
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
        }
    }

    fn grunt_descriptor() -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some("grunt".to_string()),
            light: None,
            emitter: None,
            movement: None,
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
        assert!(!r.is_empty());
    }

    #[test]
    fn clear_drops_reactions_but_keeps_entity_descriptors() {
        let mut r = DataRegistry::new();
        r.populate_from_manifest(sample_manifest());
        r.upsert_entity_type(grunt_descriptor());
        r.clear();
        assert_eq!(r.reactions.len(), 0);
        assert_eq!(r.entities.len(), 1, "entities survive level unload");
    }

    #[test]
    fn upsert_entity_type_inserts_new_descriptor() {
        let mut r = DataRegistry::new();
        r.upsert_entity_type(grunt_descriptor());
        assert_eq!(r.entities.len(), 1);
        assert_eq!(r.entities[0].canonical_name.as_deref(), Some("grunt"));
    }

    #[test]
    fn upsert_entity_type_replays_identical_descriptor_silently() {
        let mut r = DataRegistry::new();
        r.upsert_entity_type(grunt_descriptor());
        r.upsert_entity_type(grunt_descriptor());
        assert_eq!(r.entities.len(), 1);
    }

    #[test]
    fn upsert_entity_type_overwrites_when_different() {
        let mut r = DataRegistry::new();
        r.upsert_entity_type(grunt_descriptor());
        let mut next = grunt_descriptor();
        next.light = Some(crate::scripting::data_descriptors::LightDescriptor {
            color: [1.0, 0.0, 0.0],
            intensity: 1.0,
            range: 5.0,
            is_dynamic: true,
        });
        r.upsert_entity_type(next.clone());
        assert_eq!(r.entities.len(), 1);
        assert_eq!(r.entities[0], next);
    }
}
