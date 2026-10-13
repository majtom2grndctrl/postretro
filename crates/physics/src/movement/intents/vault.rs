// Vault-state velocity intent: follow the mantle path, then hand back to `Normal`.
// See: context/lib/movement.md §4, §6

use glam::Vec3;

use crate::collision::SKIN_DISTANCE;
use crate::collision::moving::CombinedCollisionWorld;
use crate::movement::Transition;
use crate::movement::carry::CarryRule;
use crate::movement::vault::{VaultFrame, path_feet};
use postretro_foundation::{MovementState, PlayerMovementComponent};

/// How far, metres, the pawn may sit from the path point it was steered to
/// before the vault counts as blocked and aborts. A pawn on the path misses it
/// only by collision skin and float noise; knockback or a wall that stops the
/// rise moves it further in one tick.
const BLOCKED_TOLERANCE: f32 = 0.1;

/// Downward speed on the exit tick, metres/sec, so the substrate's sweep finds
/// the top and the pawn leaves the vault grounded.
const EXIT_SETTLE_SPEED: f32 = 1.0;

/// Committed mantle along the path the entry probe cleared. Each tick the
/// velocity is the displacement to the next path point over `dt`: no gravity,
/// no input, and the substrate still resolves collision. Dash, jump and crouch
/// input are ignored for the whole vault.
///
/// - **Blocked.** A pawn further than [`BLOCKED_TOLERANCE`] from where the path
///   put it (knockback, a wall that moved in, a mover that dropped the lip)
///   aborts to `Normal`, airborne unless the substrate grounds it.
/// - **Mover gone.** A target on a mover with no pose this tick aborts the same
///   way.
/// - **Exit.** Once `elapsed_ms` reaches the duration the pawn stands on the
///   target; the exit tick restores the entry horizontal speed along the exit
///   direction with a small downward settle and returns to `Normal`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn vault_intent(
    component: &mut PlayerMovementComponent,
    dt: f32,
    collision: &CombinedCollisionWorld<'_>,
    position: Vec3,
    elapsed_ms: &mut f32,
    target: Vec3,
    start_offset: Vec3,
    mover: Option<u32>,
    entry_speed: f32,
) -> Option<Transition> {
    let Some(duration_ms) = component.vault.as_ref().map(|vault| vault.duration_ms) else {
        return Some(abort(component));
    };
    let Some(frame) = VaultFrame::resolve(mover, collision) else {
        return Some(abort(component));
    };
    let center_offset = Vec3::Y * (component.capsule.half_height + component.capsule.radius);
    let progress = (*elapsed_ms / duration_ms).min(1.0);
    let expected = frame.to_world(path_feet(target, start_offset, progress)) + center_offset;
    if position.distance(expected) > BLOCKED_TOLERANCE {
        return Some(abort(component));
    }

    if *elapsed_ms >= duration_ms {
        let exit_dir = frame
            .rotate(Vec3::new(-start_offset.x, 0.0, -start_offset.z))
            .normalize_or_zero();
        component.velocity = exit_dir * entry_speed + Vec3::NEG_Y * EXIT_SETTLE_SPEED;
        return Some(exit());
    }

    *elapsed_ms = (*elapsed_ms + dt * 1000.0).min(duration_ms);
    let next_progress = *elapsed_ms / duration_ms;
    let next = frame.to_world(path_feet(target, start_offset, next_progress)) + center_offset;
    component.velocity = (next - position) / dt;
    // Rising off a floor is a launch, as a jump is: the substrate must not clamp
    // the rise or stick the pawn back down.
    if component.velocity.y > SKIN_DISTANCE && component.is_grounded() {
        component.set_grounded(false);
    }
    None
}

fn exit() -> Transition {
    Transition {
        next: MovementState::Normal,
        carry: CarryRule::KEEP_ALL,
    }
}

fn abort(component: &mut PlayerMovementComponent) -> Transition {
    component.velocity = Vec3::ZERO;
    exit()
}
