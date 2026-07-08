// Headless fixed-tick game-state advance seam.
// See: context/lib/entity_model.md §5
// See: context/plans/in-progress/M15--p0-headless-sim-seam/index.md  (command shapes, four-bucket event return, host-callback protocol)

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use glam::Vec3;

use crate::agent_steering;
use crate::collision::CollisionWorld;
use crate::collision::moving::{CombinedCollisionWorld, MoverCollider};
use crate::kinematic_mover::{self, MoverTickStateTable};
use crate::movement::MovementInput;
use crate::nav::NavGraph;
use crate::scripting_systems;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::weapon::{self, FireButtonState, WeaponFireCommand};
use postretro_entities::components::health::{
    DamageContext, HealthComponent, apply_damage_with_context,
};
use postretro_entities::components::weapon::{UNKNOWN_WEAPON_CREDIT_SOURCE, WeaponComponent};
use postretro_entities::{ComponentKind, EntityId, EntityRegistry};
use postretro_scripting_core::reaction_dispatch::ProgressTracker;

#[derive(Debug, Clone)]
pub(crate) struct SimCommand {
    pub(crate) movement: MovementInput,
    pub(crate) fire_button: FireButtonState,
}

pub(crate) struct PostMovementCommand {
    pub(crate) aim_origin: Vec3,
    pub(crate) aim_direction: Vec3,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TickEvents {
    pub(crate) movement: Vec<&'static str>,
    pub(crate) ai: Vec<&'static str>,
    pub(crate) weapon: Vec<&'static str>,
    pub(crate) death: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn simulate_tick(
    registry: Rc<RefCell<EntityRegistry>>,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    nav_graph: Option<&NavGraph>,
    gravity: f32,
    active_wieldable: Option<EntityId>,
    anim_time: f64,
    progress_tracker: &mut ProgressTracker,
    ai_warned: &mut HashSet<String>,
    mover_colliders: &[MoverCollider],
    mover_tick_states: &mut MoverTickStateTable,
    remote_pawn_inputs: &[(EntityId, MovementInput)],
    command: &SimCommand,
    mut post_movement: impl FnMut(&Rc<RefCell<EntityRegistry>>) -> PostMovementCommand,
    tick_dt: f32,
) -> TickEvents {
    registry.borrow_mut().snapshot_transforms();

    {
        let mut registry = registry.borrow_mut();
        kinematic_mover::run_kinematic_mover_tick(&mut registry, mover_tick_states, tick_dt);
    }

    let combined_collision =
        CombinedCollisionWorld::new(collision_world, mover_colliders, mover_tick_states);
    let mut movement = {
        let mut registry = registry.borrow_mut();
        host_movement::run_host_movement_tick(
            &mut registry,
            &combined_collision,
            gravity,
            remote_pawn_inputs,
            tick_dt,
        )
    };
    movement.extend(run_movement_tick(
        &registry,
        &combined_collision,
        gravity,
        &command.movement,
        tick_dt,
    ));
    let ai = {
        let mut registry = registry.borrow_mut();
        scripting_systems::ai::run_ai_tick_with_navigation(
            &mut registry,
            ai_warned,
            tick_dt,
            nav_graph,
            Some(collision_world),
        )
    };

    let post_movement_command = post_movement(&registry);

    {
        let mut registry = registry.borrow_mut();
        // AgentTickResult only carries a diagnostic `replans` counter, not observable sim state, so the return value is intentionally discarded.
        let _ = agent_steering::tick(&mut registry, collision_world, nav_graph, gravity, tick_dt);
    }

    let weapon_fire = weapon_fire_command(command.fire_button, post_movement_command);
    let weapon = run_weapon_fire_tick(
        &registry,
        active_wieldable,
        &weapon_fire,
        collision_world,
        hit_zone_store,
        anim_time,
        tick_dt,
    );
    let death = run_death_sweep(&registry, progress_tracker);

    TickEvents {
        movement,
        ai,
        weapon,
        death,
    }
}

mod host_movement;

#[cfg(test)]
pub(crate) use host_movement::run_host_movement_tick;

#[cfg(test)]
mod determinism_tests;
#[cfg(test)]
mod divergence_spike_tests;
#[cfg(any(test, feature = "dev-tools"))]
pub(crate) mod predict_reconcile;

/// Single-player / single-pawn movement stage. Resolves the local movement pawn via
/// the registry marker, then drives it through the shared host multi-pawn seam
/// (`host_movement::run_host_movement_tick`) with a one-element input list. The host
/// netcode path bypasses this entirely and calls the seam directly with EVERY
/// authoritative pawn (Task 4) — `local_movement_pawn` is the single-player resolver
/// only, never the authoritative-host resolver.
fn run_movement_tick(
    registry: &Rc<RefCell<EntityRegistry>>,
    collision: &impl crate::movement::MovementCollisionSource,
    gravity: f32,
    input: &MovementInput,
    tick_dt: f32,
) -> Vec<&'static str> {
    let local = {
        let registry = registry.borrow();
        local_movement_pawn(&registry)
    };
    let Some(id) = local else {
        return Vec::new();
    };

    let pawn_inputs = [(id, input.clone())];
    let mut registry = registry.borrow_mut();
    host_movement::run_host_movement_tick(&mut registry, collision, gravity, &pawn_inputs, tick_dt)
}

/// Resolve the local movement pawn: registry marker first, then first
/// `PlayerMovement` entity. See also `followed_player_pawn` (main.rs)
/// and `player_position` (scripting/systems/ai.rs).
fn local_movement_pawn(registry: &EntityRegistry) -> Option<EntityId> {
    if let Some(id) = registry.local_player_pawn() {
        if matches!(
            registry.has_component_kind(id, ComponentKind::PlayerMovement),
            Ok(true)
        ) {
            return Some(id);
        }
    }

    registry
        .iter_with_kind(ComponentKind::PlayerMovement)
        .next()
        .map(|(id, _)| id)
}

fn weapon_fire_command(
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

#[allow(clippy::too_many_arguments)]
fn run_weapon_fire_tick(
    registry: &Rc<RefCell<EntityRegistry>>,
    active_wieldable: Option<EntityId>,
    command: &WeaponFireCommand,
    collision_world: &CollisionWorld,
    hit_zone_store: &HitZoneStore,
    anim_time: f64,
    tick_dt: f32,
) -> Vec<&'static str> {
    let mut registry = registry.borrow_mut();
    let events = weapon::tick_resolved(
        &mut registry,
        active_wieldable,
        command,
        collision_world,
        hit_zone_store,
        anim_time,
        tick_dt,
    );
    if let Some(impact) = events.impact.as_ref() {
        weapon::spawn_impact_effect_at(&mut registry, impact.point, impact.normal);
        apply_weapon_impact_damage(&mut registry, active_wieldable, impact);
    }
    events.event_names()
}

fn apply_weapon_impact_damage(
    registry: &mut EntityRegistry,
    active_wieldable: Option<EntityId>,
    impact: &weapon::WeaponImpact,
) {
    let (Some(target), weapon::ActivationOutcome::Hit(payload)) = (impact.target, impact.outcome)
    else {
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
    let source_id = if effective.credit_source.is_empty() {
        log::warn!(
            "[Weapon] active wieldable {weapon_id} resolved an empty credit source; using {UNKNOWN_WEAPON_CREDIT_SOURCE}"
        );
        UNKNOWN_WEAPON_CREDIT_SOURCE.to_string()
    } else {
        effective.credit_source
    };
    let attacker = local_movement_pawn(registry);
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
        amount: payload.amount * multiplier,
    };
    apply_damage_with_context(
        registry,
        target,
        &scaled,
        DamageContext {
            source_id,
            attacker,
            weapon: Some(weapon_id),
            zone: impact.zone.clone(),
        },
    );
}

fn run_death_sweep(
    registry: &Rc<RefCell<EntityRegistry>>,
    progress_tracker: &mut ProgressTracker,
) -> Vec<String> {
    let report = {
        let mut registry = registry.borrow_mut();
        scripting_systems::health::sweep_deaths(&mut registry)
    };

    let mut events = Vec::new();
    for tags in &report.killed_tags {
        events.extend(progress_tracker.on_entity_killed(tags));
    }
    if report.player_died {
        events.push(scripting_systems::health::PLAYER_DIED_EVENT.to_string());
    }
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::Transform;
    use postretro_foundation::{FireMode, ResolutionMode, WeaponDescriptor};

    fn weapon_component(credit_source: &str) -> WeaponComponent {
        WeaponComponent::from_descriptor(&WeaponDescriptor {
            damage: 10.0,
            range: 100.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            credit_source: Some(credit_source.to_string()),
        })
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

        apply_weapon_impact_damage(&mut registry, Some(weapon_id), &impact);

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
        assert_eq!(entry.last_attacker, None);
    }
}
