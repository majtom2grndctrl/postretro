// Sliding velocity intent and Normal -> Sliding entry.
// See: context/lib/movement.md §§4,6.

use glam::{Vec2, Vec3};

use crate::collision::moving::CombinedCollisionWorld;
use crate::movement::carry::CarryRule;
use crate::movement::dispatch::{stand_up_resize, stand_up_transition};
use crate::movement::substrate::{
    JumpEdges, ResizeAnchor, air_jump_ready, resize_capsule, standup_clearance_probe,
    wish_dir_from_input,
};
use crate::movement::{MovementEvents, MovementInput, Transition};
use postretro_foundation::{MovementState, PlayerMovementComponent};

/// Generous hard safety bound for a slide. Terrain, release, and drag are the
/// normal exits; this makes a frictionless held-crouch slide finite.
pub(crate) const SLIDE_MAX_MS: f32 = 3_000.0;

/// Attempt the grounded `Normal` -> `Sliding` edge. Slide is unavailable without
/// both descriptor blocks: it reuses crouch's capsule dimensions and eye target.
pub(crate) fn try_enter_slide(
    component: &mut PlayerMovementComponent,
    position: &mut Vec3,
) -> Option<Transition> {
    let slide = component.slide.as_ref()?;
    let crouch = component.crouch.as_ref()?;
    let horizontal = Vec2::new(component.velocity.x, component.velocity.z);
    if horizontal.length() < slide.min_speed {
        return None;
    }

    let direction = horizontal.normalize_or_zero();
    let boost = Vec3::new(
        horizontal.x + direction.x * slide.entry_boost,
        0.0,
        horizontal.y + direction.y * slide.entry_boost,
    );

    // The entry owns the entire horizontal velocity as boost. That makes the
    // base layer zero at the edge, so dispatch's KEEP_ALL is exactly a no-op.
    component.velocity.x = boost.x;
    component.velocity.z = boost.z;

    let eye_current = component.capsule.eye_height;
    let delta = resize_capsule(
        component,
        crouch.half_height,
        crouch.eye_height,
        ResizeAnchor::Feet,
    );
    position.y += delta;
    // Collision shrinks immediately, while the view smoothly descends.
    component.capsule.eye_height = eye_current;

    Some(Transition {
        next: MovementState::Sliding {
            elapsed_ms: 0.0,
            boost,
            eye_current,
        },
        carry: CarryRule::KEEP_ALL,
    })
}

/// Convert an active slide to its natural terminal state. Held crouch keeps the
/// already-crouched capsule directly; released crouch probes before standing.
fn natural_exit(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    collision: &CombinedCollisionWorld<'_>,
    position: &mut Vec3,
    eye_current: f32,
) -> Transition {
    if input.crouch_intent {
        return Transition {
            next: MovementState::Crouching { eye_current },
            carry: CarryRule::KEEP_ALL,
        };
    }

    if standup_clearance_probe(
        component,
        collision,
        *position,
        component.capsule.half_height,
        component.standing_half_height,
    ) {
        // Slide entry is always feet-anchored, even if it subsequently reaches
        // a ledge, so its inverse stand-up resize must use Feet as well.
        stand_up_resize(component, position, ResizeAnchor::Feet);
        return stand_up_transition();
    }

    Transition {
        next: MovementState::Crouching { eye_current },
        carry: CarryRule::KEEP_ALL,
    }
}

/// Rotate a horizontal boost toward movement input by no more than `max_radians`.
/// This changes direction only; the caller retains the speed exactly.
fn steer_boost(boost: &mut Vec3, wish_dir: Vec3, max_radians: f32) {
    let speed = Vec2::new(boost.x, boost.z).length();
    let target = Vec2::new(wish_dir.x, wish_dir.z);
    if speed == 0.0 || target.length_squared() == 0.0 || max_radians <= 0.0 {
        return;
    }

    let current = Vec2::new(boost.x / speed, boost.z / speed);
    let target = target.normalize();
    let signed_angle = (current.x * target.y - current.y * target.x).atan2(current.dot(target));
    let angle = signed_angle.clamp(-max_radians, max_radians);
    let (sin, cos) = angle.sin_cos();
    let rotated = Vec2::new(
        current.x * cos - current.y * sin,
        current.x * sin + current.y * cos,
    );
    boost.x = rotated.x * speed;
    boost.z = rotated.y * speed;
}

