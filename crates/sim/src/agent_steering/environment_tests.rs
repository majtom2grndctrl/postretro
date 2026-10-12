// Agent gravity under environment volumes (per agent, tick-start position, downward part only; push ignored) and the resolve's no-allocation guard.
// See: context/plans/in-progress/E24--water-and-environment-volumes/index.md

use glam::Vec3;

use postretro_collision::{
    ConvexLeaf, EnvironmentView, EnvironmentVolume, EnvironmentVolumes, LeafPlane,
};
use postretro_entities::components::agent::AgentComponent;
use postretro_entities::{EntityId, EntityRegistry, Transform};

use super::tick;
use crate::collision::CollisionWorld;

const DT: f32 = 1.0 / 60.0;
const LEVEL_GRAVITY: f32 = -20.0;
const VOLUME_GRAVITY: f32 = -3.0;
const VEL_EPS: f32 = 1.0e-4;

fn volume_box(min: Vec3, max: Vec3) -> ConvexLeaf {
    let plane = |normal: Vec3, distance: f32| LeafPlane { normal, distance };
    ConvexLeaf {
        planes: vec![
            plane(Vec3::X, max.x),
            plane(Vec3::NEG_X, -min.x),
            plane(Vec3::Y, max.y),
            plane(Vec3::NEG_Y, -min.y),
            plane(Vec3::Z, max.z),
            plane(Vec3::NEG_Z, -min.z),
        ],
        aabb_min: min,
        aabb_max: max,
    }
}

fn gravity_volume(min: Vec3, max: Vec3, gravity: f32) -> EnvironmentVolume {
    EnvironmentVolume {
        source_entity: 1,
        priority: 0,
        gravity: Some(gravity),
        push: None,
        fluid: None,
        brushes: vec![volume_box(min, max)],
    }
}

/// An idle agent (no destination) falling with vertical velocity `vy`. The empty
/// collision world keeps it airborne, so the idle settle path integrates gravity
/// every tick.
fn spawn_falling_agent(registry: &mut EntityRegistry, position: Vec3, vy: f32) -> EntityId {
    let id = registry.spawn(Transform {
        position,
        ..Transform::default()
    });
    let mut agent = AgentComponent::new(0.35, 1.8, 0.4, 4.0);
    agent.velocity = Vec3::new(0.0, vy, 0.0);
    registry.set_component(id, agent).unwrap();
    id
}

/// One steering tick; returns the agent's new position and its change in
/// vertical velocity.
fn step(
    registry: &mut EntityRegistry,
    id: EntityId,
    environment: EnvironmentView<'_>,
) -> (Vec3, f32) {
    let vy_before = registry
        .get_component::<AgentComponent>(id)
        .unwrap()
        .velocity
        .y;
    let _ = tick(registry, &CollisionWorld::new(), None, environment, DT);
    let position = registry.get_component::<Transform>(id).unwrap().position;
    let vy_after = registry
        .get_component::<AgentComponent>(id)
        .unwrap()
        .velocity
        .y;
    (position, vy_after - vy_before)
}

#[test]
fn agent_falls_at_volume_gravity_inside_low_gravity_volume() {
    let volumes = EnvironmentVolumes::build(vec![gravity_volume(
        Vec3::splat(-50.0),
        Vec3::splat(50.0),
        VOLUME_GRAVITY,
    )]);
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    let mut registry = EntityRegistry::new();
    let agent = spawn_falling_agent(&mut registry, Vec3::new(0.0, 10.0, 0.0), 0.0);
    for _ in 0..30 {
        let (_, dv) = step(&mut registry, agent, environment);
        assert!(
            (dv - VOLUME_GRAVITY * DT).abs() < VEL_EPS,
            "inside the volume the agent integrates its gravity: dv={dv}"
        );
    }
}

// Ordering pin P1, agent half: gravity resolves at the agent's tick-start position.
#[test]
fn agent_crossing_out_of_gravity_volume_integrates_volume_gravity_then_level_gravity() {
    let volumes = EnvironmentVolumes::build(vec![gravity_volume(
        Vec3::new(-50.0, 0.0, -50.0),
        Vec3::new(50.0, 20.0, 50.0),
        VOLUME_GRAVITY,
    )]);
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    let mut registry = EntityRegistry::new();
    let agent = spawn_falling_agent(&mut registry, Vec3::new(0.0, 0.05, 0.0), -6.0);

    let (after_crossing, crossing_dv) = step(&mut registry, agent, environment);
    assert!(
        after_crossing.y < 0.0,
        "fixture: the crossing tick must leave the volume, ended at {after_crossing:?}"
    );
    assert!(
        (crossing_dv - VOLUME_GRAVITY * DT).abs() < VEL_EPS,
        "the crossing tick integrates the volume's gravity: dv={crossing_dv}"
    );
    let (_, next_dv) = step(&mut registry, agent, environment);
    assert!(
        (next_dv - LEVEL_GRAVITY * DT).abs() < VEL_EPS,
        "the first tick outside integrates level gravity: dv={next_dv}"
    );
}

