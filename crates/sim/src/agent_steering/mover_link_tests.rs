// Lifts ride baked links: board an already docked lift, ride with steering and
// stuck detection paused, exit at the planned landing, and replan. A real mover
// tick over a hand-built graph: a low floor, a shaft and a high floor.
// See: context/lib/build_pipeline.md §Navigation bake

use glam::Vec3;
use postretro_collision::EnvironmentView;
use postretro_entities::components::agent::{PinEscape, PlannedRide};
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
        self.lift_position().y
    }

    fn lift_position(&self) -> Vec3 {
        self.registry
            .get_component::<Transform>(self.lift)
            .unwrap()
            .position
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
    agent.knockback_velocity = Vec3::new(-8.0, 6.0, 0.0);
    agent.velocity = agent.knockback_velocity;
    rig.registry.set_component(id, agent).unwrap();
    // Landing off the deck, or staying up past the airborne grace, ends it.
    assert!(
        rig.run_until(id, 30, |rig| rig.state(id).ride.is_none()),
        "off the ride, at {:?}",
        rig.position(id)
    );
    assert_eq!(rig.state(id).planned_destination, None, "a replan is owed");
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
        height: HEIGHT,
    };
    let path =
        crate::nav::find_path_from(&rig.graph, NavGate::new(rig.movers.docks()), on_floor, goal)
            .expect("it rides up");
    let ride = path.ride().expect("through a ride");
    assert_eq!(ride.board_waypoint, 0);
    assert_eq!(ride.exit_waypoint, 1);
    assert!(path.crossings().is_empty());
}

// Regression: a walk across a flush lift recorded no crossing, so the lift
// leaving just before the agent stepped on left its path standing and it
// walked into the shaft.
#[test]
fn an_agent_about_to_cross_a_flush_lift_that_leaves_drops_its_path_at_the_landing() {
    let mut rig = Rig::with_east_floor(LiftSpec::on_trigger(), 0.0);
    let goal = Vec3::new(20.0, rest_y(0.0), 5.0);
    let id = rig.agent(Vec3::new(6.0, rest_y(0.0), 5.0), Some(goal));
    assert!(
        rig.run_until(id, 120, |rig| rig.position(id).x > 9.0),
        "it walked up to the lift, at {:?}",
        rig.position(id)
    );
    assert!(
        !rig.state(id).path_crossings.is_empty(),
        "its route crosses the lift"
    );
    assert_ne!(
        rig.state(id).ground,
        GroundRef::Mover(LIFT),
        "not on it yet"
    );

    rig.command(MoverCommand::Start);
    for _ in 0..120 {
        rig.tick();
        let at = rig.position(id);
        assert!(
            at.x < 10.0 && at.y > rest_y(0.0) - 0.2,
            "it never entered the shaft, at {at:?}"
        );
        assert!(rig.state(id).ride.is_none());
    }
    let agent = rig.state(id);
    assert!(
        agent.path.is_empty() && agent.blocked,
        "it dropped its path and latched blocked at the landing"
    );
}

// Regression: a lift passing through its stops never docks, so a body it
// carried off rode it for ever with no stop to end the ride at.
#[test]
fn a_lift_that_never_rests_at_a_stop_carries_an_agent_without_a_ride() {
    let mut rig = Rig::new(LiftSpec::shuttle(0.0));
    let deck = rig.lift_y();
    let id = rig.agent(Vec3::new(12.0, deck + rest_y(0.0), 5.0), None);
    let mut agent = rig.state(id).clone();
    agent.ground = GroundRef::Mover(LIFT);
    rig.registry.set_component(id, agent).unwrap();
    let mut carried = 0;
    // Past a full cycle: two 2 s legs.
    for _ in 0..300 {
        rig.tick();
        assert_eq!(
            rig.movers.docks().docked_waypoint(LIFT),
            None,
            "never docks"
        );
        assert!(rig.state(id).ride.is_none(), "no ride starts");
        carried += u32::from(rig.state(id).ground == GroundRef::Mover(LIFT));
    }
    assert!(carried > 200, "the lift carried it: {carried} ticks");
}

