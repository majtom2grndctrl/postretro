use postretro_entities::components::weapon::{ReloadFeedback, WeaponComponent};
use postretro_entities::components::wieldable_state::WieldableState;
use postretro_entities::data_descriptors::ReloadStyle;
use postretro_entities::{AmmoReserve, ComponentKind, EntityId, EntityRegistry};

use super::super::{ReloadDelivery, ReloadOutcome};

pub(super) enum WieldableStateEvent {
    BeginReload {
        duration_ms: u32,
        reload_style: ReloadStyle,
        feedback_tick: u64,
    },
    Expired {
        pawn_present: bool,
        feedback_tick: u64,
    },
    Cancel {
        feedback_tick: u64,
    },
    BeginLower {
        duration_ms: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StateTransition {
    Noop,
    ReloadStarted,
    ReloadStep {
        shell_loaded: bool,
        completed: Option<u32>,
        restart_duration_ms: Option<u32>,
    },
    ReloadCancelled {
        transferred: u32,
    },
    Lowered,
}

/// The sole state/event dispatch. New wieldable states add rows here rather than
/// changing component shape or teaching callers what a timer expiry means.
pub(super) fn transition_wieldable_state(
    component: &mut WeaponComponent,
    event: WieldableStateEvent,
    reserve: Option<&mut AmmoReserve>,
) -> StateTransition {
    match (component.state, event) {
        (
            WieldableState::Idle,
            WieldableStateEvent::BeginReload {
                duration_ms,
                reload_style,
                feedback_tick,
            },
        ) => {
            component.state = match reload_style {
                ReloadStyle::Magazine => WieldableState::Reloading,
                ReloadStyle::PerShell => WieldableState::ShellLoading,
            };
            component.state_total_ms = duration_ms;
            component.state_remaining_ms = duration_ms;
            component.state_elapsed_sub_ms = 0.0;
            component.reload_credited = 0;
            component.publish_reload_feedback(ReloadFeedback::Started, feedback_tick);
            StateTransition::ReloadStarted
        }
        (WieldableState::Idle, WieldableStateEvent::Expired { .. })
        | (WieldableState::Idle, WieldableStateEvent::Cancel { .. }) => StateTransition::Noop,
        (WieldableState::Idle, WieldableStateEvent::BeginLower { duration_ms }) => {
            begin_lowering(component, duration_ms);
            StateTransition::Noop
        }
        (WieldableState::Reloading, WieldableStateEvent::BeginReload { .. }) => {
            StateTransition::Noop
        }
        (
            WieldableState::Reloading,
            WieldableStateEvent::Expired {
                pawn_present,
                feedback_tick,
            },
        ) => {
            if !pawn_present {
                transition_to_idle(component);
                return StateTransition::Noop;
            }

            // Completion honors refreshed ammo tuning, while preserving the live
            // timed state until this decision point.
            let effective_ammo = component
                .effective()
                .ammo
                .map(|ammo| (ammo.capacity, ammo.ammo_type.to_string()));
            let transferred = if let Some((capacity, ammo_type)) = effective_ammo {
                let requested = capacity.saturating_sub(component.magazine).min(
                    reserve
                        .as_ref()
                        .map_or(0, |reserve| reserve.available(&ammo_type)),
                );
                let transferred = reserve.map_or(0, |reserve| reserve.take(&ammo_type, requested));
                component.magazine = component.magazine.saturating_add(transferred);
                transferred
            } else {
                0
            };
            component.reload_credited = component.reload_credited.saturating_add(transferred);
            complete_reload(component, false, feedback_tick)
        }
        (WieldableState::ShellLoading, WieldableStateEvent::BeginReload { .. }) => {
            StateTransition::Noop
        }
        (
            WieldableState::ShellLoading,
            WieldableStateEvent::Expired {
                pawn_present,
                feedback_tick,
            },
        ) => {
            if !pawn_present {
                transition_to_idle(component);
                return StateTransition::Noop;
            }

            let Some((capacity, ammo_type, reload_ms, reload_style)) =
                component.effective().ammo.map(|ammo| {
                    (
                        ammo.capacity,
                        ammo.ammo_type.to_string(),
                        ammo.reload_ms,
                        ammo.reload_style,
                    )
                })
            else {
                return complete_reload(component, false, feedback_tick);
            };

            // The continue guard reads this same one-tick working copy as the
            // debit below. A zero result is unreachable during ordinary play but
            // protects a reserve that was emptied between guard and credit.
            let Some(reserve) = reserve else {
                return complete_reload(component, false, feedback_tick);
            };
            let transferred = reserve.take(&ammo_type, 1);
            if transferred == 0 {
                return complete_reload(component, false, feedback_tick);
            }

            component.magazine = component.magazine.saturating_add(transferred);
            component.reload_credited = component.reload_credited.saturating_add(transferred);
            let continues = component.magazine < capacity
                && reserve.available(&ammo_type) > 0
                && reload_style == ReloadStyle::PerShell;
            if continues {
                // A per-shell boundary completes the current meter step even
                // though the loop immediately starts the next one.
                component.publish_reload_feedback(ReloadFeedback::Completed, feedback_tick);
                StateTransition::ReloadStep {
                    shell_loaded: true,
                    completed: None,
                    restart_duration_ms: Some(reload_ms),
                }
            } else {
                complete_reload(component, true, feedback_tick)
            }
        }
        (WieldableState::ShellLoading, WieldableStateEvent::Cancel { feedback_tick }) => {
            let transferred = component.reload_credited;
            transition_to_idle(component);
            component.reload_press_consumed = false;
            component.clear_cancelled_reload_feedback(feedback_tick);
            StateTransition::ReloadCancelled { transferred }
        }
        (WieldableState::Reloading, WieldableStateEvent::Cancel { .. }) => StateTransition::Noop,
        (WieldableState::Reloading, WieldableStateEvent::BeginLower { duration_ms }) => {
            begin_lowering(component, duration_ms);
            StateTransition::Noop
        }
        (WieldableState::ShellLoading, WieldableStateEvent::BeginLower { duration_ms }) => {
            begin_lowering(component, duration_ms);
            StateTransition::Noop
        }
        (WieldableState::Lowering, WieldableStateEvent::BeginReload { .. }) => {
            StateTransition::Noop
        }
        (WieldableState::Lowering, WieldableStateEvent::Expired { .. }) => StateTransition::Lowered,
        (WieldableState::Lowering, WieldableStateEvent::Cancel { .. }) => StateTransition::Noop,
        (WieldableState::Lowering, WieldableStateEvent::BeginLower { .. }) => {
            component.clear_equip_reload_feedback();
            StateTransition::Noop
        }
        (WieldableState::Raising, WieldableStateEvent::BeginReload { .. }) => StateTransition::Noop,
        (WieldableState::Raising, WieldableStateEvent::Expired { .. }) => {
            transition_to_idle(component);
            StateTransition::Noop
        }
        (WieldableState::Raising, WieldableStateEvent::Cancel { .. }) => StateTransition::Noop,
        (WieldableState::Raising, WieldableStateEvent::BeginLower { duration_ms }) => {
            begin_lowering(component, duration_ms);
            StateTransition::Noop
        }
    }
}

fn complete_reload(
    component: &mut WeaponComponent,
    shell_loaded: bool,
    feedback_tick: u64,
) -> StateTransition {
    let transferred = component.reload_credited;
    transition_to_idle(component);
    component.publish_reload_feedback(ReloadFeedback::Completed, feedback_tick);
    StateTransition::ReloadStep {
        shell_loaded,
        completed: Some(transferred),
        restart_duration_ms: None,
    }
}

pub(super) fn resolve_expired_state(
    registry: &mut EntityRegistry,
    pawn: Option<EntityId>,
    weapon: EntityId,
    component: &mut WeaponComponent,
    mut overshoot_ms: f64,
    feedback_tick: u64,
    deliveries: &mut Vec<ReloadDelivery>,
) -> bool {
    let reserve_was_present = pawn.is_some_and(|pawn| {
        registry.has_component_kind(pawn, ComponentKind::AmmoReserve) == Ok(true)
    });
    let mut working_reserve = pawn.map(|pawn| {
        registry
            .get_component::<AmmoReserve>(pawn)
            .cloned()
            .unwrap_or_default()
    });

    let mut lowered = false;
    while component.state.is_timed_state() && component.state_remaining_ms == 0 {
        let transition = transition_wieldable_state(
            component,
            WieldableStateEvent::Expired {
                pawn_present: pawn.is_some(),
                feedback_tick,
            },
            working_reserve.as_mut(),
        );
        if transition == StateTransition::Lowered {
            lowered = true;
            break;
        }

        let StateTransition::ReloadStep {
            shell_loaded,
            completed,
            restart_duration_ms,
        } = transition
        else {
            break;
        };

        if let Some(pawn) = pawn {
            if shell_loaded {
                deliveries.push(ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::ShellLoaded,
                });
            }
            if let Some(transferred) = completed {
                deliveries.push(ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::Completed { transferred },
                });
            }
        }

        let Some(duration_ms) = restart_duration_ms else {
            break;
        };
        if restart_timed_step(component, duration_ms, overshoot_ms) {
            overshoot_ms = (overshoot_ms - f64::from(duration_ms)).max(0.0);
            continue;
        }
        break;
    }

