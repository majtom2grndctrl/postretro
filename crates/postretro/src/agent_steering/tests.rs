// Steering-tick tests: path-following around geometry, arrived/blocked status,
// crowd separation, the replan budget, and the replan-starvation fairness gate.
//
// The L-wall fixture derives BOTH the collision trimesh AND the hand-built
// navmesh from ONE wall description (`LWall`), so the navmesh corridor and the
// solid geometry agree geometrically — a path that the navmesh says wraps the
// corner is the same corner the trimesh blocks.

use super::*;

use parry3d::math::{Isometry, Point};
use parry3d::shape::TriMesh;
use postretro_entities::Transform;
use postretro_level_format::navmesh::{NAVMESH_VERSION, NavMeshSection, NavPortal, NavRegion};

use crate::nav::NavAgentParams;

const EPS: f32 = 1e-3;
const DT: f32 = 1.0 / 60.0;
const GRAVITY: f32 = -20.0;

/// Canonical agent params for the fixtures: 0.35 m radius, 1.8 m tall, 0.4 m
/// step. Matches the harness's own test capsule.
fn agent_params() -> NavAgentParams {
    NavAgentParams {
        radius: 0.35,
        height: 1.8,
        step_height: 0.4,
        max_slope_deg: 45.0,
    }
}

/// Resting capsule-center height above the floor for the canonical agent: the
/// agent sweeps to one skin width above the floor contact. Used to place agents
/// grounded at spawn so gravity does not dominate the first ticks.
fn rest_y(params: &NavAgentParams) -> f32 {
    use crate::collision::SKIN_DISTANCE;
    let half_height = params.height / 2.0 - params.radius;
    half_height + params.radius + SKIN_DISTANCE
}

/// Spawn a grounded agent at world `(x, _, z)` with a destination already set.
/// Returns its id. The agent's capsule is seeded from `agent_params`.
fn spawn_agent(registry: &mut EntityRegistry, x: f32, z: f32, params: &NavAgentParams) -> EntityId {
    let transform = Transform {
        position: Vec3::new(x, rest_y(params), z),
        ..Transform::default()
    };
    let id = registry.spawn(transform);
    let agent = AgentComponent::from_nav_params(params, 4.0);
    registry.set_component(id, agent).unwrap();
    id
}

/// One wall description: a solid axis-aligned box (the obstacle) sitting on the
/// floor, plus the floor's own square extent. Both the collision trimesh and the
/// hand-built navmesh corridor are derived from this so they agree.
///
/// Floor: XZ square `[0, extent] x [0, extent]` at y=0.
/// Obstacle: the box `[bx0, bx1] x [bz0, bz1]`, full height — the agent must
/// route AROUND it. The navmesh covers the floor MINUS the box footprint as an
/// L-shaped corridor.
struct LWall {
    extent: f32,
    /// Obstacle box footprint on XZ (min/max), a corner of the floor square.
    bx0: f32,
    bx1: f32,
    bz0: f32,
    bz1: f32,
    height: f32,
}

impl LWall {
    /// The fixture used by the path-around-wall test. The obstacle occupies the
    /// +X/-Z corner (`x in [4,8], z in [0,4]`) of an 8x8 floor, leaving an
    /// L-shaped walkable region. Cell-aligned to unit cells so the navmesh
    /// region rects (cell space) match the world box exactly.
    fn fixture() -> Self {
        LWall {
            extent: 8.0,
            bx0: 4.0,
            bx1: 8.0,
            bz0: 0.0,
            bz1: 4.0,
            height: 3.0,
        }
    }

