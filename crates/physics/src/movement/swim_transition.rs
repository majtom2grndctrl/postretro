// Swim entry and exit by immersion at the tick seam, the standing capsule on entry and the fluid-jump re-entry latch.
// See: context/lib/movement.md §4

use glam::Vec3;

use crate::collision::moving::CombinedCollisionWorld;
use crate::movement::carry::CarryRule;
use crate::movement::dispatch::stand_up_resize;
use crate::movement::environment::resolve_standing;
use crate::movement::intents::{fluid_jump_input, leave_swim};
use crate::movement::substrate::{ResizeAnchor, standup_clearance_probe, upward_clearance_probe};
use crate::movement::{MovementInput, Transition};
use postretro_collision::{EnvironmentView, ResolvedEnvironment};
use postretro_foundation::{MovementState, PlayerMovementComponent};

/// Margin added to the rise a fluid jump needs, metres, so a jump predicted to
/// clear `exit_depth` by a rounding error still takes the capsule clearly below
/// it. One centimetre: well under any authored depth band, well over float
/// noise on the immersion fraction times the capsule length.
const FLUID_JUMP_RISE_MARGIN: f32 = 0.01;

/// What the swim seam decided for this tick: the environment-driven edge, if
/// any, and whether the swim intent fires a fluid jump. The fact is computed
/// here because qualifying needs a headroom cast, and intents never query
/// collision (`movement.md` §4).
pub(super) struct SwimSeam {
    pub(super) transition: Option<Transition>,
    pub(super) fluid_jump: bool,
}

impl SwimSeam {
    const NONE: Self = Self {
        transition: None,
        fluid_jump: false,
    };
}

/// The environment-driven swim transition for this tick, evaluated against the
/// tick-start resolve after the protected layers are peeled and before the
/// state intent runs, so the entered state's intent owns the tick. Dispatch
/// applies the returned edge and its carry. Every rule reads `resolved`, a pure
/// function of the replayed position, plus the combined collision world (static
/// geometry and active movers, which prediction replays too), so prediction and
/// replay take the same edges on the same ticks.
///
/// - **Latch.** The fluid-jump latch clears on any tick whose immersion is below
///   `exit_depth`; while set, swim cannot re-enter (`research.md` P10).
/// - **Fluid jump.** Qualifies only for a pawn already in `Swim` at tick start
///   (never on the tick it enters, so a pawn falling in with jump held cannot
///   bounce straight back out), with jump held and the tick eye out, and only
///   when the jump clears the water ([`fluid_jump_clears`]). Otherwise jump at
///   the surface does nothing beyond treading.
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
) -> SwimSeam {
    let swimming = matches!(component.movement_state, MovementState::Swim { .. });
    let Some(swim) = component.swim.as_ref() else {
        component.swim_reentry_latched = false;
        return SwimSeam {
            transition: swimming.then(leave_swim),
            fluid_jump: false,
        };
    };
    let enter_depth = swim.enter_depth;
    let exit_depth = swim.exit_depth;
    let fluid_jump_velocity = swim.fluid_jump_velocity;

    if resolved.immersion < exit_depth {
        component.swim_reentry_latched = false;
    }

    if swimming {
        let fluid_jump = fluid_jump_input(input, resolved)
            && fluid_jump_clears(
                component,
                exit_depth,
                fluid_jump_velocity,
                resolved,
                collision,
                *position,
            );
        let stays = resolved.immersion >= exit_depth || fluid_jump;
        return SwimSeam {
            transition: (!stays).then(leave_swim),
            fluid_jump,
        };
    }

    if component.swim_reentry_latched {
        return SwimSeam::NONE;
    }
    let Some(fluid) = resolved.fluid else {
        return SwimSeam::NONE;
    };

    let crouched_half_height = component.capsule.half_height;
    let standing_half_height = component.standing_half_height;
    if crouched_half_height >= standing_half_height {
        return SwimSeam {
            transition: (resolved.immersion >= enter_depth).then_some(enter_swim(fluid)),
            fluid_jump: false,
        };
    }

    if !standup_clearance_probe(
        component,
        collision,
        *position,
        crouched_half_height,
        standing_half_height,
    ) {
        return SwimSeam::NONE;
    }
    let standing = resolve_standing(component, view, *position);
    if standing.immersion < enter_depth {
        return SwimSeam::NONE;
    }
    // Feet-anchored: the probe above cleared exactly that growth, and a
    // swimmer's crouch-entry anchor has no gameplay meaning to mirror.
    stand_up_resize(component, position, ResizeAnchor::Feet);
    let fluid = standing.fluid.unwrap_or(fluid);
    *resolved = standing;
    SwimSeam {
        transition: Some(enter_swim(fluid)),
        fluid_jump: false,
    }
}

/// Whether a fluid jump from the tick-start pose clears the water: rises far
/// enough for immersion to fall below `exit_depth`, which also clears the
/// re-entry latch it arms. A jump that cannot clear would latch swim off with
/// the pawn still in the water, sinking it unable to swim (`research.md` P10's
/// soft-lock), so it does not fire.
///
/// - **Required rise.** Immersion is the fraction of the capsule's vertical
///   segment (feet to head, `2 × (half_height + radius)`) the fluid covers.
///   Assuming the covered part is contiguous up to the surface — no second fluid
///   brush floating above it within the rise — rising `d` uncovers `d` of the
///   segment, so immersion falls below `exit_depth` once
///   `d > (immersion − exit_depth) × segment`; [`FLUID_JUMP_RISE_MARGIN`] covers
///   the strict inequality and rounding.
/// - **Ballistic rise.** The jump sets vertical velocity `v =
///   fluid_jump_velocity`; afterwards the pawn is airborne under the tick-start
///   resolve's gravity plus vertical push, `a = gravity + push.y`. With `a < 0`
///   it peaks at `v² / (2|a|)`; with `a ≥ 0` it never stops rising. The fixed
///   tick's integration (no gravity on the jump tick itself) overshoots that
///   continuous peak by about `v·dt / 2`, so the test is conservative.
/// - **Headroom.** The standing capsule must be able to sweep up the required
///   rise ([`upward_clearance_probe`], the cast crouch stand-up uses), so a low
///   ceiling over the water blocks it.
fn fluid_jump_clears(
    component: &PlayerMovementComponent,
    exit_depth: f32,
    fluid_jump_velocity: f32,
    resolved: &ResolvedEnvironment,
    collision: &CombinedCollisionWorld<'_>,
    position: Vec3,
) -> bool {
    let half_height = component.capsule.half_height;
    let segment = 2.0 * (half_height + component.capsule.radius);
    let required = ((resolved.immersion - exit_depth) * segment).max(0.0) + FLUID_JUMP_RISE_MARGIN;
    let acceleration = resolved.gravity + resolved.push.y;
    if acceleration < 0.0 {
        let ballistic = fluid_jump_velocity * fluid_jump_velocity / (2.0 * -acceleration);
        if ballistic < required {
            return false;
        }
    }
    upward_clearance_probe(component, collision, position, half_height, required)
}

/// Any state → `Swim`, momentum unchanged. `KEEP_ALL` folds a dash or slide
/// boost into swim locomotion, where drag bleeds it.
fn enter_swim(fluid: u16) -> Transition {
    Transition {
        next: MovementState::Swim { fluid },
        carry: CarryRule::KEEP_ALL,
    }
}
