// Push volumes in player movement: lift-off, the protected drift layer, its medium decay and terminal speed along the push.
// See: context/plans/in-progress/E24--water-and-environment-volumes/plan.md (Owner ruling 2026-10-11)

use glam::{Vec2, Vec3};

use postretro_collision::ResolvedEnvironment;
use postretro_foundation::{MovementState, PlayerMovementComponent};

use super::intents::GROUND_STOP_FRICTION;

/// Below this squared speed a decaying drift stops outright, as damped knockback
/// does, so a landed drift reaches exactly zero instead of trailing denormals.
const DRIFT_STOP_SPEED_SQ: f32 = 1.0e-6;

/// Rule 1, lift-off: a grounded body leaves the ground when the push points up
/// harder than gravity pulls down. Only gravity's downward part resists, so
/// upward gravity never holds a body down; with no push this is always false,
/// and upward gravity alone never lifts a grounded body. `tick` treats a true
/// result like a knockback launch: it clears grounded before the intent and
/// tells the substrate to skip the upward clamp and ground-stick.
pub(super) fn lifts_off(environment: &ResolvedEnvironment) -> bool {
    let push_y = environment.push.y;
    push_y > 0.0 && push_y + environment.gravity.min(0.0) > 0.0
}

/// Rule 3, after the intent. Called while `velocity` holds locomotion only.
/// The medium first decays the drift built up so far, then horizontal push
/// accrues in the protected drift layer (`push_velocity`), which intent caps
/// and steering never see. Vertical push joins locomotion directly; no intent
/// caps vertical speed, and gravity, lift-off and the terminal cap govern it.
/// While the body stays grounded the floor bears the vertical part, exactly as
/// every intent gates gravity on airborne; a push strong enough to lift has
/// already cleared grounded through [`lifts_off`].
pub(super) fn apply(
    component: &mut PlayerMovementComponent,
    environment: &ResolvedEnvironment,
    dt: f32,
) {
    let push = environment.push;
    decay_drift(component, environment.viscosity, dt);
    component.push_velocity += Vec3::new(push.x, 0.0, push.z) * dt;
    if !component.is_grounded() {
        component.velocity.y += push.y * dt;
    }
}

/// The medium a body's drift decays in this tick.
enum DriftMedium {
    /// Swimming: the fluid's viscosity drags the drift, so a current settles
    /// near push / viscosity. A swimmer touching the pool floor still counts
    /// as swimming here: the fluid, not floor friction, is the medium.
    Fluid { viscosity: f32 },
    /// Grounded: friction bleeds the drift.
    Ground,
    /// Airborne: nothing resists, so drift carries out of a wind until landing.
    Air,
}

/// Read after the intent, so a jump or a launch this tick counts as airborne,
/// and a fluid jump this tick (which left swim) counts as airborne too.
/// `viscosity` is the tick's resolved fluid drag.
fn drift_medium(component: &PlayerMovementComponent, viscosity: f32) -> DriftMedium {
    if matches!(component.movement_state, MovementState::Swim { .. }) {
        DriftMedium::Fluid { viscosity }
    } else if component.is_grounded() {
        DriftMedium::Ground
    } else {
        DriftMedium::Air
    }
}

/// The medium's decay of the drift layer, applied every tick before the tick's
/// push accrues. Ground friction is the intents' stop-friction model on
/// locomotion, applied whether or not input is held because drift is never
/// voluntary; with a steady push the drift settles at push /
/// `GROUND_STOP_FRICTION`. A fluid removes the same linear fraction per tick as
/// the swim intent's drag on locomotion (`max(0, 1 - viscosity * dt)`).
fn decay_drift(component: &mut PlayerMovementComponent, viscosity: f32, dt: f32) {
    let medium = drift_medium(component, viscosity);
    let drift = &mut component.push_velocity;
    match medium {
        DriftMedium::Fluid { viscosity } => {
            *drift *= (1.0 - viscosity * dt).max(0.0);
            if drift.length_squared() < DRIFT_STOP_SPEED_SQ {
                *drift = Vec3::ZERO;
            }
        }
        DriftMedium::Ground => {
            let speed = Vec2::new(drift.x, drift.z).length();
            if speed > 0.0 {
                let drop = speed * GROUND_STOP_FRICTION * dt;
                let scale = (speed - drop).max(0.0) / speed;
                drift.x *= scale;
                drift.z *= scale;
            }
            if drift.length_squared() < DRIFT_STOP_SPEED_SQ {
                *drift = Vec3::ZERO;
            }
        }
        DriftMedium::Air => {}
    }
}

/// Project the drift layer against a contact normal exactly as collision
/// projects total velocity and knockback, so a wall absorbs the drift into it.
/// The layer stays horizontal: any vertical part the projection produces stays
/// in total velocity, where the next tick reads it as locomotion.
pub(super) fn project_drift(drift: &mut Vec3, normal: Vec3) {
    *drift -= normal * drift.dot(normal);
    drift.y = 0.0;
}

/// Rule 2, terminal speed: cap the total speed along the push direction at the
/// descriptor's fall terminal velocity, so an updraft or a wind does not
/// accelerate a body forever. Only the component along the push is touched.
/// Runs while `velocity` holds only locomotion. The excess splits into its
/// horizontal and vertical parts, each taken from the protected layers first,
/// as `knockback::clamp_fall_speed` does, so a decaying layer never leaves
/// locomotion pushing against the push:
/// - horizontal: from the drift layer, then knockback, then locomotion;
/// - vertical: from knockback, then locomotion `velocity.y` (drift has none).
pub(super) fn clamp_speed_along_push(component: &mut PlayerMovementComponent, push: Vec3) {
    let Some(direction) = push.try_normalize() else {
        return;
    };
    let total = component.velocity + component.knockback_velocity + component.push_velocity;
    let excess = (total.dot(direction) - component.fall.terminal_velocity).max(0.0);
    if excess == 0.0 {
        return;
    }
    let excess = direction * excess;

    let horizontal = Vec3::new(excess.x, 0.0, excess.z);
    if let Some(along) = horizontal.try_normalize() {
        let mut remaining = horizontal.length();
        for layer in [
            &mut component.push_velocity,
            &mut component.knockback_velocity,
        ] {
            let cut = remaining.min(layer.dot(along).max(0.0));
            *layer -= along * cut;
            remaining -= cut;
        }
        component.velocity -= along * remaining;
    }

    let sign = excess.y.signum();
    let vertical = excess.y.abs();
    let cut = vertical.min((component.knockback_velocity.y * sign).max(0.0));
    component.knockback_velocity.y -= sign * cut;
    component.velocity.y -= sign * (vertical - cut);
}
