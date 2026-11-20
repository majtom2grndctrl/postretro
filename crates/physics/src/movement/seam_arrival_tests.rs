// Seam arrival proofs: landmark-relative capture and apply equal a seamless teleport.
// See: context/lib/entity_model.md §7e · context/lib/movement.md §6 (Teleport)

use super::*;
use crate::collision::CollisionWorld;
use crate::movement::tests::{canonical_descriptor, idle_input};
use crate::movement::{MovementEvents, teleport};
use crate::teleporters::Teleporters;
use postretro_foundation::{GroundRef, MovementStateKind};
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::teleporters::{TeleportMode, TeleporterRecord, TeleportersSection};

fn brush(min: Vec3, max: Vec3) -> EnvironmentBrush {
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
    EnvironmentBrush {
        aabb_min: min.to_array(),
        aabb_max: max.to_array(),
        planes,
    }
}

/// A seamless teleporter around the source landmark that maps it onto the
/// destination landmark: the teleport the seam arrival must equal.
fn seamless_world(source: Landmark, destination: Landmark) -> CollisionWorld {
    let section = TeleportersSection {
        teleporters: vec![TeleporterRecord {
            source_entity: 1,
            mode: TeleportMode::Seamless,
            enabled_on_spawn: true,
            brushes: vec![brush(
                source.origin - Vec3::splat(4.0),
                source.origin + Vec3::splat(4.0),
            )],
            destination_origin: destination.origin.to_array(),
            destination_yaw: destination.yaw,
            landmark_origin: source.origin.to_array(),
            landmark_yaw: source.yaw,
            arrival_sound: String::new(),
            on_teleport: String::new(),
        }],
    };
    let mut world = CollisionWorld::new();
    world.teleporters =
        Teleporters::from_section(&TeleportersSection::from_bytes(&section.to_bytes()).unwrap());
    world
}

fn pawn() -> PlayerMovementComponent {
    PlayerMovementComponent::from_descriptor(&canonical_descriptor())
}

fn moving_pawn(state: MovementState) -> PlayerMovementComponent {
    let mut pawn = pawn();
    pawn.velocity = Vec3::new(7.0, 3.0, -5.0);
    pawn.knockback_velocity = Vec3::new(2.0, 1.0, 0.0);
    pawn.push_velocity = Vec3::new(0.0, 0.0, -1.0);
    pawn.movement_state = state;
    pawn
}

fn near(a: Vec3, b: Vec3) {
    assert!((a - b).length() < 1.0e-4, "{a:?} != {b:?}");
}

fn near_yaw(a: f32, b: f32) {
    assert!(
        wrap_teleport_yaw(a - b).abs() < 1.0e-4,
        "yaw {a} != {b} (mod 2π)"
    );
}

fn boost(state: &MovementState) -> Vec3 {
    match state {
        MovementState::Dash { boost, .. } | MovementState::Sliding { boost, .. } => *boost,
        other => panic!("no boost on {other:?}"),
    }
}

fn elapsed(state: &MovementState) -> f32 {
    match state {
        MovementState::Dash { elapsed_ms, .. } | MovementState::Sliding { elapsed_ms, .. } => {
            *elapsed_ms
        }
        other => panic!("no clock on {other:?}"),
    }
}

