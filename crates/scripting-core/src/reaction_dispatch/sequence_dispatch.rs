// Sequence-body dispatch and install-time sequence primitive validation.
// See: context/lib/scripting.md §12 (Reaction Dispatch Model)

use crate::ctx::ScriptCtx;
use crate::data_descriptors::{NamedReaction, ReactionDescriptor, SequenceStep, SequenceTarget};
use crate::data_registry::ScopedReaction;
use crate::reaction_registry::ReactionPrimitiveRegistry;
use crate::sequence::SequencedPrimitiveRegistry;

use super::group_dispatch::dispatch_group;

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
///
/// A group step resolves its kind against the registry as it runs, through the
/// entity-targeted `reaction_registry` handler — so a group step after a `wait`
/// reaches whoever exists at landing, in authored order with member steps.
pub(super) fn dispatch_sequence(
    address: &str,
    body_ordinal: usize,
    steps: &[SequenceStep],
    sequence_registry: &SequencedPrimitiveRegistry,
    reaction_registry: &ReactionPrimitiveRegistry,
    script_ctx: &ScriptCtx,
) -> Vec<String> {
    let mut fired = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let id = match &step.id {
            SequenceTarget::Wait => {
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
            SequenceTarget::Fire => {
                match step.args.get("event").and_then(serde_json::Value::as_str) {
                    Some(event) => fired.push(event.to_string()),
                    None => log::warn!(
                        "[Scripting] sequence step {i}: fire step is missing its `event` name; skipping"
                    ),
                }
                continue;
            }
            SequenceTarget::Group(group) => {
                dispatch_group(
                    &step.primitive,
                    group,
                    &step.args,
                    reaction_registry,
                    script_ctx,
                );
                continue;
            }
            SequenceTarget::Entity(id) => *id,
            SequenceTarget::Activators
            | SequenceTarget::FiredTrigger
            | SequenceTarget::EventPlayer => {
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
        // Group and subject-token steps name entity-targeted reaction
        // primitives (`applyDamage`, `grantHealth`, `updateNpcState`, …), which
        // this id-step registry does not hold. A group step resolves its handler
        // when it runs and warns there if none is registered; a subject-token
        // step binds by name in the trigger tick, and Pass A's V4a — not this
        // check — owns rejecting one after a `wait`, with an error naming the
        // reaction.
        if matches!(
            step.id,
            SequenceTarget::Group(_) | SequenceTarget::Activators | SequenceTarget::FiredTrigger
        ) {
            continue;
        }
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
