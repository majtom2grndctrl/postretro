// Reaction dispatch: walks the data-script reaction registry, fires named
// events, and tracks per-tag kill progress for `progress` reactions.
// See: context/lib/scripting.md §2 (Data context lifecycle)
//
// Lives separate from the behavior `HandlerTable`. Clearing behavior handlers
// does not touch [`ProgressTracker`] state and vice versa, so a behavior
// hot-reload preserves in-flight progress subscriptions.
//
// Primitive reaction bodies (moveGeometry, activateGroup, ...) are out of
// scope for this task — dispatch logs the attempt and the implementation lands
// in follow-on work.

use std::collections::HashMap;

use super::data_descriptors::{
    EntityTypeDescriptor, NamedReaction, ReactionDescriptor, SequenceStep,
};
use super::data_registry::DataRegistry;
use super::registry::EntityRegistry;
use super::sequence::SequencedPrimitiveRegistry;

/// Per-subscription kill-count state for a single progress reaction.
///
/// `total` is captured at level load; subsequent spawns do NOT raise it. The
/// progress ratio is `(total - killed) / total` walking from `total → 0` as
/// kills come in, but the threshold compare is expressed as `killed/total >= at`
/// for symmetry with the script-side spelling (`at: 1.0` means "all dead").
#[derive(Debug, Clone, PartialEq)]
struct ProgressState {
    total: u32,
    killed: u32,
    at: f32,
    /// Event name to fire via [`fire_named_event`] once the threshold trips.
    fire: String,
    /// One-shot guard: a progress subscription fires its `fire` event exactly
    /// once even if more entities die after the threshold has been crossed.
    fired: bool,
}

/// Active progress subscriptions for the current level, keyed by the spawn tag
/// each subscription watches.
///
/// One reaction whose `progress.tag = "wave1"` produces one entry under
/// `subscriptions["wave1"]`. An entity tagged with both `"wave1"` and
/// `"reactorMonster"` (carrying space-delimited `_tags`) decrements counters in
/// both buckets independently when it dies — see the multi-tag test below.
pub(crate) struct ProgressTracker {
    subscriptions: HashMap<String, Vec<ProgressState>>,
}

impl ProgressTracker {
    pub(crate) fn new() -> Self {
        Self {
            subscriptions: HashMap::new(),
        }
    }

    /// Populate from the reaction registry and entity registry. For each
    /// progress reaction, count how many live entities currently carry the
    /// reaction's `tag`; that count becomes the subscription's `total`.
    ///
    /// Idempotent in the sense that it always rebuilds from current state —
    /// callers should `clear()` first if they want a fresh population without
    /// duplicate subscriptions.
    pub(crate) fn initialize(
        &mut self,
        data_registry: &DataRegistry,
        entity_registry: &EntityRegistry,
    ) {
        for named in &data_registry.reactions {
            if let ReactionDescriptor::Progress(p) = &named.descriptor {
                let total = count_entities_with_tag(entity_registry, &p.tag);
                let bucket = self.subscriptions.entry(p.tag.clone()).or_default();
                bucket.push(ProgressState {
                    total,
                    killed: 0,
                    at: p.at,
                    fire: p.fire.clone(),
                    fired: false,
                });
            }
        }
    }

    /// Called when an entity carrying `tags` is killed. For each tag that has
    /// an active subscription, increment the killed counter and (if the
    /// threshold is now crossed) record the subscription's `fire` event.
    ///
    /// Returns the list of event names to fire — caller drains and runs them
    /// through [`fire_named_event`]. Returning rather than firing inline keeps
    /// dispatch single-borrow and lets the caller decide ordering.
    pub(crate) fn on_entity_killed(&mut self, tags: &[String]) -> Vec<String> {
        let mut to_fire = Vec::new();
        for tag in tags {
            let Some(subs) = self.subscriptions.get_mut(tag) else {
                continue;
            };
            for state in subs.iter_mut() {
                if state.fired || state.total == 0 {
                    continue;
                }
                state.killed = state.killed.saturating_add(1);
                let ratio = state.killed as f32 / state.total as f32;
                if ratio >= state.at {
                    state.fired = true;
                    to_fire.push(state.fire.clone());
                }
            }
        }
        to_fire
    }

    /// Drop every subscription. Called on level unload and during data-script
    /// hot-reload — independent from the behavior `HandlerTable` clear path.
    pub(crate) fn clear(&mut self) {
        self.subscriptions.clear();
    }

