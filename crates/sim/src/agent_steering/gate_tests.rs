// Doors gate routes: a dock closes baked portals, routes and replans respect it,
// and a gate change is answered first within the replan budget. Runs a real
// mover tick over a hand-built graph of two rooms joined by a doorway.
// See: context/lib/build_pipeline.md §Navigation bake

use glam::Vec3;
use postretro_collision::EnvironmentView;
use postretro_entities::{
    KinematicMoverComponent, KinematicMoverConfig, KinematicMoverMode, MoverCommand, Transform,
};
use postretro_level_format::navmesh::{
    NAVMESH_VERSION, NavMeshSection, NavMoverGate, NavPortal, NavRegion,
};
use postretro_test_log_capture::LogCapture;

use super::*;
use crate::collision::moving::MoverCollider;
use crate::collision::{CollisionWorld, SKIN_DISTANCE};
use crate::kinematic_mover::{MoverTickStateTable, apply_mover_command, run_kinematic_mover_tick};
use crate::nav::{NavMoverBound, find_path_gated};

const DT: f32 = 1.0 / 60.0;
const ENV: EnvironmentView<'static> = EnvironmentView::level_only(-20.0);
const DOOR: u32 = 4;
const RADIUS: f32 = 0.35;
const HEIGHT: f32 = 1.8;
const REST_Y: f32 = HEIGHT / 2.0 + SKIN_DISTANCE;
/// A point in the west room, and one in the east room.
const WEST: Vec3 = Vec3::new(5.0, REST_Y, 5.0);
const EAST: Vec3 = Vec3::new(16.0, REST_Y, 5.0);
/// The door's closed and open origins: it lifts out of the doorway.
const CLOSED: Vec3 = Vec3::new(10.0, 0.0, 4.0);
const OPEN: Vec3 = Vec3::new(10.0, 4.0, 4.0);

/// West room, doorway, east room: regions 0, 1, 2. A door docked at waypoint 0
/// closes both portals of the doorway.
fn section(gated: bool) -> NavMeshSection {
    let region = |x0, x1, z0, z1| NavRegion {
        x0,
        z0,
        x1,
        z1,
        floor_y_min: 0.0,
        floor_y_max: 0.0,
    };
    let portal = |region_a, region_b, x: f32| NavPortal {
        region_a,
        region_b,
        left: [x, 0.0, 6.0],
        right: [x, 0.0, 4.0],
    };
    NavMeshSection {
        version: NAVMESH_VERSION,
        origin: [0.0, 0.0, 0.0],
        cell_size: 1.0,
        dim_x: 21,
        dim_z: 10,
        agent_radius: RADIUS,
        agent_height: HEIGHT,
        step_height: 0.4,
        max_slope_deg: 45.0,
        regions: vec![
            region(0, 10, 0, 10),
            region(10, 11, 4, 6),
            region(11, 21, 0, 10),
        ],
        portals: vec![portal(0, 1, 10.0), portal(1, 2, 11.0)],
        gates: if gated {
            vec![
                NavMoverGate {
                    mover_id: DOOR,
                    waypoint: 0,
                    portal: 0,
                },
                NavMoverGate {
                    mover_id: DOOR,
                    waypoint: 0,
                    portal: 1,
                },
            ]
        } else {
            Vec::new()
        },
        links: Vec::new(),
    }
}

fn quad(points: &mut Vec<Vec3>, tris: &mut Vec<[u32; 3]>, corners: [Vec3; 4]) {
    let base = points.len() as u32;
    points.extend(corners);
    tris.push([base, base + 2, base + 1]);
    tris.push([base, base + 3, base + 2]);
}

