// Relocation chokepoint proofs: placement, firing, latches, refusal and arrival state.
// See: context/lib/movement.md §6 (Teleport)

use super::*;
use crate::collision::CollisionWorld;
use crate::movement::tests::{canonical_descriptor, idle_input};
use crate::teleporters::Teleporters;
use postretro_level_format::environment_volumes::{EnvironmentBrush, EnvironmentPlane};
use postretro_level_format::teleporters::{TeleporterRecord, TeleportersSection};

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
fn record(mode: TeleportMode, origin: Vec3, yaw: f32) -> TeleporterRecord {
    TeleporterRecord {
        source_entity: 1,
        mode,
        enabled_on_spawn: true,
        brushes: vec![brush(Vec3::splat(-3.0), Vec3::splat(3.0))],
        destination_origin: origin.to_array(),
        destination_yaw: yaw,
        landmark_origin: Vec3::ZERO.to_array(),
        landmark_yaw: 0.0,
        arrival_sound: String::new(),
        on_teleport: String::new(),
    }
}
fn install(world: &mut CollisionWorld, records: Vec<TeleporterRecord>) {
    // Exercise the real PRL reader-to-query adapter instead of constructing
    // a second region representation in the test.
    let section = TeleportersSection {
        teleporters: records,
    };
    let section = TeleportersSection::from_bytes(&section.to_bytes()).unwrap();
    world.teleporters = Teleporters::from_section(&section);
}
fn pawn() -> PlayerMovementComponent {
    let mut pawn = PlayerMovementComponent::from_descriptor(&canonical_descriptor());
    pawn.velocity = Vec3::new(7.0, 3.0, -5.0);
    pawn.knockback_velocity = Vec3::new(2.0, 1.0, 0.0);
    pawn.push_velocity = Vec3::new(0.0, 0.0, -1.0);
    pawn
}
fn near(a: Vec3, b: Vec3) {
    assert!((a - b).length() < 0.0001, "{a:?} != {b:?}");
}
fn relocate(
    world: &CollisionWorld,
    pawn: &mut PlayerMovementComponent,
    position: Vec3,
    yaw: f32,
) -> (Vec3, MovementEvents) {
    let mut events = MovementEvents::default();
    let mut input = idle_input();
    input.facing_yaw = yaw;
    let next = resolve(
        pawn,
        &input,
        &CombinedCollisionWorld::static_only(world),
        position,
        &mut events,
        |_| true,
    );
    (next, events)
}

#[test]
fn teleport_classic_exact_arrival_rotates_all_velocity_channels() {
    let mut world = CollisionWorld::new();
    let destination = Vec3::new(20.0, 4.0, 10.0);
    let yaw = std::f32::consts::FRAC_PI_2;
    install(
        &mut world,
        vec![record(TeleportMode::Classic, destination, yaw)],
    );
    let mut pawn = pawn();
    pawn.ground = GroundRef::World;
    let before = pawn.clone();
    let (arrival, events) = relocate(&world, &mut pawn, Vec3::new(1.0, 2.0, 1.0), 0.25);
    near(arrival, destination);
    let event = events.teleport.unwrap();
    assert!((0.25 + event.yaw_delta - yaw).abs() < 0.0001);
    let turn = Quat::from_rotation_y(yaw - 0.25);
    near(pawn.velocity, turn * before.velocity);
    near(pawn.knockback_velocity, turn * before.knockback_velocity);
    near(pawn.push_velocity, turn * before.push_velocity);
    assert!((pawn.velocity.length() - before.velocity.length()).abs() < 0.0001);
    assert_eq!(pawn.ground, GroundRef::Airborne);
    assert_eq!(pawn.teleport.generation, 1);
    assert_eq!(pawn.teleport.last_teleporter, 0);
}

#[test]
fn teleport_seamless_landmark_transforms_vertical_offset_and_motion() {
    for yaw in [
        0.0,
        std::f32::consts::FRAC_PI_2,
        std::f32::consts::PI,
        -std::f32::consts::FRAC_PI_2,
    ] {
        let mut world = CollisionWorld::new();
        let destination = Vec3::new(20.0, 8.0, 10.0);
        install(
            &mut world,
            vec![record(TeleportMode::Seamless, destination, yaw)],
        );
        let mut pawn = pawn();
        let before = pawn.clone();
        let offset = Vec3::new(1.0, 2.0, -1.0);
        let (arrival, events) = relocate(&world, &mut pawn, offset, 0.4);
        let turn = Quat::from_rotation_y(yaw);
        near(arrival, destination + turn * offset);
        near(pawn.velocity, turn * before.velocity);
        near(pawn.knockback_velocity, turn * before.knockback_velocity);
        near(pawn.push_velocity, turn * before.push_velocity);
        assert!((events.teleport.unwrap().yaw_delta - yaw).abs() < 0.0001);
        assert!((arrival.y - (destination.y + offset.y)).abs() < 0.0001);
    }
}

#[test]
fn teleport_pair_round_trip_requires_exit_and_keeps_frame() {
    let mut world = CollisionWorld::new();
    let target = Vec3::new(20.0, 0.0, 0.0);
    let yaw = std::f32::consts::FRAC_PI_2;
    let forward = record(TeleportMode::Seamless, target, yaw);
    let mut back = record(TeleportMode::Seamless, Vec3::ZERO, 0.0);
    back.source_entity = 2;
    back.landmark_origin = target.to_array();
    back.landmark_yaw = yaw;
    back.brushes = vec![brush(target - Vec3::splat(3.0), target + Vec3::splat(3.0))];
    install(&mut world, vec![forward, back]);
    let mut pawn = pawn();
    let original = pawn.clone();
    let offset = Vec3::new(1.0, 2.0, 1.0);
    let (arrival, _) = relocate(&world, &mut pawn, offset, 0.4);
    assert_eq!(pawn.teleport.arrival_latches, vec![1]);
    let (_, events) = relocate(&world, &mut pawn, arrival, 0.4 + yaw);
    assert!(events.teleport.is_none());
    relocate(&world, &mut pawn, target + Vec3::X * 5.0, 0.4 + yaw);
    let (returned, events) = relocate(&world, &mut pawn, arrival, 0.4 + yaw);
    near(returned, offset);
    near(pawn.velocity, original.velocity);
    near(pawn.knockback_velocity, original.knockback_velocity);
    near(pawn.push_velocity, original.push_velocity);
    assert!((0.4 + yaw + events.teleport.unwrap().yaw_delta - 0.4).abs() < 0.0001);
    assert_eq!(pawn.teleport.generation, 2);
}

