// Data-script registries: per-level reactions, engine-global entity types, and
// hot-reload descriptor replacement via replace_entity_types().
// See: context/lib/scripting.md §2 (Data context lifecycle)
//
// Held inside `ScriptCtx` (not directly on `App`) so primitive closures can
// access it via the same captured handle they use for the entity registry.

use std::collections::{HashMap, HashSet};

use super::data_descriptors::{
    CrossingDescriptor, EntityTypeDescriptor, LevelManifest, NamedReaction,
};
use super::runtime::ModMapEntry;

/// Engine-global reaction definition plus its optional level-tag scope.
/// Empty `levels` means all levels; activation/composition happens separately.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScopedReaction {
    pub(crate) reaction: NamedReaction,
    pub(crate) levels: Vec<String>,
}

/// Engine-global state-crossing definition plus its optional level-tag scope.
/// Empty `levels` means all levels; activation/composition happens separately.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScopedCrossing {
    pub(crate) crossing: CrossingDescriptor,
    pub(crate) levels: Vec<String>,
}

/// Data registries collected from script execution.
/// `reactions` and `crossings` are per-level and cleared on unload; entity,
/// map, and global reaction/crossing definitions survive level unload.
#[derive(Debug, Default)]
pub(crate) struct DataRegistry {
    /// Active reactions for this level after composing matching mod-global
    /// definitions with level-local definitions. Existing dispatch reads here.
    pub(crate) reactions: Vec<NamedReaction>,
    /// Active state-crossing watchers for this level after composition (M13 HUD
    /// dynamics). Per-level — cleared on unload with `reactions`. The crossing
    /// detector reads these to know which slots to watch.
    pub(crate) crossings: Vec<CrossingDescriptor>,
    /// Engine-global reaction definitions returned from `setupMod()`.
    /// These are durable definitions, not the currently active per-level set.
    pub(crate) global_reactions: Vec<ScopedReaction>,
    /// Engine-global crossing definitions returned from `setupMod()`.
    /// These are durable definitions, not the currently active per-level set.
    pub(crate) global_crossings: Vec<ScopedCrossing>,
    /// Level-local reaction definitions from `setupLevel()`. Retained so a
    /// staged mod-init reload can recompose active globals without rerunning the
    /// level data script.
    level_reactions: Vec<NamedReaction>,
    /// Level-local crossing definitions from `setupLevel()`. Retained for the
    /// same staged-reload recomposition path as [`Self::level_reactions`].
    level_crossings: Vec<CrossingDescriptor>,
    /// Entity-type descriptors. Engine-global — survive level unload.
    /// Populated by the boot caller after `run_mod_init`: it drains the
    /// `entities` field of the validated `setupMod()` return value into here
    /// via [`Self::upsert_entity_type`]. Read by the data-archetype spawn
    /// sweep. Not populated from `setupLevel()`.
    pub(crate) entities: Vec<EntityTypeDescriptor>,
    /// Mod map catalog entries. Engine-global — survive level unload.
    /// Populated by the boot caller from `setupMod()`'s `maps` field so the
    /// frontend and catalog-id load path can discover maps before a level is
    /// loaded. Not populated from `setupLevel()`.
    pub(crate) maps: Vec<ModMapEntry>,
}

