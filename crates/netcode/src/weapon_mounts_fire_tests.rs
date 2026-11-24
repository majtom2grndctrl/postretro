// Tests: a weapon mount is presentation only; fire origin and hits never
// read it.
// See: context/lib/networking.md §Weapon placement is content

use glam::Vec3;
use postretro_entities::Transform;
use postretro_entities::provenance::DescriptorSpawnPath;
use postretro_foundation::ResolutionMode;

use super::tests::{RIFLE_EXO, SLING, descriptors, mounted, pairs, spawn_pawn, store, weapon};
use super::*;

/// The shooter's eye, the camera a pawn's obstructed muzzle falls back to.
const EYE: Vec3 = Vec3::new(0.0, 1.5, 0.0);

/// A wall across the shot between the eye and the composed muzzle.
fn wall_at_the_muzzle() -> crate::collision::CollisionWorld {
    let points = vec![
        Vec3::new(-2.0, 0.0, -0.3),
        Vec3::new(2.0, 0.0, -0.3),
        Vec3::new(2.0, 3.0, -0.3),
        Vec3::new(-2.0, 3.0, -0.3),
    ];
    crate::collision::CollisionWorld::from_triangles_for_test(points, vec![[0, 1, 2], [0, 2, 3]])
}

/// Fire one shot from `shooter` along -Z at a hitbox target, in `world`, and
/// return the client resolution: its hits, world contacts and projectile
/// launch origin.
fn fire(
    registry: &EntityRegistry,
    shooter: EntityId,
    store: &HitZoneStore,
    resolution: ResolutionMode,
    world: &crate::collision::CollisionWorld,
) -> crate::weapon::ClientFireResolution {
    let descriptor = weapon(resolution);
    let placement = postretro_foundation::WeaponPlacementDescriptor {
        offset: postretro_foundation::PlacementOffset {
            right: 0.2,
            up: -0.2,
            forward: 0.4,
        },
        rotation: Default::default(),
    };
    let mut component =
        postretro_entities::components::weapon::WeaponComponent::from_descriptor(&descriptor);
    crate::weapon::resolve_test_client_shot(
        Some(shooter),
        &mut component,
        "rifle",
        0,
        crate::weapon::FireButtonState {
            pressed: true,
            active: true,
        },
        EYE,
        Vec3::NEG_Z,
        &placement,
        descriptor.muzzle_offset.map(Vec3::from),
        3,
        &[0.0],
        &[],
        &postretro_physics::collision::moving::CombinedCollisionWorld::static_only(world),
        registry,
        store,
        0.0,
        0.0,
    )
    .expect("a fresh press fires")
}

// E21, and E10 npc-weapon-mounts S6: a pawn's fire origin is identical with
// and without a `mounts` table, and a pawn whose muzzle is obstructed still
// launches from its eye with the table as without.
#[test]
fn fire_origin_and_hits_are_identical_with_and_without_a_weapon_mount_table() {
    let store = store();
    let mut results = Vec::new();
    for with_table in [true, false] {
        let mut descriptors = descriptors();
        if !with_table {
            for descriptor in &mut descriptors {
                if let Some(mesh) = descriptor.mesh.as_mut() {
                    mesh.mounts = None;
                }
            }
        }
        let mut registry = EntityRegistry::new();
        let (shooter, _) = spawn_pawn(
            &mut registry,
            &descriptors,
            "player",
            DescriptorSpawnPath::PlayerSpawn,
            &["rifle"],
            Some(0),
        );
        let (target, _) = spawn_pawn(
            &mut registry,
            &descriptors,
            "trooper",
            DescriptorSpawnPath::NetworkSlot,
            &["rifle"],
            Some(0),
        );
        registry
            .set_component(
                target,
                Transform {
                    position: Vec3::new(0.0, 1.0, -6.0),
                    ..Transform::default()
                },
            )
            .unwrap();
        registry
            .set_component(
                target,
                postretro_entities::components::health::HealthComponent::from_descriptor(
                    &postretro_entities::data_descriptors::HealthDescriptor {
                        max: 50.0,
                        hitbox: Some(postretro_entities::data_descriptors::HitboxDescriptor {
                            half_extents: [0.4, 1.0, 0.4],
                            offset: None,
                        }),
                        ..Default::default()
                    },
                ),
            )
            .unwrap();
        for pawn in [shooter, target] {
            synchronize_weapon_attachment_for_pawn(&mut registry, pawn, &descriptors, &store);
        }
        let open = crate::collision::CollisionWorld::new();
        let hitscan = fire(&registry, shooter, &store, ResolutionMode::Hitscan, &open);
        assert_eq!(hitscan.hits.len(), 1, "the hitscan shot hits the target");
        assert_eq!(hitscan.hits[0].target, target);
        let projectile = fire(
            &registry,
            shooter,
            &store,
            ResolutionMode::Projectile,
            &open,
        );
        let launch = projectile
            .projectile_launch
            .as_ref()
            .expect("a projectile launch");
        assert!(
            launch.origin.distance(EYE) > 0.1,
            "fixture: in the open the pawn launches from its composed muzzle: {}",
            launch.origin
        );
        let obstructed = fire(
            &registry,
            shooter,
            &store,
            ResolutionMode::Projectile,
            &wall_at_the_muzzle(),
        );
        let launch = obstructed
            .projectile_launch
            .as_ref()
            .expect("an obstructed projectile launch");
        assert!(
            launch.origin.distance(EYE) < 1.0e-4,
            "with_table {with_table}: an obstructed muzzle launches from the eye: {}",
            launch.origin
        );
        results.push((mounted(&registry, shooter), hitscan, projectile, obstructed));
    }
    let (with_table, without_table) = (&results[0], &results[1]);
    assert_eq!(
        with_table.0,
        pairs(&[("back", SLING), ("hand_r", RIFLE_EXO)])
    );
    assert_ne!(with_table.0, without_table.0, "fixture: the mounts differ");
    assert_eq!(
        (&with_table.1, &with_table.2, &with_table.3),
        (&without_table.1, &without_table.2, &without_table.3),
        "a mount is presentation only: fire origin and hits never read it"
    );
}
