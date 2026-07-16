//! Bound trigger commands and their fixed-tick execution.

use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, MoverCommand, ScriptCtx, SlotTable, SlotValue,
};
use postretro_foundation::{BoundProgram, eval_and_write};
use postretro_scripting_core::ir_scopes::StoreScope;
use postretro_scripting_core::store_bridge::apply_store_slot_batch;

use crate::health::reactions::{self as health_reactions, ApplyDamageArgs};
use crate::kinematic_mover::{MoverCommandDiagnostics, apply_mover_command_to_targets};
use crate::scripting::reactions::animation::{self as animation_reactions, SetAnimationStateArgs};
use crate::trigger_system::{arm_trigger_targets, disarm_trigger_targets};

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
        value: BoundStoreValue,
    },
    AnimationState {
        target: BoundTarget,
        state: String,
    },
}

/// The trigger-owned form of a `setState` value. Literal writes keep the
/// pre-existing fast path; inline IR is bound once while the level installs and
/// runs against the live store only at the tick write point.
#[derive(Debug, Clone)]
pub(crate) enum BoundStoreValue {
    Literal(SlotValue),
    Ir(BoundProgram<StoreScope>),
}

#[derive(Debug, Clone)]
pub(crate) enum BoundTarget {
    Tag(String),
    Entity(EntityId),
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundTriggerCommandKind {
    Mover,
    Damage,
    Arm,
    Disarm,
    StoreSlot,
    AnimationState,
}

impl BoundTriggerCommand {
    pub(crate) fn execute(
        &self,
        registry: &mut EntityRegistry,
        slot_table: &mut SlotTable,
        command_diagnostics: &MoverCommandDiagnostics,
    ) {
        match self {
            Self::StoreSlot { slot, value } => {
                let BoundStoreValue::Literal(value) = value else {
                    unreachable!("IR-valued trigger setState must execute with a ScriptCtx");
                };
                if let Err(error) =
                    apply_store_slot_batch(slot_table, &[(slot.clone(), value.clone())])
                {
                    log::warn!("[Trigger] setState binding for `{slot}` failed: {error}");
                }
            }
            _ => self.execute_non_store(registry, command_diagnostics),
        }
    }

    pub(crate) fn execute_with_script_ctx(
        &self,
        registry: &mut EntityRegistry,
        script_ctx: &ScriptCtx,
        command_diagnostics: &MoverCommandDiagnostics,
    ) {
        match self {
            Self::StoreSlot { slot, value } => match value {
                BoundStoreValue::Literal(value) => {
                    let mut slot_table = script_ctx.slot_table.borrow_mut();
                    if let Err(error) =
                        apply_store_slot_batch(&mut slot_table, &[(slot.clone(), value.clone())])
                    {
                        log::warn!("[Trigger] setState binding for `{slot}` failed: {error}");
                    }
                }
                BoundStoreValue::Ir(program) => {
                    let mut scope = StoreScope::script(script_ctx.clone());
                    eval_and_write(program, &mut scope);
                }
            },
            _ => self.execute_non_store(registry, command_diagnostics),
        }
    }

    fn execute_non_store(
        &self,
        registry: &mut EntityRegistry,
        command_diagnostics: &MoverCommandDiagnostics,
    ) {
        match self {
            Self::Mover { target, command } => apply_mover_command_to_targets(
                registry,
                &target.resolve(registry),
                command,
                command_diagnostics,
            ),
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
            Self::Arm { target } => {
                arm_trigger_targets(registry, &target.resolve(registry), command_diagnostics)
            }
            Self::Disarm { target } => {
                disarm_trigger_targets(registry, &target.resolve(registry), command_diagnostics)
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
            Self::StoreSlot { .. } => unreachable!("store slots execute through their store path"),
        }
    }

    #[cfg(test)]
    pub(crate) fn kind(&self) -> BoundTriggerCommandKind {
        match self {
            Self::Mover { .. } => BoundTriggerCommandKind::Mover,
            Self::Damage { .. } => BoundTriggerCommandKind::Damage,
            Self::Arm { .. } => BoundTriggerCommandKind::Arm,
            Self::Disarm { .. } => BoundTriggerCommandKind::Disarm,
            Self::StoreSlot { .. } => BoundTriggerCommandKind::StoreSlot,
            Self::AnimationState { .. } => BoundTriggerCommandKind::AnimationState,
        }
    }
}

impl BoundTarget {
    pub(crate) fn resolve(&self, registry: &EntityRegistry) -> Vec<EntityId> {
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