#[test]
fn agent_in_upward_gravity_volume_keeps_vertical_velocity() {
    // Agents take only gravity's downward part: an upward-gravity volume acts as
    // zero gravity for them.
    let volumes = EnvironmentVolumes::build(vec![gravity_volume(
        Vec3::splat(-50.0),
        Vec3::splat(50.0),
        5.0,
    )]);
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    let mut registry = EntityRegistry::new();
    let agent = spawn_falling_agent(&mut registry, Vec3::new(0.0, 10.0, 0.0), -1.0);
    for _ in 0..10 {
        let (_, dv) = step(&mut registry, agent, environment);
        assert!(
            dv.abs() < VEL_EPS,
            "upward gravity never lifts an agent: dv={dv}"
        );
    }
}

/// A flat floor at y = 0 spanning x, z in [-50, 50].
fn floor_world() -> CollisionWorld {
    let points = vec![
        Vec3::new(-50.0, 0.0, -50.0),
        Vec3::new(50.0, 0.0, -50.0),
        Vec3::new(50.0, 0.0, 50.0),
        Vec3::new(-50.0, 0.0, 50.0),
    ];
    CollisionWorld::from_triangles_for_test(points, vec![[0, 2, 1], [0, 3, 2]])
}

/// A grounded agent at rest on `floor_world` (capsule one skin above the floor).
fn spawn_resting_agent(registry: &mut EntityRegistry, x: f32, z: f32) -> (EntityId, f32) {
    let agent = AgentComponent::new(0.35, 1.8, 0.4, 4.0);
    let rest_y = agent.half_height() + agent.radius + crate::collision::SKIN_DISTANCE;
    let id = registry.spawn(Transform {
        position: Vec3::new(x, rest_y, z),
        ..Transform::default()
    });
    registry.set_component(id, agent).unwrap();
    (id, rest_y)
}

fn agent_pose(registry: &EntityRegistry, id: EntityId) -> (Vec3, bool) {
    let position = registry.get_component::<Transform>(id).unwrap().position;
    let grounded = registry
        .get_component::<AgentComponent>(id)
        .unwrap()
        .is_grounded;
    (position, grounded)
}

/// Walk a resting agent along a straight manual path across `floor_world` for
/// `ticks` ticks; returns its pose after each tick.
fn walk_path(environment: EnvironmentView<'_>, ticks: usize) -> Vec<(Vec3, bool)> {
    let world = floor_world();
    let mut registry = EntityRegistry::new();
    let (id, rest_y) = spawn_resting_agent(&mut registry, -10.0, 0.0);
    let destination = Vec3::new(10.0, rest_y, 0.0);
    let mut agent = registry
        .get_component::<AgentComponent>(id)
        .unwrap()
        .clone();
    agent.destination = Some(destination);
    agent.planned_destination = Some(destination);
    agent.path = vec![Vec3::new(-10.0, rest_y, 0.0), destination];
    agent.mandatory_waypoints.clear();
    agent.waypoint_cursor = 0;
    agent.replan_cooldown_ticks = u32::MAX;
    registry.set_component(id, agent).unwrap();
    (0..ticks)
        .map(|_| {
            let _ = tick(&mut registry, &world, None, environment, DT);
            agent_pose(&registry, id)
        })
        .collect()
}

// M17: agents ignore push.
#[test]
fn agent_in_push_volume_walks_its_path_unaffected() {
    // Sideways and upward, stronger than gravity: a player here would lift off
    // and drift; an agent resolves gravity only.
    let volumes = EnvironmentVolumes::build(vec![EnvironmentVolume {
        gravity: None,
        push: Some(Vec3::new(8.0, 30.0, 0.0)),
        ..gravity_volume(Vec3::splat(-50.0), Vec3::splat(50.0), 0.0)
    }]);
    let pushed = walk_path(EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]), 90);
    let calm = walk_path(EnvironmentView::level_only(LEVEL_GRAVITY), 90);

    for ((pushed_position, pushed_grounded), (calm_position, _)) in pushed.iter().zip(&calm) {
        assert!(
            *pushed_grounded,
            "the push never lifts the agent off the floor"
        );
        assert!(
            (*pushed_position - *calm_position).length() < 1.0e-6,
            "the push leaves the agent's path unchanged: {pushed_position:?} vs {calm_position:?}"
        );
    }
    let (end, _) = pushed.last().copied().unwrap();
    assert!(
        end.x > -5.0,
        "fixture: the agent walked its path, ended at {end:?}"
    );
}

