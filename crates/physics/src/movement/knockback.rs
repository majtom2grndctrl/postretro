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
