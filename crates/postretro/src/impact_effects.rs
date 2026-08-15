// Command-buffer effects applied to live recipients resolved from impact target tokens.
// See: context/lib/scripting.md §11 · context/lib/entity_model.md §5.

use postretro_entities::components::brain::BrainComponent;
use postretro_entities::components::grant::{grant_ammo, grant_health};
use postretro_entities::components::health::{
    HealthComponent, PendingKillCredit, set_health_absolute,
};
use postretro_entities::components::mesh::{SwitchResult, switch_animation_state};
use postretro_entities::{
    DeferredEffectComponent, DeferredEffectKind, EntityId, EntityRegistry,
    MAX_PENDING_EFFECTS_PER_ENTITY, PendingEffect,
};

use crate::scripting_systems::health::ContributorLedgerSnapshot;

/// Postretro-side kill-report facts handed out by the deferred-removal seam.
///
/// `PendingKillCredit` remains entities-resident because it is component state;
/// this snapshot is deliberately made after reading that state and before its
/// destruction, so downstream report consumers cannot retain a component type.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct KillReportCredit {
    pub(crate) tags: Vec<String>,
    pub(crate) contributor_ledger: ContributorLedgerSnapshot,
}

impl From<&PendingKillCredit> for KillReportCredit {
    fn from(pending: &PendingKillCredit) -> Self {
        Self {
            tags: pending.tags.clone(),
            contributor_ledger: ContributorLedgerSnapshot::from_contributor_ledger(
                &pending.contributor_ledger,
            ),
        }
    }
}

/// Closed non-IR impact-effect instructions consumed by the policy evaluator.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ImpactEffect {
    Despawn {
        after_ms: Option<f32>,
    },
    SetHealth {
        value: f32,
        after_ms: Option<f32>,
    },
    GrantHealth {
        amount: f32,
    },
    GrantAmmo {
        pool: String,
        amount: f32,
    },
    PlayAnimation {
        state: String,
    },
    /// A passive visual command. The policy runtime intercepts it while the
    /// dispatch and scripting context are still available, so it must never
    /// attempt to resolve a registry target through this generic applier.
    Present {
        template: String,
        value: f32,
    },
    /// Owner-addressed store writes resolve their destination seat through the
    /// policy runtime, where the ScriptCtx slot table is available. They must
    /// never reach this registry-only applier.
    SetOwnerSlot {
        slot: String,
        value: f32,
    },
}

/// Apply one command-buffer effect to the resolved target id.
///
/// The target is the live `EntityId` resolved from the impact's opaque command
/// target; it is never interpreted as a numeric IR input.
pub(crate) fn apply_effect(registry: &mut EntityRegistry, target: EntityId, effect: &ImpactEffect) {
    match effect {
        ImpactEffect::Despawn { after_ms } => despawn(registry, target, *after_ms),
        ImpactEffect::SetHealth { value, after_ms } => {
            set_health(registry, target, *value, *after_ms)
        }
        ImpactEffect::GrantHealth { amount } => {
            let _ = grant_health(registry, target, *amount);
        }
        ImpactEffect::GrantAmmo { pool, amount } => {
            let _ = grant_ammo(registry, target, pool, *amount);
        }
        ImpactEffect::PlayAnimation { state } => {
            let _ = play_animation(registry, target, state);
        }
        ImpactEffect::Present { .. } => {
            // Presentation effects are consumed by ImpactPolicyRuntime before
            // this registry-only fallback. Keeping this harmless makes a
            // malformed/manual command degrade rather than panic.
        }
        ImpactEffect::SetOwnerSlot { .. } => {
            unreachable!("owner slot writes are intercepted by ImpactPolicyRuntime")
        }
    }
}

/// Apply or enqueue an absolute health write.
///
/// `Some(0.0)` is still deferred: only an absent `afterMs` is immediate. That
/// distinction makes the first countdown decrement unambiguously belong to a
/// later game-logic tick.
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
            remaining_us: delay_micros(after_ms),
            value: Some(value),
        },
    );
}

