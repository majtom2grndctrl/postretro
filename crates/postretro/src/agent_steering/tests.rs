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
// The segmented concave fixture routes around finite wall ends via portals.
// That route still produces a visible recovery displacement, but less direct
// goal projection than the old all-floor shortcut.
const SEGMENTED_CONCAVE_ESCAPE_DISTANCE: f32 = 0.025;

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

fn set_manual_path(registry: &mut EntityRegistry, id: EntityId, path: Vec<Vec3>) {
    let destination = *path.last().expect("manual path must have a destination");
    let mut agent = registry
        .get_component::<AgentComponent>(id)
        .unwrap()
        .clone();
    agent.destination = Some(destination);
    agent.planned_destination = Some(destination);
    agent.path = path;
    agent.waypoint_cursor = 0;
    agent.replan_cooldown_ticks = u32::MAX;
    agent.arrived = false;
    agent.blocked = false;
    registry.set_component(id, agent).unwrap();
}

fn steer_speed(registry: &EntityRegistry, id: EntityId) -> f32 {
    xz_length(
        registry
            .get_component::<AgentComponent>(id)
            .unwrap()
            .steer_velocity,
    )
}

fn angle_between_xz(a: Vec3, b: Vec3) -> f32 {
    let a = Vec3::new(a.x, 0.0, a.z).normalize();
    let b = Vec3::new(b.x, 0.0, b.z).normalize();
    a.dot(b).clamp(-1.0, 1.0).acos()
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

/// Concave-corner recovery fixture: two vertical wall segments meet at an
/// interior corner. The agent approaches from southwest toward northeast, so
/// collision can consume the goal-directed motion while the fixed +90deg
/// recovery tangent rotates it toward the open north-side escape lane.
///
/// The same finite wall description drives collision and navmesh: the navmesh
/// splits the floor into rectangles on either side of those two wall segments,
/// then adds portals only around the wall ends. That keeps the route valid
/// around the wedge without making the whole floor one permissive region.
struct ConcaveCorner {
    floor_min: f32,
    floor_max: f32,
    corner: f32,
    wall_end: f32,
    height: f32,
}

impl ConcaveCorner {
    fn fixture() -> Self {
        Self {
            floor_min: -2.0,
            floor_max: 8.0,
            corner: 2.0,
            wall_end: 7.0,
            height: 3.0,
        }
    }

    fn cell(&self, world: f32) -> u32 {
        (world - self.floor_min) as u32
    }

    fn collision_world(&self) -> CollisionWorld {
        let mut points: Vec<Point<f32>> = Vec::new();
        let mut tris: Vec<[u32; 3]> = Vec::new();

        let base = points.len() as u32;
        points.push(Point::new(self.floor_min, 0.0, self.floor_min));
        points.push(Point::new(self.floor_max, 0.0, self.floor_min));
        points.push(Point::new(self.floor_max, 0.0, self.floor_max));
        points.push(Point::new(self.floor_min, 0.0, self.floor_max));
        tris.push([base, base + 1, base + 2]);
        tris.push([base, base + 2, base + 3]);

        let mut push_wall = |x0: f32, z0: f32, x1: f32, z1: f32| {
            let base = points.len() as u32;
            points.push(Point::new(x0, 0.0, z0));
            points.push(Point::new(x1, 0.0, z1));
            points.push(Point::new(x1, self.height, z1));
            points.push(Point::new(x0, self.height, z0));
            tris.push([base, base + 1, base + 2]);
            tris.push([base, base + 2, base + 3]);
            tris.push([base, base + 2, base + 1]);
            tris.push([base, base + 3, base + 2]);
        };

        // East-side wall: once beside the corner, +X motion is blocked.
        push_wall(self.corner, self.corner, self.corner, self.wall_end);
        // North-side wall starts at the same corner and extends east; it
        // supplies the second corner contact while leaving the west side open
        // for the fixed-handedness recovery slide.
        push_wall(self.corner, self.corner, self.wall_end, self.corner);

        CollisionWorld {
            mesh: TriMesh::new(points, tris),
            isometry: Isometry::identity(),
        }
    }

    fn navmesh(&self) -> NavMeshSection {
        let min = self.cell(self.floor_min);
        let corner = self.cell(self.corner);
        let wall_end = self.cell(self.wall_end);
        let max = self.cell(self.floor_max);

        let region = |x0, z0, x1, z1| NavRegion {
            x0,
            z0,
            x1,
            z1,
            floor_y_min: 0.0,
            floor_y_max: 0.25,
        };

        NavMeshSection {
            version: NAVMESH_VERSION,
            origin: [self.floor_min, 0.0, self.floor_min],
            cell_size: 1.0,
            dim_x: 64,
            dim_z: 64,
            agent_radius: 0.35,
            agent_height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
            regions: vec![
                // Region 0: southwest approach, before both wall segments.
                region(min, min, corner, corner),
                // Region 1: west lane beside the x=corner wall.
                region(min, corner, corner, wall_end),
                // Region 2: north lane past the x=corner wall end.
                region(min, wall_end, max, max),
                // Region 3: south lane beside the z=corner wall.
                region(corner, min, max, corner),
                // Region 4: interior target area behind the concave corner.
                region(corner, corner, wall_end, wall_end),
                // Region 5: east lane past the z=corner wall end.
                region(wall_end, corner, max, wall_end),
            ],
            portals: vec![
                // Open approach into the west lane. No portal crosses either
                // wall span from z=corner..wall_end or x=corner..wall_end.
                NavPortal {
                    region_a: 0,
                    region_b: 1,
                    left: [self.floor_min, 0.0, self.corner],
                    right: [self.corner, 0.0, self.corner],
                },
                // North-side route around the end of the x=corner wall.
                NavPortal {
                    region_a: 1,
                    region_b: 2,
                    left: [self.floor_min, 0.0, self.wall_end],
                    right: [self.corner, 0.0, self.wall_end],
                },
                NavPortal {
                    region_a: 2,
                    region_b: 4,
                    left: [self.wall_end, 0.0, self.wall_end],
                    right: [self.corner, 0.0, self.wall_end],
                },
                // Alternate east-side route around the end of the z=corner wall.
                NavPortal {
                    region_a: 0,
                    region_b: 3,
                    left: [self.corner, 0.0, self.corner],
                    right: [self.corner, 0.0, self.floor_min],
                },
                NavPortal {
                    region_a: 3,
                    region_b: 5,
                    left: [self.wall_end, 0.0, self.corner],
                    right: [self.floor_max, 0.0, self.corner],
                },
                NavPortal {
                    region_a: 5,
                    region_b: 4,
                    left: [self.wall_end, 0.0, self.corner],
                    right: [self.wall_end, 0.0, self.wall_end],
                },
            ],
        }
    }

    fn nav_graph(&self) -> NavGraph {
        NavGraph::from_section(&self.navmesh())
    }
}

fn agent_position(registry: &EntityRegistry, id: EntityId) -> Vec3 {
    registry.get_component::<Transform>(id).unwrap().position
}

fn goal_projected_xz_progress(start: Vec3, end: Vec3, heading: Vec3) -> f32 {
    let heading = Vec3::new(heading.x, 0.0, heading.z);
    if heading.length_squared() <= 0.0 {
        return 0.0;
    }
    let displacement = Vec3::new(end.x - start.x, 0.0, end.z - start.z);
    displacement.dot(heading.normalize())
}

fn run_until_stuck_threshold(
    registry: &mut EntityRegistry,
    id: EntityId,
    world: &CollisionWorld,
    max_ticks: usize,
) {
    for _ in 0..max_ticks {
        let before = agent_position(registry, id);
        tick(registry, world, None, GRAVITY, DT);
        let agent = registry.get_component::<AgentComponent>(id).unwrap();
        if agent.stuck_ticks >= STUCK_TICKS_THRESHOLD {
            let after = agent_position(registry, id);
            let progress = goal_projected_xz_progress(before, after, agent.steer_velocity);
            assert!(
                progress < STUCK_PROGRESS_EPSILON,
                "threshold tick must still show near-zero goal-projected progress; got {progress}"
            );
            assert_eq!(
                agent.unstick_window_remaining, 0,
                "detection should reach threshold before recovery fires"
            );
            return;
        }
    }

    let agent = registry.get_component::<AgentComponent>(id).unwrap();
    panic!(
        "agent did not reach stuck threshold within {max_ticks} ticks; stuck_ticks={}, pos={:?}, steer={:?}, velocity={:?}",
        agent.stuck_ticks,
        agent_position(registry, id),
        agent.steer_velocity,
        agent.velocity
    );
}

#[test]
fn agent_path_following_speed_accelerates_over_multiple_ticks() {
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let params = agent_params();
    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 1.0, 6.0, &params);
    let destination = Vec3::new(7.0, rest_y(&params), 6.0);
    set_manual_path(
        &mut registry,
        id,
        vec![Vec3::new(1.0, rest_y(&params), 6.0), destination],
    );

    let mut speeds = Vec::new();
    for _ in 0..12 {
        tick(&mut registry, &world, None, GRAVITY, DT);
        speeds.push(steer_speed(&registry, id));
        if speeds.last().copied().unwrap() >= 4.0 - EPS {
            break;
        }
    }

    assert!(
        speeds[0] > 0.0 && speeds[0] < 4.0,
        "first tick should ramp below move_speed, got {:?}",
        speeds
    );
    for pair in speeds.windows(2) {
        assert!(
            pair[1] + EPS >= pair[0],
            "steer_velocity speed should ramp monotonically, got {:?}",
            speeds
        );
    }
    assert!(
        speeds.last().copied().unwrap() >= 4.0 - EPS,
        "steer_velocity should converge to move_speed before arrival easing, got {:?}",
        speeds
    );
}

