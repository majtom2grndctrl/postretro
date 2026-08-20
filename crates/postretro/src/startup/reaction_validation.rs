// E18 install-time validation for timed/delayed reaction steps.
//
// Two read-only passes reject malformed and context-dependent `wait`/`fire`
// bodies before any frame runs, and derive the paired Exit edge an interruptible
// wait needs as its cancel source. Neither pass rewrites a body: a rejected
// reaction is replaced in place with an inert `Sequence(vec![])` so no later pass
// or index observes a shifted vector, and the level installs with every other
// reaction unaffected — matching how `validate_sequence_primitives` already drops
// one whole reaction for one bad step.
//
// The passes run at two points in `install_world_cpu` because their inputs
// arrive at different times, and again in the staged-manifest commit block so a
// hot reload re-validates rather than inheriting a stale verdict:
//   * Pass A (V1, V4a, V6) needs only the `DataRegistry`, and runs where
//     `slot_accumulator_bindings.rebuild` sits, before any body is bound.
//   * Pass B (V2, V3, V4b, V5) needs reaction-to-trigger provenance and the
//     bound system-reaction programs, so it runs after `install_manifest_events`
//     (install) / after both binder rebuilds (staged commit).
//
// See: context/plans/in-progress/E18--timed-reaction-steps/index.md — Install
// validation.

use std::collections::{HashMap, HashSet};

use postretro_entities::{
    ComponentKind, EntityId, ScriptCtx, TriggerFireMode, TriggerVolumeComponent,
};
use postretro_scripting_core::data_descriptors::{ReactionDescriptor, SequenceStep, SequenceTarget};

use crate::scripting_systems::system_reactions::SystemReactionIrBindings;
use crate::trigger_bindings::TriggerBindingTable;
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