    /// Collision world: the floor quad plus the four vertical side faces of the
    /// obstacle box (each two-sided so an agent on either side is blocked).
    fn collision_world(&self) -> CollisionWorld {
        let mut points: Vec<Point<f32>> = Vec::new();
        let mut tris: Vec<[u32; 3]> = Vec::new();

        // Floor quad.
        let base = points.len() as u32;
        points.push(Point::new(0.0, 0.0, 0.0));
        points.push(Point::new(self.extent, 0.0, 0.0));
        points.push(Point::new(self.extent, 0.0, self.extent));
        points.push(Point::new(0.0, 0.0, self.extent));
        tris.push([base, base + 1, base + 2]);
        tris.push([base, base + 2, base + 3]);

        // Helper: push a two-sided vertical quad between two XZ corners, from
        // y=0 to y=height.
        let mut push_wall = |x0: f32, z0: f32, x1: f32, z1: f32| {
            let base = points.len() as u32;
            points.push(Point::new(x0, 0.0, z0));
            points.push(Point::new(x1, 0.0, z1));
            points.push(Point::new(x1, self.height, z1));
            points.push(Point::new(x0, self.height, z0));
            // Front + back winding so the agent is blocked from either side.
            tris.push([base, base + 1, base + 2]);
            tris.push([base, base + 2, base + 3]);
            tris.push([base, base + 2, base + 1]);
            tris.push([base, base + 3, base + 2]);
        };

        // The two obstacle faces that bound the walkable L (the -X face at
        // x=bx0 and the +Z face at z=bz1). The other two faces back onto the
        // floor edge and are never approached.
        push_wall(self.bx0, self.bz0, self.bx0, self.bz1); // -X face (x = bx0)
        push_wall(self.bx0, self.bz1, self.bx1, self.bz1); // +Z face (z = bz1)

        let mesh = TriMesh::new(points, tris);
        CollisionWorld {
            mesh,
            isometry: Isometry::identity(),
        }
    }

    /// Hand-built navmesh covering the floor MINUS the obstacle footprint as an
    /// L-corridor: region 0 (low strip, full width, z in [0, bz1]) minus the box
    /// is the `x in [0, bx0]` strip; region 1 is the full-width top strip
    /// (`z in [bz1, extent]`). Portals join them along z = bz1.
    ///
    /// Concretely, for the fixture (box at x[4,8] z[0,4], extent 8):
    ///   region 0: x[0,4] z[0,4]   (left of the box)
    ///   region 1: x[0,8] z[4,8]   (above the box)
    /// joined by a portal along z=4, x in [0,4]. A start in region 0 and a goal
    /// in region 1's +X half must route up-then-right around the box's corner.
    fn navmesh(&self) -> NavMeshSection {
        // Unit cells, origin at world zero, so cell coords equal world coords.
        let bx0 = self.bx0 as u32;
        let bz1 = self.bz1 as u32;
        let extent = self.extent as u32;

        NavMeshSection {
            version: NAVMESH_VERSION,
            origin: [0.0, 0.0, 0.0],
            cell_size: 1.0,
            dim_x: 64,
            dim_z: 64,
            agent_radius: 0.35,
            agent_height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
            regions: vec![
                // Region 0: left strip, x[0,bx0] z[0,bz1].
                NavRegion {
                    x0: 0,
                    z0: 0,
                    x1: bx0,
                    z1: bz1,
                    floor_y_min: 0.0,
                    floor_y_max: 0.25,
                },
                // Region 1: top strip, full width, z[bz1,extent].
                NavRegion {
                    x0: 0,
                    z0: bz1,
                    x1: extent,
                    z1: extent,
                    floor_y_min: 0.0,
                    floor_y_max: 0.25,
                },
            ],
            // Portal along z=bz1, spanning x in [0,bx0] (the shared edge).
            portals: vec![NavPortal {
                region_a: 0,
                region_b: 1,
                left: [0.0, 0.0, self.bz1],
                right: [self.bx0, 0.0, self.bz1],
            }],
        }
    }

    fn nav_graph(&self) -> NavGraph {
        NavGraph::from_section(&self.navmesh())
    }
}

