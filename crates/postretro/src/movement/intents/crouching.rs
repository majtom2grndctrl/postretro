// Crouching velocity intent.
// See: context/lib/movement.md §4

use glam::{Vec2, Vec3};

use crate::collision::moving::CombinedCollisionWorld;
use crate::movement::dispatch::{stand_up_resize, stand_up_transition};
use crate::movement::substrate::{
    JumpEdges, ResizeAnchor, air_jump_ready, pm_accelerate, standup_clearance_probe,
    wish_dir_from_input,
};
use crate::movement::{MovementEvents, MovementInput, Transition};
use postretro_foundation::PlayerMovementComponent;

use super::dash::try_enter_dash;
use super::{GROUND_STOP_FRICTION, OVERSPEED_BLEED_MARGIN};

/// The `Crouching` state's per-tick velocity intent. Locomotion mirrors
/// `Normal` (gravity, jump/air-jump, ground/air acceleration, friction, the
/// airborne cap) with ONE substitution: the omnidirectional horizontal speed
/// target is the crouch tier `ground.speed.crouch` instead of walk/run (D5).
/// Jump access is NEVER suppressed (D10) — the grounded/air jump branch and the
/// `Dash` transition stay available exactly as in `Normal`.
///
/// Beyond locomotion the intent owns three crouch-specific responsibilities:
///   - Eye smoothing (D3): `eye_current` eases toward the crouched eye target
///     by a framerate-independent exponential approach at `transition_rate` per
///     second, written into `component.capsule.eye_height` each tick for the
///     camera follow.
///   - Stand-up (D7): while `crouch_intent` is INACTIVE, sweep the standing
///     capsule up via `standup_clearance_probe`; when CLEAR, resize back to
///     standing (apply the center delta to `position` — the feet stay planted,
///     the center rises), transition to `Normal` with `KEEP_ALL` (a resize, not
///     a velocity reset). When BLOCKED, stay crouched and retry next tick.
///   - Crouch-jump (D10): when a jump edge fires while `crouch_intent` is STILL
///     ACTIVE, run the same stand-up probe FIRST — clear headroom ⇒ stand
///     (resize, shift position) and transition to `Normal`, then apply the jump
///     this tick; blocked ⇒ apply the jump from the crouched state (lower arc,
///     crouched capsule retained). The jump is never swallowed.
///
/// `eye_current` is borrowed in place from the active `Crouching` variant (the
/// dispatch resolves the component-vs-state borrow once), so the intent advances
/// its own smoothing source directly. Returns the warranted transition (always
/// `KEEP_ALL` — crouch never transforms momentum at the seam) or `None` to stay
/// `Crouching`.
// Mirrors `normal_intent`'s shape; grouping the substrate/position handles would
// add an abstraction with one production caller and no reuse.
#[allow(clippy::too_many_arguments)]
pub(crate) fn crouching_intent(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    jump_edges: JumpEdges,
    gravity: f32,
    dt: f32,
    collision: &CombinedCollisionWorld<'_>,
    position: &mut Vec3,
    events: &mut MovementEvents,
    eye_current: &mut f32,
) -> Option<Transition> {
    let crouched_half_height = component.capsule.half_height;
    let standing_half_height = component.standing_half_height;

    // Stand-up anchor, decided from grounded-AT-TICK-START — BEFORE the jump
    // branch below may clear `is_grounded`. This must mirror the crouch-ENTRY
    // anchor so an entry→exit cycle nets to no center drift (D4): grounded entry
    // anchors `Feet`, airborne entry anchors `Head`. A ground-origin crouch-jump
    // clears `is_grounded` before the stand-up resize runs, so reading the flag
    // at the call site would wrongly pick `Head` and drive the launching feet into
    // the floor — hence the snapshot here.
    let stand_up_anchor = if component.is_grounded() {
        ResizeAnchor::Feet
    } else {
        ResizeAnchor::Head
    };

    // 1. Gravity (airborne only) — identical to `Normal`.
    if !component.is_grounded() {
        component.velocity.y += gravity * dt;
        let terminal = component.fall.terminal_velocity;
        if component.velocity.y < -terminal {
            component.velocity.y = -terminal;
        }
    }

    // 2. Jump — NEVER suppressed while crouched (D10). A grounded/coyote/buffered
    // edge or an air-jump edge fires exactly as in `Normal`. The crouch-jump
    // stand-if-clear behavior is resolved AFTER the velocity is authored (below):
    // here we only launch the arc and record that a jump fired this tick.
    let mut jumped_this_tick = false;
    if jump_edges.grounded {
        component.velocity.y = component.air.jump_velocity;
        component.set_grounded(false);
        component.jump_spent = true;
        events.jumped = true;
        jumped_this_tick = true;
    } else if jump_edges.air && air_jump_ready(component) {
        component.velocity.y = component.air.jump_velocity;
        component.air_jumps_remaining -= 1;
        component.jump_spent = true;
        events.jumped = true;
        jumped_this_tick = true;
    }

    // 3. Locomotion: ground vs air branch, mirroring `Normal` steps 4/5 but with
    // the crouch speed tier as the target (and airborne cap). Crouch is
    // omnidirectional like walk/run — the tier just sits below them.
    let ground_speed = component.ground_params.speed.crouch;
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
            let horiz = Vec2::new(component.velocity.x, component.velocity.z);
            let h_speed = horiz.length();
            if h_speed > ground_speed {
                let scale = ground_speed / h_speed;
                component.velocity.x *= scale;
                component.velocity.z *= scale;
            }
        }
    }

    // 4. Ground friction — same contextual decay as `Normal` step 6, using the
    // crouch tier as the cap so a crouched player bleeds to a stop / back to the
    // crouch cap rather than the run cap.
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
        let h_speed = Vec2::new(component.velocity.x, component.velocity.z).length();
        if h_speed > ground_speed * OVERSPEED_BLEED_MARGIN {
            let drop = (h_speed - ground_speed) * GROUND_STOP_FRICTION * dt;
            let new_speed = (h_speed - drop).max(ground_speed);
            let scale = new_speed / h_speed;
            component.velocity.x *= scale;
            component.velocity.z *= scale;
        }
    }

    // 5. Eye smoothing (D3). Ease `eye_current` toward the crouched eye target
    // with a framerate-independent exponential approach and write it into the
    // live capsule for the camera follow. `crouch` is `Some` here — the state was
    // only entered when it was — but fall back gracefully (no eye change) if a
    // descriptor swap cleared it mid-crouch.
    if let Some(crouch) = component.crouch.as_ref() {
        let target_eye = crouch.eye_height;
        let rate = crouch.transition_rate;
        let alpha = 1.0 - (-rate * dt).exp();
        *eye_current += (target_eye - *eye_current) * alpha;
        component.capsule.eye_height = *eye_current;
    }

    // 6. Stand-up decision. The `Dash` transition stays available from
    // `Crouching` (D10) — check it first so a dash press exits crouch into the
    // dash burst regardless of crouch/jump state.
    if input.dash_pressed {
        if let Some(transition) = try_enter_dash(component, input) {
            return Some(transition);
        }
    }

    // Crouch-jump (D10): a jump fired this tick while `crouch_intent` is STILL
    // active. Probe headroom — clear ⇒ stand (resize, shift the center up) and
    // exit to `Normal` carrying the jump just launched; blocked ⇒ stay crouched,
    // the jump still applies (lower arc). Either way the jump is never swallowed.
    if jumped_this_tick && input.crouch_intent {
        if standup_clearance_probe(
            component,
            collision,
            *position,
            crouched_half_height,
            standing_half_height,
        ) {
            stand_up_resize(component, position, stand_up_anchor);
            *eye_current = component.capsule.eye_height;
            return Some(stand_up_transition());
        }
        // Blocked: remain `Crouching` with the crouched capsule, jump applied.
        return None;
    }

    // Stand-up on release (D7): `crouch_intent` inactive. Probe the standing
    // capsule upward; CLEAR ⇒ resize to standing (center rises, feet planted) and
    // exit to `Normal`; BLOCKED ⇒ stay crouched and retry next tick.
    if !input.crouch_intent
        && standup_clearance_probe(
            component,
            collision,
            *position,
            crouched_half_height,
            standing_half_height,
        )
    {
        stand_up_resize(component, position, stand_up_anchor);
        *eye_current = component.capsule.eye_height;
        return Some(stand_up_transition());
    }

    None
}
