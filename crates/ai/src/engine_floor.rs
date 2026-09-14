// The engine floor's non-authorable knobs: the steering-intent vocabulary the
// tick applies, the think-stride bands that time-slice target acquisition, and
// the target-switch hysteresis. None of these are graph-authorable — a behavior
// graph picks a motion verb, and this module owns what the engine does with it.
// See: context/lib/entity_model.md §2 (engine components)

use glam::Vec3;

/// XZ distance within which a fixed position goal counts as reached.
pub(crate) const POSITION_GOAL_ARRIVAL_EPSILON: f32 = 0.5;

/// Think-stride bands. Target acquisition is time-sliced by player distance:
/// near enemies re-evaluate every tick, mid enemies every few ticks, distant
/// enemies rarely. Retained-target reads and attack-in-range/cooldown checks
/// are NOT strided — they run every tick regardless, so a strided acquisition
/// gap cannot suppress an in-range attack.
///
/// Distances are XZ ground distances (the navmesh plane); the bands are coarse
/// by design — stride is a cost knob, not a gameplay contract.
pub(super) const STRIDE_NEAR_DISTANCE: f32 = 12.0;
pub(super) const STRIDE_MID_DISTANCE: f32 = 30.0;
/// Stride divisor for each band: `1` = every tick, `n` = once every `n` ticks.
const STRIDE_NEAR: u32 = 1;
const STRIDE_MID: u32 = 4;
const STRIDE_FAR: u32 = 12;

/// Target switching hysteresis in world units on the XZ plane. A retained target
/// stays sticky unless another pawn is MORE than this much closer, preventing
/// co-op target churn when players are only slightly offset from one another.
/// The think stride (in ticks) for an enemy at `distance` (XZ) from the player:
/// `1` near, larger as the player recedes. Pure helper so the stride policy is
/// testable in isolation.
pub(crate) fn think_stride_for_distance(distance: f32) -> u32 {
    if distance <= STRIDE_NEAR_DISTANCE {
        STRIDE_NEAR
    } else if distance <= STRIDE_MID_DISTANCE {
        STRIDE_MID
    } else {
        STRIDE_FAR
    }
}

/// What the selected graph state wants the steering layer to do this tick.
/// Decoupled from the steering API itself so the pure verb mapping
/// (`graph_eval::steering_for`) carries no registry dependency — the tick
/// wrapper translates the intent into `set_destination`/`clear_destination`
/// calls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum SteeringIntent {
    /// Chase: the wrapper prefers a combat slot around the selected target and
    /// falls back to the target position. The `chaseTarget` motion verb — but
    /// only while there IS a target: with none, the tick degrades a chase to
    /// [`SteeringIntent::Clear`], since there is nothing to move relative to and
    /// the agent would otherwise keep walking to a stale destination.
    Chase,
    /// Steer toward an engine-resolved fixed world-space position.
    MoveTo(Vec3),
    /// Stand down: the wrapper clears the agent destination. The `hold` motion
    /// verb, what the engine floor forces when the aggro gate closes, and what a
    /// target-less chase degrades to.
    Clear,
    /// The `freeze` motion verb. On the tick the graph ENTERS a frozen state
    /// the wrapper clears the destination once — releasing the combat slot a
    /// freeze surrenders while continuing to walk toward it are mutually
    /// exclusive, so the walk stops — and on every later tick spent in that
    /// state it touches neither destination nor steering, so a death
    /// animation, ragdoll, or scripted mover can drive the frozen entity
    /// without this arm fighting it.
    Hold,
}
pub(super) const TARGET_SWITCH_HYSTERESIS_DISTANCE: f32 = 1.0;

pub(super) fn is_meaningfully_closer(candidate_distance: f32, retained_distance: f32) -> bool {
    candidate_distance + TARGET_SWITCH_HYSTERESIS_DISTANCE < retained_distance
}
