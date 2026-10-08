//! Lower one consequential primitive or sequence step into a validated
//! fixed-tick `BoundTriggerCommand`.
//! See: context/lib/scripting.md §12

use postretro_entities::{MoverCommand, ScriptCtx, SlotTable};
use postretro_foundation::{BakedIr, CURRENT_IR_VERSION, ir_node_from_json};
use postretro_scripting_core::data_descriptors::{
    PrimitiveDescriptor, SequenceStep, SequenceTarget,
};
use postretro_scripting_core::ir::bind;
use postretro_scripting_core::ir_scopes::DispatchScope;
use postretro_scripting_core::store_bridge::{json_value_for_slot, validate_slot_value};
use serde::Deserialize;

use super::TRIGGER_EVENT_INPUTS;
use crate::grant::{GrantAmmoArgs, GrantHealthArgs};
use crate::health::reactions::ApplyDamageArgs;
use crate::mover_commands::MoverSetSpinRateArgs;
use crate::scripting::reactions::animation::SetAnimationStateArgs;
use crate::scripting::reactions::npc_state::UpdateNpcStateArgs;
use crate::trigger_commands::{BoundStoreValue, BoundTarget, BoundTriggerCommand};

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

pub(super) fn bind_primitive(
    primitive: &PrimitiveDescriptor,
    slot_table: &SlotTable,
    script_ctx: Option<&ScriptCtx>,
) -> Option<BoundTriggerCommand> {
    if primitive.primitive == "setState"
        && (primitive.tag.is_some() || primitive.target.is_some() || primitive.kind.is_some())
    {
        log::warn!(
            "[Trigger] setState is system-targeted and cannot carry a target tag, group or sentinel; not binding"
        );
        return None;
    }
    // A group keeps its kind: lowering it to a kindless tag binding would hit
    // every tagged entity, a player pawn under an npc group included. A
    // tagless group is a complete target, so it never reads as "no target".
    let target = if let Some(group) = primitive.group_target() {
        Some(BoundTarget::Group(group))
    } else if let Some(sentinel) = primitive.target.as_deref() {
        match sentinel {
            "@activators" => Some(BoundTarget::Activators),
            "@trigger" => Some(BoundTarget::FiredTrigger),
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

pub(super) fn bind_sequence_step(
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
    let target = Some(match &step.id {
        SequenceTarget::Entity(id) => BoundTarget::Entity(*id),
        SequenceTarget::Group(group) => BoundTarget::Group(group.clone()),
        SequenceTarget::Activators => BoundTarget::Activators,
        SequenceTarget::FiredTrigger => BoundTarget::FiredTrigger,
        // Control steps never bind to an in-tick command: `BoundTarget` has no
        // analogue for them. The amended `partition_direct_reaction` (Task 3)
        // routes a `Fire` to a `DeferredEvent` and a `Wait` plus its tail to the
        // residual before reaching here, so this arm is the belt-and-braces guard.
        SequenceTarget::Wait | SequenceTarget::Fire => return None,
    });
    bind_command(&step.primitive, target, &step.args, slot_table, script_ctx)
}

pub(super) fn bind_command(
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
            let args: AddSlotArgs = match serde_json::from_value::<AddSlotArgs>(args.clone()) {
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
            let Some(target) = target_from_context else {
                log::warn!(
                    "[Trigger] addSlot for slot `{}` has no target tag; not binding",
                    args.slot
                );
                return None;
            };
            Some(BoundTriggerCommand::AddOwnerSlot {
                target,
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
        "updateNpcState" => {
            let Some(target) = target_from_context else {
                log::warn!("[Trigger] updateNpcState requires a fire-time tag target; not binding");
                return None;
            };
            let args: UpdateNpcStateArgs = match serde_json::from_value(args.clone()) {
                Ok(args) => args,
                Err(error) => {
                    log::warn!("[Trigger] updateNpcState has invalid args; not binding: {error}");
                    return None;
                }
            };
            Some(BoundTriggerCommand::UpdateNpcState {
                target,
                aggro: args.aggro,
            })
        }
        "spawnFromSpawner" => match target_from_context {
            // A spawner member's `fire()` step: that spawner only.
            Some(target @ BoundTarget::Entity(_)) => Some(BoundTriggerCommand::Spawn { target }),
            // The raw tag-keyed descriptor stays valid wire data.
            Some(BoundTarget::Tag(tag)) if !tag.is_empty() => Some(BoundTriggerCommand::Spawn {
                target: BoundTarget::Tag(tag),
            }),
            Some(BoundTarget::Tag(_)) => {
                log::warn!(
                    "[Trigger] spawnFromSpawner requires a non-empty fire-time tag target; not binding"
                );
                None
            }
            _ => {
                log::warn!(
                    "[Trigger] spawnFromSpawner requires a spawner member or fire-time tag target; not binding"
                );
                None
            }
        },
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
    if slot_table
        .get(&args.slot)
        .is_some_and(|record| record.schema.per_owner)
    {
        log::warn!(
            "[Trigger] setState rejects per-owner slot `{}` at bind time",
            args.slot
        );
        return None;
    }
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
