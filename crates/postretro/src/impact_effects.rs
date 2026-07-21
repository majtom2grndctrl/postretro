// Command-buffer effects addressed to one live impact target.
// See: context/lib/scripting.md §11 · context/lib/entity_model.md §5.

#[cfg(test)]
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::health::set_health_absolute;
use postretro_entities::components::mesh::{SwitchResult, switch_animation_state};
use postretro_entities::{
    ComponentKind, DeferredEffectComponent, DeferredEffectKind, EntityId, EntityRegistry,
    PendingEffect,
};

/// Closed impact-effect instruction set consumed by the later policy evaluator.
///
/// `setState` is deliberately absent: Task 2 already owns it as an
/// `EntityScope` IR output. These instructions are the non-IR, target-token
/// effects that run against Task 1's resolved `ImpactDispatch.target` id.
// Task 5 consumes this closed command vocabulary after it binds impact-policy
// descriptors. Task 3 owns the execution seam first, so it is intentionally
// uncalled until that evaluator lands.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ImpactEffect {
    Despawn { after_ms: Option<f32> },
    SetHealth { value: f32, after_ms: Option<f32> },
    PlayAnimation { state: String },
}

/// Apply one command-buffer effect to the resolved target id.
///
/// The target is a live `EntityId` supplied by Task 1's opaque command-target
/// channel; it is never interpreted as a numeric IR input.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn apply_effect(registry: &mut EntityRegistry, target: EntityId, effect: &ImpactEffect) {
    match effect {
        ImpactEffect::Despawn { after_ms } => despawn(registry, target, *after_ms),
        ImpactEffect::SetHealth { value, after_ms } => {
            set_health(registry, target, *value, *after_ms)
        }
        ImpactEffect::PlayAnimation { state } => {
            let _ = play_animation(registry, target, state);
        }
    }
}

/// Apply or enqueue an absolute health write.
///
/// `Some(0.0)` is still deferred: only an absent `afterMs` is immediate. That
/// distinction makes the first countdown decrement unambiguously belong to a
/// later game-logic tick.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn set_health(
    registry: &mut EntityRegistry,
    target: EntityId,
    value: f32,
    after_ms: Option<f32>,
) {
    let Some(after_ms) = after_ms else {
        set_health_absolute(registry, target, value);
        return;
    };

    enqueue(
        registry,
        target,
        PendingEffect {
            kind: DeferredEffectKind::SetHealth,
            countdown_ms: after_ms.max(0.0),
            value: Some(value),
        },
    );
}

/// Apply or enqueue a terminal despawn.
///
/// Delayed despawns leave the entity active until their countdown elapses. An
/// immediate despawn (or an elapsed queued despawn) makes it inert, clears the
/// whole queue, and stages it for the one app-owned frame-end removal pass.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn despawn(registry: &mut EntityRegistry, target: EntityId, after_ms: Option<f32>) {
    if let Some(after_ms) = after_ms {
        enqueue(
            registry,
            target,
            PendingEffect {
                kind: DeferredEffectKind::Despawn,
                countdown_ms: after_ms.max(0.0),
                value: None,
            },
        );
        return;
    }

    mark_terminal_despawn(registry, target);
}

/// Switch a declared mesh animation state synchronously while the target is
/// still live. This deliberately bypasses tag-targeted reaction/app-drain
/// dispatch so an in-group `playAnim` following `despawn()` still lands.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn play_animation(
    registry: &mut EntityRegistry,
    target: EntityId,
    state: &str,
) -> SwitchResult {
    switch_animation_state(registry, target, state)
}

/// Advance every deferred-effect queue after the weapon and enemy-melee damage
/// chokepoints. The caller supplies fixed-tick seconds; queue values stay in
/// milliseconds to match the script surface.
pub(crate) fn tick_deferred_effects(registry: &mut EntityRegistry, tick_dt: f32) {
    let dt_ms = tick_dt.max(0.0) * 1000.0;
    let targets: Vec<EntityId> = registry
        .iter_with_kind(ComponentKind::DeferredEffect)
        .map(|(id, _)| id)
        .collect();

    for target in targets {
        let Ok(component) = registry
            .get_component::<DeferredEffectComponent>(target)
            .cloned()
        else {
            continue;
        };
        if component.inert || component.pending.is_empty() {
            continue;
        }

        let mut updated = component;
        for effect in &mut updated.pending {
            effect.countdown_ms = (effect.countdown_ms - dt_ms).max(0.0);
        }

        let mut remaining = Vec::with_capacity(updated.pending.len());
        let mut terminal = false;
        for effect in updated.pending.drain(..) {
            if effect.countdown_ms > 0.0 {
                remaining.push(effect);
                continue;
            }

            match effect.kind {
                DeferredEffectKind::SetHealth => {
                    set_health_absolute(registry, target, effect.value.unwrap_or(0.0));
                }
                DeferredEffectKind::Despawn => {
                    updated.inert = true;
                    terminal = true;
                    break;
                }
            }
        }

        updated.pending = if terminal { Vec::new() } else { remaining };
        let _ = registry.set_component(target, updated);
        if terminal {
            let _ = registry.mark_for_end_of_frame_removal(target);
        }
    }
}

/// Reap all terminally marked entities exactly once at the app's frame-end
/// stage. The callback is the future lifecycle/reporting sink; Task 3 supplies
/// no death or kill behavior itself.
pub(crate) fn run_end_of_frame_removal_pass(
    registry: &mut EntityRegistry,
    mut on_removed: impl FnMut(EntityId),
) {
    for target in registry.take_end_of_frame_removals() {
        if registry.despawn(target).is_ok() {
            on_removed(target);
        }
    }
}

