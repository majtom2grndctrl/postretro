//! External velocity integration shared by player and navigation-agent movement.
use glam::Vec3;
use postretro_foundation::KnockbackResponse;

/// Linear fractional damping, with a small stop threshold so control recovers
/// after a damped shove. A zero drag preserves arbitrarily small authored pushes.
pub fn decay(velocity: &mut Vec3, response: &KnockbackResponse, grounded: bool, dt: f32) {
    let drag = if grounded {
        response.ground_drag
    } else {
        response.air_drag
    };
    *velocity *= (1.0 - drag * dt).clamp(0.0, 1.0);
    if drag > 0.0 && velocity.length_squared() < 1.0e-6 {
        *velocity = Vec3::ZERO;
    }
}

/// [`decay`] over `ticks` fixed ticks of `dt`, for a body that moves once per
/// `ticks`. Leaves `velocity` where `ticks` calls to [`decay`] would, and
/// returns the mean velocity of those ticks: moving at it for the whole step
/// travels what the 60 Hz ticks would, each moving at its own decayed
/// velocity. One tick is [`decay`] exactly.
pub fn decay_steps(
    velocity: &mut Vec3,
    response: &KnockbackResponse,
    grounded: bool,
    dt: f32,
    ticks: u32,
) -> Vec3 {
    if ticks <= 1 {
        decay(velocity, response, grounded, dt);
        return *velocity;
    }
    let drag = if grounded {
        response.ground_drag
    } else {
        response.air_drag
    };
    let factor = (1.0 - drag * dt).clamp(0.0, 1.0);
    // Σ factor^k for k in 1..=ticks, the ticks' travel in units of v·dt.
    let travelled = if factor >= 1.0 {
        ticks as f32
    } else {
        factor * (1.0 - factor.powi(ticks as i32)) / (1.0 - factor)
    };
    let mean = *velocity * (travelled / ticks as f32);
    *velocity *= factor.powi(ticks as i32);
    if drag > 0.0 && velocity.length_squared() < 1.0e-6 {
        *velocity = Vec3::ZERO;
    }
    mean
}

/// Apply the same contact-plane projection to the external layer as to total
/// velocity. Keeping both in the same frame avoids reconstructing a phantom
/// reverse velocity when next tick subtracts the protected layer.
pub fn project(velocity: &mut Vec3, normal: Vec3) {
    *velocity -= normal * velocity.dot(normal);
}

/// Clamp the combined fall speed while `velocity` contains the voluntary base.
/// The total counts both protected layers: knockback, and the drift layer's
/// vertical part, which is nonzero only while swimming (a downward current).
/// Excess comes from the protected layers first — drift, then knockback, then
/// the base — as `push::clamp_speed_along_push` takes it: putting the
/// correction entirely into the base would create upward velocity when a large
/// downward shove or current decays.
pub(super) fn clamp_fall_speed(component: &mut postretro_foundation::PlayerMovementComponent) {
    let total_y = component.velocity.y + component.knockback_velocity.y + component.push_velocity.y;
    let mut correction = (-component.fall.terminal_velocity - total_y).max(0.0);
    for layer_y in [
        &mut component.push_velocity.y,
        &mut component.knockback_velocity.y,
    ] {
        let cancelled = correction.min((-*layer_y).max(0.0));
        *layer_y += cancelled;
        correction -= cancelled;
    }
    component.velocity.y += correction;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(drag: f32) -> KnockbackResponse {
        KnockbackResponse {
            ground_drag: drag,
            air_drag: drag,
            ..KnockbackResponse::default()
        }
    }

    // A step over n ticks travels what n ticks of `decay` do, and ends at the
    // same velocity, for every drag from none to one that stops a shove in a
    // tick.
    #[test]
    fn decay_steps_travels_and_ends_as_the_ticks_it_spans() {
        let dt = 1.0 / 60.0;
        for drag in [0.0, 0.5, 1.0, 2.0, 8.0, 30.0, 60.0, 120.0] {
            for ticks in [1, 2, 4, 30] {
                let response = response(drag);
                let start = Vec3::new(6.0, 1.0, -3.0);
                let mut ticked = start;
                let mut travelled = Vec3::ZERO;
                for _ in 0..ticks {
                    decay(&mut ticked, &response, true, dt);
                    travelled += ticked * dt;
                }
                let mut stepped = start;
                let mean = decay_steps(&mut stepped, &response, true, dt, ticks);
                let step_travel = mean * dt * ticks as f32;
                assert!(
                    (step_travel - travelled).length() < 1.0e-3,
                    "drag {drag}, {ticks} ticks: stepped {step_travel:?}, ticked {travelled:?}"
                );
                assert!(
                    (stepped - ticked).length() < 1.0e-3,
                    "drag {drag}, {ticks} ticks: ends at {stepped:?}, ticked {ticked:?}"
                );
            }
        }
    }

    #[test]
    fn one_tick_of_decay_steps_is_decay() {
        let response = response(3.0);
        let mut a = Vec3::new(2.0, 0.5, 1.0);
        let mut b = a;
        decay(&mut a, &response, false, 1.0 / 60.0);
        let mean = decay_steps(&mut b, &response, false, 1.0 / 60.0, 1);
        assert_eq!(a, b);
        assert_eq!(mean, b);
    }
}