#[test]
fn teleport_blocked_arrival_refuses_without_state_or_event() {
    let logs = postretro_test_log_capture::LogCapture::start();
    let mut world = CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(20.0, -10.0, -10.0),
            Vec3::new(20.0, 10.0, -10.0),
            Vec3::new(20.0, 0.0, 10.0),
        ],
        vec![[0, 1, 2]],
    );
    install(
        &mut world,
        vec![record(
            TeleportMode::Classic,
            Vec3::new(20.0, 0.0, 0.0),
            1.0,
        )],
    );
    let mut pawn = pawn();
    let before = pawn.clone();
    let position = Vec3::ZERO;
    let (arrival, events) = relocate(&world, &mut pawn, position, 0.0);
    near(arrival, position);
    assert_eq!(pawn, before);
    assert!(events.teleport.is_none());
    relocate(&world, &mut pawn, position, 0.0);
    logs.assert_logged_once(
        log::Level::Warn,
        "teleporter entity 1 arrival blocked; teleport refused",
    );
}

#[test]
fn teleport_latches_all_arrival_regions_and_allows_unlatched_candidate() {
    let mut world = CollisionWorld::new();
    let target = Vec3::new(20.0, 0.0, 0.0);
    let first = record(TeleportMode::Classic, target, 0.0);
    let mut second = record(TeleportMode::Classic, Vec3::new(40.0, 0.0, 0.0), 0.0);
    second.source_entity = 2;
    second.brushes = vec![brush(target - Vec3::ONE, target + Vec3::ONE)];
    let mut third = second.clone();
    third.source_entity = 3;
    install(&mut world, vec![first, second, third]);
    let mut pawn = pawn();
    let (arrival, _) = relocate(&world, &mut pawn, Vec3::ZERO, 0.0);
    assert_eq!(pawn.teleport.arrival_latches, vec![1, 2]);
    assert!(
        relocate(&world, &mut pawn, arrival, 0.0)
            .1
            .teleport
            .is_none()
    );
    pawn.teleport.arrival_latches.retain(|index| *index != 2);
    assert_eq!(
        relocate(&world, &mut pawn, arrival, 0.0)
            .1
            .teleport
            .unwrap()
            .teleporter,
        2
    );
}
#[test]
fn teleport_refuses_arrival_wholly_inside_solid_or_exterior_runtime_cell() {
    use postretro_level_loader::{CellData, CellLocatorChild, CellLocatorNodeData, LevelWorld};
    // No triangles at all: only the cell locator can see the enclosure.
    let cell = |min, max, is_solid, is_exterior| CellData {
        bounds_min: min,
        bounds_max: max,
        face_start: 0,
        face_count: 0,
        portal_ref_start: 0,
        portal_ref_count: 0,
        is_solid,
        is_exterior,
        is_drawable: false,
    };
    for (is_solid, is_exterior) in [(true, false), (false, true)] {
        let mut level = LevelWorld::new_visibility_only(
            vec![
                cell(
                    Vec3::splat(-100.0),
                    Vec3::new(10.0, 100.0, 100.0),
                    false,
                    false,
                ),
                cell(
                    Vec3::new(10.0, -100.0, -100.0),
                    Vec3::splat(100.0),
                    is_solid,
                    is_exterior,
                ),
            ],
            Vec::new(),
            CellLocatorChild::Node(0),
            vec![CellLocatorNodeData {
                plane_normal: Vec3::X,
                plane_distance: 10.0,
                front: CellLocatorChild::Cell(1),
                back: CellLocatorChild::Cell(0),
            }],
            Vec::new(),
            false,
        )
        .unwrap();
        for (destination, blocked) in [(Vec3::X * 20.0, true), (Vec3::X * -20.0, false)] {
            level.teleporters = TeleportersSection {
                teleporters: vec![record(TeleportMode::Classic, destination, 0.0)],
            };
            let world = CollisionWorld::from_level(&level).unwrap();
            let mut pawn = pawn();
            let (arrival, events) = relocate(&world, &mut pawn, Vec3::ZERO, 0.0);
            assert_eq!(
                events.teleport.is_none(),
                blocked,
                "solid {is_solid} exterior {is_exterior} destination {destination:?}"
            );
            near(arrival, if blocked { Vec3::ZERO } else { destination });
        }
    }
}