impl DataRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Append a manifest's level-local definitions, then recompose the active
    /// per-level reaction/crossing sets from matching globals plus locals.
    /// Existing level-local definitions are preserved — call [`Self::clear`]
    /// first for a fresh population. Entity-type descriptors arrive separately
    /// via `setupMod()`'s `entities` return field (they outlive level unload).
    pub(crate) fn populate_from_manifest(&mut self, manifest: LevelManifest, tags: &[String]) {
        let LevelManifest {
            reactions,
            crossings,
            // Level-scope UI trees are drained off the manifest and registered
            // into the app-side `UiTreeRegistry` at the level-load drain point
            // (main.rs), before this `populate_from_manifest` consumes the rest.
            // They are not engine-global data-registry state, so nothing lands
            // here.
            ui_trees: _,
        } = manifest;
        self.level_reactions.extend(reactions);
        self.level_crossings.extend(crossings);
        self.recompose_active_sets(tags);
    }

    /// Rebuild the active per-level sets from durable globals plus retained
    /// level-local definitions. Empty `levels` scopes match every level;
    /// otherwise matching is exact, case-sensitive intersection with `tags`.
    pub(crate) fn recompose_active_sets(&mut self, tags: &[String]) {
        let mut reactions: Vec<NamedReaction> = self
            .global_reactions
            .iter()
            .filter(|reaction| Self::levels_match(&reaction.levels, tags))
            .map(|reaction| reaction.reaction.clone())
            .collect();
        reactions.extend(self.level_reactions.iter().cloned());
        Self::warn_duplicate_reaction_names(&reactions);

        let mut crossings: Vec<CrossingDescriptor> = self
            .global_crossings
            .iter()
            .filter(|crossing| Self::levels_match(&crossing.levels, tags))
            .map(|crossing| crossing.crossing.clone())
            .collect();
        crossings.extend(self.level_crossings.iter().cloned());

        self.reactions = reactions;
        self.crossings = crossings;
    }

    fn levels_match(levels: &[String], tags: &[String]) -> bool {
        levels.is_empty()
            || levels
                .iter()
                .any(|level| tags.iter().any(|tag| tag == level))
    }

    fn warn_duplicate_reaction_names(reactions: &[NamedReaction]) {
        let mut seen = HashSet::new();
        for reaction in reactions {
            if !seen.insert(reaction.name.as_str()) {
                log::warn!(
                    "[Loader] duplicate active reaction name `{}` in composed reaction set; all matching reactions will fire",
                    reaction.name,
                );
            }
        }
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

    /// Replace the engine-global map catalog snapshot as one complete commit.
    /// Used by startup and successful staged mod-init commits.
    pub(crate) fn replace_maps(&mut self, maps: Vec<ModMapEntry>) {
        self.maps = maps;
    }

    /// Replace the engine-global reaction definition snapshot as one complete
    /// commit. No dedupe: same-name collisions are preserved intentionally.
    pub(crate) fn replace_global_reactions(&mut self, reactions: Vec<ScopedReaction>) {
        self.global_reactions = reactions;
    }

    /// Replace the engine-global crossing definition snapshot as one complete
    /// commit. No dedupe: collisions are preserved intentionally.
    pub(crate) fn replace_global_crossings(&mut self, crossings: Vec<ScopedCrossing>) {
        self.global_crossings = crossings;
    }

    /// Drop every active per-level reaction/crossing. Engine-global entity,
    /// map, and global reaction/crossing definitions outlive the clear. Called
    /// on level unload.
    /// See [`Self::upsert_entity_type`].
    pub(crate) fn clear(&mut self) {
        self.reactions.clear();
        self.crossings.clear();
        self.level_reactions.clear();
        self.level_crossings.clear();
    }

    /// Returns `true` only when both collections are empty. After level unload,
    /// `entities` or `maps` may still be populated, so this returns `false` —
    /// production code should use `reactions.is_empty()` for level-unload checks.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.reactions.is_empty()
            && self.crossings.is_empty()
            && self.global_reactions.is_empty()
            && self.global_crossings.is_empty()
            && self.level_reactions.is_empty()
            && self.level_crossings.is_empty()
            && self.entities.is_empty()
            && self.maps.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::data_descriptors::{
        CrossingCondition, CrossingDescriptor, EntityTypeDescriptor, NamedReaction,
        PrimitiveDescriptor, ReactionDescriptor,
    };
    use crate::scripting::{reaction_dispatch, reactions::log_capture};

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

    fn sample_map(id: &str) -> ModMapEntry {
        ModMapEntry {
            id: id.to_string(),
            path: format!("maps/{id}.prl"),
            name: id.to_string(),
            tags: vec!["campaign".to_string()],
        }
    }

    fn sample_global_reaction(name: &str) -> ScopedReaction {
        sample_scoped_reaction(name, &["campaign"])
    }

    fn sample_scoped_reaction(name: &str, levels: &[&str]) -> ScopedReaction {
        sample_scoped_reaction_with_on_complete(name, levels, None)
    }

    fn sample_scoped_reaction_with_on_complete(
        name: &str,
        levels: &[&str],
        on_complete: Option<&str>,
    ) -> ScopedReaction {
        ScopedReaction {
            reaction: NamedReaction {
                name: name.to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "moveGeometry".to_string(),
                    tag: Some("reactorChambers".to_string()),
                    on_complete: on_complete.map(str::to_string),
                    args: serde_json::Value::Object(Default::default()),
                }),
            },
            levels: levels.iter().map(|level| level.to_string()).collect(),
        }
    }

    fn sample_global_crossing(slot: &str) -> ScopedCrossing {
        sample_scoped_crossing(slot, &["campaign"])
    }

    fn sample_scoped_crossing(slot: &str, levels: &[&str]) -> ScopedCrossing {
        ScopedCrossing {
            crossing: CrossingDescriptor {
                slot: slot.to_string(),
                condition: CrossingCondition::Below { threshold: 0.5 },
                max: 1.0,
                fire: vec!["lowHealth".to_string()],
            },
            levels: levels.iter().map(|level| level.to_string()).collect(),
        }
    }

    fn tags(tags: &[&str]) -> Vec<String> {
        tags.iter().map(|tag| tag.to_string()).collect()
    }

    fn reaction_names(registry: &DataRegistry) -> Vec<String> {
        registry
            .reactions
            .iter()
            .map(|reaction| reaction.name.clone())
            .collect()
    }

    fn crossing_slots(registry: &DataRegistry) -> Vec<String> {
        registry
            .crossings
            .iter()
            .map(|crossing| crossing.slot.clone())
            .collect()
    }

    #[test]
    fn new_registry_is_empty() {
        let r = DataRegistry::new();
        assert!(r.is_empty());
    }

    #[test]
    fn populate_appends_manifest_entries() {
        let mut r = DataRegistry::new();
        r.populate_from_manifest(sample_manifest(), &[]);
        assert_eq!(r.reactions.len(), 1);
        assert!(!r.is_empty());
    }

    #[test]
    fn populate_composes_unscoped_global_reactions_into_every_level() {
        let mut r = DataRegistry::new();
        r.replace_global_reactions(vec![sample_scoped_reaction("globalLoad", &[])]);

        r.populate_from_manifest(sample_manifest(), &tags(&["deathmatch"]));

        assert_eq!(
            reaction_names(&r),
            vec!["globalLoad".to_string(), "wave1Complete".to_string()]
        );
    }

    #[test]
    fn populate_filters_scoped_global_reactions_by_exact_tag_intersection() {
        let mut r = DataRegistry::new();
        r.replace_global_reactions(vec![
            sample_scoped_reaction("campaignLoad", &["campaign"]),
            sample_scoped_reaction("deathmatchLoad", &["deathmatch"]),
            sample_scoped_reaction("caseMismatchLoad", &["Campaign"]),
        ]);

        r.populate_from_manifest(LevelManifest::default(), &tags(&["campaign"]));

        assert_eq!(reaction_names(&r), vec!["campaignLoad".to_string()]);
    }

    #[test]
    fn recompose_isolates_disjoint_campaign_and_deathmatch_scopes() {
        let mut r = DataRegistry::new();
        r.replace_global_reactions(vec![
            sample_scoped_reaction("campaignLoad", &["campaign"]),
            sample_scoped_reaction("deathmatchLoad", &["deathmatch"]),
        ]);

        r.populate_from_manifest(LevelManifest::default(), &tags(&["campaign"]));
        assert_eq!(reaction_names(&r), vec!["campaignLoad".to_string()]);

        r.clear();
        r.populate_from_manifest(LevelManifest::default(), &tags(&["deathmatch"]));
        assert_eq!(reaction_names(&r), vec!["deathmatchLoad".to_string()]);
    }

    #[test]
    fn populate_appends_level_local_reactions_after_matching_globals() {
        let mut r = DataRegistry::new();
        r.replace_global_reactions(vec![sample_scoped_reaction("globalLoad", &["campaign"])]);

        r.populate_from_manifest(sample_manifest(), &tags(&["campaign", "intro"]));

        assert_eq!(
            reaction_names(&r),
            vec!["globalLoad".to_string(), "wave1Complete".to_string()]
        );
    }

    #[test]
    fn populate_preserves_same_name_active_reactions() {
        let mut r = DataRegistry::new();
        r.replace_global_reactions(vec![sample_scoped_reaction("levelLoad", &[])]);

        let records = log_capture::capture(|| {
            r.populate_from_manifest(
                LevelManifest {
                    reactions: vec![NamedReaction {
                        name: "levelLoad".to_string(),
                        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                            primitive: "activateGroup".to_string(),
                            tag: Some("local".to_string()),
                            on_complete: None,
                            args: serde_json::Value::Object(Default::default()),
                        }),
                    }],
                    crossings: Vec::new(),
                    ui_trees: Vec::new(),
                },
                &tags(&["campaign"]),
            );
        });

        assert!(
            records.iter().any(|(level, message)| {
                *level == log::Level::Warn
                    && message.contains("duplicate active reaction name `levelLoad`")
                    && message.contains("all matching reactions will fire")
            }),
            "same-name composition must warn, got {records:?}"
        );
        assert_eq!(
            reaction_names(&r),
            vec!["levelLoad".to_string(), "levelLoad".to_string()]
        );
    }

    #[test]
    fn same_name_active_reactions_both_dispatch_additively() {
        let mut r = DataRegistry::new();
        r.replace_global_reactions(vec![sample_scoped_reaction_with_on_complete(
            "levelLoad",
            &[],
            Some("globalDone"),
        )]);

        r.populate_from_manifest(
            LevelManifest {
                reactions: vec![NamedReaction {
                    name: "levelLoad".to_string(),
                    descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                        primitive: "activateGroup".to_string(),
                        tag: Some("local".to_string()),
                        on_complete: Some("localDone".to_string()),
                        args: serde_json::Value::Object(Default::default()),
                    }),
                }],
                crossings: Vec::new(),
                ui_trees: Vec::new(),
            },
            &tags(&["campaign"]),
        );

        assert_eq!(
            reaction_names(&r),
            vec!["levelLoad".to_string(), "levelLoad".to_string()]
        );
        assert_eq!(
            reaction_dispatch::fire_named_event("levelLoad", &r),
            vec!["globalDone".to_string(), "localDone".to_string()]
        );
    }

    #[test]
    fn populate_composes_crossings_from_matching_globals_and_level_locals() {
        let mut r = DataRegistry::new();
        r.replace_global_crossings(vec![
            sample_scoped_crossing("global.health", &["campaign"]),
            sample_scoped_crossing("deathmatch.health", &["deathmatch"]),
            sample_scoped_crossing("global.ammo", &[]),
        ]);

        r.populate_from_manifest(
            LevelManifest {
                reactions: Vec::new(),
                crossings: vec![sample_global_crossing("local.health").crossing],
                ui_trees: Vec::new(),
            },
            &tags(&["campaign"]),
        );

        assert_eq!(
            crossing_slots(&r),
            vec![
                "global.health".to_string(),
                "global.ammo".to_string(),
                "local.health".to_string(),
            ]
        );
    }

    #[test]
    fn replace_global_definitions_then_recompose_updates_active_sets() {
        let mut r = DataRegistry::new();
        let active_tags = tags(&["campaign"]);
        r.replace_global_reactions(vec![sample_scoped_reaction("oldGlobal", &[])]);
        r.replace_global_crossings(vec![sample_scoped_crossing("old.health", &[])]);
        r.populate_from_manifest(sample_manifest(), &active_tags);

        r.replace_global_reactions(vec![
            sample_scoped_reaction("newCampaignGlobal", &["campaign"]),
            sample_scoped_reaction("newDeathmatchGlobal", &["deathmatch"]),
        ]);
        r.replace_global_crossings(vec![
            sample_scoped_crossing("new.health", &["campaign"]),
            sample_scoped_crossing("excluded.health", &["deathmatch"]),
        ]);
        r.recompose_active_sets(&active_tags);

        assert_eq!(
            reaction_names(&r),
            vec!["newCampaignGlobal".to_string(), "wave1Complete".to_string(),]
        );
        assert_eq!(crossing_slots(&r), vec!["new.health".to_string()]);
    }

    #[test]
    fn skipped_staged_replace_leaves_active_sets_unchanged() {
        let mut r = DataRegistry::new();
        let active_tags = tags(&["campaign"]);
        r.replace_global_reactions(vec![sample_scoped_reaction("oldGlobal", &[])]);
        r.replace_global_crossings(vec![sample_scoped_crossing("old.health", &[])]);
        r.populate_from_manifest(sample_manifest(), &active_tags);
        let active_reactions_before = r.reactions.clone();
        let active_crossings_before = r.crossings.clone();
        let global_reactions_before = r.global_reactions.clone();
        let global_crossings_before = r.global_crossings.clone();
        let skipped_reactions = vec![sample_scoped_reaction("newGlobal", &[])];
        let skipped_crossings = vec![sample_scoped_crossing("new.health", &[])];

        // Simulates the stale/failed staged-manifest early-return path: the
        // candidate snapshot exists, but the registry receives no replacement.
        drop((skipped_reactions, skipped_crossings));
        r.recompose_active_sets(&active_tags);

        assert_eq!(r.global_reactions, global_reactions_before);
        assert_eq!(r.global_crossings, global_crossings_before);
        assert_eq!(r.reactions, active_reactions_before);
        assert_eq!(r.crossings, active_crossings_before);
    }

    #[test]
    fn clear_drops_reactions_but_keeps_entity_descriptors() {
        let mut r = DataRegistry::new();
        r.populate_from_manifest(sample_manifest(), &[]);
        r.upsert_entity_type(grunt_descriptor());
        r.clear();
        assert_eq!(r.reactions.len(), 0);
        assert_eq!(r.entities.len(), 1, "entities survive level unload");
    }

    #[test]
    fn clear_drops_reactions_but_keeps_map_catalog() {
        let mut r = DataRegistry::new();
        r.populate_from_manifest(sample_manifest(), &[]);
        r.replace_maps(vec![sample_map("e1m1")]);

        r.clear();

        assert_eq!(r.reactions.len(), 0);
        assert_eq!(r.maps, vec![sample_map("e1m1")]);
    }

    #[test]
    fn clear_drops_active_sets_but_keeps_global_reactions_and_crossings() {
        let mut r = DataRegistry::new();
        r.populate_from_manifest(sample_manifest(), &[]);
        let global_reactions = vec![sample_global_reaction("levelLoad")];
        let global_crossings = vec![sample_global_crossing("player.health")];
        r.replace_global_reactions(global_reactions.clone());
        r.replace_global_crossings(global_crossings.clone());

        r.clear();

        assert!(r.reactions.is_empty());
        assert!(r.crossings.is_empty());
        assert_eq!(r.global_reactions, global_reactions);
        assert_eq!(r.global_crossings, global_crossings);
    }

    #[test]
    fn global_definitions_survive_clear_and_recompose_after_reload() {
        let mut r = DataRegistry::new();
        let global_reactions = vec![sample_global_reaction("levelLoad")];
        let global_crossings = vec![sample_global_crossing("player.health")];
        r.replace_global_reactions(global_reactions.clone());
        r.replace_global_crossings(global_crossings.clone());
        r.populate_from_manifest(LevelManifest::default(), &tags(&["campaign"]));

        r.clear();

        assert!(r.reactions.is_empty());
        assert!(r.crossings.is_empty());
        assert_eq!(r.global_reactions, global_reactions);
        assert_eq!(r.global_crossings, global_crossings);

        r.populate_from_manifest(LevelManifest::default(), &tags(&["campaign"]));

        assert_eq!(reaction_names(&r), vec!["levelLoad".to_string()]);
        assert_eq!(crossing_slots(&r), vec!["player.health".to_string()]);
    }

    #[test]
    fn replace_global_reactions_preserves_collisions() {
        let mut r = DataRegistry::new();
        let first = sample_global_reaction("levelLoad");
        let second = sample_global_reaction("levelLoad");

        r.replace_global_reactions(vec![first.clone(), second.clone()]);

        assert_eq!(r.global_reactions, vec![first, second]);
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
