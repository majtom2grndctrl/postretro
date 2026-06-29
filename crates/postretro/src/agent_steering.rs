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
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform};

// Re-export the one-shot path query as part of the steering module's surface, so
// a caller that wants a path without owning an agent imports it from here rather
// than reaching into `nav` directly.
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

/// World-space XZ distance the LIVE destination may drift from the position the
/// current plan was built for (`planned_destination`) before [`tick`] wants a
/// fresh path. The comparison is CUMULATIVE drift-from-the-plan, never a
/// successive per-call delta: a destination that creeps a little each tick (a
/// chased, moving player) accrues drift against the one stored plan and only
/// crosses this band after several ticks, so a moving target cannot force a
/// replan EVERY tick and defeat the per-tick replan budget.
///
/// Sized to roughly the agent's arrival band (`ARRIVAL_RADIUS_FACTOR * radius`,
/// ~0.5 m for the canonical 0.35 m agent): a plan stays valid while the goal is
/// within about one acceptance radius of where it was planned for — close enough
/// that the existing waypoints still lead the agent to the goal. The staleness
/// window ([`REPLAN_STALENESS_TICKS`]) refreshes the path regardless; this only
/// governs how promptly a genuinely-moved goal earns an earlier replan. Replaces
/// the former successive-delta epsilon, which wiped the path on every change and
/// froze chasers beyond the budget when the target moved.
const REPLAN_DEST_THRESHOLD: f32 = 0.5;

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

/// Read-back of one agent's path-following state. The enemy-AI FSM tick
/// (`scripting/systems/ai.rs`) reads this to decide AI behavior; every field is
/// derived from the live component, never recomputed here.
///
/// The steering-API surface (`set_destination`/`clear_destination`/`path_state`
/// and this struct) is consumed by that FSM tick, which drives
/// `set_destination`/`clear_destination` per chasing enemy and reads
/// `path_state` for arrival/blocked.
///
/// A re-issued destination NEVER wipes the path: [`set_destination`] only
/// records the new target, and [`tick`] is the sole place the path is rebuilt,
/// under the per-tick replan budget. An agent that wants a fresh route but loses
/// the budget race keeps `has_path` true and keeps following its last (stale)
/// route — stale-but-moving, not frozen.
///
/// Plan-pending state: `has_destination && !has_path && !blocked && !arrived`
/// therefore means the agent has a destination but has not yet landed its FIRST
/// plan (it is waiting for a replan-budget slot) — not stuck, and not a chaser
/// mid-pursuit (which retains its path). A genuinely unroutable agent reads
/// `blocked`; an idle one reads `!has_destination`.
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

/// Set (or replace) an agent's destination.
///
/// Records `destination = pos` and NOTHING destructive. The path, waypoint
/// cursor, `planned_destination`, replan cooldown, and the `arrived`/`blocked`
/// flags are all PRESERVED — the path is the only thing keeping an agent moving
/// between replans, so re-issuing the destination must never wipe it. WHEN to
/// replan is decided solely by [`tick`], which compares the live `destination`
/// against the position the current plan was built for (`planned_destination`)
/// and rebuilds the path under the per-tick replan budget. This function only
/// updates the target; it does not touch the plan.
///
/// This decoupling is the crux of the chase loop: the primary consumer
/// (`scripting/systems/ai.rs`) re-issues the player's position EVERY tick while
/// chasing. If this wiped the path on each change, chasers beyond the per-tick
/// replan budget would end the tick with an empty path and freeze; preserving
/// the path lets them keep following their last route (stale-but-moving) until
/// a budget slot frees up. It also stops a transient call from clearing
/// `blocked` before the FSM's blocked-warn can observe it.
///
/// A non-finite `pos` is rejected as a silent no-op (matching `find_path`'s
/// finiteness guard) so a NaN/inf target never enters the steering state. Also a
/// silent no-op when the entity has no agent component.
pub(crate) fn set_destination(registry: &mut EntityRegistry, agent: EntityId, pos: Vec3) {
    if !pos.is_finite() {
        return;
    }
    let Ok(component) = registry.get_component::<AgentComponent>(agent) else {
        return;
    };
    // Record the target only; the plan (path/cursor/planned_destination/cooldown/
    // arrived/blocked) is left intact. `tick` owns the replan decision and is the
    // sole place the path is rebuilt.
    let mut updated = component.clone();
    updated.destination = Some(pos);
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

    // Admission pass: decide which agents may replan this tick, BEFORE any agent
    // moves. The per-tick budget is contended, so admit DRIFT-driven replans (the
    // target genuinely moved or was never planned) ahead of STALENESS-only ones
    // (a refresh of an essentially-unchanged plan whose cooldown merely elapsed).
    // Without this priority a staleness refresher earlier in slot order could
    // consume a budget slot on a no-op re-plan that a genuinely-drifted agent
    // needed this tick — crowding out the agent whose target actually moved.
    //
    // Reads each agent's LIVE component (set/clear destination may have mutated it
    // after the snapshot) with the cooldown decremented exactly as the apply loop
    // will, so the two passes agree on each agent's drift/staleness verdict.
    // Admitted ids are collected in slot order; total admissions stay ≤
    // REPLAN_BUDGET_PER_TICK.
    let admitted = admit_replans(registry, &snapshot);

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
            // No destination: idle steering, but still run the shared capsule
            // settle path so spawned/stationary agents obey gravity and
            // ground-stick before they ever acquire aggro.
            let capsule = AgentCapsule {
                radius: agent.radius,
                half_height: agent.half_height(),
                step_height: agent.step_height,
            };
            let result = collide_and_slide(
                collision_world,
                &capsule,
                position,
                Vec3::ZERO,
                agent.velocity.y,
                gravity,
                dt,
            );
            agent.velocity = result.velocity;
            agent.is_grounded = result.grounded;
            if let Ok(transform) = registry.get_component::<Transform>(current.id) {
                let mut t = *transform;
                t.position = result.position;
                let _ = registry.set_component(current.id, t);
            }
            let _ = registry.set_component(current.id, agent);
            continue;
        };

        // Whether this agent replans this tick was decided by the prioritized
        // admission pass above (drift-driven before staleness-only, capped at the
        // budget) — the ONLY place the path is (re)built. An agent that WANTED a
        // replan but lost the prioritized race is simply not in `admitted`.
        if admitted.contains(&current.id) {
            // Admitted: rebuild the path now.
            replans += 1;
            agent.replan_cooldown_ticks = REPLAN_STALENESS_TICKS;
            agent.planned_destination = Some(destination);
            match nav_graph.and_then(|graph| find_path(graph, position, destination)) {
                Some(path) => {
                    agent.path = path;
                    agent.waypoint_cursor = 0;
                    agent.arrived = false;
                    agent.blocked = false;
                }
                None => {
                    // No route: a genuine no-route stop (distinct from a budget
                    // loss). Drop the path and hold position; do NOT walk toward
                    // the raw destination. The cooldown (set above) keeps this
                    // from re-qualifying every tick.
                    agent.path.clear();
                    agent.waypoint_cursor = 0;
                    agent.blocked = true;
                }
            }
        }
        // An agent that WANTED a replan but lost the prioritized budget race this
        // tick keeps its existing `path` and `planned_destination` untouched (the
        // path is only ever mutated inside the admitted-replan block above), so it
        // follows its last route — stale-but-moving — instead of freezing. It
        // stays eligible: `planned_destination` is unchanged, so the drift test
        // (or the cooldown) still fires next tick until a slot frees up.

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