#[test]
fn teleport_refuses_arrival_inside_closed_mover_at_its_current_pose() {
    use crate::collision::moving::{MoverCollider, MoverPose, MoverPoseSource};
    use postretro_entities::Transform;
    struct Poses(Vec3);
    impl MoverPoseSource for Poses {
        fn pose(&self, id: u32) -> Option<MoverPose> {
            (id == 7).then_some(MoverPose {
                transform: Transform {
                    position: self.0,
                    ..Default::default()
                },
                linear_velocity: Vec3::ZERO,
                tick_delta: Vec3::ZERO,
                angular_velocity: Vec3::ZERO,
                tick_rotation_delta: Quat::IDENTITY,
                carry_yaw: false,
                tick_dt: 1.0 / 60.0,
            })
        }
    }
    let vertices = [
        Vec3::new(-5.0, -5.0, -5.0),
        Vec3::new(5.0, -5.0, -5.0),
        Vec3::new(5.0, 5.0, -5.0),
        Vec3::new(-5.0, 5.0, -5.0),
        Vec3::new(-5.0, -5.0, 5.0),
        Vec3::new(5.0, -5.0, 5.0),
        Vec3::new(5.0, 5.0, 5.0),
        Vec3::new(-5.0, 5.0, 5.0),
    ];
    let triangles = [
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [3, 7, 6],
        [3, 6, 2],
        [0, 4, 7],
        [0, 7, 3],
        [1, 2, 6],
        [1, 6, 5],
    ];
    let movers = [MoverCollider::from_local_triangles(7, &vertices, &triangles).unwrap()];
    let mut world = CollisionWorld::new();
    install(
        &mut world,
        vec![record(TeleportMode::Classic, Vec3::X * 20.0, 0.0)],
    );
    for (position, blocked) in [(Vec3::X * 20.0, true), (Vec3::X * 40.0, false)] {
        let poses = Poses(position);
        let combined = CombinedCollisionWorld::new(&world, &movers, &poses);
        let mut pawn = pawn();
        let mut events = MovementEvents::default();
        let arrival = resolve(
            &mut pawn,
            &idle_input(),
            &combined,
            Vec3::ZERO,
            &mut events,
            |_| true,
        );
        assert_eq!(events.teleport.is_none(), blocked);
        near(arrival, if blocked { Vec3::ZERO } else { Vec3::X * 20.0 });
    }
}

/// One whole movement tick through the production entry, every region armed.
fn step(
    world: &CollisionWorld,
    pawn: &mut PlayerMovementComponent,
    position: Vec3,
) -> (Vec3, MovementEvents) {
    let (next, events, _) = crate::movement::tick_with_immersion_and_armed(
        pawn,
        &idle_input(),
        world,
        postretro_collision::EnvironmentView::level_only(0.0),
        1.0 / 60.0,
        position,
        |_| true,
    );
    (next, events)
}

#[test]
fn teleport_body_point_union_fires_on_the_post_step_point_never_on_capsule_overlap() {
    let mut world = CollisionWorld::new();
    let destination = Vec3::new(40.0, 0.0, 0.0);
    // One entity, two disjoint brushes: x in [-3, 3] and [9, 15]. The gap
    // between them is inside the union's bounds but outside the union.
    let mut gate = record(TeleportMode::Classic, destination, 0.0);
    gate.brushes = vec![
        brush(Vec3::splat(-3.0), Vec3::splat(3.0)),
        brush(Vec3::new(9.0, -3.0, -3.0), Vec3::new(15.0, 3.0, 3.0)),
    ];
    install(&mut world, vec![gate]);
    let fresh = |velocity: Vec3| {
        let mut pawn = PlayerMovementComponent::from_descriptor(&canonical_descriptor());
        pawn.velocity = velocity;
        pawn
    };
    // 12 m/s for one 60 Hz tick is a 0.2 m step.
    let inward = Vec3::X * 12.0;

    // Body point outside at tick start, inside after the step: fires.
    let mut pawn = fresh(inward);
    let (arrival, events) = step(&world, &mut pawn, Vec3::new(-3.1, 0.0, 0.0));
    assert!(events.teleport.is_some(), "post-step body point is inside");
    near(arrival, destination);

    // Body point inside at tick start, outside after the step: no fire.
    let mut pawn = fresh(inward);
    let (after, events) = step(&world, &mut pawn, Vec3::new(2.9, 0.0, 0.0));
    assert!(
        events.teleport.is_none(),
        "tick-start containment never fires"
    );
    assert!(
        after.x > 3.0,
        "fixture: the step carried the body point out"
    );

    // Capsule (radius 0.4) overlapping the region, body point outside: never.
    let mut pawn = fresh(Vec3::ZERO);
    let mut position = Vec3::new(3.3, 0.0, 0.0);
    for _ in 0..5 {
        let (next, events) = step(&world, &mut pawn, position);
        assert!(
            events.teleport.is_none(),
            "capsule overlap alone never fires"
        );
        position = next;
    }
    assert!(
        position.x - pawn.capsule.radius < 3.0,
        "fixture: the capsule still overlaps the region"
    );

    // Anywhere in the union fires; the gap between brushes does not.
    for (point, fires) in [
        (Vec3::new(-2.5, 1.0, 2.0), true),
        (Vec3::new(12.0, -2.0, 0.5), true),
        (Vec3::new(14.9, 2.9, -2.9), true),
        (Vec3::new(6.0, 0.0, 0.0), false),
    ] {
        let mut pawn = fresh(Vec3::ZERO);
        let (next, events) = step(&world, &mut pawn, point);
        assert_eq!(events.teleport.is_some(), fires, "body point {point:?}");
        near(next, if fires { destination } else { point });
    }
}

