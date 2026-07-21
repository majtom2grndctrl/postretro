// Per-tick navigation-agent steering: refresh paths under a replan budget,
// steer along the current path with lookahead, separate from crowding neighbors,
// and move each agent through the world via the collide-and-slide harness.
//
// This is the primary production caller for `nav::find_path` and the agent
// component — `combat_positioning` also queries `find_path` when scoring
// candidate positions. It owns the replan policy (per-tick budget + per-agent
// staleness gate), the waypoint-following loop, and the O(n²) separation pass;
// the actual capsule sweep lives in `agent::collide_and_slide`.
//
// See: context/lib/build_pipeline.md §Navigation bake (pathfinding query surface)
//      context/lib/entity_model.md §7 (collision), §5 (fixed-tick game logic)
//      context/lib/movement.md §1 (custom-kinematic capsule, collision-only)

use glam::Vec3;

use crate::agent::{AgentCapsule, collide_and_slide};
use crate::collision::CollisionWorld;
use crate::nav::{NavGraph, distance_xz, find_path};
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry, Transform};
use postretro_entities::{DeferredEffectComponent, DeferredEffectKind};

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

/// Arrival-slowdown radius as a multiple of the agent capsule radius. This is
/// deliberately larger than the hard-stop radius (`ARRIVAL_RADIUS_FACTOR`) so
/// easing happens before `arrived` zeroes the final low-speed tail.
const ARRIVAL_SLOWDOWN_RADIUS_FACTOR: f32 = 7.0;

/// Tight collision-scale acceptance band for a mandatory clearance vertex: two
/// skins, one for the collision-tangent target and one for the fixed-tick landing
/// slop. Landing THIS band means the agent is essentially on the vertex, and on
/// that tick [`goal_speed`] zeroes `steer_velocity` to re-establish the outgoing
/// safe chord (heading restart) before rounding the corner.
///
/// It is NOT the only way to advance past a mandatory vertex, though: a chasing
/// agent on a live heading rarely lands a sub-skin band exactly, so requiring it
/// would let the agent creep toward an unreachable point forever (a permanent
/// silent stall). The cursor also advances once the agent has PASSED the vertex
/// plane toward the next waypoint while inside the ordinary arrival band — it has
/// rounded the corner — see [`mandatory_waypoint_cleared`]. The clearance intent
/// is preserved (the plane runs through the vertex, so the agent cannot cut back
/// inside the corner disk) without making progress hostage to sub-skin precision.
///
/// Intermediate mandatory vertices are traversed at full `move_speed` (no arrival
/// throttle — they are corners to round, not points to settle onto). But near the
/// vertex the goal-projected forward progress can still legitimately be small for
/// a tick or two: the heading-restart zeroing above resets the speed to re-accel
/// from zero, and a hard full-speed turn advances mostly sideways. So while an
/// agent is inside a mandatory vertex's arrival band the stuck detector measures
/// progress against a much smaller easing floor rather than accumulating against
/// the absolute floor (see [`update_stuck_ticks`]) — bounded suppression: a
/// genuine no-progress wedge at the vertex still escalates to tangent recovery.
const MANDATORY_WAYPOINT_ARRIVAL_RADIUS: f32 = 2.0 * crate::collision::SKIN_DISTANCE;

/// Goal-projected progress floor used while an agent is inside a mandatory
/// clearance vertex's arrival band. Rounding a mandatory vertex at full speed can
/// briefly show tiny forward progress — the heading-restart zeroing re-accelerates
/// from zero, and a hard turn advances mostly sideways — so measuring against the
/// absolute `STUCK_PROGRESS_EPSILON` floor would false-trip stuck recovery on a
/// legitimate corner turn. This far smaller floor still distinguishes a legitimate
/// turn — which always makes some positive progress — from a genuine WEDGE that
/// consumes all motion (progress ~= 0), so the suppression is bounded and a real
/// wedge escalates to recovery.
const MANDATORY_EASING_PROGRESS_EPSILON: f32 = STUCK_PROGRESS_EPSILON * 0.05;

