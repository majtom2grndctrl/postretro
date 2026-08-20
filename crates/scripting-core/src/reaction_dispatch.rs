// Reaction dispatch: named events and per-tag kill progress.
// See: context/lib/scripting.md §10

use std::collections::{HashMap, VecDeque};

use super::ctx::ScriptCtx;
use super::data_descriptors::{
    EntityTypeDescriptor, NamedReaction, PrimitiveDescriptor, ReactionDescriptor, SequenceStep,
};
use super::data_registry::{DataRegistry, ScopedReaction};
use super::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionCommand, SystemReactionRegistry,
};
use super::registry::{ComponentKind, EntityId, EntityRegistry};
use super::sequence::SequencedPrimitiveRegistry;
use super::slot_table::SlotType;
use postretro_foundation::ir::IrValue;
use serde::Deserialize;

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
    // Ordinal of this body among same-named matches. It is the second component
    // of a scheduler instance key and cannot be reconstructed downstream — the
    // resume path has no `matched` loop — so the enrolling dispatch supplies it.
    // Counts EVERY same-named match (any descriptor kind), matching the index the
    // trigger binder derives from `matched.iter().enumerate()` over the same
    // `data_registry.reactions` order.
    let mut body_ordinal = 0;
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
                    body_ordinal += 1;
                    continue;
                }
                dispatch_primitive(p, reaction_registry, system_registry, script_ctx);
                if let Some(on_complete) = &p.on_complete {
                    chained.push(on_complete.clone());
                }
            }
            ReactionDescriptor::Sequence(steps) => {
                chained.extend(dispatch_sequence(
                    &named.name,
                    body_ordinal,
                    steps,
                    sequence_registry,
                    script_ctx,
                ));
            }
        }
        body_ordinal += 1;
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
                // Address and ordinal ride on the step in Task 3, once
                // `PrepartitionedReactionStep::Descriptor` is widened to carry
                // them; until then this executor has no name in scope. A resumed
                // tail is presentation-only past its wait, so the empty address /
                // zero ordinal is unused for Task 1's landings.
                chained.extend(dispatch_sequence(
                    "",
                    0,
                    steps,
                    sequence_registry,
                    script_ctx,
                ));
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
            | "addSlot"
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
    // Connected clients compose the same descriptor graph for presentation
    // work, but must not make authoritative per-owner state mutations or emit
    // diagnostics for host-only gameplay events.
    if descriptor.primitive == "addSlot" && !script_ctx.owner_slot_writes_enabled.get() {
        return;
    }

    let Some(tag) = descriptor.tag.as_deref() else {
        if descriptor.primitive == "addSlot" {
            log::warn!("[Scripting] addSlot requires a target tag; reaction had no effect");
            return;
        }
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

    if descriptor.primitive == "addSlot" {
        dispatch_add_owner_slot(descriptor, &targets, script_ctx);
        return;
    }

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddSlotArgs {
    slot: String,
    delta: f32,
}

/// Resolve the tagged pawn recipients while a named/crossing/level-load
/// reaction fires, then defer the actual addition to the host app drain. This
/// keeps reaction dispatch independent from the session seat ledger while the
/// drain remains responsible for skipping seats released in the meantime.
fn dispatch_add_owner_slot(
    descriptor: &PrimitiveDescriptor,
    targets: &[EntityId],
    script_ctx: &ScriptCtx,
) {
    let args: AddSlotArgs = match serde_json::from_value::<AddSlotArgs>(descriptor.args.clone()) {
        Ok(args) if args.delta.is_finite() => args,
        Ok(_) => {
            log::warn!("[Scripting] addSlot delta must be finite; reaction had no effect");
            return;
        }
        Err(error) => {
            log::warn!("[Scripting] addSlot has invalid args; reaction had no effect: {error}");
            return;
        }
    };

    {
        let slot_table = script_ctx.slot_table.borrow();
        let Some(record) = slot_table.get(&args.slot) else {
            log::warn!(
                "[Scripting] addSlot references unknown slot `{}`; reaction had no effect",
                args.slot
            );
            return;
        };
        if !record.schema.per_owner {
            log::warn!(
                "[Scripting] addSlot requires per-owner slot `{}`; reaction had no effect",
                args.slot
            );
            return;
        }
        if record.schema.slot_type != SlotType::Number {
            log::warn!(
                "[Scripting] addSlot requires numeric slot `{}`; reaction had no effect",
                args.slot
            );
            return;
        }
        if record.schema.readonly {
            log::warn!(
                "[Scripting] addSlot rejects readonly slot `{}`; reaction had no effect",
                args.slot
            );
            return;
        }
    }

    // A valid descriptor with no matching pawns is a normal no-op. Validate
    // first so malformed named level-load and crossing descriptors cannot
    // survive merely because their current level has no recipients.
    if targets.is_empty() {
        return;
    }

    let seats: Vec<_> = {
        let registry = script_ctx.registry.borrow();
        targets
            .iter()
            .filter_map(|target| match registry.seat_for_pawn(*target) {
                Some(seat) => Some(seat),
                None => {
                    log::warn!(
                        "[Scripting] addSlot target {target:?} has no player seat; skipping"
                    );
                    None
                }
            })
            .collect()
    };
    if seats.is_empty() {
        return;
    }

    script_ctx
        .system_commands
        .push(SystemReactionCommand::AddOwnerSlot {
            slot: args.slot,
            seats,
            delta: args.delta,
        });
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

/// Dispatch a `sequence` body. Returns the `fire`-step event names collected
/// while walking the body (in authored order), which callers extend into their
/// `chained` list for the app-side deferred dispatch hop.
///
/// The control arm sits **ahead of** the entity-target guard: on a `@wait` step
/// it hands the remaining steps, the wait's args, the reaction `address`, and the
/// `body_ordinal` (the first two components of a scheduler instance key, which
/// nothing downstream can reconstruct) to the registered control handler, then
/// `break`s — no step past a wait runs in this drain. On a `@fire` step it
/// collects the target `event` name.
fn dispatch_sequence(
    address: &str,
    body_ordinal: usize,
    steps: &[SequenceStep],
    sequence_registry: &SequencedPrimitiveRegistry,
    script_ctx: &ScriptCtx,
) -> Vec<String> {
    let mut fired = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let id = match step.id {
            postretro_entities::SequenceTarget::Wait => {
                if let Some(control) = sequence_registry.get_control(&step.primitive) {
                    control(address, body_ordinal, &steps[i + 1..], &step.args);
                } else {
                    log::error!(
                        "[Scripting] sequence step {i}: control primitive '{}' has no registered handler; the tail will not run",
                        step.primitive
                    );
                }
                break;
            }
            postretro_entities::SequenceTarget::Fire => {
                match step.args.get("event").and_then(serde_json::Value::as_str) {
                    Some(event) => fired.push(event.to_string()),
                    None => log::warn!(
                        "[Scripting] sequence step {i}: fire step is missing its `event` name; skipping"
                    ),
                }
                continue;
            }
            postretro_entities::SequenceTarget::Entity(id) => id,
            postretro_entities::SequenceTarget::Activators
            | postretro_entities::SequenceTarget::FiredTrigger => {
                log::warn!(
                    "[Scripting] sequence step {i}: sentinel target has no trigger fire context; skipping"
                );
                continue;
            }
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
    fired
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
    use crate::slot_table::{
        NumericRange, ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    };
    use log::Level;
    use postretro_foundation::Seat;
    use postretro_test_log_capture::LogCapture;

    fn per_owner_number_slot(value: f32) -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: SlotType::Number,
            default: Some(SlotValue::Number(value)),
            range: Some(NumericRange {
                min: -10_000.0,
                max: 10_000.0,
            }),
            persist: false,
            readonly: false,
            ownership: SlotOwnership::Mod,
            network: ReplicationScope::None,
            per_owner: true,
            accumulate: None,
        })
    }

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
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        });

        let resolved = resolve_entity_type("grunt", &data);
        assert_eq!(
            resolved,
            Some(&EntityTypeDescriptor {
                canonical_name: Some("grunt".to_string()),
                inventory: None,
                light: None,
                emitter: None,
                movement: None,
                weapon: None,
                touchable: None,
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

    #[test]
    fn add_slot_tag_target_resolves_owner_seat_for_level_load_and_crossing_dispatch() {
        use crate::reaction_registry::SystemReactionCommand;

        let script_ctx = ScriptCtx::new();
        script_ctx
            .slot_table
            .borrow_mut()
            .insert("currency.xp".into(), per_owner_number_slot(0.0))
            .expect("new owner slot");
        let pawn = script_ctx.registry.borrow_mut().spawn(Transform::default());
        {
            let mut registry = script_ctx.registry.borrow_mut();
            registry
                .set_tags(pawn, vec!["players".to_string()])
                .unwrap();
            registry.bind_pawn_seat(pawn, Seat(4));
        }
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![NamedReaction {
                name: "levelLoadOrCrossing".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "addSlot".to_string(),
                    target: None,
                    tag: Some("players".to_string()),
                    on_complete: None,
                    args: serde_json::json!({ "slot": "currency.xp", "delta": 3.0 }),
                }),
            }],
            Vec::new(),
            &[],
        );

        fire_named_event_with_sequences(
            "levelLoadOrCrossing",
            &data,
            &SequencedPrimitiveRegistry::new(),
            &ReactionPrimitiveRegistry::new(),
            &SystemReactionRegistry::new(),
            &script_ctx,
            None,
        );

        assert_eq!(
            script_ctx.system_commands.take(),
            vec![SystemReactionCommand::AddOwnerSlot {
                slot: "currency.xp".to_string(),
                seats: vec![Seat(4)],
                delta: 3.0,
            }],
            "the same tag resolver is used by level-load and crossing named dispatches"
        );
    }

    #[test]
    fn add_slot_zero_recipients_and_client_dispatch_are_silent_no_ops() {
        let script_ctx = ScriptCtx::new();
        script_ctx
            .slot_table
            .borrow_mut()
            .insert("currency.xp".into(), per_owner_number_slot(0.0))
            .expect("new owner slot");
        let descriptor = PrimitiveDescriptor {
            primitive: "addSlot".to_string(),
            target: None,
            tag: Some("no-pawns".to_string()),
            on_complete: None,
            args: serde_json::json!({ "slot": "currency.xp", "delta": 1.0 }),
        };
        let logs = LogCapture::start();
        dispatch_primitive(
            &descriptor,
            &ReactionPrimitiveRegistry::new(),
            &SystemReactionRegistry::new(),
            &script_ctx,
        );
        assert!(script_ctx.system_commands.is_empty());
        assert!(logs.records().is_empty(), "zero recipients must not warn");
        drop(logs);

        script_ctx.owner_slot_writes_enabled.set(false);
        let pawn = script_ctx.registry.borrow_mut().spawn(Transform::default());
        script_ctx
            .registry
            .borrow_mut()
            .set_tags(pawn, vec!["no-pawns".to_string()])
            .unwrap();
        let logs = LogCapture::start();
        dispatch_primitive(
            &descriptor,
            &ReactionPrimitiveRegistry::new(),
            &SystemReactionRegistry::new(),
            &script_ctx,
        );
        assert!(script_ctx.system_commands.is_empty());
        assert!(logs.records().is_empty(), "client addSlot must be silent");
    }

    // Regression: an invalid named addSlot escaped validation when its tag had no matches.
    #[test]
    fn named_add_slot_validates_global_slot_without_recipients_and_runs_sibling_effect() {
        let script_ctx = ScriptCtx::new();
        let mut global_slot = per_owner_number_slot(0.0);
        global_slot.schema.per_owner = false;
        script_ctx
            .slot_table
            .borrow_mut()
            .insert("currency.teamKills".into(), global_slot)
            .expect("new global slot");

        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls_for_handler = std::sync::Arc::clone(&calls);
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
                NamedReaction {
                    name: "levelLoadOrCrossing".to_string(),
                    descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                        primitive: "addSlot".to_string(),
                        target: None,
                        tag: Some("no-pawns".to_string()),
                        on_complete: None,
                        args: serde_json::json!({
                            "slot": "currency.teamKills",
                            "delta": 1.0
                        }),
                    }),
                },
                NamedReaction {
                    name: "levelLoadOrCrossing".to_string(),
                    descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                        primitive: "record".to_string(),
                        target: None,
                        tag: None,
                        on_complete: None,
                        args: serde_json::json!({ "label": "sibling" }),
                    }),
                },
            ],
            Vec::new(),
            &[],
        );

        let logs = LogCapture::start();
        fire_named_event_with_sequences(
            "levelLoadOrCrossing",
            &data,
            &SequencedPrimitiveRegistry::new(),
            &ReactionPrimitiveRegistry::new(),
            &system_registry,
            &script_ctx,
            None,
        );

        logs.assert_logged_once(
            Level::Warn,
            "[Scripting] addSlot requires per-owner slot `currency.teamKills`; reaction had no effect",
        );
        assert_eq!(calls.lock().unwrap().as_slice(), ["sibling".to_string()]);
    }

    #[test]
    fn named_add_slot_with_valid_descriptor_and_zero_recipients_is_silent_and_runs_sibling_effect()
    {
        let script_ctx = ScriptCtx::new();
        script_ctx
            .slot_table
            .borrow_mut()
            .insert("currency.xp".into(), per_owner_number_slot(0.0))
            .expect("new owner slot");

        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls_for_handler = std::sync::Arc::clone(&calls);
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
                NamedReaction {
                    name: "levelLoadOrCrossing".to_string(),
                    descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                        primitive: "addSlot".to_string(),
                        target: None,
                        tag: Some("no-pawns".to_string()),
                        on_complete: None,
                        args: serde_json::json!({ "slot": "currency.xp", "delta": 1.0 }),
                    }),
                },
                NamedReaction {
                    name: "levelLoadOrCrossing".to_string(),
                    descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                        primitive: "record".to_string(),
                        target: None,
                        tag: None,
                        on_complete: None,
                        args: serde_json::json!({ "label": "sibling" }),
                    }),
                },
            ],
            Vec::new(),
            &[],
        );

        let logs = LogCapture::start();
        fire_named_event_with_sequences(
            "levelLoadOrCrossing",
            &data,
            &SequencedPrimitiveRegistry::new(),
            &ReactionPrimitiveRegistry::new(),
            &system_registry,
            &script_ctx,
            None,
        );

        assert!(
            logs.records()
                .iter()
                .all(|record| record.level != Level::Warn),
            "valid zero-recipient addSlot must not warn"
        );
        assert_eq!(calls.lock().unwrap().as_slice(), ["sibling".to_string()]);
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

    // O35: a reaction containing `@wait`/`@fire` control steps survives
    // `setupLevel` validation (`wait`/`fire` are registered names), while a
    // sequence naming an unknown *action* primitive is still dropped.
    #[test]
    fn wait_and_fire_survive_sequence_validation_but_unknown_action_is_dropped() {
        let mut seq_reg = SequencedPrimitiveRegistry::new();
        // Inert admission entries, as the binary registers them.
        seq_reg.register("wait", |_id, _args| Ok(()));
        seq_reg.register("fire", |_id, _args| Ok(()));
        seq_reg.register("setLightAnimation", |_id, _args| Ok(()));

        let bogus_id = EntityId::from_raw(0x0001_0000);
        let reactions = vec![
            sequence_reaction(
                "timedReveal",
                vec![
                    SequenceStep {
                        id: postretro_entities::SequenceTarget::Fire,
                        primitive: "fire".into(),
                        args: serde_json::json!({ "event": "raiseAlarm" }),
                    },
                    SequenceStep {
                        id: postretro_entities::SequenceTarget::Wait,
                        primitive: "wait".into(),
                        args: serde_json::json!({ "durationMs": 800, "interruptible": true }),
                    },
                    SequenceStep {
                        id: bogus_id.into(),
                        primitive: "setLightAnimation".into(),
                        args: serde_json::Value::Null,
                    },
                ],
            ),
            sequence_reaction(
                "bogusAction",
                vec![SequenceStep {
                    id: bogus_id.into(),
                    primitive: "notARegisteredPrimitive".into(),
                    args: serde_json::Value::Null,
                }],
            ),
        ];

        let surviving = validate_sequence_primitives(reactions, &seq_reg);
        assert_eq!(surviving.len(), 1, "only the unknown-action reaction is dropped");
        assert_eq!(surviving[0].name, "timedReveal");
    }

    // O34: firing a named body that contains a wait runs only up to that wait,
    // including at hop depth >= 1 inside a deferred batch. `S = [x, fire(R)]`,
    // `R = [alarm, wait(800), moverStart]`; firing S dispatches R via the
    // deferred hop, R runs `alarm`, hits the `@wait` arm, enrolls the tail, and
    // breaks — `moverStart` never runs in the same drain.
    #[test]
    fn name_fired_body_runs_only_up_to_its_wait_across_a_deferred_hop() {
        let script_ctx = ScriptCtx::new();
        let x = script_ctx.registry.borrow_mut().spawn(Transform::default());
        let alarm = script_ctx.registry.borrow_mut().spawn(Transform::default());
        let mover = script_ctx.registry.borrow_mut().spawn(Transform::default());

        let ran: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
        let enrolled_tail_len: Arc<std::sync::Mutex<Option<usize>>> =
            Arc::new(std::sync::Mutex::new(None));

        let mut seq_reg = SequencedPrimitiveRegistry::new();
        seq_reg.register("wait", |_id, _args| Ok(()));
        seq_reg.register("fire", |_id, _args| Ok(()));
        let ran_note = Arc::clone(&ran);
        seq_reg.register("note", move |_id, args| {
            let label = args
                .get("label")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            ran_note.lock().unwrap().push(label);
            Ok(())
        });
        let captured = Arc::clone(&enrolled_tail_len);
        seq_reg.register_control("wait", move |_address, _ordinal, tail, _args| {
            // Model enrollment: record the tail length; do NOT run it.
            *captured.lock().unwrap() = Some(tail.len());
        });

        let note = |label: &str, id: EntityId| SequenceStep {
            id: id.into(),
            primitive: "note".into(),
            args: serde_json::json!({ "label": label }),
        };
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![
                sequence_reaction(
                    "S",
                    vec![
                        note("x", x),
                        SequenceStep {
                            id: postretro_entities::SequenceTarget::Fire,
                            primitive: "fire".into(),
                            args: serde_json::json!({ "event": "R" }),
                        },
                    ],
                ),
                sequence_reaction(
                    "R",
                    vec![
                        note("alarm", alarm),
                        SequenceStep {
                            id: postretro_entities::SequenceTarget::Wait,
                            primitive: "wait".into(),
                            args: serde_json::json!({ "durationMs": 800 }),
                        },
                        note("moverStart", mover),
                    ],
                ),
            ],
            Vec::new(),
            &[],
        );

        let reaction_reg = ReactionPrimitiveRegistry::new();
        let system_reg = SystemReactionRegistry::new();
        // Fire S: it runs `x`, then its `fire(R)` step is collected as a chained
        // name and dispatched through the deferred batch (hop depth 1).
        let chained = fire_named_event_with_sequences(
            "S",
            &data,
            &seq_reg,
            &reaction_reg,
            &system_reg,
            &script_ctx,
            None,
        );
        assert_eq!(chained, vec!["R".to_string()], "the fire step collects R");
        dispatch_deferred_named_events_with_sequences(
            chained,
            &data,
            &seq_reg,
            &reaction_reg,
            &system_reg,
            &script_ctx,
        );

        assert_eq!(
            ran.lock().unwrap().as_slice(),
            ["x".to_string(), "alarm".to_string()],
            "the body runs only up to the wait; moverStart never runs in this drain"
        );
        assert_eq!(
            *enrolled_tail_len.lock().unwrap(),
            Some(1),
            "the enrolled tail is exactly [moverStart]"
        );
    }
}