#[test]
fn teleport_movement_state_carry_aborts_vault_keeps_dash_and_slide_rotated_and_mid_air_airborne() {
    let mut world = CollisionWorld::new();
    let yaw = std::f32::consts::FRAC_PI_2;
    let destination = Vec3::new(40.0, 0.0, 0.0);
    install(
        &mut world,
        vec![record(TeleportMode::Classic, destination, yaw)],
    );
    let turn = Quat::from_rotation_y(yaw);
    let boost = Vec3::new(12.0, 0.0, 3.0);
    for state in [
        MovementState::Dash {
            elapsed_ms: 32.0,
            boost,
        },
        MovementState::Sliding {
            elapsed_ms: 64.0,
            boost,
            eye_current: 0.3,
        },
        MovementState::Vault {
            elapsed_ms: 48.0,
            target: Vec3::new(1.0, 2.0, 0.0),
            start_offset: Vec3::new(-0.8, -0.7, 0.0),
            mover: None,
            entry_speed: 4.0,
        },
    ] {
        let mut pawn = pawn();
        pawn.ground = GroundRef::World;
        pawn.movement_state = state;
        let before = pawn.clone();
        let (arrival, events) = relocate(&world, &mut pawn, Vec3::ZERO, 0.0);
        assert!(events.teleport.is_some(), "{state:?}");
        near(arrival, destination);
        // No floor under this arrival: airborne, and the source ground owes no
        // coyote jump.
        assert_eq!(pawn.ground, GroundRef::Airborne, "{state:?}");
        assert!(pawn.jump_spent, "{state:?}: coyote is spent");
        let aborted = [MovementStateEdge {
            from: MovementStateKind::Vault,
            to: MovementStateKind::Normal,
        }];
        let expected_edges: &[MovementStateEdge] = match state {
            MovementState::Vault { .. } => &aborted,
            _ => &[],
        };
        assert_eq!(events.state_edges, expected_edges, "{state:?}");
        near(pawn.velocity, turn * before.velocity);
        near(pawn.knockback_velocity, turn * before.knockback_velocity);
        near(pawn.push_velocity, turn * before.push_velocity);
        match (&state, &pawn.movement_state) {
            (
                MovementState::Dash { elapsed_ms, .. },
                MovementState::Dash {
                    elapsed_ms: kept,
                    boost: rotated,
                },
            )
            | (
                MovementState::Sliding { elapsed_ms, .. },
                MovementState::Sliding {
                    elapsed_ms: kept,
                    boost: rotated,
                    ..
                },
            ) => {
                assert_eq!(kept, elapsed_ms, "the dash/slide clock carries");
                near(*rotated, turn * boost);
            }
            (MovementState::Vault { .. }, MovementState::Normal) => {}
            (from, to) => panic!("{from:?} arrived as {to:?}"),
        }
    }

    // A dash survives the arrival and keeps driving the pawn along the
    // rotated heading on the following tick.
    let mut descriptor = canonical_descriptor();
    descriptor.dash = Some(postretro_foundation::DashParams {
        boost_speed: 18.0.into(),
        momentum_retention: 0.65.into(),
        steer_control: 0.0.into(),
        dash_drag: 4.0.into(),
        cooldown_ms: 250.0.into(),
        air_dashes: 1,
        preserve_vertical: false.into(),
    });
    let mut pawn = PlayerMovementComponent::from_descriptor(&descriptor);
    pawn.velocity = Vec3::X * 20.0;
    pawn.movement_state = MovementState::Dash {
        elapsed_ms: 16.0,
        boost: Vec3::X * 14.0,
    };
    let (arrival, events) = step(&world, &mut pawn, Vec3::new(-3.2, 0.0, 0.0));
    assert!(events.teleport.is_some());
    let heading = turn * Vec3::X;
    let (after, _) = step(&world, &mut pawn, arrival);
    assert!(matches!(pawn.movement_state, MovementState::Dash { .. }));
    let travelled = (after - arrival).normalize();
    assert!(
        travelled.dot(heading) > 0.99,
        "dash continues along {heading:?}, moved {travelled:?}"
    );
}

const GRAVITY: f32 = -20.32;
const DT: f32 = 1.0 / 60.0;

/// One flat static floor at y = 0 under a source room and its copy at x = 60.
fn floor_world() -> CollisionWorld {
    CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(-50.0, 0.0, -50.0),
            Vec3::new(-50.0, 0.0, 50.0),
            Vec3::new(100.0, 0.0, 50.0),
            Vec3::new(100.0, 0.0, -50.0),
        ],
        vec![[0, 1, 2], [0, 2, 3]],
    )
}

/// One production movement tick under level gravity, every region armed or none.
fn step_with(
    world: &CollisionWorld,
    pawn: &mut PlayerMovementComponent,
    input: &MovementInput,
    position: Vec3,
    armed: bool,
) -> (Vec3, MovementEvents) {
    let (next, events, _) = crate::movement::tick_with_immersion_and_armed(
        pawn,
        input,
        world,
        postretro_collision::EnvironmentView::level_only(GRAVITY),
        DT,
        position,
        |_| armed,
    );
    (next, events)
}

/// Idle ticks until the pawn rests on the floor at `start`'s x and z.
fn settled(
    world: &CollisionWorld,
    descriptor: &postretro_foundation::PlayerMovementDescriptor,
    start: Vec3,
) -> (PlayerMovementComponent, Vec3) {
    let mut pawn = PlayerMovementComponent::from_descriptor(descriptor);
    let mut position = start;
    for _ in 0..30 {
        position = step_with(world, &mut pawn, &idle_input(), position, false).0;
    }
    assert!(pawn.is_grounded(), "fixture pawn settles onto the floor");
    (pawn, position)
}

/// Forward at `facing`: yaw -pi/2 walks world +X.
fn walk(facing_yaw: f32) -> MovementInput {
    MovementInput {
        wish_dir: glam::Vec2::new(0.0, 1.0),
        facing_yaw,
        ..idle_input()
    }
}

/// The seamless region the pawn walks into along +X, onto a copy of the same
/// floor 60 m away and turned 90 degrees.
fn seamless_shift_world() -> CollisionWorld {
    let mut world = floor_world();
    let mut shift = record(
        TeleportMode::Seamless,
        Vec3::new(60.0, 0.0, 0.0),
        std::f32::consts::FRAC_PI_2,
    );
    shift.brushes = vec![brush(Vec3::new(2.0, -1.0, -3.0), Vec3::new(6.0, 4.0, 3.0))];
    install(&mut world, vec![shift]);
    world
}

fn named(events: &MovementEvents) -> Vec<&'static str> {
    let mut names = Vec::new();
    events.append_named_events(&mut names);
    names
}

