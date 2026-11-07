// Gate contention over the compiled stress map: every agent chases a moving goal
// while the map's movers cycle, and no gate change waits past the replan budget.
// Ignored: it needs a current `content/dev/maps/stress-env-volumes.prl`.
// See: context/lib/build_pipeline.md §Navigation bake

use glam::Vec3;
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::{
    EntityId, EntityRegistry, KinematicMoverComponent, MoverCommand, Transform,
};
use postretro_physics::collision::CollisionWorld;
use postretro_physics::collision::moving::CombinedCollisionWorld;
use postretro_physics::kinematic_mover::{
    MoverTickStateTable, apply_mover_command, run_kinematic_mover_tick,
};
use postretro_sim::agent_steering::{
    MoverPins, REPLAN_BUDGET_PER_TICK, set_destination, tick_with_mover_pins,
};
use postretro_sim::nav::{NavGate, NavGraph, install_mover_records};

const MAP: &str = "content/dev/maps/stress-env-volumes.prl";
const AGENTS: usize = 64;
const TICKS: u32 = 3600;
const DT: f32 = 1.0 / 60.0;

/// How often the harness commands every mover, and moves the goal, in ticks.
const COMMAND_PERIOD: u32 = 180;
const GOAL_PERIOD: u32 = 45;

fn region_centre(graph: &NavGraph, index: usize) -> Vec3 {
    let region = &graph.regions()[index];
    Vec3::new(
        (region.world_min_xz[0] + region.world_max_xz[0]) * 0.5,
        region.floor_y_max + graph.agent_params().height * 0.5 + 0.03,
        (region.world_min_xz[1] + region.world_max_xz[1]) * 0.5,
    )
}

fn spawn_agents(registry: &mut EntityRegistry, graph: &NavGraph) -> Vec<EntityId> {
    let stride = (graph.regions().len() / AGENTS).max(1);
    (0..AGENTS)
        .map(|index| {
            let at = region_centre(graph, (index * stride) % graph.regions().len());
            let id = registry.spawn(Transform {
                position: at,
                ..Transform::default()
            });
            registry
                .set_component(
                    id,
                    AgentComponent::from_nav_params(&graph.agent_params(), 4.0),
                )
                .unwrap();
            id
        })
        .collect()
}

fn command_every_mover(registry: &mut EntityRegistry, command: &MoverCommand) {
    let movers: Vec<EntityId> = registry
        .iter_with_kind(postretro_entities::ComponentKind::KinematicMover)
        .map(|(id, _)| id)
        .collect();
    for id in movers {
        let mut mover = registry
            .get_component::<KinematicMoverComponent>(id)
            .unwrap()
            .clone();
        apply_mover_command(&mut mover, command);
        registry.set_component(id, mover).unwrap();
    }
}

/// Whether the live gate generation still owes `agent` a replan.
fn owed(agent: &AgentComponent, graph: &NavGraph, gate: NavGate<'_>) -> bool {
    agent.gate_generation != gate.generation()
        && agent.destination.is_some()
        && agent.ride.is_none()
        && agent.pin_escape.is_none()
        && (agent.blocked
            || agent
                .path_portals
                .iter()
                .any(|portal| gate.portal_closed(graph, *portal as usize)))
}

#[test]
#[ignore = "needs a current stress-env-volumes.prl; run once at the gate"]
fn under_contention_on_the_stress_map_no_gate_change_outlasts_the_budget() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");
    let world = postretro_level_loader::load_prl(&format!("{root}{MAP}")).expect("loads");
    let mut graph = NavGraph::from_section(world.navmesh.as_ref().expect("the map has a navmesh"));
    let collision_world = CollisionWorld::from_level(&world).expect("collision builds");
    let colliders = crate::runtime_movers::build_loaded_mover_colliders(&world);
    let mut registry = EntityRegistry::new();
    crate::runtime_movers::spawn_loaded_kinematic_movers(&mut registry, &world, 0.0)
        .expect("movers spawn");
    let mut movers = MoverTickStateTable::default();
    install_mover_records(&mut graph, &registry, &mut movers);
    assert!(!graph.gates().is_empty(), "the map's doors gate portals");
    let ids = spawn_agents(&mut registry, &graph);

    let limit = (AGENTS as u32).div_ceil(REPLAN_BUDGET_PER_TICK);
    let environment = postretro_collision::EnvironmentView::level_only(-20.0);
    let mut waiting = vec![0_u32; ids.len()];
    let mut worst = 0;
    let mut flips = 0;
    let mut last_generation = 0;
    for tick in 0..TICKS {
        if tick % COMMAND_PERIOD == 0 {
            let command = if (tick / COMMAND_PERIOD).is_multiple_of(2) {
                MoverCommand::Start
            } else {
                MoverCommand::Reverse
            };
            command_every_mover(&mut registry, &command);
        }
        if tick % GOAL_PERIOD == 0 {
            let index = (tick / GOAL_PERIOD) as usize * 97 % graph.regions().len();
            let goal = region_centre(&graph, index);
            for id in &ids {
                set_destination(&mut registry, *id, goal);
            }
        }
        run_kinematic_mover_tick(&mut registry, &mut movers, DT);
        let gate = NavGate::new(movers.docks());
        flips += u32::from(gate.generation() != last_generation);
        last_generation = gate.generation();
        let collision = CombinedCollisionWorld::new(&collision_world, &colliders, &movers);
        let result = tick_with_mover_pins(
            &mut registry,
            &collision,
            Some(&graph),
            MoverPins { yields: &[], gate },
            &mut Vec::new(),
            environment,
            DT,
        );
        assert!(result.replans <= REPLAN_BUDGET_PER_TICK);
        for (index, id) in ids.iter().enumerate() {
            let agent = registry.get_component::<AgentComponent>(*id).unwrap();
            waiting[index] = if owed(agent, &graph, gate) {
                waiting[index] + 1
            } else {
                0
            };
            worst = worst.max(waiting[index]);
        }
    }
    println!("gate flips {flips}, worst wait {worst} ticks, limit {limit}");
    assert!(flips > 4, "the movers exercised the gate");
    assert!(
        worst <= limit,
        "an agent waited {worst} ticks, limit {limit}"
    );
}