#[test]
fn steer_velocity_ramps_independently_of_wall_clamped_collision_velocity() {
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let params = agent_params();
    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 3.55, 2.0, &params);
    set_manual_path(
        &mut registry,
        id,
        vec![
            Vec3::new(3.55, rest_y(&params), 2.0),
            Vec3::new(6.0, rest_y(&params), 2.0),
        ],
    );

    let mut steer_speeds = Vec::new();
    let mut post_collision_speeds = Vec::new();
    for _ in 0..10 {
        tick(&mut registry, &world, None, GRAVITY, DT);
        let agent = registry.get_component::<AgentComponent>(id).unwrap();
        steer_speeds.push(xz_length(agent.steer_velocity));
        post_collision_speeds.push(xz_length(agent.velocity));
    }

    for pair in steer_speeds.windows(2) {
        assert!(
            pair[1] + EPS >= pair[0],
            "pre-collision steer_velocity should keep ramping despite wall contact: {:?}",
            steer_speeds
        );
    }
    assert!(
        post_collision_speeds
            .iter()
            .zip(steer_speeds.iter())
            .any(|(post, steer)| *steer > 1.0 && *post + 0.25 < *steer),
        "post-collision velocity should diverge from pre-collision steer_velocity; steer={:?}, post={:?}",
        steer_speeds,
        post_collision_speeds
    );
}

