// Sequence-body dispatch and install-time sequence primitive validation.
// See: context/lib/scripting.md §12 (Reaction Dispatch Model)

use crate::ctx::ScriptCtx;
use crate::data_descriptors::{NamedReaction, ReactionDescriptor, SequenceStep};
use crate::data_registry::ScopedReaction;
use crate::sequence::SequencedPrimitiveRegistry;

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
pub(super) fn dispatch_sequence(
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
