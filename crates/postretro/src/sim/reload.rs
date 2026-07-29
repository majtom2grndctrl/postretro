// Weapon reload timing, reserve transfer, feedback endpoints, and delivery events.
// See: context/lib/entity_model.md §5 · context/lib/ui.md §3

use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::{ComponentKind, EntityId, EntityRegistry};

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
    ShellLoaded,
    Cancelled { transferred: u32 },
}

impl ReloadOutcome {
    pub(crate) const fn event_name(self) -> &'static str {
        match self {
            Self::Started => "reload_started",
            Self::Completed { .. } => "reload_completed",
            Self::BlockedFull => "reload_blocked_full",
            Self::BlockedEmpty => "reload_blocked_empty",
            Self::ShellLoaded => "reload_shell_loaded",
            Self::Cancelled { .. } => "reload_cancelled",
        }
    }
}

pub(super) fn tick_ms(tick_dt: f32) -> f64 {
    if tick_dt.is_finite() && tick_dt > 0.0 {
        f64::from(tick_dt) * 1000.0
    } else {
        0.0
    }
}

/// Treat an authored integer-millisecond boundary as reached when converting
/// the original `f32` tick to `f64` left it just below that same `f32` value.
/// The elapsed value itself stays unrounded so non-boundary carry remains exact.
pub(super) fn reaches_millisecond_boundary(elapsed_ms: f64, boundary_ms: u32) -> bool {
    let boundary = f64::from(boundary_ms);
    elapsed_ms >= boundary || elapsed_ms as f32 >= boundary_ms as f32
}

/// Advance the timed-state countdown and return its full millisecond overshoot
/// when it expires. The carry remains strictly sub-millisecond; callers that
/// restart a timed step apply the whole overshoot to that new step.
pub(super) fn advance_timer(component: &mut WeaponComponent, tick_dt: f32) -> Option<f64> {
    let carried_ms = if component.state_elapsed_sub_ms.is_finite()
        && component.state_elapsed_sub_ms >= 0.0
        && component.state_elapsed_sub_ms < 1.0
    {
        component.state_elapsed_sub_ms
    } else {
        0.0
    };
    let elapsed_ms = carried_ms + tick_ms(tick_dt);
    if reaches_millisecond_boundary(elapsed_ms, component.state_remaining_ms) {
        let overshoot_ms = (elapsed_ms - f64::from(component.state_remaining_ms)).max(0.0);
        component.state_remaining_ms = 0;
        component.state_elapsed_sub_ms = 0.0;
        return Some(overshoot_ms);
    }

    let whole_ms = elapsed_ms.floor() as u32;
    component.state_remaining_ms -= whole_ms;
    component.state_elapsed_sub_ms = elapsed_ms - f64::from(whole_ms);
    None
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
    use postretro_entities::data_descriptors::{FireMode, ResolutionMode, WeaponDescriptor};

    // Regression: exact f32 tick durations widened just below integer millisecond boundaries.
    #[test]
    fn exact_f32_tick_durations_reach_integer_millisecond_boundaries() {
        for (remaining_ms, tick_dt) in [(10, 0.01), (20, 0.02)] {
            let mut component = WeaponComponent::from_descriptor(&WeaponDescriptor {
                damage: 0.0,
                range: 0.0,
                cooldown_ms: 0.0,
                fire_mode: FireMode::Semi,
                resolution: ResolutionMode::Hitscan,
                credit_source: None,
                third_person_model: None,
                viewmodel: None,
                resource: None,
            });
            component.state_remaining_ms = remaining_ms;
            component.state_total_ms = remaining_ms;

            assert_eq!(advance_timer(&mut component, tick_dt), Some(0.0));
            assert_eq!(component.state_remaining_ms, 0);
            assert_eq!(component.state_elapsed_sub_ms, 0.0);
        }
    }

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
        assert_eq!(
            ReloadOutcome::ShellLoaded.event_name(),
            "reload_shell_loaded"
        );
        assert_eq!(
            ReloadOutcome::Cancelled { transferred: 7 }.event_name(),
            "reload_cancelled"
        );
    }
}
