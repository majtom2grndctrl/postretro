// Weapon command orchestration, impact damage, and reload delivery.
// See: context/lib/entity_model.md §5 · context/lib/networking.md

use std::cell::RefCell;
use std::rc::Rc;

use glam::Vec3;

use crate::collision::CollisionWorld;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::weapon::{self, FireButtonState, WeaponFireAuthorization, WeaponFireCommand};
use postretro_entities::components::health::{
    DamageContext, DamageProducer, HealthComponent, apply_damage_with_context,
};
use postretro_entities::components::weapon::{UNKNOWN_WEAPON_CREDIT_SOURCE, WeaponComponent};
use postretro_entities::{EntityId, EntityRegistry};

use super::{OpenAuthorizedShot, PostMovementCommand, RemotePawnCommand, ReloadDelivery, ReloadOutcome};

pub(super) fn weapon_fire_command(
    button: FireButtonState,
    post_movement: PostMovementCommand,
) -> WeaponFireCommand {
    // The aim normalization and `can_fire` gate below are degenerate-input guards.
    // `camera.aim_ray()` already returns normalized, finite values in normal operation;
    // these checks protect against NaN/zero vectors from headless or mocked callers.
    if post_movement.aim_origin.is_finite()
        && let Some(aim_direction) = normalize_aim_direction(post_movement.aim_direction)
    {
        return WeaponFireCommand {
            button,
            aim_origin: post_movement.aim_origin,
            aim_direction,
            can_fire: true,
        };
    }

    WeaponFireCommand {
        button,
        aim_origin: Vec3::ZERO,
        aim_direction: Vec3::Z,
        can_fire: false,
    }
}

fn normalize_aim_direction(direction: Vec3) -> Option<Vec3> {
    if !direction.is_finite() {
        return None;
    }
    let length_squared = direction.length_squared();
    if !length_squared.is_finite() || length_squared <= 1.0e-12 {
        return None;
    }
    Some(direction / length_squared.sqrt())
}