#[test]
fn seam_arrival_equals_a_seamless_teleport_between_the_same_landmarks() {
    let source = Landmark {
        origin: Vec3::new(4.0, 1.0, -6.0),
        yaw: 0.3,
    };
    let body = source.origin + Vec3::new(1.0, 1.5, -0.5);
    let (view_yaw, view_pitch) = (1.1, -0.4);
    for destination_yaw in [0.0, std::f32::consts::FRAC_PI_2, std::f32::consts::PI, -2.0] {
        let destination = Landmark {
            origin: Vec3::new(-30.0, 9.0, 12.0),
            yaw: destination_yaw,
        };
        let states = [
            MovementState::Normal,
            MovementState::Dash {
                elapsed_ms: 40.0,
                boost: Vec3::new(6.0, 0.0, 2.0),
            },
            MovementState::Sliding {
                elapsed_ms: 120.0,
                boost: Vec3::new(-3.0, 0.0, 5.0),
                eye_current: 0.9,
            },
        ];
        for state in states {
            // The teleport: the pawn's own body point through the seamless mapping.
            let world = seamless_world(source, destination);
            let collision = CombinedCollisionWorld::static_only(&world);
            let mut teleported = moving_pawn(state);
            let mut events = MovementEvents::default();
            let mut input = idle_input();
            input.facing_yaw = view_yaw;
            let teleport_body = teleport::resolve(
                &mut teleported,
                &input,
                &collision,
                body,
                &mut events,
                |_| true,
            );
            let yaw_delta = events
                .teleport
                .expect("the seamless teleport fires")
                .yaw_delta;

            // The seam: capture before unload, apply to a fresh spawn.
            let crossing = moving_pawn(state);
            let arrival = LandmarkRelative::capture(&crossing, body, view_yaw, view_pitch, source);
            let empty = CollisionWorld::new();
            let mut spawned = pawn();
            let (pose, edge) = apply_seam_arrival(
                &mut spawned,
                &CombinedCollisionWorld::static_only(&empty),
                &arrival,
                destination,
                |_| true,
            )
            .expect("an open world fits the arrival");

            assert!(edge.is_none());
            near(pose.body, teleport_body);
            near_yaw(pose.view_yaw, view_yaw + yaw_delta);
            assert_eq!(pose.view_pitch, view_pitch, "a landmark turns only yaw");
            near(spawned.velocity, teleported.velocity);
            near(spawned.knockback_velocity, teleported.knockback_velocity);
            near(spawned.push_velocity, teleported.push_velocity);
            assert_eq!(
                spawned.movement_state.kind(),
                teleported.movement_state.kind()
            );
            if !matches!(state, MovementState::Normal) {
                near(
                    boost(&spawned.movement_state),
                    boost(&teleported.movement_state),
                );
                assert_eq!(
                    elapsed(&spawned.movement_state),
                    elapsed(&state),
                    "a dash or slide resumes rather than restarts"
                );
            }
            // The vertical offset survives: a landmark turns about world up.
            assert!(((pose.body.y - destination.origin.y) - 1.5).abs() < 1.0e-4);
        }
    }
}

#[test]
fn seam_arrival_aborts_a_vault_to_normal_and_keeps_its_velocity() {
    let source = Landmark {
        origin: Vec3::ZERO,
        yaw: 0.0,
    };
    let destination = Landmark {
        origin: Vec3::new(10.0, 0.0, 0.0),
        yaw: std::f32::consts::FRAC_PI_2,
    };
    let crossing = moving_pawn(MovementState::Vault {
        elapsed_ms: 50.0,
        target: Vec3::new(0.0, 1.0, -1.0),
        start_offset: Vec3::new(0.0, -1.0, 1.0),
        mover: None,
        entry_speed: 4.0,
    });
    let arrival = LandmarkRelative::capture(&crossing, Vec3::Y, 0.0, 0.0, source);
    let world = CollisionWorld::new();
    let mut spawned = pawn();
    let (_, edge) = apply_seam_arrival(
        &mut spawned,
        &CombinedCollisionWorld::static_only(&world),
        &arrival,
        destination,
        |_| true,
    )
    .unwrap();
    assert_eq!(spawned.movement_state, MovementState::Normal);
    assert_eq!(
        edge.map(|edge| (edge.from, edge.to)),
        Some((MovementStateKind::Vault, MovementStateKind::Normal))
    );
    near(
        spawned.velocity,
        Quat::from_rotation_y(std::f32::consts::FRAC_PI_2) * crossing.velocity,
    );
}

#[test]
fn seam_arrival_without_a_landmark_rotation_keeps_world_motion() {
    // A seam with no authored landmark uses its bounds centre at yaw 0 in both
    // maps: the arrival is a pure translation.
    let centre = Landmark {
        origin: Vec3::new(3.0, 2.0, 1.0),
        yaw: 0.0,
    };
    let crossing = moving_pawn(MovementState::Normal);
    let body = centre.origin + Vec3::new(0.5, 0.25, -1.0);
    let arrival = LandmarkRelative::capture(&crossing, body, 0.7, 0.1, centre);
    let world = CollisionWorld::new();
    let mut spawned = pawn();
    let (pose, _) = apply_seam_arrival(
        &mut spawned,
        &CombinedCollisionWorld::static_only(&world),
        &arrival,
        centre,
        |_| true,
    )
    .unwrap();
    near(pose.body, body);
    near_yaw(pose.view_yaw, 0.7);
    near(spawned.velocity, crossing.velocity);
}

