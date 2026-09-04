// Per-state velocity intents for player movement.
// See: context/lib/movement.md §4

use glam::{Vec2, Vec3};

use crate::movement::MovementInput;
use postretro_foundation::PlayerMovementComponent;

mod crouching;
mod dash;
mod normal;

pub(super) use crouching::crouching_intent;
#[cfg(test)]
pub(super) use dash::DASH_MAX_MS;
pub(super) use dash::dash_intent;
pub(super) use normal::normal_intent;

/// Exponential-style ground deceleration (`v *= max(0, 1 - k*dt)`) — not the Q3
/// stop/slide-threshold friction model. Value matches Quake's default
/// `sv_friction` (6.0). Two use sites:
///   1. `Normal` step 6 / `apply_normal_horizontal_decay`: grounded with no
///      movement input held — the stop-friction that bleeds idle speed. Gated
///      on no-input because `pm_accelerate` already caps in-band grounded speed.
///   2. `dash_intent`'s `dash_drag == 0` grounded boost path: applied
///      UNCONDITIONALLY (no no-input gate). The boost deliberately sits above
///      the grounded cap, so the no-input gate cannot apply — a held stick must
///      still bleed the over-cap boost rather than freezing it.
///
/// Promote to `GroundParams` if per-entity friction tuning becomes necessary.
pub(super) const GROUND_STOP_FRICTION: f32 = 6.0;

/// Fractional margin above the run cap before the held-input grounded
/// over-speed bleed (`normal_intent` step 6) engages. `pm_accelerate`'s
/// projection clamp leaves sub-unit floating-point overshoot just above the cap
/// during normal running (~1e-4); reacting to that would perturb steady-state
/// motion. Real banked momentum (a dash handing off above the cap) clears this
/// margin by a wide margin, so 0.2 % cleanly separates signal from float noise.
pub(super) const OVERSPEED_BLEED_MARGIN: f32 = 1.002;

/// Apply `Normal`'s contextual horizontal decay to a horizontal velocity vector
/// in place: when grounded, the no-input stop-friction branch of `Normal` step 6
/// only; when airborne, the horizontal cap (mirroring steps 4/5). Step 6 has a
/// second grounded branch — the held-input over-speed bleed above
/// `OVERSPEED_BLEED_MARGIN` — which this helper deliberately omits: the vectors
/// `dash_intent` passes in (the retained base; the boost only when
/// `dash_drag == 0`) are already bounded below the run cap, so there is no
/// over-cap residue for that branch to act on. Reads the grounded flag and
/// friction params off the component.
pub(super) fn apply_normal_horizontal_decay(
    velocity: &mut Vec3,
    component: &PlayerMovementComponent,
    input: &MovementInput,
    ground_speed: f32,
    dt: f32,
) {
    if component.is_grounded() {
        if input.wish_dir.length_squared() < 0.001 {
            let h_speed = Vec2::new(velocity.x, velocity.z).length();
            if h_speed > 0.0 {
                let drop = h_speed * GROUND_STOP_FRICTION * dt;
                let new_speed = (h_speed - drop).max(0.0);
                let scale = new_speed / h_speed;
                velocity.x *= scale;
                velocity.z *= scale;
            }
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
