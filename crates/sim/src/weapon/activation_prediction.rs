// See: context/lib/entity_model.md §4, §5 · context/lib/networking.md
//! Stateless adapters over the central weapon state, used by client fixed ticks.
use super::execution::{
    ActivationCommand, WeaponActivationAdvance, advance_weapon_activation_with_shell_preemption,
};
use postretro_combat_model::activation::{ActivationAdvance, advance_activation, start_activation};
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::components::wieldable_state::WieldableState;
use postretro_foundation::{ActivationInput, ActivationPhase, ActivationProgram, ActivationToken};

/// Fixture adapter retains no cursor; central state is always the owner.
#[derive(Debug, Default)]
pub struct ClientActivationTiming;
impl ClientActivationTiming {
    pub fn start(
        &mut self,
        state: &mut WieldableState,
        token: ActivationToken,
        pawn: u32,
        tick: u32,
        program: &ActivationProgram,
    ) -> bool {
        if !state.allows_fire() {
            return false;
        }
        let cursor = start_activation(token, pawn, tick, program);
        *state = match cursor.phase {
            ActivationPhase::Charging => WieldableState::Charging(cursor),
            _ => WieldableState::Executing(cursor),
        };
        true
    }
    pub fn tick(
        &mut self,
        state: &mut WieldableState,
        program: &ActivationProgram,
        tick: u32,
        input: ActivationInput,
    ) -> ActivationAdvance {
        let Some(mut cursor) = state.activation_cursor() else {
            return ActivationAdvance::default();
        };
        let result = advance_activation(&mut cursor, program, tick, true, input);
        *state = match cursor.phase {
            ActivationPhase::Charging => WieldableState::Charging(cursor),
            ActivationPhase::Executing => WieldableState::Executing(cursor),
            ActivationPhase::Terminal => WieldableState::Idle,
        };
        result
    }
    pub fn cancel(&mut self, state: &mut WieldableState, token: ActivationToken) {
        if state
            .activation_cursor()
            .is_some_and(|cursor| cursor.token == token)
        {
            *state = WieldableState::Idle;
        }
    }
}
/// Every returned shot is frozen immediately by the fixed-tick producer, then
/// resolved at rendered aim. Render-only frames never call this adapter.
pub fn advance_predicted_weapon_tick(
    weapon: &mut WeaponComponent,
    command: ActivationCommand,
    reload: bool,
    dt_ms: f32,
    allow_start: bool,
) -> WeaponActivationAdvance {
    let fresh_reload = reload && !weapon.reload_press_consumed;
    weapon.reload_press_consumed = reload;
    let cancelled = fresh_reload.then(|| weapon.cancel_activation()).flatten();
    let mut result = advance_weapon_activation_with_shell_preemption(
        weapon,
        command,
        dt_ms,
        true,
        allow_start && !fresh_reload,
        true,
        true,
        |weapon| {
            crate::sim::weapon_stage::cancel_predicted_shell_reload(weapon);
        },
    );
    if let Some(token) = cancelled {
        result.terminal = Some((
            token,
            postretro_combat_model::activation::ActivationTermination::Cancelled,
        ));
    }
    result
}
