// Binding for an impact policy's `source.setLifeState(name)`.
// See: context/lib/scripting.md §2 (Impact-policy evaluation), §12 (Player life states)

use postretro_entities::components::health::IMPACT_SOURCE_TOKEN;
use postretro_scripting_core::ir_scopes::EntityScope;
use serde_json::{Map, Value};

use super::{BoundEffect, require_impact_token, required_string};

/// Only the damager is addressable: a life state belongs to a player pawn, and
/// the impact target is not one by construction. An unknown name fails the
/// whole policy here, so `rebuild`'s skip warning names the policy and state.
pub(super) fn bind_set_life_state(
    target: Option<&str>,
    args: &Map<String, Value>,
    scope: &EntityScope,
) -> Result<BoundEffect, String> {
    require_impact_token(target, "setLifeState", IMPACT_SOURCE_TOKEN)?;
    let state = required_string(args, "state", "setLifeState args")?;
    if !scope.declares_life_state(state) {
        return Err(format!("setLifeState names unknown life state `{state}`"));
    }
    Ok(BoundEffect::SetLifeState {
        state: state.to_string(),
    })
}
