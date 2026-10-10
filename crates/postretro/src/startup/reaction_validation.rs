// E18 install-time validation of `wait`/`fire` reaction bodies (V1–V6) and the
// V5 interruptible-wait Exit-edge derivation.
// See: context/plans/done/E18--timed-reaction-steps/index.md §Install validation

use std::collections::{HashMap, HashSet};

use postretro_entities::{
    ComponentKind, EntityId, ScriptCtx, TriggerFireMode, TriggerVolumeComponent,
};
use postretro_scripting_core::data_descriptors::{
    ReactionDescriptor, SequenceStep, SequenceTarget,
};

use crate::scripting_systems::system_reactions::SystemReactionIrBindings;
use crate::trigger_bindings::{
    ResolutionDiagnostics, TriggerBindingTable, resolve_manifest_trigger_events,
};
use crate::trigger_system::TriggerEventEdge;

/// Whether a `wait` step's `durationMs` is a valid positive, finite duration that
/// converts to a representable whole-tick countdown. V1's sole rejection point:
/// the deserializers accept any JSON number and default a missing `args` to
/// `Null`, so this must tolerate `Null`/missing (rejected) and guard the
/// `u32`-overflow bound even though Pass A does not convert. The bound uses the
/// enrollment conversion rule `ticks = ceil(durationMs * 1000 / 16_667)` against
/// `TICK_DURATION`, so a value that would overflow the countdown is rejected here
/// rather than clamping to a `u32::MAX` wait.
fn wait_duration_is_valid(args: &serde_json::Value) -> bool {
    let Some(duration_ms) = args.get("durationMs").and_then(serde_json::Value::as_f64) else {
        return false;
    };
    if !duration_ms.is_finite() || duration_ms <= 0.0 {
        return false;
    }
    let micros = crate::frame_timing::TICK_DURATION.as_micros() as f64;
    let ticks = (duration_ms * 1000.0 / micros).ceil();
    ticks < u32::MAX as f64
}