/// Pass A — body-only validation (V1, V4a, V6). Reads and mutates the
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
        if let Some(wait_pos) = steps
            .iter()
            .position(|step| matches!(step.id, SequenceTarget::Wait))
        {
            if let Some(offset) = steps[wait_pos + 1..].iter().position(|step| {
                matches!(
                    step.id,
                    SequenceTarget::Activators | SequenceTarget::FiredTrigger
                )
            }) {
                let step_index = wait_pos + 1 + offset;
                log::error!(
                    "[Scripting] reaction `{name}` step {step_index}: a post-`wait` step targets a trigger-fire sentinel (@activators/@trigger) whose fire context no `wait` survives; dropping the reaction (V4a)"
                );
                data_registry.reactions[index].descriptor =
                    ReactionDescriptor::Sequence(Vec::new());
                continue;
            }
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

/// Pass B — trigger-coupled validation (V2, V3, V4b) and the V5 Exit-edge
/// derivation. Provenance is read from the sources the binder read — never from
/// `TriggerBindingTable`, which discards reaction identity: brush-KVP
/// `TriggerVolumeComponent.on_fire`/`on_exit` across `EntityRegistry` triggers,
/// plus manifest `data_registry.trigger_events`. V4b reads the precomputed
/// `required_dispatch_inputs` from `system_bindings`. V5 derives the paired Exit
/// edge on `trigger_bindings` via `bind_edge_only`.
pub(crate) fn validate_trigger_coupled_pass_b(
    script_ctx: &ScriptCtx,
    trigger_bindings: &mut TriggerBindingTable,
    system_bindings: &SystemReactionIrBindings,
) {
    // Reactions whose bound system-`setState` program reads a seeded dispatch
    // input (e.g. `@rising`). A `fire` step targeting one has no fire context on
    // the app drain, so V4b rejects the reaction that fires it (at any position).
    let scoped_fire_targets: HashSet<String> = system_bindings
        .reaction_dispatch_inputs()
        .filter(|(_, inputs)| !inputs.is_empty())
        .map(|(name, _)| name.to_string())
        .collect();

    // Enter-binding provenance: reaction name → triggers it is Enter-bound to,
    // plus per-trigger `fire_mode` for V2.
    let mut enter_triggers: HashMap<String, Vec<EntityId>> = HashMap::new();
    let mut trigger_fire_mode: HashMap<EntityId, TriggerFireMode> = HashMap::new();
    {
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

        // Manifest `onTriggerEvent` bindings: (tag, "enter", fire names).
        let trigger_events = script_ctx.data_registry.borrow().trigger_events.clone();
        for descriptor in &trigger_events {
            if descriptor.event != "enter" {
                continue;
            }
            let mut triggers: Vec<EntityId> = registry
                .query_by_component_and_tag(ComponentKind::TriggerVolume, Some(&descriptor.tag))
                .map(|(id, _)| id)
                .collect();
            triggers.sort_unstable();
            for event_name in &descriptor.fire {
                for &trigger in &triggers {
                    enter_triggers
                        .entry(event_name.clone())
                        .or_default()
                        .push(trigger);
                }
            }
        }
    }

    let mut rejects: Vec<usize> = Vec::new();
    // Every Enter trigger of a surviving interruptible-wait reaction gets its
    // paired Exit edge derived (V5), deduplicated below.
    let mut exit_derivations: Vec<EntityId> = Vec::new();
    {
        let data_registry = script_ctx.data_registry.borrow();
        for (index, reaction) in data_registry.reactions.iter().enumerate() {
            let ReactionDescriptor::Sequence(steps) = &reaction.descriptor else {
                continue;
            };
            let name = &reaction.name;

            // V4b: any `fire` step whose target reads a seeded dispatch input.
            if let Some(step_index) = steps.iter().position(|step| {
                matches!(step.id, SequenceTarget::Fire)
                    && step
                        .args
                        .get("event")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|event| scoped_fire_targets.contains(event))
            }) {
                log::error!(
                    "[Scripting] reaction `{name}` step {step_index}: `fire` targets a system reaction that reads fire-time dispatch context, which no resumed step has; dropping the reaction (V4b)"
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
            let enter = enter_triggers.get(name);

            // V2: Enter-bound to a `once` trigger — the latch is spent on first
            // fire, so a cancel destroys the set-piece permanently.
            if enter.is_some_and(|triggers| {
                triggers
                    .iter()
                    .any(|trigger| matches!(trigger_fire_mode.get(trigger), Some(TriggerFireMode::Once)))
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

            // V5: derive the paired Exit edge for every Enter trigger, so an
            // interruptible wait always has a live cancel source even with no
            // authored `on_exit` KVP.
            if let Some(triggers) = enter {
                exit_derivations.extend(triggers.iter().copied());
            }
        }
    }

    if !rejects.is_empty() {
        let mut data_registry = script_ctx.data_registry.borrow_mut();
        for index in rejects {
            data_registry.reactions[index].descriptor = ReactionDescriptor::Sequence(Vec::new());
        }
    }

    // Deduplicate: a reaction Enter-bound to two triggers, or two reactions
    // sharing a trigger, must not insert the same edge twice (the insert is
    // idempotent anyway, this only avoids redundant work).
    let mut derived: HashSet<EntityId> = HashSet::new();
    for trigger in exit_derivations {
        if derived.insert(trigger) {
            trigger_bindings.bind_edge_only(trigger, TriggerEventEdge::Exit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::{
        MoverCommand, NamedReaction, PrimitiveDescriptor, SlotOwnership, SlotRecord, SlotSchema,
        SlotType, SlotValue, Transform, TriggerActivation, TriggerEventDescriptor,
        TriggerVolumeComponent,
    };
    use postretro_test_log_capture::LogCapture;
    use serde_json::json;

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
            primitive: "updateEnemyState".to_string(),
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
                tag: None,
                on_complete: None,
                args: json!({ "slot": slot, "value": value }),
            }),
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
                    range: Some(postretro_entities::NumericRange { min: 0.0, max: 100.0 }),
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

    fn empty_table() -> TriggerBindingTable {
        TriggerBindingTable::default()
    }

    fn run_pass_b(ctx: &ScriptCtx, table: &mut TriggerBindingTable) {
        let bindings = built_system_bindings(ctx);
        validate_trigger_coupled_pass_b(ctx, table, &bindings);
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
            assert!(is_dropped(&ctx, "bad"), "malformed duration {bad} drops `bad`");
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
        let mut table = empty_table();
        run_pass_b(&ctx, &mut table);
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
        let mut table = empty_table();
        run_pass_b(&ctx, &mut table);
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
        let mut table = empty_table();
        run_pass_b(&ctx, &mut table);
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
        let mut table = empty_table();
        run_pass_b(&ctx, &mut table);
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
        let mut table = empty_table();
        run_pass_b(&ctx, &mut table);
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
        let mut table = empty_table();
        run_pass_b(&ctx, &mut table);
        assert!(!is_dropped(&ctx, "reveal"));
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
        let mut table = empty_table();
        assert!(
            !table
                .bound_edges()
                .contains(&(trigger, TriggerEventEdge::Exit)),
            "no Exit edge before Pass B",
        );
        run_pass_b(&ctx, &mut table);
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
        let mut table = empty_table();
        run_pass_b(&ctx, &mut table);
        assert!(
            !table
                .bound_edges()
                .contains(&(trigger, TriggerEventEdge::Exit)),
            "a non-interruptible wait needs no derived Exit edge",
        );
    }

    // --- O36: V2/V3/V5 with a manifest onTriggerEvent binding ---------------

    // O36: the consumer's reveal is Enter-bound only through a manifest
    // `onTriggerEvent({tag}, "enter", [reveal])`. V3 does not drop it (it has an
    // Enter binding) and V5 derives the Exit edge from the tag-matched trigger.
    #[test]
    fn o36_manifest_enter_binding_survives_v3_and_derives_v5_exit() {
        let ctx = ScriptCtx::new();
        ctx.data_registry.borrow_mut().populate_level_with_trigger_events(
            vec![sequence(
                "reveal",
                vec![wait_step(json!(800), true), entity_step(1)],
            )],
            Vec::new(),
            vec![TriggerEventDescriptor {
                tag: "closet_reveal_plate".to_string(),
                event: "enter".to_string(),
                fire: vec!["reveal".to_string()],
                levels: Vec::new(),
            }],
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
        let mut table = empty_table();
        run_pass_b(&ctx, &mut table);
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

    // --- O46: body whose first step is the wait -----------------------------

    // O46: a non-interruptible wait whose body's first step is the wait, bound
    // solely by a manifest `onTriggerEvent`, still installs (Pass A/B leave it
    // untouched) — the binder half of O46 lives in the trigger binder, but the
    // validation passes must not reject it.
    #[test]
    fn o46_first_step_wait_survives_validation() {
        let ctx = ScriptCtx::new();
        ctx.data_registry.borrow_mut().populate_level_with_trigger_events(
            vec![sequence(
                "reveal",
                vec![wait_step(json!(800), false), entity_step(1)],
            )],
            Vec::new(),
            vec![TriggerEventDescriptor {
                tag: "plate".to_string(),
                event: "enter".to_string(),
                fire: vec!["reveal".to_string()],
                levels: Vec::new(),
            }],
            Vec::new(),
            &[],
        );
        spawn_trigger(&ctx, "", "", TriggerFireMode::Multiple, &["plate"]);
        validate_reaction_bodies_pass_a(&ctx);
        let mut table = empty_table();
        run_pass_b(&ctx, &mut table);
        assert!(!is_dropped(&ctx, "reveal"));
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
        let mut table = empty_table();
        run_pass_b(&ctx, &mut table);
        assert!(!is_dropped(&ctx, "reveal"));
    }
}
