// Reaction dispatch: named events and per-tag kill progress.
// See: context/lib/scripting.md §12 (Reaction Dispatch Model)

mod group_dispatch;
mod primitive_dispatch;
mod progress;
mod sequence_dispatch;
#[cfg(test)]
mod tests;

use std::collections::VecDeque;

use super::ctx::ScriptCtx;
use super::data_descriptors::{EntityTypeDescriptor, ReactionDescriptor};
use super::data_registry::DataRegistry;
use super::reaction_registry::{ReactionPrimitiveRegistry, SystemReactionRegistry};
use super::sequence::SequencedPrimitiveRegistry;
use postretro_foundation::ir::IrValue;
use primitive_dispatch::dispatch_primitive;
pub use progress::ProgressTracker;
use sequence_dispatch::dispatch_sequence;
pub use sequence_dispatch::{validate_scoped_sequence_primitives, validate_sequence_primitives};

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
    let (values, emitter) = dispatch_context
        .map(|context| (context.values.to_vec(), context.emitter))
        .unwrap_or_default();
    let previous_context = script_ctx.system_commands.replace_fire_context(
        postretro_entities::SystemCommandFireContext {
            source,
            values,
            emitter,
        },
    );
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
                    reaction_registry,
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
    /// Where a named gameplay event happened; `playSound`'s `at: on.emitter`
    /// resolves against it (`scripting.md` §12).
    pub emitter: Option<postretro_entities::Emitter>,
}

/// One ordered item in a trigger residual. A descriptor is already resolved and
/// partitioned at install time; a deferred event marks the later dispatch hop
/// following a consequential step that ran in the fixed tick.
#[derive(Debug, Clone)]
pub enum PrepartitionedReactionStep {
    /// A resolved reaction descriptor plus the reaction `address` (name) and its
    /// `body_ordinal` among same-named matches. That pair is the first two
    /// components of a scheduler instance key and nothing downstream can
    /// reconstruct it — a resumed `Sequence` tail whose `dispatch_sequence` hits a
    /// nested wait re-enrolls under its own key from them. The `Primitive` and
    /// `Progress` arms carry the pair without reading it (the shared variant is
    /// not split); only the `Sequence` arm feeds it to `dispatch_sequence`.
    Descriptor(String, usize, ReactionDescriptor),
    DeferredEvent(String),
}

/// Who is draining a prepartitioned residual. The two debug-only guards in
/// [`fire_prepartitioned_reactions_with_sequences`] assert that no
/// `is_trigger_consequential_primitive` reaches the app drain — the binder should
/// have bound such work into the fixed tick. That holds for a `TriggerBinding`
/// residual, but a `ResumedTail` is by construction everything *after* a wait: the
/// binder deliberately deferred it, so consequential work draining app-side is
/// legitimate. The exemption keys on the caller, not on the steps — a content rule
/// ("exempt a residual that contains a `Wait`") would still panic on a post-wait
/// tail, which never contains a wait (O62).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidualOrigin {
    TriggerBinding,
    ResumedTail,
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
    origin: ResidualOrigin,
) -> Vec<String> {
    // `origin` gates the two debug-only residual guards below; it is inert in
    // release. Keeping the parameter live in release (rather than `cfg`-gating it
    // out of the signature) keeps every call site identical across build modes.
    #[cfg(not(debug_assertions))]
    let _ = origin;
    let mut chained = Vec::new();
    for step in steps {
        match step {
            PrepartitionedReactionStep::DeferredEvent(event_name) => {
                chained.push(event_name.clone());
            }
            PrepartitionedReactionStep::Descriptor(_, _, ReactionDescriptor::Progress(_)) => {
                // Tracked independently via ProgressTracker; no-op here prevents double-fire.
                // The tracker owns the completion target and fires it once `killed/total >= at`.
                // Pushing `progress.fire` here would fire that target immediately — with zero
                // kills — and then again at the real threshold.
            }
            PrepartitionedReactionStep::Descriptor(
                _,
                _,
                ReactionDescriptor::Primitive(primitive),
            ) => {
                // The guard catches consequential work draining app-side when the
                // binder should have bound it in-tick. A resumed scheduler tail is
                // exempt: it is everything AFTER a wait, so the binder legitimately
                // deferred it (O62).
                #[cfg(debug_assertions)]
                debug_assert!(
                    origin == ResidualOrigin::ResumedTail
                        || !is_trigger_consequential_primitive(&primitive.primitive),
                    "trigger residual contains consequential primitive `{}`; binding must execute it in the fixed tick",
                    primitive.primitive,
                );
                dispatch_primitive(primitive, reaction_registry, system_registry, script_ctx);
                if let Some(on_complete) = &primitive.on_complete {
                    chained.push(on_complete.clone());
                }
            }
            PrepartitionedReactionStep::Descriptor(
                address,
                body_ordinal,
                ReactionDescriptor::Sequence(steps),
            ) => {
                // Same exemption rationale as the Primitive arm: a resumed tail
                // may contain a consequential step the binder deferred past its
                // wait. Only the steps BEFORE the first `Wait` run synchronously
                // in this drain — `dispatch_sequence` breaks at the leading wait —
                // so a consequential step after the wait is legitimately deferred
                // and only the pre-wait prefix is guarded.
                #[cfg(debug_assertions)]
                debug_assert!(
                    origin == ResidualOrigin::ResumedTail
                        || steps
                            .iter()
                            .take_while(|step| {
                                !matches!(step.id, postretro_entities::SequenceTarget::Wait)
                            })
                            .all(|step| !is_trigger_consequential_primitive(&step.primitive)),
                    "trigger residual contains a consequential sequence step before its first wait; binding must execute it in the fixed tick",
                );
                // The address and ordinal ride on the step: a nested wait inside
                // this tail re-enrolls under the reaction's own key, which nothing
                // downstream can reconstruct.
                chained.extend(dispatch_sequence(
                    address,
                    *body_ordinal,
                    steps,
                    sequence_registry,
                    reaction_registry,
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
            | "updateNpcState"
            | "spawnFromSpawner"
    )
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