#[test]
fn stuck_detection_reaches_threshold_then_recovery_fires_next_tick() {
    let corner = ConcaveCorner::fixture();
    let world = corner.collision_world();
    let graph = corner.nav_graph();
    let params = agent_params();
    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 1.2, 1.2, &params);
    let destination = Vec3::new(5.0, rest_y(&params), 5.0);
    set_manual_path(&mut registry, id, vec![destination]);

    run_until_stuck_threshold(&mut registry, id, &world, 180);
    {
        let agent = registry.get_component::<AgentComponent>(id).unwrap();
        assert_eq!(agent.stuck_ticks, STUCK_TICKS_THRESHOLD);
        assert!(!agent.path.is_empty());
        assert!(!agent.blocked);
    }

    tick(&mut registry, &world, Some(&graph), GRAVITY, DT);

    let agent = registry.get_component::<AgentComponent>(id).unwrap();
    assert_eq!(agent.stuck_ticks, 0, "recovery fire resets detector");
    assert_eq!(
        agent.unstick_window_remaining,
        UNSTICK_WINDOW - 1,
        "recovery applies one biased move on the fire tick and stores the remaining window"
    );
    assert_eq!(
        agent.planned_destination, None,
        "recovery must request a budgeted replan by clearing the plan latch"
    );
}

#[test]
fn recovery_window_escapes_concave_corner_with_goal_projected_progress() {
    let corner = ConcaveCorner::fixture();
    let world = corner.collision_world();
    let graph = corner.nav_graph();
    let params = agent_params();
    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 1.2, 1.2, &params);
    let destination = Vec3::new(5.0, rest_y(&params), 5.0);
    set_manual_path(&mut registry, id, vec![destination]);

    run_until_stuck_threshold(&mut registry, id, &world, 180);
    let start = agent_position(&registry, id);
    let goal_dir = Vec3::new(destination.x - start.x, 0.0, destination.z - start.z).normalize();

    for _ in 0..UNSTICK_WINDOW {
        tick(&mut registry, &world, Some(&graph), GRAVITY, DT);
    }

    let end = agent_position(&registry, id);
    let displacement = Vec3::new(end.x - start.x, 0.0, end.z - start.z);
    let progress = displacement.dot(goal_dir);
    assert!(
        progress > SEGMENTED_CONCAVE_ESCAPE_DISTANCE,
        "recovery should escape the segmented concave route with goal-projected progress > {SEGMENTED_CONCAVE_ESCAPE_DISTANCE}, got {progress}; start={start:?}, end={end:?}"
    );
    assert!(
        xz_length(displacement) > SEGMENTED_CONCAVE_ESCAPE_DISTANCE,
        "recovery should produce visible displacement during the window; got displacement={displacement:?}"
    );
    let agent = registry.get_component::<AgentComponent>(id).unwrap();
    assert_eq!(
        agent.unstick_window_remaining, 0,
        "recovery window should expire after its fixed tick budget"
    );
}

#[test]
fn recovery_tangent_has_fixed_positive_quarter_turn_and_degenerate_noop() {
    let steer = Vec3::new(3.0, 0.0, 4.0);
    let bias = recovery_tangent_bias(steer, 2.0);
    let expected = Vec3::new(-4.0, 0.0, 3.0).normalize() * (TANGENT_BIAS * 2.0);
    assert!(
        bias.abs_diff_eq(expected, EPS),
        "tangent must be fixed +90deg in XZ: got {bias:?}, expected {expected:?}"
    );

    let degenerate = recovery_tangent_bias(Vec3::ZERO, 2.0);
    assert_eq!(degenerate, Vec3::ZERO);
    assert!(degenerate.is_finite());
}