#[test]
fn agent_steers_around_l_wall_without_penetrating_it() {
    // Start in the left strip (region 0), goal in the top strip's +X half
    // (region 1) — reachable only by routing up around the obstacle's corner. A
    // straight line would cut through the box at x in [4,8], z in [0,4].
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let graph = wall.nav_graph();
    let params = agent_params();

    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 1.0, 1.0, &params);
    // Goal: +X side of the top strip, just past the box corner.
    set_destination(&mut registry, id, Vec3::new(6.0, rest_y(&params), 6.0));

    // The box's -X face is at x=4 for z in [0,4]; the agent's capsule surface
    // (center x + radius) must never cross it while z is still within the box
    // band. That is the "does not penetrate the wall" invariant.
    for _ in 0..600 {
        tick(&mut registry, &world, Some(&graph), GRAVITY, DT);
        let state = path_state(&registry, id).unwrap();
        let p = state.position;
        // While the agent is still beside the box (z < bz1), it must stay left
        // of the box's -X face.
        if p.z < wall.bz1 - EPS {
            assert!(
                p.x + params.radius <= wall.bx0 + 0.05,
                "agent penetrated the box's -X face: center x={}, z={}",
                p.x,
                p.z
            );
        }
        if state.arrived {
            break;
        }
    }

    let state = path_state(&registry, id).unwrap();
    assert!(
        state.arrived,
        "agent should reach the goal around the L-wall, ended at {:?}",
        state.position
    );
    // And it ended near the goal in XZ.
    assert!(
        distance_xz(state.position, Vec3::new(6.0, 0.0, 6.0)) < 1.0,
        "agent should end near the goal, at {:?}",
        state.position
    );
}

#[test]
fn agent_reaching_destination_reports_arrived() {
    // Single open region: a flat floor, a navmesh covering it, a destination in
    // the same region. The agent walks straight to it and reports arrived.
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let graph = wall.nav_graph();
    let params = agent_params();

    let mut registry = EntityRegistry::new();
    // Both points in region 1 (top strip), clear of the box.
    let id = spawn_agent(&mut registry, 1.0, 6.0, &params);
    set_destination(&mut registry, id, Vec3::new(5.0, rest_y(&params), 6.0));

    let mut arrived = false;
    for _ in 0..600 {
        tick(&mut registry, &world, Some(&graph), GRAVITY, DT);
        if path_state(&registry, id).unwrap().arrived {
            arrived = true;
            break;
        }
    }
    assert!(arrived, "agent in an open region should report arrived");
}

#[test]
fn idle_agent_without_destination_still_settles_to_ground() {
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let params = agent_params();

    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 1.0, 6.0, &params);
    {
        let mut transform = *registry.get_component::<Transform>(id).unwrap();
        transform.position.y = rest_y(&params) + 0.25;
        registry.set_component(id, transform).unwrap();
    }

    for _ in 0..60 {
        tick(&mut registry, &world, None, GRAVITY, DT);
    }

    let state = path_state(&registry, id).unwrap();
    assert!(
        (state.position.y - rest_y(&params)).abs() <= EPS,
        "idle agent should settle to capsule-center rest height, got {:?}",
        state.position
    );
    assert!(
        registry
            .get_component::<AgentComponent>(id)
            .unwrap()
            .is_grounded,
        "idle agent should be grounded after settling"
    );
}

#[test]
fn agent_with_no_path_reports_blocked_and_holds_position() {
    // Destination outside every navmesh region: pathfinding returns None, so the
    // agent reports blocked and does NOT walk toward the raw destination (it
    // would otherwise march into the box).
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let graph = wall.nav_graph();
    let params = agent_params();

    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 1.0, 1.0, &params);
    let start_xz = Vec3::new(1.0, 0.0, 1.0);
    // A point off the navmesh entirely (z=20 is past the floor) — unreachable.
    set_destination(&mut registry, id, Vec3::new(1.0, rest_y(&params), 20.0));

    for _ in 0..120 {
        tick(&mut registry, &world, Some(&graph), GRAVITY, DT);
    }

    let state = path_state(&registry, id).unwrap();
    assert!(
        state.blocked,
        "unreachable destination should report blocked"
    );
    assert!(!state.has_path, "a blocked agent should hold no path");
    // It stayed put in XZ (gravity may settle it in Y, but it did not steer).
    assert!(
        distance_xz(state.position, start_xz) < 0.1,
        "blocked agent should not walk, moved to {:?}",
        state.position
    );
}

