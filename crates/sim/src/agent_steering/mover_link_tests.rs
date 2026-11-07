// Lifts ride baked links: board an already docked lift, ride with steering and
// stuck detection paused, exit at the planned landing, and replan. A real mover
// tick over a hand-built graph: a low floor, a shaft and a high floor.
// See: context/lib/build_pipeline.md §Navigation bake

use glam::Vec3;
use postretro_collision::EnvironmentView;
use postretro_entities::{
    KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode, MoverCommand, Transform,
};
use postretro_foundation::GroundRef;
use postretro_level_format::navmesh::{NAVMESH_VERSION, NavMeshSection, NavMoverLink, NavRegion};

use super::*;
use crate::collision::moving::MoverCollider;
use crate::collision::{CollisionWorld, SKIN_DISTANCE};
use crate::kinematic_mover::{MoverTickStateTable, apply_mover_command, run_kinematic_mover_tick};

const DT: f32 = 1.0 / 60.0;
const ENV: EnvironmentView<'static> = EnvironmentView::level_only(-20.0);
const LIFT: u32 = 9;
const RADIUS: f32 = 0.35;
const HEIGHT: f32 = 1.8;
const TOP: f32 = 4.0;

fn rest_y(floor: f32) -> f32 {
    floor + HEIGHT / 2.0 + SKIN_DISTANCE
}

/// Regions: west floor at 0, east floor at `TOP`. The lift's top spans x 10..14,
/// z 3..7, with a boarding edge on each side.
fn section(east_floor: f32) -> NavMeshSection {
    let region = |x0, x1, floor| NavRegion {
        x0,
        z0: 0,
        x1,
        z1: 10,
        floor_y_min: floor,
        floor_y_max: floor,
    };
    let west_link = |waypoint| NavMoverLink {
        mover_id: LIFT,
        waypoint,
        region: 0,
        left: [10.0, 0.0, 3.0],
        right: [10.0, 0.0, 7.0],
    };
    let east_link = |waypoint| NavMoverLink {
        mover_id: LIFT,
        waypoint,
        region: 1,
        left: [14.0, east_floor, 7.0],
        right: [14.0, east_floor, 3.0],
    };
    let links = if east_floor == 0.0 {
        // Flush with both floors at one dock.
        vec![west_link(0), east_link(0)]
    } else {
        vec![west_link(0), east_link(1)]
    };
    NavMeshSection {
        version: NAVMESH_VERSION,
        origin: [0.0, 0.0, 0.0],
        cell_size: 1.0,
        dim_x: 24,
        dim_z: 10,
        agent_radius: RADIUS,
        agent_height: HEIGHT,
        step_height: 0.4,
        max_slope_deg: 45.0,
        regions: vec![region(0, 10, 0.0), region(14, 24, east_floor)],
        portals: Vec::new(),
        gates: Vec::new(),
        links,
    }
}

fn quad(points: &mut Vec<Vec3>, tris: &mut Vec<[u32; 3]>, corners: [Vec3; 4]) {
    let base = points.len() as u32;
    points.extend(corners);
    tris.push([base, base + 2, base + 1]);
    tris.push([base, base + 3, base + 2]);
}

fn floor(points: &mut Vec<Vec3>, tris: &mut Vec<[u32; 3]>, x: (f32, f32), y: f32) {
    quad(
        points,
        tris,
        [
            Vec3::new(x.0, y, -5.0),
            Vec3::new(x.1, y, -5.0),
            Vec3::new(x.1, y, 15.0),
            Vec3::new(x.0, y, 15.0),
        ],
    );
}

fn world(east_floor: f32) -> CollisionWorld {
    let mut points = Vec::new();
    let mut tris = Vec::new();
    floor(&mut points, &mut tris, (-5.0, 10.0), 0.0);
    floor(&mut points, &mut tris, (10.0, 14.0), -8.0);
    floor(&mut points, &mut tris, (14.0, 30.0), east_floor);
    CollisionWorld::from_triangles_for_test(points, tris)
}

/// The lift's top: a slab 0.2 m thick, its top at the mover origin's height.
fn deck() -> MoverCollider {
    let (x0, x1, z0, z1) = (10.0, 14.0, 3.0, 7.0);
    MoverCollider::from_local_triangles(
        LIFT,
        &[
            Vec3::new(x0, 0.0, z0),
            Vec3::new(x1, 0.0, z0),
            Vec3::new(x1, 0.0, z1),
            Vec3::new(x0, 0.0, z1),
        ],
        &[[0, 2, 1], [0, 3, 2]],
    )
    .unwrap()
}

