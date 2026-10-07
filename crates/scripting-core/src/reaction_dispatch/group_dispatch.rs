// Group-command dispatch for named fires and sequence steps: role gate, kind
// resolution, and handoff to the entity-targeted primitive handler.
// See: context/lib/scripting.md §12 (Entity addressing)

use crate::ctx::ScriptCtx;
use crate::data_descriptors::GroupTarget;
use crate::group_resolution::resolve_group;
use crate::reaction_registry::ReactionPrimitiveRegistry;

use super::primitive_dispatch::dispatch_add_owner_slot;

/// Run `primitive` over every live member of `target` on this machine.
///
/// Group commands are host and single-player only. Every machine runs the same
/// reactions, so a group command reaching a connected client is normal content:
/// it applies nothing and logs at debug, the same silence a client `addSlot`
/// keeps. Member and raw tag steps beside it are unaffected.
pub(super) fn dispatch_group(
    primitive: &str,
    target: &GroupTarget,
    args: &serde_json::Value,
    reaction_registry: &ReactionPrimitiveRegistry,
    script_ctx: &ScriptCtx,
) {
    let kind = target.kind.as_wire();
    if !script_ctx.owner_slot_writes_enabled.get() {
        log::debug!(
            "[Scripting] group command '{primitive}' on kind '{kind}' is host-only; skipped on this client"
        );
        return;
    }

    let targets = resolve_group(&script_ctx.registry.borrow(), target);

    if primitive == "addSlot" {
        dispatch_add_owner_slot(args, &targets, script_ctx);
        return;
    }
    if !reaction_registry.contains(primitive) {
        log::warn!(
            "[Scripting] primitive '{primitive}' is not registered; group command on kind '{kind}' had no effect"
        );
        return;
    }
    if targets.is_empty() {
        log::debug!(
            "[Scripting] group command '{primitive}' on kind '{kind}' (tag {:?}) matched nothing; no-op",
            target.tag,
        );
        return;
    }

    log::debug!(
        "[Scripting] dispatch group command '{primitive}' on kind '{kind}' (tag {:?}, {} targets)",
        target.tag,
        targets.len(),
    );
    let mut registry = script_ctx.registry.borrow_mut();
    if let Err(error) = reaction_registry.dispatch_tagged(
        primitive,
        &mut registry,
        target.tag.as_deref().unwrap_or(""),
        &targets,
        args,
    ) {
        log::warn!("[Scripting] group command '{primitive}' on kind '{kind}' failed: {error:?}");
    }
}
