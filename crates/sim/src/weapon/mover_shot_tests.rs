// Hitscan against movers at their pose: a mover contact is a world contact.
// See: context/lib/networking.md §Combat authority · context/lib/entity_model.md §7c
use super::tests::{
    assert_vec3_approx, only_impact, spawn_hitbox_entity, wall_world, weapon_component,
};
use super::*;
use crate::collision::CollisionWorld;
use crate::collision::moving::{CombinedCollisionWorld, MoverCollider};
use crate::collision::test_movers::{DoorPoses, door_slab};
use glam::Quat;
use postretro_foundation::FireMode;

const DOOR: u32 = 3;
/// The door's near face sits at x = 4.75, exact in binary so a tie is a tie.
const DOOR_X: f32 = 5.0;
const HALF_THICKNESS: f32 = 0.25;

fn door() -> [MoverCollider; 1] {
    [door_slab(DOOR, HALF_THICKNESS, 3.0, 2.0)]
}

fn closed() -> DoorPoses {
    DoorPoses::default().place(DOOR, Vec3::new(DOOR_X, -1.0, 0.0), Quat::IDENTITY)
}

fn raised() -> DoorPoses {
    DoorPoses::default().place(DOOR, Vec3::new(DOOR_X, 4.0, 0.0), Quat::IDENTITY)
}

/// Fire one exact-axis hitscan pellet along +X from the origin.
fn fire_along_x(
    registry: &mut EntityRegistry,
    world: &CombinedCollisionWorld<'_>,
) -> WeaponFireEvents {
    let mut weapon = weapon_component(FireMode::Semi, 0.0);
    tick_resolved_component(
        registry,
        None,
        &mut weapon,
        "weapon.test.mover",
        0,
        &WeaponFireCommand {
            button: FireButtonState {
                pressed: true,
                active: true,
            },
            aim_origin: Vec3::ZERO,
            aim_direction: Vec3::X,
            can_fire: true,
        },
        &WeaponPlacementDescriptor::default(),
        world,
        EnvironmentView::level_only(0.0),
        &HitZoneStore::new(),
        0.0,
        WeaponFireAuthorization::Accepted,
    )
}

#[test]
fn hitscan_stops_at_a_closed_mover_with_a_world_contact() {
    let mut registry = EntityRegistry::new();
    spawn_hitbox_entity(
        &mut registry,
        Vec3::new(8.0, 0.0, 0.0),
        Vec3::splat(0.5),
        Vec3::ZERO,
    );
    let static_world = CollisionWorld::new();
    let movers = door();
    let poses = closed();

    let events = fire_along_x(
        &mut registry,
        &CombinedCollisionWorld::new(&static_world, &movers, &poses),
    );

    let impact = only_impact(&events);
    assert_eq!(impact.target, None, "the target behind the door is not hit");
    assert_vec3_approx(impact.point, Vec3::new(DOOR_X - HALF_THICKNESS, 0.0, 0.0));
    assert_vec3_approx(impact.normal, Vec3::NEG_X);
}

#[test]
fn hitscan_reaches_the_target_through_an_open_gap() {
    let mut registry = EntityRegistry::new();
    let target = spawn_hitbox_entity(
        &mut registry,
        Vec3::new(8.0, 0.0, 0.0),
        Vec3::splat(0.5),
        Vec3::ZERO,
    );
    let static_world = CollisionWorld::new();
    let movers = door();
    let poses = raised();

    let events = fire_along_x(
        &mut registry,
        &CombinedCollisionWorld::new(&static_world, &movers, &poses),
    );

    assert_eq!(only_impact(&events).target, Some(target));
}

