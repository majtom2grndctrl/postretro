use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{AmmoReserve, EntityId, EntityRegistry};

use crate::weapon::{WeaponFireAuthorization, WeaponFireCommand};

use super::super::{ReloadDelivery, ReloadOutcome};
use super::fire::{FireAuthorizationContext, authorize_fire};
use super::state::{
    StateTransition, WieldableStateEvent, resolve_expired_state, transition_wieldable_state,
};

pub(super) struct WeaponMachineTick {
    pub(super) authorization: WeaponFireAuthorization,
    pub(super) deliveries: Vec<ReloadDelivery>,
    pub(super) lowered: bool,
}
/// Run the one ordered weapon machine shared by local and host-simulated pawns.
/// Reload entry must run before expiry and fire: a reload started this tick owns
/// the fire gate even when a short duration completes before that gate runs.
#[allow(clippy::too_many_arguments)]
pub(super) fn tick_weapon_machine(
    registry: &mut EntityRegistry,
    pawn: Option<EntityId>,
    weapon: EntityId,
    component: &mut WeaponComponent,
    reload: bool,
    command: &WeaponFireCommand,
    suppress_fire: bool,
    tick_dt: f32,
) -> WeaponMachineTick {
    let pawn = pawn.filter(|pawn| registry.exists(*pawn));
    let feedback_tick = component.begin_reload_feedback_tick();
    let mut deliveries = Vec::new();
    let mut reload_started_this_tick = false;

    // 1. Reload intent. Pawnless ticks intentionally leave the edge untouched:
    // a held level becomes a real rising edge once its pawn returns.
    if let Some(pawn) = pawn {
        let fresh_press = reload && !component.reload_press_consumed;
        component.reload_press_consumed = reload;
        if fresh_press && component.state.allows_reload() {
            if let Some((capacity, ammo_type, reload_ms, reload_style)) =
                component.effective().ammo.map(|ammo| {
                    (
                        ammo.capacity,
                        ammo.ammo_type.to_string(),
                        ammo.reload_ms,
                        ammo.reload_style,
                    )
                })
            {
                if component.magazine >= capacity {
                    deliveries.push(ReloadDelivery {
                        pawn,
                        weapon,
                        outcome: ReloadOutcome::BlockedFull,
                    });
                } else if registry
                    .get_component::<AmmoReserve>(pawn)
                    .map_or(0, |reserve| reserve.available(&ammo_type))
                    == 0
                {
                    deliveries.push(ReloadDelivery {
                        pawn,
                        weapon,
                        outcome: ReloadOutcome::BlockedEmpty,
                    });
                } else if transition_wieldable_state(
                    component,
                    WieldableStateEvent::BeginReload {
                        duration_ms: reload_ms,
                        reload_style,
                        feedback_tick,
                    },
                    None,
                ) == StateTransition::ReloadStarted
                {
                    reload_started_this_tick = true;
                    deliveries.push(ReloadDelivery {
                        pawn,
                        weapon,
                        outcome: ReloadOutcome::Started,
                    });
                }
            }
        }
    }

    // 2. Timer advance. The shared helper owns the sub-millisecond carry.
    let mut lowered = false;
    if component.state.is_timed_state()
        && let Some(overshoot_ms) = super::super::reload::advance_timer(component, tick_dt)
    {
        // 3. State expiry. Its meaning is selected by the state/event transition,
        // never by a second state match in the machine.
        lowered = resolve_expired_state(
            registry,
            pawn,
            weapon,
            component,
            overshoot_ms,
            feedback_tick,
            &mut deliveries,
        );
    }

    // 4. Fire intent. Cooldown remains orthogonal to wieldable state.
    let fire_command = WeaponFireCommand {
        can_fire: command.can_fire && !suppress_fire,
        ..*command
    };
    let authorization = authorize_fire(
        component,
        &fire_command,
        FireAuthorizationContext {
            tick_dt,
            reload_started_this_tick,
            feedback_tick,
            pawn,
            weapon,
            deliveries: &mut deliveries,
        },
    );
    WeaponMachineTick {
        authorization,
        deliveries,
        lowered,
    }
}

#[cfg(test)]
pub(super) fn deliver_reload_to_weapon(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    weapon: EntityId,
    reload_pressed: bool,
    tick_dt: f32,
) -> Vec<ReloadDelivery> {
    let Ok(mut component) = registry.get_component::<WeaponComponent>(weapon).cloned() else {
        return Vec::new();
    };
    let command = WeaponFireCommand {
        button: crate::weapon::FireButtonState {
            pressed: false,
            active: false,
        },
        aim_origin: glam::Vec3::ZERO,
        aim_direction: glam::Vec3::Z,
        can_fire: false,
    };
    let machine = tick_weapon_machine(
        registry,
        Some(pawn),
        weapon,
        &mut component,
        reload_pressed,
        &command,
        false,
        tick_dt,
    );
    let _ = registry.set_component(weapon, component);
    machine.deliveries
}
