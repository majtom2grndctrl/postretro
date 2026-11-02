//! Authoritative respawn enqueueing shared by bound and deferred reactions.
//! See: context/lib/scripting.md §12 (Player events).

use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{EntityId, EntityRegistry, RespawnRequest, ScriptCtx};
use postretro_foundation::PlayerMovementComponent;
use postretro_scripting_core::group_resolution::{group_commands_apply_here, is_player_pawn};
use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

pub(crate) fn enqueue_respawns(
    ctx: &ScriptCtx,
    registry: &EntityRegistry,
    targets: &[EntityId],
    health: Option<f32>,
    state: Option<&str>,
) {
    if !group_commands_apply_here(ctx) {
        return;
    }
    let data = ctx.data_registry.borrow();
    let mut pending = ctx.pending_respawns.borrow_mut();
    for &pawn in targets {
        if !is_player_pawn(registry, pawn)
            || registry
                .get_component::<PlayerMovementComponent>(pawn)
                .is_err()
        {
            log::warn!("[Respawn] target {pawn:?} is not a live player pawn; skipping");
            continue;
        }
        if let Some(state) = state {
            let declares_state = registry
                .get_component::<DescriptorProvenance>(pawn)
                .ok()
                .and_then(|provenance| {
                    data.entities.iter().find(|descriptor| {
                        descriptor.canonical_name.as_deref()
                            == Some(provenance.canonical_name.as_str())
                    })
                })
                .and_then(|descriptor| descriptor.life_states.as_ref())
                .is_some_and(|block| block.state(state).is_some());
            if !declares_state {
                log::warn!("[Respawn] target {pawn:?} does not declare state `{state}`; skipping");
                continue;
            }
        }
        // Keep the name until drain: a frame-tail reload may reassign table ids.
        pending.insert(
            pawn,
            RespawnRequest {
                pawn,
                health,
                state: state.map(str::to_owned),
            },
        );
    }
}

/// Payload validation checks authored types. Preserve an overflow (or malformed
/// runtime value) as invalid health so the drain warns and normalizes to max.
pub(crate) fn respawn_health(args: &serde_json::Value) -> Option<f32> {
    args.get("health")
        .map(|value| value.as_f64().unwrap_or(f64::NAN) as f32)
}

pub fn register_respawn_reaction_primitives(
    registry: &mut ReactionPrimitiveRegistry,
    ctx: ScriptCtx,
) {
    registry.register("respawn", move |registry, targets, args| {
        enqueue_respawns(
            &ctx,
            registry,
            targets,
            respawn_health(args),
            args.get("state").and_then(serde_json::Value::as_str),
        );
        Ok(())
    });
}

pub fn register_sequenced_respawn_primitives(
    registry: &mut SequencedPrimitiveRegistry,
    ctx: ScriptCtx,
) {
    registry.register("respawn", move |pawn, args| {
        enqueue_respawns(
            &ctx,
            &ctx.registry.borrow(),
            &[pawn],
            respawn_health(args),
            args.get("state").and_then(serde_json::Value::as_str),
        );
        Ok(())
    });
}

#[cfg(test)]
mod tests;
