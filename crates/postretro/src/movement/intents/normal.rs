// Normal-state velocity intent.
// See: context/lib/movement.md §4

use glam::{Vec2, Vec3};

use crate::movement::carry::CarryRule;
use crate::movement::substrate::{
    JumpEdges, ResizeAnchor, air_jump_ready, pm_accelerate, resize_capsule, wish_dir_from_input,
};
use crate::movement::{MovementEvents, MovementInput, Transition};
use postretro_foundation::{MovementState, PlayerMovementComponent};

use super::dash::try_enter_dash;
use super::slide::try_enter_slide;
use super::{GROUND_STOP_FRICTION, OVERSPEED_BLEED_MARGIN};

/// gravity, jump/air-jump, ground/air acceleration, ground friction, and the
/// airborne horizontal cap. This is the walk/run/jump/air-control baseline —
/// the behavior-unchanged locomotion.
///
/// Operates on `component.velocity`, reading the grounded flag carried from the
/// previous tick (`component.is_grounded()`). Steps 2/3 may clear `is_grounded`
/// when a jump launches; that clear is part of the intent (a jump is no longer
/// grounded) and the substrate reads the post-intent flag.
///
/// Sets `events.jumped` when a jump launches. Returns the warranted transition
/// (next state + its carry-rule) or `None` to stay in `Normal`. `Normal`
/// transitions to `Dash` on a rising-edge dash input (see `try_enter_dash`) and
/// to `Crouching` on the resolved `crouch_intent` bit when `CrouchParams` is
/// present; future states (slide, wall-run) plug in behind the same seam without
/// reshaping callers.
///
/// `jump_edges` are the forgiveness-derived edges (coyote + buffer), computed
/// ONCE per tick by `derive_jump_edges` before this intent runs (D5). The jump
/// steps consume `jump_edges.grounded` / `jump_edges.air` IN PLACE OF the raw
/// `jump_pressed` bit so forgiveness is never re-derived here.
pub(crate) fn normal_intent(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    jump_edges: JumpEdges,
    gravity: f32,
    dt: f32,
    position: &mut Vec3,
    events: &mut MovementEvents,
) -> Option<Transition> {
    // 1. Gravity (airborne only).
    if !component.is_grounded() {
        component.velocity.y += gravity * dt;
        let terminal = component.fall.terminal_velocity;
        if component.velocity.y < -terminal {
            component.velocity.y = -terminal;
        }
    }

    // 2. Grounded jump — fired off the DERIVED grounded edge (a fresh grounded
    // press, a coyote-granted press, or a buffered press landing) rather than
    // raw `jump_pressed`. Consumes NO air-jump charge: a coyote/buffered jump is
    // a grounded jump. Sets `jump_spent` so coyote cannot re-arm this stretch.
    if jump_edges.grounded {
        component.velocity.y = component.air.jump_velocity;
        component.set_grounded(false);
        component.jump_spent = true;
        events.jumped = true;
    }
    // 3. Air-jump (double-jump): a named airborne ability under the budget
    // model. Fires off the DERIVED air edge (an airborne press the grounded edge
    // did not claim) AND the budget/ceiling gate. Consumes one charge from
    // `air_jumps_remaining`, which refreshes uniformly on floor contact through
    // `refresh_on_landing` (the single landing-refresh point shared with other
    // air-budget abilities, e.g. air-dash). The ceiling gate
    // (`velocity.y <= air.jump_ceiling`) keeps it from firing at the top of the
    // rising arc; the launch reuses the ground jump velocity. Spends the
    // jump-spent flag so coyote cannot re-arm after an air jump.
    else if jump_edges.air && air_jump_ready(component) {
        component.velocity.y = component.air.jump_velocity;
        component.air_jumps_remaining -= 1;
        component.jump_spent = true;
        events.jumped = true;
    }

    // 4 + 5. Locomotion: ground vs air branch on the same input. Sprint picks
    // the run speed; the same value caps airborne horizontal speed so a
    // sprint-then-jump arc doesn't instantly decelerate mid-air.
    let ground_speed = if input.running {
        component.ground_params.speed.run
    } else {
        component.ground_params.speed.walk
    };
    let input_dir_3d = wish_dir_from_input(input.wish_dir, input.facing_yaw);
    if component.is_grounded() {
        if input_dir_3d.length_squared() > 0.0 {
            pm_accelerate(
                &mut component.velocity,
                input_dir_3d,
                ground_speed,
                component.ground_params.accel,
                dt,
            );
        }
    } else if input_dir_3d.length_squared() > 0.0 {
        // Blend toward facing only on forward/back input: strafing left/right
        // should not redirect the capsule toward the player's nose.
        let wish_dir_3d = if input.wish_dir.y.abs() > 1e-3 {
            let facing_dir = Vec3::new(-input.facing_yaw.sin(), 0.0, -input.facing_yaw.cos());
            let steer = component.air.forward_steer.clamp(0.0, 1.0);
            let blended = input_dir_3d.lerp(facing_dir, steer);
            if blended.length_squared() > 0.0 {
                blended.normalize()
            } else {
                Vec3::ZERO
            }
        } else {
            input_dir_3d
        };
        let wish_speed = component.air.max_control_speed;
        pm_accelerate(
            &mut component.velocity,
            wish_dir_3d,
            wish_speed,
            component.air.accel,
            dt,
        );
        if !component.air.bunny_hop {
            // Cap horizontal speed; vertical velocity (jump/gravity) untouched.
            let horiz = Vec2::new(component.velocity.x, component.velocity.z);
            let h_speed = horiz.length();
            let cap = ground_speed;
            if h_speed > cap {
                let scale = cap / h_speed;
                component.velocity.x *= scale;
                component.velocity.z *= scale;
            }
        }
    }

    // 6. Ground friction. With no directional input, bleed toward a stop. With
    // input held, bleed only the *over-speed* above the run cap back toward the
    // cap: `pm_accelerate` governs actively-driven motion up to the cap but
    // cannot remove speed already above it, and the stop-friction is
    // no-input-only. In normal play a grounded player never exceeds the cap, so
    // the input-held branch is a no-op there; it exists so post-dash over-speed
    // decays even while the stick is held, and a dash hands back into the steady
    // band cleanly after the `DASH_MAX_MS` guard.
    if component.is_grounded() && input.wish_dir.length_squared() < 0.001 {
        let horiz = Vec2::new(component.velocity.x, component.velocity.z);
        let h_speed = horiz.length();
        if h_speed > 0.0 {
            let drop = h_speed * GROUND_STOP_FRICTION * dt;
            let new_speed = (h_speed - drop).max(0.0);
            let scale = new_speed / h_speed;
            component.velocity.x *= scale;
            component.velocity.z *= scale;
        }
    } else if component.is_grounded() {
        // Input held: `pm_accelerate` governs motion up to the run cap but cannot
        // remove speed already above it, and the stop-friction above is
        // no-input-only. Bleed only the *over-speed* above the cap back toward it
        // so a dash (which deliberately exceeds the cap) decays even while the
        // stick is held. The `OVERSPEED_BLEED_MARGIN` guard keeps this a no-op in
        // normal play, where running sits at the cap modulo float overshoot.
        let h_speed = Vec2::new(component.velocity.x, component.velocity.z).length();
        if h_speed > ground_speed * OVERSPEED_BLEED_MARGIN {
            let drop = (h_speed - ground_speed) * GROUND_STOP_FRICTION * dt;
            let new_speed = (h_speed - drop).max(ground_speed);
            let scale = new_speed / h_speed;
            component.velocity.x *= scale;
            component.velocity.z *= scale;
        }
    }

    // `Normal` → `Dash`: fire on the dash rising edge when the cooldown is ready
    // and — if airborne — an air-dash charge remains. Disabled when no
    // `DashParams` is materialized (descriptor omitted `movement.dash`). The
    // entry blends velocity (retained base + boost), applies `preserve_vertical`,
    // consumes the airborne charge, and arms the cooldown; it returns the seeded
    // `Dash` state for `tick` to apply after the substrate resolves collision.
    if input.dash_pressed {
        if let Some(transition) = try_enter_dash(component, input) {
            return Some(transition);
        }
    }

    // `Normal` → `Crouching`: fire on the resolved `crouch_intent` bit when a
    // `CrouchParams` is materialized. Absent `crouch` ⇒ the transition NEVER
    // fires (crouch disabled — no resize, no effect). The entry resize runs here
    // (the edge): shrink the collision capsule to the crouched size with the
    // anchor chosen by grounded-vs-airborne — `Feet` when grounded (plant the
    // feet, drop the center, D2), `Head` when airborne (pin the head, raise the
    // feet, D4) — and apply the helper-returned center delta to `position`. The
    // eye smooths from the current standing eye toward the crouched target inside
    // the `Crouching` intent; seed `eye_current` at the standing eye so the first
    // tick begins the descent. The carry is `KEEP_ALL`: crouch is a resize, not a
    // velocity reset, so momentum is preserved unchanged (the §6 parity no-op).
    if input.crouch_intent {
        if component.is_grounded() {
            if let Some(transition) = try_enter_slide(component, position) {
                return Some(transition);
            }
        }
        if let Some(crouch) = component.crouch.as_ref() {
            let target_half_height = crouch.half_height;
            let target_eye_height = crouch.eye_height;
            let anchor = if component.is_grounded() {
                ResizeAnchor::Feet
            } else {
                ResizeAnchor::Head
            };
            let eye_current = component.capsule.eye_height;
            let delta = resize_capsule(component, target_half_height, target_eye_height, anchor);
            position.y += delta;
            // `resize_capsule` snapped `eye_height` to the crouched target; the
            // eye must SMOOTH instead (D3). Restore the live `eye_height` to the
            // pre-entry value so the camera does not pop on the entry tick — the
            // `Crouching` intent advances `eye_current` toward the crouched target
            // from here and writes the smoothed value each tick. (`half_height`
            // keeps the crouched value the helper set: collision shrinks
            // immediately; only the camera eye eases.)
            component.capsule.eye_height = eye_current;
            return Some(Transition {
                next: MovementState::Crouching { eye_current },
                carry: CarryRule::KEEP_ALL,
            });
        }
    }

    None
}