#[test]
fn recovery_escape_path_is_deterministic_for_identical_inputs() {
    fn escape_path() -> Vec<Vec3> {
        let corner = ConcaveCorner::fixture();
        let world = corner.collision_world();
        let graph = corner.nav_graph();
        let params = agent_params();
        let mut registry = EntityRegistry::new();
        let id = spawn_agent(&mut registry, 1.2, 1.2, &params);
        let destination = Vec3::new(5.0, rest_y(&params), 5.0);
        set_manual_path(&mut registry, id, vec![destination]);
        run_until_stuck_threshold(&mut registry, id, &world, 180);

        let mut positions = Vec::new();
        for _ in 0..UNSTICK_WINDOW {
            tick(&mut registry, &world, Some(&graph), GRAVITY, DT);
            positions.push(agent_position(&registry, id));
        }
        positions
    }

    let a = escape_path();
    let b = escape_path();
    assert_eq!(a.len(), b.len());
    for (index, (pa, pb)) in a.iter().zip(b.iter()).enumerate() {
        assert!(
            pa.abs_diff_eq(*pb, EPS),
            "escape paths diverged at {index}: {pa:?} vs {pb:?}"
        );
    }
}

#[test]
fn stuck_detection_resets_for_idle_blocked_and_arrived_gates() {
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let params = agent_params();

    let mut idle_registry = EntityRegistry::new();
    let idle = spawn_agent(&mut idle_registry, 1.0, 6.0, &params);
    {
        let mut agent = idle_registry
            .get_component::<AgentComponent>(idle)
            .unwrap()
            .clone();
        agent.stuck_ticks = 7;
        agent.unstick_window_remaining = 3;
        idle_registry.set_component(idle, agent).unwrap();
    }
    tick(&mut idle_registry, &world, None, GRAVITY, DT);
    let agent = idle_registry.get_component::<AgentComponent>(idle).unwrap();
    assert_eq!(
        agent.stuck_ticks, 0,
        "idle/no-path gate should clear detection"
    );
    assert_eq!(
        agent.unstick_window_remaining, 0,
        "idle/no-path gate should clear stale recovery windows"
    );

    let mut blocked_registry = EntityRegistry::new();
    let blocked = spawn_agent(&mut blocked_registry, 1.0, 6.0, &params);
    set_manual_path(
        &mut blocked_registry,
        blocked,
        vec![Vec3::new(7.0, rest_y(&params), 6.0)],
    );
    {
        let mut agent = blocked_registry
            .get_component::<AgentComponent>(blocked)
            .unwrap()
            .clone();
        agent.blocked = true;
        agent.stuck_ticks = STUCK_TICKS_THRESHOLD;
        agent.unstick_window_remaining = 3;
        blocked_registry.set_component(blocked, agent).unwrap();
    }
    tick(&mut blocked_registry, &world, None, GRAVITY, DT);
    let agent = blocked_registry
        .get_component::<AgentComponent>(blocked)
        .unwrap();
    assert_eq!(
        agent.stuck_ticks, 0,
        "blocked/no-route gate should clear detection"
    );
    assert_eq!(
        agent.unstick_window_remaining, 0,
        "blocked/no-route gate should clear stale recovery windows"
    );

    let mut arrived_registry = EntityRegistry::new();
    let arrived = spawn_agent(&mut arrived_registry, 1.0, 6.0, &params);
    set_manual_path(
        &mut arrived_registry,
        arrived,
        vec![Vec3::new(1.0, rest_y(&params), 6.0)],
    );
    {
        let mut agent = arrived_registry
            .get_component::<AgentComponent>(arrived)
            .unwrap()
            .clone();
        agent.stuck_ticks = STUCK_TICKS_THRESHOLD;
        agent.unstick_window_remaining = 3;
        arrived_registry.set_component(arrived, agent).unwrap();
    }
    tick(&mut arrived_registry, &world, None, GRAVITY, DT);
    let agent = arrived_registry
        .get_component::<AgentComponent>(arrived)
        .unwrap();
    assert!(
        agent.arrived,
        "fixture should engage final-waypoint arrival"
    );
    assert_eq!(
        agent.stuck_ticks, 0,
        "near-zero final-waypoint intent should clear detection"
    );
    assert_eq!(
        agent.unstick_window_remaining, 0,
        "near-zero final-waypoint intent should clear stale recovery windows"
    );
}

#[test]
fn failed_same_tick_replan_suppresses_stale_recovery_threshold() {
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let params = agent_params();
    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 1.0, 6.0, &params);
    let destination = Vec3::new(7.0, rest_y(&params), 6.0);
    set_manual_path(&mut registry, id, vec![destination]);
    {
        let mut agent = registry
            .get_component::<AgentComponent>(id)
            .unwrap()
            .clone();
        agent.stuck_ticks = STUCK_TICKS_THRESHOLD;
        agent.planned_destination = None;
        registry.set_component(id, agent).unwrap();
    }

    tick(&mut registry, &world, None, GRAVITY, DT);

    let agent = registry.get_component::<AgentComponent>(id).unwrap();
    assert!(
        agent.blocked,
        "missing nav graph should produce a no-route state"
    );
    assert!(
        agent.path.is_empty(),
        "failed replan should drop the stale path"
    );
    assert_eq!(
        agent.stuck_ticks, 0,
        "failed same-tick replan should clear stale detection"
    );
    assert_eq!(
        agent.unstick_window_remaining, 0,
        "failed same-tick replan must not seed recovery"
    );
    assert_eq!(
        agent.planned_destination,
        Some(destination),
        "failed replan should keep the failed-plan latch for the cooldown gate"
    );
}

