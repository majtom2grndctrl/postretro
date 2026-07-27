// Reaction dispatch: named events and per-tag kill progress.
// See: context/lib/scripting.md §10

use std::collections::{HashMap, VecDeque};

use super::ctx::ScriptCtx;
use super::data_descriptors::{
    EntityTypeDescriptor, NamedReaction, PrimitiveDescriptor, ReactionDescriptor, SequenceStep,
};
use super::data_registry::{DataRegistry, ScopedReaction};
use super::reaction_registry::{ReactionPrimitiveRegistry, SystemReactionRegistry};
use super::registry::{ComponentKind, EntityId, EntityRegistry};
use super::sequence::SequencedPrimitiveRegistry;
use postretro_foundation::ir::IrValue;

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
pub struct ProgressTracker {
    subscriptions: HashMap<String, Vec<ProgressState>>,
}

impl ProgressTracker {
    pub fn new() -> Self {
        Self {
            subscriptions: HashMap::new(),
        }
    }

    /// Callers should `clear()` first to avoid duplicate subscriptions.
    pub fn initialize(&mut self, data_registry: &DataRegistry, entity_registry: &EntityRegistry) {
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

    /// Returns event names to fire; caller passes each name to [`fire_named_event_with_sequences`].
    pub fn on_entity_killed(&mut self, tags: &[String]) -> Vec<String> {
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

    pub fn clear(&mut self) {
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
pub fn fire_named_event(event_name: &str, data_registry: &DataRegistry) -> Vec<String> {
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
                log::debug!(
                    "[Scripting] primitive '{}' matched (tag {:?}); deferred — handlers run only via the sequence-aware drain",
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
pub fn fire_named_event_with_sequences(
    event_name: &str,
    data_registry: &DataRegistry,
    sequence_registry: &SequencedPrimitiveRegistry,
    reaction_registry: &ReactionPrimitiveRegistry,
    system_registry: &SystemReactionRegistry,
    script_ctx: &ScriptCtx,
    dispatch_context: Option<NamedEventDispatchContext<'_>>,
) -> Vec<String> {
    let source = dispatch_context.as_ref().map_or_else(
        || format!("named:{event_name}"),
        |context| context.source.clone(),
    );
    let values = dispatch_context
        .map(|context| context.values.to_vec())
        .unwrap_or_default();
    let previous_context = script_ctx
        .system_commands
        .replace_fire_context(postretro_entities::SystemCommandFireContext { source, values });
    let mut chained = Vec::new();
    for named in &data_registry.reactions {
        if named.name != event_name {
            continue;
        }
        match &named.descriptor {
            ReactionDescriptor::Progress(_) => {}
            ReactionDescriptor::Primitive(p) => {
                if p.target.is_some() {
                    log::warn!(
                        "[Scripting] named dispatch `{event_name}` has no trigger fire context for sentinel target; skipping primitive"
                    );
                    continue;
                }
                dispatch_primitive(p, reaction_registry, system_registry, script_ctx);
                if let Some(on_complete) = &p.on_complete {
                    chained.push(on_complete.clone());
                }
            }
            ReactionDescriptor::Sequence(steps) => {
                dispatch_sequence(steps, sequence_registry, script_ctx);
            }
        }
    }
    script_ctx
        .system_commands
        .replace_fire_context(previous_context);
    chained
}

/// Explicit per-fire context for sources that publish ephemeral dispatch
/// inputs. Ordinary named events derive their source identity from the event
/// name and pass `None`.
pub struct NamedEventDispatchContext<'a> {
    pub source: String,
    pub values: &'a [(String, IrValue)],
}

/// One ordered item in a trigger residual. A descriptor is already resolved and
/// partitioned at install time; a deferred event marks the later dispatch hop
/// following a consequential step that ran in the fixed tick.
#[derive(Debug, Clone)]
pub enum PrepartitionedReactionStep {
    Descriptor(ReactionDescriptor),
    DeferredEvent(String),
}

/// Execute steps resolved and partitioned earlier, without a reaction-name
/// lookup. Trigger residuals use this after their consequential commands have
/// already executed in the fixed simulation tick. Returns named work for the
/// next app-side dispatch hop in the residual's authored composition order.
pub fn fire_prepartitioned_reactions_with_sequences(
    steps: &[PrepartitionedReactionStep],
    sequence_registry: &SequencedPrimitiveRegistry,
    reaction_registry: &ReactionPrimitiveRegistry,
    system_registry: &SystemReactionRegistry,
    script_ctx: &ScriptCtx,
) -> Vec<String> {
    let mut chained = Vec::new();
    for step in steps {
        match step {
            PrepartitionedReactionStep::DeferredEvent(event_name) => {
                chained.push(event_name.clone());
            }
            PrepartitionedReactionStep::Descriptor(ReactionDescriptor::Progress(_)) => {
                // Tracked independently via ProgressTracker; no-op here prevents double-fire.
                // The tracker owns the completion target and fires it once `killed/total >= at`.
                // Pushing `progress.fire` here would fire that target immediately — with zero
                // kills — and then again at the real threshold.
            }
            PrepartitionedReactionStep::Descriptor(ReactionDescriptor::Primitive(primitive)) => {
                #[cfg(debug_assertions)]
                debug_assert!(
                    !is_trigger_consequential_primitive(&primitive.primitive),
                    "trigger residual contains consequential primitive `{}`; binding must execute it in the fixed tick",
                    primitive.primitive,
                );
                dispatch_primitive(primitive, reaction_registry, system_registry, script_ctx);
                if let Some(on_complete) = &primitive.on_complete {
                    chained.push(on_complete.clone());
                }
            }
            PrepartitionedReactionStep::Descriptor(ReactionDescriptor::Sequence(steps)) => {
                #[cfg(debug_assertions)]
                debug_assert!(
                    steps
                        .iter()
                        .all(|step| !is_trigger_consequential_primitive(&step.primitive)),
                    "trigger residual contains a consequential sequence step; binding must execute it in the fixed tick",
                );
                dispatch_sequence(steps, sequence_registry, script_ctx);
            }
        }
    }
    chained
}

/// The trigger app-frame drain supplies every same-frame residual root in one
/// batch. Follow-ups dispatch breadth-first across that batch, with one shared
/// 256-hop cap: this is deliberately a cycle breaker, not a per-root delivery
/// budget.
/// It bounds malformed duplicate-name graphs without changing FIFO order among
/// the work that fits below the cap.
pub fn dispatch_deferred_named_events_with_sequences(
    initial_events: impl IntoIterator<Item = String>,
    data_registry: &DataRegistry,
    sequence_registry: &SequencedPrimitiveRegistry,
    reaction_registry: &ReactionPrimitiveRegistry,
    system_registry: &SystemReactionRegistry,
    script_ctx: &ScriptCtx,
) {
    const MAX_BATCH_DISPATCH_HOPS: usize = 256;
    let _ = dispatch_deferred_named_events_with_sequences_up_to(
        initial_events,
        data_registry,
        sequence_registry,
        reaction_registry,
        system_registry,
        script_ctx,
        MAX_BATCH_DISPATCH_HOPS,
    );
}

fn dispatch_deferred_named_events_with_sequences_up_to(
    initial_events: impl IntoIterator<Item = String>,
    data_registry: &DataRegistry,
    sequence_registry: &SequencedPrimitiveRegistry,
    reaction_registry: &ReactionPrimitiveRegistry,
    system_registry: &SystemReactionRegistry,
    script_ctx: &ScriptCtx,
    max_dispatch_hops: usize,
) -> usize {
    let mut pending: VecDeque<String> = initial_events.into_iter().collect();
    let mut dispatched = 0;
    while let Some(event_name) = pending.pop_front() {
        if dispatched == max_dispatch_hops {
            log::warn!(
                "[Scripting] deferred reaction dispatch reached the {max_dispatch_hops}-hop aggregate batch cap; dropping {} queued event(s)",
                pending.len() + 1,
            );
            break;
        }
        dispatched += 1;
        if !data_registry
            .reactions
            .iter()
            .any(|reaction| reaction.name == event_name)
        {
            log::warn!(
                "[Scripting] deferred reaction event `{event_name}` does not match an active composed reaction; skipping"
            );
            continue;
        }
        pending.extend(fire_named_event_with_sequences(
            &event_name,
            data_registry,
            sequence_registry,
            reaction_registry,
            system_registry,
            script_ctx,
            None,
        ));
    }
    dispatched
}

/// Mirrors the trigger binder's closed fixed-tick command set. This assertion
/// lives at the residual executor boundary so a future partitioning path cannot
/// silently run consequential work twice. It is debug-only because validated
/// level-install bindings are the release contract.
#[cfg(debug_assertions)]
fn is_trigger_consequential_primitive(primitive: &str) -> bool {
    matches!(
        primitive,
        "moverStart"
            | "moverStop"
            | "moverReverse"
            | "moverGoToPathNode"
            | "moverSetSpinRate"
            | "applyDamage"
            | "armTrigger"
            | "disarmTrigger"
            | "setState"
            | "setAnimationState"
            | "updateEnemyState"
            | "spawnFromSpawner"
    )
}

/// Routes a `Primitive` descriptor to one of two execution arms (M13 HUD
/// dynamics): a `Some(tag)` resolves entities and runs the entity-targeted
/// `ReactionPrimitiveRegistry`; a `None` tag is a system reaction, dispatched
/// against the `SystemReactionRegistry`, which enqueues a typed command onto
/// `ScriptCtx::system_commands` for the app's per-frame drain. Both arms share
/// the one named-event vocabulary.
fn dispatch_primitive(
    descriptor: &PrimitiveDescriptor,
    reaction_registry: &ReactionPrimitiveRegistry,
    system_registry: &SystemReactionRegistry,
    script_ctx: &ScriptCtx,
) {
    let Some(tag) = descriptor.tag.as_deref() else {
        dispatch_system_primitive(descriptor, system_registry, script_ctx);
        return;
    };

    // Targeting walks the Transform column per the invariant in
    // `count_entities_with_tag`. Empty target sets are passed through; handlers
    // decide whether to warn.
    let targets: Vec<EntityId> = {
        let reg = script_ctx.registry.borrow();
        reg.query_by_component_and_tag(ComponentKind::Transform, Some(tag))
            .map(|(id, _)| id)
            .collect()
    };

    log::info!(
        "[Scripting] dispatch primitive '{}' on tag '{}' ({} targets)",
        descriptor.primitive,
        tag,
        targets.len(),
    );

    let mut reg = script_ctx.registry.borrow_mut();
    match reaction_registry.dispatch_tagged(
        &descriptor.primitive,
        &mut reg,
        tag,
        &targets,
        &descriptor.args,
    ) {
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

/// System-reaction arm: no entity targets. The handler parses `args` and
/// enqueues a typed command; the app drains the queue once per frame.
fn dispatch_system_primitive(
    descriptor: &PrimitiveDescriptor,
    system_registry: &SystemReactionRegistry,
    script_ctx: &ScriptCtx,
) {
    log::info!(
        "[Scripting] dispatch system reaction '{}'",
        descriptor.primitive,
    );

    match system_registry.dispatch(
        &descriptor.primitive,
        &descriptor.args,
        &script_ctx.system_commands,
    ) {
        Ok(true) => {}
        Ok(false) => log::warn!(
            "[Scripting] system reaction '{}' is not registered; reaction had no effect",
            descriptor.primitive,
        ),
        Err(e) => log::warn!(
            "[Scripting] system reaction '{}' dispatch failed: {e:?}",
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
        let postretro_entities::SequenceTarget::Entity(id) = step.id else {
            log::warn!(
                "[Scripting] sequence step {i}: sentinel target has no trigger fire context; skipping"
            );
            continue;
        };
        if !script_ctx.registry.borrow().exists(id) {
            log::warn!(
                "[Scripting] sequence step {i}: entity {:?} not found, skipping",
                id
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
        if let Err(e) = handler(id, &step.args) {
            log::warn!(
                "[Scripting] sequence step {i}: primitive '{}' on entity {:?} failed: {e}",
                step.primitive,
                id
            );
        }
    }
}

/// Called at `setupLevel()` time, before reactions land in [`DataRegistry`].
/// Drops any `Sequence` reaction whose steps name an unknown primitive; logs an error per rejection.
pub fn validate_sequence_primitives(
    reactions: Vec<NamedReaction>,
    sequence_registry: &SequencedPrimitiveRegistry,
) -> Vec<NamedReaction> {
    reactions
        .into_iter()
        .filter(|named| sequence_primitives_are_valid(named, sequence_registry, "setupLevel"))
        .collect()
}

/// Called before `ModManifest.reactions` land in durable global storage.
/// Preserves each surviving reaction's level scope.
pub fn validate_scoped_sequence_primitives(
    reactions: Vec<ScopedReaction>,
    sequence_registry: &SequencedPrimitiveRegistry,
) -> Vec<ScopedReaction> {
    reactions
        .into_iter()
        .filter(|scoped| {
            sequence_primitives_are_valid(
                &scoped.reaction,
                sequence_registry,
                "ModManifest.reactions",
            )
        })
        .collect()
}

fn sequence_primitives_are_valid(
    named: &NamedReaction,
    sequence_registry: &SequencedPrimitiveRegistry,
    source: &str,
) -> bool {
    let ReactionDescriptor::Sequence(steps) = &named.descriptor else {
        return true;
    };
    for (i, step) in steps.iter().enumerate() {
        if !sequence_registry.contains(&step.primitive) {
            log::error!(
                "[Scripting] {source}: sequence step {i} names unknown primitive \"{}\"",
                step.primitive
            );
            return false;
        }
    }
    true
}

/// Linear scan — entity-type counts per level are small and this runs at instantiation time, not in a hot loop.
pub fn resolve_entity_type<'a>(
    classname: &str,
    data_registry: &'a DataRegistry,
) -> Option<&'a EntityTypeDescriptor> {
    data_registry
        .entities
        .iter()
        .find(|e| e.canonical_name.as_deref() == Some(classname))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_descriptors::{
        EntityTypeDescriptor, NamedReaction, PrimitiveDescriptor, ProgressDescriptor,
        ReactionDescriptor,
    };
    use crate::registry::{EntityRegistry, Transform};

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
                target: None,
                tag: Some(tag.to_string()),
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

    #[cfg(debug_assertions)]
    #[test]
    fn update_enemy_state_stays_in_trigger_consequential_mirror() {
        assert!(is_trigger_consequential_primitive("updateEnemyState"));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn spawn_from_spawner_stays_in_trigger_consequential_mirror() {
        assert!(is_trigger_consequential_primitive("spawnFromSpawner"));
    }

    #[test]
    fn progress_threshold_fires_when_all_dead_at_full_ratio() {
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![progress_reaction("waveDone", "wave1", 1.0, "powerOn")],
            Vec::new(),
            &[],
        );

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
        data.populate_level(
            vec![progress_reaction("waveDone", "wave1", 1.0, "powerOn")],
            Vec::new(),
            &[],
        );

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
        data.populate_level(
            vec![progress_reaction("half", "wave1", 0.5, "midwave")],
            Vec::new(),
            &[],
        );

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
        data.populate_level(
            vec![
                progress_reaction("waveDone", "wave1", 1.0, "powerOn"),
                progress_reaction("reactorDown", "reactorMonster", 1.0, "reactorOff"),
            ],
            Vec::new(),
            &[],
        );

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
        data.populate_level(
            vec![
                progress_reaction("waveDone", "wave1", 0.5, "powerOn"),
                progress_reaction("reactorDown", "reactorMonster", 0.5, "reactorOff"),
            ],
            Vec::new(),
            &[],
        );

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
        data.populate_level(
            vec![progress_reaction("waveDone", "wave1", 1.0, "powerOn")],
            Vec::new(),
            &[],
        );
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
        data.populate_level(
            vec![progress_reaction("waveDone", "ghosts", 1.0, "spooky")],
            Vec::new(),
            &[],
        );
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
            canonical_name: Some("grunt".to_string()),
            default_weapon: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            mesh: None,
            health: None,
            behavior: None,
        });

        let resolved = resolve_entity_type("grunt", &data);
        assert_eq!(
            resolved,
            Some(&EntityTypeDescriptor {
                canonical_name: Some("grunt".to_string()),
                default_weapon: None,
                light: None,
                emitter: None,
                movement: None,
                weapon: None,
                mesh: None,
                health: None,
                behavior: None,
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
        data.populate_level(
            vec![primitive_reaction(
                "wave1Complete",
                "moveGeometry",
                "reactorChambers",
                Some("wave2Revealed"),
            )],
            Vec::new(),
            &[],
        );

        let chained = fire_named_event("wave1Complete", &data);
        assert_eq!(chained, vec!["wave2Revealed".to_string()]);
    }

    #[test]
    fn fire_named_event_on_primitive_without_on_complete_returns_empty() {
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![primitive_reaction(
                "wave2Revealed",
                "activateGroup",
                "reactorWave2Monsters",
                None,
            )],
            Vec::new(),
            &[],
        );

        let chained = fire_named_event("wave2Revealed", &data);
        assert!(chained.is_empty());
    }

    #[test]
    fn fire_named_event_on_progress_is_a_noop() {
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![progress_reaction("waveDone", "wave1", 1.0, "powerOn")],
            Vec::new(),
            &[],
        );
        let chained = fire_named_event("waveDone", &data);
        assert!(chained.is_empty());
    }

    #[test]
    fn fire_named_event_unknown_name_returns_empty() {
        let data = DataRegistry::new();
        let chained = fire_named_event("nothingHere", &data);
        assert!(chained.is_empty());
    }

    // A trigger-bound Progress reaction means "the tracker watches this tag", not
    // "fire the target now". Firing it from the residual would double-fire the
    // target — once on the trigger with zero kills, once again at the threshold.
    #[test]
    fn prepartitioned_progress_is_a_noop_and_yields_no_follow_up() {
        let script_ctx = ScriptCtx::new();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls_for_handler = Arc::clone(&calls);
        let mut system_registry = SystemReactionRegistry::new();
        system_registry.register("record", move |args, _queue| {
            calls_for_handler.lock().unwrap().push(
                args.get("label")
                    .and_then(serde_json::Value::as_str)
                    .unwrap()
                    .to_string(),
            );
            Ok(())
        });

        let progress = ProgressDescriptor {
            tag: "wave".into(),
            at: 1.0,
            fire: "release".into(),
        };
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![NamedReaction {
                name: "release".into(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "record".into(),
                    target: None,
                    tag: None,
                    on_complete: None,
                    args: serde_json::json!({ "label": "release" }),
                }),
            }],
            Vec::new(),
            &[],
        );

        let sequence_registry = SequencedPrimitiveRegistry::new();
        let reaction_registry = ReactionPrimitiveRegistry::new();
        let follow_ups = fire_prepartitioned_reactions_with_sequences(
            &[PrepartitionedReactionStep::Descriptor(
                ReactionDescriptor::Progress(progress),
            )],
            &sequence_registry,
            &reaction_registry,
            &system_registry,
            &script_ctx,
        );
        assert!(
            follow_ups.is_empty(),
            "a prepartitioned Progress descriptor must not queue its fire target",
        );

        dispatch_deferred_named_events_with_sequences(
            follow_ups,
            &data,
            &sequence_registry,
            &reaction_registry,
            &system_registry,
            &script_ctx,
        );
        assert!(
            calls.lock().unwrap().is_empty(),
            "the Progress target must not execute on the app drain",
        );
    }

    // Companion to the test above: no-oping the residual must not break the real
    // progress path — the tracker still owns and fires the target at threshold.
    #[test]
    fn progress_tracker_still_fires_its_target_at_threshold() {
        let script_ctx = ScriptCtx::new();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls_for_handler = Arc::clone(&calls);
        let mut system_registry = SystemReactionRegistry::new();
        system_registry.register("record", move |args, _queue| {
            calls_for_handler.lock().unwrap().push(
                args.get("label")
                    .and_then(serde_json::Value::as_str)
                    .unwrap()
                    .to_string(),
            );
            Ok(())
        });

        let mut data = DataRegistry::new();
        data.populate_level(
            vec![
                progress_reaction("waveDone", "wave", 1.0, "release"),
                NamedReaction {
                    name: "release".into(),
                    descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                        primitive: "record".into(),
                        target: None,
                        tag: None,
                        on_complete: None,
                        args: serde_json::json!({ "label": "release" }),
                    }),
                },
            ],
            Vec::new(),
            &[],
        );

        let mut entities = EntityRegistry::new();
        spawn_with_tags(&mut entities, &["wave"]);
        spawn_with_tags(&mut entities, &["wave"]);

        let mut tracker = ProgressTracker::new();
        tracker.initialize(&data, &entities);

        let sequence_registry = SequencedPrimitiveRegistry::new();
        let reaction_registry = ReactionPrimitiveRegistry::new();

        let fired = tracker.on_entity_killed(&["wave".to_string()]);
        assert!(fired.is_empty(), "half the wave is not the threshold");

        let fired = tracker.on_entity_killed(&["wave".to_string()]);
        assert_eq!(fired, vec!["release".to_string()]);

        dispatch_deferred_named_events_with_sequences(
            fired,
            &data,
            &sequence_registry,
            &reaction_registry,
            &system_registry,
            &script_ctx,
        );
        assert_eq!(calls.lock().unwrap().as_slice(), ["release".to_string()]);
    }

    #[test]
    fn deferred_named_events_are_breadth_first_and_batch_hop_bounded() {
        let script_ctx = ScriptCtx::new();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls_for_handler = Arc::clone(&calls);
        let mut system_registry = SystemReactionRegistry::new();
        system_registry.register("record", move |args, _queue| {
            calls_for_handler.lock().unwrap().push(
                args.get("label")
                    .and_then(serde_json::Value::as_str)
                    .unwrap()
                    .to_string(),
            );
            Ok(())
        });

        let named = |name: &str, on_complete: Option<&str>| NamedReaction {
            name: name.into(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "record".into(),
                target: None,
                tag: None,
                on_complete: on_complete.map(str::to_string),
                args: serde_json::json!({ "label": name }),
            }),
        };
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![
                named("first", Some("after_first")),
                named("second", Some("after_second")),
                named("after_first", Some("first")),
                named("after_second", None),
            ],
            Vec::new(),
            &[],
        );

        let sequence_registry = SequencedPrimitiveRegistry::new();
        let reaction_registry = ReactionPrimitiveRegistry::new();
        let dispatched = dispatch_deferred_named_events_with_sequences_up_to(
            ["first".to_string(), "second".to_string()],
            &data,
            &sequence_registry,
            &reaction_registry,
            &system_registry,
            &script_ctx,
            3,
        );

        assert_eq!(dispatched, 3);
        assert_eq!(
            calls.lock().unwrap().as_slice(),
            [
                "first".to_string(),
                "second".to_string(),
                "after_first".to_string(),
            ],
            "the shared batch cap applies after FIFO drains both roots, then the first follow-up"
        );
    }

    use crate::ctx::ScriptCtx;
    use crate::data_descriptors::SequenceStep;
    use crate::data_registry::ScopedReaction;
    use crate::registry::EntityId;
    use crate::sequence::{SequenceError, SequencedPrimitiveRegistry};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn sequence_reaction(name: &str, steps: Vec<SequenceStep>) -> NamedReaction {
        NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Sequence(steps),
        }
    }

