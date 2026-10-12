// Agent gravity under environment volumes (per agent, tick-start position, downward part only) and the resolve's no-allocation guard.
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
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes);
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
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes);
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
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes);
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

#[test]
fn environment_resolve_performs_no_allocation() {
    // The resolve runs per body per tick: it must never touch the heap, whether
    // the point hits overlapping volumes, a single one, or none.
    let volumes = EnvironmentVolumes::build(vec![
        gravity_volume(Vec3::splat(-50.0), Vec3::splat(50.0), VOLUME_GRAVITY),
        gravity_volume(Vec3::ZERO, Vec3::splat(4.0), 0.0),
        EnvironmentVolume {
            push: Some(Vec3::new(8.0, 0.0, 0.0)),
            gravity: None,
            ..gravity_volume(Vec3::splat(1.0), Vec3::splat(2.0), 0.0)
        },
    ]);
    let environment = EnvironmentView::new(LEVEL_GRAVITY, &volumes);
    let points = [
        Vec3::splat(1.5),
        Vec3::splat(3.0),
        Vec3::splat(20.0),
        Vec3::splat(80.0),
    ];
    let probe = crate::alloc_probe::AllocSnapshot::arm();
    let mut checksum = 0.0;
    for point in points {
        let resolved = postretro_collision::resolve_environment(
            environment,
            &postretro_collision::BodyProbe {
                position: point,
                half_height: 0.55,
                radius: 0.35,
                eye_height: 0.0,
            },
        );
        checksum += resolved.gravity + resolved.push.x;
    }
    assert_eq!(probe.allocs_since(), 0, "resolve allocated");
    assert!(checksum.is_finite());
}
