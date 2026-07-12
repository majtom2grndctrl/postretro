// Weapon reload timing, reserve transfer, feedback endpoints, and delivery events.
// See: context/lib/entity_model.md §5 · context/lib/ui.md §3

use postretro_entities::components::weapon::{ReloadFeedback, WeaponComponent};
use postretro_entities::{AmmoReserve, ComponentKind, EntityId, EntityRegistry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReloadDelivery {
    pub(crate) pawn: EntityId,
    pub(crate) weapon: EntityId,
    pub(crate) outcome: ReloadOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReloadOutcome {
    Started,
    Completed { transferred: u32 },
    BlockedFull,
    BlockedEmpty,
}

impl ReloadOutcome {
    pub(crate) const fn event_name(self) -> &'static str {
        match self {
            Self::Started => "reload_started",
            Self::Completed { .. } => "reload_completed",
            Self::BlockedFull => "reload_blocked_full",
            Self::BlockedEmpty => "reload_blocked_empty",
        }
    }
}

pub(super) fn tick(
    registry: &mut EntityRegistry,
    pawn: EntityId,
    weapon: EntityId,
    component: &mut WeaponComponent,
    reload: bool,
    tick_dt: f32,
) -> Vec<ReloadDelivery> {
    let was_reloading = component.reload_remaining_ms > 0;
    let fresh_press = reload && !component.reload_press_consumed;
    component.reload_press_consumed = reload;
    let mut deliveries = Vec::new();

    if !was_reloading {
        if !fresh_press {
            return deliveries;
        }

        let Some(ammo) = component.effective().ammo else {
            return deliveries;
        };
        let capacity = ammo.capacity;
        let reload_ms = ammo.reload_ms;
        let ammo_type = ammo.ammo_type.to_string();
        if component.magazine >= capacity {
            deliveries.push(ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::BlockedFull,
            });
            return deliveries;
        }
        let available = registry
            .get_component::<AmmoReserve>(pawn)
            .map_or(0, |reserve| reserve.available(&ammo_type));
        if available == 0 {
            deliveries.push(ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::BlockedEmpty,
            });
            return deliveries;
        }

        component.reload_total_ms = reload_ms;
        component.reload_remaining_ms = reload_ms;
        component.reload_elapsed_sub_ms = 0.0;
        component.reload_feedback = Some(ReloadFeedback::Started);
        deliveries.push(ReloadDelivery {
            pawn,
            weapon,
            outcome: ReloadOutcome::Started,
        });

        // The feedback marker preserves the exact zero-progress start sample
        // while the authoritative timer still advances on this tick. A duration
        // no longer than one tick therefore retains immediate completion.
    }

    advance_timer(component, tick_dt);
    if component.reload_remaining_ms > 0 {
        return deliveries;
    }

    let effective_ammo = component
        .effective()
        .ammo
        .map(|ammo| (ammo.capacity, ammo.ammo_type.to_string()));
    let transferred = if let Some((capacity, ammo_type)) = effective_ammo {
        let need = capacity.saturating_sub(component.magazine);
        let mut reserve = registry
            .get_component::<AmmoReserve>(pawn)
            .cloned()
            .unwrap_or_default();
        let requested = need.min(reserve.available(&ammo_type));
        let transferred = reserve.take(&ammo_type, requested);
        component.magazine = component.magazine.saturating_add(transferred);
        if registry.has_component_kind(pawn, ComponentKind::AmmoReserve) == Ok(true) {
            let _ = registry.set_component(pawn, reserve);
        }
        transferred
    } else {
        0
    };
    component.reload_feedback = Some(ReloadFeedback::Completed);
    deliveries.push(ReloadDelivery {
        pawn,
        weapon,
        outcome: ReloadOutcome::Completed { transferred },
    });
    deliveries
}

fn tick_ms(tick_dt: f32) -> f64 {
    if tick_dt.is_finite() && tick_dt > 0.0 {
        f64::from(tick_dt) * 1000.0
    } else {
        0.0
    }
}

fn advance_timer(component: &mut WeaponComponent, tick_dt: f32) {
    let carried_ms = if component.reload_elapsed_sub_ms.is_finite()
        && component.reload_elapsed_sub_ms >= 0.0
        && component.reload_elapsed_sub_ms < 1.0
    {
        component.reload_elapsed_sub_ms
    } else {
        0.0
    };
    let elapsed_ms = carried_ms + tick_ms(tick_dt);
    if elapsed_ms >= f64::from(component.reload_remaining_ms) {
        component.reload_remaining_ms = 0;
        component.reload_elapsed_sub_ms = 0.0;
        return;
    }

    let whole_ms = elapsed_ms.floor() as u32;
    component.reload_remaining_ms -= whole_ms;
    component.reload_elapsed_sub_ms = elapsed_ms - f64::from(whole_ms);
}

/// Clear feedback endpoints after network projection and local HUD publication
/// have both observed the settled frame. Only endpoint-bearing weapons clone and
/// write back, so the idle fixed-tick path remains allocation-free beyond the
/// pre-existing shared weapon checkout.
pub(crate) fn clear_all_feedback(registry: &mut EntityRegistry) {
    let ids: Vec<EntityId> = registry
        .iter_with_kind(ComponentKind::Weapon)
        .filter_map(|(id, _)| {
            registry
                .get_component::<WeaponComponent>(id)
                .ok()
                .and_then(|weapon| weapon.reload_feedback.map(|_| id))
        })
        .collect();

    for id in ids {
        clear_feedback_for_weapon(registry, id);
    }
}

pub(crate) fn clear_feedback_for_weapon(registry: &mut EntityRegistry, id: EntityId) {
    if registry
        .get_component::<WeaponComponent>(id)
        .map_or(true, |weapon| weapon.reload_feedback.is_none())
    {
        return;
    }
    let Ok(mut weapon) = registry.get_component::<WeaponComponent>(id).cloned() else {
        return;
    };
    if weapon.reload_feedback.take().is_some() {
        let _ = registry.set_component(id, weapon);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes_map_to_stable_reaction_event_names() {
        assert_eq!(ReloadOutcome::Started.event_name(), "reload_started");
        assert_eq!(
            ReloadOutcome::Completed { transferred: 7 }.event_name(),
            "reload_completed"
        );
        assert_eq!(
            ReloadOutcome::BlockedFull.event_name(),
            "reload_blocked_full"
        );
        assert_eq!(
            ReloadOutcome::BlockedEmpty.event_name(),
            "reload_blocked_empty"
        );
    }
}