    #[cfg(test)]
    fn subscription_count(&self, tag: &str) -> usize {
        self.subscriptions.get(tag).map(|v| v.len()).unwrap_or(0)
    }
}

impl Default for ProgressTracker {
    fn default() -> Self {
        Self::new()
    }
}

fn count_entities_with_tag(entity_registry: &EntityRegistry, tag: &str) -> u32 {
    use super::registry::ComponentKind;

    // INVARIANT: every spawned entity carries a Transform component.
    // `EntityRegistry::spawn` writes the Transform column unconditionally, so a
    // single Transform-keyed query enumerates the entire live entity set. If
    // that invariant ever breaks (e.g., a spawn path skips Transform), this
    // count silently underreports — so the invariant is load-bearing for
    // progress-tracker correctness, not just an implementation detail.
    //
    // We intentionally walk just the Transform column rather than unioning
    // matches across every `ComponentKind`: a union query risks double-counting
    // entities that carry multiple components.
    entity_registry
        .query_by_component_and_tag(ComponentKind::Transform, Some(tag))
        .count() as u32
}

/// Walk the reaction registry for `event_name`. For each matching reaction:
/// - **Progress**: no-op at dispatch time. Progress subscriptions track kills
///   independently via [`ProgressTracker`]; their reaction *name* is just a
///   label for diagnostics, not an activation trigger.
/// - **Primitive**: log the dispatch attempt. The actual primitive body
///   (moveGeometry, activateGroup, ...) is implemented in follow-on tasks;
///   this hook proves the routing is wired before that work lands.
///
/// Returns event names produced by primitive `onComplete` fields so callers
/// can chain dispatches without needing to inspect descriptors themselves.
/// (Primitives are not actually executed here — only logged; `onComplete`
/// names are returned immediately rather than deferred, which will change
/// once primitive bodies dispatch to behavior-script handlers.)
pub(crate) fn fire_named_event(event_name: &str, data_registry: &DataRegistry) -> Vec<String> {
    let mut chained = Vec::new();
    for named in &data_registry.reactions {
        if named.name != event_name {
            continue;
        }
        match &named.descriptor {
            ReactionDescriptor::Progress(_) => {
                // Tracked independently — see ProgressTracker. No-op at fire
                // time so that `fire_named_event("waveDone")` from arbitrary
                // call sites does not double-fire a subscription.
            }
            ReactionDescriptor::Primitive(p) => {
                log::info!(
                    "[Scripting] dispatch primitive '{}' on tag '{}'",
                    p.primitive,
                    p.tag,
                );
                if let Some(on_complete) = &p.on_complete {
                    chained.push(on_complete.clone());
                }
            }
            ReactionDescriptor::Sequence(_) => {
                // Sequence dispatch needs the entity registry and sequenced-
                // primitive table — see [`fire_named_event_with_sequences`].
                // Reaching this branch via the registry-only path is silent
                // intentionally: callers that don't supply a sequence registry
                // (most don't yet) get a no-op rather than a panic.
            }
        }
    }
    chained
}

/// Dispatch variant that also runs `sequence` reactions through the supplied
/// [`SequencedPrimitiveRegistry`]. Behavior matches [`fire_named_event`] for
/// the `Progress` and `Primitive` variants; sequences iterate their steps
/// here.
///
/// Per-step semantics:
/// - Stale [`EntityId`] (not present in `entity_registry`): log a warning and
///   continue with the next step. Not fatal.
/// - Unknown primitive name: validated at registration time (see
///   [`validate_sequence_primitives`]). At dispatch time, defensively log a
///   warning and continue.
/// - Handler returns `Err`: log a warning and continue.
pub(crate) fn fire_named_event_with_sequences(
    event_name: &str,
    data_registry: &DataRegistry,
    sequence_registry: &SequencedPrimitiveRegistry,
    entity_registry: &EntityRegistry,
) -> Vec<String> {
    let mut chained = Vec::new();
    for named in &data_registry.reactions {
        if named.name != event_name {
            continue;
        }
        match &named.descriptor {
            ReactionDescriptor::Progress(_) => {}
            ReactionDescriptor::Primitive(p) => {
                log::info!(
                    "[Scripting] dispatch primitive '{}' on tag '{}'",
                    p.primitive,
                    p.tag,
                );
                if let Some(on_complete) = &p.on_complete {
                    chained.push(on_complete.clone());
                }
            }
            ReactionDescriptor::Sequence(steps) => {
                dispatch_sequence(steps, sequence_registry, entity_registry);
            }
        }
    }
    chained
}

