// Fluid statuses through the fixed tick: a pawn whose body is in a status
// fluid at any depth takes its status every tick, as `fluid.<name>`; leaving
// leaves one lingering tick; dry pawns, statusless fluids and enemies take none.
// See: context/lib/scripting.md §2 (Fluid declarations, Status declarations)

use std::sync::Arc;

use glam::Vec3;
use postretro_collision::{ConvexLeaf, EnvironmentVolume, EnvironmentVolumes, LeafPlane};
use postretro_entities::components::health::HealthComponent;
use postretro_entities::{EntityId, Transform};
use postretro_foundation::ImmersionComponent;
use postretro_foundation::defense::DefenseVocabulary;
use postretro_foundation::status::parse_statuses;
use serde_json::json;

use super::status_tick_tests::{World, burning, spawn};

const SCALDING: u16 = 0;
/// A pawn's capsule centre when its feet rest at y = 0 (reach 1.2 m).
const STAND_Y: f32 = 1.2;
const IN_LAVA: Vec3 = Vec3::new(30.0, STAND_Y, 0.0);
const IN_WATER: Vec3 = Vec3::new(70.0, STAND_Y, 0.0);
const ON_SURFACE: Vec3 = Vec3::new(110.0, STAND_Y, 0.0);
const DRY: Vec3 = Vec3::new(-30.0, STAND_Y, 0.0);

fn volume(source_entity: u32, fluid: u16, min: Vec3, max: Vec3) -> EnvironmentVolume {
    let plane = |normal: Vec3, distance: f32| LeafPlane { normal, distance };
    EnvironmentVolume {
        source_entity,
        priority: 0,
        gravity: None,
        push: None,
        force: 1.0,
        fluid: Some(fluid),
        brushes: vec![ConvexLeaf {
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
        }],
    }
}

/// Ankle-deep lava (fluid 0) at x in [20, 40], ankle-deep water (fluid 1,
/// no status) at x in [60, 80], and lava whose surface is exactly at the
/// feet at x in [100, 120].
fn pools() -> EnvironmentVolumes {
    EnvironmentVolumes::build(vec![
        volume(
            1,
            0,
            Vec3::new(20.0, -1.0, -50.0),
            Vec3::new(40.0, 0.3, 50.0),
        ),
        volume(
            2,
            1,
            Vec3::new(60.0, -1.0, -50.0),
            Vec3::new(80.0, 0.3, 50.0),
        ),
        volume(
            3,
            0,
            Vec3::new(100.0, -1.0, -50.0),
            Vec3::new(120.0, 0.0, 50.0),
        ),
    ])
}

fn lava_world() -> World {
    let mut world = World::new(burning("refresh", 500.0, None));
    {
        let mut registry = world.registry();
        let mut vocabulary: DefenseVocabulary = (**registry.defense_vocabulary()).clone();
        vocabulary.statuses = parse_statuses(
            Some(&json!([{ "kind": "status", "name": "scalding", "damage": 10, "tickMs": 250, "durationMs": 250 }])),
            |_| None,
        )
        .unwrap();
        registry.set_defense_vocabulary(Arc::new(vocabulary));
        registry.set_fluid_statuses([("lava", Some("scalding")), ("water", None)]);
    }
    world.rig.environment = Some((pools(), vec![2.0, 2.0]));
    world
}

fn health(world: &World, entity: EntityId) -> f32 {
    world
        .registry()
        .get_component::<HealthComponent>(entity)
        .unwrap()
        .current
}

/// Ticks `n` times, returning the host ticks on which `pawn` lost health.
fn scald_ticks(world: &mut World, pawn: EntityId, n: usize) -> Vec<u64> {
    let mut landed = Vec::new();
    for _ in 0..n {
        let before = health(world, pawn);
        world.tick();
        if health(world, pawn) < before {
            landed.push(world.host_tick());
        }
    }
    landed
}

// P4: an ankle-deep pawn takes `scalding` ticks; after leaving, exactly one
// more lands within `tickMs`, then none; feet exactly on the surface, a
// statusless fluid and an enemy in lava take none; re-entering on a
// lingering instance's last tick keeps it and its phase.
#[test]
fn fluid_status_applies_to_wading_pawn_and_lingers_one_tick() {
    let mut world = lava_world();
    let pawn = world.rig.local_pawn(IN_LAVA);
    let enemy = spawn(&world.rig, 100.0);
    world
        .registry()
        .set_component(
            enemy,
            Transform {
                position: IN_LAVA,
                ..Transform::default()
            },
        )
        .unwrap();
    world.tick();
    let entered = world.host_tick();
    let immersion = *world
        .registry()
        .get_component::<ImmersionComponent>(pawn)
        .unwrap();
    assert_eq!(immersion.fluid, Some(0));
    assert!(
        immersion.immersion > 0.0 && immersion.immersion < 0.2,
        "ankle-deep"
    );
    let landed = scald_ticks(&mut world, pawn, 45);
    assert_eq!(landed, vec![entered + 15, entered + 30, entered + 45]);
    assert_eq!(health(&world, enemy), 100.0, "enemies resolve no fluid");

    // Leave: exactly one more tick within `tickMs`, then none.
    world.rig.place(pawn, DRY);
    let left = world.host_tick();
    let landed = scald_ticks(&mut world, pawn, 60);
    assert_eq!(landed.len(), 1);
    assert!(
        landed[0] > left && landed[0] <= left + 15,
        "{landed:?} after {left}"
    );
    assert!(!world.registry().has_status(pawn, SCALDING));

    // Statusless water and lava exactly at the feet take none.
    for position in [IN_WATER, ON_SURFACE] {
        world.rig.place(pawn, position);
        assert!(scald_ticks(&mut world, pawn, 40).is_empty(), "{position}");
    }

    // Re-enter on the lingering instance's last tick: kept, phase unmoved.
    world.rig.place(pawn, IN_LAVA);
    world.tick();
    let instance = *world.registry().statuses().sets()[0]
        .instances()
        .next()
        .unwrap();
    world.rig.place(pawn, DRY);
    while world.host_tick() + 1 < instance.expires.unwrap() {
        world.tick();
    }
    world.rig.place(pawn, IN_LAVA);
    world.tick();
    let kept = *world.registry().statuses().sets()[0]
        .instances()
        .next()
        .unwrap();
    assert_eq!(kept.serial, instance.serial, "the same instance");
    assert_eq!(kept.next_due, instance.next_due + 15, "its phase held");
}

// P17: a pawn whose movement did not step (a remote pawn with no command)
// keeps its last resolve and keeps taking the fluid's status.
#[test]
fn stalled_pawn_keeps_fluid_status() {
    let mut world = lava_world();
    let stalled = spawn(&world.rig, 1000.0);
    world
        .registry()
        .set_component(
            stalled,
            ImmersionComponent {
                immersion: 0.4,
                fluid: Some(0),
            },
        )
        .unwrap();
    // Applied on the first tick, so 31 ticks reach its second due tick.
    let landed = scald_ticks(&mut world, stalled, 31);
    assert_eq!(
        landed.len(),
        2,
        "every tickMs for 30 ticks without a resolve"
    );
}