struct Rig {
    registry: EntityRegistry,
    world: CollisionWorld,
    colliders: Vec<MoverCollider>,
    movers: MoverTickStateTable,
    graph: NavGraph,
    lift: EntityId,
}

struct LiftSpec {
    mode: KinematicMoverMode,
    started: bool,
    wait_ms: f32,
    waypoints: Vec<Vec3>,
}

impl LiftSpec {
    /// A lift that shuttles by itself, waiting `wait_ms` at each end.
    fn shuttle(wait_ms: f32) -> Self {
        Self {
            mode: KinematicMoverMode::PingPong,
            started: true,
            wait_ms,
            waypoints: vec![Vec3::ZERO, Vec3::Y * TOP],
        }
    }

    /// A lift that moves only when told.
    fn on_trigger() -> Self {
        Self {
            mode: KinematicMoverMode::Once,
            started: false,
            wait_ms: 0.0,
            waypoints: vec![Vec3::ZERO, Vec3::Y * TOP],
        }
    }
}

impl Rig {
    fn new(spec: LiftSpec) -> Self {
        Self::with_east_floor(spec, TOP)
    }

    fn with_east_floor(spec: LiftSpec, east_floor: f32) -> Self {
        let mut registry = EntityRegistry::new();
        let lift = registry.spawn(Transform {
            position: spec.waypoints[0],
            ..Transform::default()
        });
        let names = (0..spec.waypoints.len()).map(|i| format!("w{i}")).collect();
        let mover = KinematicMoverComponent::new(
            LIFT,
            KinematicMoverConfig {
                waypoints: spec.waypoints,
                waypoint_names: names,
                speed_mps: 2.0,
                wait_ms: spec.wait_ms,
                mode: spec.mode,
                started: spec.started,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        );
        registry.set_component(lift, mover).unwrap();
        let mut movers = MoverTickStateTable::default();
        let mut graph = NavGraph::from_section(&section(east_floor));
        crate::nav::install_mover_records(&mut graph, &registry, &mut movers);
        let mut rig = Self {
            registry,
            world: world(east_floor),
            colliders: vec![deck()],
            movers,
            graph,
            lift,
        };
        // A started shuttle leaves its first dock at once: run it round to the
        // start of a wait there.
        for _ in 0..3000 {
            run_kinematic_mover_tick(&mut rig.registry, &mut rig.movers, DT);
            if rig.movers.docks().docked_waypoint(LIFT) == Some(0) {
                break;
            }
        }
        rig
    }

    fn agent(&mut self, at: Vec3, goal: Option<Vec3>) -> EntityId {
        let id = self.registry.spawn(Transform {
            position: at,
            ..Transform::default()
        });
        let mut agent = AgentComponent::new(RADIUS, HEIGHT, 0.4, 4.0);
        agent.destination = goal;
        self.registry.set_component(id, agent).unwrap();
        id
    }

    fn tick(&mut self) -> AgentTickResult {
        run_kinematic_mover_tick(&mut self.registry, &mut self.movers, DT);
        let collision = CombinedCollisionWorld::new(&self.world, &self.colliders, &self.movers);
        tick_with_mover_pins(
            &mut self.registry,
            &collision,
            Some(&self.graph),
            MoverPins {
                yields: &[],
                gate: NavGate::new(self.movers.docks()),
            },
            &mut Vec::new(),
            ENV,
            DT,
        )
    }

    fn command(&mut self, command: MoverCommand) {
        let mut mover = self
            .registry
            .get_component::<KinematicMoverComponent>(self.lift)
            .unwrap()
            .clone();
        apply_mover_command(&mut mover, &command);
        self.registry.set_component(self.lift, mover).unwrap();
    }

    fn position(&self, id: EntityId) -> Vec3 {
        self.registry
            .get_component::<Transform>(id)
            .unwrap()
            .position
    }

    fn state(&self, id: EntityId) -> &AgentComponent {
        self.registry.get_component::<AgentComponent>(id).unwrap()
    }

    fn lift_y(&self) -> f32 {
        self.registry
            .get_component::<Transform>(self.lift)
            .unwrap()
            .position
            .y
    }

    /// Tick until `done`, at most `ticks`. Asserts no stuck recovery while riding.
    fn run_until(&mut self, id: EntityId, ticks: u32, done: impl Fn(&Self) -> bool) -> bool {
        for _ in 0..ticks {
            self.tick();
            let agent = self.state(id);
            if agent.ride.is_some() {
                assert_eq!(agent.stuck_ticks, 0, "no stuck evidence on a ride");
                assert_eq!(agent.unstick_window_remaining, 0);
            }
            if done(self) {
                return true;
            }
        }
        false
    }

    /// A rider standing on the docked lift's top, its ride plan already made.
    fn boarded(&mut self) -> EntityId {
        let goal = Vec3::new(20.0, rest_y(TOP), 5.0);
        let id = self.agent(Vec3::new(12.0, rest_y(0.0), 5.0), Some(goal));
        let mut agent = self.state(id).clone();
        agent.ground = GroundRef::Mover(LIFT);
        self.registry.set_component(id, agent).unwrap();
        assert!(
            self.run_until(id, 10, |rig| rig.state(id).ride.is_some()),
            "it boarded, at {:?}",
            self.position(id)
        );
        id
    }
}

#[test]
fn an_agent_boards_rides_exits_at_the_far_landing_and_replans() {
    let mut rig = Rig::new(LiftSpec::shuttle(3000.0));
    let goal = Vec3::new(20.0, rest_y(TOP), 5.0);
    let id = rig.agent(Vec3::new(5.0, rest_y(0.0), 5.0), Some(goal));

    assert!(rig.run_until(id, 150, |rig| rig.state(id).planned_ride.is_some()));
    assert!(
        rig.run_until(id, 200, |rig| rig.state(id).ride.is_some()),
        "it boarded the docked lift, at {:?}",
        rig.position(id)
    );
    let replans_before_ride_ends = rig.tick().replans;
    assert_eq!(replans_before_ride_ends, 0);

    assert!(
        rig.run_until(id, 900, |rig| rig.position(id).x > 15.0),
        "it stepped off at the far landing, at {:?}",
        rig.position(id)
    );
    assert!(rig.state(id).ride.is_none());
    assert!(
        rig.run_until(id, 300, |rig| (rig.position(id).x - goal.x).abs() < 1.0),
        "and walked on to its goal, at {:?}",
        rig.position(id)
    );
    assert!(rig.position(id).y > TOP, "on the high floor");
}

#[test]
fn an_agent_holds_through_the_wait_on_a_docked_lift_and_rides_when_it_leaves() {
    let mut rig = Rig::new(LiftSpec::shuttle(3000.0));
    let id = rig.boarded();
    let on_board = rig.position(id);
    let mut waited = 0;
    while rig.lift_y() == 0.0 {
        rig.tick();
        waited += 1;
        assert!(waited < 400, "the lift left");
        assert!(
            rig.position(id).distance(on_board) < 0.8,
            "it stays on through the wait, at {:?}",
            rig.position(id)
        );
    }
    assert!(rig.state(id).ride.is_some());
    rig.tick();
    assert!(
        rig.position(id).y > on_board.y,
        "it rode when the lift left"
    );
}

#[test]
fn no_replan_is_admitted_mid_ride() {
    let mut rig = Rig::new(LiftSpec::shuttle(500.0));
    let id = rig.boarded();
    let mut agent = rig.state(id).clone();
    agent.planned_destination = None;
    agent.replan_cooldown_ticks = 0;
    rig.registry.set_component(id, agent).unwrap();
    for _ in 0..60 {
        if rig.state(id).ride.is_none() {
            break;
        }
        assert_eq!(rig.tick().replans, 0);
    }
}

#[test]
fn a_lift_that_moves_only_on_a_trigger_offers_no_path() {
    let mut rig = Rig::new(LiftSpec::on_trigger());
    let goal = Vec3::new(20.0, rest_y(TOP), 5.0);
    let id = rig.agent(Vec3::new(5.0, rest_y(0.0), 5.0), Some(goal));
    for _ in 0..30 {
        rig.tick();
    }
    assert!(rig.state(id).blocked, "no ride: it latches blocked");
    assert!(rig.state(id).planned_ride.is_none());
}

#[test]
fn a_parked_lift_offers_no_path() {
    // `stop` parks it at this dock.
    let mut stopped = Rig::new(LiftSpec::shuttle(3000.0));
    stopped.tick();
    stopped.command(MoverCommand::Stop);
    let goal = Vec3::new(20.0, rest_y(TOP), 5.0);
    let id = stopped.agent(Vec3::new(5.0, rest_y(0.0), 5.0), Some(goal));
    for _ in 0..30 {
        stopped.tick();
    }
    assert!(stopped.state(id).blocked, "a stopped lift offers no ride");

    // A `goToPathNode` hold parks it at the far dock, with its link still open.
    let mut held = Rig::new(LiftSpec::shuttle(3000.0));
    held.command(MoverCommand::GoToPathNode("w1".to_string()));
    for _ in 0..240 {
        held.tick();
    }
    assert_eq!(held.movers.docks().docked_waypoint(LIFT), Some(1));
    let goal = Vec3::new(5.0, rest_y(0.0), 5.0);
    let id = held.agent(Vec3::new(20.0, rest_y(TOP), 5.0), Some(goal));
    for _ in 0..30 {
        held.tick();
    }
    assert!(held.state(id).blocked, "a held lift offers no ride");
}

#[test]
fn an_agent_whose_lift_leaves_before_it_steps_on_stays_at_the_landing() {
    let mut rig = Rig::new(LiftSpec::shuttle(600.0));
    let goal = Vec3::new(20.0, rest_y(TOP), 5.0);
    // Far enough that the wait ends before it reaches the rim.
    let id = rig.agent(Vec3::new(1.0, rest_y(0.0), 5.0), Some(goal));
    for _ in 0..200 {
        rig.tick();
        assert!(
            rig.position(id).y > 0.5,
            "it never walked into the shaft, at {:?}",
            rig.position(id)
        );
        if rig.state(id).ride.is_some() {
            panic!("the fixture boarded before the lift left");
        }
        if rig.lift_y() > 1.0 {
            break;
        }
    }
    let agent = rig.state(id);
    assert!(agent.path.is_empty() && agent.blocked, "dropped its path");
    assert!(rig.position(id).x < 10.0, "at the landing");
}

#[test]
fn a_rider_stopped_between_docks_stays_on_and_does_not_walk_off() {
    let mut rig = Rig::new(LiftSpec::shuttle(300.0));
    let id = rig.boarded();
    assert!(rig.run_until(id, 300, |rig| rig.lift_y() > 1.5));
    rig.command(MoverCommand::Stop);
    let at = rig.position(id);
    for _ in 0..120 {
        rig.tick();
        assert!(rig.state(id).ride.is_some(), "still riding");
        assert_eq!(rig.state(id).ground, GroundRef::Mover(LIFT));
        assert_eq!(rig.state(id).stuck_ticks, 0);
    }
    assert!(rig.position(id).distance(at) < 0.3, "it stayed put");
}

#[test]
fn a_rider_whose_lift_leaves_mid_exit_stays_on_and_never_walks_into_the_shaft() {
    let mut rig = Rig::new(LiftSpec::shuttle(150.0));
    let id = rig.boarded();
    for _ in 0..900 {
        rig.tick();
        if rig.state(id).ride.is_none() {
            break;
        }
        assert_eq!(
            rig.state(id).ground,
            GroundRef::Mover(LIFT),
            "on the lift until it steps off, at {:?}",
            rig.position(id)
        );
    }
    let at = rig.position(id);
    assert!(
        at.y > rest_y(0.0) - 0.5,
        "it left on a landing, not into the shaft, at {at:?}"
    );
}

#[test]
fn a_rider_knocked_off_resumes_steering_and_replans() {
    let mut rig = Rig::new(LiftSpec::shuttle(3000.0));
    let id = rig.boarded();
    let mut agent = rig.state(id).clone();
    agent.ground = GroundRef::Airborne;
    agent.knockback_velocity = Vec3::new(-8.0, 3.0, 0.0);
    agent.velocity = agent.knockback_velocity;
    rig.registry.set_component(id, agent).unwrap();
    rig.tick();
    let agent = rig.state(id);
    assert!(agent.ride.is_none(), "off the ride");
    assert_eq!(agent.planned_destination, None, "a replan is owed");
}

#[test]
fn an_agent_a_riser_lifts_rides_as_a_boarded_one_and_leaves_at_the_dock() {
    let mut rig = Rig::new(LiftSpec::shuttle(300.0));
    let id = rig.agent(Vec3::new(12.0, rest_y(0.0), 5.0), None);
    assert!(
        rig.run_until(id, 120, |rig| rig.state(id).ride.is_some()),
        "carried off, it rides"
    );
    assert_eq!(rig.state(id).ride.unwrap().exit_waypoint, None);
    assert!(
        rig.run_until(id, 300, |rig| rig.state(id).ride.is_none()),
        "at the next dock it leaves Ride"
    );
}

#[test]
fn a_rider_past_one_cycle_leaves_ride_at_the_dock() {
    let mut rig = Rig::new(LiftSpec::shuttle(300.0));
    let id = rig.boarded();
    // The planned landing is gone: the rider must not stay aboard for ever.
    let mut agent = rig.state(id).clone();
    let mut ride = agent.ride.unwrap();
    ride.exit_waypoint = Some(7);
    agent.ride = Some(ride);
    rig.registry.set_component(id, agent).unwrap();
    assert!(
        rig.run_until(id, 400, |rig| rig.state(id).ride.is_none()),
        "off within one cycle"
    );
}

#[test]
fn a_hold_at_a_middle_node_ends_the_ride_there() {
    let middle = Vec3::Y * 2.0;
    let spec = LiftSpec {
        waypoints: vec![Vec3::ZERO, middle, Vec3::Y * TOP],
        ..LiftSpec::shuttle(300.0)
    };
    let mut rig = Rig::new(spec);
    let mut section = section(TOP);
    section.links[1].waypoint = 2;
    rig.graph = NavGraph::from_section(&section);
    crate::nav::install_mover_records(&mut rig.graph, &rig.registry, &mut rig.movers);
    for _ in 0..3 {
        run_kinematic_mover_tick(&mut rig.registry, &mut rig.movers, DT);
    }
    let id = rig.boarded();
    assert!(rig.run_until(id, 200, |rig| rig.lift_y() > 0.2));
    rig.command(MoverCommand::GoToPathNode("w1".to_string()));
    assert!(
        rig.run_until(id, 240, |rig| rig.state(id).ride.is_none()),
        "a hold elsewhere ends the ride, at {:?}",
        rig.position(id)
    );
}

#[test]
fn an_agent_walks_across_a_lift_docked_flush_without_stopping() {
    let mut rig = Rig::with_east_floor(LiftSpec::on_trigger(), 0.0);
    let goal = Vec3::new(20.0, rest_y(0.0), 5.0);
    let id = rig.agent(Vec3::new(6.0, rest_y(0.0), 5.0), Some(goal));
    let mut last = rig.position(id).x;
    let mut idle = 0;
    assert!(
        rig.run_until(id, 300, |rig| rig.position(id).x > 15.0),
        "it crossed, at {:?}",
        rig.position(id)
    );
    for _ in 0..30 {
        rig.tick();
        let x = rig.position(id).x;
        idle += u32::from(x - last < 1.0e-3);
        last = x;
    }
    assert!(rig.state(id).ride.is_none() && rig.state(id).planned_ride.is_none());
    assert!(idle < 30, "it kept walking");
}

#[test]
fn a_lift_that_leaves_while_an_agent_crosses_carries_it() {
    let mut rig = Rig::with_east_floor(LiftSpec::on_trigger(), 0.0);
    let goal = Vec3::new(20.0, rest_y(0.0), 5.0);
    let id = rig.agent(Vec3::new(6.0, rest_y(0.0), 5.0), Some(goal));
    assert!(rig.run_until(id, 200, |rig| {
        rig.state(id).ground == GroundRef::Mover(LIFT)
    }));
    rig.command(MoverCommand::Start);
    assert!(
        rig.run_until(id, 30, |rig| rig.state(id).ride.is_some()),
        "carried off the crossing, it rides"
    );
}

#[test]
fn a_body_inside_the_lowered_room_routes_by_its_own_surface_only() {
    // The room's floor has no link at the lowered dock and its roof does: a body
    // on the floor must ride up, never route over the roof.
    let mut rig = Rig::new(LiftSpec::shuttle(6000.0));
    let mut section = section(TOP);
    section.links.push(NavMoverLink {
        mover_id: LIFT,
        waypoint: 0,
        region: 1,
        left: [14.0, TOP, 7.0],
        right: [14.0, TOP, 3.0],
    });
    section.links[0] = NavMoverLink {
        region: 1,
        ..section.links[1]
    };
    section.links[0].waypoint = 1;
    section.links[0].left[1] = TOP;
    section.links[0].right[1] = TOP;
    section.links.remove(1);
    section.links.sort_by_key(|link| link.waypoint);
    rig.graph = NavGraph::from_section(&section);
    crate::nav::install_mover_records(&mut rig.graph, &rig.registry, &mut rig.movers);
    rig.tick();

    let goal = Vec3::new(20.0, rest_y(TOP), 5.0);
    let on_floor = NavStart {
        position: Vec3::new(12.0, rest_y(0.0), 5.0),
        ground: GroundRef::Mover(LIFT),
    };
    let path =
        crate::nav::find_path_from(&rig.graph, NavGate::new(rig.movers.docks()), on_floor, goal)
            .expect("it rides up");
    let ride = path.ride().expect("through a ride");
    assert_eq!(ride.board_waypoint, 0);
    assert_eq!(ride.exit_waypoint, 1);
    assert!(path.portals().is_empty());
}