#[test]
fn two_agents_to_same_destination_separate_from_overlap() {
    // Two agents spawned overlapping (centers closer than a capsule diameter),
    // both pathing to the same point. Over several ticks the separation term
    // must push them from overlapping to non-overlapping.
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let graph = wall.nav_graph();
    let params = agent_params();

    let mut registry = EntityRegistry::new();
    // Spawn the two within a capsule radius of each other (definitely overlapping).
    let a = spawn_agent(&mut registry, 4.0, 6.0, &params);
    let b = spawn_agent(&mut registry, 4.2, 6.0, &params);
    let dest = Vec3::new(4.0, rest_y(&params), 6.0);
    set_destination(&mut registry, a, dest);
    set_destination(&mut registry, b, dest);

    let start_gap = {
        let pa = path_state(&registry, a).unwrap().position;
        let pb = path_state(&registry, b).unwrap().position;
        distance_xz(pa, pb)
    };
    assert!(
        start_gap < 2.0 * params.radius,
        "agents should start overlapping, gap {start_gap}"
    );

    for _ in 0..300 {
        tick(&mut registry, &world, Some(&graph), GRAVITY, DT);
    }

    let pa = path_state(&registry, a).unwrap().position;
    let pb = path_state(&registry, b).unwrap().position;
    let end_gap = distance_xz(pa, pb);
    assert!(
        end_gap >= 2.0 * params.radius,
        "agents should separate to non-overlapping, end gap {end_gap} (radii sum {})",
        2.0 * params.radius
    );
}

#[test]
fn steering_api_exposes_set_clear_destination_path_state_and_find_path() {
    // The steering API surface: set/clear destination mutate the component;
    // path_state reads it back; the re-exported find_path runs the one-shot
    // query. One test exercises all four entry points.
    let wall = LWall::fixture();
    let graph = wall.nav_graph();
    let params = agent_params();

    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 1.0, 1.0, &params);

    // No destination yet.
    let s0 = path_state(&registry, id).unwrap();
    assert!(!s0.has_destination);

    // set_destination records it.
    let dest = Vec3::new(5.0, rest_y(&params), 6.0);
    set_destination(&mut registry, id, dest);
    let s1 = path_state(&registry, id).unwrap();
    assert!(s1.has_destination);
    assert!(s1.distance_to_destination > 0.0);

    // clear_destination removes it.
    clear_destination(&mut registry, id);
    assert!(!path_state(&registry, id).unwrap().has_destination);

    // The re-exported one-shot find_path resolves a same-region path.
    let path = find_path(&graph, Vec3::new(1.0, 0.0, 1.0), Vec3::new(3.0, 0.0, 3.0))
        .expect("same-region path exists");
    assert_eq!(path.len(), 2);
}

#[test]
fn replans_are_bounded_by_budget_per_tick() {
    // More agents wanting a fresh plan than the budget → only up to the budget
    // recompute in a single tick. Spawn budget+extra agents, all with a fresh
    // (never-planned) destination, and assert the first tick's replan count is
    // exactly the budget.
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let graph = wall.nav_graph();
    let params = agent_params();

    let mut registry = EntityRegistry::new();
    let count = REPLAN_BUDGET_PER_TICK + 3;
    for i in 0..count {
        // Spread them across the top strip so they all sit in a region.
        let id = spawn_agent(&mut registry, 1.0 + i as f32 * 0.5, 6.0, &params);
        set_destination(&mut registry, id, Vec3::new(7.0, rest_y(&params), 6.0));
    }

    let result = tick(&mut registry, &world, Some(&graph), GRAVITY, DT);
    assert_eq!(
        result.replans, REPLAN_BUDGET_PER_TICK,
        "first tick should replan exactly the budget, not all {count} agents"
    );
}