// Regression: a boarder or a rider took a replan slot it never used, so the
// budget served fewer agents than it admitted.
#[test]
fn replan_slots_go_past_an_agent_the_ride_code_steers() {
    let mut rig = Rig::new(LiftSpec::shuttle(3000.0));
    let goal = Vec3::new(20.0, rest_y(TOP), 5.0);
    let at = Vec3::new(9.4, rest_y(0.0), 5.0);
    let boarder = rig.agent(at, Some(goal));
    let board_point = Vec3::new(14.0 - 3.0 * RADIUS, 0.0, 5.0);
    let mut agent = rig.state(boarder).clone();
    // At the end of its walk to the lift, its plan owed a refresh.
    agent.ground = GroundRef::World;
    agent.planned_ride = Some(PlannedRide {
        mover_id: LIFT,
        board_waypoint: 0,
        exit_waypoint: 1,
        board_point,
        exit_point: Vec3::new(15.05, TOP, 5.0),
    });
    agent.path = vec![at, board_point];
    agent.mandatory_waypoints = vec![false, false];
    agent.arrived = true;
    agent.planned_destination = None;
    rig.registry.set_component(boarder, agent).unwrap();
    // Later in slot order, enough drifters to fill the budget.
    let drifters: Vec<EntityId> = (0..REPLAN_BUDGET_PER_TICK)
        .map(|i| {
            let from = Vec3::new(2.0 + i as f32, rest_y(0.0), 2.0);
            rig.agent(from, Some(Vec3::new(2.0, rest_y(0.0), 8.0)))
        })
        .collect();

    assert_eq!(
        rig.tick().replans,
        REPLAN_BUDGET_PER_TICK,
        "every slot replanned an agent"
    );
    for id in drifters {
        assert!(
            rig.state(id).planned_destination.is_some(),
            "{id:?} planned"
        );
    }
    assert!(
        rig.state(boarder).planned_ride.is_some(),
        "boarding went on"
    );
}

// Regression: the replan gate read the floor under a body on a docked lift, so
// a target there read as directly routable every tick, and a search from the
// lift that failed ran every tick past the cooldown.
#[test]
fn a_blocked_agent_on_a_docked_lift_waits_out_its_cooldown() {
    let mut rig = Rig::new(LiftSpec::shuttle(6000.0));
    let mut section = section(TOP);
    // The shaft's floor: no portal or link reaches it.
    section.regions.push(NavRegion {
        x0: 10,
        z0: 0,
        x1: 14,
        z1: 10,
        floor_y_min: -8.0,
        floor_y_max: -8.0,
    });
    rig.graph = NavGraph::from_section(&section);
    crate::nav::install_mover_records(&mut rig.graph, &rig.registry, &mut rig.movers);
    let target = Vec3::new(12.0, rest_y(-8.0), 5.0);
    let id = rig.agent(Vec3::new(12.0, rest_y(0.0), 5.0), Some(target));
    let mut agent = rig.state(id).clone();
    agent.ground = GroundRef::Mover(LIFT);
    rig.registry.set_component(id, agent).unwrap();

    let replans: u32 = (0..60).map(|_| rig.tick().replans).sum();
    assert_eq!(rig.state(id).ground, GroundRef::Mover(LIFT), "on the lift");
    assert!(rig.state(id).blocked, "no route from the lift to the shaft");
    assert!(
        replans <= 3,
        "{replans} replans in 60 ticks: the cooldown holds"
    );
}

// Regression: the ride began at the first touch of the deck, leaving the body
// on the rim; a lift setting off down from its top dock could drop it.
#[test]
fn an_agent_boarding_a_descending_lift_at_its_top_dock_stays_aboard() {
    let mut rig = Rig::new(LiftSpec::shuttle(3000.0));
    for _ in 0..600 {
        rig.tick();
        if rig.movers.docks().docked_waypoint(LIFT) == Some(1) {
            break;
        }
    }
    assert_eq!(
        rig.movers.docks().docked_waypoint(LIFT),
        Some(1),
        "at the top"
    );
    let goal = Vec3::new(5.0, rest_y(0.0), 5.0);
    let id = rig.agent(Vec3::new(18.0, rest_y(TOP), 5.0), Some(goal));
    assert!(
        rig.run_until(id, 150, |rig| rig.state(id).ride.is_some()),
        "it boarded at the top, at {:?}",
        rig.position(id)
    );
    // The boarding point lies three radii in from the east edge at x 14.
    let board_x = 14.0 - 3.0 * RADIUS;
    assert!(
        rig.position(id).x < board_x + 1.5 * RADIUS + 0.05,
        "it walked well onto the deck before standing, at {:?}",
        rig.position(id)
    );

    let mut descended = false;
    for _ in 0..600 {
        rig.tick();
        let agent = rig.state(id);
        if agent.ride.is_none() {
            break;
        }
        // Mid-shaft only: stepping off at the low dock changes its ground.
        if (0.1..TOP - 0.1).contains(&rig.lift_y()) {
            descended = true;
            assert_eq!(
                agent.ground,
                GroundRef::Mover(LIFT),
                "aboard on the way down, at {:?}",
                rig.position(id)
            );
        }
    }
    assert!(descended, "it rode down");
    assert!(
        rig.run_until(id, 300, |rig| rig.position(id).x < 9.0),
        "it stepped off at the low landing, at {:?}",
        rig.position(id)
    );
    assert!(rig.position(id).y < 1.5, "on the low floor");
}