/// Whether a `wait` step is authored `interruptible`. Absent/`Null` defaults to
/// `false`, matching the enrollment reader.
fn wait_is_interruptible(args: &serde_json::Value) -> bool {
    args.get("interruptible")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Whether a primitive's args read the emitter token: `playSound`'s
/// `at: on.emitter` (`scripting.md` §12). Like the trigger sentinels, the token
/// is fire context: no `wait` survives it and a `fire` step cannot supply it.
fn args_read_emitter(args: &serde_json::Value) -> bool {
    args.get("at").and_then(serde_json::Value::as_str) == Some(postretro_entities::EMITTER_AT_TOKEN)
}

/// A `playSound` that plays `at` an emitter names a bus other than SFX.
/// Positional sounds play only on SFX, whose volume governs them.
fn emitter_on_non_sfx_bus(args: &serde_json::Value) -> Option<&str> {
    if !args_read_emitter(args) {
        return None;
    }
    args.get("bus")
        .and_then(serde_json::Value::as_str)
        .filter(|bus| !bus.eq_ignore_ascii_case("sfx"))
}

/// A present `at` value that is not exactly the emitter token
/// (`postretro_entities::EMITTER_AT_TOKEN`). `at` accepts only that opaque
/// token (`scripting.md` §12); both SDKs lower any other authored value to
/// `"@invalid"`, and a raw/Luau descriptor can author anything else. Either
/// way the value never installs.
fn invalid_at_value(args: &serde_json::Value) -> Option<&str> {
    args.get("at")
        .and_then(serde_json::Value::as_str)
        .filter(|at| *at != postretro_entities::EMITTER_AT_TOKEN)
}

/// Pass A — body-only validation (V1, V4a, V6, and the emitter-token/bus rules). Reads and mutates the
/// `DataRegistry` alone. Must run before `build_trigger_bindings`, or the binder
/// binds a body this pass rejects.
pub(crate) fn validate_reaction_bodies_pass_a(script_ctx: &ScriptCtx) {
    let mut data_registry = script_ctx.data_registry.borrow_mut();

    // V6 checks a `fire` step's target against the set of reaction names present.
    // Duplicates collapse; an entry this pass rejects to `Sequence(vec![])` still
    // counts as present because entries are never removed.
    let known_reactions: HashSet<String> = data_registry
        .reactions
        .iter()
        .map(|reaction| reaction.name.clone())
        .collect();

    for index in 0..data_registry.reactions.len() {
        // `at`, when present, names only the opaque emitter token; any other
        // value — including the SDKs' lowered `"@invalid"` sentinel — never
        // installs. Checked before the bus and sequence-only rows below.
        let at_violation = match &data_registry.reactions[index].descriptor {
            ReactionDescriptor::Primitive(primitive) => invalid_at_value(&primitive.args)
                .map(|at| (primitive.primitive.clone(), at.to_string())),
            ReactionDescriptor::Sequence(steps) => steps.iter().find_map(|step| {
                invalid_at_value(&step.args).map(|at| (step.primitive.clone(), at.to_string()))
            }),
            ReactionDescriptor::Progress(_) => None,
        };
        if let Some((primitive, at)) = at_violation {
            let name = &data_registry.reactions[index].name;
            log::error!(
                "[Scripting] reaction `{name}`: `{primitive}` plays `at: {at}`; `at` accepts only the emitter token (`on.emitter`). Dropping the reaction"
            );
            data_registry.reactions[index].descriptor = ReactionDescriptor::Sequence(Vec::new());
            continue;
        }

        // `at: on.emitter` pairs only with the SFX bus. Checked on every body
        // shape before the sequence-only rows below.
        let bus_violation = match &data_registry.reactions[index].descriptor {
            ReactionDescriptor::Primitive(primitive) => emitter_on_non_sfx_bus(&primitive.args)
                .map(|bus| (primitive.primitive.clone(), bus.to_string())),
            ReactionDescriptor::Sequence(steps) => steps.iter().find_map(|step| {
                emitter_on_non_sfx_bus(&step.args)
                    .map(|bus| (step.primitive.clone(), bus.to_string()))
            }),
            ReactionDescriptor::Progress(_) => None,
        };
        if let Some((primitive, bus)) = bus_violation {
            let name = &data_registry.reactions[index].name;
            log::error!(
                "[Scripting] reaction `{name}`: `{primitive}` plays `at: on.emitter` on bus `{bus}`; positioned sounds play only on the sfx bus. Dropping the reaction"
            );
            data_registry.reactions[index].descriptor = ReactionDescriptor::Sequence(Vec::new());
            continue;
        }

        let ReactionDescriptor::Sequence(steps) = &data_registry.reactions[index].descriptor else {
            continue;
        };
        let name = data_registry.reactions[index].name.clone();
        let steps = steps.clone();

        // V1: a `wait` step with a malformed `durationMs` drops the whole reaction —
        // never a silent 1-tick wait (`NaN as u32` → 0 → `.max(1)`) or a `u32::MAX`
        // countdown.
        if let Some(step_index) = steps.iter().position(|step| {
            matches!(step.id, SequenceTarget::Wait) && !wait_duration_is_valid(&step.args)
        }) {
            log::error!(
                "[Scripting] reaction `{name}` step {step_index}: `wait` durationMs is zero, negative, NaN, non-finite, or overflows the tick countdown; dropping the reaction (V1). The level installs and other reactions are unaffected"
            );
            data_registry.reactions[index].descriptor = ReactionDescriptor::Sequence(Vec::new());
            continue;
        }

        // V4a: any step AFTER a `wait` carrying an `@activators`/`@trigger`
        // sentinel reads fire context that no `wait` survives. Drop the reaction.
        // `args_read_emitter` here guards a sequenced step's raw args, not a
        // live `playSound` step: `playSound` registers only as a system
        // primitive, so `validate_sequence_primitives` (`setupLevel`) drops
        // any sequence naming it before this pass ever runs.
        if let Some(wait_pos) = steps
            .iter()
            .position(|step| matches!(step.id, SequenceTarget::Wait))
            && let Some(offset) = steps[wait_pos + 1..].iter().position(|step| {
                matches!(
                    step.id,
                    SequenceTarget::Activators
                        | SequenceTarget::FiredTrigger
                        | SequenceTarget::EventPlayer
                ) || args_read_emitter(&step.args)
                    || postretro_scripting_core::player_event_scope::reads_event_player(&step.args)
            })
        {
            let step_index = wait_pos + 1 + offset;
            log::error!(
                "[Scripting] reaction `{name}` step {step_index}: a post-`wait` step reads fire context (`@activators`/`@trigger`/`@player`, `byPlayer(on.player)`, or `at: on.emitter` in its raw args) that no `wait` survives; dropping the reaction (V4a)"
            );
            data_registry.reactions[index].descriptor = ReactionDescriptor::Sequence(Vec::new());
            continue;
        }

        // V6: a `fire` step naming a reaction absent from the registry drops just
        // that step and keeps the reaction (mirrors the unknown-event `warn!` in
        // `dispatch_deferred_named_events_with_sequences`).
        let mut retained: Vec<SequenceStep> = Vec::with_capacity(steps.len());
        let mut dropped_any = false;
        for (step_index, step) in steps.iter().enumerate() {
            if matches!(step.id, SequenceTarget::Fire) {
                match step.args.get("event").and_then(serde_json::Value::as_str) {
                    Some(event) if known_reactions.contains(event) => retained.push(step.clone()),
                    Some(event) => {
                        log::warn!(
                            "[Scripting] reaction `{name}` step {step_index}: `fire` names unknown reaction `{event}`; dropping the step and keeping the reaction (V6)"
                        );
                        dropped_any = true;
                    }
                    None => {
                        log::warn!(
                            "[Scripting] reaction `{name}` step {step_index}: `fire` is missing its `event` name; dropping the step and keeping the reaction (V6)"
                        );
                        dropped_any = true;
                    }
                }
            } else {
                retained.push(step.clone());
            }
        }
        if dropped_any {
            data_registry.reactions[index].descriptor = ReactionDescriptor::Sequence(retained);
        }
    }
}

/// Enter-binding provenance for Pass B, read from the sources the binder reads —
/// never from `TriggerBindingTable`, which discards reaction identity:
/// brush-KVP `TriggerVolumeComponent.on_fire` across `EntityRegistry` triggers,
/// plus the manifest trigger events the binder resolves
/// ([`resolve_manifest_trigger_events`]: mod-global tag rules and level member
/// events, Enter edge).
struct EnterBindingProvenance {
    /// Reaction name → triggers it is Enter-bound to.
    enter_triggers: HashMap<String, Vec<EntityId>>,
    /// Per-trigger `fire_mode`, for V2.
    trigger_fire_mode: HashMap<EntityId, TriggerFireMode>,
}

fn collect_enter_provenance(script_ctx: &ScriptCtx) -> EnterBindingProvenance {
    let mut enter_triggers: HashMap<String, Vec<EntityId>> = HashMap::new();
    let mut trigger_fire_mode: HashMap<EntityId, TriggerFireMode> = HashMap::new();
    let registry = script_ctx.registry.borrow();
    let mut trigger_ids: Vec<EntityId> = registry
        .iter_with_kind(ComponentKind::TriggerVolume)
        .map(|(id, _)| id)
        .collect();
    trigger_ids.sort_unstable();
    for trigger in trigger_ids {
        let Ok(component) = registry
            .get_component::<TriggerVolumeComponent>(trigger)
            .cloned()
        else {
            continue;
        };
        trigger_fire_mode.insert(trigger, component.fire_mode);
        // Brush KVP: `on_fire` is the Enter reaction (the binder reads the
        // same field for `TriggerEventEdge::Enter`).
        if !component.on_fire.is_empty() {
            enter_triggers
                .entry(component.on_fire.clone())
                .or_default()
                .push(trigger);
        }
    }

    // Manifest trigger events, resolved exactly as the binder resolves them:
    // mod-global `defineTriggerEvent` rules expand over their tag, level
    // member events (`t.on`) name their one volume. A volume-keyed Enter
    // therefore counts toward V3, and V2/V5 see only that volume.
    let resolved = {
        let data_registry = script_ctx.data_registry.borrow();
        resolve_manifest_trigger_events(&registry, &data_registry, ResolutionDiagnostics::Silent)
    };
    for binding in resolved {
        if binding.edge != TriggerEventEdge::Enter {
            continue;
        }
        enter_triggers
            .entry(binding.reaction)
            .or_default()
            .push(binding.trigger);
    }
    EnterBindingProvenance {
        enter_triggers,
        trigger_fire_mode,
    }
}

/// Collect reaction addresses that cannot be dispatched without a trigger-fire
/// scope. The runtime descriptor graph is authoritative here: Luau and raw
/// descriptor authors can bypass TypeScript's `Reaction<{}>` gate, and a
/// string-valued `fire` target carries no static scope information.
fn scoped_fire_targets(
    script_ctx: &ScriptCtx,
    system_bindings: &SystemReactionIrBindings,
) -> HashMap<String, String> {
    let mut targets = HashMap::new();

    let data_registry = script_ctx.data_registry.borrow();
    for reaction in &data_registry.reactions {
        let requirement = match &reaction.descriptor {
            ReactionDescriptor::Primitive(primitive) => primitive
                .target
                .as_deref()
                .map(|target| format!("primitive target sentinel `{target}`"))
                .or_else(|| {
                    args_read_emitter(&primitive.args)
                        .then(|| format!("`{}` at `on.emitter`", primitive.primitive))
                })
                .or_else(|| {
                    postretro_scripting_core::player_event_scope::reads_event_player(
                        &primitive.args,
                    )
                    .then(|| format!("`{}` reads `byPlayer(on.player)`", primitive.primitive))
                }),
            ReactionDescriptor::Sequence(steps) => {
                steps
                    .iter()
                    .enumerate()
                    .find_map(|(step_index, step)| match &step.id {
                        SequenceTarget::Activators => Some(format!(
                            "sequence step {step_index} target sentinel `@activators`"
                        )),
                        SequenceTarget::FiredTrigger => Some(format!(
                            "sequence step {step_index} target sentinel `@trigger`"
                        )),
                        SequenceTarget::EventPlayer => Some(format!(
                            "sequence step {step_index} target sentinel `@player`"
                        )),
                        // Guards a sequenced step's raw args, not a live
                        // `playSound` step: `playSound` is a system-only
                        // primitive, so no sequence naming it survives
                        // `validate_sequence_primitives` (`setupLevel`) to
                        // reach V4b.
                        SequenceTarget::Entity(_)
                        | SequenceTarget::Group(_)
                        | SequenceTarget::Wait
                        | SequenceTarget::Fire => args_read_emitter(&step.args)
                            .then(|| format!("sequence step {step_index} `at` `on.emitter`")),
                    })
            }
            ReactionDescriptor::Progress(_) => None,
        };
        if let Some(requirement) = requirement {
            targets.entry(reaction.name.clone()).or_insert(requirement);
        }
    }
    drop(data_registry);

    // Runtime setState IR is the remaining scoped descriptor shape. Reuse the
    // binding table's precomputed input names so this analysis cannot drift from
    // the evaluator's accepted IR tree.
    for (name, inputs) in system_bindings.reaction_dispatch_inputs() {
        if !inputs.is_empty() {
            targets
                .entry(name.to_string())
                .or_insert_with(|| format!("runtime dispatch inputs {inputs:?}"));
        }
    }

    targets
}

/// Pass B — trigger-coupled rejection rows (V2, V3, V4b). Like Pass A, this
/// MUST run BEFORE `build_trigger_bindings`: dropping a reaction after the
/// binder has run is a no-op for trigger-bound content, because
/// `partition_direct_reaction` copies the body into owned in-tick commands and
/// residual steps at bind time and the runtime drain never re-reads the
/// `DataRegistry` body — a post-bind drop leaves a V2 wait enrolling on Enter
/// and a V4b pre-wait `fire` dispatching as a `DeferredEvent`. Rejecting first
/// means the binder matches an inert `Sequence(vec![])` and binds nothing.
/// Nothing here needs the built table: provenance comes from the same raw
/// sources the binder reads (see [`collect_enter_provenance`]), and V4b reads
/// every scope-dependent descriptor shape plus precomputed system-IR dispatch
/// inputs before any body is copied into a binding. Only the V5 derivation
/// ([`derive_interruptible_wait_exit_edges`]) must follow the binder, because
/// it inserts into the freshly built table.
pub(crate) fn validate_trigger_coupled_pass_b(
    script_ctx: &ScriptCtx,
    system_bindings: &SystemReactionIrBindings,
) {
    // A `fire` step runs from a contextless app drain. Its target therefore may
    // not require either trigger target sentinels or seeded runtime inputs.
    let scoped_fire_targets = scoped_fire_targets(script_ctx, system_bindings);

    let provenance = collect_enter_provenance(script_ctx);

    let mut rejects: Vec<usize> = Vec::new();
    {
        let data_registry = script_ctx.data_registry.borrow();
        for (index, reaction) in data_registry.reactions.iter().enumerate() {
            let ReactionDescriptor::Sequence(steps) = &reaction.descriptor else {
                continue;
            };
            let name = &reaction.name;

            // V4b: any `fire` step whose target needs a dispatch scope. Find the
            // exact target as well as the index so the diagnostic explains the
            // descriptor shape that made a raw/string call invalid.
            if let Some((step_index, event, requirement)) =
                steps.iter().enumerate().find_map(|(step_index, step)| {
                    if !matches!(step.id, SequenceTarget::Fire) {
                        return None;
                    }
                    let event = step.args.get("event")?.as_str()?;
                    let requirement = scoped_fire_targets.get(event)?;
                    Some((step_index, event, requirement))
                })
            {
                log::error!(
                    "[Scripting] reaction `{name}` step {step_index}: `fire` targets reaction `{event}`, which requires trigger-fire dispatch scope ({requirement}); a `fire` control step dispatches without that scope, so the reaction is dropped (V4b)"
                );
                rejects.push(index);
                continue;
            }

            // Remaining rows govern only interruptible waits.
            let Some(wait_index) = steps.iter().position(|step| {
                matches!(step.id, SequenceTarget::Wait) && wait_is_interruptible(&step.args)
            }) else {
                continue;
            };
            let enter = provenance.enter_triggers.get(name);

            // V2: Enter-bound to a `once` trigger — the latch is spent on first
            // fire, so a cancel destroys the set-piece permanently.
            if enter.is_some_and(|triggers| {
                triggers.iter().any(|trigger| {
                    matches!(
                        provenance.trigger_fire_mode.get(trigger),
                        Some(TriggerFireMode::Once)
                    )
                })
            }) {
                log::error!(
                    "[Scripting] reaction `{name}` step {wait_index}: interruptible `wait` is Enter-bound to a `once` trigger whose spent latch a cancel cannot re-open; dropping the reaction (V2)"
                );
                rejects.push(index);
                continue;
            }

            // V3: no trigger-Enter binding at all — the interruptible flag has no
            // cancel source.
            let has_enter = enter.is_some_and(|triggers| !triggers.is_empty());
            if !has_enter {
                log::error!(
                    "[Scripting] reaction `{name}` step {wait_index}: interruptible `wait` has no trigger-Enter binding and therefore no cancel source; dropping the reaction (V3)"
                );
                rejects.push(index);
                continue;
            }
        }
    }

    if !rejects.is_empty() {
        let mut data_registry = script_ctx.data_registry.borrow_mut();
        for index in rejects {
            data_registry.reactions[index].descriptor = ReactionDescriptor::Sequence(Vec::new());
        }
    }
}

/// Pass B — the V5 Exit-edge derivation. Runs AFTER `build_trigger_bindings` +
/// `install_manifest_events` (the table is built from scratch there, so an
/// earlier insert would be lost) and after the rejection rows above (a dropped
/// reaction is `Sequence(vec![])` by now, so it derives nothing). Every Enter
/// trigger of a surviving interruptible-wait reaction gets its paired
/// `(trigger, Exit)` edge inserted into `bound_edges`, so cancellation has a
/// live source even with no authored `on_exit` KVP.
pub(crate) fn derive_interruptible_wait_exit_edges(
    script_ctx: &ScriptCtx,
    trigger_bindings: &mut TriggerBindingTable,
) {
    let provenance = collect_enter_provenance(script_ctx);
    // Deduplicate: a reaction Enter-bound to two triggers, or two reactions
    // sharing a trigger, must not insert the same edge twice (the insert is
    // idempotent anyway, this only avoids redundant work).
    let mut derived: HashSet<EntityId> = HashSet::new();
    let data_registry = script_ctx.data_registry.borrow();
    for reaction in &data_registry.reactions {
        let ReactionDescriptor::Sequence(steps) = &reaction.descriptor else {
            continue;
        };
        if !steps.iter().any(|step| {
            matches!(step.id, SequenceTarget::Wait) && wait_is_interruptible(&step.args)
        }) {
            continue;
        }
        let Some(triggers) = provenance.enter_triggers.get(&reaction.name) else {
            continue;
        };
        for &trigger in triggers {
            if derived.insert(trigger) {
                trigger_bindings.bind_edge_only(trigger, TriggerEventEdge::Exit);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::{
        MoverCommand, NamedReaction, PrimitiveDescriptor, SlotOwnership, SlotRecord, SlotSchema,
        SlotType, SlotValue, Transform, TriggerActivation, TriggerEventDescriptor,
        TriggerVolumeComponent, VolumeTriggerEventDescriptor,
    };
    use postretro_scripting_core::reaction_dispatch::{
        PrepartitionedReactionStep, ResidualOrigin, fire_prepartitioned_reactions_with_sequences,
    };
    use postretro_scripting_core::reaction_registry::{
        ReactionPrimitiveRegistry, SystemReactionRegistry,
    };
    use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
    use postretro_test_log_capture::LogCapture;
    use serde_json::json;

    use crate::scripting_systems::reaction_scheduler::{
        ReactionScheduler, register_reaction_control_primitives,
    };
    use crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge;

    // --- fixture builders ---------------------------------------------------

    fn wait_step(duration_ms: serde_json::Value, interruptible: bool) -> SequenceStep {
        SequenceStep {
            id: SequenceTarget::Wait,
            primitive: "wait".to_string(),
            args: json!({ "durationMs": duration_ms, "interruptible": interruptible }),
        }
    }

    fn fire_step(event: &str) -> SequenceStep {
        SequenceStep {
            id: SequenceTarget::Fire,
            primitive: "fire".to_string(),
            args: json!({ "event": event }),
        }
    }

    fn entity_step(id: u32) -> SequenceStep {
        SequenceStep {
            id: SequenceTarget::Entity(EntityId::from_raw(id)),
            primitive: "setLightAnimation".to_string(),
            args: json!({}),
        }
    }

    fn sentinel_step(target: SequenceTarget) -> SequenceStep {
        SequenceStep {
            id: target,
            primitive: "updateNpcState".to_string(),
            args: json!({ "aggro": true }),
        }
    }

    fn sequence(name: &str, steps: Vec<SequenceStep>) -> NamedReaction {
        NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Sequence(steps),
        }
    }

    fn set_state(name: &str, slot: &str, value: serde_json::Value) -> NamedReaction {
        NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "setState".to_string(),
                target: None,
                kind: None,
                tag: None,
                on_complete: None,
                args: json!({ "slot": slot, "value": value }),
            }),
        }
    }

    fn activator_primitive(name: &str) -> NamedReaction {
        NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "applyDamage".to_string(),
                target: Some("@activators".to_string()),
                kind: None,
                tag: None,
                on_complete: None,
                args: json!({ "amount": 5.0 }),
            }),
        }
    }

    /// A `playSound` primitive reaction; `at_emitter` authors `at: on.emitter`.
    fn play_sound(name: &str, bus: Option<&str>, at_emitter: bool) -> NamedReaction {
        let mut args = json!({ "sound": "sfx/test_tone" });
        if let Some(bus) = bus {
            args["bus"] = json!(bus);
        }
        if at_emitter {
            args["at"] = json!(postretro_entities::EMITTER_AT_TOKEN);
        }
        NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "playSound".to_string(),
                target: None,
                kind: None,
                tag: None,
                on_complete: None,
                args,
            }),
        }
    }

    /// A `playSound` primitive reaction whose `at` is an arbitrary raw
    /// string, not necessarily the emitter token.
    fn play_sound_at(name: &str, at: &str) -> NamedReaction {
        NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "playSound".to_string(),
                target: None,
                kind: None,
                tag: None,
                on_complete: None,
                args: json!({ "sound": "sfx/test_tone", "at": at }),
            }),
        }
    }

    /// A raw sequence step reading the emitter token.
    fn emitter_step() -> SequenceStep {
        SequenceStep {
            id: SequenceTarget::Entity(EntityId::from_raw(1)),
            primitive: "playSound".to_string(),
            args: json!({ "sound": "sfx/test_tone", "at": postretro_entities::EMITTER_AT_TOKEN }),
        }
    }

    /// A level member `t.on("enter", fire)` on `trigger`.
    fn member_enter(trigger: EntityId, fire: &[&str]) -> VolumeTriggerEventDescriptor {
        VolumeTriggerEventDescriptor {
            trigger,
            event: "enter".to_string(),
            fire: fire.iter().map(|name| name.to_string()).collect(),
        }
    }

    fn ctx_with_reactions(reactions: Vec<NamedReaction>) -> ScriptCtx {
        let ctx = ScriptCtx::new();
        ctx.data_registry
            .borrow_mut()
            .populate_level(reactions, Vec::new(), &[]);
        ctx
    }

    fn insert_writable_number(ctx: &ScriptCtx, slot: &str) {
        ctx.slot_table
            .borrow_mut()
            .insert(
                slot.to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(0.0)),
                    range: Some(postretro_entities::NumericRange {
                        min: 0.0,
                        max: 100.0,
                    }),
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: Default::default(),
                    per_owner: false,
                    accumulate: None,
                }),
            )
            .expect("fixture slot should be vacant");
    }

    fn spawn_trigger(
        ctx: &ScriptCtx,
        on_fire: &str,
        on_exit: &str,
        fire_mode: TriggerFireMode,
        tags: &[&str],
    ) -> EntityId {
        let mut registry = ctx.registry.borrow_mut();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                TriggerVolumeComponent::new(
                    TriggerActivation::Touch,
                    String::new(),
                    on_fire.to_string(),
                    on_exit.to_string(),
                    MoverCommand::Start,
                    fire_mode,
                    0.0,
                    true,
                ),
            )
            .unwrap();
        if !tags.is_empty() {
            registry
                .set_tags(id, tags.iter().map(|t| t.to_string()).collect())
                .unwrap();
        }
        id
    }

    fn body(ctx: &ScriptCtx, name: &str) -> ReactionDescriptor {
        ctx.data_registry
            .borrow()
            .reactions
            .iter()
            .find(|reaction| reaction.name == name)
            .expect("reaction present")
            .descriptor
            .clone()
    }

    fn is_dropped(ctx: &ScriptCtx, name: &str) -> bool {
        matches!(body(ctx, name), ReactionDescriptor::Sequence(steps) if steps.is_empty())
    }

    fn built_system_bindings(ctx: &ScriptCtx) -> SystemReactionIrBindings {
        let mut bindings = SystemReactionIrBindings::default();
        bindings.rebuild(&ctx.data_registry.borrow(), ctx);
        bindings
    }

    fn run_pass_b(ctx: &ScriptCtx) {
        let bindings = built_system_bindings(ctx);
        validate_trigger_coupled_pass_b(ctx, &bindings);
    }

    /// Mirror the production install/staged-commit order exactly
    /// (`install_world_cpu` / `poll_staged_manifest_results`): Pass A → Pass B
    /// rejection rows → binder build + manifest events → V5 Exit-edge
    /// derivation. Tests that assert what a trigger-bound reaction can do at
    /// runtime must go through this, never a hand-ordered variant — an
    /// empty-table shortcut is exactly the blind spot that shipped the
    /// bind-before-drop bug.
    fn validated_bindings(ctx: &ScriptCtx) -> TriggerBindingTable {
        validate_reaction_bodies_pass_a(ctx);
        run_pass_b(ctx);
        let mut table = {
            let registry = ctx.registry.borrow();
            let data_registry = ctx.data_registry.borrow();
            TriggerBindingTable::build_with_script_ctx(&registry, &data_registry, ctx)
        };
        {
            let registry = ctx.registry.borrow();
            let data_registry = ctx.data_registry.borrow();
            table.install_manifest_events(&registry, &data_registry, ctx);
        }
        derive_interruptible_wait_exit_edges(ctx, &mut table);
        table
    }

    /// Fire the trigger's Enter binding exactly as `simulate_tick` does and
    /// return what runtime work exists for it: the in-tick command count and
    /// the residual steps the frame-end drain would receive. `(0, None)` means
    /// the edge is runtime-inert — nothing can run or enroll from it.
    fn enter_execution(
        table: &TriggerBindingTable,
        ctx: &ScriptCtx,
        trigger: EntityId,
    ) -> (usize, Option<Vec<PrepartitionedReactionStep>>) {
        let mut registry = ctx.registry.borrow_mut();
        let mut slot_table = ctx.slot_table.borrow_mut();
        let execution = table.execute(
            trigger,
            TriggerEventEdge::Enter,
            &mut registry,
            &mut slot_table,
            &crate::trigger_commands::TriggerFireContext::default(),
        );
        let command_count = execution.command_count();
        let steps = execution
            .residual()
            .and_then(|handle| table.residual(handle))
            .map(|residual| residual.steps().to_vec());
        (command_count, steps)
    }

    // --- V1 (Pass A) --------------------------------------------------------

    // V1 / O29: a malformed `durationMs` drops the reaction; the level continues
    // and a sibling reaction is unaffected.
    #[test]
    fn v1_drops_reaction_with_malformed_duration_and_leaves_siblings() {
        for bad in [json!(0), json!(-5), json!(1e12)] {
            let ctx = ctx_with_reactions(vec![
                sequence("bad", vec![wait_step(bad.clone(), false), entity_step(1)]),
                sequence("good", vec![entity_step(2)]),
            ]);
            let capture = LogCapture::start();
            validate_reaction_bodies_pass_a(&ctx);
            capture.assert_logged_once(log::Level::Error, "reaction `bad` step 0");
            assert!(
                is_dropped(&ctx, "bad"),
                "malformed duration {bad} drops `bad`"
            );
            assert!(
                matches!(body(&ctx, "good"), ReactionDescriptor::Sequence(steps) if steps.len() == 1),
                "the sibling reaction is untouched",
            );
        }
    }

    // A valid duration survives Pass A.
    #[test]
    fn v1_accepts_a_valid_duration() {
        let ctx = ctx_with_reactions(vec![sequence(
            "ok",
            vec![wait_step(json!(800), false), entity_step(1)],
        )]);
        validate_reaction_bodies_pass_a(&ctx);
        assert!(!is_dropped(&ctx, "ok"));
    }

    // --- V4a (Pass A) -------------------------------------------------------

    // V4a: a post-`wait` step targeting a trigger-fire sentinel drops the reaction.
    #[test]
    fn v4a_drops_post_wait_sentinel_step() {
        for sentinel in [SequenceTarget::Activators, SequenceTarget::FiredTrigger] {
            let ctx = ctx_with_reactions(vec![sequence(
                "reveal",
                vec![
                    entity_step(1),
                    wait_step(json!(200), false),
                    sentinel_step(sentinel),
                ],
            )]);
            let capture = LogCapture::start();
            validate_reaction_bodies_pass_a(&ctx);
            capture.assert_logged_once(log::Level::Error, "reaction `reveal` step 2");
            assert!(is_dropped(&ctx, "reveal"));
        }
    }

    // `on.player` is fire context too: a post-`wait` subject step or
    // `byPlayer(on.player)` read drops the reaction; the same step before the
    // wait is kept.
    #[test]
    fn v4a_drops_post_wait_event_player_and_keeps_it_before_the_wait() {
        let owner_read = SequenceStep {
            id: SequenceTarget::Entity(postretro_entities::EntityId::from_raw(1)),
            primitive: "updateNpcState".to_string(),
            args: json!({ "aggro": { "op": "input", "name": "player.health", "owner": "@player" } }),
        };
        let ctx = ctx_with_reactions(vec![
            sequence(
                "lateStep",
                vec![wait_step(json!(200), false), sentinel_step(SequenceTarget::EventPlayer)],
            ),
            sequence("lateRead", vec![wait_step(json!(200), false), owner_read]),
            sequence(
                "early",
                vec![sentinel_step(SequenceTarget::EventPlayer), wait_step(json!(200), false)],
            ),
        ]);
        let capture = LogCapture::start();
        validate_reaction_bodies_pass_a(&ctx);
        capture.assert_logged_once(log::Level::Error, "reaction `lateStep` step 1");
        capture.assert_logged_once(log::Level::Error, "reaction `lateRead` step 1");
        assert!(is_dropped(&ctx, "lateStep"));
        assert!(is_dropped(&ctx, "lateRead"));
        assert!(!is_dropped(&ctx, "early"));
    }

    // A pre-`wait` sentinel step is legitimate fire-time context and is not
    // rejected by V4a.
    #[test]
    fn v4a_keeps_pre_wait_sentinel_step() {
        let ctx = ctx_with_reactions(vec![sequence(
            "reveal",
            vec![
                sentinel_step(SequenceTarget::Activators),
                wait_step(json!(200), false),
                entity_step(1),
            ],
        )]);
        validate_reaction_bodies_pass_a(&ctx);
        assert!(!is_dropped(&ctx, "reveal"));
    }

    // The emitter token is fire context: no `wait` survives it.
    #[test]
    fn v4a_drops_post_wait_emitter_token_and_keeps_it_before_the_wait() {
        let ctx = ctx_with_reactions(vec![
            sequence("late", vec![wait_step(json!(200), false), emitter_step()]),
            sequence("early", vec![emitter_step(), wait_step(json!(200), false)]),
        ]);
        let capture = LogCapture::start();
        validate_reaction_bodies_pass_a(&ctx);
        capture.assert_logged_once(log::Level::Error, "reaction `late` step 1");
        assert!(is_dropped(&ctx, "late"));
        assert!(!is_dropped(&ctx, "early"));
    }

    // `at`, when present, names only the opaque emitter token. Any other raw
    // value — including the SDKs' lowered `"@invalid"` sentinel — drops the
    // reaction at install; a sibling with a valid `at` or none at all installs.
    #[test]
    fn play_sound_rejects_an_at_value_other_than_the_emitter_token() {
        let ctx = ctx_with_reactions(vec![
            play_sound_at("invalidToken", "@invalid"),
            play_sound_at("otherSentinel", "@activators"),
            play_sound("anchored", None, true),
            play_sound("unanchored", None, false),
        ]);
        let capture = LogCapture::start();
        validate_reaction_bodies_pass_a(&ctx);
        capture.assert_logged_once(
            log::Level::Error,
            "reaction `invalidToken`: `playSound` plays `at: @invalid`",
        );
        capture.assert_logged_once(
            log::Level::Error,
            "reaction `otherSentinel`: `playSound` plays `at: @activators`",
        );
        for dropped in ["invalidToken", "otherSentinel"] {
            assert!(is_dropped(&ctx, dropped), "{dropped} is dropped");
        }
        for kept in ["anchored", "unanchored"] {
            assert!(!is_dropped(&ctx, kept), "{kept} installs");
        }
    }

    // `at: on.emitter` plays positioned, and positioned sounds play only on SFX.
    #[test]
    fn emitter_anchored_play_sound_rejects_a_non_sfx_bus() {
        let ctx = ctx_with_reactions(vec![
            play_sound("uiAnchored", Some("ui"), true),
            play_sound("sfxAnchored", Some("SFX"), true),
            play_sound("anchored", None, true),
            play_sound("uiUnanchored", Some("ui"), false),
        ]);
        let capture = LogCapture::start();
        validate_reaction_bodies_pass_a(&ctx);
        capture.assert_logged_once(
            log::Level::Error,
            "reaction `uiAnchored`: `playSound` plays `at: on.emitter` on bus `ui`",
        );
        assert!(is_dropped(&ctx, "uiAnchored"));
        for kept in ["sfxAnchored", "anchored", "uiUnanchored"] {
            assert!(!is_dropped(&ctx, kept), "{kept} installs");
        }
    }

    // --- V6 (Pass A) --------------------------------------------------------

    // V6: a `fire` step naming an absent reaction drops the step and keeps the
    // reaction; a `fire` naming a present reaction is retained.
    #[test]
    fn v6_drops_unknown_fire_step_and_keeps_reaction() {
        let ctx = ctx_with_reactions(vec![
            sequence(
                "reveal",
                vec![fire_step("present"), fire_step("absent"), entity_step(1)],
            ),
            sequence("present", vec![entity_step(2)]),
        ]);
        let capture = LogCapture::start();
        validate_reaction_bodies_pass_a(&ctx);
        capture.assert_logged_once(log::Level::Warn, "names unknown reaction `absent`");
        let ReactionDescriptor::Sequence(steps) = body(&ctx, "reveal") else {
            panic!("reveal stays a sequence");
        };
        assert_eq!(steps.len(), 2, "only the unknown fire step is dropped");
        assert!(matches!(steps[0].id, SequenceTarget::Fire));
        assert!(matches!(steps[1].id, SequenceTarget::Entity(_)));
    }

    // --- V2 (Pass B) --------------------------------------------------------

    // V2: an interruptible wait Enter-bound to a `once` trigger drops the reaction.
    #[test]
    fn v2_drops_interruptible_wait_on_once_trigger() {
        let ctx = ctx_with_reactions(vec![sequence(
            "reveal",
            vec![wait_step(json!(800), true), entity_step(1)],
        )]);
        spawn_trigger(&ctx, "reveal", "", TriggerFireMode::Once, &[]);
        let capture = LogCapture::start();
        run_pass_b(&ctx);
        capture.assert_logged_once(log::Level::Error, "reaction `reveal` step 0");
        assert!(is_dropped(&ctx, "reveal"));
    }

    // A `multiple` trigger is a valid cancel source: V2 does not fire.
    #[test]
    fn v2_keeps_interruptible_wait_on_multiple_trigger() {
        let ctx = ctx_with_reactions(vec![sequence(
            "reveal",
            vec![wait_step(json!(800), true), entity_step(1)],
        )]);
        spawn_trigger(&ctx, "reveal", "", TriggerFireMode::Multiple, &[]);
        run_pass_b(&ctx);
        assert!(!is_dropped(&ctx, "reveal"));
    }

    // --- V3 (Pass B) --------------------------------------------------------

    // V3: an interruptible wait with no trigger-Enter binding drops the reaction.
    #[test]
    fn v3_drops_interruptible_wait_with_no_enter_binding() {
        let ctx = ctx_with_reactions(vec![sequence(
            "levelLoad",
            vec![wait_step(json!(800), true), entity_step(1)],
        )]);
        let capture = LogCapture::start();
        run_pass_b(&ctx);
        capture.assert_logged_once(log::Level::Error, "reaction `levelLoad` step 0");
        assert!(is_dropped(&ctx, "levelLoad"));
    }

    // A non-interruptible wait needs no cancel source, so V3 leaves it alone even
    // with no Enter binding.
    #[test]
    fn v3_keeps_non_interruptible_wait_without_enter_binding() {
        let ctx = ctx_with_reactions(vec![sequence(
            "levelLoad",
            vec![wait_step(json!(800), false), entity_step(1)],
        )]);
        run_pass_b(&ctx);
        assert!(!is_dropped(&ctx, "levelLoad"));
    }

    // --- V4b (Pass B) -------------------------------------------------------

    // V4b: a `fire` step whose target is a scoped system `setState` (reads
    // `@rising`) drops the reaction, at any position.
    #[test]
    fn v4b_drops_fire_of_scoped_system_reaction() {
        let scoped_value = json!({
            "op": "select",
            "cond": { "op": "input", "name": "@rising" },
            "a": { "op": "const", "value": 1.0 },
            "b": { "op": "const", "value": 0.0 }
        });
        let ctx = ctx_with_reactions(vec![
            set_state("raiseAlarm", "puzzle.target", scoped_value),
            sequence("reveal", vec![entity_step(1), fire_step("raiseAlarm")]),
        ]);
        insert_writable_number(&ctx, "puzzle.target");
        let capture = LogCapture::start();
        run_pass_b(&ctx);
        capture.assert_logged_once(log::Level::Error, "reaction `reveal` step 1");
        assert!(is_dropped(&ctx, "reveal"));
    }

    // A `fire` step whose target is a plain (sourceless) system `setState` is
    // fine: V4b only rejects targets that read a seeded dispatch input.
    #[test]
    fn v4b_keeps_fire_of_sourceless_system_reaction() {
        let plain_value = json!({ "op": "const", "value": 1.0 });
        let ctx = ctx_with_reactions(vec![
            set_state("raiseAlarm", "puzzle.target", plain_value),
            sequence("reveal", vec![entity_step(1), fire_step("raiseAlarm")]),
        ]);
        insert_writable_number(&ctx, "puzzle.target");
        run_pass_b(&ctx);
        assert!(!is_dropped(&ctx, "reveal"));
    }

    // Regression: V4b once considered only bound system-setState IR. A raw or
    // Luau `fire("damageActivators")` then installed successfully and merely
    // warn-skipped its primitive at the contextless app drain.
    #[test]
    fn v4b_drops_fire_of_primitive_sentinel_target() {
        let ctx = ctx_with_reactions(vec![
            activator_primitive("damageActivators"),
            sequence("reveal", vec![fire_step("damageActivators")]),
        ]);
        let capture = LogCapture::start();
        run_pass_b(&ctx);
        capture.assert_logged_once(
            log::Level::Error,
            "requires trigger-fire dispatch scope (primitive target sentinel `@activators`)",
        );
        assert!(is_dropped(&ctx, "reveal"));
    }

    // A `fire` step dispatches with no emitter, so it cannot target a reaction
    // that plays `at: on.emitter`; the same target without the token is fine.
    #[test]
    fn v4b_drops_fire_of_an_emitter_reading_reaction() {
        let ctx = ctx_with_reactions(vec![
            play_sound("thud", None, true),
            play_sound("chime", None, false),
            sequence("anchoredChain", vec![fire_step("thud")]),
            sequence("plainChain", vec![fire_step("chime")]),
        ]);
        let capture = LogCapture::start();
        run_pass_b(&ctx);
        capture.assert_logged_once(log::Level::Error, "`playSound` at `on.emitter`");
        assert!(is_dropped(&ctx, "anchoredChain"));
        assert!(!is_dropped(&ctx, "plainChain"));
    }

    // Regression: a `fire` target can itself be a sequence whose entity-like
    // target is supplied only by a trigger fire. Cover both opaque sequence
    // target variants; neither may survive as a contextless nested dispatch.
    #[test]
    fn v4b_drops_fire_of_sequence_sentinel_target() {
        for (sentinel, spelling) in [
            (SequenceTarget::Activators, "@activators"),
            (SequenceTarget::FiredTrigger, "@trigger"),
        ] {
            let ctx = ctx_with_reactions(vec![
                sequence("scoped", vec![sentinel_step(sentinel)]),
                sequence("reveal", vec![fire_step("scoped")]),
            ]);
            let capture = LogCapture::start();
            run_pass_b(&ctx);
            capture.assert_logged_once(
                log::Level::Error,
                &format!("sequence step 0 target sentinel `{spelling}`"),
            );
            assert!(is_dropped(&ctx, "reveal"));
            assert!(
                !is_dropped(&ctx, "scoped"),
                "the scoped target remains valid for a direct trigger binding",
            );
        }
    }

    // --- V5 (Pass B) --------------------------------------------------------

    // V5 end-to-end: an interruptible wait on an Enter-bound reaction derives the
    // trigger's Exit edge into `bound_edges`, so cancellation has a source even
    // with no authored `on_exit` KVP.
    #[test]
    fn v5_derives_exit_edge_with_no_authored_on_exit() {
        let ctx = ctx_with_reactions(vec![sequence(
            "reveal",
            vec![wait_step(json!(800), true), entity_step(1)],
        )]);
        // `on_exit` is empty — no authored Exit KVP.
        let trigger = spawn_trigger(&ctx, "reveal", "", TriggerFireMode::Multiple, &[]);
        let table = validated_bindings(&ctx);
        assert!(!is_dropped(&ctx, "reveal"));
        assert!(
            table
                .bound_edges()
                .contains(&(trigger, TriggerEventEdge::Exit)),
            "V5 derived the paired Exit edge",
        );
    }

    // V5 with a non-interruptible wait: no Exit edge is derived (nothing to
    // cancel).
    #[test]
    fn v5_does_not_derive_exit_for_non_interruptible_wait() {
        let ctx = ctx_with_reactions(vec![sequence(
            "reveal",
            vec![wait_step(json!(800), false), entity_step(1)],
        )]);
        let trigger = spawn_trigger(&ctx, "reveal", "", TriggerFireMode::Multiple, &[]);
        let table = validated_bindings(&ctx);
        assert!(
            !table
                .bound_edges()
                .contains(&(trigger, TriggerEventEdge::Exit)),
            "a non-interruptible wait needs no derived Exit edge",
        );
    }

    // --- O36: V2/V3/V5 with a mod-global defineTriggerEvent binding ---------

    // O36: the consumer's reveal is Enter-bound only through a mod-global
    // `defineTriggerEvent({ tag, event: "enter", fire: [reveal] })`. V3 does
    // not drop it (it has an Enter binding) and V5 derives the Exit edge from
    // the tag-matched trigger.
    #[test]
    fn o36_manifest_enter_binding_survives_v3_and_derives_v5_exit() {
        let ctx = ScriptCtx::new();
        ctx.data_registry
            .borrow_mut()
            .replace_global_trigger_events(vec![TriggerEventDescriptor {
                tag: "closet_reveal_plate".to_string(),
                event: "enter".to_string(),
                fire: vec!["reveal".to_string()],
                levels: Vec::new(),
            }]);
        ctx.data_registry.borrow_mut().populate_level(
            vec![sequence(
                "reveal",
                vec![wait_step(json!(800), true), entity_step(1)],
            )],
            Vec::new(),
            &[],
        );
        let trigger = spawn_trigger(
            &ctx,
            "",
            "",
            TriggerFireMode::Multiple,
            &["closet_reveal_plate"],
        );
        let table = validated_bindings(&ctx);
        assert!(
            !is_dropped(&ctx, "reveal"),
            "a manifest-bound reveal is not dropped by V3 (O36)",
        );
        assert!(
            table
                .bound_edges()
                .contains(&(trigger, TriggerEventEdge::Exit)),
            "V5's Exit-edge derivation reaches a manifest-bound trigger (O36)",
        );
    }

    // --- Interruptible wait bound only by a level member `on` -------

    /// One volume-overlap box per trigger, centred at `x`.
    fn place(bridge: &mut TriggerVolumeBridge, trigger: EntityId, x: f32) {
        let center = glam::Vec3::new(x, 0.0, 0.0);
        bridge.insert_for_test(
            trigger,
            center + glam::Vec3::new(-1.0, 0.0, -1.0),
            center + glam::Vec3::new(1.0, 2.0, 1.0),
        );
    }

    // `reveal` holds an interruptible wait and is bound only
    // through a member `t.on("enter", [reveal])`; sibling `t2` carries the
    // same tag and no binding. `reveal` installs (V3 counts the member
    // binding); V5 derives the Exit edge for `t` only; leaving `t` before the
    // landing cancels the tail; entering and leaving `t2` does nothing.
    #[test]
    fn member_enter_alone_installs_an_interruptible_wait_and_only_its_exit_cancels_it() {
        let ctx = ScriptCtx::new();
        let t = spawn_trigger(&ctx, "", "", TriggerFireMode::Multiple, &["plate"]);
        let t2 = spawn_trigger(&ctx, "", "", TriggerFireMode::Multiple, &["plate"]);
        let mut bridge = TriggerVolumeBridge::new();
        place(&mut bridge, t, 0.0);
        place(&mut bridge, t2, 10.0);
        let landed = note_step(&ctx);
        ctx.data_registry
            .borrow_mut()
            .populate_level_with_trigger_events(
                vec![sequence(
                    "reveal",
                    vec![wait_step(json!(800), true), landed],
                )],
                Vec::new(),
                vec![member_enter(t, &["reveal"])],
                Vec::new(),
                &[],
            );

        let capture = LogCapture::start();
        let table = validated_bindings(&ctx);
        assert!(!is_dropped(&ctx, "reveal"), "V3 counts the member binding");
        capture.assert_not_logged(log::Level::Error, "reaction `reveal`");
        let edges = table.bound_edges();
        assert!(edges.contains(&(t, TriggerEventEdge::Enter)));
        assert!(
            edges.contains(&(t, TriggerEventEdge::Exit)),
            "V5 derives the member volume's Exit edge"
        );
        assert!(
            !edges.contains(&(t2, TriggerEventEdge::Enter))
                && !edges.contains(&(t2, TriggerEventEdge::Exit)),
            "the sibling with the same tag gains no edge"
        );

        // Runtime: the frame order of the session — trigger tick, Exit cancels
        // (`evaluate`), residual drain under the fire's origin, landings.
        let scheduler = ReactionScheduler::default();
        scheduler.set_enabled(true);
        let landings = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let mut sequence_registry = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut sequence_registry, scheduler.clone());
        {
            let landings = landings.clone();
            sequence_registry.register("note", move |_id, _args| {
                landings.set(landings.get() + 1);
                Ok(())
            });
        }
        let reaction_registry = ReactionPrimitiveRegistry::new();
        let system_registry = SystemReactionRegistry::new();
        let pawn = {
            let mut registry = ctx.registry.borrow_mut();
            let pawn = registry.spawn(Transform {
                position: glam::Vec3::new(-20.0, 1.0, 0.0),
                ..Transform::default()
            });
            registry
                .set_component(
                    pawn,
                    postretro_foundation::PlayerMovementComponent::from_descriptor(
                        &crate::tests::minimal_player_descriptor(),
                    ),
                )
                .unwrap();
            pawn
        };
        let player = crate::trigger_system::AuthoritativePlayer {
            id: crate::trigger_system::PlayerId::Local(pawn),
            pawn,
        };
        let mut system = crate::trigger_system::TriggerSystem::default();
        let mut frame = |x: f32| -> Vec<(EntityId, TriggerEventEdge)> {
            {
                let mut registry = ctx.registry.borrow_mut();
                let mut transform = *registry.get_component::<Transform>(pawn).unwrap();
                transform.position = glam::Vec3::new(x, 1.0, 0.0);
                registry.set_component(pawn, transform).unwrap();
            }
            scheduler.begin_frame();
            let alive = HashSet::from([player.id]);
            let mut dispatched = Vec::new();
            let mut residuals = Vec::new();
            {
                let mut registry = ctx.registry.borrow_mut();
                let mut slot_table = ctx.slot_table.borrow_mut();
                system.run_authoritative_tick_with_dispatch(
                    &mut registry,
                    &bridge,
                    crate::trigger_system::TriggerTickInputs {
                        players: &[player],
                        use_pressed: &HashMap::new(),
                        tick_dt: 1.0 / 60.0,
                    },
                    crate::trigger_system::TriggerDispatchInputs {
                        alive_players: &alive,
                        bound_edges: table.bound_edges(),
                    },
                    |event, _occupancy, registry| {
                        dispatched.push((event.fire.trigger, event.edge));
                        let execution = table.execute(
                            event.fire.trigger,
                            event.edge,
                            registry,
                            &mut slot_table,
                            &crate::trigger_commands::TriggerFireContext::default(),
                        );
                        if let Some(handle) = execution.residual() {
                            residuals.push((event.fire.trigger, handle));
                        }
                    },
                );
            }
            let exits: Vec<_> = dispatched
                .iter()
                .filter(|(_, edge)| *edge == TriggerEventEdge::Exit)
                .map(|(trigger, _)| (*trigger, player.id))
                .collect();
            scheduler.evaluate(&exits);
            for (trigger, handle) in residuals {
                let standing = system.paired_enters().contains(&(trigger, player.id));
                let _origin = scheduler.begin_origin(trigger, player.id, standing);
                let steps = table.residual(handle).unwrap().steps().to_vec();
                fire_prepartitioned_reactions_with_sequences(
                    &steps,
                    &sequence_registry,
                    &reaction_registry,
                    &system_registry,
                    &ctx,
                    ResidualOrigin::TriggerBinding,
                );
            }
            let data_registry = ctx.data_registry.borrow();
            scheduler.drain_landings(
                &data_registry,
                &sequence_registry,
                &reaction_registry,
                &system_registry,
                &ctx,
            );
            dispatched
        };

        // The sibling: entering and leaving `t2` dispatches nothing.
        assert!(frame(10.0).is_empty());
        assert!(frame(-20.0).is_empty());
        assert_eq!(scheduler.pending_len(), 0);

        // Enter `t`: the wait parks under `t`'s origin.
        assert_eq!(frame(0.0), vec![(t, TriggerEventEdge::Enter)]);
        assert_eq!(scheduler.pending_len(), 1, "the wait parks on Enter");
        // Walk through the sibling while parked: no edge, no cancel.
        assert!(frame(10.0).iter().all(|(trigger, _)| *trigger == t));
        assert_eq!(scheduler.pending_len(), 0, "leaving `t` cancels the tail");
        for _ in 0..80 {
            frame(-20.0);
        }
        assert_eq!(landings.get(), 0, "a cancelled tail never lands");

        // Control: staying inside `t` lets the same wait land.
        frame(0.0);
        assert_eq!(scheduler.pending_len(), 1);
        for _ in 0..80 {
            frame(0.0);
        }
        assert_eq!(landings.get(), 1, "an uncancelled tail lands once");
    }

    // On a `once` volume, a member-bound interruptible wait is dropped by V2,
    // because a cancel would spend the latch.
    #[test]
    fn member_enter_on_a_once_volume_drops_the_interruptible_wait() {
        let ctx = ScriptCtx::new();
        let t = spawn_trigger(&ctx, "", "", TriggerFireMode::Once, &["plate"]);
        ctx.data_registry
            .borrow_mut()
            .populate_level_with_trigger_events(
                vec![sequence(
                    "reveal",
                    vec![wait_step(json!(800), true), entity_step(1)],
                )],
                Vec::new(),
                vec![member_enter(t, &["reveal"])],
                Vec::new(),
                &[],
            );
        let capture = LogCapture::start();
        let table = validated_bindings(&ctx);
        capture.assert_logged_once(log::Level::Error, "(V2)");
        assert!(is_dropped(&ctx, "reveal"));
        let (commands, steps) = enter_execution(&table, &ctx, t);
        assert_eq!(commands, 0);
        assert!(steps.is_none(), "the dropped reaction binds nothing");
    }

    // --- O46: body whose first step is the wait -----------------------------

    // O46: a non-interruptible wait whose body's first step is the wait, bound
    // solely by a level member `t.on("enter", …)`, still installs (Pass A/B
    // leave it untouched) — the binder half of O46 lives in the trigger binder,
    // but the validation passes must not reject it.
    #[test]
    fn o46_first_step_wait_survives_validation() {
        let ctx = ScriptCtx::new();
        let trigger = spawn_trigger(&ctx, "", "", TriggerFireMode::Multiple, &["plate"]);
        ctx.data_registry
            .borrow_mut()
            .populate_level_with_trigger_events(
                vec![sequence(
                    "reveal",
                    vec![wait_step(json!(800), false), entity_step(1)],
                )],
                Vec::new(),
                vec![member_enter(trigger, &["reveal"])],
                Vec::new(),
                &[],
            );
        let table = validated_bindings(&ctx);
        assert!(!is_dropped(&ctx, "reveal"));
        assert!(
            table
                .bound_edges()
                .contains(&(trigger, TriggerEventEdge::Enter)),
            "a wait-first residual is non-empty, so the Enter edge binds (O46)",
        );
    }

    // A reaction that is both Enter-bound and a `fire` target keeps its Enter
    // instance: V3 counts the Enter binding, so an interruptible wait there is not
    // demoted at install (the sourceless `fire`-path demotion is Task 5's runtime
    // concern, not an install rejection).
    #[test]
    fn interruptible_wait_with_enter_binding_is_not_dropped() {
        let ctx = ctx_with_reactions(vec![sequence(
            "reveal",
            vec![wait_step(json!(800), true), entity_step(1)],
        )]);
        spawn_trigger(&ctx, "reveal", "", TriggerFireMode::Multiple, &[]);
        run_pass_b(&ctx);
        assert!(!is_dropped(&ctx, "reveal"));
    }

    // --- Production-order rejection ↔ binder tests --------------------------
    //
    // Regression: Pass B once ran AFTER `build_trigger_bindings`, so emptying a
    // rejected reaction's `DataRegistry` body was a no-op for trigger-bound
    // content — the binder had already copied the body into owned commands and
    // residual steps the runtime drain iterates. A V2-rejected interruptible
    // wait still enrolled on Enter (and a paired Exit spent the `once` latch
    // permanently), and a V4b-rejected pre-wait `fire` still dispatched as a
    // `DeferredEvent`. These tests go through `validated_bindings` (the real
    // production order) and assert runtime inertness, never just the registry
    // drop.

    /// An Entity-targeted presentation step on a freshly spawned entity, using
    /// the registered-at-drain `note` primitive so the residual drain can
    /// dispatch it without warns.
    fn note_step(ctx: &ScriptCtx) -> SequenceStep {
        let id = ctx.registry.borrow_mut().spawn(Transform::default());
        SequenceStep {
            id: SequenceTarget::Entity(id),
            primitive: "note".to_string(),
            args: json!({}),
        }
    }

    /// Drain residual steps exactly as the frame-end drain does — a live
    /// scheduler with `wait`/`fire` registered — and report the chained names
    /// plus how many instances enrolled.
    fn drain_trigger_residual(
        ctx: &ScriptCtx,
        steps: &[PrepartitionedReactionStep],
    ) -> (Vec<String>, usize) {
        let scheduler = ReactionScheduler::default();
        scheduler.set_enabled(true);
        let mut sequence_registry = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut sequence_registry, scheduler.clone());
        sequence_registry.register("note", |_id, _args| Ok(()));
        let follow_ups = fire_prepartitioned_reactions_with_sequences(
            steps,
            &sequence_registry,
            &ReactionPrimitiveRegistry::new(),
            &SystemReactionRegistry::new(),
            ctx,
            ResidualOrigin::TriggerBinding,
        );
        (follow_ups, scheduler.pending_len())
    }

    // V2 with the binder in the loop: the rejected reaction is never bound, so
    // its Enter fire yields no in-tick command and no residual — nothing can
    // enroll, and no Exit cancel can ever spend the `once` latch. Covered both
    // without and with an authored `on_exit` (the authored Exit binding belongs
    // to its own reaction and must survive).
    #[test]
    fn v2_rejected_once_reaction_is_never_bound_and_cannot_enroll() {
        for on_exit in ["", "exitCue"] {
            let ctx = ctx_with_reactions(vec![
                sequence("reveal", vec![wait_step(json!(800), true), entity_step(1)]),
                sequence("exitCue", vec![entity_step(2)]),
            ]);
            let trigger = spawn_trigger(&ctx, "reveal", on_exit, TriggerFireMode::Once, &[]);
            let capture = LogCapture::start();
            let table = validated_bindings(&ctx);
            capture.assert_logged_once(log::Level::Error, "reaction `reveal` step 0");
            assert!(is_dropped(&ctx, "reveal"));
            let (commands, steps) = enter_execution(&table, &ctx, trigger);
            assert_eq!(commands, 0, "no in-tick command for the dropped reaction");
            assert!(
                steps.is_none(),
                "no Enter residual: the dropped reaction cannot enroll at runtime \
                 (authored on_exit: `{on_exit}`)",
            );
            assert!(
                !table
                    .bound_edges()
                    .contains(&(trigger, TriggerEventEdge::Enter)),
                "the Enter edge is not even registered",
            );
            let exit_bound = table
                .bound_edges()
                .contains(&(trigger, TriggerEventEdge::Exit));
            if on_exit.is_empty() {
                assert!(!exit_bound, "V5 derives nothing for a dropped reaction");
            } else {
                assert!(exit_bound, "the authored `exitCue` Exit binding survives");
            }
        }
    }

    // V2 against the merged-residual reality: `bind_event` merges every
    // reaction matched on one `(trigger, edge)` into a single residual. The
    // innocent manifest-bound sibling keeps its steps; the rejected reaction
    // contributes none; and the REAL frame-end drain enrolls nothing.
    #[test]
    fn v2_rejection_leaves_sibling_in_merged_residual_and_nothing_enrolls() {
        let ctx = ScriptCtx::new();
        let trigger = spawn_trigger(&ctx, "reveal", "", TriggerFireMode::Once, &["plate"]);
        let innocent_body = vec![note_step(&ctx)];
        ctx.data_registry
            .borrow_mut()
            .populate_level_with_trigger_events(
                vec![
                    sequence("reveal", vec![wait_step(json!(800), true), entity_step(1)]),
                    sequence("innocent", innocent_body),
                ],
                Vec::new(),
                vec![member_enter(trigger, &["innocent"])],
                Vec::new(),
                &[],
            );

        let table = validated_bindings(&ctx);
        assert!(is_dropped(&ctx, "reveal"));
        assert!(!is_dropped(&ctx, "innocent"));

        let (commands, steps) = enter_execution(&table, &ctx, trigger);
        assert_eq!(commands, 0);
        let steps = steps.expect("the innocent sibling keeps the Enter residual");
        assert!(
            steps.iter().all(|step| matches!(
                step,
                PrepartitionedReactionStep::Descriptor(name, _, _) if name == "innocent"
            )),
            "the merged residual holds only the innocent sibling's steps: {steps:?}",
        );

        let (follow_ups, pending) = drain_trigger_residual(&ctx, &steps);
        assert!(follow_ups.is_empty());
        assert_eq!(
            pending, 0,
            "no instance parks for the rejected reaction's wait",
        );
    }

    // V4b with the binder in the loop: the pre-wait `fire` of a scoped
    // `setState` is never lowered to a `DeferredEvent`, because the reaction is
    // dropped before the binder runs.
    #[test]
    fn v4b_rejected_pre_wait_fire_never_reaches_the_enter_residual() {
        let scoped_value = json!({
            "op": "select",
            "cond": { "op": "input", "name": "@rising" },
            "a": { "op": "const", "value": 1.0 },
            "b": { "op": "const", "value": 0.0 }
        });
        let ctx = ctx_with_reactions(vec![
            set_state("raiseAlarm", "puzzle.target", scoped_value),
            sequence(
                "reveal",
                vec![
                    fire_step("raiseAlarm"),
                    wait_step(json!(800), false),
                    entity_step(1),
                ],
            ),
        ]);
        insert_writable_number(&ctx, "puzzle.target");
        let trigger = spawn_trigger(&ctx, "reveal", "", TriggerFireMode::Multiple, &[]);
        let table = validated_bindings(&ctx);
        assert!(is_dropped(&ctx, "reveal"));
        let (commands, steps) = enter_execution(&table, &ctx, trigger);
        assert_eq!(commands, 0);
        assert!(
            steps.is_none(),
            "no Enter residual: the scoped `fire` can never dispatch contextless",
        );
    }

    // Regression: sentinel-scoped fire targets must be rejected before the
    // trigger binder copies the pre-wait fire into a DeferredEvent.
    #[test]
    fn v4b_rejected_sentinel_fire_never_reaches_the_enter_residual() {
        let ctx = ctx_with_reactions(vec![
            activator_primitive("damageActivators"),
            sequence(
                "reveal",
                vec![
                    fire_step("damageActivators"),
                    wait_step(json!(800), false),
                    entity_step(1),
                ],
            ),
        ]);
        let trigger = spawn_trigger(&ctx, "reveal", "", TriggerFireMode::Multiple, &[]);
        let table = validated_bindings(&ctx);
        assert!(is_dropped(&ctx, "reveal"));
        let (commands, steps) = enter_execution(&table, &ctx, trigger);
        assert_eq!(commands, 0);
        assert!(
            steps.is_none(),
            "the scoped sentinel target cannot survive into a contextless DeferredEvent",
        );
    }

    // V4b's merged-residual variant, driven through the real drain: the
    // rejected reaction's `DeferredEvent` must be absent, so the drain chains
    // no `raiseAlarm` dispatch, while the innocent sibling still runs.
    #[test]
    fn v4b_rejection_strips_deferred_event_from_merged_residual_drain() {
        let ctx = ScriptCtx::new();
        let trigger = spawn_trigger(&ctx, "reveal", "", TriggerFireMode::Multiple, &["plate"]);
        let scoped_value = json!({
            "op": "select",
            "cond": { "op": "input", "name": "@rising" },
            "a": { "op": "const", "value": 1.0 },
            "b": { "op": "const", "value": 0.0 }
        });
        let innocent_body = vec![note_step(&ctx)];
        ctx.data_registry
            .borrow_mut()
            .populate_level_with_trigger_events(
                vec![
                    set_state("raiseAlarm", "puzzle.target", scoped_value),
                    sequence(
                        "reveal",
                        vec![
                            fire_step("raiseAlarm"),
                            wait_step(json!(800), false),
                            entity_step(1),
                        ],
                    ),
                    sequence("innocent", innocent_body),
                ],
                Vec::new(),
                vec![member_enter(trigger, &["innocent"])],
                Vec::new(),
                &[],
            );
        insert_writable_number(&ctx, "puzzle.target");

        let table = validated_bindings(&ctx);
        assert!(is_dropped(&ctx, "reveal"));

        let (commands, steps) = enter_execution(&table, &ctx, trigger);
        assert_eq!(commands, 0);
        let steps = steps.expect("the innocent sibling keeps the Enter residual");
        assert!(
            steps
                .iter()
                .all(|step| !matches!(step, PrepartitionedReactionStep::DeferredEvent(_))),
            "no `DeferredEvent` survives from the rejected pre-wait `fire`: {steps:?}",
        );

        let (follow_ups, pending) = drain_trigger_residual(&ctx, &steps);
        assert!(
            follow_ups.is_empty(),
            "the drain chains no contextless `raiseAlarm` dispatch",
        );
        assert_eq!(pending, 0);
    }

    // O40's validation half at the seam level: `recompose_active_sets` rebuilds
    // `DataRegistry.reactions` from retained originals, erasing the in-place
    // drop — the staged commit must re-validate and rebind, not inherit the
    // stale verdict, and the rebuilt table must again bind nothing for the
    // offender.
    #[test]
    fn staged_recompose_revalidates_and_rebinds_without_stale_bindings() {
        let ctx = ctx_with_reactions(vec![sequence(
            "reveal",
            vec![wait_step(json!(800), true), entity_step(1)],
        )]);
        let trigger = spawn_trigger(&ctx, "reveal", "", TriggerFireMode::Once, &[]);

        let table = validated_bindings(&ctx);
        assert!(is_dropped(&ctx, "reveal"));
        assert!(enter_execution(&table, &ctx, trigger).1.is_none());

        // Hot reload: the recompose restores the authored body...
        ctx.data_registry.borrow_mut().recompose_active_sets(&[]);
        assert!(
            !is_dropped(&ctx, "reveal"),
            "recompose restores the authored body from retained originals",
        );

        // ...and the staged-commit sequence re-runs validation before rebinding.
        let table = validated_bindings(&ctx);
        assert!(is_dropped(&ctx, "reveal"), "the verdict is re-derived");
        let (commands, steps) = enter_execution(&table, &ctx, trigger);
        assert_eq!(commands, 0);
        assert!(
            steps.is_none(),
            "the rebuilt table is inert for the offender"
        );
    }

    // Regression: staged recompose restores raw descriptor bodies, so the
    // broadened V4b descriptor-scope analysis must rerun before the fresh
    // binder just like the system-IR analysis does.
    #[test]
    fn staged_recompose_rejects_sentinel_scoped_fire_before_rebinding() {
        let ctx = ctx_with_reactions(vec![
            activator_primitive("damageActivators"),
            sequence("reveal", vec![fire_step("damageActivators")]),
        ]);
        let trigger = spawn_trigger(&ctx, "reveal", "", TriggerFireMode::Multiple, &[]);

        let table = validated_bindings(&ctx);
        assert!(is_dropped(&ctx, "reveal"));
        assert!(enter_execution(&table, &ctx, trigger).1.is_none());

        ctx.data_registry.borrow_mut().recompose_active_sets(&[]);
        assert!(!is_dropped(&ctx, "reveal"));

        let table = validated_bindings(&ctx);
        assert!(is_dropped(&ctx, "reveal"));
        assert!(
            enter_execution(&table, &ctx, trigger).1.is_none(),
            "the staged binder never receives the restored scoped fire",
        );
    }

    /// Bundle `source`, a TypeScript level script importing `"postretro"`,
    /// through the `scripts-build` library and run its `setupLevel` the way
    /// level load does, returning the SDK-authored reactions.
    fn sdk_reactions(source: &str) -> Vec<NamedReaction> {
        use postretro_level_format::data_script::DataScriptSection;
        use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
        use postretro_scripting_core::runtime::{ScriptRuntime, ScriptRuntimeConfig};

        let dir = tempfile::tempdir().expect("script dir");
        let entry = dir.path().join("level.ts");
        std::fs::write(&entry, source).expect("level script writes");
        let bundled =
            postretro_script_compiler::bundle_entry(&entry).expect("level script bundles");
        let runtime = ScriptRuntime::new(
            &PrimitiveRegistry::new(),
            &ScriptRuntimeConfig::default(),
            &ScriptCtx::new(),
        )
        .expect("script runtime constructs");
        runtime
            .run_data_script(
                &DataScriptSection {
                    compiled_bytes: bundled.into_bytes(),
                    source_path: entry.to_string_lossy().into_owned(),
                },
                dir.path(),
            )
            .reactions
    }

    // Runs SDK-authored reactions through the real `setupLevel` order: id-step
    // primitive validation, then Pass A. A subject-token entry after a `wait`
    // drops its reaction with an error naming it (V4a), never as an unnamed
    // unknown primitive; the same entry before the `wait` installs, and a
    // `players()` group entry after the `wait` installs and lands.
    #[test]
    fn subject_token_after_wait_drops_while_a_group_step_after_wait_installs_and_lands() {
        use postretro_entities::components::health::HealthComponent;
        use postretro_entities::data_descriptors::HealthDescriptor;
        use postretro_scripting_core::reaction_dispatch::{
            fire_named_event_with_sequences, validate_sequence_primitives,
        };

        let scheduler = ReactionScheduler::default();
        scheduler.set_enabled(true);
        let mut sequence_registry = SequencedPrimitiveRegistry::new();
        register_reaction_control_primitives(&mut sequence_registry, scheduler.clone());
        let mut reaction_registry = ReactionPrimitiveRegistry::new();
        crate::scripting::reactions::registry::register_grant_reactions(&mut reaction_registry);

        let authored = sdk_reactions(
            r#"
            import { defineReaction, players, wait, type TriggerEventParams } from "postretro";
            export function setupLevel() {
              return {
                reactions: [
                  defineReaction("tokenAfterWait", (on: TriggerEventParams) => ({
                    sequence: [...wait(800), on.activators.grantHealth(5)],
                  })),
                  defineReaction("tokenBeforeWait", (on: TriggerEventParams) => ({
                    sequence: [on.activators.grantHealth(5), on.trigger.disarm(), ...wait(800)],
                  })),
                  defineReaction("groupAfterWait", {
                    sequence: [...wait(800), players().grantHealth(5)],
                  }),
                ],
              };
            }
            "#,
        );
        assert_eq!(
            authored.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["tokenAfterWait", "tokenBeforeWait", "groupAfterWait"],
            "every SDK-authored reaction parses"
        );

        let capture = LogCapture::start();
        let installed = validate_sequence_primitives(authored, &sequence_registry);
        assert_eq!(
            installed.len(),
            3,
            "no step is an unknown id-step primitive"
        );
        let ctx = ctx_with_reactions(installed);
        validate_reaction_bodies_pass_a(&ctx);
        capture.assert_logged_once(log::Level::Error, "reaction `tokenAfterWait` step 1");
        capture.assert_not_logged(log::Level::Error, "names unknown primitive");
        capture.assert_not_logged(log::Level::Error, "reaction `groupAfterWait`");
        capture.assert_not_logged(log::Level::Error, "reaction `tokenBeforeWait`");
        assert!(is_dropped(&ctx, "tokenAfterWait"));
        assert!(!is_dropped(&ctx, "tokenBeforeWait"));
        assert!(!is_dropped(&ctx, "groupAfterWait"));

        let pawn = {
            let mut registry = ctx.registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            let mut health = HealthComponent::from_descriptor(&HealthDescriptor {
                max: 100.0,
                hitbox: None,
                zone_multipliers: HashMap::new(),
            });
            health.current = 50.0;
            registry.set_component(pawn, health).unwrap();
            registry.bind_pawn_seat(pawn, postretro_foundation::Seat(0));
            pawn
        };
        let mut data = postretro_entities::DataRegistry::new();
        data.populate_level(
            ctx.data_registry.borrow().reactions.clone(),
            Vec::new(),
            &[],
        );
        let system_registry = SystemReactionRegistry::new();
        let _ = fire_named_event_with_sequences(
            "groupAfterWait",
            &data,
            &sequence_registry,
            &reaction_registry,
            &system_registry,
            &ctx,
            None,
        );
        assert_eq!(scheduler.pending_len(), 1);
        for _ in 0..60 {
            scheduler.begin_frame();
            scheduler.evaluate(&[]);
            scheduler.drain_landings(
                &data,
                &sequence_registry,
                &reaction_registry,
                &system_registry,
                &ctx,
            );
        }
        assert_eq!(scheduler.pending_len(), 0);
        assert_eq!(
            ctx.registry
                .borrow()
                .get_component::<HealthComponent>(pawn)
                .unwrap()
                .current,
            55.0,
            "the group step lands on the seat-bound pawn",
        );
    }
}
