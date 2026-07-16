//! Bind trigger event reactions at level install and execute their fixed-tick work.
//! See: context/lib/entity_model.md §5 · context/lib/scripting.md §10

use std::collections::{HashMap, HashSet};

use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, MoverCommand, ScriptCtx, SlotTable,
    TriggerVolumeComponent,
};
use postretro_foundation::{BakedIr, CURRENT_IR_VERSION, ir_node_from_json};
use postretro_scripting_core::data_descriptors::{
    NamedReaction, PrimitiveDescriptor, ProgressDescriptor, ReactionDescriptor, SequenceStep,
};
use postretro_scripting_core::data_registry::DataRegistry;
use postretro_scripting_core::ir::bind;
use postretro_scripting_core::ir_scopes::StoreScope;
use postretro_scripting_core::reaction_dispatch::PrepartitionedReactionStep;
use postretro_scripting_core::store_bridge::{json_value_for_slot, validate_slot_value};
use serde::Deserialize;

use crate::health::reactions::ApplyDamageArgs;
use crate::kinematic_mover::MoverCommandDiagnostics;
use crate::scripting::reactions::animation::SetAnimationStateArgs;
#[cfg(test)]
pub(crate) use crate::trigger_commands::BoundTriggerCommandKind;
use crate::trigger_commands::{BoundStoreValue, BoundTarget, BoundTriggerCommand};
use crate::trigger_system::TriggerEventEdge;

const CONSEQUENTIAL_PRIMITIVES: &[&str] = &[
    "moverStart",
    "moverStop",
    "moverReverse",
    "moverGoToPathNode",
    "applyDamage",
    "armTrigger",
    "disarmTrigger",
    "setState",
    "setAnimationState",
];

