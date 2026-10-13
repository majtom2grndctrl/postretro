// Swim-state velocity intent: 3D fluid locomotion, viscosity drag, gravity-scaled sink and the fluid jump.
// See: context/lib/movement.md §4

use std::f32::consts::FRAC_PI_2;

use glam::Vec3;

use crate::movement::carry::CarryRule;
use crate::movement::{MovementEvents, MovementInput, Transition};
use postretro_collision::ResolvedEnvironment;
use postretro_foundation::{MovementState, PlayerMovementComponent};
use postretro_level_format::environment_volumes::DEFAULT_WORLD_GRAVITY_MPS2;

/// Default level gravity's magnitude, m/s². `swim.sink_speed` is authored at
/// this gravity and scales linearly with the resolved gravity.
const REFERENCE_GRAVITY: f32 = -DEFAULT_WORLD_GRAVITY_MPS2;

/// The input half of a fluid jump: jump held with the TICK eye (the tick-start
/// resolve's `position.y + eye_height`) outside the fluid. The presented camera
/// eye, view feel included, never reaches movement, so host, prediction and
/// replay agree on it (`movement.md` §4, Swim). Jump is the same level bit every other
/// jump fires off (`MovementInput::jump_pressed`): held at the surface it leaps
/// out, held underwater it swims up. The swim seam adds the rest — swimming at
/// tick start, and a rise that clears the water — and hands the intent the
/// verdict (`swim_transition::environment_transition`).
pub(crate) fn fluid_jump_input(input: &MovementInput, environment: &ResolvedEnvironment) -> bool {
    input.jump_pressed && !environment.tick_eye_in_fluid
}

/// The `Swim` state's per-tick velocity intent. Entry and exit by immersion are
/// applied at the tick seam before this runs (`swim_transition`), which also
/// decides whether this tick fires a fluid jump (`fluid_jump`); this intent
/// owns the fluid jump and the fluid locomotion.
///
/// 1. Fluid jump: with `fluid_jump`, vertical velocity is SET to
///    `swim.fluid_jump_velocity` (horizontal momentum kept; the vertical parts
///    of both protected layers, the drift layer's current and knockback, are
///    discarded, so the jump leaves at exactly the velocity the seam's
///    clearance test assumed), the re-entry latch is armed, and the
///    state leaves to airborne `Normal` with no solid needed.
/// 2. Drag: the fluid's viscosity removes a linear fraction of locomotion per
///    tick, `max(0, 1 - viscosity * dt)`, on all three axes. It is the only
///    drag while swimming; a swimmer touching the pool floor gets no ground
///    friction.
/// 3. Acceleration toward the 3D wish velocity at `swim.accel` (m/s², scaled
///    by knockback control) up to `swim.speed`. Like `pm_accelerate` it only
///    adds speed along the wish direction, so carried momentum above `speed` is
///    never clipped; drag alone bleeds it. Gravity is not integrated: with no
///    vertical input the wish's vertical part is the sink velocity. While a
///    floor or ceiling holds part of locomotion against the drift layer
///    (`held_by_contact`, from `push::held_by_contact`), the horizontal result
///    comes from accelerating locomotion with that part excluded, and the
///    vertical result from locomotion whole (motion relative to the water,
///    which decides lift-off). Mid-water the part is zero and both agree.
///
/// A swimmer touching the pool floor stays grounded for collision (ground
/// reference, landing refresh, mover carry, step-up) but not for its medium:
/// no ground friction, and its push drift decays at the fluid's viscosity. It
/// leaves the floor once its total vertical motion, current included, points
/// up (`push::apply`).
// One more input than the clippy default: the contact's held reaction comes
// from dispatch, which owns the collision handles its ceiling probe needs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn swim_intent(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    environment: &ResolvedEnvironment,
    fluid_jump: bool,
    held_by_contact: f32,
    dt: f32,
    events: &mut MovementEvents,
    fluid: &mut u16,
) -> Option<Transition> {
    // Follow the fluid while swimming, so the state names the one being left.
    if let Some(current) = environment.fluid {
        *fluid = current;
    }
    // The tick seam leaves swim when the block is gone (a descriptor hot swap);
    // stay total here regardless.
    let Some(swim) = component.swim.as_ref() else {
        return Some(leave_swim());
    };
    let speed = swim.speed;
    let accel = swim.accel;
    let sink_speed = swim.sink_speed;
    // The seam sets `fluid_jump` only when the velocity is `Some`.
    if let Some(fluid_jump_velocity) = swim.fluid_jump_velocity.filter(|_| fluid_jump) {
        component.velocity.y = fluid_jump_velocity;
        component.push_velocity.y = 0.0;
        component.knockback_velocity.y = 0.0;
        component.set_grounded(false);
        component.jump_spent = true;
        component.swim_reentry_latched = true;
        events.jumped = true;
        return Some(leave_swim());
    }

    let drag = (1.0 - environment.viscosity * dt).max(0.0);
    component.velocity *= drag;

    let target = wish_velocity(input, speed, sink_speed, environment);
    let accel = accel * component.knockback_control();
    // Vertical: locomotion is motion relative to the water, held part
    // included, so a swimmer the floor holds against an upward current is
    // already sinking through the water at that current and leaves the floor
    // once the current outruns its sink (`push::release_rising_swimmer`).
    let mut relative = component.velocity;
    accelerate_toward(&mut relative, target, accel, dt);
    // Horizontal: steering reads the swimmer's own motion only. The held part
    // is vertical motion the contact pins, not speed along the wish; projected
    // onto a wish with a vertical part it would read as speed the swimmer
    // lacks, and horizontal speed would climb past `swim.speed` to make it up.
    // It scales with drag as the drift it cancels does (`push::decay_drift`).
    let held = Vec3::new(0.0, held_by_contact * drag, 0.0);
    let mut own = component.velocity - held;
    accelerate_toward(&mut own, target, accel, dt);
    component.velocity = Vec3::new(own.x, relative.y, own.z);
    None
}

