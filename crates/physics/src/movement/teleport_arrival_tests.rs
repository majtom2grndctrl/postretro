//! Shared teleporter/respawn tail: exact arrival classification and silent cut metadata.
use super::*;
use crate::collision::CollisionWorld;
use crate::collision::moving::{MoverCollider, MoverPose, MoverPoseSource};
use crate::movement::tests::{canonical_descriptor, idle_input};
use crate::teleporters::Teleporters;
use postretro_foundation::NO_TELEPORTER;
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::teleporters::{TeleporterRecord, TeleportersSection};

fn pawn() -> PlayerMovementComponent {
    PlayerMovementComponent::from_descriptor(&canonical_descriptor())
}

fn arrival_gate(source: u32) -> TeleporterRecord {
    let min = Vec3::splat(-3.0);
    let max = Vec3::splat(3.0);
    let mut planes = Vec::new();
    for axis in 0..3 {
        let mut normal = Vec3::ZERO;
        normal[axis] = 1.0;
        planes.push(EnvironmentPlane {
            normal: normal.to_array(),
            distance: max[axis],
        });
        planes.push(EnvironmentPlane {
            normal: (-normal).to_array(),
            distance: -min[axis],
        });
    }
    TeleporterRecord {
        source_entity: source,
        mode: TeleportMode::Classic,
        // A current armed-state callback overrides the install-time default.
        enabled_on_spawn: false,
        brushes: vec![EnvironmentBrush {
            aabb_min: min.to_array(),
            aabb_max: max.to_array(),
            planes,
        }],
        destination_origin: [20.0, 4.0, 0.0],
        destination_yaw: 0.0,
        landmark_origin: [0.0; 3],
        landmark_yaw: 0.0,
        arrival_sound: "previous-gate-cue".into(),
        on_teleport: String::new(),
    }
}

#[test]
fn respawn_arrival_clears_previous_cue_identity_and_wraps_generation_yaw() {
    let world = CollisionWorld::new();
    let mut pawn = pawn();
    pawn.teleport.generation = u8::MAX;
    pawn.teleport.accumulated_yaw = std::f32::consts::PI - 0.1;
    pawn.teleport.last_teleporter = 7;
    pawn.teleport.arrival_latches = postretro_foundation::TeleportLatches::from_slice(&[7, 8]);
    pawn.ground = GroundRef::Mover(3);
    pawn.velocity = Vec3::new(3.0, -2.0, 4.0);
    pawn.knockback_velocity = Vec3::new(1.0, 0.0, 2.0);
    pawn.push_velocity = Vec3::new(0.5, 0.0, 0.25);
    let before = pawn.clone();
    settle_arrival(
        &mut pawn,
        &CombinedCollisionWorld::static_only(&world),
        Vec3::Y * 10.0,
        0.3,
        NO_TELEPORTER,
        |_| true,
    );
    assert_eq!(pawn.teleport.generation, 0);
    assert!((pawn.teleport.accumulated_yaw - (-std::f32::consts::PI + 0.2)).abs() < 1.0e-5);
    assert_eq!(
        pawn.teleport.last_teleporter, NO_TELEPORTER,
        "a prior teleporter cannot supply the respawn cue"
    );
    assert!(pawn.teleport.arrival_latches.is_empty());
    assert_eq!(pawn.ground, GroundRef::Airborne);
    // The shared tail classifies only: rotation/reset remains its caller's work.
    assert_eq!(pawn.velocity, before.velocity);
    assert_eq!(pawn.knockback_velocity, before.knockback_velocity);
    assert_eq!(pawn.push_velocity, before.push_velocity);
    assert_eq!(pawn.capsule, before.capsule);
}