    if reserve_was_present && let (Some(pawn), Some(reserve)) = (pawn, working_reserve) {
        let _ = registry.set_component(pawn, reserve);
    }
    lowered
}

/// Restart a repeated timed step. The carry stores only the fractional
/// remainder; a whole-step overshoot leaves the timer expired so the caller can
/// credit the next shell in this same tick. The shared boundary comparison keeps
/// an exact f32-authored duration from deferring a shell one fixed tick.
fn restart_timed_step(
    component: &mut WeaponComponent,
    duration_ms: u32,
    overshoot_ms: f64,
) -> bool {
    component.state_total_ms = duration_ms;
    if super::super::reload::reaches_millisecond_boundary(overshoot_ms, duration_ms) {
        component.state_remaining_ms = 0;
        component.state_elapsed_sub_ms = 0.0;
        return true;
    }

    let whole_ms = overshoot_ms.floor() as u32;
    component.state_remaining_ms = duration_ms - whole_ms;
    component.state_elapsed_sub_ms = overshoot_ms - f64::from(whole_ms);
    false
}

fn transition_to_idle(component: &mut WeaponComponent) {
    component.state = WieldableState::Idle;
    component.state_remaining_ms = 0;
    component.state_total_ms = 0;
    component.state_elapsed_sub_ms = 0.0;
    component.reload_credited = 0;
}

fn begin_lowering(component: &mut WeaponComponent, duration_ms: u32) {
    component.clear_equip_reload_feedback();
    component.state = WieldableState::Lowering;
    component.state_remaining_ms = duration_ms;
    component.state_total_ms = duration_ms;
    component.state_elapsed_sub_ms = 0.0;
    component.reload_credited = 0;
}

pub(super) fn finish_lowering(component: &mut WeaponComponent) {
    transition_to_idle(component);
}

pub(super) fn begin_raising(component: &mut WeaponComponent) {
    component.state = WieldableState::Raising;
    component.state_remaining_ms = component.raise_ms;
    component.state_total_ms = component.raise_ms;
    component.state_elapsed_sub_ms = 0.0;
    component.reload_credited = 0;
}
