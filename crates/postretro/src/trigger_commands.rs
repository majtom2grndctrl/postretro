//! Bound trigger commands and their fixed-tick execution.
//! Governing context: `context/lib/development_guide.md` §4.3; `context/lib/scripting.md` §12.

use postretro_entities::{
    ComponentKind, EntityId, EntityRegistry, MoverCommand, ScriptCtx, SlotTable, SlotValue,
};
use postretro_foundation::{BoundProgram, IrValue, eval_and_write};
use postretro_scripting_core::ir_scopes::DispatchScope;
use postretro_scripting_core::store_bridge::{apply_store_slot_batch, validate_slot_value};

use crate::health::reactions::{self as health_reactions, ApplyDamageArgs};
use crate::kinematic_mover::{MoverCommandDiagnostics, apply_mover_command_to_targets};
use crate::scripting::reactions::animation::{self as animation_reactions, SetAnimationStateArgs};
use crate::scripting::reactions::enemy_state::{
    UpdateEnemyStateArgs, apply_update_enemy_state_to_brain,
};
use crate::spawner::{SpawnContext, spawn_from_spawner_tag};
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
    GrantHealth {
        target: BoundTarget,
        amount: f32,
    },
    GrantAmmo {
        target: BoundTarget,
        ammo_type: String,
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
    AddOwnerSlot {
        target: BoundTarget,
        slot: String,
        delta: f32,
    },
    AnimationState {
        target: BoundTarget,
        state: String,
    },
    UpdateEnemyState {
        target: BoundTarget,
        aggro: Option<bool>,
    },
    Spawn {
        target: BoundTarget,
    },
}

/// The trigger-owned form of a `setState` value. Literal writes keep the
/// pre-existing fast path; inline IR is bound once while the level installs and
/// runs against the live store only at the tick write point.
#[derive(Debug, Clone)]
pub(crate) enum BoundStoreValue {
    Literal(SlotValue),
    Ir(BoundProgram<DispatchScope>),
}