/// Path-following acceleration/deceleration as "top-speed changes per second".
/// A value above 1 spans multiple fixed ticks while still letting agents brake
/// down through the short arrival band.
const STEERING_ACCEL_PER_SPEED: f32 = 8.0;

/// Maximum path-following heading rotation, in radians/sec.
pub(crate) const MAX_TURN_RATE: f32 = std::f32::consts::TAU;

/// Corridor lookahead as a multiple of the agent capsule radius. This exceeds
/// the waypoint-reached radius so agents can lead a corner, but stays below the
/// project fixtures' typical corridor segment length.
const LOOKAHEAD_DISTANCE_RADIUS_FACTOR: f32 = 3.0;

/// Shared zero-length XZ guard, matching the AI facing precedent.
const MIN_XZ_LEN_SQ: f32 = 1e-8;

/// Consecutive no-progress intent ticks before the steering system starts a
/// bounded tangent-slide recovery window.
const STUCK_TICKS_THRESHOLD: u32 = 20;

/// Number of ticks to keep the deterministic lateral recovery bias active.
const UNSTICK_WINDOW: u32 = 10;

/// Minimum requested path-following speed that counts as "the agent intends to
/// move." Speeds below this are arrival/idle tails, not stuck evidence.
const STUCK_INTENT_SPEED_EPSILON: f32 = 0.05;

/// Per-tick goal-projected displacement below this counts as no useful
/// progress. This is a distance, intentionally distinct from the intent-speed
/// gate above.
const STUCK_PROGRESS_EPSILON: f32 = 0.005;

