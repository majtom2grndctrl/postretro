// Data-script registries: per-level reactions, engine-global entity types, and
// hot-reload descriptor replacement via replace_entity_types().
// See: context/lib/scripting.md §2 (Data context lifecycle)
//
// Held inside `ScriptCtx` (not directly on `App`) so primitive closures can
// access it via the same captured handle they use for the entity registry.

use std::collections::HashMap;

use super::data_descriptors::{
    CrossingDescriptor, EntityTypeDescriptor, LevelManifest, NamedReaction,
};

/// Data registries collected from data-context script execution.
/// `reactions` are per-level and cleared on unload; `entities` are
/// engine-global (populated via the mod-init path) and survive level unload.
#[derive(Debug, Default)]
pub(crate) struct DataRegistry {
    /// Reactions registered for this level. Each entry pairs an event name
    /// with the descriptor body the script supplied.
    pub(crate) reactions: Vec<NamedReaction>,
    /// State-crossing watchers registered for this level (M13 HUD dynamics).
    /// Per-level — cleared on unload with `reactions`. The crossing detector
    /// reads these to know which slots to watch and what to fire on a crossing.
    pub(crate) crossings: Vec<CrossingDescriptor>,
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
        let LevelManifest {
            reactions,
            crossings,
            // Level-scope UI trees are drained off the manifest and registered
            // into the app-side `UiTreeRegistry` at the level-load drain point
            // (main.rs), before this `populate_from_manifest` consumes the rest.
            // They are not engine-global data-registry state, so nothing lands
            // here. G1b Task 3.
            ui_trees: _,
        } = manifest;
        self.reactions.extend(reactions);
        self.crossings.extend(crossings);
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

    /// Dedup a complete entity descriptor snapshot before hot-reload commit,
    /// keeping the LAST occurrence per `canonical_name` so the result matches
    /// startup's `upsert_entity_type` last-write-wins (where a descriptor
    /// spread later in `setupMod`'s `entities` array overwrites an earlier one
    /// with the same name). Each collision logs at `warn!`. Descriptors with
    /// no `canonical_name` pass through untouched. Surviving entries keep their
    /// last-appearance order.
    pub(crate) fn dedup_entity_type_snapshot(
        descriptors: Vec<EntityTypeDescriptor>,
    ) -> Vec<EntityTypeDescriptor> {
        // First pass records the last index each name appears at; second pass
        // keeps only that occurrence, preserving last-appearance order.
        let mut last_index: HashMap<String, usize> = HashMap::new();
        for (index, descriptor) in descriptors.iter().enumerate() {
            if let Some(name) = descriptor.canonical_name.as_deref() {
                if last_index.insert(name.to_string(), index).is_some() {
                    log::warn!(
                        "[Loader] duplicate entity descriptor canonicalName `{name}` in replacement snapshot; later declaration wins"
                    );
                }
            }
        }
        descriptors
            .into_iter()
            .enumerate()
            .filter(
                |(index, descriptor)| match descriptor.canonical_name.as_deref() {
                    Some(name) => last_index.get(name) == Some(index),
                    None => true,
                },
            )
            .map(|(_, descriptor)| descriptor)
            .collect()
    }

    /// Replace the engine-global descriptor snapshot as one complete commit.
    /// Used by dev hot reload after a staged manifest has validated and its
    /// live refresh plan has applied. Startup may continue to use upsert.
    /// Duplicate `canonical_name`s are deduped last-write-wins to match
    /// startup, so this is infallible.
    pub(crate) fn replace_entity_types(&mut self, descriptors: Vec<EntityTypeDescriptor>) {
        self.entities = Self::dedup_entity_type_snapshot(descriptors);
    }

    /// Drop every registered reaction. `entities` outlives the clear
    /// (engine-global; set via the mod-init path); only `reactions` are
    /// per-level and wiped here. Called on level unload.
    /// See [`Self::upsert_entity_type`].
    pub(crate) fn clear(&mut self) {
        self.reactions.clear();
        self.crossings.clear();
    }

    /// Returns `true` only when both collections are empty. After level unload,
    /// `entities` is still populated, so this returns `false` — production code
    /// should use `reactions.is_empty()` for level-unload checks.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.reactions.is_empty() && self.crossings.is_empty() && self.entities.is_empty()
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
                    tag: Some("reactorChambers".to_string()),
                    on_complete: None,
                    args: serde_json::Value::Object(Default::default()),
                }),
            }],
            crossings: Vec::new(),
            ui_trees: Vec::new(),
        }
    }

    fn grunt_descriptor() -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some("grunt".to_string()),
            default_weapon: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            mesh: None,
            health: None,
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

    #[test]
    fn replace_entity_types_removes_absent_descriptors() {
        let mut r = DataRegistry::new();
        r.upsert_entity_type(grunt_descriptor());
        let mut replacement = grunt_descriptor();
        replacement.canonical_name = Some("enforcer".to_string());

        r.replace_entity_types(vec![replacement.clone()]);

        assert_eq!(r.entities, vec![replacement]);
    }

    #[test]
    fn replace_entity_types_dedups_duplicate_canonical_names_last_wins() {
        let mut r = DataRegistry::new();
        r.upsert_entity_type(grunt_descriptor());
        let earlier = grunt_descriptor();
        let mut later = grunt_descriptor();
        later.light = Some(crate::scripting::data_descriptors::LightDescriptor {
            color: [1.0, 0.0, 0.0],
            intensity: 1.0,
            range: 5.0,
            is_dynamic: true,
        });

        r.replace_entity_types(vec![earlier, later.clone()]);

        assert_eq!(
            r.entities,
            vec![later],
            "later declaration wins on collision"
        );
    }

    #[test]
    fn dedup_entity_type_snapshot_keeps_last_occurrence_and_passes_through_unnamed() {
        let earlier = grunt_descriptor();
        let mut later = grunt_descriptor();
        later.light = Some(crate::scripting::data_descriptors::LightDescriptor {
            color: [0.0, 1.0, 0.0],
            intensity: 2.0,
            range: 8.0,
            is_dynamic: false,
        });
        let mut unnamed = grunt_descriptor();
        unnamed.canonical_name = None;

        let deduped =
            DataRegistry::dedup_entity_type_snapshot(vec![earlier, unnamed.clone(), later.clone()]);

        // The named collision collapses to its last occurrence (last-appearance
        // order), and the unnamed descriptor passes through untouched.
        assert_eq!(deduped, vec![unnamed, later]);
    }
}