#[derive(Debug, Clone)]
pub(crate) enum BoundTarget {
    Tag(String),
    Entity(EntityId),
    Activators,
    FiredTrigger,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TriggerFireContext {
    pub(crate) fired_trigger: Option<EntityId>,
    /// One authoritative player produces each fire. Keeping that identity as
    /// an option lets command dispatch borrow it instead of cloning a vector
    /// for every command in the fixed-tick path.
    pub(crate) activator: Option<EntityId>,
    pub(crate) occupancy: usize,
}

enum ResolvedTargets<'a> {
    Borrowed(&'a [EntityId]),
    Single(EntityId),
    Owned(Vec<EntityId>),
}

impl ResolvedTargets<'_> {
    fn as_slice(&self) -> &[EntityId] {
        match self {
            Self::Borrowed(targets) => targets,
            Self::Single(target) => std::slice::from_ref(target),
            Self::Owned(targets) => targets,
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundTriggerCommandKind {
    Mover,
    Damage,
    GrantHealth,
    GrantAmmo,
    Arm,
    Disarm,
    StoreSlot,
    AddOwnerSlot,
    AnimationState,
    UpdateEnemyState,
    Spawn,
}

impl BoundTriggerCommand {
    pub(crate) fn execute(
        &self,
        registry: &mut EntityRegistry,
        slot_table: &mut SlotTable,
        command_diagnostics: &MoverCommandDiagnostics,
        spawn_context: &SpawnContext,
        fire_context: &TriggerFireContext,
    ) {
        match self {
            Self::StoreSlot { slot, value } => {
                let BoundStoreValue::Literal(value) = value else {
                    unreachable!("IR-valued trigger setState must execute with a ScriptCtx");
                };
                if slot_table
                    .get(slot)
                    .is_some_and(|record| record.schema.per_owner)
                {
                    log::warn!(
                        "[Trigger] setState rejects per-owner slot `{slot}` at execution time"
                    );
                    return;
                }
                if let Err(error) =
                    apply_store_slot_batch(slot_table, &[(slot.clone(), value.clone())])
                {
                    log::warn!("[Trigger] setState binding for `{slot}` failed: {error}");
                }
            }
            Self::AddOwnerSlot {
                target,
                slot,
                delta,
            } => Self::apply_owner_slot_delta(
                registry,
                slot_table,
                target,
                slot,
                *delta,
                fire_context,
            ),
            _ => self.execute_non_store(registry, command_diagnostics, spawn_context, fire_context),
        }
    }

    pub(crate) fn execute_with_script_ctx(
        &self,
        registry: &mut EntityRegistry,
        script_ctx: &ScriptCtx,
        dispatch_scope: &mut DispatchScope,
        command_diagnostics: &MoverCommandDiagnostics,
        spawn_context: &SpawnContext,
        fire_context: &TriggerFireContext,
    ) {
        match self {
            Self::StoreSlot { slot, value } => match value {
                BoundStoreValue::Literal(value) => {
                    let mut slot_table = script_ctx.slot_table.borrow_mut();
                    if slot_table
                        .get(slot)
                        .is_some_and(|record| record.schema.per_owner)
                    {
                        log::warn!(
                            "[Trigger] setState rejects per-owner slot `{slot}` at execution time"
                        );
                        return;
                    }
                    if let Err(error) =
                        apply_store_slot_batch(&mut slot_table, &[(slot.clone(), value.clone())])
                    {
                        log::warn!("[Trigger] setState binding for `{slot}` failed: {error}");
                    }
                }
                BoundStoreValue::Ir(program) => {
                    if let Err(error) = dispatch_scope
                        .seed("@occupancy", IrValue::Number(fire_context.occupancy as f32))
                    {
                        log::warn!("[Trigger] failed to seed @occupancy: {error:?}");
                        return;
                    }
                    eval_and_write(program, dispatch_scope);
                }
            },
            Self::AddOwnerSlot {
                target,
                slot,
                delta,
            } => {
                if !script_ctx.owner_slot_writes_enabled.get() {
                    return;
                }
                let mut slot_table = script_ctx.slot_table.borrow_mut();
                Self::apply_owner_slot_delta(
                    registry,
                    &mut slot_table,
                    target,
                    slot,
                    *delta,
                    fire_context,
                );
            }
            _ => self.execute_non_store(registry, command_diagnostics, spawn_context, fire_context),
        }
    }

    fn apply_owner_slot_delta(
        registry: &EntityRegistry,
        slot_table: &mut SlotTable,
        target: &BoundTarget,
        slot: &str,
        delta: f32,
        fire_context: &TriggerFireContext,
    ) {
        let targets = target.resolve(registry, fire_context);
        if targets.as_slice().is_empty() {
            return;
        }
        for &pawn in targets.as_slice() {
            let Some(seat) = registry.seat_for_pawn(pawn) else {
                log::warn!("[Trigger] addSlot target {pawn:?} has no player seat; skipping");
                continue;
            };
            let Some(record) = slot_table.get_mut(slot) else {
                debug_assert!(false, "bound addSlot `{slot}` disappeared before execution");
                return;
            };
            let Some(SlotValue::Number(current)) = record.per_seat_value(seat) else {
                log::warn!("[Trigger] addSlot requires numeric slot `{slot}`; skipping");
                return;
            };
            match validate_slot_value(slot, &record.schema, SlotValue::Number(*current + delta)) {
                Ok(next) => record.set_per_seat_value(seat, next),
                Err(error) => log::warn!(
                    "[Trigger] addSlot for `{slot}` failed validation; skipping: {error}"
                ),
            }
        }
    }

    fn execute_non_store(
        &self,
        registry: &mut EntityRegistry,
        command_diagnostics: &MoverCommandDiagnostics,
        spawn_context: &SpawnContext,
        fire_context: &TriggerFireContext,
    ) {
        match self {
            Self::Mover { target, command } => apply_mover_command_to_targets(
                registry,
                target.resolve(registry, fire_context).as_slice(),
                command,
                command_diagnostics,
            ),
            Self::Damage { target, amount } => {
                let targets = target.resolve(registry, fire_context);
                if let Err(error) = health_reactions::dispatch(
                    registry,
                    targets.as_slice(),
                    &ApplyDamageArgs { amount: *amount },
                ) {
                    log::warn!("[Trigger] applyDamage binding failed: {error}");
                }
            }
            Self::GrantHealth { target, amount } => {
                let targets = target.resolve(registry, fire_context);
                for &target in targets.as_slice() {
                    let _ = postretro_entities::components::grant::grant_health(
                        registry, target, *amount,
                    );
                }
            }
            Self::GrantAmmo {
                target,
                ammo_type,
                amount,
            } => {
                let targets = target.resolve(registry, fire_context);
                for &target in targets.as_slice() {
                    let _ = postretro_entities::components::grant::grant_ammo(
                        registry, target, ammo_type, *amount,
                    );
                }
            }
            Self::Arm { target } => arm_trigger_targets(
                registry,
                target.resolve(registry, fire_context).as_slice(),
                command_diagnostics,
            ),
            Self::Disarm { target } => disarm_trigger_targets(
                registry,
                target.resolve(registry, fire_context).as_slice(),
                command_diagnostics,
            ),
            Self::AnimationState { target, state } => {
                let targets = target.resolve(registry, fire_context);
                if let Err(error) = animation_reactions::dispatch(
                    registry,
                    targets.as_slice(),
                    &SetAnimationStateArgs {
                        state: state.clone(),
                    },
                ) {
                    log::warn!("[Trigger] setAnimationState binding failed: {error}");
                }
            }
            Self::UpdateEnemyState { target, aggro } => {
                let BoundTarget::Tag(tag) = target else {
                    log::warn!(
                        "[Trigger] updateEnemyState requires a tag target; special target is invalid; skipping"
                    );
                    return;
                };
                let targets: Vec<_> = registry
                    .query_by_component_and_tag(ComponentKind::Brain, Some(tag))
                    .map(|(entity, _)| entity)
                    .collect();
                if targets.is_empty() {
                    log::debug!("[Trigger] updateEnemyState: empty Brain tag match, no-op");
                    return;
                }
                let args = UpdateEnemyStateArgs { aggro: *aggro };
                for entity in targets {
                    apply_update_enemy_state_to_brain(registry, entity, &args);
                }
            }
            Self::Spawn { target } => {
                let BoundTarget::Tag(tag) = target else {
                    log::warn!(
                        "[Trigger] spawnFromSpawner requires a fire-time tag target; special target is invalid; skipping"
                    );
                    return;
                };
                spawn_from_spawner_tag(registry, tag, spawn_context);
            }
            Self::StoreSlot { .. } | Self::AddOwnerSlot { .. } => {
                unreachable!("store slots execute through their store path")
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn kind(&self) -> BoundTriggerCommandKind {
        match self {
            Self::Mover { .. } => BoundTriggerCommandKind::Mover,
            Self::Damage { .. } => BoundTriggerCommandKind::Damage,
            Self::GrantHealth { .. } => BoundTriggerCommandKind::GrantHealth,
            Self::GrantAmmo { .. } => BoundTriggerCommandKind::GrantAmmo,
            Self::Arm { .. } => BoundTriggerCommandKind::Arm,
            Self::Disarm { .. } => BoundTriggerCommandKind::Disarm,
            Self::StoreSlot { .. } => BoundTriggerCommandKind::StoreSlot,
            Self::AddOwnerSlot { .. } => BoundTriggerCommandKind::AddOwnerSlot,
            Self::AnimationState { .. } => BoundTriggerCommandKind::AnimationState,
            Self::UpdateEnemyState { .. } => BoundTriggerCommandKind::UpdateEnemyState,
            Self::Spawn { .. } => BoundTriggerCommandKind::Spawn,
        }
    }
}

impl BoundTarget {
    fn resolve<'a>(
        &self,
        registry: &EntityRegistry,
        fire_context: &'a TriggerFireContext,
    ) -> ResolvedTargets<'a> {
        match self {
            Self::Tag(tag) => ResolvedTargets::Owned(
                registry
                    .query_by_component_and_tag(ComponentKind::Transform, Some(tag))
                    .map(|(id, _)| id)
                    .collect(),
            ),
            Self::Entity(id) => {
                if registry.exists(*id) {
                    ResolvedTargets::Single(*id)
                } else {
                    log::warn!(
                        "[Trigger] sequenced binding target {id:?} no longer exists; skipping"
                    );
                    ResolvedTargets::Borrowed(&[])
                }
            }
            Self::Activators => ResolvedTargets::Borrowed(fire_context.activator.as_slice()),
            Self::FiredTrigger => ResolvedTargets::Borrowed(fire_context.fired_trigger.as_slice()),
        }
    }
}