#[test]
fn failed_forced_recovery_replan_holds_without_stale_steer_or_tangent() {
    // Regression: a recovery-forced replan can fail from a wedged/off-navmesh
    // position; once the path is cleared, stale steer_velocity and the fixed
    // tangent bias must not move the blocked agent toward a global side.
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let params = agent_params();
    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 1.0, 6.0, &params);
    let destination = Vec3::new(7.0, rest_y(&params), 6.0);
    set_manual_path(&mut registry, id, vec![destination]);
    {
        let mut agent = registry
            .get_component::<AgentComponent>(id)
            .unwrap()
            .clone();
        agent.planned_destination = None;
        agent.unstick_window_remaining = 3;
        agent.steer_velocity = Vec3::X * agent.move_speed;
        registry.set_component(id, agent).unwrap();
    }

    let start = agent_position(&registry, id);
    let result = tick(&mut registry, &world, None, GRAVITY, DT);
    let end = agent_position(&registry, id);

    let agent = registry.get_component::<AgentComponent>(id).unwrap();
    assert_eq!(
        result.replans, 1,
        "planned_destination=None should still route through the budgeted replan path"
    );
    assert!(
        agent.blocked,
        "the failed query still reports blocked while recovery continues"
    );
    assert!(
        agent.path.is_empty(),
        "the failed query still drops the stale path"
    );
    assert_eq!(
        agent.planned_destination, None,
        "active recovery should keep retrying budgeted forced replans during its window"
    );
    assert_eq!(
        agent.unstick_window_remaining, 2,
        "failed forced replan should spend one recovery tick, not cancel the retry window"
    );
    assert_eq!(
        agent.steer_velocity,
        Vec3::ZERO,
        "empty blocked path should zero steer_velocity instead of retaining the stale heading"
    );
    assert!(
        distance_xz(start, end) <= EPS,
        "failed forced replan should produce no XZ movement; start={start:?}, end={end:?}"
    );
}

#[test]
fn final_recovery_tick_failed_forced_replan_closes_latch_for_cooldown() {
    // Regression: a failed recovery-forced replan on the final recovery tick
    // left planned_destination=None, causing one more drift-driven retry after
    // the bounded window had expired.
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let params = agent_params();
    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 1.0, 6.0, &params);
    let destination = Vec3::new(7.0, rest_y(&params), 6.0);
    set_manual_path(&mut registry, id, vec![destination]);
    {
        let mut agent = registry
            .get_component::<AgentComponent>(id)
            .unwrap()
            .clone();
        agent.planned_destination = None;
        agent.unstick_window_remaining = 1;
        agent.steer_velocity = Vec3::X * agent.move_speed;
        registry.set_component(id, agent).unwrap();
    }

    let final_recovery_result = tick(&mut registry, &world, None, GRAVITY, DT);

    {
        let agent = registry.get_component::<AgentComponent>(id).unwrap();
        assert_eq!(
            final_recovery_result.replans, 1,
            "the final recovery tick should attempt its forced replan through the budget"
        );
        assert!(agent.blocked, "missing nav graph should mark blocked");
        assert!(agent.path.is_empty(), "failed replan should drop the path");
        assert_eq!(
            agent.unstick_window_remaining, 0,
            "the final recovery tick should spend the remaining window"
        );
        assert_eq!(
            agent.planned_destination,
            Some(destination),
            "an expired recovery window should close the forced-replan latch"
        );
    }

    let following_result = tick(&mut registry, &world, None, GRAVITY, DT);

    let agent = registry.get_component::<AgentComponent>(id).unwrap();
    assert_eq!(
        following_result.replans, 0,
        "after recovery expires, the failed-plan cooldown should block an immediate retry"
    );
    assert_eq!(
        agent.planned_destination,
        Some(destination),
        "normal blocked lifecycle should keep the failed-plan latch closed"
    );
    assert_eq!(
        agent.unstick_window_remaining, 0,
        "the expired recovery window should stay closed"
    );
}