#[test]
fn teleport_seamless_slide_onto_a_floor_keeps_sliding_with_no_slide_ended() {
    // Regression: arrival forced airborne, so the slide intent's airborne exit
    // ended every teleported slide one tick later.
    let world = seamless_shift_world();
    let mut descriptor = canonical_descriptor();
    descriptor.crouch = Some(postretro_foundation::CrouchParams {
        half_height: 0.4,
        eye_height: 0.2,
        transition_rate: 15.0,
    });
    descriptor.slide = Some(postretro_foundation::SlideParams {
        min_speed: 8.0,
        slide_drag: 2.0,
        slope_assist: 1.0,
        steer_rate: 180.0,
        entry_boost: 1.5,
        min_duration_ms: 250.0,
    });
    let (mut pawn, mut position) = settled(&world, &descriptor, Vec3::new(1.0, 1.3, 0.0));
    position.y += crate::movement::substrate::resize_capsule(
        &mut pawn,
        0.4,
        0.2,
        crate::movement::substrate::ResizeAnchor::Feet,
    );
    pawn.velocity = Vec3::X * 12.0;
    pawn.movement_state = MovementState::Sliding {
        elapsed_ms: 0.0,
        boost: Vec3::X * 4.0,
        eye_current: 0.2,
    };
    let crouched = MovementInput {
        crouch_intent: true,
        ..idle_input()
    };
    let slide_ended = |events: &MovementEvents| named(events).contains(&"slide_ended");
    let mut teleported = false;
    for _ in 0..20 {
        let (next, events) = step_with(&world, &mut pawn, &crouched, position, true);
        position = next;
        assert!(
            !slide_ended(&events),
            "fixture: the slide reaches the region"
        );
        if events.teleport.is_some() {
            teleported = true;
            break;
        }
    }
    assert!(teleported, "fixture: the slide crosses into the region");
    assert_eq!(
        pawn.ground,
        GroundRef::World,
        "the arrival floor grounds it"
    );
    for tick in 1..=3 {
        let (next, events) = step_with(&world, &mut pawn, &crouched, position, true);
        position = next;
        assert!(!slide_ended(&events), "tick {tick} after arrival");
        assert!(
            matches!(pawn.movement_state, MovementState::Sliding { .. }),
            "tick {tick}: still sliding"
        );
        assert!(pawn.is_grounded(), "tick {tick}: still on the copy's floor");
    }
}

#[test]
fn teleport_seamless_walking_shift_takes_no_air_tick_against_an_unshifted_control() {
    let world = seamless_shift_world();
    let (mut pawn, mut position) =
        settled(&world, &canonical_descriptor(), Vec3::new(-6.0, 1.3, 0.0));
    let (mut control, mut control_position) = (pawn.clone(), position);
    let (mut facing, control_facing) = (-std::f32::consts::FRAC_PI_2, -std::f32::consts::FRAC_PI_2);
    let mut after_shift = None;
    for _ in 0..240 {
        let (next, events) = step_with(&world, &mut pawn, &walk(facing), position, true);
        position = next;
        control_position = step_with(
            &world,
            &mut control,
            &walk(control_facing),
            control_position,
            false,
        )
        .0;
        if let Some(event) = events.teleport {
            // The caller turns the view by the delta, as host and prediction do.
            facing += event.yaw_delta;
            after_shift = Some(0);
        }
        if let Some(ticks) = after_shift.as_mut() {
            // From the teleport tick on, the shifted pawn walks exactly as the
            // control does: grounded, no gravity accrual, no change in height
            // or speed. One air tick would show as any of these.
            assert_eq!(
                pawn.ground,
                GroundRef::World,
                "tick {ticks} after the shift"
            );
            assert_eq!(pawn.air_ticks, 0, "tick {ticks}");
            assert!(
                (pawn.velocity.y - control.velocity.y).abs() < 1.0e-6,
                "tick {ticks}: vertical velocity {} vs control {}",
                pawn.velocity.y,
                control.velocity.y
            );
            assert!(
                (position.y - control_position.y).abs() < 1.0e-5,
                "tick {ticks}: height {} vs control {}",
                position.y,
                control_position.y
            );
            let horizontal = |v: Vec3| glam::Vec2::new(v.x, v.z).length();
            assert!(
                (horizontal(pawn.velocity) - horizontal(control.velocity)).abs() < 1.0e-4,
                "tick {ticks}: speed"
            );
            *ticks += 1;
            if *ticks > 3 {
                break;
            }
        }
    }
    assert!(
        after_shift.is_some_and(|ticks| ticks > 3),
        "fixture: the walk shifts"
    );
}

#[test]
fn teleport_classic_mid_air_arrival_owes_no_coyote_jump() {
    let mut world = floor_world();
    let mut gate = record(TeleportMode::Classic, Vec3::new(60.0, 20.0, 0.0), 0.0);
    gate.brushes = vec![brush(Vec3::new(2.0, -1.0, -3.0), Vec3::new(6.0, 4.0, 3.0))];
    install(&mut world, vec![gate]);
    let mut descriptor = canonical_descriptor();
    descriptor.forgiveness = Some(postretro_foundation::ForgivenessParams {
        coyote_ms: 150.0,
        jump_buffer_ms: 0.0,
    });
    let (mut pawn, mut position) = settled(&world, &descriptor, Vec3::new(1.0, 1.3, 0.0));
    pawn.velocity = Vec3::X * 7.0;
    let mut teleported = false;
    for _ in 0..20 {
        let (next, events) = step_with(
            &world,
            &mut pawn,
            &walk(-std::f32::consts::FRAC_PI_2),
            position,
            true,
        );
        position = next;
        if events.teleport.is_some() {
            teleported = true;
            break;
        }
    }
    assert!(teleported, "fixture: the walk crosses into the region");
    assert_eq!(pawn.ground, GroundRef::Airborne, "20 m above the floor");
    let jump = MovementInput {
        jump_pressed: true,
        ..idle_input()
    };
    let (_, events) = step_with(&world, &mut pawn, &jump, position, true);
    assert!(
        !events.jumped,
        "the source floor grants no coyote jump at a mid-air destination"
    );
    assert!(pawn.velocity.y < 0.0, "the pawn falls");
}

