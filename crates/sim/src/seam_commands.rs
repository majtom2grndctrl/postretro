//! Ordered host-only level-seam arm queue, consumed at authoritative tick start.
//! See: context/lib/entity_model.md §7e (Level Seams) · context/lib/scripting.md §12

use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, ScriptCtx, SeamComponent,
};
use postretro_scripting_core::group_resolution::group_commands_apply_here;
use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

pub(crate) fn enqueue_seam_mutations(
    ctx: &ScriptCtx,
    registry: &EntityRegistry,
    targets: &[EntityId],
    armed: bool,
) {
    if !group_commands_apply_here(ctx) {
        return;
    }
    let mut pending = ctx.pending_seam_mutations.borrow_mut();
    for &id in targets {
        if registry.get_component::<SeamComponent>(id).is_ok() {
            pending.push((id, armed));
        } else {
            log::warn!("[Seam] command target {id:?} is not a live level seam; skipping");
        }
    }
}

/// Apply once at authoritative tick start, in queue order. Pushes the seam
/// index of every false-to-true edge into `newly_armed`, so the trigger stage
/// can fire a touch seam for its standing occupants. Unlike a teleporter's
/// arm, it touches no latch: arming keeps latches. Host and single player
/// only; a connected client drops the queue.
pub fn apply_pending_seam_mutations(ctx: &ScriptCtx, newly_armed: &mut Vec<u16>) {
    let mut pending = ctx.pending_seam_mutations.borrow_mut();
    if !group_commands_apply_here(ctx) {
        pending.clear();
        return;
    }
    if pending.is_empty() {
        return;
    }
    let mut registry = ctx.registry.borrow_mut();
    for (entity, armed) in pending.drain(..) {
        match registry.get_component_value_mut(entity, ComponentKind::Seam) {
            Ok(ComponentValue::Seam(seam)) => {
                if armed && !seam.armed {
                    newly_armed.push(seam.index);
                }
                seam.armed = armed;
            }
            _ => log::warn!("[Seam] queued target {entity:?} no longer exists; skipping"),
        }
    }
}

pub fn register_seam_reaction_primitives(registry: &mut ReactionPrimitiveRegistry, ctx: ScriptCtx) {
    for (name, armed) in [("armSeam", true), ("disarmSeam", false)] {
        let ctx = ctx.clone();
        registry.register(name, move |registry, targets, _| {
            enqueue_seam_mutations(&ctx, registry, targets, armed);
            Ok(())
        });
    }
}

pub fn register_sequenced_seam_primitives(
    registry: &mut SequencedPrimitiveRegistry,
    ctx: ScriptCtx,
) {
    for (name, armed) in [("armSeam", true), ("disarmSeam", false)] {
        let ctx = ctx.clone();
        registry.register(name, move |id, _| {
            enqueue_seam_mutations(&ctx, &ctx.registry.borrow(), &[id], armed);
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests;
