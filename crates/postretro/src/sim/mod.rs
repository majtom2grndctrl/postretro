// Headless fixed-tick game-state advance seam.
// See: context/lib/entity_model.md §5
// See: context/plans/in-progress/M15--p0-headless-sim-seam/index.md  (command shapes, four-bucket event return, host-callback protocol)

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use glam::Vec3;

use crate::agent_steering;
use crate::collision::CollisionWorld;
use crate::movement::{MovementInput, tick as movement_tick};
use crate::nav::NavGraph;
use crate::scripting::components::health::apply_damage;
use crate::scripting::components::player_movement::PlayerMovementComponent;
use crate::scripting::reaction_dispatch::ProgressTracker;
use crate::scripting::registry::{
    ComponentKind, EntityId, EntityRegistry, Transform,
};
use crate::scripting_systems;
use crate::scripting_systems::hit_zones::HitZoneStore;
use crate::weapon::{self, FireButtonState, WeaponFireCommand};

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
    command: &SimCommand,
    mut post_movement: impl FnMut(&Rc<RefCell<EntityRegistry>>) -> PostMovementCommand,
    tick_dt: f32,
) -> TickEvents {
    registry.borrow_mut().snapshot_transforms();

    let movement = run_movement_tick(
        &registry,
        collision_world,
        gravity,
        &command.movement,
        tick_dt,
    );
    let ai = {
        let mut registry = registry.borrow_mut();
        scripting_systems::ai::run_ai_tick(&mut registry, ai_warned, tick_dt)
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

#[cfg(test)]
mod determinism_tests;
#[cfg(test)]
mod divergence_spike_tests;
#[cfg(any(test, feature = "dev-tools"))]
pub(crate) mod predict_reconcile;

fn run_movement_tick(
    registry: &Rc<RefCell<EntityRegistry>>,
    collision_world: &CollisionWorld,
    gravity: f32,
    input: &MovementInput,
    tick_dt: f32,
) -> Vec<&'static str> {
    let mut events_out: Vec<&'static str> = Vec::new();
    let mut snapshots: Vec<(EntityId, PlayerMovementComponent, Vec3)> = Vec::new();
    {
        let registry = registry.borrow();
        if let Some(id) = local_movement_pawn(&registry) {
            if let (Ok(component), Ok(transform)) = (
                registry.get_component::<PlayerMovementComponent>(id),
                registry.get_component::<Transform>(id),
            ) {
                snapshots.push((id, component.clone(), transform.position));
            }
        }
    }

    let mut registry = registry.borrow_mut();
    for (id, mut component, position) in snapshots {
        let (new_pos, events) = movement_tick(
            &mut component,
            input,
            collision_world,
            gravity,
            tick_dt,
            position,
        );
        if let Ok(transform) = registry.get_component::<Transform>(id) {
            let mut t = *transform;
            t.position = new_pos;
            let _ = registry.set_component(id, t);
        }
        let _ = registry.set_component(id, component);
        if events.landed {
            events_out.push("landed");
        }
        if events.jumped {
            events_out.push("jumped");
        }
    }

    events_out
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
        if let (Some(target), weapon::ActivationOutcome::Hit(payload)) =
            (impact.target, impact.outcome)
        {
            let multiplier = impact
                .zone
                .as_deref()
                .and_then(|tag| {
                    registry
                        .get_component::<crate::scripting::components::health::HealthComponent>(
                            target,
                        )
                        .ok()
                        .and_then(|health| health.zone_multipliers.get(tag).copied())
                })
                .unwrap_or(1.0);
            let scaled = weapon::DamagePayload {
                amount: payload.amount * multiplier,
            };
            apply_damage(&mut registry, target, &scaled);
        }
    }
    events.event_names()
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
