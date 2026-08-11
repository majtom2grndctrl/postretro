//! Bind trigger event reactions at level install and execute their fixed-tick work.
//! See: context/lib/entity_model.md §5 · context/lib/scripting.md §12

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt;

use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, MoverCommand, ScriptCtx, SlotTable,
    TriggerVolumeComponent,
};
use postretro_foundation::{BakedIr, CURRENT_IR_VERSION, ir_node_from_json};
use postretro_scripting_core::data_descriptors::{
    NamedReaction, PrimitiveDescriptor, ProgressDescriptor, ReactionDescriptor, SequenceStep,
    SequenceTarget,
};
use postretro_scripting_core::data_registry::DataRegistry;
use postretro_scripting_core::ir::bind;
use postretro_scripting_core::ir_scopes::DispatchScope;
use postretro_scripting_core::reaction_dispatch::PrepartitionedReactionStep;
use postretro_scripting_core::store_bridge::{json_value_for_slot, validate_slot_value};
use serde::Deserialize;

use crate::grant::{GrantAmmoArgs, GrantHealthArgs};
use crate::health::reactions::ApplyDamageArgs;
use crate::kinematic_mover::{MoverCommandDiagnostics, MoverSetSpinRateArgs};
use crate::scripting::reactions::animation::SetAnimationStateArgs;
use crate::scripting::reactions::enemy_state::UpdateEnemyStateArgs;
#[cfg(test)]
pub(crate) use crate::trigger_commands::BoundTriggerCommandKind;
use crate::trigger_commands::{
    BoundStoreValue, BoundTarget, BoundTriggerCommand, TriggerFireContext,
};
use crate::trigger_system::TriggerEventEdge;

const TRIGGER_EVENT_INPUTS: &[(&str, postretro_foundation::IrType)] =
    &[("@occupancy", postretro_foundation::IrType::Number)];

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

#[derive(Default)]
pub(crate) struct TriggerBindingTable {
    bindings: HashMap<(EntityId, TriggerEventEdge), TriggerBinding>,
    /// The fixed set of script-owned edges is built at level install and
    /// reused by every tick. Rebuilding this from `bindings` in the tick loop
    /// needlessly allocated a hash set each frame.
    bound_edges: HashSet<(EntityId, TriggerEventEdge)>,
    /// Trigger-event programs bind once and share this sequential dispatch
    /// scope. Fixed-tick fires never overlap, so reseeding it per evaluation
    /// preserves isolation without allocating a boxed input array per command.
    dispatch_scope: Option<RefCell<DispatchScope>>,
    residuals: Vec<TriggerResidual>,
    command_diagnostics: MoverCommandDiagnostics,
    spawn_context: crate::spawner::SpawnContext,
}

impl fmt::Debug for TriggerBindingTable {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TriggerBindingTable")
            .field("bindings", &self.bindings)
            .field("bound_edges", &self.bound_edges)
            .field(
                "dispatch_scope",
                &self.dispatch_scope.as_ref().map(|_| "install-owned"),
            )
            .field("residuals", &self.residuals)
            .field("command_diagnostics", &self.command_diagnostics)
            .field("spawn_context", &self.spawn_context)
            .finish()
    }
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
#[serde(rename_all = "camelCase")]
struct AddSlotArgs {
    slot: String,
    delta: f32,
}

#[derive(Debug, Deserialize)]
struct MoverGoToPathNodeArgs {
    node: String,
}

