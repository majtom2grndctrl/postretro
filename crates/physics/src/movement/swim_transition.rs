// Swim entry and exit by immersion at the tick seam, the standing capsule on entry and the fluid-jump re-entry latch.
// See: context/lib/movement.md §4

use glam::Vec3;

use crate::collision::moving::CombinedCollisionWorld;
use crate::movement::carry::CarryRule;
use crate::movement::dispatch::stand_up_resize;
use crate::movement::environment::resolve_standing;
use crate::movement::intents::{fluid_jump_qualifies, leave_swim};
use crate::movement::substrate::{ResizeAnchor, standup_clearance_probe};
use crate::movement::{MovementInput, Transition};
use postretro_collision::{EnvironmentView, ResolvedEnvironment};
use postretro_foundation::{MovementState, PlayerMovementComponent};

/// The environment-driven swim transition for this tick, evaluated against the
/// tick-start resolve after the protected layers are peeled and before the
/// state intent runs, so the entered state's intent owns the tick. Dispatch
/// applies the returned edge and its carry. Every rule reads `resolved`, a pure
/// function of the replayed position, so prediction and replay take the same
/// edges on the same ticks.
///
/// - **Latch.** The fluid-jump latch clears on any tick whose immersion is below
///   `exit_depth`; while set, swim cannot re-enter (`research.md` P10).
/// - **Exit.** `Swim` leaves for `Normal` when immersion is below `exit_depth`
///   (exactly at it stays), unless a fluid jump qualifies this tick: the jump
///   wins the tie, and the swim intent fires it.
/// - **Entry.** Any other state enters `Swim` when the swimming capsule's
///   immersion is at or above `enter_depth`, the component has a `swim` block,
///   and the latch is clear. Between the two thresholds nothing toggles.
/// - **Standing capsule.** A crouched capsule (crouch or slide) stands up on
///   entry, feet planted, through the same headroom probe and resize crouch
///   stand-up uses; entry is tested against the STANDING capsule's immersion,
///   and on entry `resolved` is replaced by that resolve so the swim intent
///   reads the eye it will swim with. Blocked headroom (a flooded crawlspace
///   lower than the standing capsule) keeps the current state: the pawn wades
///   crouched under gravity and retries every tick.
/// - **No swim block.** The pawn never swims and walks fluid floors; a hot swap
///   that removes the block mid-swim leaves to `Normal`.
pub(super) fn environment_transition(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    view: EnvironmentView<'_>,
    resolved: &mut ResolvedEnvironment,
    collision: &CombinedCollisionWorld<'_>,
    position: &mut Vec3,
) -> Option<Transition> {
    let swimming = matches!(component.movement_state, MovementState::Swim { .. });
    let Some(swim) = component.swim.as_ref() else {
        component.swim_reentry_latched = false;
        return swimming.then(leave_swim);
    };
    let enter_depth = swim.enter_depth;
    let exit_depth = swim.exit_depth;

    if resolved.immersion < exit_depth {
        component.swim_reentry_latched = false;
    }

    if swimming {
        if resolved.immersion >= exit_depth || fluid_jump_qualifies(input, resolved) {
            return None;
        }
        return Some(leave_swim());
    }

    if component.swim_reentry_latched {
        return None;
    }
    let fluid = resolved.fluid?;

    let crouched_half_height = component.capsule.half_height;
    let standing_half_height = component.standing_half_height;
    if crouched_half_height >= standing_half_height {
        return (resolved.immersion >= enter_depth).then_some(enter_swim(fluid));
    }

    if !standup_clearance_probe(
        component,
        collision,
        *position,
        crouched_half_height,
        standing_half_height,
    ) {
        return None;
    }
    let standing = resolve_standing(component, view, *position);
    if standing.immersion < enter_depth {
        return None;
    }
    // Feet-anchored: the probe above cleared exactly that growth, and a
    // swimmer's crouch-entry anchor has no gameplay meaning to mirror.
    stand_up_resize(component, position, ResizeAnchor::Feet);
    let fluid = standing.fluid.unwrap_or(fluid);
    *resolved = standing;
    Some(enter_swim(fluid))
}

/// Any state → `Swim`, momentum unchanged. `KEEP_ALL` folds a dash or slide
/// boost into swim locomotion, where drag bleeds it.
fn enter_swim(fluid: u16) -> Transition {
    Transition {
        next: MovementState::Swim { fluid },
        carry: CarryRule::KEEP_ALL,
    }
}