#[test]
fn teleport_classic_arrival_onto_a_floor_grounds_in_place_and_can_jump() {
    use crate::collision::moving::{MoverCollider, MoverPose, MoverPoseSource};
    struct Platform;
    impl MoverPoseSource for Platform {
        fn pose(&self, id: u32) -> Option<MoverPose> {
            (id == 3).then_some(MoverPose {
                transform: postretro_entities::Transform {
                    position: Vec3::new(-60.0, 10.0, 0.0),
                    ..Default::default()
                },
                linear_velocity: Vec3::ZERO,
                tick_delta: Vec3::ZERO,
                angular_velocity: Vec3::ZERO,
                tick_rotation_delta: Quat::IDENTITY,
                carry_yaw: false,
                tick_dt: DT,
            })
        }
    }
    let platform = [MoverCollider::from_local_triangles(
        3,
        &[
            Vec3::new(-4.0, 0.0, -4.0),
            Vec3::new(4.0, 0.0, -4.0),
            Vec3::new(4.0, 0.0, 4.0),
            Vec3::new(-4.0, 0.0, 4.0),
        ],
        &[[0, 1, 2], [0, 2, 3]],
    )
    .unwrap()];
    // Capsule center to feet is half height plus radius; destinations sit a
    // hair above their floor, as authored destinations do.
    let reach = 1.2;
    for (destination, floor) in [
        (Vec3::new(60.0, reach + 0.02, 0.0), true),
        // A mover top is no arrival floor: mover carry never follows a teleport.
        (Vec3::new(-60.0, 10.0 + reach + 0.02, 0.0), false),
    ] {
        let mut world = floor_world();
        let mut gate = record(TeleportMode::Classic, destination, 0.0);
        gate.brushes = vec![brush(
            Vec3::new(-3.0, -1.0, -3.0),
            Vec3::new(3.0, 12.0, 3.0),
        )];
        install(&mut world, vec![gate]);
        let combined = CombinedCollisionWorld::new(&world, &platform, &Platform);
        let mut pawn = PlayerMovementComponent::from_descriptor(&canonical_descriptor());
        // Falling, long airborne: no ground or coyote of its own to jump from.
        pawn.ground = GroundRef::Airborne;
        pawn.coyote_timer_ms = 1000.0;
        pawn.velocity = Vec3::new(0.0, -5.0, 0.0);
        let tick = |pawn: &mut PlayerMovementComponent, input: &MovementInput, position: Vec3| {
            crate::movement::tick_with_immersion_and_armed(
                pawn,
                input,
                &combined,
                postretro_collision::EnvironmentView::level_only(GRAVITY),
                DT,
                position,
                |_| true,
            )
        };
        let (arrival, events, _) = tick(&mut pawn, &idle_input(), Vec3::new(0.0, 8.0, 0.0));
        assert!(events.teleport.is_some(), "{destination:?}");
        near(arrival, destination);
        let falling = pawn.velocity;
        assert!(
            falling.y < -5.0,
            "the fall keeps its speed through the arrival"
        );
        if floor {
            assert_eq!(pawn.ground, GroundRef::World);
            near(pawn.last_floor_normal.unwrap(), Vec3::Y);
        } else {
            assert_eq!(pawn.ground, GroundRef::Airborne, "never Mover");
            assert_eq!(pawn.last_floor_normal, None);
        }
        let jump = MovementInput {
            jump_pressed: true,
            ..idle_input()
        };
        let (_, events, _) = tick(&mut pawn, &jump, arrival);
        assert_eq!(events.jumped, floor, "{destination:?}");
    }
}

// Teleport arrival keeps its floor classification: only ground-stick's drop
// became swept, so a wall pressed against the arrival capsule (which can turn
// the swept down-cast's first hit into the wall's) still grounds it.
#[test]
fn teleport_classic_arrival_beside_a_wall_still_lands_grounded() {
    let reach = 1.2;
    let radius = canonical_descriptor().capsule.radius;
    let destination = Vec3::new(60.0, reach + 0.02, 0.0);
    // The floor, plus a wall facing -X a centimetre past the arrival capsule.
    let wall_x = destination.x + radius + 0.01;
    let mut world = CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(-50.0, 0.0, -50.0),
            Vec3::new(-50.0, 0.0, 50.0),
            Vec3::new(100.0, 0.0, 50.0),
            Vec3::new(100.0, 0.0, -50.0),
            Vec3::new(wall_x, 0.0, -50.0),
            Vec3::new(wall_x, 5.0, -50.0),
            Vec3::new(wall_x, 5.0, 50.0),
            Vec3::new(wall_x, 0.0, 50.0),
        ],
        vec![[0, 1, 2], [0, 2, 3], [4, 6, 5], [4, 7, 6]],
    );
    let mut gate = record(TeleportMode::Classic, destination, 0.0);
    gate.brushes = vec![brush(
        Vec3::new(-3.0, -1.0, -3.0),
        Vec3::new(3.0, 12.0, 3.0),
    )];
    install(&mut world, vec![gate]);
    let mut pawn = PlayerMovementComponent::from_descriptor(&canonical_descriptor());
    pawn.ground = GroundRef::Airborne;
    pawn.coyote_timer_ms = 1000.0;
    pawn.velocity = Vec3::new(0.0, -5.0, 0.0);
    let (arrival, events) = step_with(
        &world,
        &mut pawn,
        &idle_input(),
        Vec3::new(0.0, 8.0, 0.0),
        true,
    );
    assert!(events.teleport.is_some());
    near(arrival, destination);
    assert_eq!(pawn.ground, GroundRef::World, "the arrival lands grounded");
}

/// Walk a settled pawn into a classic gate whose destination puts its feet
/// `feet_above_floor` over the copy floor at x = 60; returns the arrival.
fn walk_into_gate_arriving(
    descriptor: &postretro_foundation::PlayerMovementDescriptor,
    feet_above_floor: f32,
) -> (CollisionWorld, PlayerMovementComponent, Vec3) {
    let mut world = floor_world();
    // Canonical capsule: center to feet is half height plus radius.
    let destination = Vec3::new(60.0, 1.2 + feet_above_floor, 0.0);
    let mut gate = record(TeleportMode::Classic, destination, 0.0);
    gate.brushes = vec![brush(Vec3::new(2.0, -1.0, -3.0), Vec3::new(6.0, 4.0, 3.0))];
    install(&mut world, vec![gate]);
    let (mut pawn, mut position) = settled(&world, descriptor, Vec3::new(1.0, 1.3, 0.0));
    pawn.velocity = Vec3::X * 7.0;
    for _ in 0..20 {
        let (next, events) = step_with(
            &world,
            &mut pawn,
            &walk(-std::f32::consts::FRAC_PI_2),
            position,
            true,
        );
        position = next;
        if events.teleport.is_some() {
            near(position, destination);
            return (world, pawn, position);
        }
    }
    panic!("fixture: the walk crosses into the region");
}