#[test]
fn blocked_agents_do_not_starve_reachable_agent_replan() {
    // Replan-starvation fairness gate (Fold-in Fix 1): more permanently-blocked
    // agents than the budget, plus one reachable agent. A failed (empty) plan
    // must NOT re-qualify every tick, so the reachable agent obtains its path
    // within a bounded number of ticks rather than being starved forever.
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let graph = wall.nav_graph();
    let params = agent_params();

    let mut registry = EntityRegistry::new();

    // More blocked agents than the budget, all targeting an off-navmesh point.
    let blocked_count = REPLAN_BUDGET_PER_TICK + 4;
    for i in 0..blocked_count {
        let id = spawn_agent(&mut registry, 1.0 + i as f32 * 0.3, 6.0, &params);
        set_destination(&mut registry, id, Vec3::new(1.0, rest_y(&params), 20.0));
    }

    // One reachable agent: a destination inside the navmesh.
    let reachable = spawn_agent(&mut registry, 1.0, 5.0, &params);
    set_destination(
        &mut registry,
        reachable,
        Vec3::new(5.0, rest_y(&params), 6.0),
    );

    // Without the staleness gate, the blocked agents would re-spend the whole
    // budget every tick and the reachable agent would never plan. With the gate,
    // each blocked agent only re-qualifies once per staleness window, so the
    // reachable agent is served within a bounded number of ticks. The gate caps
    // blocked re-qualification at one window; the reachable agent's slot frees
    // up well before then.
    let bound = (blocked_count / REPLAN_BUDGET_PER_TICK + 2) as usize;
    let mut got_path = false;
    for _ in 0..bound {
        tick(&mut registry, &world, Some(&graph), GRAVITY, DT);
        if path_state(&registry, reachable).unwrap().has_path {
            got_path = true;
            break;
        }
    }
    assert!(
        got_path,
        "reachable agent should obtain a path within {bound} ticks, not be starved by blocked agents"
    );
}

/// Force an agent into a staleness-only-eligible state: it already holds a plan
/// to (essentially) its current destination, and its cooldown is one tick from
/// elapsing — so the steering tick's decrement reaches 0 and it qualifies ONLY
/// via staleness, never drift. The path is non-empty so it has something to keep
/// following if it loses the budget race.
fn make_staleness_only(registry: &mut EntityRegistry, id: EntityId, dest: Vec3) {
    let mut agent = registry
        .get_component::<AgentComponent>(id)
        .unwrap()
        .clone();
    agent.destination = Some(dest);
    agent.planned_destination = Some(dest); // drift == 0, ≤ threshold.
    agent.path = vec![dest];
    agent.waypoint_cursor = 0;
    agent.replan_cooldown_ticks = 1; // decrements to 0 this tick → stale.
    agent.arrived = false;
    agent.blocked = false;
    registry.set_component(id, agent).unwrap();
}

#[test]
fn drift_driven_replan_beats_staleness_refreshers_for_budget() {
    // Budget-contention priority: MORE than the budget of staleness-only-eligible
    // agents sit EARLIER in slot order than one drift-driven agent. First-come
    // allocation would spend the whole budget on the no-op refreshers and crowd
    // out the genuinely-moved agent. Drift priority must admit it THIS tick.
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let graph = wall.nav_graph();
    let params = agent_params();

    let mut registry = EntityRegistry::new();

    // Staleness-only agents FIRST in slot order, more than the whole budget, each
    // already planned to its own current spot (drift == 0, cooldown about to
    // elapse). All sit in region 1 (top strip) so a refresh would route fine.
    let staleness_count = REPLAN_BUDGET_PER_TICK + 1;
    for i in 0..staleness_count {
        let x = 1.0 + i as f32 * 0.5;
        let id = spawn_agent(&mut registry, x, 6.0, &params);
        make_staleness_only(&mut registry, id, Vec3::new(x, rest_y(&params), 6.0));
    }

    // One drift-driven agent LAST in slot order: it has a plan to an OLD spot, but
    // its live destination has since moved far past the drift threshold.
    let drifter = spawn_agent(&mut registry, 1.0, 5.0, &params);
    let old_dest = Vec3::new(1.0, rest_y(&params), 5.0);
    let new_dest = Vec3::new(6.0, rest_y(&params), 6.0); // > REPLAN_DEST_THRESHOLD away.
    {
        let mut agent = registry
            .get_component::<AgentComponent>(drifter)
            .unwrap()
            .clone();
        agent.destination = Some(new_dest);
        agent.planned_destination = Some(old_dest); // stale plan to the OLD spot.
        agent.path = vec![old_dest];
        agent.waypoint_cursor = 0;
        agent.replan_cooldown_ticks = REPLAN_STALENESS_TICKS; // NOT staleness-eligible.
        registry.set_component(drifter, agent).unwrap();
    }
    assert!(
        distance_xz(old_dest, new_dest) > REPLAN_DEST_THRESHOLD,
        "test setup: destination must have drifted past the threshold"
    );

    let result = tick(&mut registry, &world, Some(&graph), GRAVITY, DT);
    assert_eq!(
        result.replans, REPLAN_BUDGET_PER_TICK,
        "the budget is fully spent ({REPLAN_BUDGET_PER_TICK} replans)"
    );

    // The drift-driven agent replanned THIS tick: its plan now targets the moved
    // destination, not the old one it was crowded out toward.
    let drifter_agent = registry.get_component::<AgentComponent>(drifter).unwrap();
    let planned = drifter_agent
        .planned_destination
        .expect("drifter should have a plan after replanning");
    assert!(
        distance_xz(planned, new_dest) <= EPS,
        "drift-driven agent must replan to the moved destination this tick, planned {planned:?}"
    );
    assert!(
        distance_xz(planned, old_dest) > REPLAN_DEST_THRESHOLD,
        "drift-driven agent's plan must no longer target the old destination"
    );
}