const LIFECYCLE_PRIMITIVES: &[&str] = &["loadLevel", "restartLevel", "returnToFrontend"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TriggerResidualHandle(usize);

#[derive(Debug, Clone)]
pub(crate) struct TriggerResidual {
    steps: Vec<PrepartitionedReactionStep>,
}

impl TriggerResidual {
    pub(crate) fn steps(&self) -> &[PrepartitionedReactionStep] {
        &self.steps
    }
}

#[derive(Debug, Default)]
pub(crate) struct TriggerBindingTable {
    bindings: HashMap<(EntityId, TriggerEventEdge), TriggerBinding>,
    residuals: Vec<TriggerResidual>,
    command_diagnostics: MoverCommandDiagnostics,
}

#[derive(Debug)]
struct TriggerBinding {
    commands: Vec<BoundTriggerCommand>,
    residual: Option<TriggerResidualHandle>,
}

/// Result of one fixed-tick binding execution. The test-only command list is
/// deliberately captured at the command-buffer boundary, where duplicate
/// partitioning cannot hide behind idempotent final component state.
#[derive(Debug)]
pub(crate) struct TriggerBindingExecution {
    residual: Option<TriggerResidualHandle>,
    #[cfg(test)]
    pub(crate) commands: Vec<BoundTriggerCommandKind>,
}

impl TriggerBindingExecution {
    pub(crate) fn residual(self) -> Option<TriggerResidualHandle> {
        self.residual
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrimitiveClass {
    Consequential,
    Lifecycle,
    Presentation,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetStateArgs {
    slot: String,
    value: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct MoverGoToPathNodeArgs {
    node: String,
}

impl TriggerBindingTable {
    /// Construct bindings after reaction composition has completed. Empty event
    /// names intentionally carry no binding; unknown names warn once per trigger
    /// edge and do not fall back to a later drain-time lookup.
    #[cfg(test)]
    pub(crate) fn build(
        registry: &EntityRegistry,
        data_registry: &DataRegistry,
        slot_table: &SlotTable,
    ) -> Self {
        Self::build_inner(registry, data_registry, slot_table, None)
    }

    /// Bind against the script-capability `StoreScope` at level install. The
    /// standalone table-only builder remains for literal-only test fixtures;
    /// real level installation must use this path so IR writes are validated
    /// against the live slot declarations once.
    #[cfg(test)]
    pub(crate) fn build_with_script_ctx(
        registry: &EntityRegistry,
        data_registry: &DataRegistry,
        script_ctx: &ScriptCtx,
    ) -> Self {
        let slot_table = script_ctx.slot_table.borrow();
        Self::build_inner(registry, data_registry, &slot_table, Some(script_ctx))
    }

    pub(crate) fn build_with_script_ctx_and_diagnostics(
        registry: &EntityRegistry,
        data_registry: &DataRegistry,
        script_ctx: &ScriptCtx,
        command_diagnostics: MoverCommandDiagnostics,
    ) -> Self {
        let slot_table = script_ctx.slot_table.borrow();
        let mut table = Self::build_inner(registry, data_registry, &slot_table, Some(script_ctx));
        table.command_diagnostics = command_diagnostics;
        table
    }

    fn build_inner(
        registry: &EntityRegistry,
        data_registry: &DataRegistry,
        slot_table: &SlotTable,
        script_ctx: Option<&ScriptCtx>,
    ) -> Self {
        let mut trigger_ids: Vec<EntityId> = registry
            .iter_with_kind(ComponentKind::TriggerVolume)
            .map(|(id, _)| id)
            .collect();
        trigger_ids.sort_unstable();

        let mut table = Self::default();
        for trigger in trigger_ids {
            let Ok(component) = registry
                .get_component::<TriggerVolumeComponent>(trigger)
                .cloned()
            else {
                continue;
            };
            table.bind_event(
                trigger,
                TriggerEventEdge::Enter,
                &component.on_fire,
                data_registry,
                slot_table,
                script_ctx,
            );
            table.bind_event(
                trigger,
                TriggerEventEdge::Exit,
                &component.on_exit,
                data_registry,
                slot_table,
                script_ctx,
            );
        }
        table
    }

    fn bind_event(
        &mut self,
        trigger: EntityId,
        edge: TriggerEventEdge,
        event_name: &str,
        data_registry: &DataRegistry,
        slot_table: &SlotTable,
        script_ctx: Option<&ScriptCtx>,
    ) {
        if event_name.is_empty() {
            return;
        }
        let matched: Vec<&NamedReaction> = data_registry
            .reactions
            .iter()
            .filter(|reaction| reaction.name == event_name)
            .collect();
        if matched.is_empty() {
            log::warn!(
                "[Trigger] {edge:?} event `{event_name}` on {trigger} does not match an active composed reaction; not binding"
            );
            return;
        }

        let mut commands = Vec::new();
        let mut steps = Vec::new();
        for reaction in &matched {
            partition_direct_reaction(
                reaction,
                data_registry,
                slot_table,
                script_ctx,
                &mut commands,
                &mut steps,
            );
        }

        let residual = (!steps.is_empty()).then(|| {
            let handle = TriggerResidualHandle(self.residuals.len());
            self.residuals.push(TriggerResidual { steps });
            handle
        });
        self.bindings
            .insert((trigger, edge), TriggerBinding { commands, residual });
    }

    pub(crate) fn execute(
        &self,
        trigger: EntityId,
        edge: TriggerEventEdge,
        registry: &mut EntityRegistry,
        slot_table: &mut SlotTable,
    ) -> TriggerBindingExecution {
        let Some(binding) = self.bindings.get(&(trigger, edge)) else {
            return TriggerBindingExecution {
                residual: None,
                #[cfg(test)]
                commands: Vec::new(),
            };
        };
        #[cfg(test)]
        let mut commands = Vec::with_capacity(binding.commands.len());
        for command in &binding.commands {
            command.execute(registry, slot_table, &self.command_diagnostics);
            #[cfg(test)]
            commands.push(command.kind());
        }
        TriggerBindingExecution {
            residual: binding.residual,
            #[cfg(test)]
            commands,
        }
    }

    /// Execute against the live script context. IR commands evaluate through a
    /// fresh script-capability `StoreScope`, while literal writes borrow the
    /// same slot table only for their existing validated batch operation.
    pub(crate) fn execute_with_script_ctx(
        &self,
        trigger: EntityId,
        edge: TriggerEventEdge,
        registry: &mut EntityRegistry,
        script_ctx: &ScriptCtx,
    ) -> TriggerBindingExecution {
        let Some(binding) = self.bindings.get(&(trigger, edge)) else {
            return TriggerBindingExecution {
                residual: None,
                #[cfg(test)]
                commands: Vec::new(),
            };
        };
        #[cfg(test)]
        let mut commands = Vec::with_capacity(binding.commands.len());
        for command in &binding.commands {
            command.execute_with_script_ctx(registry, script_ctx, &self.command_diagnostics);
            #[cfg(test)]
            commands.push(command.kind());
        }
        TriggerBindingExecution {
            residual: binding.residual,
            #[cfg(test)]
            commands,
        }
    }

    pub(crate) fn residual(&self, handle: TriggerResidualHandle) -> Option<&TriggerResidual> {
        self.residuals.get(handle.0)
    }

    /// Whether the authored event name for this trigger edge resolved against
    /// the active composed reaction set at level install.
    #[cfg(feature = "dev-tools")]
    pub(crate) fn is_bound(&self, trigger: EntityId, edge: TriggerEventEdge) -> bool {
        self.bindings.contains_key(&(trigger, edge))
    }

    #[cfg(test)]
    fn binding(&self, trigger: EntityId, edge: TriggerEventEdge) -> Option<&TriggerBinding> {
        self.bindings.get(&(trigger, edge))
    }
}

/// Keep only directly-owned work in the binding. `onComplete` names remain
/// ordered residual hops, so their graphs resolve when the app drains rather
/// than flattening recursively at level install.
fn partition_direct_reaction(
    reaction: &NamedReaction,
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
                if let Some(on_complete) = &primitive.on_complete {
                    warn_for_deferred_event(&reaction.name, on_complete, data_registry);
                }
                steps.push(PrepartitionedReactionStep::Descriptor(
                    ReactionDescriptor::Primitive(primitive.clone()),
                ));
            }
        }
        ReactionDescriptor::Sequence(sequence) => {
            let mut residual_steps = Vec::new();
            for step in sequence {
                if classify(&step.primitive) == PrimitiveClass::Consequential {
                    if let Some(command) = bind_sequence_step(step, slot_table, script_ctx) {
                        commands.push(command);
                    }
                } else {
                    residual_steps.push(step.clone());
                }
            }
            if !residual_steps.is_empty() {
                steps.push(PrepartitionedReactionStep::Descriptor(
                    ReactionDescriptor::Sequence(residual_steps),
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

fn bind_primitive(
    primitive: &PrimitiveDescriptor,
    slot_table: &SlotTable,
    script_ctx: Option<&ScriptCtx>,
) -> Option<BoundTriggerCommand> {
    if primitive.primitive == "setState" && primitive.tag.is_some() {
        log::warn!(
            "[Trigger] setState is system-targeted and cannot carry a target tag; not binding"
        );
        return None;
    }
    let target = primitive
        .tag
        .as_deref()
        .map(|tag| BoundTarget::Tag(tag.to_string()));
    bind_command(
        &primitive.primitive,
        target,
        &primitive.args,
        slot_table,
        script_ctx,
    )
}

fn bind_sequence_step(
    step: &SequenceStep,
    slot_table: &SlotTable,
    script_ctx: Option<&ScriptCtx>,
) -> Option<BoundTriggerCommand> {
    if step.primitive == "setState" {
        log::warn!(
            "[Trigger] setState is system-targeted and cannot carry an entity target; not binding"
        );
        return None;
    }
    let target = Some(BoundTarget::Entity(step.id));
    bind_command(&step.primitive, target, &step.args, slot_table, script_ctx)
}

fn bind_command(
    primitive: &str,
    target: Option<BoundTarget>,
    args: &serde_json::Value,
    slot_table: &SlotTable,
    script_ctx: Option<&ScriptCtx>,
) -> Option<BoundTriggerCommand> {
    let target = |name: &str| {
        target.clone().or_else(|| {
            log::warn!("[Trigger] consequential primitive `{name}` has no target tag; not binding");
            None
        })
    };
    match primitive {
        "moverStart" => Some(BoundTriggerCommand::Mover {
            target: target(primitive)?,
            command: MoverCommand::Start,
        }),
        "moverStop" => Some(BoundTriggerCommand::Mover {
            target: target(primitive)?,
            command: MoverCommand::Stop,
        }),
        "moverReverse" => Some(BoundTriggerCommand::Mover {
            target: target(primitive)?,
            command: MoverCommand::Reverse,
        }),
        "moverGoToPathNode" => {
            let args: MoverGoToPathNodeArgs = match serde_json::from_value(args.clone()) {
                Ok(args) => args,
                Err(error) => {
                    log::warn!(
                        "[Trigger] moverGoToPathNode has invalid args; not binding: {error}"
                    );
                    return None;
                }
            };
            Some(BoundTriggerCommand::Mover {
                target: target(primitive)?,
                command: MoverCommand::GoToPathNode(args.node),
            })
        }
        "applyDamage" => {
            let args: ApplyDamageArgs =
                match serde_json::from_value::<ApplyDamageArgs>(args.clone()) {
                    Ok(args) if args.amount.is_finite() && args.amount >= 0.0 => args,
                    Ok(_) => {
                        log::warn!(
                            "[Trigger] applyDamage amount is negative or non-finite; not binding"
                        );
                        return None;
                    }
                    Err(error) => {
                        log::warn!("[Trigger] applyDamage has invalid args; not binding: {error}");
                        return None;
                    }
                };
            Some(BoundTriggerCommand::Damage {
                target: target(primitive)?,
                amount: args.amount,
            })
        }
        "armTrigger" => Some(BoundTriggerCommand::Arm {
            target: target(primitive)?,
        }),
        "disarmTrigger" => Some(BoundTriggerCommand::Disarm {
            target: target(primitive)?,
        }),
        "setState" => bind_store_slot(args, slot_table, script_ctx),
        "setAnimationState" => {
            let args: SetAnimationStateArgs = match serde_json::from_value(args.clone()) {
                Ok(args) => args,
                Err(error) => {
                    log::warn!(
                        "[Trigger] setAnimationState has invalid args; not binding: {error}"
                    );
                    return None;
                }
            };
            Some(BoundTriggerCommand::AnimationState {
                target: target(primitive)?,
                state: args.state,
            })
        }
        _ => None,
    }
}

fn bind_store_slot(
    args: &serde_json::Value,
    slot_table: &SlotTable,
    script_ctx: Option<&ScriptCtx>,
) -> Option<BoundTriggerCommand> {
    let args: SetStateArgs = match serde_json::from_value(args.clone()) {
        Ok(args) => args,
        Err(error) => {
            log::warn!("[Trigger] setState has invalid args; not binding: {error}");
            return None;
        }
    };
    if crate::scripting::reactions::system_commands::is_ir_node(&args.value) {
        let Some(script_ctx) = script_ctx else {
            log::warn!(
                "[Trigger] runtime setState for `{}` requires the install-time ScriptCtx; not binding",
                args.slot
            );
            return None;
        };
        let root = match ir_node_from_json(args.value.clone(), "setState.value") {
            Ok(root) => root,
            Err(error) => {
                log::warn!(
                    "[Trigger] setState runtime value for `{}` is invalid; not binding: {error}",
                    args.slot
                );
                return None;
            }
        };
        let baked = BakedIr {
            version: CURRENT_IR_VERSION,
            output: Some(args.slot.clone()),
            root,
        };
        let scope = StoreScope::script(script_ctx.clone());
        let program = match bind(&baked, &scope) {
            Ok(program) => program,
            Err(error) => {
                log::warn!(
                    "[Trigger] setState runtime value for `{}` cannot bind; not binding: {error}",
                    args.slot
                );
                return None;
            }
        };
        return Some(BoundTriggerCommand::StoreSlot {
            slot: args.slot,
            value: BoundStoreValue::Ir(program),
        });
    }

    let Some(record) = slot_table.get(&args.slot) else {
        log::warn!(
            "[Trigger] setState references unknown slot `{}`; not binding",
            args.slot
        );
        return None;
    };
    if record.schema.readonly {
        log::warn!(
            "[Trigger] setState rejects readonly slot `{}` at bind time",
            args.slot
        );
        return None;
    }
    let value = match json_value_for_slot(&args.slot, &record.schema.slot_type, &args.value)
        .and_then(|value| validate_slot_value(&args.slot, &record.schema, value))
    {
        Ok(value) => value,
        Err(error) => {
            log::warn!(
                "[Trigger] setState for `{}` is invalid; not binding: {error}",
                args.slot
            );
            return None;
        }
    };
    Some(BoundTriggerCommand::StoreSlot {
        slot: args.slot,
        value: BoundStoreValue::Literal(value),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::{
        NumericRange, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue, Transform,
        TriggerActivation, TriggerFireMode,
    };

    fn primitive(
        name: &str,
        primitive: &str,
        tag: Option<&str>,
        args: serde_json::Value,
        on_complete: Option<&str>,
    ) -> NamedReaction {
        NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: primitive.to_string(),
                tag: tag.map(str::to_string),
                on_complete: on_complete.map(str::to_string),
                args,
            }),
        }
    }

    fn spawn_trigger(registry: &mut EntityRegistry, on_fire: &str) -> EntityId {
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                TriggerVolumeComponent::new(
                    TriggerActivation::Touch,
                    String::new(),
                    on_fire.to_string(),
                    String::new(),
                    MoverCommand::Start,
                    TriggerFireMode::Multiple,
                    0.0,
                    true,
                ),
            )
            .unwrap();
        id
    }

    fn writable_slots() -> SlotTable {
        let mut slots = SlotTable::new();
        slots
            .insert_namespace(
                "trigger",
                vec![(
                    "flag".to_string(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(0.0)),
                        range: Some(NumericRange { min: 0.0, max: 1.0 }),
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: Default::default(),
                        accumulate: None,
                    }),
                )],
            )
            .unwrap();
        slots
    }

    #[test]
    fn runtime_set_state_accumulates_in_trigger_enqueue_order() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "increment");
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert(
                "trigger.count".to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(0.0)),
                    range: Some(NumericRange {
                        min: 0.0,
                        max: 100.0,
                    }),
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: Default::default(),
                    accumulate: None,
                }),
            )
            .unwrap();
        let mut data = DataRegistry::new();
        let increment = |name: &str| {
            primitive(
                name,
                "setState",
                None,
                serde_json::json!({
                    "slot": "trigger.count",
                    "value": {
                        "op": "add",
                        "a": { "op": "input", "name": "trigger.count" },
                        "b": { "op": "const", "value": 1.0 }
                    }
                }),
                None,
            )
        };
        data.populate_level(
            vec![increment("increment"), increment("increment")],
            Vec::new(),
            &[],
        );
        let table = TriggerBindingTable::build_with_script_ctx(&registry, &data, &ctx);
        table.execute_with_script_ctx(trigger, TriggerEventEdge::Enter, &mut registry, &ctx);
        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("trigger.count")
                .and_then(|record| record.value.as_ref()),
            Some(&SlotValue::Number(2.0)),
            "same-tick IR writes must evaluate and commit in trigger command order"
        );
    }

    #[test]
    fn trigger_bind_rejects_crossing_dispatch_input_without_blocking_other_work() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "mixed");
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert_namespace(
                "trigger",
                vec![
                    (
                        "direction".to_string(),
                        SlotRecord::new(SlotSchema {
                            slot_type: SlotType::Number,
                            default: Some(SlotValue::Number(7.0)),
                            range: None,
                            persist: false,
                            readonly: false,
                            ownership: SlotOwnership::Mod,
                            network: Default::default(),
                            accumulate: None,
                        }),
                    ),
                    (
                        "unrelated".to_string(),
                        SlotRecord::new(SlotSchema {
                            slot_type: SlotType::Number,
                            default: Some(SlotValue::Number(0.0)),
                            range: None,
                            persist: false,
                            readonly: false,
                            ownership: SlotOwnership::Mod,
                            network: Default::default(),
                            accumulate: None,
                        }),
                    ),
                ],
            )
            .unwrap();
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![
                primitive(
                    "mixed",
                    "setState",
                    None,
                    serde_json::json!({
                        "slot": "trigger.direction",
                        "value": {
                            "op": "select",
                            "cond": { "op": "input", "name": "@rising" },
                            "a": { "op": "const", "value": 1 },
                            "b": { "op": "const", "value": 0 }
                        }
                    }),
                    None,
                ),
                primitive(
                    "mixed",
                    "setState",
                    None,
                    serde_json::json!({ "slot": "trigger.unrelated", "value": 1 }),
                    None,
                ),
            ],
            Vec::new(),
            &[],
        );

        let table = TriggerBindingTable::build_with_script_ctx(&registry, &data, &ctx);
        let execution =
            table.execute_with_script_ctx(trigger, TriggerEventEdge::Enter, &mut registry, &ctx);

        assert_eq!(execution.commands, vec![BoundTriggerCommandKind::StoreSlot]);
        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("trigger.direction")
                .unwrap()
                .value,
            Some(SlotValue::Number(7.0)),
            "the trigger site has no @rising vocabulary, so its scoped write is rejected"
        );
        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("trigger.unrelated")
                .unwrap()
                .value,
            Some(SlotValue::Number(1.0)),
            "one rejected reaction must not suppress unrelated trigger work"
        );
    }

    #[test]
    #[should_panic(expected = "IR-valued trigger setState must execute with a ScriptCtx")]
    fn execute_without_script_ctx_rejects_ir_set_state() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "increment");
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert(
                "trigger.count".to_string(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(0.0)),
                    range: None,
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: Default::default(),
                    accumulate: None,
                }),
            )
            .unwrap();
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![primitive(
                "increment",
                "setState",
                None,
                serde_json::json!({
                    "slot": "trigger.count",
                    "value": {
                        "op": "add",
                        "a": { "op": "input", "name": "trigger.count" },
                        "b": { "op": "const", "value": 1.0 }
                    }
                }),
                None,
            )],
            Vec::new(),
            &[],
        );
        let table = TriggerBindingTable::build_with_script_ctx(&registry, &data, &ctx);
        let mut slots = writable_slots();

        table.execute(trigger, TriggerEventEdge::Enter, &mut registry, &mut slots);
    }

    #[test]
    fn bind_partitions_direct_consequential_steps_and_retains_presentation() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "open");
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![
                primitive(
                    "open",
                    "moverStart",
                    Some("door"),
                    serde_json::json!({}),
                    None,
                ),
                primitive(
                    "open",
                    "setState",
                    None,
                    serde_json::json!({ "slot": "trigger.flag", "value": 1 }),
                    None,
                ),
                primitive(
                    "open",
                    "flashScreen",
                    None,
                    serde_json::json!({ "color": [1, 0, 0], "durationMs": 20 }),
                    None,
                ),
            ],
            Vec::new(),
            &[],
        );

        let table = TriggerBindingTable::build(&registry, &data, &writable_slots());
        let binding = table
            .binding(trigger, TriggerEventEdge::Enter)
            .expect("named trigger event binds");
        assert_eq!(binding.commands.len(), 2);
        assert!(matches!(
            binding.commands[0],
            BoundTriggerCommand::Mover {
                command: MoverCommand::Start,
                ..
            }
        ));
        assert!(matches!(
            binding.commands[1],
            BoundTriggerCommand::StoreSlot { ref slot, .. } if slot == "trigger.flag"
        ));
        let residual = table
            .residual(binding.residual.expect("presentation is residual"))
            .unwrap();
        assert!(matches!(
            residual.steps(),
            [PrepartitionedReactionStep::Descriptor(ReactionDescriptor::Primitive(
                PrimitiveDescriptor { primitive, .. }
            ))] if primitive == "flashScreen"
        ));
    }

    #[test]
    fn bind_defers_consequential_on_complete_as_later_residual_hop() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "open");
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![
                primitive(
                    "open",
                    "moverStart",
                    Some("door"),
                    serde_json::json!({}),
                    Some("after_open"),
                ),
                primitive(
                    "after_open",
                    "applyDamage",
                    Some("enemy"),
                    serde_json::json!({ "amount": 5 }),
                    None,
                ),
            ],
            Vec::new(),
            &[],
        );

        let table = TriggerBindingTable::build(&registry, &data, &writable_slots());
        let binding = table
            .binding(trigger, TriggerEventEdge::Enter)
            .expect("named trigger event binds");
        assert_eq!(binding.commands.len(), 1);
        let residual = table
            .residual(binding.residual.expect("chain is residual"))
            .unwrap();
        assert!(matches!(
            residual.steps(),
            [PrepartitionedReactionStep::DeferredEvent(event_name)] if event_name == "after_open"
        ));
    }

    #[test]
    fn bind_rejects_readonly_set_state_at_install() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "set_readonly");
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![primitive(
                "set_readonly",
                "setState",
                None,
                serde_json::json!({ "slot": "player.health", "value": 1 }),
                None,
            )],
            Vec::new(),
            &[],
        );

        let table = TriggerBindingTable::build(&registry, &data, &SlotTable::new());
        let binding = table
            .binding(trigger, TriggerEventEdge::Enter)
            .expect("the known event is recorded even when it has no executable work");
        assert!(binding.commands.is_empty());
        assert!(binding.residual.is_none());
    }

    // Regression: tagged setState bypassed the normal system-only dispatch contract.
    #[test]
    fn bind_rejects_tagged_set_state_without_an_in_tick_write() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "set_tagged");
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![primitive(
                "set_tagged",
                "setState",
                Some("not-a-system-target"),
                serde_json::json!({ "slot": "trigger.flag", "value": 1 }),
                None,
            )],
            Vec::new(),
            &[],
        );
        let mut slots = writable_slots();

        let table = TriggerBindingTable::build(&registry, &data, &slots);
        let binding = table
            .binding(trigger, TriggerEventEdge::Enter)
            .expect("the known event is recorded even when its invalid step is rejected");
        assert!(binding.commands.is_empty());
        assert!(binding.residual.is_none());

        assert!(
            table
                .execute(trigger, TriggerEventEdge::Enter, &mut registry, &mut slots,)
                .residual()
                .is_none(),
            "a rejected tagged setState must not leave residual work"
        );
        assert_eq!(
            slots
                .get("trigger.flag")
                .and_then(|record| record.value.as_ref()),
            Some(&SlotValue::Number(0.0)),
            "tagged setState must retain the normal entity-targeted no-op contract"
        );
    }

    // Regression: sequence setState silently discarded its entity target and
    // performed a system write despite the malformed authoring shape.
    #[test]
    fn bind_rejects_entity_targeted_sequence_set_state_without_an_in_tick_write() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "set_sequence_tagged");
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![NamedReaction {
                name: "set_sequence_tagged".to_string(),
                descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
                    id: trigger,
                    primitive: "setState".to_string(),
                    args: serde_json::json!({ "slot": "trigger.flag", "value": 1 }),
                }]),
            }],
            Vec::new(),
            &[],
        );
        let mut slots = writable_slots();

        let table = TriggerBindingTable::build(&registry, &data, &slots);
        let binding = table
            .binding(trigger, TriggerEventEdge::Enter)
            .expect("the known event is recorded even when its invalid step is rejected");
        assert!(binding.commands.is_empty());
        assert!(binding.residual.is_none());

        assert!(
            table
                .execute(trigger, TriggerEventEdge::Enter, &mut registry, &mut slots,)
                .residual()
                .is_none(),
            "a rejected sequence setState must not leave residual work"
        );
        assert_eq!(
            slots
                .get("trigger.flag")
                .and_then(|record| record.value.as_ref()),
            Some(&SlotValue::Number(0.0)),
            "entity-targeted sequence setState must not perform an in-tick write"
        );
    }

    // Regression: the binding retained a Progress descriptor in its residual, which
    // fired the progress target on the app drain with zero kills — and then again
    // when ProgressTracker hit the real threshold.
    #[test]
    fn bind_leaves_no_residual_for_a_progress_reaction() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "wave_started");
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![
                NamedReaction {
                    name: "wave_started".to_string(),
                    descriptor: ReactionDescriptor::Progress(ProgressDescriptor {
                        tag: "wave1".to_string(),
                        at: 1.0,
                        fire: "open_vault".to_string(),
                    }),
                },
                primitive(
                    "open_vault",
                    "moverStart",
                    Some("vault"),
                    serde_json::json!({}),
                    None,
                ),
            ],
            Vec::new(),
            &[],
        );

        let table = TriggerBindingTable::build(&registry, &data, &writable_slots());
        let binding = table
            .binding(trigger, TriggerEventEdge::Enter)
            .expect("the known event is recorded even when it has no executable work");
        assert!(
            binding.commands.is_empty(),
            "a Progress reaction owns no in-tick trigger work"
        );
        assert!(
            binding.residual.is_none(),
            "ProgressTracker owns the progress target; the residual must not fire it too"
        );
    }
}