// Regression: one airborne tick ended a ride, losing its exit; landing back on
// the deck then started an unplanned one.
#[test]
fn a_rider_keeps_its_ride_through_a_short_hop_and_loses_it_past_the_grace() {
    let mut rig = Rig::new(LiftSpec::shuttle(6000.0));
    let id = rig.boarded();
    let exit = rig.state(id).ride.unwrap().exit_waypoint;
    assert!(exit.is_some(), "a planned ride");
    let hop = |rig: &mut Rig, up: f32| {
        let mut agent = rig.state(id).clone();
        agent.ground = GroundRef::Airborne;
        agent.velocity = Vec3::Y * up;
        rig.registry.set_component(id, agent).unwrap();
        let mut transform = *rig.registry.get_component::<Transform>(id).unwrap();
        transform.position.y += 0.05;
        rig.registry.set_component(id, transform).unwrap();
    };

    // About 0.12 s in the air: inside the 150 ms grace.
    hop(&mut rig, 1.0);
    let mut airborne = 0;
    for _ in 0..30 {
        rig.tick();
        let ride = rig.state(id).ride.expect("a short hop keeps the ride");
        assert_eq!(ride.exit_waypoint, exit, "and its exit");
        airborne += u32::from(rig.state(id).ground == GroundRef::Airborne);
    }
    assert!(airborne > 0, "it left the deck");
    assert_eq!(rig.state(id).ground, GroundRef::Mover(LIFT), "and landed");

    // About 0.4 s in the air: past the grace.
    hop(&mut rig, 4.0);
    assert!(
        rig.run_until(id, 30, |rig| rig.state(id).ride.is_none()),
        "a long leap ends the ride"
    );
    assert_eq!(
        rig.state(id).ground,
        GroundRef::Airborne,
        "the grace ran out in the air"
    );
}

// Regression: a walk across a lift gave up only when an admitted replan failed,
// so under contention path steering drove the agents that lost the budget race
// on toward the rim for up to ⌈N / budget⌉ ticks after the lift left.
#[test]
fn every_agent_about_to_cross_a_flush_lift_that_leaves_drops_its_path_that_tick_under_contention() {
    let mut rig = Rig::with_east_floor(LiftSpec::on_trigger(), 0.0);
    let goal = Vec3::new(20.0, rest_y(0.0), 5.0);
    // More crossers than two ticks of replan budget, all owed on the same tick.
    let mut starts: Vec<Vec3> = [8.2, 6.7]
        .into_iter()
        .flat_map(|x| [3.6, 4.5, 5.4, 6.3].map(|z| Vec3::new(x, rest_y(0.0), z)))
        .collect();
    starts.push(Vec3::new(5.2, rest_y(0.0), 5.0));
    assert!(starts.len() > 2 * REPLAN_BUDGET_PER_TICK as usize);
    let ids: Vec<EntityId> = starts.iter().map(|&at| rig.agent(at, Some(goal))).collect();
    let planned = |rig: &Rig| {
        ids.iter()
            .all(|&id| !rig.state(id).path_crossings.is_empty())
    };
    for _ in 0..(ids.len() as u32).div_ceil(REPLAN_BUDGET_PER_TICK) + 1 {
        if planned(&rig) {
            break;
        }
        rig.tick();
    }
    assert!(planned(&rig), "every route crosses the lift");
    for &id in &ids {
        assert!(rig.position(id).x < 9.0, "short of the rim");
    }

    rig.command(MoverCommand::Start);
    let result = rig.tick();
    assert_eq!(
        rig.movers.docks().docked_waypoint(LIFT),
        None,
        "the lift left"
    );
    assert!(result.replans <= REPLAN_BUDGET_PER_TICK);
    for &id in &ids {
        let agent = rig.state(id);
        assert!(
            agent.path.is_empty() && agent.blocked,
            "{id:?} dropped its path and latched blocked the tick the lift left"
        );
    }
    for _ in 0..120 {
        rig.tick();
        for &id in &ids {
            let at = rig.position(id);
            assert!(
                at.x < 10.0 && at.y > rest_y(0.0) - 0.2,
                "{id:?} never entered the shaft, at {at:?}"
            );
        }
    }
}