/// Floor plus the two wall lengths beside the doorway at x = 10.
fn world() -> CollisionWorld {
    let mut points = Vec::new();
    let mut tris = Vec::new();
    quad(
        &mut points,
        &mut tris,
        [
            Vec3::new(-5.0, 0.0, -5.0),
            Vec3::new(30.0, 0.0, -5.0),
            Vec3::new(30.0, 0.0, 15.0),
            Vec3::new(-5.0, 0.0, 15.0),
        ],
    );
    for (z0, z1) in [(-5.0, 4.0), (6.0, 15.0)] {
        quad(
            &mut points,
            &mut tris,
            [
                Vec3::new(10.0, 0.0, z0),
                Vec3::new(10.0, 4.0, z0),
                Vec3::new(10.0, 4.0, z1),
                Vec3::new(10.0, 0.0, z1),
            ],
        );
    }
    CollisionWorld::from_triangles_for_test(points, tris)
}

/// The door leaf: a plane across the doorway.
fn leaf() -> MoverCollider {
    MoverCollider::from_local_triangles(
        DOOR,
        &[
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 3.5, 0.0),
            Vec3::new(0.0, 3.5, 2.0),
            Vec3::new(0.0, 0.0, 2.0),
        ],
        &[[0, 1, 2], [0, 2, 3]],
    )
    .unwrap()
}

struct Rig {
    registry: EntityRegistry,
    world: CollisionWorld,
    colliders: Vec<MoverCollider>,
    movers: MoverTickStateTable,
    graph: NavGraph,
    door: EntityId,
}

impl Rig {
    fn new(mode: KinematicMoverMode, wait_ms: f32, started: bool) -> Self {
        Self::with_waypoints(vec![CLOSED, OPEN], mode, wait_ms, started)
    }

    fn with_waypoints(
        waypoints: Vec<Vec3>,
        mode: KinematicMoverMode,
        wait_ms: f32,
        started: bool,
    ) -> Self {
        let mut registry = EntityRegistry::new();
        let door = registry.spawn(Transform {
            position: waypoints[0],
            ..Transform::default()
        });
        let names = (0..waypoints.len()).map(|i| format!("w{i}")).collect();
        let mover = KinematicMoverComponent::new(
            DOOR,
            KinematicMoverConfig {
                waypoints,
                waypoint_names: names,
                speed_mps: 4.0,
                wait_ms,
                mode,
                started,
                spin_axis: Vec3::ZERO,
                initial_spin_rate_rad_s: 0.0,
                spin_accel_rad_s2: 0.0,
                carry_yaw: false,
            },
        );
        registry.set_component(door, mover).unwrap();
        let mut movers = MoverTickStateTable::default();
        let mut graph = NavGraph::from_section(&section(true));
        crate::nav::install_mover_records(&mut graph, &registry, &mut movers);
        Self {
            registry,
            world: world(),
            colliders: vec![leaf()],
            movers,
            graph,
            door,
        }
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

    /// One mover tick and one steering tick, in fixed-tick order.
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
            .get_component::<KinematicMoverComponent>(self.door)
            .unwrap()
            .clone();
        apply_mover_command(&mut mover, &command);
        self.registry.set_component(self.door, mover).unwrap();
    }

    fn gate(&self) -> NavGate<'_> {
        NavGate::new(self.movers.docks())
    }

    fn routed(&self, from: Vec3, to: Vec3) -> bool {
        find_path_gated(&self.graph, self.gate(), from, to).is_some()
    }

    fn position(&self, id: EntityId) -> Vec3 {
        self.registry
            .get_component::<Transform>(id)
            .unwrap()
            .position
    }

    fn agent_state(&self, id: EntityId) -> &AgentComponent {
        self.registry.get_component::<AgentComponent>(id).unwrap()
    }
}

fn closed_door() -> Rig {
    Rig::new(KinematicMoverMode::Once, 0.0, false)
}

#[test]
fn a_door_spawned_closed_gates_routes_from_the_first_tick() {
    let mut rig = closed_door();
    rig.tick();
    assert!(!rig.routed(WEST, EAST));
    assert!(!rig.routed(EAST, WEST));
}

