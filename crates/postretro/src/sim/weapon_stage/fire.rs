use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::components::wieldable_state::WieldableState;

use crate::weapon::{WeaponFireAuthorization, WeaponFireCommand};
use postretro_entities::EntityId;

use super::super::{ReloadDelivery, ReloadOutcome};
use super::state::{StateTransition, WieldableStateEvent, transition_wieldable_state};

pub(super) struct FireAuthorizationContext<'a> {
    pub(super) tick_dt: f32,
    pub(super) reload_started_this_tick: bool,
    pub(super) feedback_tick: u64,
    pub(super) pawn: Option<EntityId>,
    pub(super) weapon: EntityId,
    pub(super) deliveries: &'a mut Vec<ReloadDelivery>,
}

pub(super) fn authorize_fire(
    weapon: &mut WeaponComponent,
    command: &WeaponFireCommand,
    context: FireAuthorizationContext<'_>,
) -> WeaponFireAuthorization {
    let dt_ms = (context.tick_dt.max(0.0)) * 1000.0;
    weapon.cooldown_remaining_ms = (weapon.cooldown_remaining_ms - dt_ms).max(0.0);

    let verdict = weapon_fire_authorization_verdict(weapon, command);
    if weapon.state == WieldableState::ShellLoading
        && verdict == WeaponFireAuthorization::Accepted
        && !context.reload_started_this_tick
        && let StateTransition::ReloadCancelled { transferred } = transition_wieldable_state(
            weapon,
            WieldableStateEvent::Cancel {
                feedback_tick: context.feedback_tick,
            },
            None,
        )
        && let Some(pawn) = context.pawn
    {
        context.deliveries.push(ReloadDelivery {
            pawn,
            weapon: context.weapon,
            outcome: ReloadOutcome::Cancelled { transferred },
        });
    }

    let fire_mode = weapon.effective().fire_mode;
    if fire_mode == postretro_entities::data_descriptors::FireMode::Semi && command.button.pressed {
        weapon.shoot_press_consumed = true;
    } else if !command.button.active {
        weapon.shoot_press_consumed = false;
    }

    if context.reload_started_this_tick || !weapon.state.allows_fire() {
        return WeaponFireAuthorization::Rejected;
    }

    match verdict {
        WeaponFireAuthorization::Rejected => WeaponFireAuthorization::Rejected,
        WeaponFireAuthorization::Empty => {
            weapon.cooldown_remaining_ms = weapon.effective().cooldown_ms;
            WeaponFireAuthorization::Empty
        }
        WeaponFireAuthorization::Accepted => {
            let (cooldown_ms, cost_per_shot) = {
                let stats = weapon.effective();
                (
                    stats.cooldown_ms,
                    stats.ammo.as_ref().map(|ammo| ammo.cost_per_shot),
                )
            };
            if let Some(cost_per_shot) = cost_per_shot {
                weapon.magazine -= cost_per_shot;
            }
            weapon.cooldown_remaining_ms = cooldown_ms;
            WeaponFireAuthorization::Accepted
        }
    }
}

/// Answer the fire question as if the weapon were Idle. This is deliberately
/// state- and reload-latch-blind so a ShellLoading loop can decide whether to
/// cancel before the real gate applies state legality.
pub(super) fn weapon_fire_authorization_verdict(
    weapon: &WeaponComponent,
    command: &WeaponFireCommand,
) -> WeaponFireAuthorization {
    let stats = weapon.effective();
    let wants_fire = match stats.fire_mode {
        postretro_entities::data_descriptors::FireMode::Semi => {
            command.button.pressed && !weapon.shoot_press_consumed
        }
        postretro_entities::data_descriptors::FireMode::Auto => command.button.active,
    };
    if !command.can_fire || !wants_fire || weapon.cooldown_remaining_ms > 0.0 {
        return WeaponFireAuthorization::Rejected;
    }
    if let Some(cost_per_shot) = stats.ammo.as_ref().map(|ammo| ammo.cost_per_shot) {
        if weapon.magazine < cost_per_shot {
            return WeaponFireAuthorization::Empty;
        }
    }
    WeaponFireAuthorization::Accepted
}