#[test]
fn hitscan_tie_between_a_mover_and_an_entity_gives_the_world_contact() {
    let mut registry = EntityRegistry::new();
    // The hitbox's near face meets the door's near face at x = 4.75.
    spawn_hitbox_entity(
        &mut registry,
        Vec3::new(DOOR_X - HALF_THICKNESS + 0.5, 0.0, 0.0),
        Vec3::splat(0.5),
        Vec3::ZERO,
    );
    let static_world = CollisionWorld::new();
    let movers = door();
    let poses = closed();

    let events = fire_along_x(
        &mut registry,
        &CombinedCollisionWorld::new(&static_world, &movers, &poses),
    );

    let impact = only_impact(&events);
    assert_eq!(impact.target, None, "world wins the tie, as a wall does");
    assert_vec3_approx(impact.point, Vec3::new(DOOR_X - HALF_THICKNESS, 0.0, 0.0));
}

/// A door across the -Z aim, thin along Z, its near face at `z + 0.05`.
fn door_across_z(z: f32) -> DoorPoses {
    DoorPoses::default().place(
        DOOR,
        Vec3::new(0.0, -1.0, z),
        Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
    )
}

fn thin_door() -> [MoverCollider; 1] {
    [door_slab(DOOR, 0.05, 3.0, 2.0)]
}

const MUZZLE: Vec3 = Vec3::new(0.5, 0.0, -0.8);

fn launch_pose(world: &CombinedCollisionWorld<'_>) -> (Vec3, Vec3) {
    resolve_projectile_launch_pose(
        None,
        Vec3::ZERO,
        Vec3::NEG_Z,
        &WeaponPlacementDescriptor::default(),
        Some(MUZZLE),
        0.0,
        world,
        &EntityRegistry::new(),
        &HitZoneStore::new(),
        0.0,
        10.0,
    )
}

#[test]
fn composed_muzzle_beyond_a_closed_mover_launches_from_the_eye() {
    let static_world = CollisionWorld::new();
    let movers = thin_door();
    let poses = door_across_z(-0.5);
    let (origin, direction) =
        launch_pose(&CombinedCollisionWorld::new(&static_world, &movers, &poses));
    assert_vec3_approx(origin, Vec3::ZERO);
    assert_vec3_approx(direction, Vec3::NEG_Z);
}

#[test]
fn muzzle_clear_of_a_closed_door_converges_on_the_door_not_the_wall_behind() {
    let static_world = wall_world();
    let movers = thin_door();
    let poses = door_across_z(-3.0);
    let (origin, direction) =
        launch_pose(&CombinedCollisionWorld::new(&static_world, &movers, &poses));
    assert_vec3_approx(origin, MUZZLE);
    let door_point = Vec3::new(0.0, 0.0, -2.95);
    assert_vec3_approx(direction, (door_point - MUZZLE).normalize());
}

#[test]
fn client_predicted_launch_reads_the_door_in_its_prediction() {
    let static_world = wall_world();
    let movers = thin_door();
    let launch = |poses: &DoorPoses| {
        let mut weapon = super::tests::projectile_weapon_component(Some(MUZZLE));
        resolve_test_client_shot(
            None,
            &mut weapon,
            "weapon.test.mover",
            0,
            FireButtonState {
                pressed: true,
                active: true,
            },
            Vec3::ZERO,
            Vec3::NEG_Z,
            &WeaponPlacementDescriptor::default(),
            Some(MUZZLE),
            1,
            &[],
            &[],
            &CombinedCollisionWorld::new(&static_world, &movers, poses),
            &EntityRegistry::new(),
            &HitZoneStore::new(),
            0.0,
            1.0 / 60.0,
        )
        .expect("client fires")
        .projectile_launch
        .expect("a projectile weapon launches")
    };

    let across = launch(&door_across_z(-0.5));
    assert_vec3_approx(across.origin, Vec3::ZERO);
    assert_vec3_approx(across.direction, Vec3::NEG_Z);

    let ahead = launch(&door_across_z(-3.0));
    assert_vec3_approx(ahead.origin, MUZZLE);
    assert_vec3_approx(
        ahead.direction,
        (Vec3::new(0.0, 0.0, -2.95) - MUZZLE).normalize(),
    );
}