#[test]
fn an_agent_beside_a_closed_door_plans_on_its_own_side() {
    let mut rig = closed_door();
    rig.tick();
    let beside = Vec3::new(9.5, REST_Y, 5.0);
    assert!(rig.routed(beside, WEST), "out on its own side");
    assert!(rig.routed(WEST, beside), "reached from its own side");
    assert!(!rig.routed(beside, EAST), "and not through the door");
}

#[test]
fn a_chaser_latches_blocked_holds_and_passes_once_the_door_opens() {
    let mut rig = closed_door();
    let id = rig.agent(Vec3::new(8.0, REST_Y, 5.0), Some(EAST));
    let mut low = Vec3::splat(f32::MAX);
    let mut high = Vec3::splat(f32::MIN);
    let mut recoveries = 0;
    for tick in 0..300 {
        rig.tick();
        if tick >= 2 {
            assert!(rig.agent_state(id).blocked, "tick {tick}: latched");
        }
        low = low.min(rig.position(id));
        high = high.max(rig.position(id));
        recoveries += u32::from(rig.agent_state(id).unstick_window_remaining > 0);
    }
    assert!(
        (high - low).max_element() <= 0.05,
        "held still: {low:?}..{high:?}"
    );
    assert!(recoveries <= 1, "stuck recovery fired {recoveries} times");

    rig.command(MoverCommand::Start);
    let passed = (0..300).position(|_| {
        rig.tick();
        rig.position(id).x > 10.5
    });
    let open_at = 4.0 / 4.0 * 60.0;
    let passed = passed.expect("the agent passed") as f32;
    assert!(
        passed <= open_at + 120.0,
        "passed {passed} ticks after the command, the door docks open at {open_at}"
    );
}

#[test]
fn a_kept_path_through_a_door_that_docks_is_dropped() {
    let mut rig = Rig::new(KinematicMoverMode::Once, 0.0, false);
    rig.command(MoverCommand::Start);
    let id = rig.agent(Vec3::new(2.0, REST_Y, 5.0), Some(EAST));
    // Fully open and parked, so the route crosses the doorway.
    for _ in 0..80 {
        rig.tick();
    }
    assert!(!rig.agent_state(id).path_portals.is_empty(), "it planned");
    rig.command(MoverCommand::Reverse);
    let mut closed = false;
    for _ in 0..120 {
        rig.tick();
        closed |= rig.gate().portal_closed(&rig.graph, 0);
        if closed && rig.agent_state(id).blocked {
            break;
        }
    }
    assert!(closed, "the door docked closed");
    let agent = rig.agent_state(id);
    assert!(agent.path.is_empty() && agent.path_portals.is_empty());
    assert!(agent.blocked);
}

#[test]
fn a_failed_replan_keeps_a_path_that_crosses_no_gate() {
    let mut rig = closed_door();
    let id = rig.agent(Vec3::new(2.0, REST_Y, 2.0), Some(EAST));
    let mut agent = rig.agent_state(id).clone();
    let kept = vec![Vec3::new(4.0, REST_Y, 2.0), Vec3::new(8.0, REST_Y, 2.0)];
    agent.path = kept.clone();
    agent.mandatory_waypoints = vec![false, false];
    agent.planned_destination = None;
    rig.registry.set_component(id, agent).unwrap();
    rig.tick();
    let agent = rig.agent_state(id);
    assert_eq!(agent.path, kept, "stale but moving");
    assert!(!agent.blocked);
}

#[test]
fn two_gate_flips_before_admission_cost_one_replan() {
    let mut rig = closed_door();
    let id = rig.agent(Vec3::new(8.0, REST_Y, 5.0), Some(EAST));
    for _ in 0..40 {
        rig.tick();
    }
    assert!(rig.agent_state(id).blocked);
    let mut agent = rig.agent_state(id).clone();
    agent.replan_cooldown_ticks = u32::MAX;
    agent.planned_destination = Some(EAST);
    rig.registry.set_component(id, agent).unwrap();

    // Two flips land before the agent is next admitted.
    let settled = rig.gate().generation();
    rig.command(MoverCommand::Start);
    run_kinematic_mover_tick(&mut rig.registry, &mut rig.movers, DT);
    rig.command(MoverCommand::Reverse);
    for _ in 0..6 {
        run_kinematic_mover_tick(&mut rig.registry, &mut rig.movers, DT);
    }
    assert!(rig.gate().generation() >= settled + 2, "both flips counted");
    let replans: u32 = (0..5).map(|_| rig.tick().replans).sum();
    assert_eq!(replans, 1);
}

