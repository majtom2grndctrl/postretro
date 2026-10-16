//! Ordered host-only teleporter mutation queue, consumed before movement.
//! See: context/lib/scripting.md §12 (Teleport events)

use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, ScriptCtx, TeleporterComponent,
};
use postretro_scripting_core::group_resolution::group_commands_apply_here;
use postretro_scripting_core::reaction_registry::ReactionPrimitiveRegistry;
use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;

pub(crate) fn enqueue_teleporter_mutations(
    ctx: &ScriptCtx,
    registry: &EntityRegistry,
    targets: &[EntityId],
    armed: bool,
) {
    if !group_commands_apply_here(ctx) {
        return;
    }
    let mut pending = ctx.pending_teleporter_mutations.borrow_mut();
    for &id in targets {
        if registry.get_component::<TeleporterComponent>(id).is_ok() {
            pending.push((id, armed));
        } else {
            log::warn!("[Teleport] command target {id:?} is not a live teleporter; skipping");
        }
    }
}

/// Apply once at authoritative tick start. Every pawn then reads the same state.
/// Retains queue capacity and visits pawn latches only on a false-to-true edge.
pub fn apply_pending_teleporter_mutations(ctx: &ScriptCtx) {
    let mut pending = ctx.pending_teleporter_mutations.borrow_mut();
    if !group_commands_apply_here(ctx) {
        pending.clear();
        return;
    }
    if pending.is_empty() {
        return;
    }
    let mut registry = ctx.registry.borrow_mut();
    for (entity, armed) in pending.drain(..) {
        let index = match registry.get_component_value_mut(entity, ComponentKind::Teleporter) {
            Ok(ComponentValue::Teleporter(teleporter)) => {
                let newly_armed = armed && !teleporter.armed;
                teleporter.armed = armed;
                newly_armed.then_some(teleporter.index)
            }
            _ => {
                log::warn!("[Teleport] queued target {entity:?} no longer exists; skipping");
                None
            }
        };
        if let Some(index) = index {
            registry.for_each_with_kind_mut(ComponentKind::PlayerMovement, |_, value| {
                if let ComponentValue::PlayerMovement(pawn) = value {
                    pawn.teleport
                        .arrival_latches
                        .retain(|latched| *latched != index);
                }
            });
        }
    }
}

pub fn register_teleporter_reaction_primitives(
    registry: &mut ReactionPrimitiveRegistry,
    ctx: ScriptCtx,
) {
    for (name, armed) in [("armTeleporter", true), ("disarmTeleporter", false)] {
        let ctx = ctx.clone();
        registry.register(name, move |registry, targets, _| {
            enqueue_teleporter_mutations(&ctx, registry, targets, armed);
            Ok(())
        });
    }
}

pub fn register_sequenced_teleporter_primitives(
    registry: &mut SequencedPrimitiveRegistry,
    ctx: ScriptCtx,
) {
    for (name, armed) in [("armTeleporter", true), ("disarmTeleporter", false)] {
        let ctx = ctx.clone();
        registry.register(name, move |id, _| {
            enqueue_teleporter_mutations(&ctx, &ctx.registry.borrow(), &[id], armed);
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::Transform;
    fn teleporter(ctx: &ScriptCtx, armed: bool) -> EntityId {
        let mut registry = ctx.registry.borrow_mut();
        let id = registry.spawn(Transform::default());
        registry
            .set_component(
                id,
                TeleporterComponent {
                    index: 0,
                    source_entity: 1,
                    armed,
                    arrival_sound: String::new(),
                    on_teleport: String::new(),
                },
            )
            .unwrap();
        id
    }
    #[test]
    fn teleport_member_commands_apply_at_next_tick_in_order_and_clients_ignore_them() {
        let ctx = ScriptCtx::new();
        let id = teleporter(&ctx, false);
        let mut steps = SequencedPrimitiveRegistry::new();
        register_sequenced_teleporter_primitives(&mut steps, ctx.clone());
        for name in ["armTeleporter", "disarmTeleporter"] {
            steps.get(name).unwrap()(id, &serde_json::json!({})).unwrap();
        }
        assert!(
            !ctx.registry
                .borrow()
                .get_component::<TeleporterComponent>(id)
                .unwrap()
                .armed
        );
        assert_eq!(ctx.pending_teleporter_mutations.borrow().len(), 2);
        apply_pending_teleporter_mutations(&ctx);
        assert!(
            !ctx.registry
                .borrow()
                .get_component::<TeleporterComponent>(id)
                .unwrap()
                .armed
        );
        steps.get("armTeleporter").unwrap()(id, &serde_json::json!({})).unwrap();
        apply_pending_teleporter_mutations(&ctx);
        assert!(
            ctx.registry
                .borrow()
                .get_component::<TeleporterComponent>(id)
                .unwrap()
                .armed
        );
        ctx.owner_slot_writes_enabled.set(false);
        steps.get("disarmTeleporter").unwrap()(id, &serde_json::json!({})).unwrap();
        assert!(ctx.pending_teleporter_mutations.borrow().is_empty());
        apply_pending_teleporter_mutations(&ctx);
        assert!(
            ctx.registry
                .borrow()
                .get_component::<TeleporterComponent>(id)
                .unwrap()
                .armed
        );
    }
}

#[cfg(test)]
mod entry_path_tests;