#[test]
fn arrived_agent_reacquires_moved_destination_this_tick() {
    // An agent that has ARRIVED at D_old; its destination then moves to D_new
    // (past the drift threshold). With budget pressure from staleness-only agents
    // ahead of it in slot order, drift priority must still admit it THIS tick:
    // arrived cleared, plan now targets D_new — prompt re-acquisition, not a pause.
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let graph = wall.nav_graph();
    let params = agent_params();

    let mut registry = EntityRegistry::new();

    // Fill the budget with staleness-only refreshers ahead of the arrived agent.
    let staleness_count = REPLAN_BUDGET_PER_TICK + 1;
    for i in 0..staleness_count {
        let x = 1.0 + i as f32 * 0.5;
        let id = spawn_agent(&mut registry, x, 7.0, &params);
        make_staleness_only(&mut registry, id, Vec3::new(x, rest_y(&params), 7.0));
    }

    // The arrived agent, last in slot order. It reached D_old; D_new is far away.
    let arrived = spawn_agent(&mut registry, 2.0, 6.0, &params);
    let d_old = Vec3::new(2.0, rest_y(&params), 6.0);
    let d_new = Vec3::new(6.0, rest_y(&params), 6.0);
    {
        let mut agent = registry
            .get_component::<AgentComponent>(arrived)
            .unwrap()
            .clone();
        agent.destination = Some(d_new); // already moved past the threshold.
        agent.planned_destination = Some(d_old);
        agent.path = vec![d_old];
        agent.waypoint_cursor = 0;
        agent.arrived = true; // sitting at the old goal.
        agent.replan_cooldown_ticks = REPLAN_STALENESS_TICKS;
        registry.set_component(arrived, agent).unwrap();
    }
    assert!(
        distance_xz(d_old, d_new) > REPLAN_DEST_THRESHOLD,
        "test setup: destination must have moved past the threshold"
    );

    tick(&mut registry, &world, Some(&graph), GRAVITY, DT);

    let agent = registry.get_component::<AgentComponent>(arrived).unwrap();
    assert!(
        !agent.arrived,
        "arrived must clear once the agent replans toward the moved destination"
    );
    let planned = agent
        .planned_destination
        .expect("arrived agent should have replanned to the new destination");
    assert!(
        distance_xz(planned, d_new) <= EPS,
        "arrived agent must re-acquire the moved destination this tick, planned {planned:?}"
    );
    assert!(
        !agent.path.is_empty(),
        "arrived agent's path should target the new destination, not be empty"
    );
}

#[test]
fn set_and_clear_run_without_dev_tools_feature() {
    // DEFAULT-features proof (no dev-tools): the steering API entry points
    // compile and run in the default build. This module is only compiled in the
    // default feature set; the test invoking set_destination / path_state /
    // find_path here demonstrates they do not depend on `dev-tools`.
    let wall = LWall::fixture();
    let graph = wall.nav_graph();
    let params = agent_params();

    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 1.0, 6.0, &params);
    set_destination(&mut registry, id, Vec3::new(5.0, rest_y(&params), 6.0));
    assert!(path_state(&registry, id).unwrap().has_destination);

    let path = find_path(&graph, Vec3::new(1.0, 0.0, 6.0), Vec3::new(5.0, 0.0, 6.0));
    assert!(path.is_some(), "find_path runs without dev-tools");
}
