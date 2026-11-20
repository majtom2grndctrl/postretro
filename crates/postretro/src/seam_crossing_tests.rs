// Seam crossing proofs: who the harvest reads, from which view, and how a spawn
// takes or falls back from its arrival.
// See: context/lib/entity_model.md §7e

use super::*;
use postretro_entities::components::health::HealthComponent;
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::seams::{SeamActivation, SeamMode, SeamRecord, SeamsSection};
use postretro_physics::collision::CollisionWorld;
use postretro_test_log_capture::LogCapture;

fn box_brush(min: Vec3, max: Vec3) -> EnvironmentBrush {
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

fn stage(name: &str, landmark: Vec3, yaw: f32) -> SeamStage {
    SeamStage::install(
        &SeamsSection {
            seams: vec![SeamRecord {
                source_entity: 1,
                name: name.into(),
                target: "next".into(),
                activation: SeamActivation::Touch,
                mode: SeamMode::Classic,
                enabled_on_spawn: true,
                brushes: vec![box_brush(
                    landmark - Vec3::new(4.0, 0.0, 4.0),
                    landmark + Vec3::new(4.0, 4.0, 4.0),
                )],
                landmark_origin: landmark.to_array(),
                landmark_yaw: yaw,
                on_arrive: String::new(),
            }],
        },
        false,
    )
}

fn movement() -> PlayerMovementComponent {
    PlayerMovementComponent::from_descriptor(&crate::mod_digest::tests::movement_descriptor())
}

fn spawn_pawn(registry: &mut EntityRegistry, seat: Seat, position: Vec3, yaw: f32) -> EntityId {
    let pawn = registry.spawn(Transform {
        position,
        rotation: Quat::from_rotation_y(yaw),
        scale: Vec3::ONE,
    });
    let mut movement = movement();
    movement.velocity = Vec3::new(2.0, 0.0, -1.0);
    registry.set_component(pawn, movement).unwrap();
    registry.bind_pawn_seat(pawn, seat);
    pawn
}

#[test]
fn seam_harvest_reads_seats_inside_with_their_own_view_source() {
    let stage = stage("door", Vec3::ZERO, 0.0);
    let mut registry = EntityRegistry::new();
    let host = spawn_pawn(&mut registry, Seat(0), Vec3::new(1.0, 1.5, 0.0), 0.0);
    let _ = registry.mark_local_player_pawn(host);
    let client = spawn_pawn(&mut registry, Seat(1), Vec3::new(-1.0, 1.5, 0.0), 0.0);
    spawn_pawn(&mut registry, Seat(2), Vec3::new(0.0, 1.5, 1.0), 0.7);
    let outside = spawn_pawn(&mut registry, Seat(3), Vec3::new(40.0, 1.5, 0.0), 0.0);
    // A downed pawn still crosses: the seam never changes life or health.
    let mut health = HealthComponent::from_descriptor(&postretro_entities::HealthDescriptor {
        max: 100.0,
        ..Default::default()
    });
    health.current = 0.0;
    registry.set_component(client, health).unwrap();
    let _ = outside;

    let crossing = harvest_crossing(
        "door",
        &stage,
        &registry,
        &[Seat(0), Seat(1), Seat(2), Seat(3), Seat(4)],
        (0.5, -0.2),
        |seat, _| (seat == Seat(1)).then_some((1.25, 0.3)),
    );
    assert_eq!(crossing.seam, "door");
    assert_eq!(
        crossing.arrivals.keys().copied().collect::<Vec<_>>(),
        vec![Seat(0), Seat(1), Seat(2)],
        "the seat outside, and the seat with no bound pawn, get no arrival"
    );
    let host_arrival = &crossing.arrivals[&Seat(0)];
    assert_eq!(
        (host_arrival.view_yaw, host_arrival.view_pitch),
        (0.5, -0.2),
        "the host presents its camera"
    );
    assert!((host_arrival.velocity - Vec3::new(2.0, 0.0, -1.0)).length() < 1.0e-5);
    let client_arrival = &crossing.arrivals[&Seat(1)];
    assert_eq!(
        (client_arrival.view_yaw, client_arrival.view_pitch),
        (1.25, 0.3),
        "a client faces as its host-side pawn faces"
    );
    let silent_arrival = &crossing.arrivals[&Seat(2)];
    assert!((silent_arrival.view_yaw - 0.7).abs() < 1.0e-5);
    assert_eq!(
        silent_arrival.view_pitch, 0.0,
        "no resolved command: the pawn's yaw at pitch 0"
    );
}

#[test]
fn seam_harvest_of_an_unknown_seam_writes_no_arrival() {
    let stage = stage("door", Vec3::ZERO, 0.0);
    let mut registry = EntityRegistry::new();
    spawn_pawn(&mut registry, Seat(0), Vec3::new(1.0, 1.5, 0.0), 0.0);
    let crossing = harvest_crossing(
        "hatch",
        &stage,
        &registry,
        &[Seat(0)],
        (0.0, 0.0),
        |_, _| None,
    );
    assert!(crossing.arrivals.is_empty());
}

#[test]
fn seam_arrival_maps_through_the_destination_landmark_and_falls_back_with_one_warning() {
    let logs = LogCapture::start();
    let source = stage("door", Vec3::ZERO, 0.0);
    let destination = stage(
        "door",
        Vec3::new(50.0, 2.0, 0.0),
        std::f32::consts::FRAC_PI_2,
    );
    let mut registry = EntityRegistry::new();
    let crossing_pawn = spawn_pawn(&mut registry, Seat(0), Vec3::new(1.0, 1.5, 0.0), 0.0);
    let _ = registry.mark_local_player_pawn(crossing_pawn);
    let crossing = harvest_crossing(
        "door",
        &source,
        &registry,
        &[Seat(0)],
        (0.0, 0.0),
        |_, _| None,
    );
    let motion = crossing.arrivals[&Seat(0)].clone();

    let world = CollisionWorld::new();
    let collision = CombinedCollisionWorld::static_only(&world);
    let mut next = EntityRegistry::new();
    let spawned = spawn_pawn(&mut next, Seat(0), Vec3::new(-30.0, 1.0, 0.0), 0.0);
    let (index, pose) = arrive_pawn(
        &mut next,
        &collision,
        &destination,
        "door",
        spawned,
        &motion,
    )
    .unwrap();
    assert_eq!(index, 0);
    let expected = Vec3::new(50.0, 3.5, -1.0);
    assert!((pose.body - expected).length() < 1.0e-4, "{:?}", pose.body);
    assert!((pose.view_yaw - std::f32::consts::FRAC_PI_2).abs() < 1.0e-5);
    let transform = next.get_component::<Transform>(spawned).unwrap();
    assert!((transform.position - expected).length() < 1.0e-4);

    // No seam of the name: the pawn stays at its start.
    let other = stage("hatch", Vec3::ZERO, 0.0);
    let start = Vec3::new(-30.0, 1.0, 0.0);
    let mut fresh = EntityRegistry::new();
    let pawn = spawn_pawn(&mut fresh, Seat(0), start, 0.0);
    assert_eq!(
        arrive_pawn(&mut fresh, &collision, &other, "door", pawn, &motion),
        Err(ArrivalFallback::MissingSeam)
    );
    assert_eq!(
        fresh.get_component::<Transform>(pawn).unwrap().position,
        start
    );

    // A live pawn standing on the arrival blocks it.
    let mut crowded = EntityRegistry::new();
    spawn_pawn(&mut crowded, Seat(1), expected, 0.0);
    let pawn = spawn_pawn(&mut crowded, Seat(0), start, 0.0);
    assert_eq!(
        arrive_pawn(
            &mut crowded,
            &collision,
            &destination,
            "door",
            pawn,
            &motion
        ),
        Err(ArrivalFallback::Blocked)
    );
    assert_eq!(
        crowded.get_component::<Transform>(pawn).unwrap().position,
        start
    );
    warn_fallback("door", ArrivalFallback::Blocked, "seat 0");
    logs.assert_logged_once(log::Level::Warn, "seam `door` arrival blocked");
}