#[test]
fn just_fired_recovery_window_survives_next_tick_failed_forced_replan() {
    // Regression: recovery fires by clearing planned_destination, then the next
    // tick's forced replan fails; that failure must not erase the just-seeded
    // window.
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let params = agent_params();
    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 1.0, 6.0, &params);
    let destination = Vec3::new(7.0, rest_y(&params), 6.0);
    set_manual_path(&mut registry, id, vec![destination]);
    {
        let mut agent = registry
            .get_component::<AgentComponent>(id)
            .unwrap()
            .clone();
        agent.stuck_ticks = STUCK_TICKS_THRESHOLD;
        registry.set_component(id, agent).unwrap();
    }

    let fire_result = tick(&mut registry, &world, None, GRAVITY, DT);
    {
        let agent = registry.get_component::<AgentComponent>(id).unwrap();
        assert_eq!(
            fire_result.replans, 0,
            "threshold fire should only clear the replan latch; admission happens on a later tick"
        );
        assert_eq!(
            agent.unstick_window_remaining,
            UNSTICK_WINDOW - 1,
            "fire tick should seed and spend one recovery tick"
        );
        assert_eq!(
            agent.planned_destination, None,
            "fire tick should request a budgeted forced replan"
        );
    }

    let replan_result = tick(&mut registry, &world, None, GRAVITY, DT);

    let agent = registry.get_component::<AgentComponent>(id).unwrap();
    assert_eq!(
        replan_result.replans, 1,
        "the follow-up forced replan is admitted through the normal budget"
    );
    assert!(agent.blocked, "missing nav graph should still mark blocked");
    assert!(
        agent.path.is_empty(),
        "failed forced replan should still clear the path"
    );
    assert_eq!(
        agent.unstick_window_remaining,
        UNSTICK_WINDOW - 2,
        "failed forced replan should preserve the just-fired recovery window"
    );
    assert_eq!(
        agent.planned_destination, None,
        "just-fired recovery should remain eligible for budgeted retry while the window runs"
    );
}

#[test]
fn agent_decelerates_inside_arrival_band_before_hard_stop() {
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let params = agent_params();
    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 1.0, 6.0, &params);
    let destination = Vec3::new(7.0, rest_y(&params), 6.0);
    set_manual_path(
        &mut registry,
        id,
        vec![Vec3::new(1.0, rest_y(&params), 6.0), destination],
    );

    let slowdown_radius = ARRIVAL_SLOWDOWN_RADIUS_FACTOR * params.radius;
    let mut arrival_band_speeds = Vec::new();
    let mut last_pre_stop_speed = None;
    for _ in 0..300 {
        tick(&mut registry, &world, None, GRAVITY, DT);
        let state = path_state(&registry, id).unwrap();
        let agent = registry.get_component::<AgentComponent>(id).unwrap();
        if agent.arrived {
            break;
        }
        let speed = xz_length(agent.steer_velocity);
        if distance_xz(state.position, destination) <= slowdown_radius {
            arrival_band_speeds.push(speed);
            last_pre_stop_speed = Some(speed);
        }
    }

    assert!(
        arrival_band_speeds.len() >= 2,
        "arrival easing should be observable across multiple ticks, got {:?}",
        arrival_band_speeds
    );
    assert!(
        arrival_band_speeds.first().unwrap() > arrival_band_speeds.last().unwrap(),
        "speed should decrease through the arrival band, got {:?}",
        arrival_band_speeds
    );
    assert!(
        last_pre_stop_speed.unwrap() <= 0.25 * 4.0 + EPS,
        "last pre-stop steer speed should be a low tail, got {:?}",
        arrival_band_speeds
    );
    assert!(
        registry
            .get_component::<AgentComponent>(id)
            .unwrap()
            .arrived,
        "agent should still hard-stop at arrived"
    );
    assert_eq!(
        registry
            .get_component::<AgentComponent>(id)
            .unwrap()
            .steer_velocity,
        Vec3::ZERO,
        "arrived zeroes the integration state"
    );
}

#[test]
fn arrived_agent_displaced_outside_final_radius_resumes_steering() {
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let params = agent_params();
    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 1.0, 6.0, &params);
    let destination = Vec3::new(2.0, rest_y(&params), 6.0);
    set_manual_path(
        &mut registry,
        id,
        vec![Vec3::new(1.0, rest_y(&params), 6.0), destination],
    );

    for _ in 0..180 {
        tick(&mut registry, &world, None, GRAVITY, DT);
        if registry
            .get_component::<AgentComponent>(id)
            .unwrap()
            .arrived
        {
            break;
        }
    }
    assert!(
        registry
            .get_component::<AgentComponent>(id)
            .unwrap()
            .arrived,
        "fixture should reach the destination before displacement"
    );

    let mut transform = *registry.get_component::<Transform>(id).unwrap();
    transform.position.x = destination.x - ARRIVAL_RADIUS_FACTOR * params.radius - 0.25;
    registry.set_component(id, transform).unwrap();

    tick(&mut registry, &world, None, GRAVITY, DT);

    let agent = registry.get_component::<AgentComponent>(id).unwrap();
    assert!(
        !agent.arrived,
        "leaving the final radius should clear the arrived latch"
    );
    assert!(
        agent.steer_velocity.x > 0.0,
        "displaced arrived agent should steer back toward the destination, got {:?}",
        agent.steer_velocity
    );
}

