//! Authoritative player life-state writes, shared by fixed-tick and deferred commands.
//! See: context/lib/scripting.md §12 (Player events).

use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry, ScriptCtx};
use postretro_foundation::enter_life_state;
use postretro_scripting_core::group_resolution::group_commands_apply_here;
use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

pub fn set_life_state(
    ctx: &ScriptCtx,
    registry: &mut EntityRegistry,
    targets: &[EntityId],
    state: &str,
) {
    if !group_commands_apply_here(ctx) {
        return;
    }
    let data = ctx.data_registry.borrow();
    for &pawn in targets {
        let block = registry
            .get_component::<DescriptorProvenance>(pawn)
            .ok()
            .and_then(|provenance| {
                data.entities.iter().find(|descriptor| {
                    descriptor.canonical_name.as_deref() == Some(provenance.canonical_name.as_str())
                })
            })
            .and_then(|descriptor| descriptor.life_states.as_ref());
        let Some((id, profile)) = block.and_then(|block| block.state(state)) else {
            log::warn!("[LifeState] target {pawn:?} does not declare state `{state}`; skipping");
            continue;
        };
        let revision = block.expect("resolved state has a block").table_revision;
        let Ok(ComponentValue::PlayerMovement(movement)) =
            registry.get_component_value_mut(pawn, ComponentKind::PlayerMovement)
        else {
            log::warn!("[LifeState] target {pawn:?} is not a player pawn; skipping");
            continue;
        };
        if movement.life_state == id && movement.life_state_table_revision == revision {
            continue;
        }
        enter_life_state(movement, id, profile, revision);
        if !profile.fire {
            crate::scripting_systems::health::cancel_owned_weapon_activations(registry, pawn);
        }
    }
}

pub fn register_life_state_reaction_primitives(
    registry: &mut ReactionPrimitiveRegistry,
    ctx: ScriptCtx,
) {
    registry.register("setLifeState", move |registry, targets, args| {
        if let Some(state) = args.get("state").and_then(serde_json::Value::as_str) {
            set_life_state(&ctx, registry, targets, state);
        }
        Ok(())
    });
}

pub fn register_sequenced_life_state_primitives(
    registry: &mut SequencedPrimitiveRegistry,
    ctx: ScriptCtx,
) {
    registry.register("setLifeState", move |pawn, args| {
        if let Some(state) = args.get("state").and_then(serde_json::Value::as_str) {
            set_life_state(&ctx, &mut ctx.registry.borrow_mut(), &[pawn], state);
        }
        Ok(())
    });
}

/// Remote participation may exist before its first movement command arrives.
/// Its life clock still advances once per host tick, never once per input step.
pub(crate) fn advance_unstepped_life_state_time(
    registry: &mut EntityRegistry,
    stepped: &[(EntityId, crate::movement::MovementEvents)],
    stepped_local: Option<EntityId>,
) {
    registry.for_each_with_kind_mut(ComponentKind::PlayerMovement, |id, component| {
        if Some(id) == stepped_local || stepped.iter().any(|(pawn, _)| *pawn == id) {
            return;
        }
        if let ComponentValue::PlayerMovement(movement) = component
            && movement.life_state != postretro_foundation::UNDECLARED_LIFE_STATE
        {
            movement.life_state_elapsed_ticks = movement.life_state_elapsed_ticks.saturating_add(1);
        }
    });
}

#[cfg(test)]
mod tests;