#[cfg_attr(not(test), allow(dead_code))]
fn enqueue(registry: &mut EntityRegistry, target: EntityId, effect: PendingEffect) {
    let Ok(component) = registry
        .get_component::<DeferredEffectComponent>(target)
        .cloned()
    else {
        return;
    };
    if component.inert {
        return;
    }

    let mut updated = component;
    updated.pending.push(effect);
    let _ = registry.set_component(target, updated);
}

#[cfg_attr(not(test), allow(dead_code))]
fn mark_terminal_despawn(registry: &mut EntityRegistry, target: EntityId) {
    let Ok(component) = registry
        .get_component::<DeferredEffectComponent>(target)
        .cloned()
    else {
        return;
    };

    let mut updated = component;
    updated.inert = true;
    updated.pending.clear();
    let _ = registry.set_component(target, updated);
    let _ = registry.mark_for_end_of_frame_removal(target);
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::Transform;
    use postretro_entities::data_descriptors::HealthDescriptor;

    fn health_target(registry: &mut EntityRegistry, max: f32) -> EntityId {
        let target = registry.spawn(Transform::default());
        let health = HealthComponent::from_descriptor(&HealthDescriptor {
            max,
            hitbox: None,
            zone_multipliers: Default::default(),
        });
        registry
            .set_component(target, health)
            .expect("target is live");
        target
    }

    fn health(registry: &EntityRegistry, target: EntityId) -> f32 {
        registry
            .get_component::<HealthComponent>(target)
            .expect("health remains attached")
            .current
    }

    fn assert_number_approx_eq(actual: f32, expected: f32) {
        const EPSILON: f32 = 1.0e-6;
        assert!(
            (actual - expected).abs() <= EPSILON,
            "expected {expected} ± {EPSILON}, got {actual}"
        );
    }

    #[test]
    fn deferred_effects_apply_ready_entries_in_insertion_order() {
        let mut registry = EntityRegistry::new();
        let target = health_target(&mut registry, 100.0);

        apply_effect(
            &mut registry,
            target,
            &ImpactEffect::SetHealth {
                value: 15.0,
                after_ms: Some(0.0),
            },
        );
        set_health(&mut registry, target, 40.0, Some(0.0));
        tick_deferred_effects(&mut registry, 0.001);

        assert_number_approx_eq(health(&registry, target), 40.0);
        assert!(
            registry
                .get_component::<DeferredEffectComponent>(target)
                .expect("effect component remains attached")
                .pending
                .is_empty()
        );
    }

    #[test]
    fn deferred_effect_countdown_starts_on_the_next_tick() {
        let mut registry = EntityRegistry::new();
        let target = health_target(&mut registry, 100.0);

        set_health(&mut registry, target, 25.0, Some(100.0));
        assert_number_approx_eq(
            registry
                .get_component::<DeferredEffectComponent>(target)
                .expect("effect is queued this tick")
                .pending[0]
                .countdown_ms,
            100.0,
        );

        tick_deferred_effects(&mut registry, 0.040);
        assert_number_approx_eq(
            registry
                .get_component::<DeferredEffectComponent>(target)
                .expect("effect remains queued")
                .pending[0]
                .countdown_ms,
            60.0,
        );
        assert_number_approx_eq(health(&registry, target), 100.0);
    }

    #[test]
    fn immediate_despawn_marks_then_frame_end_reaps() {
        let mut registry = EntityRegistry::new();
        let target = health_target(&mut registry, 100.0);

        apply_effect(
            &mut registry,
            target,
            &ImpactEffect::Despawn { after_ms: None },
        );
        assert!(registry.exists(target), "despawn is never inline");
        assert!(
            registry
                .get_component::<DeferredEffectComponent>(target)
                .expect("target remains live before frame end")
                .inert
        );

        let mut removed = Vec::new();
        run_end_of_frame_removal_pass(&mut registry, |id| removed.push(id));
        assert_eq!(removed, vec![target]);
        assert!(!registry.exists(target));
    }

    #[test]
    fn delayed_despawn_marks_only_after_its_countdown_elapses() {
        let mut registry = EntityRegistry::new();
        let target = health_target(&mut registry, 100.0);

        despawn(&mut registry, target, Some(10.0));
        assert!(
            !registry
                .get_component::<DeferredEffectComponent>(target)
                .expect("target stays active while countdown runs")
                .inert
        );

        tick_deferred_effects(&mut registry, 0.005);
        assert!(registry.exists(target));
        assert!(
            !registry
                .get_component::<DeferredEffectComponent>(target)
                .expect("countdown is not done")
                .inert
        );

        tick_deferred_effects(&mut registry, 0.005);
        assert!(registry.exists(target), "frame-end removal has not run yet");
        assert!(
            registry
                .get_component::<DeferredEffectComponent>(target)
                .expect("elapsed despawn makes the target inert")
                .inert
        );

        run_end_of_frame_removal_pass(&mut registry, |_| {});
        assert!(!registry.exists(target));
    }

    #[test]
    fn terminal_despawn_cancels_remaining_deferred_effects() {
        let mut registry = EntityRegistry::new();
        let target = health_target(&mut registry, 100.0);

        set_health(&mut registry, target, 70.0, Some(0.0));
        despawn(&mut registry, target, Some(0.0));
        set_health(&mut registry, target, 5.0, Some(0.0));
        tick_deferred_effects(&mut registry, 0.001);

        assert_number_approx_eq(health(&registry, target), 70.0);
        let effects = registry
            .get_component::<DeferredEffectComponent>(target)
            .expect("target remains live before frame end");
        assert!(effects.inert);
        assert!(effects.pending.is_empty());
    }
}