#[test]
fn seam_arrival_blocked_by_static_world_changes_nothing() {
    let world = CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(20.0, -10.0, -10.0),
            Vec3::new(20.0, 10.0, -10.0),
            Vec3::new(20.0, 0.0, 10.0),
        ],
        vec![[0, 1, 2]],
    );
    let arrival = LandmarkRelative::at_rest(Vec3::ZERO, 0.0, 0.0);
    let destination = Landmark {
        origin: Vec3::new(20.0, 0.0, 0.0),
        yaw: 0.0,
    };
    let mut spawned = pawn();
    spawned.velocity = Vec3::X;
    let before = spawned.clone();
    assert_eq!(
        apply_seam_arrival(
            &mut spawned,
            &CombinedCollisionWorld::static_only(&world),
            &arrival,
            destination,
            |_| true,
        ),
        Err(SeamArrivalRefusal::Blocked)
    );
    assert_eq!(spawned, before);
}

#[test]
fn seam_arrival_latches_armed_teleporters_and_settles_at_rest() {
    let landmark = Landmark {
        origin: Vec3::ZERO,
        yaw: 0.0,
    };
    // A teleporter around the arrival point in the destination map.
    let world = seamless_world(
        landmark,
        Landmark {
            origin: Vec3::new(50.0, 0.0, 0.0),
            yaw: 0.0,
        },
    );
    let arrival = LandmarkRelative::at_rest(Vec3::new(0.5, 1.0, 0.0), 0.2, 0.0);
    let mut spawned = pawn();
    apply_seam_arrival(
        &mut spawned,
        &CombinedCollisionWorld::static_only(&world),
        &arrival,
        landmark,
        |_| true,
    )
    .unwrap();
    assert_eq!(spawned.teleport.arrival_latches, [0]);
    assert_eq!(spawned.velocity, Vec3::ZERO);
    assert_eq!(spawned.movement_state, MovementState::Normal);
    assert_eq!(spawned.ground, GroundRef::Airborne);
}

// Regression: Continue from a seam crossed crouched placed the standing
// capsule at the crouched centre, inside the floor, and fell back to a start.
#[test]
fn seam_rest_offset_stands_a_crouched_crossing_on_its_feet() {
    let landmark = Landmark {
        origin: Vec3::ZERO,
        yaw: 0.0,
    };
    let mut crouched = pawn();
    let standing_half_height = crouched.standing_half_height;
    crouched.capsule.half_height = standing_half_height * 0.5;
    crouched.movement_state = MovementState::Crouching { eye_current: 0.5 };
    let body = Vec3::new(
        1.0,
        crouched.capsule.half_height + crouched.capsule.radius,
        0.0,
    );
    let arrival = LandmarkRelative::capture(&crouched, body, 0.0, 0.0, landmark);
    assert_eq!(arrival.offset, body);
    assert!(
        (arrival.rest_offset.y - (standing_half_height + crouched.capsule.radius)).abs() < 1.0e-5,
        "the rest pose keeps the feet and stands up: {:?}",
        arrival.rest_offset
    );
    let standing = LandmarkRelative::capture(&pawn(), body, 0.0, 0.0, landmark);
    assert_eq!(standing.rest_offset, standing.offset);
}

// Regression: a forced-crouch swimmer keeps a crouch-sized capsule in `Swim`,
// so "crouched" reads the capsule, not the state.
#[test]
fn seam_crossing_with_a_shrunk_capsule_in_swim_arrives_crouched() {
    let landmark = Landmark {
        origin: Vec3::ZERO,
        yaw: 0.0,
    };
    let mut swimmer = pawn();
    let standing_half_height = swimmer.standing_half_height;
    swimmer.capsule.half_height = standing_half_height * 0.5;
    swimmer.movement_state = MovementState::Swim { fluid: 3 };
    let body = Vec3::new(0.0, 2.0, 0.0);
    let arrival = LandmarkRelative::capture(&swimmer, body, 0.0, 0.0, landmark);
    assert_eq!(
        arrival.crouched_capsule.as_ref().map(|c| c.half_height),
        Some(standing_half_height * 0.5)
    );
    assert!(
        (arrival.rest_offset.y - (body.y + standing_half_height * 0.5)).abs() < 1.0e-5,
        "{:?}",
        arrival.rest_offset
    );
    let world = CollisionWorld::new();
    let mut spawned = pawn();
    apply_seam_arrival(
        &mut spawned,
        &CombinedCollisionWorld::static_only(&world),
        &arrival,
        landmark,
        |_| true,
    )
    .unwrap();
    assert_eq!(spawned.capsule.half_height, standing_half_height * 0.5);
    assert!(
        matches!(spawned.movement_state, MovementState::Crouching { .. }),
        "a shrunk capsule lands in the state that retries the stand-up: {:?}",
        spawned.movement_state
    );
}