#[test]
fn teleport_arrival_floating_above_its_floor_is_airborne_and_owes_no_grounded_jump() {
    // Within the ground-stick reach but above the arrival tolerance: grounding
    // it in place would let the next tick jump from feet hanging in the air.
    let mut descriptor = canonical_descriptor();
    descriptor.forgiveness = Some(postretro_foundation::ForgivenessParams {
        coyote_ms: 150.0,
        jump_buffer_ms: 0.0,
    });
    let jump = MovementInput {
        jump_pressed: true,
        ..idle_input()
    };
    // Inside the skin (the dev maps' 40-unit destinations for a 1 m reach) and
    // one map unit over the floor (the documented margin) both stand there.
    for (feet_above_floor, grounded) in [(0.016, true), (0.0254, true), (0.3, false)] {
        let (world, mut pawn, arrival) = walk_into_gate_arriving(&descriptor, feet_above_floor);
        let expected = if grounded {
            GroundRef::World
        } else {
            GroundRef::Airborne
        };
        assert_eq!(pawn.ground, expected, "feet {feet_above_floor} m up");
        let (next, events) = step_with(&world, &mut pawn, &jump, arrival, true);
        assert_eq!(events.jumped, grounded, "feet {feet_above_floor} m up");
        if !grounded {
            assert!(next.y < arrival.y, "the floating arrival falls");
        }
    }

    // Mid-air arrivals keep their air jumps: only the coyote jump is gone.
    descriptor.air.jumps = 1;
    let (world, mut pawn, arrival) = walk_into_gate_arriving(&descriptor, 0.3);
    assert_eq!(pawn.air_jumps_remaining, 1);
    let (_, events) = step_with(&world, &mut pawn, &jump, arrival, true);
    assert!(events.jumped, "the air jump fires");
    assert_eq!(pawn.air_jumps_remaining, 0, "it spent the air charge");
}

#[test]
fn teleport_floating_arrival_beside_a_ledge_takes_no_ground_vault() {
    use crate::movement::vault_tests::{
        REACH, RISER_X, forward, ground_auto, ledge_world, vault_descriptor,
    };
    // The vault enters only from the ground: a floating arrival treated as
    // grounded would start one from its hanging feet.
    let descriptor = vault_descriptor(postretro_foundation::VaultEntry {
        air: None,
        ..ground_auto()
    });
    for (feet_above_floor, vaults) in [(0.02, true), (0.3, false)] {
        // The capsule surface 5 cm short of the riser.
        let destination = Vec3::new(RISER_X - 0.45, REACH + feet_above_floor, 0.0);
        let mut world = ledge_world(1.0, None);
        install(
            &mut world,
            vec![record(TeleportMode::Classic, destination, 0.0)],
        );
        let mut pawn = PlayerMovementComponent::from_descriptor(&descriptor);
        pawn.ground = GroundRef::World;
        let (arrival, events) = relocate(&world, &mut pawn, Vec3::new(0.0, REACH, 0.0), 0.0);
        assert!(events.teleport.is_some());
        near(arrival, destination);
        assert_eq!(pawn.is_grounded(), vaults, "feet {feet_above_floor} m up");
        step_with(&world, &mut pawn, &forward(), arrival, false);
        assert_eq!(
            pawn.movement_state.kind() == MovementStateKind::Vault,
            vaults,
            "feet {feet_above_floor} m up"
        );
    }
}

#[test]
fn teleport_crouched_slide_into_a_standing_height_pad_arrives_airborne() {
    use crate::movement::substrate::{ResizeAnchor, resize_capsule};
    let mut world = floor_world();
    // Authored for the standing capsule, with the one-unit margin.
    let destination = Vec3::new(20.0, 1.2 + 0.0254, 0.0);
    install(
        &mut world,
        vec![record(TeleportMode::Classic, destination, 0.0)],
    );
    let mut descriptor = canonical_descriptor();
    // A crouch 0.25 m shorter than standing: inside the ground-stick reach,
    // well above the arrival tolerance.
    descriptor.crouch = Some(postretro_foundation::CrouchParams {
        half_height: 0.55,
        eye_height: 0.3,
        transition_rate: 15.0,
    });
    for crouched in [false, true] {
        let mut pawn = PlayerMovementComponent::from_descriptor(&descriptor);
        let mut source = Vec3::new(0.0, 1.22, 0.0);
        if crouched {
            source.y += resize_capsule(&mut pawn, 0.55, 0.3, ResizeAnchor::Feet);
            pawn.movement_state = MovementState::Sliding {
                elapsed_ms: 0.0,
                boost: Vec3::X * 4.0,
                eye_current: 0.3,
            };
        }
        pawn.ground = GroundRef::World;
        pawn.velocity = Vec3::X * 12.0;
        let (arrival, events) = relocate(&world, &mut pawn, source, 0.0);
        assert!(events.teleport.is_some());
        near(arrival, destination);
        if crouched {
            assert_eq!(pawn.ground, GroundRef::Airborne, "the crouched feet hang");
            assert_eq!(pawn.last_floor_normal, None);
            assert!(pawn.jump_spent, "no coyote jump from the source floor");
        } else {
            assert_eq!(pawn.ground, GroundRef::World, "standing fits the pad");
        }
    }
}

