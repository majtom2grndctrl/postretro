//! Host-authoritative projectile traps: the shared request API and the fixed-tick trap stage.
//! See: context/lib/entity_model.md §Components (ProjectileTrap) · context/lib/scripting.md §12

pub mod commands;
pub mod install;

use std::sync::Arc;

use glam::Vec3;
use postretro_entities::components::projectile_trap::{
    TrapPending, TrapRequest, TrapRuntime, TrapSchedule,
};
use postretro_entities::{
    ComponentKind, ComponentValue, Emitter, EntityId, EntityRegistry, Transform, WeaponEmission,
};
use postretro_foundation::{ActivationInput, ActivationLane, ActivationToken};

use crate::scripting_systems::reaction_scheduler::ms_to_ticks;
use crate::sim::{EnemyProjectilePresentationSpawn, ProjectileSource, spawn_projectile};
use crate::weapon::execution::{
    ActivationCommand, WeaponActivationCheckpoint, advance_weapon_activation,
};
use crate::weapon::{FireButtonState, ProjectileLaunch, freeze_weapon_shot};

/// Fold `request` into a trap's pending set so the set applies exactly as the
/// issued sequence would. `fire` is a flag: start and stop never cancel it. A
/// `stop` discards an earlier `start`, and once a `start` is pending a later
/// one is a no-op, as `start` on a running trap is.
pub(crate) fn fold_trap_request(pending: &mut TrapPending, request: TrapRequest) {
    match request {
        TrapRequest::Fire => pending.fire = true,
        TrapRequest::Stop => {
            pending.stop = true;
            pending.start = None;
        }
        TrapRequest::Start {
            interval_ms,
            phase_ms,
        } => {
            if pending.start.is_none() {
                pending.start = Some((interval_ms, phase_ms));
            }
        }
    }
}

/// Queue one request on `trap` for the next trap stage. The caller owns the
/// host-only gate; this never runs on a connected client. An inert trap drops
/// the request, and a target that is not a trap warns and skips.
pub fn issue_trap_request(registry: &mut EntityRegistry, trap: EntityId, request: TrapRequest) {
    match registry.get_component_value_mut(trap, ComponentKind::ProjectileTrap) {
        Ok(ComponentValue::ProjectileTrap(component)) => {
            if component.is_armed() {
                fold_trap_request(&mut component.pending, request);
            }
        }
        _ => log::warn!("[Trap] command target {trap:?} is not a projectile trap; skipping"),
    }
}

/// Work one trap stage produced: the projectiles it launched (for the launch
/// tick's spawn grace), their presentation mirrors, and the `activate` weapon
/// emissions carrying each shot's id.
#[derive(Debug, Default)]
pub struct TrapTickOutput {
    pub projectile_spawns: Vec<EnemyProjectilePresentationSpawn>,
    pub emissions: Vec<WeaponEmission>,
}

/// Phase rounds up to whole ticks like `wait`, without its one-tick floor.
fn phase_ticks(phase_ms: f64) -> u32 {
    if phase_ms > 0.0 {
        ms_to_ticks(phase_ms)
    } else {
        0
    }
}

/// A shot one trap's weapon produced this tick, awaiting its projectile.
struct DueShot {
    trap: EntityId,
    canonical_weapon: String,
    shot: crate::weapon::ResolvedWeaponShot,
    /// Restored if the projectile cannot materialize.
    checkpoint: WeaponActivationCheckpoint,
}

/// Run every trap once for this authoritative tick, after the tick's trigger
/// and teleport requests were issued. Each trap applies its pending requests,
/// takes its due check, then advances its weapon one tick whether or not a
/// request arrived, so executions continue and recovery drains. The pass
/// walks the trap column in place; only a tick that shoots allocates.
pub fn run_trap_stage(registry: &mut EntityRegistry, tick_dt: f32) -> TrapTickOutput {
    let dt_ms = tick_dt.max(0.0) * 1000.0;
    let mut due = Vec::new();
    registry.for_each_with_kind_mut(ComponentKind::ProjectileTrap, |trap, value| {
        let ComponentValue::ProjectileTrap(component) = value else {
            return;
        };
        let pending = std::mem::take(&mut component.pending);
        let (interval_ms, phase_ms) = (component.interval_ms, component.phase_ms);
        // An inert trap has no runtime and drops what it was sent.
        let Some(runtime) = component.runtime.as_mut() else {
            return;
        };
        apply_pending(interval_ms, phase_ms, pending, runtime);
        let mut fire = pending.fire;
        if let Some(schedule) = runtime.schedule.as_mut()
            && schedule.countdown_ticks == 0
        {
            fire = true;
            schedule.countdown_ticks = schedule.interval_ticks;
        }
        if let Some(shot) = advance_trap_weapon(trap, runtime, fire, dt_ms) {
            due.push(shot);
        }
        if let Some(schedule) = runtime.schedule.as_mut() {
            schedule.countdown_ticks = schedule.countdown_ticks.saturating_sub(1);
        }
        runtime.clock = runtime.clock.wrapping_add(1);
    });
    let mut output = TrapTickOutput::default();
    for shot in due {
        launch_trap_shot(registry, shot, &mut output);
    }
    output
}

