// Per-state velocity intents for player movement.
// See: context/lib/movement.md §4

use glam::{Vec2, Vec3};

use crate::movement::MovementInput;
use crate::movement::substrate::pm_accelerate;
use postretro_foundation::PlayerMovementComponent;

mod crouching;
mod dash;
mod normal;
mod slide;
mod swim;
mod vault;

pub(super) use crouching::crouching_intent;
#[cfg(test)]
pub(super) use dash::DASH_MAX_MS;
pub(super) use dash::dash_intent;
pub(super) use normal::normal_intent;
#[cfg(test)]
pub(super) use slide::SLIDE_MAX_MS;
pub(super) use slide::sliding_intent;
pub(super) use swim::{fluid_jump_input, leave_swim, swim_intent};
pub(super) use vault::vault_intent;

/// Ground friction on a horizontal velocity, in place. Each tick removes
/// `max(speed, stop_speed) * friction * dt`, floored at zero: exponential decay
/// above `stop_speed`, then a constant deceleration that ends at exactly zero.
/// A `stop_speed` of zero is pure exponential decay. Uses:
///   - [`ground_locomotion`]: every grounded tick of `Normal` and `Crouching`,
///     with the descriptor's stop speed.
///   - `dash_intent`'s `dash_drag == 0` grounded boost bleed and the push drift
///     layer's grounded decay (`push::decay_drift`), both with no stop speed:
///     the boost is above the cap, and a gentle steady drift must survive as
///     push / friction rather than being braked to zero.
pub(super) fn apply_ground_friction(velocity: &mut Vec3, friction: f32, stop_speed: f32, dt: f32) {
    let speed = Vec2::new(velocity.x, velocity.z).length();
    if speed > 0.0 {
        let drop = speed.max(stop_speed) * friction * dt;
        let scale = (speed - drop).max(0.0) / speed;
        velocity.x *= scale;
        velocity.z *= scale;
    }
}

/// Grounded locomotion for `Normal` and `Crouching`: friction, then
/// acceleration toward the wish (Quake's `PM_Friction` then `PM_Accelerate`).
/// Friction runs whether or not input is held, so a turn sheds the old heading's
/// momentum. Acceleration may turn the velocity but never lift ground speed past
/// `wish_speed`, or past the speed friction left when that was already higher:
/// ground speed cannot climb by sweeping the aim, and speed banked above the cap
/// (a dash or slide exit) only bleeds. The airborne path is untouched, so
/// bunny-hopping keeps its carried speed.
pub(super) fn ground_locomotion(
    component: &mut PlayerMovementComponent,
    wish_dir_3d: Vec3,
    wish_speed: f32,
    control: f32,
    dt: f32,
) {
    let ground = &component.ground_params;
    let (friction, stop_speed, accel) = (ground.friction, ground.stop_speed, ground.accel);
    apply_ground_friction(&mut component.velocity, friction, stop_speed, dt);
    if wish_dir_3d.length_squared() == 0.0 {
        return;
    }
    let limit = Vec2::new(component.velocity.x, component.velocity.z)
        .length()
        .max(wish_speed);
    pm_accelerate(
        &mut component.velocity,
        wish_dir_3d,
        wish_speed,
        accel * control,
        dt,
    );
    let speed = Vec2::new(component.velocity.x, component.velocity.z).length();
    if speed > limit {
        let scale = limit / speed;
        component.velocity.x *= scale;
        component.velocity.z *= scale;
    }
}

/// The dash's decay of its retained base (and, airborne, its zero-drag boost),
/// in place: when grounded with no movement input, ground friction at the
/// descriptor's rate and stop speed; when airborne, the horizontal cap
/// (mirroring `Normal`'s air branch). The dash steers with its own
/// acceleration, so unlike [`ground_locomotion`] a held stick leaves the base
/// undecayed. Reads the grounded flag and friction params off the component.
pub(super) fn apply_normal_horizontal_decay(
    velocity: &mut Vec3,
    component: &PlayerMovementComponent,
    input: &MovementInput,
    ground_speed: f32,
    dt: f32,
) {
    if component.is_grounded() {
        if input.wish_dir.length_squared() < 0.001 {
            let ground = &component.ground_params;
            apply_ground_friction(velocity, ground.friction, ground.stop_speed, dt);
        }
    } else if !component.air.bunny_hop {
        let h_speed = Vec2::new(velocity.x, velocity.z).length();
        if h_speed > ground_speed {
            let scale = ground_speed / h_speed;
            velocity.x *= scale;
            velocity.z *= scale;
        }
    }
}
