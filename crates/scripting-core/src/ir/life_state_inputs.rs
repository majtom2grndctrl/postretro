// Life-state predicate binding shared by player-event and local store scopes.
// See: context/lib/scripting.md §12 (Player events)

use crate::ctx::ScriptCtx;

pub(crate) const LIFE_STATE_INPUT_PREFIX: &str = "player.inLifeState:";

/// Resolve at bind time, before any per-tick evaluation. The table is shared
/// by every descriptor, so a known name is false on a pawn that lacks it.
pub(crate) fn resolve_life_state_id(ctx: &ScriptCtx, name: &str) -> Option<u8> {
    let registry = ctx.data_registry.borrow();
    let Some(index) = registry
        .life_state_names
        .iter()
        .position(|state| state == name)
    else {
        log::error!("[Scripting] cannot bind inLifeState: unknown life state `{name}`");
        return None;
    };
    u8::try_from(index)
        .ok()
        .filter(|id| *id != postretro_foundation::UNDECLARED_LIFE_STATE)
}

#[cfg(test)]
#[path = "life_state_inputs_tests.rs"]
mod tests;
