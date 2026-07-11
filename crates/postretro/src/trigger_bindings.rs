//! Bind trigger event reactions at level install and execute their fixed-tick work.
//! See: context/lib/entity_model.md §5 · context/lib/scripting.md §10

use std::collections::HashMap;

use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, MoverCommand, SlotTable, SlotValue,
    TriggerVolumeComponent,
};
use postretro_scripting_core::data_descriptors::{
    NamedReaction, PrimitiveDescriptor, ReactionDescriptor, SequenceStep,
};
use postretro_scripting_core::data_registry::DataRegistry;
use postretro_scripting_core::store_bridge::{
    apply_store_slot_batch, json_value_for_slot, validate_slot_value,
};
use serde::Deserialize;

use crate::health::reactions::{self as health_reactions, ApplyDamageArgs};
use crate::kinematic_mover::apply_mover_command_to_targets;
use crate::scripting::reactions::animation::{self as animation_reactions, SetAnimationStateArgs};
use crate::trigger_system::{arm_trigger_targets, disarm_trigger_targets};

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
pub(crate) enum TriggerBindingEdge {
    Enter,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TriggerResidualHandle(usize);

#[derive(Debug, Clone)]
pub(crate) struct TriggerResidual {
    descriptors: Vec<ReactionDescriptor>,
}

impl TriggerResidual {
    pub(crate) fn descriptors(&self) -> &[ReactionDescriptor] {
        &self.descriptors
    }
}

#[derive(Debug, Default)]
pub(crate) struct TriggerBindingTable {
    bindings: HashMap<(EntityId, TriggerBindingEdge), TriggerBinding>,
    residuals: Vec<TriggerResidual>,
}

#[derive(Debug)]
struct TriggerBinding {
    commands: Vec<BoundTriggerCommand>,
    residual: Option<TriggerResidualHandle>,
}

/// The closed set of trigger work allowed in the VM-free fixed-tick seam.
/// `Tag` targets mirror named primitive dispatch; `Entity` targets preserve a
/// directly-owned sequenced step without performing a reaction-name lookup.
#[derive(Debug, Clone)]
pub(crate) enum BoundTriggerCommand {
    Mover {
        target: BoundTarget,
        command: MoverCommand,
    },
    Damage {
        target: BoundTarget,
        amount: f32,
    },
    Arm {
        target: BoundTarget,
    },
    Disarm {
        target: BoundTarget,
    },
    StoreSlot {
        slot: String,
        value: SlotValue,
    },
    AnimationState {
        target: BoundTarget,
        state: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum BoundTarget {
    Tag(String),
    Entity(EntityId),
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
    pub(crate) fn build(
        registry: &EntityRegistry,
        data_registry: &DataRegistry,
        slot_table: &SlotTable,
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
                TriggerBindingEdge::Enter,
                &component.on_fire,
                data_registry,
                slot_table,
            );
            table.bind_event(
                trigger,
                TriggerBindingEdge::Exit,
                &component.on_exit,
                data_registry,
                slot_table,
            );
        }
        table
    }

    fn bind_event(
        &mut self,
        trigger: EntityId,
        edge: TriggerBindingEdge,
        event_name: &str,
        data_registry: &DataRegistry,
        slot_table: &SlotTable,
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
        let mut descriptors = Vec::new();
        let mut chain_path = vec![event_name.to_string()];
        for reaction in &matched {
            partition_reaction(
                reaction,
                false,
                data_registry,
                slot_table,
                &mut commands,
                &mut descriptors,
            );
        }
        for next in matched
            .iter()
            .filter_map(|reaction| match &reaction.descriptor {
                ReactionDescriptor::Primitive(primitive) => primitive.on_complete.as_deref(),
                _ => None,
            })
        {
            append_chain_residuals(
                next,
                data_registry,
                slot_table,
                &mut descriptors,
                &mut chain_path,
            );
        }

        let residual = (!descriptors.is_empty()).then(|| {
            let handle = TriggerResidualHandle(self.residuals.len());
            self.residuals.push(TriggerResidual { descriptors });
            handle
        });
        self.bindings
            .insert((trigger, edge), TriggerBinding { commands, residual });
    }

    pub(crate) fn execute(
        &self,
        trigger: EntityId,
        edge: TriggerBindingEdge,
        registry: &mut EntityRegistry,
        slot_table: &mut SlotTable,
    ) -> Option<TriggerResidualHandle> {
        let binding = self.bindings.get(&(trigger, edge))?;
        for command in &binding.commands {
            command.execute(registry, slot_table);
        }
        binding.residual
    }

