// Reaction dispatch: fires named events and tracks per-tag kill progress.
// Lives separate from the behavior `HandlerTable`; a behavior hot-reload does
// not clear or rebuild the reaction registry.

use std::collections::HashMap;

use super::ctx::ScriptCtx;
use super::data_descriptors::{
    EntityTypeDescriptor, NamedReaction, PrimitiveDescriptor, ReactionDescriptor, SequenceStep,
};
use super::data_registry::DataRegistry;
use super::reactions::registry::ReactionPrimitiveRegistry;
use super::registry::{ComponentKind, EntityId, EntityRegistry};
use super::sequence::SequencedPrimitiveRegistry;

/// `total` is captured at level load; subsequent spawns do NOT raise it.
/// Threshold compare: `killed/total >= at` (`at: 1.0` means "all dead").
#[derive(Debug, Clone, PartialEq)]
struct ProgressState {
    total: u32,
    killed: u32,
    at: f32,
    fire: String,
    /// One-shot guard: fires exactly once even if more entities die after the threshold is crossed.
    fired: bool,
}

/// Active progress subscriptions for the current level, keyed by spawn tag.
/// An entity tagged with multiple values decrements each bucket independently when it dies.
pub(crate) struct ProgressTracker {
    subscriptions: HashMap<String, Vec<ProgressState>>,
}

impl ProgressTracker {
    pub(crate) fn new() -> Self {
        Self {
            subscriptions: HashMap::new(),
        }
    }

    /// Callers should `clear()` first to avoid duplicate subscriptions.
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

    /// Returns event names to fire; caller decides ordering and runs them through [`fire_named_event`].
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

    // INVARIANT: every spawned entity carries a Transform component — `EntityRegistry::spawn`
    // writes it unconditionally. A spawn path that skips Transform causes silent underreporting
    // here, which corrupts progress-tracker thresholds. Walking only the Transform column also
    // avoids double-counting entities that carry multiple components.
    entity_registry
        .query_by_component_and_tag(ComponentKind::Transform, Some(tag))
        .count() as u32
}

/// Returns event names from primitive `onComplete` fields for chained dispatch.
/// Progress reactions are always a no-op here — they are tracked via [`ProgressTracker`].
pub(crate) fn fire_named_event(event_name: &str, data_registry: &DataRegistry) -> Vec<String> {
    let mut chained = Vec::new();
    for named in &data_registry.reactions {
        if named.name != event_name {
            continue;
        }
        match &named.descriptor {
            ReactionDescriptor::Progress(_) => {
                // Tracked independently via ProgressTracker; no-op here prevents double-fire.
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
                // Requires the sequence registry — use [`fire_named_event_with_sequences`].
                // Callers without one (e.g. progress-chain dispatches) get a no-op, not a panic.
            }
        }
    }
    chained
}

/// Extends [`fire_named_event`] with sequence dispatch. Per-step errors (stale entity,
/// unknown primitive, handler `Err`) are logged as warnings and do not abort the sequence.
pub(crate) fn fire_named_event_with_sequences(
    event_name: &str,
    data_registry: &DataRegistry,
    sequence_registry: &SequencedPrimitiveRegistry,
    reaction_registry: &ReactionPrimitiveRegistry,
    script_ctx: &ScriptCtx,
) -> Vec<String> {
    let mut chained = Vec::new();
    for named in &data_registry.reactions {
        if named.name != event_name {
            continue;
        }
        match &named.descriptor {
            ReactionDescriptor::Progress(_) => {}
            ReactionDescriptor::Primitive(p) => {
                dispatch_primitive(p, reaction_registry, script_ctx);
                if let Some(on_complete) = &p.on_complete {
                    chained.push(on_complete.clone());
                }
            }
            ReactionDescriptor::Sequence(steps) => {
                dispatch_sequence(steps, sequence_registry, script_ctx);
            }
        }
    }
    chained
}

