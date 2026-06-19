// Per-tick navigation-agent steering: refresh paths under a replan budget,
// steer toward the current waypoint, separate from crowding neighbors, and move
// each agent through the world via the collide-and-slide harness.
//
// This is the production caller for `nav::find_path` and the agent component. It
// owns the replan policy (per-tick budget + per-agent staleness gate), the
// waypoint-following loop, and the O(n²) separation pass; the actual capsule
// sweep lives in `agent::collide_and_slide`.
//
// See: context/lib/build_pipeline.md §Navigation bake (pathfinding query surface)
//      context/lib/entity_model.md §7 (collision), §5 (fixed-tick game logic)
//      context/lib/movement.md §1 (custom-kinematic capsule, collision-only)

use glam::Vec3;

use crate::agent::{AgentCapsule, collide_and_slide};
use crate::collision::CollisionWorld;
use crate::nav::{NavGraph, distance_xz};
use crate::scripting::components::agent::AgentComponent;
use crate::scripting::registry::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform,
};

// Re-export the one-shot path query so the steering API and its consumers
// (plan 2) import it from one place rather than reaching into `nav` directly.
pub(crate) use crate::nav::find_path;

/// Maximum number of agents that may recompute a path in a single tick. Bounds
/// the per-frame pathfinding cost regardless of how many agents simultaneously
/// want a fresh route — overflow waits for a later tick (the staleness gate
/// keeps each waiting agent eligible). Sized for a handful of active pursuers
/// per fixed tick; raise it only behind a measured pathfinding bottleneck.
pub(crate) const REPLAN_BUDGET_PER_TICK: u32 = 4;

/// Ticks an agent must wait between path recomputations for the SAME
/// destination. A live path is refreshed at most this often; a FAILED plan
/// (no route) is likewise gated by this window so a permanently-blocked agent
/// costs at most one replan per window rather than one every tick (the
/// replan-starvation gate). A destination move bypasses this (resets the
/// cooldown to 0), so a newly-issued order plans on the next tick.
pub(crate) const REPLAN_STALENESS_TICKS: u32 = 30;

/// Arrival radius as a multiple of the agent capsule radius. The cursor advances
/// to the next waypoint once the agent is within `ARRIVAL_RADIUS_FACTOR * radius`
/// of the current one (XZ); the destination counts as reached within the same
/// band of the final waypoint. Derived from the capsule, not a magic constant,
/// so a fatter agent gets a proportionally wider acceptance window.
const ARRIVAL_RADIUS_FACTOR: f32 = 1.5;

/// Separation radius as a multiple of the agent capsule radius, measured between
/// capsule centers. Two agents push apart when their center distance is below
/// `radius_a + radius_b` (capsules overlap) OR below this comfort band — a soft
/// personal-space cushion that resolves crowding before contact.
const SEPARATION_RADIUS_FACTOR: f32 = 2.5;

/// Strength of the separation push relative to the agent's `move_speed`. The
/// summed neighbor-avoidance vector is clamped to this fraction of top speed so
/// separation nudges agents apart without overwhelming goal-directed steering.
const SEPARATION_STRENGTH: f32 = 0.6;

/// Observable result of one steering tick. `replans` makes the per-tick replan
/// bound testable: it is the count of agents that actually recomputed a path
/// this tick, which must never exceed [`REPLAN_BUDGET_PER_TICK`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct AgentTickResult {
    pub(crate) replans: u32,
}

/// Read-back of one agent's path-following state. The steering API plan 2 drives
/// reads this to decide AI behavior; every field is derived from the live
/// component, never recomputed here.
///
/// The steering-API surface (`set_destination`/`clear_destination`/`path_state`
/// and this struct) is consumed by the enemy-AI FSM tick
/// (`scripting/systems/ai.rs`), which drives `set_destination`/`clear_destination`
/// per chasing enemy and reads `path_state` for arrival/blocked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AgentPathState {
    /// The agent currently has a destination set (`Some`).
    pub(crate) has_destination: bool,
    /// The agent holds a non-empty path toward that destination.
    pub(crate) has_path: bool,
    /// The agent reached its destination (within the arrival radius).
    pub(crate) arrived: bool,
    /// The agent has a destination but pathfinding found no route to it.
    pub(crate) blocked: bool,
    /// XZ distance from the agent's current position to its destination.
    /// `0.0` when there is no destination.
    pub(crate) distance_to_destination: f32,
    /// Current agent position (capsule center, world space).
    pub(crate) position: Vec3,
    /// Live agent velocity (world space) after the last tick.
    pub(crate) velocity: Vec3,
}