    pub(crate) fn residual(&self, handle: TriggerResidualHandle) -> Option<&TriggerResidual> {
        self.residuals.get(handle.0)
    }

    #[cfg(test)]
    fn binding(&self, trigger: EntityId, edge: TriggerBindingEdge) -> Option<&TriggerBinding> {
        self.bindings.get(&(trigger, edge))
    }
}

impl BoundTriggerCommand {
    fn execute(&self, registry: &mut EntityRegistry, slot_table: &mut SlotTable) {
        match self {
            Self::Mover { target, command } => {
                apply_mover_command_to_targets(registry, &target.resolve(registry), command);
            }
            Self::Damage { target, amount } => {
                let targets = target.resolve(registry);
                if let Err(error) = health_reactions::dispatch(
                    registry,
                    &targets,
                    &ApplyDamageArgs { amount: *amount },
                ) {
                    log::warn!("[Trigger] applyDamage binding failed: {error}");
                }
            }
            Self::Arm { target } => arm_trigger_targets(registry, &target.resolve(registry)),
            Self::Disarm { target } => {
                disarm_trigger_targets(registry, &target.resolve(registry));
            }
            Self::StoreSlot { slot, value } => {
                if let Err(error) =
                    apply_store_slot_batch(slot_table, &[(slot.clone(), value.clone())])
                {
                    log::warn!("[Trigger] setState binding for `{slot}` failed: {error}");
                }
            }
            Self::AnimationState { target, state } => {
                let targets = target.resolve(registry);
                if let Err(error) = animation_reactions::dispatch(
                    registry,
                    &targets,
                    &SetAnimationStateArgs {
                        state: state.clone(),
                    },
                ) {
                    log::warn!("[Trigger] setAnimationState binding failed: {error}");
                }
            }
        }
    }
}

impl BoundTarget {
    fn resolve(&self, registry: &EntityRegistry) -> Vec<EntityId> {
        match self {
            Self::Tag(tag) => registry
                .query_by_component_and_tag(ComponentKind::Transform, Some(tag))
                .map(|(id, _)| id)
                .collect(),
            Self::Entity(id) => {
                if registry.exists(*id) {
                    vec![*id]
                } else {
                    log::warn!(
                        "[Trigger] sequenced binding target {id:?} no longer exists; skipping"
                    );
                    Vec::new()
                }
            }
        }
    }
}

fn partition_reaction(
    reaction: &NamedReaction,
    defer_consequential: bool,
    data_registry: &DataRegistry,
    slot_table: &SlotTable,
    commands: &mut Vec<BoundTriggerCommand>,
    descriptors: &mut Vec<ReactionDescriptor>,
) {
    match &reaction.descriptor {
        ReactionDescriptor::Progress(progress) => {
            if event_contains_consequential(&progress.fire, data_registry, &mut Vec::new()) {
                log::warn!(
                    "[Trigger] event `{}` reaches consequential work through Progress `{}`; it stays deferred to app dispatch",
                    reaction.name,
                    progress.fire,
                );
            }
            descriptors.push(reaction.descriptor.clone());
        }
        ReactionDescriptor::Primitive(primitive) => {
            let class = classify(&primitive.primitive);
            let mut residual = primitive.clone();
            residual.on_complete = None;
            match class {
                PrimitiveClass::Consequential if !defer_consequential => {
                    if let Some(command) = bind_primitive(primitive, slot_table) {
                        commands.push(command);
                    }
                }
                _ => descriptors.push(ReactionDescriptor::Primitive(residual)),
            }
        }
        ReactionDescriptor::Sequence(steps) => {
            let mut residual_steps = Vec::new();
            for step in steps {
                if classify(&step.primitive) == PrimitiveClass::Consequential
                    && !defer_consequential
                {
                    if let Some(command) = bind_sequence_step(step, slot_table) {
                        commands.push(command);
                    }
                } else {
                    residual_steps.push(step.clone());
                }
            }
            if !residual_steps.is_empty() {
                descriptors.push(ReactionDescriptor::Sequence(residual_steps));
            }
        }
    }
}

fn append_chain_residuals(
    event_name: &str,
    data_registry: &DataRegistry,
    slot_table: &SlotTable,
    descriptors: &mut Vec<ReactionDescriptor>,
    chain_path: &mut Vec<String>,
) {
    if chain_path.iter().any(|name| name == event_name) {
        log::warn!(
            "[Trigger] onComplete chain for `{}` contains a cycle through `{event_name}`; stopping residual expansion",
            chain_path[0],
        );
        return;
    }
    let matched: Vec<&NamedReaction> = data_registry
        .reactions
        .iter()
        .filter(|reaction| reaction.name == event_name)
        .collect();
    if matched.is_empty() {
        log::warn!(
            "[Trigger] onComplete chain for `{}` references unknown active event `{event_name}`",
            chain_path[0],
        );
        return;
    }
    if event_contains_consequential(event_name, data_registry, &mut Vec::new()) {
        log::warn!(
            "[Trigger] event `{}` buries consequential work under onComplete `{event_name}`; it stays deferred to app dispatch",
            chain_path[0],
        );
    }

    chain_path.push(event_name.to_string());
    for reaction in &matched {
        let mut ignored_commands = Vec::new();
        partition_reaction(
            reaction,
            true,
            data_registry,
            slot_table,
            &mut ignored_commands,
            descriptors,
        );
    }
    for next in matched
        .iter()
        .filter_map(|reaction| match &reaction.descriptor {
            ReactionDescriptor::Primitive(primitive) => primitive.on_complete.as_deref(),
            _ => None,
        })
    {
        append_chain_residuals(next, data_registry, slot_table, descriptors, chain_path);
    }
    let _ = chain_path.pop();
}

fn event_contains_consequential(
    event_name: &str,
    data_registry: &DataRegistry,
    seen: &mut Vec<String>,
) -> bool {
    if seen.iter().any(|name| name == event_name) {
        return false;
    }
    seen.push(event_name.to_string());
    let found = data_registry
        .reactions
        .iter()
        .filter(|reaction| reaction.name == event_name)
        .any(|reaction| match &reaction.descriptor {
            ReactionDescriptor::Progress(progress) => {
                event_contains_consequential(&progress.fire, data_registry, seen)
            }
            ReactionDescriptor::Primitive(primitive) => {
                classify(&primitive.primitive) == PrimitiveClass::Consequential
                    || primitive
                        .on_complete
                        .as_deref()
                        .is_some_and(|next| event_contains_consequential(next, data_registry, seen))
            }
            ReactionDescriptor::Sequence(steps) => steps
                .iter()
                .any(|step| classify(&step.primitive) == PrimitiveClass::Consequential),
        });
    let _ = seen.pop();
    found
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
) -> Option<BoundTriggerCommand> {
    let target = match primitive.primitive.as_str() {
        "setState" => None,
        _ => primitive
            .tag
            .as_deref()
            .map(|tag| BoundTarget::Tag(tag.to_string())),
    };
    bind_command(&primitive.primitive, target, &primitive.args, slot_table)
}

fn bind_sequence_step(step: &SequenceStep, slot_table: &SlotTable) -> Option<BoundTriggerCommand> {
    let target = (step.primitive != "setState").then_some(BoundTarget::Entity(step.id));
    bind_command(&step.primitive, target, &step.args, slot_table)
}

fn bind_command(
    primitive: &str,
    target: Option<BoundTarget>,
    args: &serde_json::Value,
    slot_table: &SlotTable,
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
        "setState" => bind_store_slot(args, slot_table),
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
) -> Option<BoundTriggerCommand> {
    let args: SetStateArgs = match serde_json::from_value(args.clone()) {
        Ok(args) => args,
        Err(error) => {
            log::warn!("[Trigger] setState has invalid args; not binding: {error}");
            return None;
        }
    };
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
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::{
        NumericRange, SlotOwnership, SlotRecord, SlotSchema, SlotType, Transform,
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
                    }),
                )],
            )
            .unwrap();
        slots
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
            .binding(trigger, TriggerBindingEdge::Enter)
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
            residual.descriptors(),
            [ReactionDescriptor::Primitive(PrimitiveDescriptor { primitive, .. })]
                if primitive == "flashScreen"
        ));
    }

    #[test]
    fn bind_defers_consequential_on_complete_chain_to_residual() {
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
            .binding(trigger, TriggerBindingEdge::Enter)
            .expect("named trigger event binds");
        assert_eq!(binding.commands.len(), 1);
        let residual = table
            .residual(binding.residual.expect("chain is residual"))
            .unwrap();
        assert!(matches!(
            residual.descriptors(),
            [ReactionDescriptor::Primitive(PrimitiveDescriptor { primitive, .. })]
                if primitive == "applyDamage"
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
            .binding(trigger, TriggerBindingEdge::Enter)
            .expect("the known event is recorded even when it has no executable work");
        assert!(binding.commands.is_empty());
        assert!(binding.residual.is_none());
    }
}