/// Weight of the +90deg XZ tangent relative to the retained goal component
/// during recovery.
const TANGENT_BIAS: f32 = 1.0;

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
///
/// `blocked` implies `!has_path` by construction: a FAILED path refresh keeps
/// the previous route (stale-but-moving) rather than wiping it, so `blocked`
/// only latches when the agent holds no path at all — its destination is
/// genuinely unroutable from where it stands (endpoints are snap-resolved by
/// `find_path`, so the eroded wall margin never causes this). The blocked
/// state is transient by design: retries ride the replan cooldown plus the
/// drift/topology/direct-routable admission clauses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AgentPathState {
    /// The agent currently has a destination set (`Some`).
    pub(crate) has_destination: bool,
    /// The agent holds a non-empty path toward that destination.
    pub(crate) has_path: bool,
    /// The agent reached its destination (within the arrival radius).
    pub(crate) arrived: bool,
    /// The agent has a destination but pathfinding found no route to it AND it
    /// holds no path to keep following (see the struct doc: a failed refresh
    /// keeps the previous route, so `blocked` implies `!has_path`).
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
    updated.mandatory_waypoints.clear();
    updated.waypoint_cursor = 0;
    updated.steer_velocity = Vec3::ZERO;
    updated.stuck_ticks = 0;
    updated.unstick_window_remaining = 0;
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
/// under the replan budget + staleness gate, steer toward the path/lookahead target,
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
            if registry
                .get_component::<DeferredEffectComponent>(id)
                .is_ok_and(|effects| {
                    effects.inert
                        || effects
                            .pending
                            .iter()
                            .any(|effect| effect.kind == DeferredEffectKind::Despawn)
                })
            {
                return None;
            }
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
    let admitted = admit_replans(registry, &snapshot, nav_graph);

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
            agent.steer_velocity = Vec3::ZERO;
            agent.stuck_ticks = 0;
            agent.unstick_window_remaining = 0;
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
                    (agent.path, agent.mandatory_waypoints) = path.into_parts();
                    agent.waypoint_cursor = 0;
                    agent.arrived = false;
                    agent.blocked = false;
                }
                None => {
                    // No route from the current position. `find_path` already
                    // snaps eroded-band endpoints onto the graph, so this is a
                    // GENUINE no-route (a disconnected or far-off-mesh
                    // destination — or a map with no navmesh at all), never a
                    // mere near-wall wobble. Path failure must stay TRANSIENT:
                    // any existing path is KEPT — the agent follows its last
                    // good route (stale-but-moving) instead of freezing — and
                    // the cooldown (set above) gates the next retry, with the
                    // drift / topology / direct-routable admission clauses able
                    // to retry sooner. `blocked` reports the no-route state
                    // only when there is nothing left to follow; a pathless
                    // blocked agent holds position rather than marching into
                    // geometry toward the raw destination.
                    agent.blocked = agent.path.is_empty();
                }
            }
        }
        // An agent that WANTED a replan but lost the prioritized budget race this
        // tick keeps its existing `path` and `planned_destination` untouched (the
        // path is only ever mutated inside the admitted-replan block above), so it
        // follows its last route — stale-but-moving — instead of freezing. It
        // stays eligible: `planned_destination` is unchanged, so the drift test
        // (or the cooldown) still fires next tick until a slot frees up.

        // Compute the smoothed path-following velocity from the current path target.
        let arrival_radius = ARRIVAL_RADIUS_FACTOR * agent.radius;
        let slowdown_radius = ARRIVAL_SLOWDOWN_RADIUS_FACTOR * agent.radius;
        let goal_speed = goal_speed(&mut agent, position, arrival_radius, slowdown_radius);
        // Is the agent inside a mandatory clearance vertex's arrival band this
        // tick? It rounds the vertex at full `move_speed`, but the heading-restart
        // zeroing and hard-turn geometry can make the goal-projected step briefly
        // small there — legitimate cornering rather than a wedge — so the stuck
        // detector must not accumulate against it (see below).
        let easing_onto_mandatory =
            easing_onto_mandatory_waypoint(&agent, position, arrival_radius);
        let steer_velocity = integrated_steer_velocity(
            &agent,
            position,
            goal_speed,
            STEERING_ACCEL_PER_SPEED * agent.move_speed,
            MAX_TURN_RATE,
            LOOKAHEAD_DISTANCE_RADIUS_FACTOR * agent.radius,
            dt,
        );
        agent.steer_velocity = steer_velocity;
        let mut desired = steer_velocity;

        let has_recovery_intent = has_stuck_recovery_intent(&agent, goal_speed, steer_velocity);
        if has_recovery_intent {
            if agent.unstick_window_remaining == 0 && agent.stuck_ticks >= STUCK_TICKS_THRESHOLD {
                // Fire recovery: clear the plan latch so the next tick's
                // admission pass treats this agent as drift-driven (a forced,
                // budgeted replan from the wedged position), and open the
                // bounded tangent-bias window. A replan that FAILS during the
                // window keeps the existing path (see the replan block), so
                // the window keeps running on live steering intent — no
                // separate latch is needed to survive a failed forced replan.
                agent.planned_destination = None;
                agent.unstick_window_remaining = UNSTICK_WINDOW;
                agent.stuck_ticks = 0;
            }
        } else {
            // No live movement intent (idle tail, arrival, or a pathless
            // blocked agent): there is nothing for recovery to bias, so the
            // detector and any stale window reset. Blocked retries ride the
            // replan cooldown, not the recovery window.
            agent.stuck_ticks = 0;
            agent.unstick_window_remaining = 0;
        }
        let recovery_active_this_tick = agent.unstick_window_remaining > 0;
        // Safe to bias with `steer_velocity` unchecked: the sibling `else`
        // above zeroes the window the instant intent is lost, so a nonzero
        // window here means intent was live this tick, i.e. `steer_velocity`
        // is provably nonzero.
        if recovery_active_this_tick {
            desired += recovery_tangent_bias(steer_velocity, agent.move_speed);
            agent.unstick_window_remaining = agent.unstick_window_remaining.saturating_sub(1);
        }

        // Separation: sum pushes from every other agent whose capsule overlaps
        // or sits within the separation band, against the frozen snapshot. When
        // path-following intent exists, keep separation lateral/forward relative
        // to that intent so crowding cannot masquerade as a retreat.
        desired += separation_preserving_goal_progress(
            steer_velocity,
            separation(current, &agent, &snapshot),
        );

        // Clamp horizontal speed to the agent's top speed so the combined
        // (goal + separation) vector never drives faster than `move_speed`.
        desired = clamp_xz_speed(desired, agent.move_speed);

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
        update_stuck_ticks(
            &mut agent,
            position,
            result.position,
            steer_velocity,
            goal_speed,
            recovery_active_this_tick,
            easing_onto_mandatory,
        );

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
/// built for OR, when a nav graph is available, the live destination resolves to
/// different nav topology than the planned destination OR a previously-blocked
/// empty plan is now directly routable from the agent's current region to the
/// live destination region. It is STALENESS-only when it qualifies ONLY because
/// the cooldown elapsed (`replan_cooldown_ticks == 0` after this tick's
/// decrement) while drift/topology/direct-routable recovery stayed unchanged —
/// a refresh of an essentially-unchanged plan.
///
/// Two passes over the snapshot: first admit drift-driven agents up to the
/// budget, then admit staleness-only agents with whatever budget remains. A
/// staleness refresher therefore never crowds out a genuinely-drifted agent; an
/// arrived agent whose destination then moved is drift-driven and re-acquires
/// promptly. Total admissions stay ≤ [`REPLAN_BUDGET_PER_TICK`]. Reads live
/// components only — no component writes happen here.
fn admit_replans(
    registry: &EntityRegistry,
    snapshot: &[AgentSnapshot],
    nav_graph: Option<&NavGraph>,
) -> Vec<EntityId> {
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

        let drifted = agent.planned_destination.is_none_or(|planned| {
            distance_xz(planned, destination) > REPLAN_DEST_THRESHOLD
                || destination_topology_changed(nav_graph, planned, destination)
        }) || blocked_destination_now_directly_routable(
            nav_graph,
            agent,
            current.position,
            destination,
        );
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

fn destination_topology_changed(
    nav_graph: Option<&NavGraph>,
    planned: Vec3,
    destination: Vec3,
) -> bool {
    let Some(graph) = nav_graph else {
        return false;
    };

    // Resolve through the snapping resolver — the same definition `find_path`
    // routes with — so a target skirting in and out of the eroded wall margin
    // does not flap between "routable" and "off-mesh" and force a replan storm.
    match (
        graph.resolve_region_at(planned),
        graph.resolve_region_at(destination),
    ) {
        (Some(planned_region), Some(destination_region)) => planned_region != destination_region,
        (Some(_), None) | (None, Some(_)) => true,
        (None, None) => false,
    }
}

fn blocked_destination_now_directly_routable(
    nav_graph: Option<&NavGraph>,
    agent: &AgentComponent,
    position: Vec3,
    destination: Vec3,
) -> bool {
    // `!agent.path.is_empty()` is now unreachable under the `blocked ⇒
    // !has_path` invariant when `agent.blocked` holds; kept as defense-in-depth.
    if !agent.blocked || !agent.path.is_empty() {
        return false;
    }

    let Some(graph) = nav_graph else {
        return false;
    };

    match (
        graph.resolve_region_at(position),
        graph.resolve_region_at(destination),
    ) {
        (Some(position_region), Some(destination_region)) => position_region == destination_region,
        _ => false,
    }
}

/// Goal path-following speed for the current waypoint, advancing the cursor as
/// the agent reaches each waypoint within `arrival_radius` (XZ). Sets `arrived`
/// when the final waypoint is reached. Returns a scalar speed only; heading is
/// integrated separately so turn-rate limiting acts on persisted state.
fn goal_speed(
    agent: &mut AgentComponent,
    position: Vec3,
    arrival_radius: f32,
    slowdown_radius: f32,
) -> f32 {
    if agent.path.is_empty() {
        return 0.0;
    }

    // Advance the cursor past every waypoint already reached, so an agent that
    // overshoots several close waypoints in one tick does not backtrack. Stops at
    // the last waypoint. A plain waypoint counts as reached inside the ordinary
    // arrival radius; a mandatory clearance vertex counts as reached once the
    // agent has effectively CLEARED it — see [`mandatory_waypoint_cleared`] — so a
    // live chase heading is not held hostage to landing the sub-skin band exactly.
    while agent.waypoint_cursor + 1 < agent.path.len() {
        let target = agent.path[agent.waypoint_cursor];
        let mandatory = agent
            .mandatory_waypoints
            .get(agent.waypoint_cursor)
            .copied()
            .unwrap_or(false);
        let reached = if mandatory {
            mandatory_waypoint_cleared(
                position,
                target,
                agent.path[agent.waypoint_cursor + 1],
                arrival_radius,
            )
        } else {
            distance_xz(position, target) <= arrival_radius
        };
        if reached {
            if mandatory && distance_xz(position, target) <= MANDATORY_WAYPOINT_ARRIVAL_RADIUS {
                // Advancing AT the vertex (tight band): a mandatory waypoint is a
                // hard clearance vertex, not just a position target. Carrying the
                // incoming smoothed heading into its next leg rounds the corner
                // back through the endpoint clearance disk. Restart steering so the
                // outgoing safe chord establishes its own heading on this tick.
                //
                // Advancing via the plane-pass clause instead means the agent is
                // already PAST the vertex moving toward the next waypoint, so its
                // heading is already outgoing — keep it. Zeroing there would also
                // erase the momentum the tangent-recovery bias rides on when a
                // recovery replan lands a fresh funnel path mid-window.
                agent.steer_velocity = Vec3::ZERO;
            }
            agent.waypoint_cursor += 1;
        } else {
            break;
        }
    }

    let target = agent.path[agent.waypoint_cursor.min(agent.path.len() - 1)];
    let is_final = agent.waypoint_cursor + 1 >= agent.path.len();
    let final_distance = distance_xz(position, target);
    // Intermediate mandatory clearance vertices are traversed at full
    // `move_speed`: they are corner-offset waypoints a chasing agent must round,
    // not points it must settle onto. The cursor advances via
    // [`mandatory_waypoint_cleared`] as soon as the agent is within the ordinary
    // arrival band AND has passed the vertex plane toward the next leg, so
    // landing the tight sub-skin band exactly is not required and there is
    // nothing to "ease onto" — an intermediate throttle only makes the agent
    // crawl the corner. Arrival deceleration below is reserved for the FINAL
    // destination. `mandatory && !is_final` therefore falls through to full
    // speed. (The stuck-suppression gate in `update_stuck_ticks` still treats
    // this window defensively: a fast turn can show legitimately small forward
    // progress while the heading swings, so suppression there stays correct.)
    if is_final {
        if final_distance <= arrival_radius {
            agent.arrived = true;
            return 0.0;
        }
        agent.arrived = false;
        if final_distance < slowdown_radius {
            return agent.move_speed * (final_distance / slowdown_radius).clamp(0.0, 1.0);
        }
    }
    agent.move_speed
}

/// Integrated pre-collision path-following velocity. Direction comes from the
/// persisted `steer_velocity` heading rotated toward a lookahead/current target;
/// magnitude steps toward `goal_speed` under the acceleration limit.
fn integrated_steer_velocity(
    agent: &AgentComponent,
    position: Vec3,
    goal_speed: f32,
    accel: f32,
    max_turn_rate: f32,
    lookahead_distance: f32,
    dt: f32,
) -> Vec3 {
    if agent.path.is_empty() || agent.arrived || goal_speed <= 0.0 {
        return Vec3::ZERO;
    }

    let Some(target_dir) = target_direction(agent, position, lookahead_distance) else {
        return Vec3::ZERO;
    };
    let heading = rotated_heading(agent.steer_velocity, target_dir, max_turn_rate * dt);
    let current_speed = xz_length(agent.steer_velocity);
    let speed = move_toward(current_speed, goal_speed, accel * dt);
    heading * speed
}

/// XZ direction toward the lookahead point when available, otherwise the
/// current waypoint. `lookahead_distance == 0` disables lookahead.
fn target_direction(
    agent: &AgentComponent,
    position: Vec3,
    lookahead_distance: f32,
) -> Option<Vec3> {
    let target = target_point(agent, position, lookahead_distance)?;
    let to_target = Vec3::new(target.x - position.x, 0.0, target.z - position.z);
    if to_target.length_squared() <= MIN_XZ_LEN_SQ {
        None
    } else {
        Some(to_target.normalize())
    }
}

fn target_point(agent: &AgentComponent, position: Vec3, lookahead_distance: f32) -> Option<Vec3> {
    if agent.path.is_empty() {
        return None;
    }

    let cursor = agent.waypoint_cursor.min(agent.path.len() - 1);
    let current = agent.path[cursor];
    if lookahead_distance <= 0.0 {
        return Some(current);
    }

    let mut remaining = lookahead_distance;
    let mut from = Vec3::new(position.x, 0.0, position.z);
    for (offset, waypoint) in agent.path[cursor..].iter().enumerate() {
        let to = Vec3::new(waypoint.x, 0.0, waypoint.z);
        let segment = to - from;
        let len = segment.length();
        if len <= 1e-6 {
            from = to;
            continue;
        }
        if remaining <= len {
            let point = from + segment * (remaining / len);
            return Some(Vec3::new(point.x, waypoint.y, point.z));
        }
        if agent
            .mandatory_waypoints
            .get(cursor + offset)
            .copied()
            .unwrap_or(false)
        {
            return Some(*waypoint);
        }
        remaining -= len;
        from = to;
    }

    // The requested lookahead lies beyond the remaining corridor; fall back to
    // the current waypoint instead of aiming off-path.
    Some(current)
}

fn rotated_heading(current_velocity: Vec3, target_dir: Vec3, max_delta: f32) -> Vec3 {
    let current_len_sq =
        current_velocity.x * current_velocity.x + current_velocity.z * current_velocity.z;
    if current_len_sq <= MIN_XZ_LEN_SQ {
        return Vec3::new(target_dir.x, 0.0, target_dir.z).normalize();
    }

    let current = Vec3::new(current_velocity.x, 0.0, current_velocity.z).normalize();
    let target = Vec3::new(target_dir.x, 0.0, target_dir.z).normalize();
    let dot = current.dot(target).clamp(-1.0, 1.0);
    let cross_y = current.x * target.z - current.z * target.x;
    let angle = cross_y.atan2(dot);
    let step = angle.clamp(-max_delta, max_delta);
    let (sin, cos) = step.sin_cos();
    Vec3::new(
        current.x * cos - current.z * sin,
        0.0,
        current.x * sin + current.z * cos,
    )
    .normalize()
}

fn move_toward(current: f32, target: f32, max_step: f32) -> f32 {
    let delta = target - current;
    if delta.abs() <= max_step {
        target
    } else {
        current + delta.signum() * max_step
    }
}

fn clamp_xz_speed(mut velocity: Vec3, max_speed: f32) -> Vec3 {
    let speed = xz_length(velocity);
    if speed > max_speed && speed > 1e-6 {
        let scale = max_speed / speed;
        velocity.x *= scale;
        velocity.z *= scale;
    }
    velocity
}

fn recovery_tangent_bias(steer_velocity: Vec3, move_speed: f32) -> Vec3 {
    let tangent = Vec3::new(-steer_velocity.z, 0.0, steer_velocity.x);
    if tangent.length_squared() <= MIN_XZ_LEN_SQ {
        Vec3::ZERO
    } else {
        tangent.normalize() * (TANGENT_BIAS * move_speed)
    }
}

/// True when the agent has effectively cleared a mandatory, non-final clearance
/// vertex and the cursor may advance past it. Either:
///   - it is within the tight collision-scale band (`MANDATORY_WAYPOINT_ARRIVAL_RADIUS`),
///     essentially on the vertex; OR
///   - it is within the ordinary arrival band of the vertex AND has passed the
///     vertex plane toward the next waypoint (`(position - vertex) · (next - vertex) >= 0`).
///
/// The second clause lets a chasing agent round the corner smoothly without
/// landing the sub-skin band exactly, while the plane running through the vertex
/// keeps it from cutting back inside the corner clearance disk. A degenerate
/// outgoing leg (next == vertex in XZ) falls back to the tight band alone.
fn mandatory_waypoint_cleared(
    position: Vec3,
    vertex: Vec3,
    next: Vec3,
    arrival_radius: f32,
) -> bool {
    let to_vertex = distance_xz(position, vertex);
    if to_vertex <= MANDATORY_WAYPOINT_ARRIVAL_RADIUS {
        return true;
    }
    if to_vertex > arrival_radius {
        return false;
    }
    let outgoing = Vec3::new(next.x - vertex.x, 0.0, next.z - vertex.z);
    if outgoing.length_squared() <= MIN_XZ_LEN_SQ {
        return false;
    }
    let past = Vec3::new(position.x - vertex.x, 0.0, position.z - vertex.z);
    past.dot(outgoing) >= 0.0
}

/// True when the agent's current path target is a mandatory clearance vertex it
/// is inside the arrival band of: mandatory, non-final, and within the ordinary
/// arrival band (`arrival_radius`). Intermediate mandatory vertices are traversed
/// at full `move_speed`, but this window still sees legitimately-small forward
/// progress on a tick or two — the tight-band heading restart in [`goal_speed`]
/// zeroes and re-accelerates the speed, and a hard full-speed corner turn
/// advances mostly sideways — so [`update_stuck_ticks`] uses it to select the
/// smaller easing progress floor there instead of the absolute floor.
fn easing_onto_mandatory_waypoint(
    agent: &AgentComponent,
    position: Vec3,
    arrival_radius: f32,
) -> bool {
    if agent.path.is_empty() {
        return false;
    }
    let cursor = agent.waypoint_cursor.min(agent.path.len() - 1);
    let is_final = agent.waypoint_cursor + 1 >= agent.path.len();
    let mandatory = agent
        .mandatory_waypoints
        .get(cursor)
        .copied()
        .unwrap_or(false);
    mandatory && !is_final && distance_xz(position, agent.path[cursor]) < arrival_radius
}

fn update_stuck_ticks(
    agent: &mut AgentComponent,
    start_position: Vec3,
    resolved_position: Vec3,
    steer_velocity: Vec3,
    goal_speed: f32,
    recovery_active_this_tick: bool,
    easing_onto_mandatory: bool,
) {
    if recovery_active_this_tick {
        return;
    }
    if !has_stuck_recovery_intent(agent, goal_speed, steer_velocity) {
        agent.stuck_ticks = 0;
        return;
    }

    let goal_dir = Vec3::new(steer_velocity.x, 0.0, steer_velocity.z).normalize();
    let displacement = Vec3::new(
        resolved_position.x - start_position.x,
        0.0,
        resolved_position.z - start_position.z,
    );
    let progress = displacement.dot(goal_dir);
    // Inside a mandatory clearance vertex's arrival band the agent runs at full
    // move_speed, but goal-projected forward progress can briefly be tiny yet
    // positive — the tight-band heading restart re-accelerates from zero and a
    // hard corner turn advances mostly sideways. Measure it against the much
    // smaller easing floor instead of the absolute floor: bounded suppression that
    // never trips on a real corner turn yet still accumulates — and eventually
    // escalates to tangent recovery — against a genuine no-progress wedge.
    let floor = if easing_onto_mandatory {
        MANDATORY_EASING_PROGRESS_EPSILON
    } else {
        STUCK_PROGRESS_EPSILON
    };
    if progress < floor {
        agent.stuck_ticks = agent.stuck_ticks.saturating_add(1);
    } else {
        agent.stuck_ticks = 0;
    }
}

fn has_stuck_recovery_intent(
    agent: &AgentComponent,
    goal_speed: f32,
    steer_velocity: Vec3,
) -> bool {
    !agent.path.is_empty()
        && !agent.blocked
        && goal_speed > STUCK_INTENT_SPEED_EPSILON
        && steer_velocity.length_squared() > MIN_XZ_LEN_SQ
}

fn xz_length(velocity: Vec3) -> f32 {
    (velocity.x * velocity.x + velocity.z * velocity.z).sqrt()
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

fn separation_preserving_goal_progress(steer_velocity: Vec3, separation: Vec3) -> Vec3 {
    if steer_velocity.length_squared() <= MIN_XZ_LEN_SQ
        || separation.length_squared() <= MIN_XZ_LEN_SQ
    {
        return separation;
    }

    let goal_dir = Vec3::new(steer_velocity.x, 0.0, steer_velocity.z).normalize();
    let backward = separation.dot(goal_dir);
    if backward < 0.0 {
        separation - goal_dir * backward
    } else {
        separation
    }
}

#[cfg(test)]
mod tests;