#[test]
fn sixteen_chasers_answer_a_door_docking_within_the_budget() {
    let mut rig = Rig::new(KinematicMoverMode::Once, 0.0, false);
    rig.command(MoverCommand::Start);
    for _ in 0..80 {
        rig.tick();
    }
    let ids: Vec<EntityId> = (0..16)
        .map(|i| {
            let at = Vec3::new(
                1.0 + 0.5 * (i % 8) as f32,
                REST_Y,
                2.0 + (i / 8) as f32 * 6.0,
            );
            rig.agent(at, Some(EAST))
        })
        .collect();
    for _ in 0..30 {
        rig.tick();
    }
    assert!(
        ids.iter().all(|id| !rig.agent_state(*id).path.is_empty()),
        "all routed through the open door"
    );
    rig.command(MoverCommand::Reverse);
    let mut docked_tick = None;
    for tick in 0..120 {
        rig.tick();
        if rig.gate().portal_closed(&rig.graph, 0) {
            docked_tick = Some(tick);
            break;
        }
    }
    docked_tick.expect("the door docked");
    let limit = 16_u32.div_ceil(REPLAN_BUDGET_PER_TICK);
    for _ in 0..limit {
        rig.tick();
    }
    for id in &ids {
        assert!(
            rig.agent_state(*id).blocked,
            "{id:?} latched within {limit}"
        );
    }
}

#[test]
fn a_gate_change_is_admitted_ahead_of_target_drift() {
    let mut rig = closed_door();
    let blocked = rig.agent(Vec3::new(8.0, REST_Y, 5.0), Some(EAST));
    for _ in 0..40 {
        rig.tick();
    }
    assert!(rig.agent_state(blocked).blocked);
    // Agents earlier in slot order than none of this: every drifter wants the
    // budget on the tick the gate opens.
    let drifters: Vec<EntityId> = (0..8)
        .map(|i| rig.agent(Vec3::new(1.0 + i as f32 * 0.8, REST_Y, 1.0), Some(WEST)))
        .collect();
    rig.command(MoverCommand::Start);
    for id in &drifters {
        let mut agent = rig.agent_state(*id).clone();
        agent.planned_destination = None;
        rig.registry.set_component(*id, agent).unwrap();
    }
    rig.tick();
    rig.tick();
    assert!(
        !rig.agent_state(blocked).blocked,
        "the blocked agent replanned first"
    );
}

#[test]
fn a_door_and_a_lift_gate_on_every_tick_of_a_wait_and_stop_on_the_tick_they_leave() {
    let mut rig = Rig::new(KinematicMoverMode::PingPong, 500.0, true);
    let mut docked_ticks = 0;
    let mut seen_undock = false;
    for _ in 0..240 {
        rig.tick();
        let mover = rig
            .registry
            .get_component::<KinematicMoverComponent>(rig.door)
            .unwrap();
        let waiting = mover.wait_remaining_ms > 0.0
            && mover.current_linear_velocity == Vec3::ZERO
            && mover.segment_index == 0;
        let moving = mover.current_linear_velocity != Vec3::ZERO;
        let closed = rig.gate().portal_closed(&rig.graph, 0);
        if waiting {
            assert!(closed, "gated through the wait");
            docked_ticks += 1;
        }
        if moving {
            assert!(!closed, "open from the tick it leaves");
            seen_undock = true;
        }
    }
    assert!(docked_ticks > 20 && seen_undock);
}