fn dispatch_sequence(
    steps: &[SequenceStep],
    sequence_registry: &SequencedPrimitiveRegistry,
    entity_registry: &EntityRegistry,
) {
    for (i, step) in steps.iter().enumerate() {
        if !entity_registry.exists(step.id) {
            log::warn!(
                "[Scripting] sequence step {i}: entity {:?} not found, skipping",
                step.id
            );
            continue;
        }
        let Some(handler) = sequence_registry.get(&step.primitive) else {
            // Validation at registration time should make this unreachable
            // for properly-loaded manifests; defensive log keeps a runtime
            // primitive-table mutation from silently corrupting dispatch.
            log::warn!(
                "[Scripting] sequence step {i}: unknown primitive '{}', skipping",
                step.primitive
            );
            continue;
        };
        if let Err(e) = handler(step.id, &step.args) {
            log::warn!(
                "[Scripting] sequence step {i}: primitive '{}' on entity {:?} failed: {e}",
                step.primitive,
                step.id
            );
        }
    }
}

/// Walk a slice of reactions and drop any whose `Sequence` body references a
/// primitive name not present in `sequence_registry`. Logs an error per
/// rejected reaction and returns the surviving set in original order.
///
/// Called at `registerLevelManifest()` time after the script's return value
/// has been deserialized but before it lands in [`DataRegistry`].
pub(crate) fn validate_sequence_primitives(
    reactions: Vec<NamedReaction>,
    sequence_registry: &SequencedPrimitiveRegistry,
) -> Vec<NamedReaction> {
    reactions
        .into_iter()
        .filter(|named| {
            let ReactionDescriptor::Sequence(steps) = &named.descriptor else {
                return true;
            };
            for (i, step) in steps.iter().enumerate() {
                if !sequence_registry.contains(&step.primitive) {
                    log::error!(
                        "[Scripting] registerLevelManifest: sequence step {i} names unknown primitive \"{}\"",
                        step.primitive
                    );
                    return false;
                }
            }
            true
        })
        .collect()
}