impl TriggerBindingTable {
    /// Construct brush-authored bindings after reaction composition. Empty brush
    /// event names add no binding here; manifest trigger events may still bind
    /// the edge. Unknown names warn once per trigger edge and do not fall back
    /// to a later drain-time lookup.
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
        spawn_context: crate::spawner::SpawnContext,
    ) -> Self {
        let slot_table = script_ctx.slot_table.borrow();
        let mut table = Self::build_inner(registry, data_registry, &slot_table, Some(script_ctx));
        table.command_diagnostics = command_diagnostics;
        table.spawn_context = spawn_context;
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
        table.dispatch_scope = script_ctx.map(|script_ctx| {
            RefCell::new(DispatchScope::script(
                script_ctx.clone(),
                TRIGGER_EVENT_INPUTS,
            ))
        });
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

        if commands.is_empty() && steps.is_empty() {
            return;
        }
        self.append_binding(trigger, edge, commands, steps);
    }

    fn append_binding(
        &mut self,
        trigger: EntityId,
        edge: TriggerEventEdge,
        commands: Vec<BoundTriggerCommand>,
        steps: Vec<PrepartitionedReactionStep>,
    ) {
        self.bound_edges.insert((trigger, edge));
        if let Some(binding) = self.bindings.get_mut(&(trigger, edge)) {
            binding.commands.extend(commands);
            if !steps.is_empty() {
                if let Some(handle) = binding.residual {
                    self.residuals[handle.0].steps.extend(steps);
                } else {
                    let handle = TriggerResidualHandle(self.residuals.len());
                    self.residuals.push(TriggerResidual { steps });
                    binding.residual = Some(handle);
                }
            }
            return;
        }
        let residual = (!steps.is_empty()).then(|| {
            let handle = TriggerResidualHandle(self.residuals.len());
            self.residuals.push(TriggerResidual { steps });
            handle
        });
        self.bindings
            .insert((trigger, edge), TriggerBinding { commands, residual });
    }

    /// Bind manifest-declared trigger events (tag + edge → fired reaction names)
    /// after brush-authored bindings are built. Manifest events append to any
    /// existing binding for the same trigger edge — brush KVP bindings always
    /// run first, manifest events after.
    pub(crate) fn install_manifest_events(
        &mut self,
        registry: &EntityRegistry,
        data_registry: &DataRegistry,
        script_ctx: &ScriptCtx,
    ) {
        let descriptors = data_registry.trigger_events.clone();
        let slots = script_ctx.slot_table.borrow();
        for descriptor in descriptors {
            let edge = match descriptor.event.as_str() {
                "enter" => TriggerEventEdge::Enter,
                "exit" => TriggerEventEdge::Exit,
                other => {
                    log::warn!(
                        "[Trigger] unknown trigger-event `{other}` on tag `{}`; descriptor is inert",
                        descriptor.tag
                    );
                    continue;
                }
            };
            let mut triggers: Vec<_> = registry
                .query_by_component_and_tag(ComponentKind::TriggerVolume, Some(&descriptor.tag))
                .map(|(id, _)| id)
                .collect();
            triggers.sort_unstable();
            if triggers.is_empty() {
                log::warn!(
                    "[Trigger] trigger-event tag `{}` matched no trigger volumes; descriptor is inert",
                    descriptor.tag
                );
                continue;
            }
            for event_name in descriptor.fire {
                for &trigger in &triggers {
                    self.bind_event(
                        trigger,
                        edge,
                        &event_name,
                        data_registry,
                        &slots,
                        Some(script_ctx),
                    );
                }
            }
        }
    }

    pub(crate) fn bound_edges(&self) -> &HashSet<(EntityId, TriggerEventEdge)> {
        &self.bound_edges
    }

    pub(crate) fn execute(
        &self,
        trigger: EntityId,
        edge: TriggerEventEdge,
        registry: &mut EntityRegistry,
        slot_table: &mut SlotTable,
        fire_context: &TriggerFireContext,
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
            command.execute(
                registry,
                slot_table,
                &self.command_diagnostics,
                &self.spawn_context,
                fire_context,
            );
            #[cfg(test)]
            commands.push(command.kind());
        }
        TriggerBindingExecution {
            residual: binding.residual,
            #[cfg(test)]
            commands,
        }
    }

    /// Execute against the live script context. IR commands reuse the
    /// install-owned script-capability `DispatchScope`, reseeded for each
    /// command; literal writes retain their validated batch operation.
    pub(crate) fn execute_with_script_ctx(
        &self,
        trigger: EntityId,
        edge: TriggerEventEdge,
        registry: &mut EntityRegistry,
        script_ctx: &ScriptCtx,
        fire_context: &TriggerFireContext,
    ) -> TriggerBindingExecution {
        let Some(binding) = self.bindings.get(&(trigger, edge)) else {
            return TriggerBindingExecution {
                residual: None,
                #[cfg(test)]
                commands: Vec::new(),
            };
        };
        let Some(dispatch_scope) = self.dispatch_scope.as_ref() else {
            log::warn!(
                "[Trigger] live script dispatch was requested without an install-owned dispatch scope"
            );
            return TriggerBindingExecution {
                residual: binding.residual,
                #[cfg(test)]
                commands: Vec::new(),
            };
        };
        let mut dispatch_scope = dispatch_scope.borrow_mut();
        #[cfg(test)]
        let mut commands = Vec::with_capacity(binding.commands.len());
        for command in &binding.commands {
            command.execute_with_script_ctx(
                registry,
                script_ctx,
                &mut dispatch_scope,
                &self.command_diagnostics,
                &self.spawn_context,
                fire_context,
            );
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

    /// Whether this trigger edge has an active binding from its brush event or
    /// a composed manifest trigger event.
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
    if primitive.primitive == "setState" && (primitive.tag.is_some() || primitive.target.is_some())
    {
        log::warn!(
            "[Trigger] setState is system-targeted and cannot carry a target tag or sentinel; not binding"
        );
        return None;
    }
    let target = if let Some(sentinel) = primitive.target.as_deref() {
        match sentinel {
            "@activators" => Some(BoundTarget::Activators),
            spelling => {
                log::warn!("[Trigger] illegal primitive target sentinel `{spelling}`; not binding");
                return None;
            }
        }
    } else {
        primitive
            .tag
            .as_deref()
            .map(|tag| BoundTarget::Tag(tag.to_string()))
    };
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
    let target = Some(match step.id {
        SequenceTarget::Entity(id) => BoundTarget::Entity(id),
        SequenceTarget::Activators => BoundTarget::Activators,
        SequenceTarget::FiredTrigger => BoundTarget::FiredTrigger,
    });
    bind_command(&step.primitive, target, &step.args, slot_table, script_ctx)
}

fn bind_command(
    primitive: &str,
    target: Option<BoundTarget>,
    args: &serde_json::Value,
    slot_table: &SlotTable,
    script_ctx: Option<&ScriptCtx>,
) -> Option<BoundTriggerCommand> {
    let target_from_context = target;
    let target = |name: &str| {
        target_from_context.clone().or_else(|| {
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
        "moverSetSpinRate" => {
            let args: MoverSetSpinRateArgs = match serde_json::from_value(args.clone()) {
                Ok(args) => args,
                Err(error) => {
                    log::warn!("[Trigger] moverSetSpinRate has invalid args; not binding: {error}");
                    return None;
                }
            };
            Some(BoundTriggerCommand::Mover {
                target: target(primitive)?,
                command: MoverCommand::SetSpinRate(args.rate),
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
        "grantHealth" => {
            let args: GrantHealthArgs =
                match serde_json::from_value::<GrantHealthArgs>(args.clone()) {
                    Ok(args) if args.amount.is_finite() => args,
                    Ok(_) => {
                        log::warn!("[Trigger] grantHealth amount is non-finite; not binding");
                        return None;
                    }
                    Err(error) => {
                        log::warn!("[Trigger] grantHealth has invalid args; not binding: {error}");
                        return None;
                    }
                };
            Some(BoundTriggerCommand::GrantHealth {
                target: target(primitive)?,
                amount: args.amount,
            })
        }
        "grantAmmo" => {
            let args: GrantAmmoArgs = match serde_json::from_value::<GrantAmmoArgs>(args.clone()) {
                Ok(args) if args.amount.is_finite() => args,
                Ok(_) => {
                    log::warn!("[Trigger] grantAmmo amount is non-finite; not binding");
                    return None;
                }
                Err(error) => {
                    log::warn!("[Trigger] grantAmmo has invalid args; not binding: {error}");
                    return None;
                }
            };
            Some(BoundTriggerCommand::GrantAmmo {
                target: target(primitive)?,
                ammo_type: args.ammo_type,
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
        "addSlot" => {
            let args: AddSlotArgs = match serde_json::from_value(args.clone()) {
                Ok(args) if args.delta.is_finite() => args,
                Ok(_) => {
                    log::warn!("[Trigger] addSlot delta must be finite; not binding");
                    return None;
                }
                Err(error) => {
                    log::warn!("[Trigger] addSlot has invalid args; not binding: {error}");
                    return None;
                }
            };
            let Some(record) = slot_table.get(&args.slot) else {
                log::warn!(
                    "[Trigger] addSlot references unknown slot `{}`; not binding",
                    args.slot
                );
                return None;
            };
            if !record.schema.per_owner {
                log::warn!(
                    "[Trigger] addSlot requires per-owner slot `{}`; not binding",
                    args.slot
                );
                return None;
            }
            if record.schema.slot_type != postretro_entities::SlotType::Number {
                log::warn!(
                    "[Trigger] addSlot requires numeric slot `{}`; not binding",
                    args.slot
                );
                return None;
            }
            if record.schema.readonly {
                log::warn!(
                    "[Trigger] addSlot rejects readonly slot `{}` at bind time",
                    args.slot
                );
                return None;
            }
            Some(BoundTriggerCommand::AddOwnerSlot {
                target: target(primitive)?,
                slot: args.slot,
                delta: args.delta,
            })
        }
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
        "updateEnemyState" => {
            let Some(target) = target_from_context else {
                log::warn!(
                    "[Trigger] updateEnemyState requires a fire-time tag target; not binding"
                );
                return None;
            };
            let args: UpdateEnemyStateArgs = match serde_json::from_value(args.clone()) {
                Ok(args) => args,
                Err(error) => {
                    log::warn!("[Trigger] updateEnemyState has invalid args; not binding: {error}");
                    return None;
                }
            };
            Some(BoundTriggerCommand::UpdateEnemyState {
                target,
                aggro: args.aggro,
            })
        }
        "spawnFromSpawner" => {
            let Some(BoundTarget::Tag(tag)) = target_from_context.as_ref() else {
                log::warn!(
                    "[Trigger] spawnFromSpawner requires a fire-time tag target; not binding"
                );
                return None;
            };
            if tag.is_empty() {
                log::warn!(
                    "[Trigger] spawnFromSpawner requires a non-empty fire-time tag target; not binding"
                );
                return None;
            }
            Some(BoundTriggerCommand::Spawn {
                target: target_from_context.expect("tag target checked above"),
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
        let scope = DispatchScope::script(script_ctx.clone(), TRIGGER_EVENT_INPUTS);
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
    use postretro_entities::components::ammo_reserve::AmmoReserve;
    use postretro_entities::components::brain::{BrainComponent, attach_brain_graph};
    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::{
        NumericRange, ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
        Transform, TriggerActivation, TriggerFireMode,
    };
    use postretro_foundation::Seat;

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
                target: None,
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

    fn spawn_brain(registry: &mut EntityRegistry, tag: &str) -> EntityId {
        let entity = registry.spawn(Transform::default());
        registry.set_tags(entity, vec![tag.into()]).unwrap();
        let graph = postretro_foundation::BehaviorGraphDescriptor {
            initial: "idle".to_string(),
            states: std::collections::BTreeMap::from([(
                "idle".to_string(),
                postretro_foundation::BehaviorStateDescriptor {
                    animation: "idle".to_string(),
                    motion: postretro_foundation::MotionVerb::Hold,
                    action: None,
                    transitions: Vec::new(),
                    on_enter: None,
                },
            )]),
            interrupts: Vec::new(),
            candidate_filter: None,
            attack: None,
            engagement_radius: None,
            move_speed: 3.5,
        };
        attach_brain_graph(registry, entity, &graph).unwrap();
        entity
    }

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

    #[test]
    fn add_slot_trigger_command_applies_to_activator_seat_and_zero_activators_is_silent() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        registry.bind_pawn_seat(pawn, Seat(6));
        let mut slots = SlotTable::new();
        slots
            .insert("currency.xp".to_string(), per_owner_number_slot(10.0))
            .unwrap();
        let command = bind_command(
            "addSlot",
            Some(BoundTarget::Activators),
            &serde_json::json!({ "slot": "currency.xp", "delta": 2.0 }),
            &slots,
            None,
        )
        .expect("owner-slot add binds");
        assert_eq!(command.kind(), BoundTriggerCommandKind::AddOwnerSlot);

        command.execute(
            &mut registry,
            &mut slots,
            &MoverCommandDiagnostics::default(),
            &crate::spawner::SpawnContext::default(),
            &TriggerFireContext {
                activator: Some(pawn),
                ..Default::default()
            },
        );
        assert_eq!(
            slots
                .get("currency.xp")
                .and_then(|record| record.per_seat_value(Seat(6))),
            Some(&SlotValue::Number(12.0)),
        );

        command.execute(
            &mut registry,
            &mut slots,
            &MoverCommandDiagnostics::default(),
            &crate::spawner::SpawnContext::default(),
            &TriggerFireContext::default(),
        );
        assert_eq!(
            slots
                .get("currency.xp")
                .and_then(|record| record.per_seat_value(Seat(6))),
            Some(&SlotValue::Number(12.0)),
            "zero activators leave the slot untouched without needing a warning path",
        );
    }

    #[test]
    fn update_enemy_state_rejects_tagless_and_unknown_key_bindings() {
        assert!(
            bind_command(
                "updateEnemyState",
                None,
                &serde_json::json!({ "aggro": true }),
                &SlotTable::new(),
                None,
            )
            .is_none()
        );
        assert!(
            bind_command(
                "updateEnemyState",
                Some(BoundTarget::Tag("closet".into())),
                &serde_json::json!({ "unknown": true }),
                &SlotTable::new(),
                None,
            )
            .is_none()
        );
    }

    #[test]
    fn spawn_from_spawner_requires_a_tag_target_at_binding() {
        let args = serde_json::json!({});
        let slots = SlotTable::new();

        let command = bind_command(
            "spawnFromSpawner",
            Some(BoundTarget::Tag("closet".into())),
            &args,
            &slots,
            None,
        )
        .expect("tag-targeted spawner command binds");
        assert_eq!(command.kind(), BoundTriggerCommandKind::Spawn);

        for target in [
            None,
            Some(BoundTarget::Activators),
            Some(BoundTarget::FiredTrigger),
        ] {
            assert!(
                bind_command("spawnFromSpawner", target, &args, &slots, None).is_none(),
                "spawnFromSpawner must reject an absent or special target"
            );
        }
    }

    #[test]
    fn update_enemy_state_resolves_later_added_brains_at_fire_time() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "release");
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![primitive(
                "release",
                "updateEnemyState",
                Some("closet"),
                serde_json::json!({ "aggro": false }),
                None,
            )],
            Vec::new(),
            &[],
        );
        let table = TriggerBindingTable::build(&registry, &data, &SlotTable::new());
        let enemy = spawn_brain(&mut registry, "closet");

        let execution = table.execute(
            trigger,
            TriggerEventEdge::Enter,
            &mut registry,
            &mut SlotTable::new(),
            &TriggerFireContext::default(),
        );

        assert_eq!(
            execution.commands,
            vec![BoundTriggerCommandKind::UpdateEnemyState]
        );
        assert!(
            !registry
                .get_component::<BrainComponent>(enemy)
                .unwrap()
                .aggro_armed
        );
    }

    #[test]
    fn update_enemy_state_empty_tag_is_debug_noop_and_keeps_fanout_work() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "fanout");
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![
                primitive(
                    "fanout",
                    "updateEnemyState",
                    Some("unspawned"),
                    serde_json::json!({ "aggro": true }),
                    None,
                ),
                primitive(
                    "fanout",
                    "setState",
                    None,
                    serde_json::json!({ "slot": "trigger.flag", "value": 1.0 }),
                    None,
                ),
            ],
            Vec::new(),
            &[],
        );
        let table = TriggerBindingTable::build(&registry, &data, &writable_slots());
        let mut slots = writable_slots();
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            let execution = table.execute(
                trigger,
                TriggerEventEdge::Enter,
                &mut registry,
                &mut slots,
                &TriggerFireContext::default(),
            );
            assert_eq!(
                execution.commands,
                vec![
                    BoundTriggerCommandKind::UpdateEnemyState,
                    BoundTriggerCommandKind::StoreSlot,
                ]
            );
        });
        assert_eq!(
            slots.get("trigger.flag").unwrap().value,
            Some(SlotValue::Number(1.0)),
        );
        assert!(captured.iter().any(|(level, message)| {
            *level == log::Level::Debug && message.contains("empty Brain tag match")
        }));
    }

    #[test]
    fn update_enemy_state_special_target_logs_and_skips() {
        let command = bind_command(
            "updateEnemyState",
            Some(BoundTarget::Activators),
            &serde_json::json!({ "aggro": false }),
            &SlotTable::new(),
            None,
        )
        .expect("special target remains bound so the fixed-tick executor can reject it");
        let mut registry = EntityRegistry::new();
        let enemy = spawn_brain(&mut registry, "closet");
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            command.execute(
                &mut registry,
                &mut SlotTable::new(),
                &Default::default(),
                &Default::default(),
                &TriggerFireContext {
                    activator: Some(enemy),
                    ..Default::default()
                },
            );
        });
        assert!(
            registry
                .get_component::<BrainComponent>(enemy)
                .unwrap()
                .aggro_armed
        );
        assert!(captured.iter().any(|(level, message)| {
            *level == log::Level::Warn && message.contains("requires a tag target")
        }));
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
                        per_owner: false,
                        accumulate: None,
                    }),
                )],
            )
            .unwrap();
        slots
    }

    #[test]
    fn manifest_trigger_events_append_after_brush_bindings() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "brush");
        registry.set_tags(trigger, vec!["plate".into()]).unwrap();
        let ctx = ScriptCtx::new();
        let mut data = DataRegistry::new();
        data.populate_level_with_trigger_events(
            vec![
                primitive(
                    "brush",
                    "setState",
                    None,
                    serde_json::json!({"slot":"trigger.flag","value":0.0}),
                    None,
                ),
                primitive(
                    "script",
                    "setState",
                    None,
                    serde_json::json!({"slot":"trigger.flag","value":1.0}),
                    None,
                ),
            ],
            Vec::new(),
            vec![
                postretro_scripting_core::data_descriptors::TriggerEventDescriptor {
                    tag: "plate".into(),
                    event: "enter".into(),
                    fire: vec!["script".into()],
                    levels: Vec::new(),
                },
            ],
            Vec::new(),
            &[],
        );
        ctx.slot_table
            .borrow_mut()
            .insert_namespace(
                "trigger",
                vec![(
                    "flag".into(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(0.0)),
                        range: Some(NumericRange { min: 0.0, max: 1.0 }),
                        persist: false,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: Default::default(),
                        per_owner: false,
                        accumulate: None,
                    }),
                )],
            )
            .unwrap();
        let mut table = TriggerBindingTable::build_with_script_ctx(&registry, &data, &ctx);
        table.install_manifest_events(&registry, &data, &ctx);

        let execution = table.execute_with_script_ctx(
            trigger,
            TriggerEventEdge::Enter,
            &mut registry,
            &ctx,
            &TriggerFireContext::default(),
        );
        assert_eq!(execution.commands.len(), 2);
        assert_eq!(
            ctx.slot_table
                .borrow()
                .get("trigger.flag")
                .and_then(|r| r.value.as_ref()),
            Some(&SlotValue::Number(1.0))
        );
        assert!(
            table
                .bound_edges()
                .contains(&(trigger, TriggerEventEdge::Enter))
        );
    }

    #[test]
    fn manifest_trigger_event_with_zero_tag_matches_is_inert() {
        let mut registry = EntityRegistry::new();
        let unrelated = spawn_trigger(&mut registry, "");
        registry
            .set_tags(unrelated, vec!["other-plate".into()])
            .unwrap();
        let ctx = ScriptCtx::new();
        let mut data = DataRegistry::new();
        data.populate_level_with_trigger_events(
            vec![primitive(
                "never",
                "setState",
                None,
                serde_json::json!({"slot":"trigger.flag","value":1.0}),
                None,
            )],
            Vec::new(),
            vec![
                postretro_scripting_core::data_descriptors::TriggerEventDescriptor {
                    tag: "missing-plate".into(),
                    event: "enter".into(),
                    fire: vec!["never".into()],
                    levels: Vec::new(),
                },
            ],
            Vec::new(),
            &[],
        );
        *ctx.slot_table.borrow_mut() = writable_slots();
        let mut table = TriggerBindingTable::build_with_script_ctx(&registry, &data, &ctx);
        table.install_manifest_events(&registry, &data, &ctx);

        assert!(table.bound_edges().is_empty());
        let execution = table.execute_with_script_ctx(
            unrelated,
            TriggerEventEdge::Enter,
            &mut registry,
            &ctx,
            &TriggerFireContext::default(),
        );
        assert!(execution.commands.is_empty());
        assert!(execution.residual().is_none());
        assert_eq!(
            ctx.slot_table.borrow().get("trigger.flag").unwrap().value,
            Some(SlotValue::Number(0.0)),
        );
    }

    #[test]
    fn presentation_sequence_step_with_sentinel_target_rejects_without_residual() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "bad-presentation");
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![NamedReaction {
                name: "bad-presentation".into(),
                descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
                    id: SequenceTarget::Activators,
                    primitive: "flashScreen".into(),
                    args: serde_json::json!({"color":[1,0,0],"durationMs":20}),
                }]),
            }],
            Vec::new(),
            &[],
        );
        let table = TriggerBindingTable::build(&registry, &data, &SlotTable::new());
        assert!(
            table.binding(trigger, TriggerEventEdge::Enter).is_none(),
            "a presentation-only sentinel step must leave the edge inert"
        );
        assert!(
            !table
                .bound_edges()
                .contains(&(trigger, TriggerEventEdge::Enter)),
            "a rejected presentation-only binding must not turn a nameless edge into a script-owned edge"
        );
        let execution = table.execute(
            trigger,
            TriggerEventEdge::Enter,
            &mut registry,
            &mut SlotTable::new(),
            &TriggerFireContext {
                fired_trigger: Some(trigger),
                activator: Some(trigger),
                occupancy: 1,
            },
        );
        assert!(execution.commands.is_empty());
        assert!(execution.residual().is_none());
    }

    fn health(registry: &mut EntityRegistry) -> EntityId {
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                HealthComponent {
                    max: 100.0,
                    current: 100.0,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: Default::default(),
                    contributor_ledger: Default::default(),
                },
            )
            .unwrap();
        id
    }

    fn ammo_reserve(registry: &mut EntityRegistry) -> EntityId {
        let id = registry.spawn(Transform::default());
        registry.set_component(id, AmmoReserve::new()).unwrap();
        id
    }

    #[test]
    fn grant_commands_bind_negative_amounts_for_the_chokepoint_to_reject() {
        let slots = SlotTable::new();
        let health = bind_command(
            "grantHealth",
            Some(BoundTarget::Activators),
            &serde_json::json!({ "amount": -1.0 }),
            &slots,
            None,
        )
        .expect("negative health grant must reach the chokepoint");
        assert_eq!(health.kind(), BoundTriggerCommandKind::GrantHealth);

        let ammo = bind_command(
            "grantAmmo",
            Some(BoundTarget::Activators),
            &serde_json::json!({ "type": "bullets.light", "amount": -1.0 }),
            &slots,
            None,
        )
        .expect("negative ammo grant must reach the chokepoint");
        assert_eq!(ammo.kind(), BoundTriggerCommandKind::GrantAmmo);
    }

    #[test]
    fn activator_grant_commands_mutate_only_the_current_activator() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "grant");
        let health_recipient = health(&mut registry);
        let mut health_component = registry
            .get_component::<HealthComponent>(health_recipient)
            .unwrap()
            .clone();
        health_component.current = 60.0;
        registry
            .set_component(health_recipient, health_component)
            .unwrap();
        let ammo_recipient = ammo_reserve(&mut registry);
        let bystander = health(&mut registry);
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![
                NamedReaction {
                    name: "grant".into(),
                    descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                        primitive: "grantHealth".into(),
                        target: Some("@activators".into()),
                        tag: None,
                        on_complete: None,
                        args: serde_json::json!({ "amount": 25.0 }),
                    }),
                },
                NamedReaction {
                    name: "grant".into(),
                    descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                        primitive: "grantAmmo".into(),
                        target: Some("@activators".into()),
                        tag: None,
                        on_complete: None,
                        args: serde_json::json!({ "type": "bullets.light", "amount": 8.0 }),
                    }),
                },
            ],
            Vec::new(),
            &[],
        );
        let table = TriggerBindingTable::build(&registry, &data, &SlotTable::new());

        let health_execution = table.execute(
            trigger,
            TriggerEventEdge::Enter,
            &mut registry,
            &mut SlotTable::new(),
            &TriggerFireContext {
                activator: Some(health_recipient),
                ..Default::default()
            },
        );
        assert_eq!(
            health_execution.commands,
            vec![
                BoundTriggerCommandKind::GrantHealth,
                BoundTriggerCommandKind::GrantAmmo,
            ]
        );
        assert_eq!(
            registry
                .get_component::<HealthComponent>(health_recipient)
                .unwrap()
                .current,
            85.0,
            "the first activator receives the health grant"
        );
        assert_eq!(
            registry
                .get_component::<HealthComponent>(bystander)
                .unwrap()
                .current,
            100.0
        );

        table.execute(
            trigger,
            TriggerEventEdge::Enter,
            &mut registry,
            &mut SlotTable::new(),
            &TriggerFireContext {
                activator: Some(ammo_recipient),
                ..Default::default()
            },
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(ammo_recipient)
                .unwrap()
                .available("bullets.light"),
            8
        );
    }

    #[test]
    fn tag_grant_skips_missing_components_without_aborting_other_targets() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "grant");
        let bare = registry.spawn(Transform::default());
        let recipient = ammo_reserve(&mut registry);
        registry.set_tags(bare, vec!["pickup".into()]).unwrap();
        registry.set_tags(recipient, vec!["pickup".into()]).unwrap();
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![primitive(
                "grant",
                "grantAmmo",
                Some("pickup"),
                serde_json::json!({ "type": "bullets.light", "amount": 6.0 }),
                None,
            )],
            Vec::new(),
            &[],
        );
        let table = TriggerBindingTable::build(&registry, &data, &SlotTable::new());
        let captured = crate::scripting::reactions::log_capture::capture(|| {
            table.execute(
                trigger,
                TriggerEventEdge::Enter,
                &mut registry,
                &mut SlotTable::new(),
                &TriggerFireContext::default(),
            );
        });

        assert_eq!(
            registry
                .get_component::<AmmoReserve>(recipient)
                .unwrap()
                .available("bullets.light"),
            6
        );
        let warnings: Vec<_> = captured
            .iter()
            .filter(|(level, _)| *level == log::Level::Warn)
            .map(|(_, message)| message.clone())
            .collect();
        assert_eq!(
            warnings,
            vec![format!(
                "[Grant] grantAmmo: entity {bare} has no AmmoReserve; skipping"
            )]
        );
    }

    #[test]
    fn activator_target_damages_each_edge_presser_once_and_leaves_bystander_untouched() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "presser");
        let first = health(&mut registry);
        let second = health(&mut registry);
        let bystander = health(&mut registry);
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![NamedReaction {
                name: "presser".into(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "applyDamage".into(),
                    target: Some("@activators".into()),
                    tag: None,
                    on_complete: None,
                    args: serde_json::json!({"amount": 25}),
                }),
            }],
            Vec::new(),
            &[],
        );
        let table = TriggerBindingTable::build(&registry, &data, &SlotTable::new());

        for entrant in [first, second] {
            table.execute(
                trigger,
                TriggerEventEdge::Enter,
                &mut registry,
                &mut SlotTable::new(),
                &TriggerFireContext {
                    fired_trigger: Some(trigger),
                    activator: Some(entrant),
                    occupancy: 2,
                },
            );
        }

        assert_eq!(
            registry
                .get_component::<HealthComponent>(first)
                .unwrap()
                .current,
            75.0
        );
        assert_eq!(
            registry
                .get_component::<HealthComponent>(second)
                .unwrap()
                .current,
            75.0
        );
        assert_eq!(
            registry
                .get_component::<HealthComponent>(bystander)
                .unwrap()
                .current,
            100.0
        );
    }

    #[test]
    fn fired_trigger_target_disarms_only_its_plate_and_can_rearm_it() {
        let mut registry = EntityRegistry::new();
        let first = spawn_trigger(&mut registry, "shared");
        let second = spawn_trigger(&mut registry, "shared");
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![NamedReaction {
                name: "shared".into(),
                descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
                    id: SequenceTarget::FiredTrigger,
                    primitive: "disarmTrigger".into(),
                    args: serde_json::json!({}),
                }]),
            }],
            Vec::new(),
            &[],
        );
        let disarm = TriggerBindingTable::build(&registry, &data, &SlotTable::new());
        disarm.execute(
            first,
            TriggerEventEdge::Enter,
            &mut registry,
            &mut SlotTable::new(),
            &TriggerFireContext {
                fired_trigger: Some(first),
                ..Default::default()
            },
        );
        assert!(
            !registry
                .get_component::<TriggerVolumeComponent>(first)
                .unwrap()
                .armed
        );
        assert!(
            registry
                .get_component::<TriggerVolumeComponent>(second)
                .unwrap()
                .armed
        );

        let mut arm_data = DataRegistry::new();
        arm_data.populate_level(
            vec![NamedReaction {
                name: "shared".into(),
                descriptor: ReactionDescriptor::Sequence(vec![SequenceStep {
                    id: SequenceTarget::FiredTrigger,
                    primitive: "armTrigger".into(),
                    args: serde_json::json!({}),
                }]),
            }],
            Vec::new(),
            &[],
        );
        let arm = TriggerBindingTable::build(&registry, &arm_data, &SlotTable::new());
        arm.execute(
            first,
            TriggerEventEdge::Enter,
            &mut registry,
            &mut SlotTable::new(),
            &TriggerFireContext {
                fired_trigger: Some(first),
                ..Default::default()
            },
        );
        assert!(
            registry
                .get_component::<TriggerVolumeComponent>(first)
                .unwrap()
                .armed
        );
    }

    #[test]
    fn occupancy_input_records_enter_and_exit_effective_counts() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "occupancy");
        let ctx = ScriptCtx::new();
        ctx.slot_table
            .borrow_mut()
            .insert(
                "trigger.occupancy".into(),
                SlotRecord::new(SlotSchema {
                    slot_type: SlotType::Number,
                    default: Some(SlotValue::Number(0.0)),
                    range: None,
                    persist: false,
                    readonly: false,
                    ownership: SlotOwnership::Mod,
                    network: Default::default(),
                    per_owner: false,
                    accumulate: None,
                }),
            )
            .unwrap();
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![primitive(
                "occupancy",
                "setState",
                None,
                serde_json::json!({
                    "slot": "trigger.occupancy",
                    "value": {"op":"input", "name":"@occupancy"}
                }),
                None,
            )],
            Vec::new(),
            &[],
        );
        let table = TriggerBindingTable::build_with_script_ctx(&registry, &data, &ctx);
        for count in [2, 1] {
            table.execute_with_script_ctx(
                trigger,
                TriggerEventEdge::Enter,
                &mut registry,
                &ctx,
                &TriggerFireContext {
                    fired_trigger: Some(trigger),
                    occupancy: count,
                    ..Default::default()
                },
            );
            assert_eq!(
                ctx.slot_table
                    .borrow()
                    .get("trigger.occupancy")
                    .unwrap()
                    .value,
                Some(SlotValue::Number(count as f32))
            );
        }
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
                    per_owner: false,
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
        table.execute_with_script_ctx(
            trigger,
            TriggerEventEdge::Enter,
            &mut registry,
            &ctx,
            &TriggerFireContext::default(),
        );
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
                            per_owner: false,
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
                            per_owner: false,
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
        let execution = table.execute_with_script_ctx(
            trigger,
            TriggerEventEdge::Enter,
            &mut registry,
            &ctx,
            &TriggerFireContext::default(),
        );

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
                    per_owner: false,
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

        table.execute(
            trigger,
            TriggerEventEdge::Enter,
            &mut registry,
            &mut slots,
            &TriggerFireContext::default(),
        );
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
                    "moverSetSpinRate",
                    Some("door"),
                    serde_json::json!({ "rate": -90.0 }),
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
        assert_eq!(binding.commands.len(), 3);
        assert!(matches!(
            binding.commands[0],
            BoundTriggerCommand::Mover {
                command: MoverCommand::Start,
                ..
            }
        ));
        assert!(matches!(
            binding.commands[1],
            BoundTriggerCommand::Mover {
                command: MoverCommand::SetSpinRate(rate),
                ..
            } if (rate + 90.0).abs() < f32::EPSILON
        ));
        assert!(matches!(
            binding.commands[2],
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
        assert!(
            table.binding(trigger, TriggerEventEdge::Enter).is_none(),
            "rejected work must not bind the trigger edge"
        );
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
        assert!(
            table.binding(trigger, TriggerEventEdge::Enter).is_none(),
            "rejected work must not bind the trigger edge"
        );

        assert!(
            table
                .execute(
                    trigger,
                    TriggerEventEdge::Enter,
                    &mut registry,
                    &mut slots,
                    &TriggerFireContext::default(),
                )
                .residual()
                .is_none(),
            "a rejected tagged setState must not leave residual work"
        );
        assert_eq!(
            slots
                .get("trigger.flag")
                .and_then(|record| record.value.as_ref()),
            Some(&SlotValue::Number(0.0)),
            "tagged setState must not perform an in-tick write"
        );
    }

    // Regression: a sentinel target bypassed the tag-only validation and was
    // discarded by bind_command while still allowing the system write.
    #[test]
    fn bind_rejects_sentinel_targeted_set_state_without_an_in_tick_write() {
        let mut registry = EntityRegistry::new();
        let trigger = spawn_trigger(&mut registry, "set_sentinel_targeted");
        let mut data = DataRegistry::new();
        data.populate_level(
            vec![NamedReaction {
                name: "set_sentinel_targeted".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "setState".to_string(),
                    target: Some("@activators".to_string()),
                    tag: None,
                    on_complete: None,
                    args: serde_json::json!({ "slot": "trigger.flag", "value": 1 }),
                }),
            }],
            Vec::new(),
            &[],
        );
        let mut slots = writable_slots();

        let table = TriggerBindingTable::build(&registry, &data, &slots);
        assert!(
            table.binding(trigger, TriggerEventEdge::Enter).is_none(),
            "rejected work must not bind the trigger edge"
        );

        table.execute(
            trigger,
            TriggerEventEdge::Enter,
            &mut registry,
            &mut slots,
            &TriggerFireContext::default(),
        );
        assert_eq!(
            slots
                .get("trigger.flag")
                .and_then(|record| record.value.as_ref()),
            Some(&SlotValue::Number(0.0)),
            "sentinel-targeted setState must not perform an in-tick write"
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
                    id: trigger.into(),
                    primitive: "setState".to_string(),
                    args: serde_json::json!({ "slot": "trigger.flag", "value": 1 }),
                }]),
            }],
            Vec::new(),
            &[],
        );
        let mut slots = writable_slots();

        let table = TriggerBindingTable::build(&registry, &data, &slots);
        assert!(
            table.binding(trigger, TriggerEventEdge::Enter).is_none(),
            "rejected work must not bind the trigger edge"
        );

        assert!(
            table
                .execute(
                    trigger,
                    TriggerEventEdge::Enter,
                    &mut registry,
                    &mut slots,
                    &TriggerFireContext::default(),
                )
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
        assert!(
            table.binding(trigger, TriggerEventEdge::Enter).is_none(),
            "ProgressTracker owns the progress target, so it must not bind the trigger edge"
        );
    }
}
