// The engine floor's non-authorable knobs: the steering-intent vocabulary the
// tick applies and the target-switch hysteresis. Acquisition runs on decide
// ticks (`decide`). None of these are graph-authorable — a behavior
// graph picks a motion verb, and this module owns what the engine does with it.
// See: context/lib/entity_model.md §2 (engine components)

use glam::Vec3;

/// XZ distance within which a fixed position goal counts as reached.
pub(crate) const POSITION_GOAL_ARRIVAL_EPSILON: f32 = 0.5;

/// Target switching hysteresis in world units on the XZ plane. A retained target
/// stays sticky unless another pawn is MORE than this much closer, preventing
/// co-op target churn when players are only slightly offset from one another.
pub(super) const TARGET_SWITCH_HYSTERESIS_DISTANCE: f32 = 1.0;

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

pub(super) fn is_meaningfully_closer(candidate_distance: f32, retained_distance: f32) -> bool {
    candidate_distance + TARGET_SWITCH_HYSTERESIS_DISTANCE < retained_distance
}