#[test]
fn a_closed_door_held_at_its_waypoint_keeps_its_gate_and_clears_on_leaving() {
    let mut rig = closed_door();
    rig.tick();
    let mut mover = rig
        .registry
        .get_component::<KinematicMoverComponent>(rig.door)
        .unwrap()
        .clone();
    apply_mover_command(&mut mover, &MoverCommand::Start);
    mover.blocked = true;
    rig.registry.set_component(rig.door, mover).unwrap();
    for _ in 0..30 {
        rig.tick();
        assert!(rig.gate().portal_closed(&rig.graph, 0), "held, still gated");
    }
    let mut mover = rig
        .registry
        .get_component::<KinematicMoverComponent>(rig.door)
        .unwrap()
        .clone();
    mover.blocked = false;
    rig.registry.set_component(rig.door, mover).unwrap();
    rig.tick();
    assert!(
        !rig.gate().portal_closed(&rig.graph, 0),
        "cleared on leaving"
    );
}

#[test]
fn a_sweeper_and_a_door_held_mid_travel_change_no_generation_and_admit_no_replan() {
    let mut sweeper = Rig::new(KinematicMoverMode::PingPong, 0.0, true);
    sweeper.tick();
    let generation = sweeper.gate().generation();
    let id = sweeper.agent(Vec3::new(2.0, REST_Y, 2.0), Some(WEST));
    sweeper.tick();
    let mut agent = sweeper.agent_state(id).clone();
    agent.replan_cooldown_ticks = u32::MAX;
    sweeper.registry.set_component(id, agent).unwrap();
    for _ in 0..360 {
        assert_eq!(sweeper.tick().replans, 0);
    }
    assert_eq!(sweeper.gate().generation(), generation);

    let mut door = Rig::new(KinematicMoverMode::Once, 0.0, false);
    door.command(MoverCommand::Start);
    for _ in 0..20 {
        door.tick();
    }
    let mut mover = door
        .registry
        .get_component::<KinematicMoverComponent>(door.door)
        .unwrap()
        .clone();
    mover.blocked = true;
    door.registry.set_component(door.door, mover).unwrap();
    door.tick();
    let generation = door.gate().generation();
    for _ in 0..120 {
        door.tick();
        assert!(!door.gate().portal_closed(&door.graph, 0));
    }
    assert_eq!(door.gate().generation(), generation);
}

#[test]
fn a_mover_held_at_a_middle_node_gates_there_and_one_passing_through_does_not() {
    let middle = Vec3::new(10.0, 2.0, 4.0);
    let mut rig = Rig::with_waypoints(
        vec![OPEN, middle, CLOSED],
        KinematicMoverMode::Once,
        0.0,
        false,
    );
    let mut section = section(false);
    section.gates = vec![NavMoverGate {
        mover_id: DOOR,
        waypoint: 1,
        portal: 0,
    }];
    rig.graph = NavGraph::from_section(&section);
    crate::nav::install_mover_records(&mut rig.graph, &rig.registry, &mut rig.movers);

    rig.command(MoverCommand::Start);
    let mut passed_closed = false;
    for _ in 0..120 {
        rig.tick();
        passed_closed |= rig.gate().portal_closed(&rig.graph, 0);
    }
    assert!(!passed_closed, "passing through the node gates nothing");

    rig.command(MoverCommand::GoToPathNode("w1".to_string()));
    for _ in 0..120 {
        rig.tick();
    }
    assert!(
        rig.gate().portal_closed(&rig.graph, 0),
        "held at the node, it gates there"
    );
}

#[test]
fn unloading_a_gated_level_leaves_nothing_on_the_next_one() {
    let mut rig = closed_door();
    rig.tick();
    assert!(!rig.routed(WEST, EAST));

    rig.movers.clear();
    rig.registry.despawn(rig.door).unwrap();
    rig.graph = NavGraph::from_section(&section(false));
    crate::nav::install_mover_records(&mut rig.graph, &rig.registry, &mut rig.movers);
    let id = rig.agent(WEST, Some(EAST));
    rig.tick();
    assert!(rig.routed(WEST, EAST), "the same portal index routes");
    assert!(!rig.agent_state(id).path.is_empty(), "on the first tick");
}

