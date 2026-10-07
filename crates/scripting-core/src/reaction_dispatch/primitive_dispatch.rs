// Primitive-descriptor dispatch: tag target resolution, owner-slot grants, and
// system reactions.
// See: context/lib/scripting.md §12 (Reaction Dispatch Model)

use serde::Deserialize;

use crate::ctx::ScriptCtx;
use crate::data_descriptors::PrimitiveDescriptor;
use crate::reaction_registry::{
    ReactionPrimitiveRegistry, SystemReactionCommand, SystemReactionRegistry,
};
use crate::registry::{ComponentKind, EntityId};
use crate::slot_table::SlotType;

/// Routes a `Primitive` descriptor to one of two execution arms (M13 HUD
/// dynamics): a `Some(tag)` resolves entities and runs the entity-targeted
/// `ReactionPrimitiveRegistry`; a `None` tag is a system reaction, dispatched
/// against the `SystemReactionRegistry`, which enqueues a typed command onto
/// `ScriptCtx::system_commands` for the app's per-frame drain. Both arms share
/// the one named-event vocabulary.
pub(super) fn dispatch_primitive(
    descriptor: &PrimitiveDescriptor,
    reaction_registry: &ReactionPrimitiveRegistry,
    system_registry: &SystemReactionRegistry,
    script_ctx: &ScriptCtx,
) {
    // Connected clients compose the same descriptor graph for presentation
    // work, but must not make authoritative per-owner state mutations or emit
    // diagnostics for host-only gameplay events.
    if descriptor.primitive == "addSlot" && !script_ctx.owner_slot_writes_enabled.get() {
        return;
    }

    let Some(tag) = descriptor.tag.as_deref() else {
        if descriptor.primitive == "addSlot" {
            log::warn!("[Scripting] addSlot requires a target tag; reaction had no effect");
            return;
        }
        dispatch_system_primitive(descriptor, system_registry, script_ctx);
        return;
    };

    // Targeting walks the Transform column per the invariant in
    // `count_entities_with_tag`. Empty target sets are passed through; handlers
    // decide whether to warn.
    let targets: Vec<EntityId> = {
        let reg = script_ctx.registry.borrow();
        reg.query_by_component_and_tag(ComponentKind::Transform, Some(tag))
            .map(|(id, _)| id)
            .collect()
    };

    if descriptor.primitive == "addSlot" {
        dispatch_add_owner_slot(descriptor, &targets, script_ctx);
        return;
    }

    log::info!(
        "[Scripting] dispatch primitive '{}' on tag '{}' ({} targets)",
        descriptor.primitive,
        tag,
        targets.len(),
    );

    let mut reg = script_ctx.registry.borrow_mut();
    match reaction_registry.dispatch_tagged(
        &descriptor.primitive,
        &mut reg,
        tag,
        &targets,
        &descriptor.args,
    ) {
        Ok(true) => {}
        Ok(false) => log::warn!(
            "[Scripting] primitive '{}' is not registered; reaction had no effect",
            descriptor.primitive,
        ),
        Err(e) => log::warn!(
            "[Scripting] primitive '{}' dispatch failed: {e:?}",
            descriptor.primitive,
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddSlotArgs {
    slot: String,
    delta: f32,
}

/// Resolve the tagged pawn recipients while a named/crossing/level-load
/// reaction fires, then defer the actual addition to the host app drain. This
/// keeps reaction dispatch independent from the session seat ledger while the
/// drain remains responsible for skipping seats released in the meantime.
fn dispatch_add_owner_slot(
    descriptor: &PrimitiveDescriptor,
    targets: &[EntityId],
    script_ctx: &ScriptCtx,
) {
    let args: AddSlotArgs = match serde_json::from_value::<AddSlotArgs>(descriptor.args.clone()) {
        Ok(args) if args.delta.is_finite() => args,
        Ok(_) => {
            log::warn!("[Scripting] addSlot delta must be finite; reaction had no effect");
            return;
        }
        Err(error) => {
            log::warn!("[Scripting] addSlot has invalid args; reaction had no effect: {error}");
            return;
        }
    };

    {
        let slot_table = script_ctx.slot_table.borrow();
        let Some(record) = slot_table.get(&args.slot) else {
            log::warn!(
                "[Scripting] addSlot references unknown slot `{}`; reaction had no effect",
                args.slot
            );
            return;
        };
        if !record.schema.per_owner {
            log::warn!(
                "[Scripting] addSlot requires per-owner slot `{}`; reaction had no effect",
                args.slot
            );
            return;
        }
        if record.schema.slot_type != SlotType::Number {
            log::warn!(
                "[Scripting] addSlot requires numeric slot `{}`; reaction had no effect",
                args.slot
            );
            return;
        }
        if record.schema.readonly {
            log::warn!(
                "[Scripting] addSlot rejects readonly slot `{}`; reaction had no effect",
                args.slot
            );
            return;
        }
    }

    // A valid descriptor with no matching pawns is a normal no-op. Validate
    // first so malformed named level-load and crossing descriptors cannot
    // survive merely because their current level has no recipients.
    if targets.is_empty() {
        return;
    }

    let seats: Vec<_> = {
        let registry = script_ctx.registry.borrow();
        targets
            .iter()
            .filter_map(|target| match registry.seat_for_pawn(*target) {
                Some(seat) => Some(seat),
                None => {
                    log::warn!(
                        "[Scripting] addSlot target {target:?} has no player seat; skipping"
                    );
                    None
                }
            })
            .collect()
    };
    if seats.is_empty() {
        return;
    }

    script_ctx
        .system_commands
        .push(SystemReactionCommand::AddOwnerSlot {
            slot: args.slot,
            seats,
            delta: args.delta,
        });
}

/// System-reaction arm: no entity targets. The handler parses `args` and
/// enqueues a typed command; the app drains the queue once per frame.
fn dispatch_system_primitive(
    descriptor: &PrimitiveDescriptor,
    system_registry: &SystemReactionRegistry,
    script_ctx: &ScriptCtx,
) {
    log::info!(
        "[Scripting] dispatch system reaction '{}'",
        descriptor.primitive,
    );

    match system_registry.dispatch(
        &descriptor.primitive,
        &descriptor.args,
        &script_ctx.system_commands,
    ) {
        Ok(true) => {}
        Ok(false) => log::warn!(
            "[Scripting] system reaction '{}' is not registered; reaction had no effect",
            descriptor.primitive,
        ),
        Err(e) => log::warn!(
            "[Scripting] system reaction '{}' dispatch failed: {e:?}",
            descriptor.primitive,
        ),
    }
}