#[test]
fn steer_heading_rotates_by_at_most_turn_rate_each_tick() {
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let params = agent_params();
    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 2.0, 6.0, &params);
    set_manual_path(
        &mut registry,
        id,
        vec![
            Vec3::new(2.0, rest_y(&params), 6.0),
            Vec3::new(2.0, rest_y(&params), 10.0),
        ],
    );
    {
        let mut agent = registry
            .get_component::<AgentComponent>(id)
            .unwrap()
            .clone();
        agent.steer_velocity = Vec3::X * agent.move_speed;
        registry.set_component(id, agent).unwrap();
    }

    tick(&mut registry, &world, None, GRAVITY, DT);

    let heading = registry
        .get_component::<AgentComponent>(id)
        .unwrap()
        .steer_velocity;
    let turned = angle_between_xz(Vec3::X, heading);
    let max_delta = MAX_TURN_RATE * DT;
    assert!(
        turned <= max_delta + EPS,
        "heading should rotate by at most max_turn_rate * dt ({max_delta}), got {turned}"
    );
    assert!(
        turned > 0.5 * max_delta,
        "fixture should engage the turn clamp, got {turned}"
    );
}

#[test]
fn lookahead_targets_path_ahead_and_falls_back_to_current_waypoint() {
    let mut agent = AgentComponent::new(0.35, 1.8, 0.4, 4.0);
    agent.path = vec![Vec3::new(2.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 4.0)];
    let position = Vec3::new(1.8, 0.0, 0.0);

    let lookahead = target_point(&agent, position, 1.0).unwrap();
    assert!(
        (lookahead.x - 2.0).abs() <= EPS && (lookahead.z - 0.8).abs() <= EPS,
        "lookahead should walk past the current waypoint along the path, got {lookahead:?}"
    );

    let disabled = target_point(&agent, position, 0.0).unwrap();
    assert_eq!(disabled, agent.path[0]);

    agent.path.truncate(1);
    let unavailable = target_point(&agent, position, 1.0).unwrap();
    assert_eq!(unavailable, agent.path[0]);
}

#[test]
fn separation_is_added_after_smoothing_then_combined_velocity_is_clamped() {
    let params = agent_params();
    let mut registry = EntityRegistry::new();
    let current_id = spawn_agent(&mut registry, 0.0, 0.0, &params);
    let other_id = spawn_agent(&mut registry, 0.0, -0.2, &params);
    let agent = registry
        .get_component::<AgentComponent>(current_id)
        .unwrap();
    let snapshot = vec![
        AgentSnapshot {
            id: current_id,
            position: Vec3::ZERO,
            radius: agent.radius,
        },
        AgentSnapshot {
            id: other_id,
            position: Vec3::new(0.0, 0.0, -0.2),
            radius: agent.radius,
        },
    ];

    let steer = Vec3::X * agent.move_speed;
    let sep = separation(&snapshot[0], agent, &snapshot);
    let combined = steer + sep;
    assert!(
        xz_length(combined) > agent.move_speed,
        "fixture must engage the clamp: steer={steer:?}, separation={sep:?}"
    );

    let clamped = clamp_xz_speed(combined, agent.move_speed);
    assert!(
        (xz_length(clamped) - agent.move_speed).abs() <= EPS,
        "combined velocity should clamp to move_speed, got {clamped:?}"
    );
    assert_eq!(
        registry
            .get_component::<AgentComponent>(current_id)
            .unwrap()
            .steer_velocity,
        Vec3::ZERO,
        "direct separation calculation must not fold back into steer_velocity"
    );
}

#[test]
fn crowd_separation_does_not_reverse_goal_progress() {
    // Regression: a dense pack in front of a chaser could let separation
    // overpower the ramping goal velocity, making the agent visibly retreat
    // away from its valid path until collision or spacing changed.
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let params = agent_params();
    let mut registry = EntityRegistry::new();
    let chaser = spawn_agent(&mut registry, 4.0, 6.0, &params);
    set_manual_path(
        &mut registry,
        chaser,
        vec![Vec3::new(6.0, rest_y(&params), 6.0)],
    );
    for offset in [0.15, 0.3, 0.45] {
        spawn_agent(&mut registry, 4.0 + offset, 6.0, &params);
    }

    let start = agent_position(&registry, chaser);
    tick(&mut registry, &world, None, GRAVITY, DT);
    let end = agent_position(&registry, chaser);

    assert!(
        end.x >= start.x - EPS,
        "crowd separation should not reverse eastward path progress; start={start:?}, end={end:?}"
    );
}

