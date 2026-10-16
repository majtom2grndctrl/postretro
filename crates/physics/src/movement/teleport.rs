// The single post-movement pawn relocation chokepoint, shared by authority and prediction.
// See: context/lib/movement.md §6 (Teleport)

use super::{MovementEvents, MovementInput};
use crate::collision::moving::CombinedCollisionWorld;
use glam::{Quat, Vec3};
use postretro_foundation::{GroundRef, MovementState, PlayerMovementComponent, wrap_teleport_yaw};
use postretro_level_format::teleporters::TeleportMode;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TeleportEvent {
    pub teleporter: u16,
    pub yaw_delta: f32,
    pub arrival: Vec3,
}

pub(super) fn resolve(
    component: &mut PlayerMovementComponent,
    input: &MovementInput,
    collision: &CombinedCollisionWorld<'_>,
    position: Vec3,
    events: &mut MovementEvents,
    mut armed: impl FnMut(usize) -> bool,
) -> Vec3 {
    let teleporters = &collision.static_world.teleporters;
    component
        .teleport
        .arrival_latches
        .retain(|index| teleporters.regions.contains(usize::from(*index), position));
    if component.vault_barred {
        return position;
    }
    let Some(index) = teleporters.regions.winner(position, |index| {
        armed(index)
            && u16::try_from(index).is_ok_and(|index| {
                component
                    .teleport
                    .arrival_latches
                    .binary_search(&index)
                    .is_err()
            })
    }) else {
        return position;
    };
    let record = &teleporters.records()[index];
    let yaw_delta = match record.mode {
        TeleportMode::Classic => record.destination_yaw - input.facing_yaw,
        TeleportMode::Seamless => record.destination_yaw - record.landmark_yaw,
    };
    let rotation = Quat::from_rotation_y(yaw_delta);
    let arrival = match record.mode {
        TeleportMode::Classic => Vec3::from(record.destination_origin),
        TeleportMode::Seamless => {
            Vec3::from(record.destination_origin)
                + rotation * (position - Vec3::from(record.landmark_origin))
        }
    };
    if !arrival.is_finite()
        || !yaw_delta.is_finite()
        || !crate::collision::capsule_fits_combined(collision, arrival, &component.capsule)
    {
        teleporters.warn_refused(index);
        return position;
    }
    // State payloads bank motion too; rotating total velocity alone would let the
    // next dash/slide intent reintroduce motion in the source room's frame.
    match &mut component.movement_state {
        MovementState::Dash { boost, .. } | MovementState::Sliding { boost, .. } => {
            *boost = rotation * *boost
        }
        MovementState::Vault { .. } => {
            component.movement_state = MovementState::Normal;
            component.swim_reentry_latched = false;
        }
        _ => {}
    }
    component.velocity = rotation * component.velocity;
    component.knockback_velocity = rotation * component.knockback_velocity;
    component.push_velocity = rotation * component.push_velocity;
    component.ground = GroundRef::Airborne;
    component.last_floor_normal = None;
    component.teleport.generation = component.teleport.generation.wrapping_add(1);
    component.teleport.accumulated_yaw =
        wrap_teleport_yaw(component.teleport.accumulated_yaw + yaw_delta);
    component.teleport.last_teleporter = index as u16;
    component.teleport.arrival_latches.clear();
    teleporters.regions.visit_containing(arrival, |index| {
        if armed(index) {
            component.teleport.arrival_latches.push(index as u16);
        }
    });
    component.teleport.arrival_latches.sort_unstable();
    events.teleport = Some(TeleportEvent {
        teleporter: index as u16,
        yaw_delta,
        arrival,
    });
    arrival
}

#[cfg(test)]
mod tests {
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
    fn teleport_movement_state_carry_aborts_vault_keeps_dash_and_slide_rotated_and_airborne() {
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
            assert_eq!(pawn.ground, GroundRef::Airborne, "{state:?}");
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
}