/// Set (or replace) an agent's destination. Clears the prior plan so the next
/// tick replans immediately toward the new goal: the path is dropped, the
/// replan cooldown is reset to 0, and the `arrived`/`blocked` status is cleared.
/// No-op (silently) when the entity has no agent component.
pub(crate) fn set_destination(registry: &mut EntityRegistry, agent: EntityId, pos: Vec3) {
    let Ok(component) = registry.get_component::<AgentComponent>(agent) else {
        return;
    };
    let mut updated = component.clone();
    updated.destination = Some(pos);
    updated.planned_destination = None;
    updated.path.clear();
    updated.waypoint_cursor = 0;
    updated.replan_cooldown_ticks = 0;
    updated.arrived = false;
    updated.blocked = false;
    let _ = registry.set_component(agent, updated);
}

/// Clear an agent's destination: drops the path and stops the agent (it keeps
/// its grounded state but no longer steers). No-op when the entity has no agent
/// component.
pub(crate) fn clear_destination(registry: &mut EntityRegistry, agent: EntityId) {
    let Ok(component) = registry.get_component::<AgentComponent>(agent) else {
        return;
    };
    let mut updated = component.clone();
    updated.destination = None;
    updated.planned_destination = None;
    updated.path.clear();
    updated.waypoint_cursor = 0;
    updated.replan_cooldown_ticks = 0;
    updated.arrived = false;
    updated.blocked = false;
    let _ = registry.set_component(agent, updated);
}

/// Read one agent's path-following state. Returns `None` when the entity has no
/// agent component (or is stale). The position is read from the agent's
/// `Transform`; the rest from the agent component.
pub(crate) fn path_state(registry: &EntityRegistry, agent: EntityId) -> Option<AgentPathState> {
    let component = registry.get_component::<AgentComponent>(agent).ok()?;
    let position = registry
        .get_component::<Transform>(agent)
        .map(|t| t.position)
        .unwrap_or(Vec3::ZERO);
    let distance_to_destination = component
        .destination
        .map(|dest| distance_xz(position, dest))
        .unwrap_or(0.0);
    Some(AgentPathState {
        has_destination: component.destination.is_some(),
        has_path: !component.path.is_empty(),
        arrived: component.arrived,
        blocked: component.blocked,
        distance_to_destination,
        position,
        velocity: component.velocity,
    })
}

/// One agent's start-of-tick snapshot for the order-independent separation pass.
/// Positions are sampled BEFORE any agent moves so neighbor avoidance reads a
/// consistent frame (agent A's push from B uses the same B position B's push
/// from A uses), making the pass independent of iteration order.
#[derive(Clone, Copy)]
struct AgentSnapshot {
    id: EntityId,
    position: Vec3,
    radius: f32,
}