/// `Swim` → `Normal`, momentum unchanged: swim carries no boost layer. Dispatch
/// folds the drift layer's vertical part into locomotion on the edge.
pub(crate) fn leave_swim() -> Transition {
    Transition {
        next: MovementState::Normal,
        carry: CarryRule::KEEP_ALL,
    }
}

/// The 3D wish velocity, Quake-style. Forward follows the view pitch
/// (`facing_pitch`, positive looks up), strafe stays horizontal, jump held
/// swims up and crouch held swims down. Any input swims at full `speed` along
/// the combined direction. With neither jump nor crouch held, the sink velocity
/// `sink_speed * gravity / REFERENCE_GRAVITY` joins the vertical part: a swimmer
/// sinks at `sink_speed` under default level gravity, floats with zero gravity
/// and rises under upward gravity. With the tick eye out of the fluid the wish
/// never points up,
/// so a swimmer treads at the surface instead of swimming out of it; the fluid
/// jump is the way out.
fn wish_velocity(
    input: &MovementInput,
    speed: f32,
    sink_speed: f32,
    environment: &ResolvedEnvironment,
) -> Vec3 {
    let (sin_yaw, cos_yaw) = input.facing_yaw.sin_cos();
    let (sin_pitch, cos_pitch) = input.facing_pitch.clamp(-FRAC_PI_2, FRAC_PI_2).sin_cos();
    let forward = Vec3::new(-sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch);
    let right = Vec3::new(cos_yaw, 0.0, -sin_yaw);
    let vertical = match (input.jump_pressed, input.crouch_intent) {
        (true, false) => 1.0,
        (false, true) => -1.0,
        _ => 0.0,
    };
    let direction = forward * input.wish_dir.y + right * input.wish_dir.x + Vec3::Y * vertical;
    let mut target = direction.normalize_or_zero() * speed;
    if !input.jump_pressed && !input.crouch_intent {
        target.y += sink_speed * (environment.gravity / REFERENCE_GRAVITY);
    }
    if !environment.tick_eye_in_fluid {
        target.y = target.y.min(0.0);
    }
    target
}

/// Add up to `accel * dt` of speed along `target`'s direction, never past
/// `target`'s length along it. Speed already at or past it along that
/// direction, and every perpendicular component, is left to drag.
fn accelerate_toward(velocity: &mut Vec3, target: Vec3, accel: f32, dt: f32) {
    let wish_speed = target.length();
    if wish_speed <= 0.0 {
        return;
    }
    let direction = target / wish_speed;
    let add_speed = wish_speed - velocity.dot(direction);
    if add_speed <= 0.0 {
        return;
    }
    *velocity += direction * add_speed.min(accel * dt);
}