// M17: upward gravity never lifts a grounded agent.
#[test]
fn grounded_agent_in_upward_gravity_volume_stays_grounded() {
    let volumes = EnvironmentVolumes::build(vec![gravity_volume(
        Vec3::splat(-50.0),
        Vec3::splat(50.0),
        5.0,
    )]);
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    let world = floor_world();
    let mut registry = EntityRegistry::new();
    let (id, rest_y) = spawn_resting_agent(&mut registry, 0.0, 0.0);
    for _ in 0..30 {
        let _ = tick(&mut registry, &world, None, environment, DT);
        let (position, grounded) = agent_pose(&registry, id);
        assert!(grounded, "upward gravity never lifts a grounded agent");
        assert!(
            (position.y - rest_y).abs() < 1.0e-4,
            "the agent stays on the floor: y={} rest={rest_y}",
            position.y
        );
    }
}

#[test]
fn environment_queries_perform_no_allocation() {
    // The resolve runs per body per tick, and the eye and crossing queries per
    // frame and per shot: none may touch the heap, whether a query meets
    // overlapping volumes, a multi-brush fluid, a single volume, or none. A few
    // hundred small volumes give the tree many levels to walk.
    let mut list = vec![
        gravity_volume(Vec3::splat(-50.0), Vec3::splat(50.0), VOLUME_GRAVITY),
        gravity_volume(Vec3::ZERO, Vec3::splat(4.0), 0.0),
        EnvironmentVolume {
            push: Some(Vec3::new(8.0, 0.0, 0.0)),
            gravity: None,
            ..gravity_volume(Vec3::splat(1.0), Vec3::splat(2.0), 0.0)
        },
        EnvironmentVolume {
            gravity: None,
            fluid: Some(0),
            brushes: vec![
                volume_box(Vec3::new(-10.0, -2.0, -10.0), Vec3::new(0.0, 2.5, 10.0)),
                volume_box(Vec3::new(0.0, -2.0, -10.0), Vec3::new(10.0, 2.5, 10.0)),
                volume_box(Vec3::new(-3.0, 1.0, -3.0), Vec3::new(3.0, 4.0, 3.0)),
            ],
            ..gravity_volume(Vec3::ZERO, Vec3::ONE, 0.0)
        },
    ];
    for i in 0..300 {
        let corner = Vec3::new(
            (i % 20) as f32 * 2.0 - 20.0,
            6.0,
            (i / 20) as f32 * 2.0 - 15.0,
        );
        let mut small = gravity_volume(corner, corner + Vec3::splat(1.5), -1.0);
        if i % 3 == 0 {
            small.gravity = None;
            small.fluid = Some(1);
        }
        list.push(small);
    }
    let volumes = EnvironmentVolumes::build(list);
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes, &[]);
    let empty = EnvironmentVolumes::empty();
    let empty_view = EnvironmentView::new(LEVEL_GRAVITY, &empty, &[]);
    let points = [
        Vec3::splat(1.5),
        Vec3::splat(3.0),
        Vec3::new(0.0, 2.0, 0.0),
        Vec3::new(-6.0, 6.5, -6.0),
        Vec3::splat(20.0),
        Vec3::splat(80.0),
    ];
    let probe = crate::alloc_probe::AllocSnapshot::arm();
    let mut checksum = 0.0;
    for view in [environment, empty_view] {
        for point in points {
            let resolved = postretro_collision::resolve_environment(
                view,
                &postretro_collision::BodyProbe {
                    position: point,
                    half_height: 0.55,
                    radius: 0.35,
                    eye_height: 0.5,
                },
            );
            checksum += resolved.gravity + resolved.push.x + resolved.immersion;
            checksum += postretro_collision::resolve_gravity(view, point);
            if let Some(eye) = postretro_collision::fluid_at_eye(view, point) {
                checksum += eye.signed_depth;
                checksum += postretro_collision::fluid_volume_signed_depth(view, eye.volume, point)
                    .unwrap_or(0.0);
            }
            postretro_collision::visit_fluid_crossings(
                view,
                point + Vec3::new(-30.0, 3.0, 0.5),
                Vec3::new(1.0, -0.1, 0.0),
                80.0,
                |crossing| checksum += crossing.t,
            );
        }
    }
    assert_eq!(probe.allocs_since(), 0, "an environment query allocated");
    assert!(checksum.is_finite());
}