/// Per-tick agent steering. For every agent with a destination: refresh its path
/// under the replan budget + staleness gate, steer toward the current waypoint,
/// add the separation term, move through the world via the collide-and-slide
/// harness, advance the waypoint cursor on arrival, and set arrived/blocked.
///
/// `nav_graph` is `None` when the loaded map has no navmesh bake; agents then
/// cannot plan (every destination resolves to blocked). `gravity` is the world
/// gravity scalar (negative); `dt` is the fixed tick delta.
///
/// Returns the count of agents that recomputed a path this tick — bounded by
/// [`REPLAN_BUDGET_PER_TICK`].
pub(crate) fn tick(
    registry: &mut EntityRegistry,
    collision_world: &CollisionWorld,
    nav_graph: Option<&NavGraph>,
    gravity: f32,
    dt: f32,
) -> AgentTickResult {
    // Start-of-tick position snapshot for separation. Built once, read by every
    // agent's neighbor pass, so the result is order-independent.
    let snapshot: Vec<AgentSnapshot> = registry
        .iter_with_kind(ComponentKind::Agent)
        .filter_map(|(id, value)| {
            let ComponentValue::Agent(agent) = value else {
                return None;
            };
            let position = registry
                .get_component::<Transform>(id)
                .map(|t| t.position)
                .unwrap_or(Vec3::ZERO);
            Some(AgentSnapshot {
                id,
                position,
                radius: agent.radius,
            })
        })
        .collect();

    let mut replans = 0u32;

    // Drive each agent. The snapshot holds the ids in slot order; we mutate one
    // agent at a time, reading its live component (set/clear destination may have
    // mutated it after the snapshot, which is fine — only positions are frozen).
    for current in &snapshot {
        let Ok(component) = registry.get_component::<AgentComponent>(current.id) else {
            continue;
        };
        let mut agent = component.clone();
        let position = registry
            .get_component::<Transform>(current.id)
            .map(|t| t.position)
            .unwrap_or(current.position);

        // Tick down the staleness cooldown regardless of what happens below.
        agent.replan_cooldown_ticks = agent.replan_cooldown_ticks.saturating_sub(1);

        let Some(destination) = agent.destination else {
            // No destination: idle. Persist the cooldown decrement and stop.
            let _ = registry.set_component(current.id, agent);
            continue;
        };

        // Replan policy. The agent qualifies for a fresh path when:
        //   - the destination moved since the last plan (immediate), OR
        //   - the cooldown has elapsed (staleness refresh / failed-plan retry).
        // The cooldown gate covers BOTH a live path going stale AND a failed
        // (empty) plan — a permanently-blocked agent therefore re-spends a
        // budget slot at most once per `REPLAN_STALENESS_TICKS`, never every
        // tick (the replan-starvation gate).
        let destination_moved = agent.planned_destination != Some(destination);
        if destination_moved {
            agent.replan_cooldown_ticks = 0;
        }
        let wants_replan = destination_moved || agent.replan_cooldown_ticks == 0;

        if wants_replan && replans < REPLAN_BUDGET_PER_TICK {
            replans += 1;
            agent.replan_cooldown_ticks = REPLAN_STALENESS_TICKS;
            agent.planned_destination = Some(destination);
            match nav_graph.and_then(|graph| find_path(graph, position, destination)) {
                Some(path) => {
                    agent.path = path;
                    agent.waypoint_cursor = 0;
                    agent.blocked = false;
                }
                None => {
                    // No route: blocked. Hold position; do NOT walk toward the
                    // raw destination. The cooldown (set above) keeps this from
                    // re-qualifying every tick.
                    agent.path.clear();
                    agent.waypoint_cursor = 0;
                    agent.blocked = true;
                }
            }
        }

        // Compute the goal-seeking steering velocity from the current waypoint.
        let arrival_radius = ARRIVAL_RADIUS_FACTOR * agent.radius;
        let mut desired = goal_velocity(&mut agent, position, arrival_radius);

        // Separation: sum pushes from every other agent whose capsule overlaps
        // or sits within the separation band, against the frozen snapshot.
        desired += separation(current, &agent, &snapshot);

        // Clamp horizontal speed to the agent's top speed so the combined
        // (goal + separation) vector never drives faster than `move_speed`.
        let horiz = Vec3::new(desired.x, 0.0, desired.z);
        let horiz_speed = horiz.length();
        if horiz_speed > agent.move_speed && horiz_speed > 1e-6 {
            let scale = agent.move_speed / horiz_speed;
            desired.x *= scale;
            desired.z *= scale;
        }

        // Move through the world.
        let capsule = AgentCapsule {
            radius: agent.radius,
            half_height: agent.half_height(),
            step_height: agent.step_height,
        };
        let result = collide_and_slide(
            collision_world,
            &capsule,
            position,
            Vec3::new(desired.x, 0.0, desired.z),
            agent.velocity.y,
            gravity,
            dt,
        );

        agent.velocity = result.velocity;
        agent.is_grounded = result.grounded;

        // Write back the resolved position and the updated agent state.
        if let Ok(transform) = registry.get_component::<Transform>(current.id) {
            let mut t = *transform;
            t.position = result.position;
            let _ = registry.set_component(current.id, t);
        }
        let _ = registry.set_component(current.id, agent);
    }

    AgentTickResult { replans }
}