#[test]
fn overlapping_agents_gain_separation_before_accel_ramp_completes() {
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let graph = wall.nav_graph();
    let params = agent_params();

    let mut registry = EntityRegistry::new();
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
    for _ in 0..2 {
        tick(&mut registry, &world, Some(&graph), GRAVITY, DT);
    }
    let end_gap = {
        let pa = path_state(&registry, a).unwrap().position;
        let pb = path_state(&registry, b).unwrap().position;
        distance_xz(pa, pb)
    };
    assert!(
        end_gap > start_gap + 0.01,
        "separation should act promptly outside the accel ramp, start {start_gap}, end {end_gap}"
    );
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
    set_destination(&mut registry, id, Vec3::new(7.0, rest_y(&params), 6.0));

    let mut arrived = false;
    for _ in 0..600 {
        tick(&mut registry, &world, Some(&graph), GRAVITY, DT);
        if path_state(&registry, id).unwrap().arrived {
            arrived = true;
            break;
        }
    }
    let final_state = path_state(&registry, id).unwrap();
    let final_agent = registry.get_component::<AgentComponent>(id).unwrap();
    assert!(
        arrived,
        "agent in an open region should report arrived; ended at {:?}, distance {}, steer {:?} ({}), velocity {:?} ({}), cursor {}, path_len {}, arrived {}, blocked {}",
        final_state.position,
        final_state.distance_to_destination,
        final_agent.steer_velocity,
        xz_length(final_agent.steer_velocity),
        final_agent.velocity,
        xz_length(final_agent.velocity),
        final_agent.waypoint_cursor,
        final_agent.path.len(),
        final_agent.arrived,
        final_agent.blocked
    );
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
fn blocked_agent_replans_when_live_destination_becomes_directly_routable() {
    // Regression: a failed plan can leave an alert enemy as
    // arrived=false/has_path=false/blocked=true. If the agent later sits on the
    // navmesh with a live target in the same region, that old blocked latch must
    // not preserve the impossible no-path state until the stale cooldown expires.
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let graph = wall.nav_graph();
    let params = agent_params();

    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, -1.0, 6.0, &params);
    let old_destination = Vec3::new(1.0, rest_y(&params), 6.0);
    set_destination(&mut registry, id, old_destination);

    let failed = tick(&mut registry, &world, Some(&graph), GRAVITY, DT);
    assert_eq!(failed.replans, 1, "first tick attempts the bad plan");
    {
        let state = path_state(&registry, id).unwrap();
        assert!(state.blocked, "off-navmesh start should fail pathfinding");
        assert!(!state.has_path, "failed plan should hold no path");
    }

    let mut transform = *registry.get_component::<Transform>(id).unwrap();
    transform.position = Vec3::new(1.0, rest_y(&params), 6.0);
    registry.set_component(id, transform).unwrap();
    let live_destination = Vec3::new(1.25, rest_y(&params), 6.0);
    assert!(
        distance_xz(old_destination, live_destination) <= REPLAN_DEST_THRESHOLD,
        "fixture must stay below the ordinary moving-target drift threshold"
    );
    assert_eq!(
        graph.region_at(transform.position),
        graph.region_at(live_destination),
        "agent and live destination are now directly routable in one region"
    );

    set_destination(&mut registry, id, live_destination);
    let recovered = tick(&mut registry, &world, Some(&graph), GRAVITY, DT);

    assert_eq!(
        recovered.replans, 1,
        "directly-routable live destination should bypass the old blocked cooldown"
    );
    let state = path_state(&registry, id).unwrap();
    assert!(
        !state.blocked,
        "successful direct replan should clear the stale blocked state"
    );
    assert!(
        state.has_path,
        "agent should hold a path to the live destination"
    );
    let agent = registry.get_component::<AgentComponent>(id).unwrap();
    assert!(
        agent
            .planned_destination
            .is_some_and(|planned| distance_xz(planned, live_destination) <= EPS),
        "planned destination should now match the live destination"
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
fn topology_changed_destination_replans_below_euclidean_threshold() {
    // Regression: a stopped chase target can move only a few centimeters across
    // a nav region boundary; Euclidean drift stays below the normal threshold,
    // but the old corridor can now be topologically stale.
    let wall = LWall::fixture();
    let world = wall.collision_world();
    let graph = wall.nav_graph();
    let params = agent_params();

    let mut registry = EntityRegistry::new();
    let id = spawn_agent(&mut registry, 1.0, 1.0, &params);
    let old_dest = Vec3::new(3.5, rest_y(&params), 3.8);
    let new_dest = Vec3::new(3.5, rest_y(&params), 4.1);
    assert!(
        distance_xz(old_dest, new_dest) <= REPLAN_DEST_THRESHOLD,
        "fixture must stay below the Euclidean drift threshold"
    );
    assert_ne!(
        graph.region_at(old_dest),
        graph.region_at(new_dest),
        "fixture must cross nav topology"
    );

    set_manual_path(&mut registry, id, vec![old_dest]);
    set_destination(&mut registry, id, new_dest);

    let result = tick(&mut registry, &world, Some(&graph), GRAVITY, DT);

    assert_eq!(
        result.replans, 1,
        "topology change should be admitted promptly even below Euclidean threshold"
    );
    let agent = registry.get_component::<AgentComponent>(id).unwrap();
    let planned = agent
        .planned_destination
        .expect("topology-driven replan should install a planned destination");
    assert!(
        distance_xz(planned, new_dest) <= EPS,
        "plan should refresh to the live destination, planned {planned:?}"
    );
    assert!(
        agent
            .path
            .last()
            .is_some_and(|waypoint| distance_xz(*waypoint, new_dest) <= EPS),
        "path should now terminate at the live destination: {:?}",
        agent.path
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
