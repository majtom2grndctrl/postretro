// Life-state predicate binding shared by player-event and local store scopes,
// and the impact-source scope (`EntityScope`, for `source.inLifeState`).
// See: context/lib/scripting.md §12 (Player events)

use crate::ctx::ScriptCtx;
use crate::registry::{EntityId, EntityRegistry};

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

/// Membership in the mod-wide name table, unlogged: the caller owns the error
/// that names the unknown state (an impact policy's `source.setLifeState`).
pub(crate) fn life_state_name_declared(ctx: &ScriptCtx, name: &str) -> bool {
    ctx.data_registry
        .borrow()
        .life_state_names
        .iter()
        .any(|state| state == name)
}

/// The life-state id a player pawn holds right now. A non-player entity (an
/// NPC damager) has none, so every `inLifeState` read on it is false.
pub(crate) fn player_life_state(registry: &EntityRegistry, pawn: EntityId) -> Option<u8> {
    registry
        .get_component::<postretro_foundation::PlayerMovementComponent>(pawn)
        .ok()
        .map(|movement| movement.life_state)
}

#[cfg(test)]
#[path = "life_state_inputs_tests.rs"]
mod tests;