#[test]
fn respawn_arrival_latches_newly_armed_regions_until_leave_and_reenter() {
    let section = TeleportersSection {
        teleporters: (1..=4).map(arrival_gate).collect(),
    };
    let mut world = CollisionWorld::new();
    world.teleporters =
        Teleporters::from_section(&TeleportersSection::from_bytes(&section.to_bytes()).unwrap());
    let mut pawn = pawn();
    pawn.teleport.arrival_latches = postretro_foundation::TeleportLatches::from_slice(&[42]);
    let armed = |index| index != 1;
    settle_arrival(
        &mut pawn,
        &CombinedCollisionWorld::static_only(&world),
        Vec3::ZERO,
        0.0,
        NO_TELEPORTER,
        armed,
    );
    assert_eq!(
        pawn.teleport.arrival_latches,
        [0, 2, 3],
        "current mutations, rather than enabled_on_spawn, supply the armed set"
    );
    let collision = CombinedCollisionWorld::static_only(&world);
    let mut events = MovementEvents::default();
    let input = idle_input();
    assert_eq!(
        resolve(
            &mut pawn,
            &input,
            &collision,
            Vec3::ZERO,
            &mut events,
            armed
        ),
        Vec3::ZERO
    );
    assert!(
        events.teleport.is_none(),
        "respawn inside a newly armed region cannot immediately teleport"
    );
    resolve(
        &mut pawn,
        &input,
        &collision,
        Vec3::X * 10.0,
        &mut events,
        armed,
    );
    assert!(pawn.teleport.arrival_latches.is_empty());
    let arrival = resolve(
        &mut pawn,
        &input,
        &collision,
        Vec3::ZERO,
        &mut events,
        armed,
    );
    assert_eq!(arrival, Vec3::new(20.0, 4.0, 0.0));
    assert!(
        events.teleport.is_some(),
        "leaving and reentering permits the ordinary teleporter event"
    );
    assert_eq!(pawn.teleport.generation, 2);
}

#[test]
fn respawn_arrival_reclassifies_static_ground_without_carrying_a_mover() {
    struct Platform;
    impl MoverPoseSource for Platform {
        fn pose(&self, id: u32) -> Option<MoverPose> {
            (id == 3).then_some(MoverPose {
                transform: postretro_entities::Transform {
                    position: Vec3::new(20.0, 5.0, 0.0),
                    ..Default::default()
                },
                linear_velocity: Vec3::X * 4.0,
                tick_delta: Vec3::X,
                angular_velocity: Vec3::ZERO,
                tick_rotation_delta: Quat::IDENTITY,
                carry_yaw: true,
                tick_dt: 1.0 / 60.0,
            })
        }
    }
    let floor = [
        Vec3::new(-5.0, 0.0, -5.0),
        Vec3::new(5.0, 0.0, -5.0),
        Vec3::new(5.0, 0.0, 5.0),
        Vec3::new(-5.0, 0.0, 5.0),
    ];
    let triangles = [[0, 2, 1], [0, 3, 2]];
    let world = CollisionWorld::from_triangles_for_test(floor.to_vec(), triangles.to_vec());
    let movers = [MoverCollider::from_local_triangles(3, &floor, &triangles).unwrap()];
    let collision = CombinedCollisionWorld::new(&world, &movers, &Platform);
    for (arrival, grounded) in [
        (Vec3::new(0.0, 1.22, 0.0), true),
        (Vec3::new(20.0, 6.22, 0.0), false),
    ] {
        let mut pawn = pawn();
        pawn.ground = GroundRef::Mover(3);
        pawn.last_floor_normal = Some(Vec3::X);
        pawn.air.jumps = 2;
        pawn.air_jumps_remaining = 1;
        pawn.air_ticks = 40;
        pawn.jump_spent = true;
        settle_arrival(&mut pawn, &collision, arrival, 0.0, NO_TELEPORTER, |_| true);
        if grounded {
            assert_eq!(pawn.ground, GroundRef::World);
            assert_eq!(pawn.last_floor_normal, Some(Vec3::Y));
            assert_eq!(pawn.air_ticks, 0);
            assert_eq!(pawn.air_jumps_remaining, 2);
            assert!(!pawn.jump_spent);
        } else {
            assert_eq!(
                pawn.ground,
                GroundRef::Airborne,
                "a destination mover supplies no arrival carry"
            );
            assert_eq!(pawn.last_floor_normal, None);
            assert_eq!(pawn.air_jumps_remaining, 1);
            assert!(pawn.jump_spent);
        }
    }
}