/// Decide which agents replan this tick under the per-tick budget, prioritizing
/// DRIFT-driven replans over STALENESS-only refreshes when the budget is
/// contended. Returns the admitted ids in slot order.
///
/// A wants-replan agent is DRIFT-driven when it has no plan yet
/// (`planned_destination` is `None`) OR its live destination has drifted more
/// than [`REPLAN_DEST_THRESHOLD`] (XZ) from the position the current plan was
/// built for — the target genuinely moved (or was never planned). It is
/// STALENESS-only when it qualifies ONLY because the cooldown elapsed
/// (`replan_cooldown_ticks == 0` after this tick's decrement) while drift ≤ the
/// threshold — a refresh of an essentially-unchanged plan.
///
/// Two passes over the snapshot: first admit drift-driven agents up to the
/// budget, then admit staleness-only agents with whatever budget remains. A
/// staleness refresher therefore never crowds out a genuinely-drifted agent; an
/// arrived agent whose destination then moved is drift-driven and re-acquires
/// promptly. Total admissions stay ≤ [`REPLAN_BUDGET_PER_TICK`]. Reads live
/// components only — no component writes happen here.
fn admit_replans(registry: &EntityRegistry, snapshot: &[AgentSnapshot]) -> Vec<EntityId> {
    // Classify each snapshot agent once: drift-driven, staleness-only, or not
    // wanting a replan at all. The cooldown is decremented exactly as the apply
    // loop will, so both passes see the same verdict.
    let mut drift_driven: Vec<EntityId> = Vec::new();
    let mut staleness_only: Vec<EntityId> = Vec::new();

    for current in snapshot {
        let Ok(agent) = registry.get_component::<AgentComponent>(current.id) else {
            continue;
        };
        let Some(destination) = agent.destination else {
            continue;
        };

        let drifted = agent
            .planned_destination
            .is_none_or(|planned| distance_xz(planned, destination) > REPLAN_DEST_THRESHOLD);
        // The apply loop decrements before the `== 0` test, so a cooldown of 1
        // (or 0) this tick reaches 0 after the decrement and counts as stale.
        let cooldown_elapsed = agent.replan_cooldown_ticks.saturating_sub(1) == 0;

        if drifted {
            drift_driven.push(current.id);
        } else if cooldown_elapsed {
            staleness_only.push(current.id);
        }
    }

    // First pass: drift-driven, up to the budget. Second pass: staleness-only,
    // with the remaining budget. Slot order is preserved within each pass.
    let budget = REPLAN_BUDGET_PER_TICK as usize;
    let mut admitted = drift_driven;
    admitted.truncate(budget);
    let remaining = budget - admitted.len();
    admitted.extend(staleness_only.into_iter().take(remaining));
    admitted
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