#[test]
fn faulty_records_drop_with_a_warning_and_the_rest_of_navigation_works() {
    let capture = LogCapture::start();
    let mut section = section(true);
    section.gates.push(NavMoverGate {
        mover_id: 99,
        waypoint: 0,
        portal: 0,
    });
    section.gates.push(NavMoverGate {
        mover_id: DOOR,
        waypoint: 9,
        portal: 0,
    });
    section.gates.push(NavMoverGate {
        mover_id: DOOR,
        waypoint: 0,
        portal: 77,
    });
    let mut graph = NavGraph::from_section(&section);
    graph.validate_mover_records(&[NavMoverBound {
        mover_id: DOOR,
        waypoint_count: 2,
    }]);
    assert_eq!(graph.gates().len(), 2, "the good records survive");
    let warnings = capture
        .records()
        .iter()
        .filter(|record| record.message.contains("dropped mover gate"))
        .count();
    assert_eq!(warnings, 3);
    assert!(find_path_gated(&graph, NavGate::OPEN, WEST, EAST).is_some());
}

#[test]
fn a_level_whose_movers_failed_to_spawn_runs_with_every_gate_off() {
    let mut graph = NavGraph::from_section(&section(true));
    let registry = EntityRegistry::new();
    let mut movers = MoverTickStateTable::default();
    crate::nav::install_mover_records(&mut graph, &registry, &mut movers);
    assert!(graph.gates().is_empty());
    assert!(find_path_gated(&graph, NavGate::new(movers.docks()), WEST, EAST).is_some());
}

#[test]
fn an_agent_sent_into_the_door_body_holds_with_no_repeated_stuck_recovery() {
    let mut rig = closed_door();
    let inside = Vec3::new(10.5, REST_Y, 5.0);
    let id = rig.agent(Vec3::new(8.0, REST_Y, 5.0), Some(inside));
    let mut fired = 0;
    let mut previous = 0;
    for _ in 0..300 {
        rig.tick();
        let window = rig.agent_state(id).unstick_window_remaining;
        fired += u32::from(window > previous);
        previous = window;
    }
    assert!(fired <= 1, "stuck recovery fired {fired} times");
}

#[test]
fn combat_positioning_never_picks_a_slot_overlapping_a_docked_door() {
    use crate::combat_positioning::{CombatQuery, combat_candidates};

    let mut rig = closed_door();
    rig.tick();
    let target = Vec3::new(9.0, REST_Y, 5.0);
    let candidates = |mover_nav| {
        combat_candidates(&CombatQuery {
            claimant_id: 1,
            agent_pos: WEST,
            engagement_radius: 0.9,
            target_pos: target,
            enemy_eye_offset: Vec3::Y * 0.7,
            target_aim: target,
            combat_slot: None,
            scan_challengers: true,
            other_agents: &[],
            nav_graph: &rig.graph,
            mover_nav,
            collision_world: &rig.world,
            path_length_score_weight: 0.1,
        })
    };
    let in_the_door = |position: Vec3| {
        position.x + RADIUS > 10.0 && (4.0 - RADIUS..6.0 + RADIUS).contains(&position.z)
    };
    assert!(
        candidates(crate::nav::MoverNav::none())
            .iter()
            .any(|candidate| in_the_door(candidate.position)),
        "the fixture offers a slot the door overlaps"
    );
    let gated = candidates(crate::nav::MoverNav {
        gate: rig.gate(),
        movers: &rig.colliders,
        poses: &rig.movers,
    });
    assert!(!gated.is_empty(), "other slots remain");
    assert!(
        gated
            .iter()
            .all(|candidate| !in_the_door(candidate.position)),
        "no slot overlaps the door: {gated:?}"
    );
}