/// Targeting walks the Transform column per the invariant in [`count_entities_with_tag`].
/// Empty target sets are passed through; handlers decide whether to warn.
fn dispatch_primitive(
    descriptor: &PrimitiveDescriptor,
    reaction_registry: &ReactionPrimitiveRegistry,
    script_ctx: &ScriptCtx,
) {
    let targets: Vec<EntityId> = {
        let reg = script_ctx.registry.borrow();
        reg.query_by_component_and_tag(ComponentKind::Transform, Some(&descriptor.tag))
            .map(|(id, _)| id)
            .collect()
    };

    log::info!(
        "[Scripting] dispatch primitive '{}' on tag '{}' ({} targets)",
        descriptor.primitive,
        descriptor.tag,
        targets.len(),
    );

    let mut reg = script_ctx.registry.borrow_mut();
    match reaction_registry.dispatch(&descriptor.primitive, &mut reg, &targets, &descriptor.args) {
        Ok(true) => {}
        Ok(false) => log::warn!(
            "[Scripting] primitive '{}' is not registered; reaction had no effect",
            descriptor.primitive,
        ),
        Err(e) => log::warn!(
            "[Scripting] primitive '{}' dispatch failed: {e:?}",
            descriptor.primitive,
        ),
    }
}

fn dispatch_sequence(
    steps: &[SequenceStep],
    sequence_registry: &SequencedPrimitiveRegistry,
    script_ctx: &ScriptCtx,
) {
    for (i, step) in steps.iter().enumerate() {
        if !script_ctx.registry.borrow().exists(step.id) {
            log::warn!(
                "[Scripting] sequence step {i}: entity {:?} not found, skipping",
                step.id
            );
            continue;
        }
        let Some(handler) = sequence_registry.get(&step.primitive) else {
            // Should be unreachable for validated manifests; guards against runtime primitive-table mutations.
            log::error!(
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

/// Called at `registerLevelManifest()` time, before reactions land in [`DataRegistry`].
/// Drops any `Sequence` reaction whose steps name an unknown primitive; logs an error per rejection.
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

/// Linear scan — entity-type counts per level are small and this runs at instantiation time, not in a hot loop.
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
                args: serde_json::Value::Object(Default::default()),
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
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![progress_reaction("waveDone", "wave1", 1.0, "powerOn")],
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
        });

        let mut entities = EntityRegistry::new();
        spawn_with_tags(&mut entities, &["wave1"]);
        spawn_with_tags(&mut entities, &["wave1"]);

        let mut tracker = ProgressTracker::new();
        tracker.initialize(&data, &entities);

        let fired = tracker.on_entity_killed(&["wave1".to_string()]);
        assert!(fired.is_empty());

        let fired = tracker.on_entity_killed(&["wave1".to_string()]);
        assert_eq!(fired, vec!["powerOn".to_string()]);
    }

    #[test]
    fn progress_fires_at_partial_ratio_when_at_below_one() {
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![progress_reaction("half", "wave1", 0.5, "midwave")],
        });

        let mut entities = EntityRegistry::new();
        for _ in 0..4 {
            spawn_with_tags(&mut entities, &["wave1"]);
        }

        let mut tracker = ProgressTracker::new();
        tracker.initialize(&data, &entities);

        assert!(tracker.on_entity_killed(&["wave1".into()]).is_empty());
        let fired = tracker.on_entity_killed(&["wave1".into()]);
        assert_eq!(fired, vec!["midwave".to_string()]);
        assert!(tracker.on_entity_killed(&["wave1".into()]).is_empty());
    }

    #[test]
    fn multi_tag_entity_decrements_both_buckets_independently() {
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![
                progress_reaction("waveDone", "wave1", 1.0, "powerOn"),
                progress_reaction("reactorDown", "reactorMonster", 1.0, "reactorOff"),
            ],
        });

        let mut entities = EntityRegistry::new();
        spawn_with_tags(&mut entities, &["wave1", "reactorMonster"]);

        let mut tracker = ProgressTracker::new();
        tracker.initialize(&data, &entities);

        assert_eq!(tracker.subscription_count("wave1"), 1);
        assert_eq!(tracker.subscription_count("reactorMonster"), 1);

        let fired = tracker.on_entity_killed(&["wave1".to_string(), "reactorMonster".to_string()]);
        assert!(fired.contains(&"powerOn".to_string()));
        assert!(fired.contains(&"reactorOff".to_string()));
        assert_eq!(fired.len(), 2);
    }

    #[test]
    fn multi_tag_entity_fires_both_subscriptions() {
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![
                progress_reaction("waveDone", "wave1", 0.5, "powerOn"),
                progress_reaction("reactorDown", "reactorMonster", 0.5, "reactorOff"),
            ],
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
        let fired = tracker.on_entity_killed(&["ghosts".to_string()]);
        assert!(fired.is_empty());
    }

    #[test]
    fn clear_drops_all_subscriptions() {
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![progress_reaction("waveDone", "wave1", 1.0, "powerOn")],
        });
        let mut entities = EntityRegistry::new();
        spawn_with_tags(&mut entities, &["wave1"]);

        let mut tracker = ProgressTracker::new();
        tracker.initialize(&data, &entities);
        assert_eq!(tracker.subscription_count("wave1"), 1);

        tracker.clear();
        assert_eq!(tracker.subscription_count("wave1"), 0);
        assert!(tracker.on_entity_killed(&["wave1".into()]).is_empty());
    }

    #[test]
    fn progress_with_zero_total_never_fires() {
        // `total == 0` at init: no division-by-zero and threshold never fires.
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![progress_reaction("waveDone", "ghosts", 1.0, "spooky")],
        });
        let entities = EntityRegistry::new();

        let mut tracker = ProgressTracker::new();
        tracker.initialize(&data, &entities);
        let fired = tracker.on_entity_killed(&["ghosts".into()]);
        assert!(fired.is_empty());
    }

    #[test]
    fn resolve_entity_type_finds_registered_classname() {
        let mut data = DataRegistry::new();
        data.upsert_entity_type(EntityTypeDescriptor {
            classname: "grunt".to_string(),
            light: None,
            emitter: None,
        });

        let resolved = resolve_entity_type("grunt", &data);
        assert_eq!(
            resolved,
            Some(&EntityTypeDescriptor {
                classname: "grunt".to_string(),
                light: None,
                emitter: None,
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
        });

        let chained = fire_named_event("wave2Revealed", &data);
        assert!(chained.is_empty());
    }

    #[test]
    fn fire_named_event_on_progress_is_a_noop() {
        let mut data = DataRegistry::new();
        data.populate_from_manifest(LevelManifest {
            reactions: vec![progress_reaction("waveDone", "wave1", 1.0, "powerOn")],
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

    use crate::scripting::ctx::ScriptCtx;
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
        let script_ctx = ScriptCtx::new();
        let id_a = script_ctx.registry.borrow_mut().spawn(Transform::default());
        let id_b = script_ctx.registry.borrow_mut().spawn(Transform::default());

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
        });

        let reaction_reg = ReactionPrimitiveRegistry::new();
        let chained =
            fire_named_event_with_sequences("go", &data, &seq_reg, &reaction_reg, &script_ctx);
        assert!(chained.is_empty());
        let observed = calls.lock().unwrap().clone();
        assert_eq!(observed, vec![(id_a.to_raw(), 1), (id_b.to_raw(), 2)]);
    }

    #[test]
    fn sequence_dispatch_skips_stale_entity_and_continues() {
        let script_ctx = ScriptCtx::new();
        let id_a = script_ctx.registry.borrow_mut().spawn(Transform::default());
        let id_b = script_ctx.registry.borrow_mut().spawn(Transform::default());

        // Stale ID: reuse a slot that was despawned (mismatched generation).
        let id_dead = script_ctx.registry.borrow_mut().spawn(Transform::default());
        script_ctx.registry.borrow_mut().despawn(id_dead).unwrap();
        assert!(!script_ctx.registry.borrow().exists(id_dead));

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
        });

        let reaction_reg = ReactionPrimitiveRegistry::new();
        fire_named_event_with_sequences("go", &data, &seq_reg, &reaction_reg, &script_ctx);
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn sequence_dispatch_continues_after_handler_error() {
        let script_ctx = ScriptCtx::new();
        let id_a = script_ctx.registry.borrow_mut().spawn(Transform::default());
        let id_b = script_ctx.registry.borrow_mut().spawn(Transform::default());

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
        });

        let reaction_reg = ReactionPrimitiveRegistry::new();
        fire_named_event_with_sequences("go", &data, &seq_reg, &reaction_reg, &script_ctx);
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

    #[test]
    fn validate_sequence_primitives_drops_reaction_when_bad_step_is_at_index_0() {
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
                "invalid_at_zero",
                vec![
                    SequenceStep {
                        id: bogus_id,
                        primitive: "ghost".into(),
                        args: serde_json::Value::Null,
                    },
                    SequenceStep {
                        id: bogus_id,
                        primitive: "known".into(),
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