/// Resolve an entity type by classname from the data registry. Returns `None`
/// when the classname is not registered.
///
/// Linear scan — entity-type counts per level are small (handful to a few
/// dozen) and the lookup happens at map-entity instantiation time, not in a
/// hot loop. A `HashMap` keyed by classname is an easy upgrade if profiling
/// ever flags this path.
pub(crate) fn resolve_entity_type<'a>(
    classname: &str,
    data_registry: &'a DataRegistry,
) -> Option<&'a EntityTypeDescriptor> {
    data_registry
        .entities
        .iter()
        .find(|e| e.classname == classname)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripting::data_descriptors::{
        EntityTypeDescriptor, LevelManifest, NamedReaction, PrimitiveDescriptor,
        ProgressDescriptor, ReactionDescriptor,
    };
    use crate::scripting::registry::{EntityRegistry, Transform};

    fn progress_reaction(name: &str, tag: &str, at: f32, fire: &str) -> NamedReaction {
        NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Progress(ProgressDescriptor {
                tag: tag.to_string(),
                at,
                fire: fire.to_string(),
            }),
        }
    }

    fn primitive_reaction(
        name: &str,
        primitive: &str,
        tag: &str,
        on_complete: Option<&str>,
    ) -> NamedReaction {
        NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: primitive.to_string(),
                tag: tag.to_string(),
                on_complete: on_complete.map(|s| s.to_string()),
            }),
        }
    }

    fn spawn_with_tags(reg: &mut EntityRegistry, tags: &[&str]) {
        let id = reg.spawn(Transform::default());
        let owned: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
        reg.set_tags(id, owned).unwrap();
    }

    #[test]
    fn progress_threshold_fires_when_all_dead_at_full_ratio() {
        // Acceptance criterion: registerReaction("waveDone", { progress: {
        //   tag: "wave1", at: 1.0, fire: "powerOn" } }) — when all entities
        // tagged "wave1" are dead, "powerOn" fires.
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![progress_reaction("waveDone", "wave1", 1.0, "powerOn")],
            entities: vec![],
        });

        let mut entities = EntityRegistry::new();
        spawn_with_tags(&mut entities, &["wave1"]);

        let mut tracker = ProgressTracker::new();
        tracker.initialize(&data, &entities);

        let fired = tracker.on_entity_killed(&["wave1".to_string()]);
        assert_eq!(fired, vec!["powerOn".to_string()]);
    }

    #[test]
    fn progress_does_not_fire_before_threshold() {
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![progress_reaction("waveDone", "wave1", 1.0, "powerOn")],
            entities: vec![],
        });

        let mut entities = EntityRegistry::new();
        spawn_with_tags(&mut entities, &["wave1"]);
        spawn_with_tags(&mut entities, &["wave1"]);

        let mut tracker = ProgressTracker::new();
        tracker.initialize(&data, &entities);

        // First kill of two: ratio 0.5 < 1.0 — no fire yet.
        let fired = tracker.on_entity_killed(&["wave1".to_string()]);
        assert!(fired.is_empty());

        // Second kill brings ratio to 1.0 and trips the threshold.
        let fired = tracker.on_entity_killed(&["wave1".to_string()]);
        assert_eq!(fired, vec!["powerOn".to_string()]);
    }

    #[test]
    fn progress_fires_at_partial_ratio_when_at_below_one() {
        // at: 0.5 fires when half are dead.
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![progress_reaction("half", "wave1", 0.5, "midwave")],
            entities: vec![],
        });

        let mut entities = EntityRegistry::new();
        for _ in 0..4 {
            spawn_with_tags(&mut entities, &["wave1"]);
        }

        let mut tracker = ProgressTracker::new();
        tracker.initialize(&data, &entities);

        // 1 of 4 dead: ratio 0.25 < 0.5
        assert!(tracker.on_entity_killed(&["wave1".into()]).is_empty());
        // 2 of 4 dead: ratio 0.5 >= 0.5 — fires.
        let fired = tracker.on_entity_killed(&["wave1".into()]);
        assert_eq!(fired, vec!["midwave".to_string()]);
        // 3 of 4 dead: already fired, no second fire.
        assert!(tracker.on_entity_killed(&["wave1".into()]).is_empty());
    }

    #[test]
    fn multi_tag_entity_decrements_both_buckets_independently() {
        // Acceptance criterion: an entity with `_tags "wave1 reactorMonster"`
        // contributes to kill counters for BOTH tags.
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![
                progress_reaction("waveDone", "wave1", 1.0, "powerOn"),
                progress_reaction("reactorDown", "reactorMonster", 1.0, "reactorOff"),
            ],
            entities: vec![],
        });

        let mut entities = EntityRegistry::new();
        spawn_with_tags(&mut entities, &["wave1", "reactorMonster"]);

        let mut tracker = ProgressTracker::new();
        tracker.initialize(&data, &entities);

        assert_eq!(tracker.subscription_count("wave1"), 1);
        assert_eq!(tracker.subscription_count("reactorMonster"), 1);

        // A single death carrying both tags should fire both events.
        let fired = tracker.on_entity_killed(&["wave1".to_string(), "reactorMonster".to_string()]);
        assert!(fired.contains(&"powerOn".to_string()));
        assert!(fired.contains(&"reactorOff".to_string()));
        assert_eq!(fired.len(), 2);
    }

    #[test]
    fn multi_tag_entity_fires_both_subscriptions() {
        // Acceptance: a single entity carrying both `wave1` and
        // `reactorMonster` is counted toward each tag's progress subscription
        // independently. With `at: 0.5` and exactly one entity per tag, that
        // one death satisfies the threshold for both subscriptions.
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![
                progress_reaction("waveDone", "wave1", 0.5, "powerOn"),
                progress_reaction("reactorDown", "reactorMonster", 0.5, "reactorOff"),
            ],
            entities: vec![],
        });

        let mut entities = EntityRegistry::new();
        spawn_with_tags(&mut entities, &["wave1", "reactorMonster"]);

        let mut tracker = ProgressTracker::new();
        tracker.initialize(&data, &entities);

        let fired = tracker.on_entity_killed(&["wave1".to_string(), "reactorMonster".to_string()]);
        assert!(fired.contains(&"powerOn".to_string()));
        assert!(fired.contains(&"reactorOff".to_string()));
        assert_eq!(fired.len(), 2);
    }

    #[test]
    fn killing_untracked_tag_is_a_no_op() {
        let mut tracker = ProgressTracker::new();
        // No subscriptions at all — death for any tag returns empty.
        let fired = tracker.on_entity_killed(&["ghosts".to_string()]);
        assert!(fired.is_empty());
    }

    #[test]
    fn clear_drops_all_subscriptions() {
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![progress_reaction("waveDone", "wave1", 1.0, "powerOn")],
            entities: vec![],
        });
        let mut entities = EntityRegistry::new();
        spawn_with_tags(&mut entities, &["wave1"]);

        let mut tracker = ProgressTracker::new();
        tracker.initialize(&data, &entities);
        assert_eq!(tracker.subscription_count("wave1"), 1);

        tracker.clear();
        assert_eq!(tracker.subscription_count("wave1"), 0);
        // After clear, kills are a no-op.
        assert!(tracker.on_entity_killed(&["wave1".into()]).is_empty());
    }

    #[test]
    fn progress_with_zero_total_never_fires() {
        // A subscription whose tag matches no live entities at init time:
        // `total == 0`. Avoid division-by-zero and never fire (no targets to
        // satisfy the "all dead" semantic).
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![progress_reaction("waveDone", "ghosts", 1.0, "spooky")],
            entities: vec![],
        });
        let entities = EntityRegistry::new();

        let mut tracker = ProgressTracker::new();
        tracker.initialize(&data, &entities);
        // Even if a stray "ghosts" death is reported, no fire.
        let fired = tracker.on_entity_killed(&["ghosts".into()]);
        assert!(fired.is_empty());
    }

    #[test]
    fn resolve_entity_type_finds_registered_classname() {
        // Acceptance: registerEntities([Grunt]) makes "grunt" resolvable.
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![],
            entities: vec![EntityTypeDescriptor {
                classname: "grunt".to_string(),
            }],
        });

        let resolved = resolve_entity_type("grunt", &data);
        assert_eq!(
            resolved,
            Some(&EntityTypeDescriptor {
                classname: "grunt".to_string()
            })
        );
    }

    #[test]
    fn resolve_entity_type_returns_none_for_missing_classname() {
        let data = DataRegistry::new();
        assert!(resolve_entity_type("grunt", &data).is_none());
    }

    #[test]
    fn fire_named_event_on_primitive_returns_on_complete_chain() {
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![primitive_reaction(
                "wave1Complete",
                "moveGeometry",
                "reactorChambers",
                Some("wave2Revealed"),
            )],
            entities: vec![],
        });

        let chained = fire_named_event("wave1Complete", &data);
        assert_eq!(chained, vec!["wave2Revealed".to_string()]);
    }

    #[test]
    fn fire_named_event_on_primitive_without_on_complete_returns_empty() {
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![primitive_reaction(
                "wave2Revealed",
                "activateGroup",
                "reactorWave2Monsters",
                None,
            )],
            entities: vec![],
        });

        let chained = fire_named_event("wave2Revealed", &data);
        assert!(chained.is_empty());
    }

    #[test]
    fn fire_named_event_on_progress_is_a_noop() {
        // Progress reactions are tracked independently — firing a named event
        // matching a progress reaction's name must not produce a chained event.
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![progress_reaction("waveDone", "wave1", 1.0, "powerOn")],
            entities: vec![],
        });
        let chained = fire_named_event("waveDone", &data);
        assert!(chained.is_empty());
    }

    #[test]
    fn fire_named_event_unknown_name_returns_empty() {
        let data = DataRegistry::new();
        let chained = fire_named_event("nothingHere", &data);
        assert!(chained.is_empty());
    }

    // --- sequence dispatch --------------------------------------------------

    use crate::scripting::data_descriptors::SequenceStep;
    use crate::scripting::registry::EntityId;
    use crate::scripting::sequence::{SequenceError, SequencedPrimitiveRegistry};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn sequence_reaction(name: &str, steps: Vec<SequenceStep>) -> NamedReaction {
        NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Sequence(steps),
        }
    }

    #[test]
    fn sequence_dispatch_runs_each_step_in_order() {
        let mut entities = EntityRegistry::new();
        let id_a = entities.spawn(Transform::default());
        let id_b = entities.spawn(Transform::default());

        let calls: Arc<std::sync::Mutex<Vec<(u32, i64)>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));

        let mut seq_reg = SequencedPrimitiveRegistry::new();
        let calls_cl = Arc::clone(&calls);
        seq_reg.register("noteValue", move |id, args| {
            let v = args["v"].as_i64().unwrap_or(-1);
            calls_cl.lock().unwrap().push((id.to_raw(), v));
            Ok(())
        });

        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![sequence_reaction(
                "go",
                vec![
                    SequenceStep {
                        id: id_a,
                        primitive: "noteValue".into(),
                        args: serde_json::json!({ "v": 1 }),
                    },
                    SequenceStep {
                        id: id_b,
                        primitive: "noteValue".into(),
                        args: serde_json::json!({ "v": 2 }),
                    },
                ],
            )],
            entities: vec![],
        });

        let chained = fire_named_event_with_sequences("go", &data, &seq_reg, &entities);
        assert!(chained.is_empty());
        let observed = calls.lock().unwrap().clone();
        assert_eq!(observed, vec![(id_a.to_raw(), 1), (id_b.to_raw(), 2)]);
    }

    #[test]
    fn sequence_dispatch_skips_stale_entity_and_continues() {
        // Acceptance: multi-step sequence where one step has a stale ID — the
        // surviving steps still execute.
        let mut entities = EntityRegistry::new();
        let id_a = entities.spawn(Transform::default());
        let id_b = entities.spawn(Transform::default());

        // Stale ID: reuse a slot that was despawned (mismatched generation).
        let id_dead = entities.spawn(Transform::default());
        entities.despawn(id_dead).unwrap();
        assert!(!entities.exists(id_dead));

        let count = Arc::new(AtomicU32::new(0));
        let count_cl = Arc::clone(&count);

        let mut seq_reg = SequencedPrimitiveRegistry::new();
        seq_reg.register("tick", move |_id, _args| {
            count_cl.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });

        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![sequence_reaction(
                "go",
                vec![
                    SequenceStep {
                        id: id_a,
                        primitive: "tick".into(),
                        args: serde_json::Value::Null,
                    },
                    SequenceStep {
                        id: id_dead,
                        primitive: "tick".into(),
                        args: serde_json::Value::Null,
                    },
                    SequenceStep {
                        id: id_b,
                        primitive: "tick".into(),
                        args: serde_json::Value::Null,
                    },
                ],
            )],
            entities: vec![],
        });

        fire_named_event_with_sequences("go", &data, &seq_reg, &entities);
        // Two surviving steps fired; the stale step was skipped.
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn sequence_dispatch_continues_after_handler_error() {
        let mut entities = EntityRegistry::new();
        let id_a = entities.spawn(Transform::default());
        let id_b = entities.spawn(Transform::default());

        let count = Arc::new(AtomicU32::new(0));
        let count_cl = Arc::clone(&count);

        let mut seq_reg = SequencedPrimitiveRegistry::new();
        seq_reg.register("ok", move |_id, _args| {
            count_cl.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        seq_reg.register("boom", |_id, _args| {
            Err(SequenceError::ExecutionFailed {
                reason: "intentional".into(),
            })
        });

        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![sequence_reaction(
                "go",
                vec![
                    SequenceStep {
                        id: id_a,
                        primitive: "boom".into(),
                        args: serde_json::Value::Null,
                    },
                    SequenceStep {
                        id: id_b,
                        primitive: "ok".into(),
                        args: serde_json::Value::Null,
                    },
                ],
            )],
            entities: vec![],
        });

        fire_named_event_with_sequences("go", &data, &seq_reg, &entities);
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn validate_sequence_primitives_drops_reaction_with_unknown_primitive() {
        let mut seq_reg = SequencedPrimitiveRegistry::new();
        seq_reg.register("known", |_id, _args| Ok(()));

        let bogus_id = EntityId::from_raw(0x0001_0000);
        let reactions = vec![
            sequence_reaction(
                "valid",
                vec![SequenceStep {
                    id: bogus_id,
                    primitive: "known".into(),
                    args: serde_json::Value::Null,
                }],
            ),
            sequence_reaction(
                "invalid",
                vec![
                    SequenceStep {
                        id: bogus_id,
                        primitive: "known".into(),
                        args: serde_json::Value::Null,
                    },
                    SequenceStep {
                        id: bogus_id,
                        primitive: "ghost".into(),
                        args: serde_json::Value::Null,
                    },
                ],
            ),
        ];

        let surviving = validate_sequence_primitives(reactions, &seq_reg);
        assert_eq!(surviving.len(), 1);
        assert_eq!(surviving[0].name, "valid");
    }
}
