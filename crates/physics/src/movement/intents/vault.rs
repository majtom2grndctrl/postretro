// Vault-state velocity intent: follow the mantle path, then hand back to `Normal`.
// See: context/lib/movement.md §4, §6

use glam::Vec3;

use crate::collision::SKIN_DISTANCE;
use crate::collision::moving::CombinedCollisionWorld;
use crate::movement::Transition;
use crate::movement::carry::CarryRule;
use crate::movement::mover_carry::apply_mover_carry;
use crate::movement::vault::VaultFrame;
use crate::movement::vault_path::{VaultPath, vault_clock, vault_ticks_run};
use postretro_foundation::{GroundRef, MovementState, PlayerMovementComponent};

/// How far, metres, the pawn may sit from the path point it was steered to
/// before the vault counts as blocked and aborts. A pawn on the path misses it
/// only by collision skin and float noise; knockback or a wall that stops the
/// rise moves it further in one tick. Mover speed does not count against it:
/// a pawn vaulting onto a mover rides that mover's carry, so however fast the
/// target moves, steady or not, the pawn moves with it. Without the ride an
/// airborne pawn would trail the target by one tick of mover motion, and any
/// mover above this per tick (6 m/s at 60 Hz) would abort every vault onto it.
const BLOCKED_TOLERANCE: f32 = 0.1;

/// Downward speed on the exit tick, metres/sec, so the substrate's sweep finds
/// the top and the pawn leaves the vault grounded.
const EXIT_SETTLE_SPEED: f32 = 1.0;

/// Committed mantle along the path the entry probe cleared. Each tick the
/// velocity is the displacement to the next path point over `dt`: no gravity,
/// no input, and the substrate still resolves collision. Dash, jump and crouch
/// input are ignored for the whole vault.
///
/// - **Ride.** The step starts where mover carry (`carry_ground`) leaves the
///   pawn, since the substrate applies that carry before it moves the body.
///   After the entry tick, which carries by the ground the pawn stood on, the
///   carry is the target's own mover, so the pawn moves with the target as a
///   grounded rider does, and each tick's path point, resolved through this
///   tick's pose, stays where the next tick looks for it. Release velocity
///   follows the same ride, so an abort keeps the mover's velocity.
/// - **Blocked.** A pawn further than [`BLOCKED_TOLERANCE`] from the point it
///   was steered to (knockback, a wall that moved in, a mover that dropped the
///   lip) aborts to `Normal`, airborne unless the substrate grounds it.
/// - **Mover gone.** A target on a mover with no pose this tick aborts the same
///   way.
/// - **Clock.** `elapsed_ms` is the vault clock: whole path ticks, written as
///   [`vault_clock`] each tick rather than accumulated, so the path's tick
///   count, this exit and the fire gate's `elapsed_ms < duration_ms` all read
///   the same tick as the end.
/// - **Exit.** Once the path's ticks have run with the pawn steered to the
///   target, the exit tick restores the entry horizontal speed along the exit
///   direction, closes any vertical gap to the target, and settles down onto
///   the top as it returns to `Normal`. The path ends moving at that speed
///   ([`VaultPath`]), so the exit adds no jump. A path tick whose forward the
///   lip held back ([`VaultPath::aimed`]) with the duration spent still owes
///   the move onto the top, so that one tick runs it before the exit.
#[allow(clippy::too_many_arguments)]
pub(crate) fn vault_intent(
    component: &mut PlayerMovementComponent,
    dt: f32,
    collision: &CombinedCollisionWorld<'_>,
    position: Vec3,
    carry_ground: GroundRef,
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
    let step_ms = dt * 1000.0;
    let center_offset = Vec3::Y * (component.capsule.half_height + component.capsule.radius);
    let carried = apply_mover_carry(position, carry_ground, collision);
    let path = VaultPath::new(
        target,
        start_offset,
        entry_speed,
        duration_ms,
        step_ms,
        component.capsule.radius,
    );
    let ticks_run = vault_ticks_run(*elapsed_ms, step_ms);
    let (aimed, at_target) = path.aimed(ticks_run);
    let expected = frame.to_world(aimed) + center_offset;
    let off_path = carried.distance(expected);
    // Fail closed: a NaN distance compares false against the tolerance, so
    // abort explicitly rather than steer from a non-finite pose. This guards the
    // vault's own path and state only; the substrate still applies this tick's
    // mover carry.
    if off_path.is_nan() || off_path > BLOCKED_TOLERANCE {
        return Some(abort(component));
    }

    if path.finished(ticks_run) && at_target {
        let exit_dir = frame
            .rotate(Vec3::new(-start_offset.x, 0.0, -start_offset.z))
            .normalize_or_zero();
        // The settle must meet the top where it is this tick: catch up to any
        // vertical gap the ride left between the pawn and the target.
        let catch_up = (frame.to_world(target).y + center_offset.y - carried.y) / dt;
        component.velocity = exit_dir * entry_speed + Vec3::Y * (catch_up - EXIT_SETTLE_SPEED);
        return Some(exit());
    }

    // The clock is never clamped to the duration, so the next tick can recover
    // this step's start (`VaultPath::aimed`).
    let next_tick = ticks_run.saturating_add(1);
    *elapsed_ms = vault_clock(next_tick, step_ms);
    let next = frame.to_world(path.aimed(next_tick).0) + center_offset;
    component.velocity = (next - carried) / dt;
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

/// Leave for `Normal` through the component's shared early-end settle, so an
/// abort leaves what death, a tuning drop and hot reload leave.
fn abort(component: &mut PlayerMovementComponent) -> Transition {
    component.settle_broken_vault();
    // The settle leaves total velocity; inside the tick `velocity` holds the
    // base alone, so peel the protected layers off again.
    component.velocity -= component.knockback_velocity + component.push_velocity;
    exit()
}