#[test]
fn teleport_vault_abort_reports_vault_ended_on_the_start_tick_and_mid_vault() {
    use crate::movement::vault_tests::{
        REACH, RISER_X, forward, ground_auto, grounded_at_riser, ledge_world, vault_descriptor,
    };
    let descriptor = vault_descriptor(ground_auto());
    let destination = Vec3::new(-30.0, REACH + 0.02, 0.0);
    // Ticks the region stays disarmed first: zero aborts the vault on the tick
    // it starts, one aborts it on its first path tick after that.
    for disarmed_ticks in [0, 1] {
        let mut world = ledge_world(1.0, None);
        let (mut pawn, mut position) = grounded_at_riser(&descriptor, &world, 0.05);
        // Installed after settling: the settle ticks arm regions on spawn.
        let mut gate = record(TeleportMode::Classic, destination, 0.0);
        gate.brushes = vec![brush(
            Vec3::new(2.0, 0.0, -2.0),
            Vec3::new(RISER_X - 0.01, 4.0, 2.0),
        )];
        install(&mut world, vec![gate]);
        let mut names = Vec::new();
        for tick in 0..=disarmed_ticks {
            let (next, events) = step_with(
                &world,
                &mut pawn,
                &forward(),
                position,
                tick == disarmed_ticks,
            );
            position = next;
            names.push(named(&events));
            if tick < disarmed_ticks {
                assert_eq!(pawn.movement_state.kind(), MovementStateKind::Vault);
            } else {
                assert!(
                    events.teleport.is_some(),
                    "the vault's post-step point is inside"
                );
            }
        }
        near(position, destination);
        assert_eq!(pawn.movement_state.kind(), MovementStateKind::Normal);
        let expected: Vec<Vec<&str>> = if disarmed_ticks == 0 {
            vec![vec!["vault_started", "vault_ended"]]
        } else {
            vec![vec!["vault_started"], vec!["vault_ended"]]
        };
        assert_eq!(names, expected, "disarmed for {disarmed_ticks} ticks");
    }
}

#[test]
fn teleport_non_finite_facing_refuses_without_spending_the_blocked_arrival_warning() {
    let logs = postretro_test_log_capture::LogCapture::start();
    let mut world = CollisionWorld::from_triangles_for_test(
        vec![
            Vec3::new(20.0, -10.0, -10.0),
            Vec3::new(20.0, 10.0, -10.0),
            Vec3::new(20.0, 0.0, 10.0),
        ],
        vec![[0, 1, 2]],
    );
    install(
        &mut world,
        vec![record(
            TeleportMode::Classic,
            Vec3::new(20.0, 0.0, 0.0),
            1.0,
        )],
    );
    let mut pawn = pawn();
    let before = pawn.clone();
    let (arrival, events) = relocate(&world, &mut pawn, Vec3::ZERO, f32::NAN);
    near(arrival, Vec3::ZERO);
    assert_eq!(pawn, before);
    assert!(events.teleport.is_none());
    logs.assert_not_logged(log::Level::Warn, "arrival blocked");
    // The genuine blocked arrival still gets its one warning.
    relocate(&world, &mut pawn, Vec3::ZERO, 0.0);
    logs.assert_logged_once(
        log::Level::Warn,
        "teleporter entity 1 arrival blocked; teleport refused",
    );
}

mod seamless_mapping_parity {
    use super::*;
    use glam::Mat4;
    use proptest::prelude::*;

    fn finite_yaw() -> impl Strategy<Value = f32> {
        prop_oneof![
            Just(0.0),
            Just(std::f32::consts::FRAC_PI_2),
            Just(std::f32::consts::PI),
            -std::f32::consts::PI..std::f32::consts::PI,
        ]
    }

    fn origin() -> impl Strategy<Value = Vec3> {
        (-40.0f32..40.0, -8.0f32..8.0, -40.0f32..40.0).prop_map(|(x, y, z)| Vec3::new(x, y, z))
    }

    proptest! {
        // A ghost drawn through a link and a seamless arrival through the same
        // landmark and target must agree, rotated pairs and vertical offsets
        // included (ghost-entities AC "ghost pose equals the seamless pose").
        #[test]
        fn seamless_teleport_arrival_is_the_shared_mapping_of_the_body_point(
            landmark in origin(),
            landmark_yaw in finite_yaw(),
            target_offset in origin(),
            target_yaw in finite_yaw(),
            body in (-2.5f32..2.5, -2.5f32..2.5, -2.5f32..2.5),
        ) {
            // Keep the arrival clear of the source region so it latches nothing.
            let target = landmark + Vec3::new(100.0, 0.0, 0.0) + target_offset;
            let mut seamless = record(TeleportMode::Seamless, target, target_yaw);
            seamless.landmark_origin = landmark.to_array();
            seamless.landmark_yaw = landmark_yaw;
            seamless.brushes = vec![brush(landmark - Vec3::splat(3.0), landmark + Vec3::splat(3.0))];
            let mut world = CollisionWorld::new();
            install(&mut world, vec![seamless]);
            let position = landmark + Vec3::new(body.0, body.1, body.2);
            let mut pawn = pawn();
            let (arrival, events) = relocate(&world, &mut pawn, position, 0.4);

            let mapping = SeamlessMapping::new(landmark, landmark_yaw, target, target_yaw);
            let event = events.teleport.expect("a body point inside the region fires");
            prop_assert_eq!(arrival, mapping.point(position));
            prop_assert_eq!(event.yaw_delta, mapping.yaw_delta());
            prop_assert!((arrival.y - (target.y + (position.y - landmark.y))).abs() < 1.0e-4);
            let by_matrix = mapping.matrix().transform_point3(position);
            prop_assert!((by_matrix - arrival).length() < 1.0e-3, "{by_matrix:?} != {arrival:?}");
            // Compare rotated basis vectors: a quaternion angle between nearly
            // equal rotations is ill-conditioned at this tolerance.
            let facing = Quat::from_rotation_y(landmark_yaw + 0.3);
            let ghost = mapping.matrix() * Mat4::from_quat(facing);
            for axis in [Vec3::X, Vec3::Z] {
                let by_matrix = ghost.transform_vector3(axis);
                let by_rotation = mapping.rotation() * facing * axis;
                prop_assert!((by_matrix - by_rotation).length() < 1.0e-4, "{by_matrix:?} != {by_rotation:?}");
            }
        }
    }
}