// Regression: a step clear aimed at a spot on the pinning platform's own deck
// walked toward that fixed world point while the deck carried the agent on, so
// the spot drifted toward the trailing edge and the agent followed it off. The
// spot is kept in the deck's frame: the agent reaches it on the deck wherever
// the platform has gone, with no hold or release in between.
#[test]
fn an_agent_stepping_clear_to_a_deck_spot_reaches_it_on_the_moving_platform() {
    let spec = LiftSpec {
        waypoints: vec![Vec3::ZERO, Vec3::X * 4.0],
        ..LiftSpec::shuttle(300.0)
    };
    let mut rig = Rig::new(spec);
    let id = rig.agent(Vec3::new(13.0, rest_y(0.0), 5.0), None);
    let mut agent = rig.state(id).clone();
    agent.ground = GroundRef::Mover(LIFT);
    rig.registry.set_component(id, agent).unwrap();
    assert!(
        rig.run_until(id, 120, |rig| rig.lift_position().x > 1.0),
        "the platform set off"
    );
    assert_eq!(rig.state(id).ground, GroundRef::Mover(LIFT), "aboard");

    // A spot 2 m back along the deck, 1 m in from its trailing edge (deck x
    // 10..14 in the mover's frame). The walk takes about 30 ticks, over which
    // the platform travels about a metre.
    let set_off_at = rig.lift_position();
    let start = rig.position(id);
    let spot = Vec3::new(11.0, start.y - set_off_at.y, 5.0);
    let mut agent = rig.state(id).clone();
    agent.pin_escape = Some(PinEscape {
        mover_id: LIFT,
        target: set_off_at + spot,
        deck_offset: Some(spot),
        ticks_left: 90,
    });
    rig.registry.set_component(id, agent).unwrap();
    let mut ticks = 0;
    while rig.state(id).pin_escape.is_some() {
        assert!(ticks < 60, "the walk arrives, at {:?}", rig.position(id));
        rig.tick();
        ticks += 1;
        let at = rig.position(id);
        assert_eq!(
            rig.state(id).ground,
            GroundRef::Mover(LIFT),
            "on the deck, at {at:?}"
        );
        assert!(at.y > rest_y(0.0) - 0.2, "never fell, at {at:?}");
    }
    assert!(
        rig.lift_position().x - set_off_at.x > 0.5,
        "the platform moved on under the walk, to {:?}",
        rig.lift_position()
    );

    // Arrived at the spot on the deck, not at the world point it set off for,
    // and carried there from then on.
    for _ in 0..20 {
        let on_deck = rig.position(id) - rig.lift_position();
        assert!(
            distance_xz(on_deck, spot) <= 0.12,
            "at the deck spot {spot:?}, at {on_deck:?} on the deck"
        );
        assert_eq!(rig.state(id).ground, GroundRef::Mover(LIFT));
        rig.tick();
    }
}

// Regression: a body on a linked lift that never rests found no route start
// while the lift was between docks, so every replan failed and it latched
// `blocked` for good, and Ai held its reachability for ever.
#[test]
fn an_agent_on_a_lift_that_never_rests_replans_from_the_floor_under_it() {
    let mut rig = Rig::new(LiftSpec::shuttle(0.0));
    let deck = rig.lift_y();
    let goal = Vec3::new(5.0, rest_y(0.0), 5.0);
    let id = rig.agent(Vec3::new(10.6, deck + rest_y(0.0), 5.0), Some(goal));
    let mut agent = rig.state(id).clone();
    agent.ground = GroundRef::Mover(LIFT);
    rig.registry.set_component(id, agent).unwrap();
    let gate = NavGate::new(rig.movers.docks());
    assert_eq!(gate.docked_waypoint(LIFT), None, "between docks");
    assert!(
        !rig.graph.between_docks(gate, GroundRef::Mover(LIFT)),
        "plain ground: nothing to wait for"
    );

    assert!(
        rig.run_until(id, 10, |rig| !rig.state(id).path.is_empty()),
        "it planned from the floor under it"
    );
    assert!(!rig.state(id).blocked);
    assert!(rig.state(id).planned_ride.is_none());
    assert!(
        rig.run_until(id, 300, |rig| rig.position(id).x < 9.0),
        "and walked off toward its goal, at {:?}",
        rig.position(id)
    );
}
