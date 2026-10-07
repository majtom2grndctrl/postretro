//! Partition a trigger-bound reaction into fixed-tick commands and an ordered
//! app-drain residual, with install-time warnings for deferred consequential work.
//! See: context/lib/scripting.md §12

use std::collections::HashSet;

use postretro_entities::{ScriptCtx, SlotTable};
use postretro_scripting_core::data_descriptors::{
    NamedReaction, ProgressDescriptor, ReactionDescriptor, SequenceTarget,
};
use postretro_scripting_core::data_registry::DataRegistry;
use postretro_scripting_core::reaction_dispatch::PrepartitionedReactionStep;

use super::command_binding::{bind_primitive, bind_sequence_step};
use crate::trigger_commands::BoundTriggerCommand;

const CONSEQUENTIAL_PRIMITIVES: &[&str] = &[
    "moverStart",
    "moverStop",
    "moverReverse",
    "moverGoToPathNode",
    "moverSetSpinRate",
    "applyDamage",
    "grantHealth",
    "grantAmmo",
    "armTrigger",
    "disarmTrigger",
    "setState",
    "addSlot",
    "setAnimationState",
    "updateEnemyState",
    "spawnFromSpawner",
];

const LIFECYCLE_PRIMITIVES: &[&str] = &["loadLevel", "restartLevel", "returnToFrontend"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrimitiveClass {
    Consequential,
    Lifecycle,
    Presentation,
}

/// Keep only directly-owned work in the binding. `onComplete` names remain
/// ordered residual hops, so their graphs resolve when the app drains rather
/// than flattening recursively at level install.
pub(super) fn partition_direct_reaction(
    reaction: &NamedReaction,
    body_ordinal: usize,
    data_registry: &DataRegistry,
    slot_table: &SlotTable,
    script_ctx: Option<&ScriptCtx>,
    commands: &mut Vec<BoundTriggerCommand>,
    steps: &mut Vec<PrepartitionedReactionStep>,
) {
    match &reaction.descriptor {
        ReactionDescriptor::Progress(progress) => {
            // No residual entry: `ProgressTracker` already subscribes every Progress
            // reaction in the DataRegistry and fires its target once the kill threshold
            // is met. Binding it to a trigger arms the tracker's watch — it does not give
            // the trigger a copy to fire. Retaining a residual descriptor here would
            // double-fire the target (and skip the threshold on the trigger's copy).
            warn_for_progress_target(&reaction.name, progress, data_registry);
        }
        ReactionDescriptor::Primitive(primitive) => {
            if classify(&primitive.primitive) == PrimitiveClass::Consequential {
                if let Some(command) = bind_primitive(primitive, slot_table, script_ctx) {
                    commands.push(command);
                }
                if let Some(on_complete) = &primitive.on_complete {
                    warn_for_deferred_event(&reaction.name, on_complete, data_registry);
                    steps.push(PrepartitionedReactionStep::DeferredEvent(
                        on_complete.clone(),
                    ));
                }
            } else {
                if primitive.target.is_some() {
                    log::warn!(
                        "[Trigger] sentinel target on non-consequential primitive `{}` cannot drain app-side; not binding",
                        primitive.primitive
                    );
                    return;
                }
                if let Some(on_complete) = &primitive.on_complete {
                    warn_for_deferred_event(&reaction.name, on_complete, data_registry);
                }
                steps.push(PrepartitionedReactionStep::Descriptor(
                    reaction.name.clone(),
                    body_ordinal,
                    ReactionDescriptor::Primitive(primitive.clone()),
                ));
            }
        }
        ReactionDescriptor::Sequence(sequence) => {
            // Stop at the first `Wait`. Steps before it partition exactly as a
            // trigger-bound body does today; the wait plus every step after it
            // goes to the residual in authored order, UNFILTERED by class or
            // target. The residual drains at frame end through `dispatch_sequence`,
            // whose control arm meets the wait, enrolls the tail with the
            // scheduler, and stops — restoring the funnel a trigger-bound body
            // otherwise bypasses, and delivering the E18-A guarantee that a
            // pre-wait consequential step still runs in-tick (O49, O50).
            let wait_index = sequence
                .iter()
                .position(|step| matches!(step.id, SequenceTarget::Wait));
            let pre_wait = &sequence[..wait_index.unwrap_or(sequence.len())];
            let mut residual_steps = Vec::new();
            for step in pre_wait {
                if matches!(step.id, SequenceTarget::Fire) {
                    // A pre-wait `fire` lowers to a deferred event so it dispatches
                    // in the firing frame's drain, never dropped as a non-`Entity`
                    // sentinel target (O37).
                    match step.args.get("event").and_then(serde_json::Value::as_str) {
                        Some(event) => {
                            warn_for_deferred_event(&reaction.name, event, data_registry);
                            steps
                                .push(PrepartitionedReactionStep::DeferredEvent(event.to_string()));
                        }
                        None => log::warn!(
                            "[Trigger] fire sequence step on `{}` is missing its `event` name; not binding",
                            reaction.name
                        ),
                    }
                    continue;
                }
                if classify(&step.primitive) == PrimitiveClass::Consequential {
                    if let Some(command) = bind_sequence_step(step, slot_table, script_ctx) {
                        commands.push(command);
                    }
                } else {
                    if !matches!(step.id, SequenceTarget::Entity(_)) {
                        log::warn!(
                            "[Trigger] sentinel target on presentation sequence step `{}` cannot drain app-side; not binding",
                            step.primitive
                        );
                        continue;
                    }
                    residual_steps.push(step.clone());
                }
            }
            if !residual_steps.is_empty() {
                steps.push(PrepartitionedReactionStep::Descriptor(
                    reaction.name.clone(),
                    body_ordinal,
                    ReactionDescriptor::Sequence(residual_steps),
                ));
            }
            if let Some(wait_index) = wait_index {
                let tail = sequence[wait_index..].to_vec();
                steps.push(PrepartitionedReactionStep::Descriptor(
                    reaction.name.clone(),
                    body_ordinal,
                    ReactionDescriptor::Sequence(tail),
                ));
            }
        }
    }
}

/// An `onComplete` chain hops to a later app-side dispatch, so its work still runs
/// on this trigger's fire — one drain later than the in-tick steps.
fn warn_for_deferred_event(root_event: &str, deferred_event: &str, data_registry: &DataRegistry) {
    if !event_exists(deferred_event, data_registry) {
        log::warn!(
            "[Trigger] event `{root_event}` references missing onComplete event `{deferred_event}`; it will be skipped at app dispatch"
        );
        return;
    }
    if event_contains_consequential(deferred_event, data_registry) {
        log::warn!(
            "[Trigger] event `{root_event}` buries consequential work behind onComplete `{deferred_event}`; it stays deferred to app dispatch"
        );
    }
}

/// A `Progress` target is not deferred to the drain — it is owned by `ProgressTracker`
/// and fires only at the kill threshold. The trigger never fires it, so an author who
/// buried consequential work there needs to hear that it is gated on kills, not on this
/// trigger.
fn warn_for_progress_target(
    root_event: &str,
    progress: &ProgressDescriptor,
    data_registry: &DataRegistry,
) {
    let fire = &progress.fire;
    if !event_exists(fire, data_registry) {
        log::warn!(
            "[Trigger] event `{root_event}` references missing Progress event `{fire}`; it will be skipped when the kill threshold is reached"
        );
        return;
    }
    if event_contains_consequential(fire, data_registry) {
        log::warn!(
            "[Trigger] event `{root_event}` buries consequential work behind Progress `{fire}`; this trigger never fires it — ProgressTracker fires it once tag `{}` reaches a {} kill ratio",
            progress.tag,
            progress.at,
        );
    }
}

fn event_exists(event_name: &str, data_registry: &DataRegistry) -> bool {
    data_registry
        .reactions
        .iter()
        .any(|reaction| reaction.name == event_name)
}

/// Follows unique event names iteratively so warning analysis cannot recursively
/// expand a duplicate-name graph at install time.
fn event_contains_consequential(event_name: &str, data_registry: &DataRegistry) -> bool {
    let mut pending = vec![event_name.to_string()];
    let mut visited = HashSet::new();
    while let Some(name) = pending.pop() {
        if !visited.insert(name.clone()) {
            continue;
        }
        for reaction in data_registry
            .reactions
            .iter()
            .filter(|reaction| reaction.name == name)
        {
            match &reaction.descriptor {
                ReactionDescriptor::Progress(progress) => pending.push(progress.fire.clone()),
                ReactionDescriptor::Primitive(primitive) => {
                    if classify(&primitive.primitive) == PrimitiveClass::Consequential {
                        return true;
                    }
                    if let Some(on_complete) = &primitive.on_complete {
                        pending.push(on_complete.clone());
                    }
                }
                ReactionDescriptor::Sequence(steps) => {
                    if steps
                        .iter()
                        .any(|step| classify(&step.primitive) == PrimitiveClass::Consequential)
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn classify(primitive: &str) -> PrimitiveClass {
    if CONSEQUENTIAL_PRIMITIVES.contains(&primitive) {
        PrimitiveClass::Consequential
    } else if LIFECYCLE_PRIMITIVES.contains(&primitive) {
        PrimitiveClass::Lifecycle
    } else {
        PrimitiveClass::Presentation
    }
}