    // A system reaction (no `tag`) fired through the SAME `fire_named_event`
    // path an entity event uses resolves through the shared vocabulary and
    // enqueues a typed command onto the queue — one namespace, two arms.
    #[test]
    fn system_reaction_fired_by_named_event_enqueues_command() {
        use crate::reaction_registry::SystemReactionCommand;

        let script_ctx = ScriptCtx::new();

        let mut data = DataRegistry::new();
        data.populate_level(
            vec![NamedReaction {
                name: "lowHealth".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "playSound".to_string(),
                    target: None,
                    // No tag ⇒ system-targeted.
                    tag: None,
                    on_complete: None,
                    args: serde_json::json!({ "sound": "alarm", "bus": "sfx" }),
                }),
            }],
            Vec::new(),
            &[],
        );

        let seq_reg = SequencedPrimitiveRegistry::new();
        let reaction_reg = ReactionPrimitiveRegistry::new();
        let mut system_reg = SystemReactionRegistry::new();
        system_reg.register("playSound", |args, queue| {
            let sound = args
                .get("sound")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let bus = args
                .get("bus")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            queue.push(SystemReactionCommand::PlaySound { sound, bus });
            Ok(())
        });

        assert!(script_ctx.system_commands.is_empty());
        fire_named_event_with_sequences(
            "lowHealth",
            &data,
            &seq_reg,
            &reaction_reg,
            &system_reg,
            &script_ctx,
            None,
        );

        assert_eq!(
            script_ctx.system_commands.take(),
            vec![SystemReactionCommand::PlaySound {
                sound: "alarm".to_string(),
                bus: Some("sfx".to_string()),
            }]
        );
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
        data.populate_level(
            vec![sequence_reaction(
                "go",
                vec![
                    SequenceStep {
                        id: id_a.into(),
                        primitive: "noteValue".into(),
                        args: serde_json::json!({ "v": 1 }),
                    },
                    SequenceStep {
                        id: id_b.into(),
                        primitive: "noteValue".into(),
                        args: serde_json::json!({ "v": 2 }),
                    },
                ],
            )],
            Vec::new(),
            &[],
        );

        let reaction_reg = ReactionPrimitiveRegistry::new();
        let system_reg = SystemReactionRegistry::new();
        let chained = fire_named_event_with_sequences(
            "go",
            &data,
            &seq_reg,
            &reaction_reg,
            &system_reg,
            &script_ctx,
            None,
        );
        assert!(chained.is_empty());
        let observed = calls.lock().unwrap().clone();
        assert_eq!(observed, vec![(id_a.to_raw(), 1), (id_b.to_raw(), 2)]);
    }

    // AC 10: a NAMED (non-trigger) dispatch has no fire context, so a primitive
    // carrying a sentinel `target` cannot resolve — it warns and is skipped,
    // while a sibling sentinel-free reaction on the same event name still runs.
    #[test]
    fn named_dispatch_skips_sentinel_target_primitive_but_runs_sentinel_free_command() {
        use crate::reaction_registry::SystemReactionCommand;

        let script_ctx = ScriptCtx::new();
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![
                // Sentinel target with no trigger fire context: must warn-skip.
                NamedReaction {
                    name: "onPress".to_string(),
                    descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                        primitive: "applyDamage".to_string(),
                        target: Some("@activators".to_string()),
                        tag: None,
                        on_complete: None,
                        args: serde_json::json!({ "amount": 25 }),
                    }),
                },
                // Sentinel-free system reaction on the same event name: must run.
                NamedReaction {
                    name: "onPress".to_string(),
                    descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                        primitive: "playSound".to_string(),
                        target: None,
                        tag: None,
                        on_complete: None,
                        args: serde_json::json!({ "sound": "beep", "bus": "sfx" }),
                    }),
                },
            ],
            Vec::new(),
            &[],
        );

        let seq_reg = SequencedPrimitiveRegistry::new();
        let reaction_reg = ReactionPrimitiveRegistry::new();
        let mut system_reg = SystemReactionRegistry::new();
        system_reg.register("playSound", |args, queue| {
            let sound = args
                .get("sound")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let bus = args
                .get("bus")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            queue.push(SystemReactionCommand::PlaySound { sound, bus });
            Ok(())
        });

        fire_named_event_with_sequences(
            "onPress",
            &data,
            &seq_reg,
            &reaction_reg,
            &system_reg,
            &script_ctx,
            None,
        );

        assert_eq!(
            script_ctx.system_commands.take(),
            vec![SystemReactionCommand::PlaySound {
                sound: "beep".to_string(),
                bus: Some("sfx".to_string()),
            }],
            "the sentinel-target primitive is skipped; only the sentinel-free command runs",
        );
    }

    // AC 10: the symmetric sequence path — a named dispatch's sequence step
    // carrying a sentinel `id` has no fire context to resolve, so it warns and
    // is skipped, while the sequence's entity-targeted step still executes.
    #[test]
    fn named_dispatch_skips_sentinel_sequence_step_but_runs_entity_step() {
        let script_ctx = ScriptCtx::new();
        let id_entity = script_ctx.registry.borrow_mut().spawn(Transform::default());

        let calls: Arc<std::sync::Mutex<Vec<(u32, i64)>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls_cl = Arc::clone(&calls);
        let mut seq_reg = SequencedPrimitiveRegistry::new();
        seq_reg.register("noteValue", move |id, args| {
            let v = args["v"].as_i64().unwrap_or(-1);
            calls_cl.lock().unwrap().push((id.to_raw(), v));
            Ok(())
        });

        let mut data = DataRegistry::new();
        data.populate_level(
            vec![sequence_reaction(
                "onComplete",
                vec![
                    SequenceStep {
                        id: postretro_entities::SequenceTarget::Activators,
                        primitive: "noteValue".into(),
                        args: serde_json::json!({ "v": 1 }),
                    },
                    SequenceStep {
                        id: id_entity.into(),
                        primitive: "noteValue".into(),
                        args: serde_json::json!({ "v": 2 }),
                    },
                ],
            )],
            Vec::new(),
            &[],
        );

        let reaction_reg = ReactionPrimitiveRegistry::new();
        let system_reg = SystemReactionRegistry::new();
        fire_named_event_with_sequences(
            "onComplete",
            &data,
            &seq_reg,
            &reaction_reg,
            &system_reg,
            &script_ctx,
            None,
        );

        assert_eq!(
            calls.lock().unwrap().clone(),
            vec![(id_entity.to_raw(), 2)],
            "the sentinel step is skipped; the entity-targeted step still runs",
        );
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
        data.populate_level(
            vec![sequence_reaction(
                "go",
                vec![
                    SequenceStep {
                        id: id_a.into(),
                        primitive: "tick".into(),
                        args: serde_json::Value::Null,
                    },
                    SequenceStep {
                        id: id_dead.into(),
                        primitive: "tick".into(),
                        args: serde_json::Value::Null,
                    },
                    SequenceStep {
                        id: id_b.into(),
                        primitive: "tick".into(),
                        args: serde_json::Value::Null,
                    },
                ],
            )],
            Vec::new(),
            &[],
        );

        let reaction_reg = ReactionPrimitiveRegistry::new();
        let system_reg = SystemReactionRegistry::new();
        fire_named_event_with_sequences(
            "go",
            &data,
            &seq_reg,
            &reaction_reg,
            &system_reg,
            &script_ctx,
            None,
        );
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
        data.populate_level(
            vec![sequence_reaction(
                "go",
                vec![
                    SequenceStep {
                        id: id_a.into(),
                        primitive: "boom".into(),
                        args: serde_json::Value::Null,
                    },
                    SequenceStep {
                        id: id_b.into(),
                        primitive: "ok".into(),
                        args: serde_json::Value::Null,
                    },
                ],
            )],
            Vec::new(),
            &[],
        );

        let reaction_reg = ReactionPrimitiveRegistry::new();
        let system_reg = SystemReactionRegistry::new();
        fire_named_event_with_sequences(
            "go",
            &data,
            &seq_reg,
            &reaction_reg,
            &system_reg,
            &script_ctx,
            None,
        );
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
                    id: bogus_id.into(),
                    primitive: "known".into(),
                    args: serde_json::Value::Null,
                }],
            ),
            sequence_reaction(
                "invalid",
                vec![
                    SequenceStep {
                        id: bogus_id.into(),
                        primitive: "known".into(),
                        args: serde_json::Value::Null,
                    },
                    SequenceStep {
                        id: bogus_id.into(),
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
                    id: bogus_id.into(),
                    primitive: "known".into(),
                    args: serde_json::Value::Null,
                }],
            ),
            sequence_reaction(
                "invalid_at_zero",
                vec![
                    SequenceStep {
                        id: bogus_id.into(),
                        primitive: "ghost".into(),
                        args: serde_json::Value::Null,
                    },
                    SequenceStep {
                        id: bogus_id.into(),
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

    #[test]
    fn validate_scoped_sequence_primitives_drops_invalid_sequences_and_preserves_levels() {
        let mut seq_reg = SequencedPrimitiveRegistry::new();
        seq_reg.register("known", |_id, _args| Ok(()));

        let bogus_id = EntityId::from_raw(0x0001_0000);
        let reactions = vec![
            ScopedReaction {
                reaction: primitive_reaction("non_sequence", "moveGeometry", "reactor", None),
                levels: vec!["campaign".to_string()],
            },
            ScopedReaction {
                reaction: sequence_reaction(
                    "valid_sequence",
                    vec![SequenceStep {
                        id: bogus_id.into(),
                        primitive: "known".into(),
                        args: serde_json::Value::Null,
                    }],
                ),
                levels: vec!["campaign".to_string(), "boss".to_string()],
            },
            ScopedReaction {
                reaction: sequence_reaction(
                    "invalid_sequence",
                    vec![SequenceStep {
                        id: bogus_id.into(),
                        primitive: "ghost".into(),
                        args: serde_json::Value::Null,
                    }],
                ),
                levels: vec!["campaign".to_string()],
            },
        ];

        let surviving = validate_scoped_sequence_primitives(reactions, &seq_reg);

        assert_eq!(surviving.len(), 2);
        assert_eq!(surviving[0].reaction.name, "non_sequence");
        assert_eq!(surviving[0].levels, vec!["campaign"]);
        assert_eq!(surviving[1].reaction.name, "valid_sequence");
        assert_eq!(surviving[1].levels, vec!["campaign", "boss"]);
    }
}