/// Apply the pending set: `stop` first, then `start`. `start` on a running
/// trap is a no-op; one that applies anchors its schedule on this stage.
fn apply_pending(interval_ms: f64, phase_ms: f64, pending: TrapPending, runtime: &mut TrapRuntime) {
    if pending.stop {
        runtime.schedule = None;
    }
    if let Some((run_interval_ms, run_phase_ms)) = pending.start
        && runtime.schedule.is_none()
    {
        runtime.schedule = Some(TrapSchedule {
            interval_ticks: ms_to_ticks(run_interval_ms.unwrap_or(interval_ms)),
            countdown_ticks: phase_ticks(run_phase_ms.unwrap_or(phase_ms)),
        });
    }
}

/// One weapon tick. A `fire` initiates an execution only where the shared gate
/// allows a start: a running execution or owed recovery drops it.
fn advance_trap_weapon(
    trap: EntityId,
    runtime: &mut TrapRuntime,
    fire: bool,
    dt_ms: f32,
) -> Option<DueShot> {
    let mut input = ActivationInput::default();
    if fire {
        input.initiation = Some(ActivationToken {
            start_tick: runtime.clock,
            lane: ActivationLane::Primary,
        });
    }
    let executing = runtime.weapon.state.activation_cursor().is_some();
    let checkpoint = WeaponActivationCheckpoint::capture(&runtime.weapon);
    let command = ActivationCommand {
        tick: runtime.clock,
        pawn: trap.to_raw(),
        real_command: true,
        input,
        controller_starts: false,
        primary: FireButtonState {
            pressed: fire,
            active: fire || executing,
        },
        secondary: FireButtonState {
            pressed: false,
            active: false,
        },
    };
    let attempt =
        advance_weapon_activation(&mut runtime.weapon, command, dt_ms, false, true).shot?;
    Some(DueShot {
        trap,
        canonical_weapon: runtime.canonical_weapon.clone(),
        shot: freeze_weapon_shot(&runtime.weapon, attempt),
        checkpoint,
    })
}

/// Launch one shot from its trap's origin along its facing. A shot whose
/// projectile cannot materialize restores the weapon's shot transaction.
fn launch_trap_shot(registry: &mut EntityRegistry, due: DueShot, output: &mut TrapTickOutput) {
    let DueShot {
        trap,
        canonical_weapon,
        shot,
        checkpoint,
    } = due;
    let pose = registry
        .get_component::<Transform>(trap)
        .copied()
        .unwrap_or_default();
    let spawned = shot.projectile.as_ref().and_then(|projectile| {
        let direction = (pose.rotation * Vec3::NEG_Z).normalize_or(Vec3::NEG_Z);
        let launch = ProjectileLaunch {
            sounds: Some(shot.sounds.clone()),
            origin: pose.position,
            direction,
            speed: projectile.speed,
            radius: projectile.radius,
            range: shot.activation.values.range,
            lifetime: projectile.lifetime_ms / 1000.0,
            damage: shot.activation.values.damage,
            knockback_impulse: shot.knockback.map_or(Vec3::ZERO, |push| {
                postretro_foundation::knockback_impulse(push.speed, push.upward_bias, direction)
            }),
            credit_source: shot.credit_source.clone(),
            descriptor: projectile.clone(),
            splash: shot.splash.clone(),
            action: Some(Arc::clone(shot.action())),
            shot_id: Some(shot.activation.shot_id),
            model_scale: shot.projectile_model_scale,
        };
        let source = ProjectileSource {
            weapon: Some(canonical_weapon.clone()),
            activation: None,
            attackerless: true,
        };
        spawn_projectile(registry, trap, trap, launch, None, source)
    });
    // Install admits projectile weapons only, so a missing body and an
    // exhausted registry are the same failure: nothing launched.
    let Some(spawned) = spawned else {
        if let Ok(ComponentValue::ProjectileTrap(component)) =
            registry.get_component_value_mut(trap, ComponentKind::ProjectileTrap)
            && let Some(runtime) = component.runtime.as_mut()
        {
            checkpoint.restore(&mut runtime.weapon);
        }
        return;
    };
    output
        .projectile_spawns
        .push(EnemyProjectilePresentationSpawn {
            projectile: spawned,
            descriptor_class: canonical_weapon.clone(),
        });
    output.emissions.push(WeaponEmission {
        sounds: Some(shot.sounds.clone()),
        action: Some(Arc::clone(shot.action())),
        shot_id: Some(shot.activation.shot_id),
        address: "activate",
        emitter: Emitter::Entity {
            id: trap,
            origin: pose.position,
        },
        weapon: Some(canonical_weapon),
    });
}

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod trap_test_fixtures;