/// Goal-seeking steering velocity toward the current waypoint, advancing the
/// cursor as the agent reaches each waypoint within `arrival_radius` (XZ). Sets
/// `arrived` when the final waypoint is reached. Returns a velocity whose
/// horizontal magnitude is the agent's `move_speed` (zero when there is no live
/// path or the agent has arrived). Mutates `agent.waypoint_cursor` /
/// `agent.arrived` in place.
fn goal_velocity(agent: &mut AgentComponent, position: Vec3, arrival_radius: f32) -> Vec3 {
    if agent.path.is_empty() {
        return Vec3::ZERO;
    }

    // Advance the cursor past every waypoint already within the arrival radius,
    // so an agent that overshoots several close waypoints in one tick does not
    // backtrack. Stops at the last waypoint.
    while agent.waypoint_cursor < agent.path.len() {
        let target = agent.path[agent.waypoint_cursor];
        if distance_xz(position, target) <= arrival_radius
            && agent.waypoint_cursor + 1 < agent.path.len()
        {
            agent.waypoint_cursor += 1;
        } else {
            break;
        }
    }

    let target = agent.path[agent.waypoint_cursor.min(agent.path.len() - 1)];
    let is_final = agent.waypoint_cursor + 1 >= agent.path.len();
    if is_final && distance_xz(position, target) <= arrival_radius {
        agent.arrived = true;
        return Vec3::ZERO;
    }

    let to_target = Vec3::new(target.x - position.x, 0.0, target.z - position.z);
    let dist = to_target.length();
    if dist <= 1e-6 {
        return Vec3::ZERO;
    }
    (to_target / dist) * agent.move_speed
}

/// Order-independent separation steering: an O(n) scan (per agent) over the
/// frozen snapshot summing a push away from each neighbor whose capsule overlaps
/// `self` (center distance < radius sum) or sits within the comfort band
/// (`SEPARATION_RADIUS_FACTOR * radius`). Pushes are weighted by how deep the
/// overlap is (closer neighbors push harder) and clamped to a fraction of the
/// agent's top speed. Self is skipped. XZ only — agents do not push each other
/// vertically.
fn separation(current: &AgentSnapshot, agent: &AgentComponent, snapshot: &[AgentSnapshot]) -> Vec3 {
    let comfort = SEPARATION_RADIUS_FACTOR * agent.radius;
    let mut push = Vec3::ZERO;

    for other in snapshot {
        if other.id == current.id {
            continue;
        }
        let offset = Vec3::new(
            current.position.x - other.position.x,
            0.0,
            current.position.z - other.position.z,
        );
        let dist = offset.length();
        // The trigger distance: capsules touching, or within the comfort band,
        // whichever is larger.
        let trigger = (agent.radius + other.radius).max(comfort);
        if dist >= trigger {
            continue;
        }

        let dir = if dist > 1e-6 {
            offset / dist
        } else {
            // Exactly coincident: pick a deterministic lateral direction so two
            // perfectly-stacked agents still separate (entity-id breaks the tie).
            if current.id.to_raw() < other.id.to_raw() {
                Vec3::X
            } else {
                Vec3::NEG_X
            }
        };
        // Weight: 1 at full overlap, → 0 at the trigger edge. Closer pushes
        // harder, so deep overlaps resolve first.
        let weight = 1.0 - (dist / trigger);
        push += dir * weight;
    }

    if push.length_squared() <= 1e-12 {
        return Vec3::ZERO;
    }
    push.normalize() * (agent.move_speed * SEPARATION_STRENGTH)
}

#[cfg(test)]
mod tests;