/// Apply or enqueue a terminal despawn.
///
/// Delayed despawns leave the entity live until their countdown elapses, while
/// brain and agent ticks quiesce it for the authored removal window. An
/// immediate despawn (or an elapsed queued despawn) makes it inert, clears the
/// whole queue, and stages it for the one app-owned frame-end removal pass.
pub(crate) fn despawn(registry: &mut EntityRegistry, target: EntityId, after_ms: Option<f32>) {
    if let Some(after_ms) = after_ms {
        enqueue(
            registry,
            target,
            PendingEffect {
                kind: DeferredEffectKind::Despawn,
                remaining_us: delay_micros(after_ms),
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
pub(crate) fn play_animation(
    registry: &mut EntityRegistry,
    target: EntityId,
    state: &str,
) -> SwitchResult {
    switch_animation_state(registry, target, state)
}

/// Whether a zero-HP entity is waiting for an authored positive-health recovery.
///
/// This is the nonterminal "downed" lifecycle: it keeps the entity present and
/// targetable, but AI and steering must hold still until the queued recovery
/// writes health back above zero. A bare zero-HP entity remains active; only an
/// explicit deferred recovery opts into this behavior.
pub(crate) fn is_downed_for_recovery(registry: &EntityRegistry, target: EntityId) -> bool {
    let Ok(health) = registry.get_component::<HealthComponent>(target) else {
        return false;
    };
    if health.current > 0.0 {
        return false;
    }

    registry
        .get_component::<DeferredEffectComponent>(target)
        .is_ok_and(|effects| {
            effects.pending.iter().any(|effect| {
                effect.kind == DeferredEffectKind::SetHealth
                    && effect
                        .value
                        .is_some_and(|value| value.is_finite() && value > 0.0)
            })
        })
}

/// Restore a downed brain's baseline presentation when its delayed health
/// recovery lands. The normal AI tick owns the later idle/walk/attack choice;
/// this only prevents a revived enemy from remaining frozen in its death pose
/// until that tick observes movement.
fn resume_recovered_brain_presentation(registry: &mut EntityRegistry, target: EntityId) {
    let Ok(mut brain) = registry.get_component::<BrainComponent>(target).cloned() else {
        return;
    };
    let rest = crate::scripting_systems::ai::rest_animation(&brain.graph).map(str::to_string);
    // The deferred period preserves the last locomotion velocity and graph
    // state. Invalidate the animation latch so the following AI tick reselects
    // the state-appropriate rest, travel, or action clip instead of leaving this
    // one-tick baseline pose in place indefinitely.
    brain.locomotion_moving = !brain.locomotion_moving;
    let _ = registry.set_component(target, brain);
    if let Some(rest) = rest {
        let _ = play_animation(registry, target, &rest);
    }
}

/// Advance active deferred-effect queues after the weapon and enemy-melee
/// damage chokepoints. The caller supplies fixed-tick seconds; script-facing
/// milliseconds are stored as integer microseconds.
pub(crate) fn tick_deferred_effects(registry: &mut EntityRegistry, tick_dt: f32) {
    let dt_us = tick_micros(tick_dt);
    let mut active = registry.take_active_deferred_effects();
    let mut retained = 0;

    for index in 0..active.len() {
        let target = active[index];
        let Ok(component) = registry.deferred_effect_mut(target) else {
            continue;
        };
        if component.inert || component.pending.is_empty() {
            continue;
        }

        let mut pending = std::mem::take(&mut component.pending);
        for effect in &mut pending {
            effect.remaining_us = effect.remaining_us.saturating_sub(dt_us);
        }

        let mut terminal = false;
        let mut pending_index = 0;
        while pending_index < pending.len() {
            if pending[pending_index].remaining_us > 0 {
                pending_index += 1;
                continue;
            }

            let effect = pending.remove(pending_index);
            match effect.kind {
                DeferredEffectKind::SetHealth => {
                    let was_downed = registry
                        .get_component::<HealthComponent>(target)
                        .is_ok_and(|health| health.current <= 0.0);
                    set_health_absolute(registry, target, effect.value.unwrap_or(0.0));
                    let recovered = registry
                        .get_component::<HealthComponent>(target)
                        .is_ok_and(|health| health.current.is_finite() && health.current > 0.0);
                    if was_downed && recovered {
                        resume_recovered_brain_presentation(registry, target);
                    }
                }
                DeferredEffectKind::Despawn => {
                    pending.clear();
                    terminal = true;
                    break;
                }
            }
        }

        let Ok(component) = registry.deferred_effect_mut(target) else {
            continue;
        };
        component.pending = pending;
        component.inert |= terminal;
        if component.pending.len() < MAX_PENDING_EFFECTS_PER_ENTITY {
            component.overflow_reported = false;
        }

        if terminal {
            let _ = registry.mark_for_end_of_frame_removal(target);
        } else if !component.pending.is_empty() {
            active[retained] = target;
            retained += 1;
        }
    }

    active.truncate(retained);
    registry.replace_active_deferred_effects(active);
}

/// Reap all terminally marked entities exactly once at the app's frame-end
/// stage. The callback observes successful removals only and receives the
/// non-player credit snapshot captured before `despawn` drops the component.
/// A missing snapshot means this was an above-zero despawn and must not report
/// a kill.
pub(crate) fn run_end_of_frame_removal_pass(
    registry: &mut EntityRegistry,
    mut on_removed: impl FnMut(EntityId, Option<KillReportCredit>),
) {
    for target in registry.take_end_of_frame_removals() {
        let pending_kill_credit = registry
            .get_component::<HealthComponent>(target)
            .ok()
            .and_then(|health| {
                health
                    .pending_kill_credit
                    .as_ref()
                    .map(KillReportCredit::from)
            });
        if registry.despawn(target).is_ok() {
            on_removed(target, pending_kill_credit);
        }
    }
}

fn enqueue(registry: &mut EntityRegistry, target: EntityId, effect: PendingEffect) {
    let Ok(component) = registry.deferred_effect_mut(target) else {
        return;
    };
    if component.inert {
        return;
    }
    if component.pending.len() >= MAX_PENDING_EFFECTS_PER_ENTITY {
        if !component.overflow_reported {
            log::warn!(
                "[Impact] deferred-effect queue for {target:?} reached {MAX_PENDING_EFFECTS_PER_ENTITY}; dropping newest effects until it drains"
            );
            component.overflow_reported = true;
        }
        return;
    }

    component.pending.push(effect);
    let _ = registry.activate_deferred_effects(target);
}

fn mark_terminal_despawn(registry: &mut EntityRegistry, target: EntityId) {
    let Ok(component) = registry.deferred_effect_mut(target) else {
        return;
    };

    component.inert = true;
    component.pending.clear();
    component.overflow_reported = false;
    let _ = registry.mark_for_end_of_frame_removal(target);
}

fn delay_micros(after_ms: f32) -> u64 {
    if after_ms <= 0.0 || after_ms.is_nan() {
        return 0;
    }
    (f64::from(after_ms) * 1_000.0).ceil() as u64
}

fn tick_micros(tick_dt: f32) -> u64 {
    if tick_dt <= 0.0 || tick_dt.is_nan() {
        return 0;
    }
    (f64::from(tick_dt) * 1_000_000.0).ceil() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::Transform;
    use postretro_entities::components::health::ContributorLedgerRecord;
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

    fn latch_nonplayer_kill_credit(
        registry: &mut EntityRegistry,
        target: EntityId,
        tags: &[&str],
        source_id: &str,
    ) {
        registry
            .set_tags(target, tags.iter().map(|tag| (*tag).to_string()).collect())
            .expect("target is live");
        let mut health = registry
            .get_component::<HealthComponent>(target)
            .expect("target has health")
            .clone();
        health.current = 0.0;
        health.record_contributor_damage(ContributorLedgerRecord::new(source_id, health.max));
        registry
            .set_component(target, health)
            .expect("target is live");

        assert_eq!(
            crate::scripting_systems::health::sweep_deaths(registry),
            crate::scripting_systems::health::DeathReport::default(),
            "zero HP latches credit but does not report a kill",
        );
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
                .remaining_us as f32,
            100_000.0,
        );

        tick_deferred_effects(&mut registry, 0.040);
        assert_number_approx_eq(
            registry
                .get_component::<DeferredEffectComponent>(target)
                .expect("effect remains queued")
                .pending[0]
                .remaining_us as f32,
            60_000.0,
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
        run_end_of_frame_removal_pass(&mut registry, |id, credit| removed.push((id, credit)));
        assert_eq!(removed, vec![(target, None)]);
        assert!(!registry.exists(target));
    }

    #[test]
    fn latched_kill_credit_reports_once_at_successful_frame_end_removal() {
        let mut registry = EntityRegistry::new();
        let target = health_target(&mut registry, 100.0);
        latch_nonplayer_kill_credit(&mut registry, target, &["wave"], "weapon.first");

        despawn(&mut registry, target, None);
        let mut reports = Vec::new();
        run_end_of_frame_removal_pass(&mut registry, |id, credit| reports.push((id, credit)));

        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].0, target);
        let credit = reports[0].1.as_ref().expect("latched kill carries credit");
        assert_eq!(credit.tags, vec!["wave".to_string()]);
        assert_eq!(
            credit.contributor_ledger.entries[0].source_id,
            "weapon.first"
        );
        assert!(!registry.exists(target));

        run_end_of_frame_removal_pass(&mut registry, |id, credit| reports.push((id, credit)));
        assert_eq!(
            reports.len(),
            1,
            "a successful removal reports exactly once"
        );
    }

    #[test]
    fn delayed_despawn_reports_latched_credit_only_when_removed() {
        let mut registry = EntityRegistry::new();
        let target = health_target(&mut registry, 100.0);
        latch_nonplayer_kill_credit(&mut registry, target, &["wave"], "weapon.delayed");

        despawn(&mut registry, target, Some(10.0));
        tick_deferred_effects(&mut registry, 0.005);
        let mut reports = Vec::new();
        run_end_of_frame_removal_pass(&mut registry, |id, credit| reports.push((id, credit)));
        assert!(registry.exists(target));
        assert!(
            reports.is_empty(),
            "the unelapsed delay cannot report a kill"
        );

        tick_deferred_effects(&mut registry, 0.005);
        run_end_of_frame_removal_pass(&mut registry, |id, credit| reports.push((id, credit)));
        assert_eq!(reports.len(), 1);
        assert_eq!(
            reports[0].1.as_ref().unwrap().tags,
            vec!["wave".to_string()]
        );
    }

    // Regression: resurrection cleared the frozen wrapper but retained the live
    // contributor ledger, so the later removal credited both downs.
    #[test]
    fn resurrected_rekill_removal_reports_only_new_lethal_contributor() {
        let mut registry = EntityRegistry::new();
        let target = health_target(&mut registry, 100.0);
        latch_nonplayer_kill_credit(&mut registry, target, &["first-down"], "weapon.first");

        set_health(&mut registry, target, 100.0, None);
        let resurrected = registry.get_component::<HealthComponent>(target).unwrap();
        assert!(!resurrected.death_handled);
        assert!(resurrected.pending_kill_credit.is_none());

        latch_nonplayer_kill_credit(&mut registry, target, &["second-down"], "weapon.second");
        despawn(&mut registry, target, None);
        let mut reports = Vec::new();
        run_end_of_frame_removal_pass(&mut registry, |id, credit| reports.push((id, credit)));

        assert_eq!(reports.len(), 1);
        assert_eq!(
            reports[0].1.as_ref().unwrap().tags,
            vec!["second-down".to_string()],
            "the revived entity reports only its re-kill's freshly latched credit",
        );
        let ledger = &reports[0].1.as_ref().unwrap().contributor_ledger;
        assert_eq!(ledger.entries.len(), 1);
        assert_eq!(ledger.entries[0].source_id, "weapon.second");
        assert!(ledger.overflow.is_none());
    }

    #[test]
    fn zero_health_effect_preserves_latched_death_and_pending_credit() {
        let mut registry = EntityRegistry::new();
        let target = health_target(&mut registry, 100.0);
        latch_nonplayer_kill_credit(&mut registry, target, &["downed"], "weapon.first");

        set_health(&mut registry, target, 0.0, None);

        let health = registry.get_component::<HealthComponent>(target).unwrap();
        assert_eq!(health.current, 0.0);
        assert!(health.death_handled);
        let credit = health
            .pending_kill_credit
            .as_ref()
            .expect("setHealth(0) must preserve the frozen down credit");
        assert_eq!(credit.tags, vec!["downed".to_string()]);
        assert_eq!(
            credit.contributor_ledger.entries()[0].source_id,
            "weapon.first"
        );
    }

    // Regression: host snapshots were collected before frame-end impact
    // removals, briefly recreating terminally removed entities on clients.
    #[test]
    fn authoritative_snapshot_after_removal_omits_terminal_entity() {
        let mut registry = EntityRegistry::new();
        let target = health_target(&mut registry, 100.0);
        let mut replicable = crate::netcode::ReplicableSet::new();
        replicable.register(target);

        despawn(&mut registry, target, None);
        run_end_of_frame_removal_pass(&mut registry, |_, _| {});

        let snapshots = crate::netcode::produce_owned_snapshots(
            &registry,
            &replicable,
            &mut crate::netcode::NetworkIdAllocator::new(),
            &crate::netcode::MovementOwners::new(),
            &crate::netcode::HostCommandQueues::new(),
        );
        assert!(snapshots.is_empty());
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

        run_end_of_frame_removal_pass(&mut registry, |_, _| {});
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

    // Regression: subtracting a fixed-tick f32 from a huge finite delay could
    // round back to the same countdown forever.
    #[test]
    fn huge_finite_delay_makes_integer_progress_each_tick() {
        let mut registry = EntityRegistry::new();
        let target = health_target(&mut registry, 100.0);

        set_health(&mut registry, target, 25.0, Some(f32::MAX));
        let before = registry
            .get_component::<DeferredEffectComponent>(target)
            .unwrap()
            .pending[0]
            .remaining_us;
        tick_deferred_effects(&mut registry, 0.001);
        let after = registry
            .get_component::<DeferredEffectComponent>(target)
            .unwrap()
            .pending[0]
            .remaining_us;

        assert!(after < before, "every positive tick must advance the delay");
    }

    // Regression: repeated impacts could grow one entity's queue without a
    // bound and destabilize fixed-tick work.
    #[test]
    fn deferred_queue_drops_newest_overflow_and_preserves_admitted_fifo() {
        let mut registry = EntityRegistry::new();
        let target = health_target(&mut registry, 100.0);

        for value in 0..=MAX_PENDING_EFFECTS_PER_ENTITY {
            set_health(&mut registry, target, value as f32, Some(0.0));
        }
        let effects = registry
            .get_component::<DeferredEffectComponent>(target)
            .unwrap();
        assert_eq!(effects.pending.len(), MAX_PENDING_EFFECTS_PER_ENTITY);
        assert_eq!(effects.pending.first().unwrap().value, Some(0.0));
        assert_eq!(
            effects.pending.last().unwrap().value,
            Some((MAX_PENDING_EFFECTS_PER_ENTITY - 1) as f32),
            "the newest overflowing request is dropped"
        );

        tick_deferred_effects(&mut registry, 0.001);
        assert_number_approx_eq(
            health(&registry, target),
            (MAX_PENDING_EFFECTS_PER_ENTITY - 1) as f32,
        );
    }
}