pub(super) fn run_remote_weapon_commands(
    registry: &Rc<RefCell<EntityRegistry>>,
    remote_pawn_commands: &[RemotePawnCommand],
    tick_dt: f32,
) -> (
    Vec<OpenAuthorizedShot>,
    Vec<ReloadDelivery>,
    Vec<&'static str>,
) {
    let mut registry = registry.borrow_mut();
    let mut authorized = Vec::new();
    let mut reload_deliveries = Vec::new();
    let mut weapon_events = Vec::new();

    for remote in remote_pawn_commands {
        let Some(weapon) = remote.weapon else {
            continue;
        };
        let Ok(mut weapon_component) = registry.get_component::<WeaponComponent>(weapon).cloned()
        else {
            continue;
        };
        let deliveries = super::reload::tick(
            &mut registry,
            remote.pawn,
            weapon,
            &mut weapon_component,
            remote.command.reload,
            tick_dt,
        );
        let reload_started = deliveries
            .iter()
            .any(|delivery| delivery.outcome == ReloadOutcome::Started);
        reload_deliveries.extend(deliveries);

        let command = WeaponFireCommand {
            button: remote.command.fire_button,
            aim_origin: Vec3::ZERO,
            aim_direction: Vec3::Z,
            // Repurposes `can_fire` (elsewhere "aim valid") to mean "pawn has a NetworkId";
            // the real fire gate is `button` -> `wants_fire`. The host casts no local aim ray.
            can_fire: remote.shot_id.is_some(),
        };
        let result = weapon::tick_state_only_component(
            &mut weapon_component,
            &command,
            tick_dt,
            reload_started,
        );
        let effective = weapon_component.effective();
        let damage = effective.damage;
        let range = effective.range;
        let credit_source = effective.credit_source.to_string();
        let _ = registry.set_component(weapon, weapon_component);
        match result {
            WeaponFireAuthorization::Accepted => {}
            WeaponFireAuthorization::Empty => {
                weapon_events.push("dry_fire");
                continue;
            }
            WeaponFireAuthorization::Rejected => continue,
        }
        let Some(shot_id) = remote.shot_id else {
            continue;
        };
        authorized.push(OpenAuthorizedShot {
            shot: super::AuthorizedShot {
                shot_id,
                pawn: remote.pawn,
                weapon,
                fire_tick: remote.fire_tick,
                damage,
                range,
                pellet_count: 1,
                credit_source,
            },
            owner_client_id: remote.owner_client_id,
        });
    }

    (authorized, reload_deliveries, weapon_events)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn run_local_weapon_command(
    registry: &Rc<RefCell<EntityRegistry>>,
    pawn: Option<EntityId>,
    active_wieldable: Option<EntityId>,
    command: &WeaponFireCommand,
    reload_pressed: bool,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    tick_dt: f32,
    on_impact: &mut impl FnMut(&mut EntityRegistry),
) -> (Vec<ReloadDelivery>, Vec<&'static str>) {
    let Some(weapon_id) = active_wieldable else {
        return (Vec::new(), Vec::new());
    };
    let mut registry = registry.borrow_mut();
    let Ok(mut weapon_component) = registry.get_component::<WeaponComponent>(weapon_id).cloned()
    else {
        return (Vec::new(), Vec::new());
    };
    let deliveries = pawn.map_or_else(Vec::new, |pawn| {
        super::reload::tick(
            &mut registry,
            pawn,
            weapon_id,
            &mut weapon_component,
            reload_pressed,
            tick_dt,
        )
    });
    let reload_started = deliveries
        .iter()
        .any(|delivery| delivery.outcome == ReloadOutcome::Started);
    let events = weapon::tick_resolved_component(
        &registry,
        &mut weapon_component,
        command,
        collision_world,
        hit_zone_store,
        anim_time,
        tick_dt,
        reload_started,
    );
    let _ = registry.set_component(weapon_id, weapon_component);
    if let Some(impact) = events.impact.as_ref() {
        weapon::spawn_impact_effect_at(&mut registry, impact.point, impact.normal);
        apply_weapon_impact_damage(&mut registry, active_wieldable, pawn, impact);
        on_impact(&mut registry);
    }
    (deliveries, events.event_names())
}

pub(crate) fn apply_weapon_impact_damage(
    registry: &mut EntityRegistry,
    active_wieldable: Option<EntityId>,
    attacker: Option<EntityId>,
    impact: &weapon::WeaponImpact,
) {
    let (Some(_), weapon::ActivationOutcome::Hit(payload)) = (impact.target, impact.outcome) else {
        return;
    };
    let Some(weapon_id) = active_wieldable else {
        log::warn!("[Weapon] hitscan impact had no active wieldable; dropping damage");
        return;
    };
    let Ok(component) = registry.get_component::<WeaponComponent>(weapon_id) else {
        log::warn!("[Weapon] active wieldable {weapon_id} has no WeaponComponent; dropping damage");
        return;
    };

    let effective = component.effective();
    apply_weapon_impact_damage_with_source(
        registry,
        weapon_id,
        attacker,
        impact,
        effective.credit_source.to_string(),
        payload.amount,
    );
}

pub(crate) fn apply_authorized_weapon_impact_damage(
    registry: &mut EntityRegistry,
    weapon_id: EntityId,
    attacker: Option<EntityId>,
    impact: &weapon::WeaponImpact,
    credit_source: String,
    damage_amount: f32,
) {
    apply_weapon_impact_damage_with_source(
        registry,
        weapon_id,
        attacker,
        impact,
        credit_source,
        damage_amount,
    );
}

fn apply_weapon_impact_damage_with_source(
    registry: &mut EntityRegistry,
    weapon_id: EntityId,
    attacker: Option<EntityId>,
    impact: &weapon::WeaponImpact,
    credit_source: String,
    damage_amount: f32,
) {
    let (Some(target), weapon::ActivationOutcome::Hit(_)) = (impact.target, impact.outcome) else {
        return;
    };
    let source_id = if credit_source.is_empty() {
        log::warn!(
            "[Weapon] active wieldable {weapon_id} resolved an empty credit source; using {UNKNOWN_WEAPON_CREDIT_SOURCE}"
        );
        UNKNOWN_WEAPON_CREDIT_SOURCE.to_string()
    } else {
        credit_source
    };
    let multiplier = impact
        .zone
        .as_deref()
        .and_then(|tag| {
            registry
                .get_component::<HealthComponent>(target)
                .ok()
                .and_then(|health| health.zone_multipliers.get(tag).copied())
        })
        .unwrap_or(1.0);
    let scaled = weapon::DamagePayload {
        amount: damage_amount * multiplier,
    };
    if !scaled.amount.is_finite() {
        log::warn!(
            "[Weapon] scaled damage amount {} is non-finite; dropping damage",
            scaled.amount
        );
        return;
    }
    apply_damage_with_context(
        registry,
        target,
        &scaled,
        DamageContext {
            source_id,
            attacker,
            weapon: Some(weapon_id),
            zone: impact.zone.clone(),
            producer: DamageProducer::InTick,
        },
    );
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
    let deliveries = super::reload::tick(
        registry,
        pawn,
        weapon,
        &mut component,
        reload_pressed,
        tick_dt,
    );
    let _ = registry.set_component(weapon, component);
    deliveries
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    use crate::collision::CollisionWorld;
    use crate::kinematic_mover::MoverTickStateTable;
    use crate::sim::{PostMovementCommand, ReloadDelivery, ReloadOutcome, ShotId, simulate_tick};
    use crate::sim::tests::{remote_command, run_local_only_tick, run_remote_only_tick, sim_command, spawn_reload_pair, trigger_movement, weapon_component};
    use crate::scripting_systems::hit_zones::HitZoneStore;
    use crate::weapon;
    use glam::Vec3;
    use postretro_entities::{AmmoReserve, EntityRegistry, Transform};
    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::components::weapon::WeaponComponent;
    use postretro_foundation::FireMode;
    use postretro_net::wire::NetworkId;
    use postretro_scripting_core::reaction_dispatch::ProgressTracker;

    #[test]
    fn remote_fire_authorizes_shot_and_does_not_damage_by_raycast() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon, target) = {
            let mut registry = registry.borrow_mut();
            let pawn = registry.spawn(Transform::default());
            let weapon = registry.spawn(Transform::default());
            registry
                .set_component(weapon, weapon_component("weapon.test.remote"))
                .unwrap();
            let target = registry.spawn(Transform::default());
            registry
                .set_component(
                    target,
                    HealthComponent {
                        max: 100.0,
                        current: 100.0,
                        hitbox: None,
                        death_handled: false,
                        pending_kill_credit: None,
                        zone_multipliers: Default::default(),
                        contributor_ledger: Default::default(),
                    },
                )
                .unwrap();
            (pawn, weapon, target)
        };

        let shot_id = ShotId::from_parts(NetworkId(42), 9);
        let events = run_remote_only_tick(
            registry.clone(),
            &[remote_command(pawn, Some(weapon), 42, 9, true, false)],
        );

        assert_eq!(events.authorized_shots.len(), 1);
        assert_eq!(events.authorized_shots[0].shot.shot_id, shot_id);
        assert_eq!(events.authorized_shots[0].shot.pawn, pawn);
        assert_eq!(events.authorized_shots[0].shot.fire_tick, 33);
        assert_eq!(events.authorized_shots[0].owner_client_id, 7);
        let registry = registry.borrow();
        let weapon_state = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(weapon_state.cooldown_remaining_ms, 100.0);
        let health = registry.get_component::<HealthComponent>(target).unwrap();
        assert_eq!(health.current, 100.0);
    }

    #[test]
    fn remote_fire_for_two_pawns_updates_only_their_mapped_weapons() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn_a, weapon_a, pawn_b, weapon_b, idle_weapon) = {
            let mut registry = registry.borrow_mut();
            let pawn_a = registry.spawn(Transform::default());
            let weapon_a = registry.spawn(Transform::default());
            registry
                .set_component(weapon_a, weapon_component("weapon.test.a"))
                .unwrap();
            let pawn_b = registry.spawn(Transform::default());
            let weapon_b = registry.spawn(Transform::default());
            registry
                .set_component(weapon_b, weapon_component("weapon.test.b"))
                .unwrap();
            let idle_weapon = registry.spawn(Transform::default());
            registry
                .set_component(idle_weapon, weapon_component("weapon.test.idle"))
                .unwrap();
            (pawn_a, weapon_a, pawn_b, weapon_b, idle_weapon)
        };

        let events = run_remote_only_tick(
            registry.clone(),
            &[
                remote_command(pawn_a, Some(weapon_a), 10, 5, true, false),
                remote_command(pawn_b, Some(weapon_b), 11, 5, true, false),
            ],
        );

        assert_eq!(events.authorized_shots.len(), 2);
        assert_eq!(events.authorized_shots[0].shot.pawn, pawn_a);
        assert_eq!(events.authorized_shots[1].shot.pawn, pawn_b);
        assert_ne!(
            events.authorized_shots[0].shot.shot_id,
            events.authorized_shots[1].shot.shot_id
        );
        let registry = registry.borrow();
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_a)
                .unwrap()
                .cooldown_remaining_ms,
            100.0
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_b)
                .unwrap()
                .cooldown_remaining_ms,
            100.0
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(idle_weapon)
                .unwrap()
                .cooldown_remaining_ms,
            0.0
        );
    }

    #[test]
    fn remote_empty_magazines_surface_each_dry_fire_without_authorizing_shots() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn_a, weapon_a, pawn_b, weapon_b) = {
            let mut registry = registry.borrow_mut();
            let (pawn_a, weapon_a) = spawn_reload_pair(&mut registry, 10, 8, 1000, 0);
            let (pawn_b, weapon_b) = spawn_reload_pair(&mut registry, 10, 8, 1000, 0);
            (pawn_a, weapon_a, pawn_b, weapon_b)
        };

        let events = run_remote_only_tick(
            registry.clone(),
            &[
                remote_command(pawn_a, Some(weapon_a), 42, 9, true, false),
                remote_command(pawn_b, Some(weapon_b), 43, 9, true, false),
            ],
        );

        assert_eq!(events.weapon, vec!["dry_fire", "dry_fire"]);
        assert!(events.authorized_shots.is_empty());
        let registry = registry.borrow();
        for weapon in [weapon_a, weapon_b] {
            assert_eq!(
                registry
                    .get_component::<WeaponComponent>(weapon)
                    .unwrap()
                    .magazine,
                0
            );
        }
    }

    // Regression: the remote Auto path drained dry_fire every fixed tick while held.
    #[test]
    fn remote_empty_auto_weapon_reemits_dry_fire_only_after_cooldown() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon) = {
            let mut registry = registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 1000, 0);
            let mut component = registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .clone();
            component.fire_mode = FireMode::Auto;
            component.cooldown_ms = 45.0;
            registry.set_component(weapon, component).unwrap();
            (pawn, weapon)
        };

        let first = run_remote_only_tick(
            registry.clone(),
            &[remote_command(pawn, Some(weapon), 42, 1, true, false)],
        );
        assert_eq!(first.weapon, vec!["dry_fire"]);
        assert!(first.authorized_shots.is_empty());

        for client_tick in [2, 3] {
            let cooling = run_remote_only_tick(
                registry.clone(),
                &[remote_command(
                    pawn,
                    Some(weapon),
                    42,
                    client_tick,
                    true,
                    false,
                )],
            );
            assert!(cooling.weapon.is_empty());
            assert!(cooling.authorized_shots.is_empty());
        }

        let ready = run_remote_only_tick(
            registry.clone(),
            &[remote_command(pawn, Some(weapon), 42, 4, true, false)],
        );
        assert_eq!(ready.weapon, vec!["dry_fire"]);
        assert!(ready.authorized_shots.is_empty());
        let component = registry
            .borrow()
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        assert_eq!(component.magazine, 0);
        assert!((component.cooldown_remaining_ms - 45.0).abs() <= 1.0e-5);
    }

    #[test]
    fn held_reload_starts_once_and_release_still_advances_to_completion() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 20, 1000, 2);
        let mut cooling = registry
            .get_component::<WeaponComponent>(weapon)
            .unwrap()
            .clone();
        cooling.cooldown_remaining_ms = 123.0;
        registry.set_component(weapon, cooling).unwrap();

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.25),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Started,
            }]
        );
        let started = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(started.reload_remaining_ms, 750);
        assert_eq!(started.reload_total_ms, 1000);
        assert!(started.reload_press_consumed);
        assert_eq!(started.magazine, 2);
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            20
        );

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.25),
            Vec::new()
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .reload_remaining_ms,
            500
        );
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.75),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Completed { transferred: 8 },
            }]
        );
        let completed = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(completed.magazine, 10);
        assert_eq!(completed.reload_remaining_ms, 0);
        assert_eq!(completed.reload_total_ms, 1000);
        assert_eq!(completed.cooldown_remaining_ms, 123.0);
        assert!(!completed.reload_press_consumed);
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            12
        );
    }

    #[test]
    fn reload_completion_atomically_transfers_partial_live_reserve_only_at_zero() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 3, 500, 2);

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.1)[0].outcome,
            ReloadOutcome::Started
        );
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.399),
            Vec::new()
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .magazine,
            2
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            3
        );

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.0011),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Completed { transferred: 3 },
            }]
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .magazine,
            5
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            0
        );
    }

    #[test]
    fn reload_start_tick_advances_and_can_complete_immediately() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.1),
            vec![
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::Started,
                },
                ReloadDelivery {
                    pawn,
                    weapon,
                    outcome: ReloadOutcome::Completed { transferred: 8 },
                },
            ]
        );
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.reload_total_ms, 100);
        assert_eq!(component.reload_remaining_ms, 0);
        assert_eq!(component.magazine, 10);
    }

    #[test]
    fn fractional_reload_elapsed_completes_one_second_at_sixty_hz_without_drift() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 1000, 2);

        for tick in 0..59 {
            let deliveries =
                deliver_reload_to_weapon(&mut registry, pawn, weapon, tick == 0, 1.0 / 60.0);
            assert!(
                !deliveries
                    .iter()
                    .any(|delivery| matches!(delivery.outcome, ReloadOutcome::Completed { .. }))
            );
        }
        assert!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .reload_remaining_ms
                > 0
        );

        let completion = deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 1.0 / 60.0)
            .iter()
            .any(|delivery| {
                matches!(
                    delivery.outcome,
                    ReloadOutcome::Completed { transferred: 8 }
                )
            });
        assert!(completion);
    }

    #[test]
    fn reload_timer_ignores_invalid_delta_and_saturates_huge_delta() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 1000, 2);

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, f32::NAN),
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Started,
            }]
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .reload_remaining_ms,
            1000
        );
        assert!(deliver_reload_to_weapon(&mut registry, pawn, weapon, false, -1.0).is_empty());
        assert!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, f32::INFINITY).is_empty()
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .reload_remaining_ms,
            1000
        );
        assert!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, f32::MAX)
                .iter()
                .any(|delivery| matches!(delivery.outcome, ReloadOutcome::Completed { .. }))
        );
    }

    #[test]
    fn reload_fresh_press_reports_full_and_empty_blocks_without_starting_timer() {
        let mut registry = EntityRegistry::new();
        let (full_pawn, full_weapon) = spawn_reload_pair(&mut registry, 10, 20, 900, 10);
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, full_pawn, full_weapon, true, 0.1)[0].outcome,
            ReloadOutcome::BlockedFull
        );
        let full = registry
            .get_component::<WeaponComponent>(full_weapon)
            .unwrap();
        assert_eq!(full.reload_remaining_ms, 0);
        assert_eq!(full.reload_total_ms, 0);
        assert_eq!(full.magazine, 10);

        let (empty_pawn, empty_weapon) = spawn_reload_pair(&mut registry, 10, 0, 900, 2);
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, empty_pawn, empty_weapon, true, 0.1)[0].outcome,
            ReloadOutcome::BlockedEmpty
        );
        let empty = registry
            .get_component::<WeaponComponent>(empty_weapon)
            .unwrap();
        assert_eq!(empty.reload_remaining_ms, 0);
        assert_eq!(empty.reload_total_ms, 0);
        assert_eq!(empty.magazine, 2);
    }

    #[test]
    fn fresh_reload_press_mid_reload_is_silent_and_does_not_restart_timer() {
        let mut registry = EntityRegistry::new();
        let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 20, 1000, 2);
        assert!(!deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.1).is_empty());
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.1),
            Vec::new()
        );
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.1),
            Vec::new()
        );
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.reload_remaining_ms, 700);
        assert_eq!(component.reload_total_ms, 1000);
    }

    #[test]
    fn resourceless_weapon_cannot_reload_and_release_clears_edge_state() {
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        let weapon = registry.spawn(Transform::default());
        registry
            .set_component(weapon, weapon_component("weapon.test.unlimited"))
            .unwrap();

        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, true, 0.1),
            Vec::new()
        );
        assert!(
            registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .reload_press_consumed
        );
        assert_eq!(
            deliver_reload_to_weapon(&mut registry, pawn, weapon, false, 0.1),
            Vec::new()
        );
        assert!(
            !registry
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .reload_press_consumed
        );
    }

    #[test]
    fn local_reload_routes_to_local_pawn_reserve_before_fire() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon) = {
            let mut registry = registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 1000, 2);
            registry.set_component(pawn, trigger_movement()).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();
            (pawn, weapon)
        };
        let world = CollisionWorld::new();
        let hit_zones = HitZoneStore::new();
        let mut progress = ProgressTracker::new();
        let mut ai_runtime = crate::scripting_systems::ai::AiRuntime::new();
        let mut mover_states = MoverTickStateTable::default();

        let events = simulate_tick(
            registry.clone(),
            &world,
            &hit_zones,
            None,
            -9.81,
            Some(weapon),
            0.0,
            &mut progress,
            &mut ai_runtime,
            &[],
            &mut mover_states,
            &[],
            &sim_command(true, true),
            |_| PostMovementCommand {
                aim_origin: Vec3::ZERO,
                aim_direction: Vec3::NEG_Z,
            },
            0.25,
            None,
            |_| {},
        );

        assert_eq!(
            events.reload_deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Started,
            }]
        );
        assert!(
            events.weapon.is_empty(),
            "reload start must block same-tick fire"
        );
        let registry = registry.borrow();
        let weapon = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(weapon.reload_remaining_ms, 750);
        assert_eq!(weapon.magazine, 2);
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            8
        );
    }

    #[test]
    fn immediate_local_reload_still_blocks_fire_for_start_tick() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon) = {
            let mut registry = registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 10, 2);
            registry.set_component(pawn, trigger_movement()).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();
            (pawn, weapon)
        };
        let events = run_local_only_tick(
            registry.clone(),
            weapon,
            &sim_command(true, true),
            1.0 / 60.0,
        );

        assert!(events.weapon.is_empty());
        assert!(events.reload_deliveries.iter().any(|delivery| {
            delivery.pawn == pawn && delivery.outcome == ReloadOutcome::Started
        }));
        assert!(events.reload_deliveries.iter().any(|delivery| {
            delivery.pawn == pawn
                && matches!(
                    delivery.outcome,
                    ReloadOutcome::Completed { transferred: 8 }
                )
        }));
        assert_eq!(
            registry
                .borrow()
                .get_component::<WeaponComponent>(weapon)
                .unwrap()
                .magazine,
            10
        );
    }

    #[test]
    fn local_reload_completion_refills_before_same_tick_fire() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon) = {
            let mut registry = registry.borrow_mut();
            let (pawn, weapon) = spawn_reload_pair(&mut registry, 10, 8, 100, 2);
            registry.set_component(pawn, trigger_movement()).unwrap();
            registry.mark_local_player_pawn(pawn).unwrap();
            (pawn, weapon)
        };

        let started =
            run_local_only_tick(registry.clone(), weapon, &sim_command(false, true), 0.04);
        assert_eq!(
            started.reload_deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Started,
            }]
        );
        let advancing =
            run_local_only_tick(registry.clone(), weapon, &sim_command(false, false), 0.04);
        assert!(advancing.reload_deliveries.is_empty());

        // Completion is not a new reload start: transfer settles before fire
        // authorization, so this tick may spend from the refilled magazine.
        let completed_and_fired =
            run_local_only_tick(registry.clone(), weapon, &sim_command(true, false), 0.021);
        assert_eq!(
            completed_and_fired.reload_deliveries,
            vec![ReloadDelivery {
                pawn,
                weapon,
                outcome: ReloadOutcome::Completed { transferred: 8 },
            }]
        );
        assert_eq!(completed_and_fired.weapon, vec!["activate"]);
        let registry = registry.borrow();
        let component = registry.get_component::<WeaponComponent>(weapon).unwrap();
        assert_eq!(component.reload_remaining_ms, 0);
        assert_eq!(component.magazine, 9);
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn)
                .unwrap()
                .available("bullets.light"),
            0
        );
    }

    #[test]
    fn immediate_remote_reload_still_blocks_fire_for_start_tick() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn, weapon) = {
            let mut registry = registry.borrow_mut();
            spawn_reload_pair(&mut registry, 10, 8, 10, 2)
        };

        let events = run_remote_only_tick(
            registry,
            &[remote_command(pawn, Some(weapon), 42, 9, true, true)],
        );

        assert!(events.authorized_shots.is_empty());
        assert!(events.weapon.is_empty());
        assert!(
            events
                .reload_deliveries
                .iter()
                .any(|delivery| { delivery.outcome == ReloadOutcome::Started })
        );
        assert!(events.reload_deliveries.iter().any(|delivery| {
            matches!(
                delivery.outcome,
                ReloadOutcome::Completed { transferred: 8 }
            )
        }));
    }

    #[test]
    fn remote_reload_delivery_routes_to_mapped_weapon_and_pawn_reserve_only() {
        let registry = Rc::new(RefCell::new(EntityRegistry::new()));
        let (pawn_a, weapon_a, pawn_b, weapon_b) = {
            let mut registry = registry.borrow_mut();
            let (pawn_a, weapon_a) = spawn_reload_pair(&mut registry, 10, 8, 1000, 2);
            let (pawn_b, weapon_b) = spawn_reload_pair(&mut registry, 10, 20, 1000, 4);
            (pawn_a, weapon_a, pawn_b, weapon_b)
        };

        let events = run_remote_only_tick(
            registry.clone(),
            &[
                remote_command(pawn_a, Some(weapon_a), 10, 5, false, true),
                remote_command(pawn_b, Some(weapon_b), 11, 5, false, false),
            ],
        );

        assert_eq!(
            events.reload_deliveries,
            vec![ReloadDelivery {
                pawn: pawn_a,
                weapon: weapon_a,
                outcome: ReloadOutcome::Started,
            }]
        );
        let registry = registry.borrow();
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn_a)
                .unwrap()
                .available("bullets.light"),
            8
        );
        assert_eq!(
            registry
                .get_component::<AmmoReserve>(pawn_b)
                .unwrap()
                .available("bullets.light"),
            20
        );
        assert_eq!(
            registry
                .get_component::<WeaponComponent>(weapon_b)
                .unwrap()
                .reload_remaining_ms,
            0
        );
    }

    #[test]
    fn weapon_impact_damage_records_effective_source_weapon_zone_and_scaled_payload() {
        let mut registry = EntityRegistry::new();
        let weapon_id = registry.spawn(Transform::default());
        registry
            .set_component(weapon_id, weapon_component("weapon.test.rifle"))
            .unwrap();

        let target = registry.spawn(Transform::default());
        let mut health = HealthComponent {
            max: 100.0,
            current: 100.0,
            hitbox: None,
            death_handled: false,
            pending_kill_credit: None,
            zone_multipliers: Default::default(),
            contributor_ledger: Default::default(),
        };
        health.zone_multipliers.insert("head".to_string(), 2.5);
        registry.set_component(target, health).unwrap();

        let impact = weapon::WeaponImpact {
            point: Vec3::ZERO,
            normal: Vec3::Y,
            target: Some(target),
            zone: Some("head".to_string()),
            outcome: weapon::ActivationOutcome::Hit(weapon::DamagePayload { amount: 10.0 }),
        };

        let attacker = Some(registry.spawn(Transform::default()));
        apply_weapon_impact_damage(&mut registry, Some(weapon_id), attacker, &impact);

        let health = registry.get_component::<HealthComponent>(target).unwrap();
        assert_eq!(health.current, 75.0);
        let entries = health.contributor_ledger.entries();
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry.source_id, "weapon.test.rifle");
        assert_eq!(entry.accumulated_damage, 25.0);
        assert_eq!(entry.last_hit_damage, 25.0);
        assert_eq!(entry.last_hit_zone.as_deref(), Some("head"));
        assert_eq!(entry.last_weapon, Some(weapon_id));
        assert_eq!(entry.last_attacker, attacker);
    }

    #[test]
    fn weapon_impact_damage_skips_non_finite_scaled_payload() {
        let mut registry = EntityRegistry::new();
        let weapon_id = registry.spawn(Transform::default());
        registry
            .set_component(weapon_id, weapon_component("weapon.test.rifle"))
            .unwrap();

        let target = registry.spawn(Transform::default());
        let mut health = HealthComponent {
            max: 100.0,
            current: 100.0,
            hitbox: None,
            death_handled: false,
            pending_kill_credit: None,
            zone_multipliers: Default::default(),
            contributor_ledger: Default::default(),
        };
        health.zone_multipliers.insert("over".to_string(), 2.0);
        registry.set_component(target, health).unwrap();

        let impact = weapon::WeaponImpact {
            point: Vec3::ZERO,
            normal: Vec3::Y,
            target: Some(target),
            zone: Some("over".to_string()),
            outcome: weapon::ActivationOutcome::Hit(weapon::DamagePayload { amount: f32::MAX }),
        };

        apply_weapon_impact_damage(&mut registry, Some(weapon_id), None, &impact);

        let health = registry.get_component::<HealthComponent>(target).unwrap();
        assert_eq!(health.current, 100.0);
        assert!(health.contributor_ledger.entries().is_empty());
        assert!(health.contributor_ledger.overflow().is_none());
    }
}