/// The active slide intent. The stored boost is reconciled against collision's
/// realized prior-tick velocity before slope input is added, preventing a wall
/// clip from reappearing as an opposite-direction base kick.
#[allow(clippy::too_many_arguments)]
pub(crate) fn sliding_intent(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    jump_edges: JumpEdges,
    gravity: f32,
    dt: f32,
    collision: &CombinedCollisionWorld<'_>,
    position: &mut Vec3,
    events: &mut MovementEvents,
    elapsed_ms: &mut f32,
    boost: &mut Vec3,
    eye_current: &mut f32,
) -> Option<Transition> {
    // A descriptor hot-swap may remove slide or crouch while the state is live.
    // Do not retain an invalid capsule state; hand it to crouch when available.
    let (slide, crouch) = match (component.slide.as_ref(), component.crouch.as_ref()) {
        (Some(slide), Some(crouch)) => (slide.clone(), crouch.clone()),
        _ => {
            return Some(Transition {
                next: MovementState::Normal,
                carry: CarryRule::KEEP_ALL,
            });
        }
    };

    // Jump is intentionally first among exits. Coyote / air-jump edges can fire
    // on the same stale-airborne tick that would otherwise hand off to crouch.
    let jumped = if jump_edges.grounded {
        component.velocity.y = component.air.jump_velocity;
        component.set_grounded(false);
        component.jump_spent = true;
        events.jumped = true;
        true
    } else if jump_edges.air && air_jump_ready(component) {
        component.velocity.y = component.air.jump_velocity;
        component.air_jumps_remaining -= 1;
        component.jump_spent = true;
        events.jumped = true;
        true
    } else {
        false
    };
    if jumped {
        if standup_clearance_probe(
            component,
            collision,
            *position,
            component.capsule.half_height,
            component.standing_half_height,
        ) {
            stand_up_resize(component, position, ResizeAnchor::Feet);
            *eye_current = component.capsule.eye_height;
            return Some(stand_up_transition());
        }
        return Some(Transition {
            next: MovementState::Crouching {
                eye_current: *eye_current,
            },
            carry: CarryRule::KEEP_ALL,
        });
    }

    if !component.is_grounded() {
        component.velocity.y =
            (component.velocity.y + gravity * dt).max(-component.fall.terminal_velocity);
        // A non-jump ledge exit retains momentum and lets Crouching own airborne
        // locomotion/stand-up from the following tick onward.
        return Some(Transition {
            next: MovementState::Crouching {
                eye_current: *eye_current,
            },
            carry: CarryRule::KEEP_ALL,
        });
    }

    // Reconcile stale tracked boost with collision's realized horizontal velocity
    // BEFORE applying fresh downhill acceleration. The boost is horizontal by
    // invariant, including after this clamp.
    let boost_len = Vec2::new(boost.x, boost.z).length();
    if boost_len > 0.0 {
        let direction = Vec3::new(boost.x / boost_len, 0.0, boost.z / boost_len);
        let realized =
            (component.velocity.x * direction.x + component.velocity.z * direction.z).max(0.0);
        let clamped = boost_len.min(realized);
        boost.x = direction.x * clamped;
        boost.z = direction.z * clamped;
    }
    boost.y = 0.0;

    let base = Vec3::new(
        component.velocity.x - boost.x,
        0.0,
        component.velocity.z - boost.z,
    );

    // Gravity projected onto the last substrate-supplied floor plane supplies
    // downhill-only horizontal acceleration. A missing or flat normal is a no-op.
    if let Some(normal) = component.last_floor_normal {
        let gravity_vec = Vec3::new(0.0, gravity, 0.0);
        let along_plane = gravity_vec - normal * gravity_vec.dot(normal);
        boost.x += along_plane.x * slide.slope_assist * dt;
        boost.z += along_plane.z * slide.slope_assist * dt;
    }

    steer_boost(
        boost,
        wish_dir_from_input(input.wish_dir, input.facing_yaw),
        slide.steer_rate.to_radians() * dt,
    );

    let speed = Vec2::new(boost.x, boost.z).length();
    if speed > 0.0 {
        let retained = (speed - slide.slide_drag * dt).max(0.0) / speed;
        boost.x *= retained;
        boost.z *= retained;
    }
    component.velocity.x = base.x + boost.x;
    component.velocity.z = base.z + boost.z;

    let alpha = 1.0 - (-crouch.transition_rate * dt).exp();
    *eye_current += (crouch.eye_height - *eye_current) * alpha;
    component.capsule.eye_height = *eye_current;
    *elapsed_ms += dt * 1000.0;

    // The hard maximum ignores minimum duration, while the ordinary release and
    // speed exits wait until the authored committed window has elapsed.
    if *elapsed_ms >= SLIDE_MAX_MS {
        return Some(natural_exit(
            component,
            input,
            collision,
            position,
            *eye_current,
        ));
    }
    if *elapsed_ms >= slide.min_duration_ms {
        let total_speed = Vec2::new(component.velocity.x, component.velocity.z).length();
        if !input.crouch_intent || total_speed <= component.ground_params.speed.crouch {
            return Some(natural_exit(
                component,
                input,
                collision,
                position,
                *eye_current,
            ));
        }
    }
    None
}
