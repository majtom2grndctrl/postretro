// Behavior-graph evaluation: the ordered-guard transition selector and the
// verb → engine-action vocabulary the tick applies around it.
// See: context/lib/entity_model.md §5 (fixed-tick game logic) ·
//      context/lib/scripting.md §11 (IR substrate — bind once, evaluate per tick)

// Everything here is pure: it reads a graph, its bound guards, and a refreshed
// scope, and reports what the tick should do. No registry, no `App`, no time —
// the registry reads/writes stay in the tick module.

use postretro_entities::components::brain::graph_state_index;
use postretro_foundation::{
    ActionVerb, BehaviorGraphDescriptor, BehaviorStateDescriptor, IrValue, MotionVerb, eval_value,
};

use super::brain_programs::BrainEntityPrograms;
use super::brain_scope::BrainScope;
use super::engine_floor::SteeringIntent;

/// Pick the state the brain moves to this tick, or `None` to stay put.
///
/// THE pluggable planner seam. A later planner replaces this function WHOLE —
/// the ordering contract lives entirely inside it, so nothing in the caller has
/// to be reordered (or even read) to swap the policy:
///
/// - `interrupts` first, in declaration order, then the current state's own
///   `transitions`, in declaration order; the first guard evaluating to
///   `IrValue::Bool(true)` wins.
/// - An interrupt whose `to` IS the current state is skipped: interrupts fire as
///   actual state transitions, never as self re-entry.
/// - Guards are evaluated every tick. No state, action, or animation blocks
///   evaluation — commitment windows are authored as `@brain.timeInStateMs`
///   guards, not engine-side latches.
/// - A `None` program is a disabled edge (its guard failed to bind): permanently
///   false, never fatal.
///
/// Total: an out-of-range `state_index`, or a winning edge whose `to` names no
/// declared state, reports "stay put". Zero-alloc, as the per-tick guard window
/// requires.
pub(super) fn select_transition(
    graph: &BehaviorGraphDescriptor,
    bound: &BrainEntityPrograms,
    scope: &BrainScope,
    state_index: usize,
) -> Option<usize> {
    let (current_name, current) = graph.states.iter().nth(state_index)?;
    graph
        .interrupts
        .iter()
        .zip(bound.interrupts())
        .filter(|(interrupt, _)| interrupt.to != *current_name)
        .chain(
            current
                .transitions
                .iter()
                .zip(bound.transitions(state_index)),
        )
        .find(|(_, program)| {
            program
                .as_ref()
                .is_some_and(|program| eval_value(program, scope) == IrValue::Bool(true))
        })
        .and_then(|(transition, _)| graph_state_index(graph, &transition.to))
}

/// The declared state at `index` in the graph's resolved state list.
pub(super) fn state_at(
    graph: &BehaviorGraphDescriptor,
    index: usize,
) -> Option<&BehaviorStateDescriptor> {
    graph.states.values().nth(index)
}

/// Whether the state at `index` pursues a target.
///
/// This is the engine floor's "engaged" test: a brain retains its acquired pawn
/// across ticks only while the state it sits in actually chases, so a resting or
/// frozen brain re-ranks candidates from scratch instead of honoring a stale id.
pub(super) fn chases(graph: &BehaviorGraphDescriptor, index: usize) -> bool {
    state_at(graph, index).is_some_and(|state| steering_for(state.motion) == SteeringIntent::Chase)
}

/// The index of the graph's `initial` state — the state the engine floor forces
/// when the aggro gate is closed or no target exists.
pub(super) fn initial_index(graph: &BehaviorGraphDescriptor) -> Option<usize> {
    graph_state_index(graph, &graph.initial)
}

/// What the steering layer does for a state's motion verb.
///
/// The naming inversion is deliberate and load-bearing: the `hold` VERB stands
/// still, which the steering layer expresses by CLEARING the destination, while
/// the `freeze` VERB touches nothing, which is the steering layer's HOLD.
pub(super) fn steering_for(motion: MotionVerb) -> SteeringIntent {
    match motion {
        MotionVerb::ChaseTarget => SteeringIntent::Chase,
        MotionVerb::Hold => SteeringIntent::Clear,
        MotionVerb::Freeze => SteeringIntent::Hold,
    }
}

/// Whether `state` is a LOCOMOTION state: it chases, and takes no action of its
/// own while doing so.
///
/// Such a state's animation is a travel cycle, which is only correct while the
/// agent is actually travelling — so it yields to the graph's rest animation at
/// a standstill ([`animation_for_state`]), and it is the state off-host
/// presentation derives its walk-playback reference from. A chasing state that
/// DOES declare an action (the lowered `attack`) is not locomotion: its
/// animation plays regardless of speed.
fn is_locomotion_state(state: &BehaviorStateDescriptor) -> bool {
    state.motion == MotionVerb::ChaseTarget && state.action.is_none()
}

/// The graph's locomotion animation-state name: the first locomotion state's, in
/// resolved-state order. `None` for a graph that never travels.
pub(crate) fn locomotion_animation(graph: &BehaviorGraphDescriptor) -> Option<&str> {
    graph
        .states
        .values()
        .find(|state| is_locomotion_state(state))
        .map(|state| state.animation.as_str())
}

/// The graph's rest animation-state name: the `initial` state's.
pub(crate) fn rest_animation(graph: &BehaviorGraphDescriptor) -> Option<&str> {
    graph
        .states
        .get(&graph.initial)
        .map(|state| state.animation.as_str())
}

/// The animation-state name to request for the state at `index`.
///
/// A locomotion state at a standstill substitutes the graph's rest animation:
/// the agent has arrived, is blocked, or was never given a destination, and its
/// travel cycle would slide in place. Every other state keeps its own animation.
pub(super) fn animation_for_state(
    graph: &BehaviorGraphDescriptor,
    index: usize,
    moving: bool,
) -> Option<&str> {
    let state = state_at(graph, index)?;
    if !moving && is_locomotion_state(state) {
        return rest_animation(graph);
    }
    Some(state.animation.as_str())
}

/// The cooldown-gated action a state takes while it is current.
pub(super) fn action_for_state(
    graph: &BehaviorGraphDescriptor,
    index: usize,
) -> Option<ActionVerb> {
    state_at(graph, index)?.action
}
