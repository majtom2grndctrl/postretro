use glam::{Quat, Vec3};
use postretro_entities::Transform;
use postretro_foundation::GroundRef;

use super::{PresentedMover, rebase_rider};
use crate::interpolation::{RiderBracket, RiderSample};

const MOVER: u32 = 7;
const LOCAL_POINT: Vec3 = Vec3::new(1.0, 1.21, 0.0);

fn point_on(frame: &Transform, local: Vec3) -> Vec3 {
    frame.position + frame.rotation * (frame.scale * local)
}

fn mover_at(yaw: f32, x: f32) -> Transform {
    Transform {
        position: Vec3::new(x, 0.0, 0.0),
        rotation: Quat::from_rotation_y(yaw),
        scale: Vec3::ONE,
    }
}

fn mover_pose(tick: u32) -> Transform {
    // Ticks 10 and 12 bracket the rider; a quarter radian of spin and 10 cm of
    // travel per tick, so the chord and the arc visibly differ.
    let ticks = tick as f32 - 10.0;
    mover_at(0.25 * ticks, 0.1 * ticks)
}

fn on_mover(transform: &Transform) -> Transform {
    Transform {
        position: point_on(transform, LOCAL_POINT),
        rotation: Quat::from_rotation_y(0.3),
        scale: Vec3::splat(1.5),
    }
}

fn rider(tick: u32, ground: Option<GroundRef>) -> RiderSample {
    RiderSample {
        server_tick: tick,
        transform: on_mover(&mover_pose(tick)),
        ground,
    }
}

fn bracket(from: Option<GroundRef>, to: Option<GroundRef>) -> RiderBracket {
    RiderBracket {
        from: rider(10, from),
        to: rider(12, to),
        alpha: 0.5,
    }
}

fn blended(bracket: &RiderBracket) -> Transform {
    Transform {
        position: bracket
            .from
            .transform
            .position
            .lerp(bracket.to.transform.position, bracket.alpha),
        ..bracket.from.transform
    }
}

fn authoritative(mover_id: u32, tick: u32) -> Option<Transform> {
    (mover_id == MOVER).then(|| mover_pose(tick))
}

const PRESENTED: Transform = Transform {
    position: Vec3::new(0.37, 0.0, 0.0),
    rotation: Quat::from_xyzw(0.0, 0.479_425_55, 0.0, 0.877_582_54),
    scale: Vec3::ONE,
};

fn presented(mover_id: u32) -> Option<PresentedMover> {
    (mover_id == MOVER).then_some(PresentedMover {
        transform: PRESENTED,
        carry_yaw: false,
    })
}

fn as_presented(transform: Transform) -> impl Fn(u32) -> Option<PresentedMover> {
    move |_| {
        Some(PresentedMover {
            transform,
            carry_yaw: false,
        })
    }
}

#[test]
fn rebase_places_planted_rider_at_its_point_on_the_presented_mover() {
    let on = Some(GroundRef::Mover(MOVER));
    let bracket = bracket(on, on);
    let world = blended(&bracket);
    let placed = rebase_rider(&bracket, &world, authoritative, presented)
        .expect("both samples stand on one mover");
    let expected = point_on(&PRESENTED, LOCAL_POINT);
    assert!(
        (placed.transform.position - expected).length() < 1.0e-5,
        "placed {:?} expected {expected:?}",
        placed.transform.position
    );
    // Sanity: the world blend is elsewhere (the delay and the chord both show).
    assert!((world.position - expected).length() > 0.1);
}

#[test]
fn rebase_keeps_blended_orientation_and_scale() {
    let on = Some(GroundRef::Mover(MOVER));
    let bracket = bracket(on, on);
    let world = blended(&bracket);
    let placed = rebase_rider(&bracket, &world, authoritative, presented).unwrap();
    assert_eq!(placed.transform.rotation, world.rotation);
    assert_eq!(placed.transform.scale, world.scale);
}

#[test]
fn rebase_attaches_only_when_both_samples_name_one_mover() {
    let cases = [
        (
            Some(GroundRef::Mover(MOVER)),
            Some(GroundRef::Mover(MOVER + 1)),
        ),
        (Some(GroundRef::World), Some(GroundRef::Mover(MOVER))),
        (Some(GroundRef::Mover(MOVER)), Some(GroundRef::Airborne)),
        (Some(GroundRef::Mover(MOVER)), None),
        (None, Some(GroundRef::Mover(MOVER))),
        (Some(GroundRef::World), Some(GroundRef::World)),
    ];
    for (from, to) in cases {
        let bracket = bracket(from, to);
        assert_eq!(
            rebase_rider(&bracket, &blended(&bracket), authoritative, presented),
            None,
            "{from:?} → {to:?} must stay in world space"
        );
    }
}

#[test]
fn rebase_falls_back_without_a_mover_pose_for_either_sample_or_the_frame() {
    let on = Some(GroundRef::Mover(MOVER));
    let bracket = bracket(on, on);
    let world = blended(&bracket);
    let missing_from = |id, tick| (tick != 10).then(|| authoritative(id, tick)).flatten();
    let missing_to = |id, tick| (tick != 12).then(|| authoritative(id, tick)).flatten();
    assert_eq!(
        rebase_rider(&bracket, &world, missing_from, presented),
        None
    );
    assert_eq!(rebase_rider(&bracket, &world, missing_to, presented), None);
    assert_eq!(
        rebase_rider(&bracket, &world, authoritative, |_| None),
        None
    );
}

#[test]
fn rebase_falls_back_on_non_finite_or_collapsed_mover_frames() {
    let on = Some(GroundRef::Mover(MOVER));
    let bracket = bracket(on, on);
    let world = blended(&bracket);
    let broken = [
        Transform {
            position: Vec3::new(f32::NAN, 0.0, 0.0),
            ..PRESENTED
        },
        Transform {
            scale: Vec3::new(1.0, 0.0, 1.0),
            ..PRESENTED
        },
        Transform {
            rotation: Quat::from_xyzw(0.0, 0.0, 0.0, 0.0),
            ..PRESENTED
        },
        Transform {
            rotation: Quat::from_xyzw(f32::INFINITY, 0.0, 0.0, 1.0),
            ..PRESENTED
        },
    ];
    for frame in broken {
        assert_eq!(
            rebase_rider(&bracket, &world, authoritative, as_presented(frame)),
            None,
            "presented {frame:?} must fall back"
        );
        assert_eq!(
            rebase_rider(&bracket, &world, |_, _| Some(frame), presented),
            None,
            "sample frame {frame:?} must fall back"
        );
    }
}

#[test]
fn held_bracket_rides_the_presented_mover() {
    let on = Some(GroundRef::Mover(MOVER));
    let held = rider(12, on);
    let bracket = RiderBracket {
        from: held,
        to: held,
        alpha: 0.0,
    };
    let placed = rebase_rider(&bracket, &held.transform, authoritative, presented).unwrap();
    assert!((placed.transform.position - point_on(&PRESENTED, LOCAL_POINT)).length() < 1.0e-5);
}

mod host_mover_poses {
    use glam::Vec3;
    use postretro_entities::{EntityRegistry, Transform};

    use crate::rider_rebase::{HOST_MOVER_POSE_TICKS, RiderMoverPoses};

    fn at(x: f32) -> Transform {
        Transform {
            position: Vec3::new(x, 0.0, 0.0),
            ..Transform::default()
        }
    }

    #[test]
    fn host_mover_pose_answers_exact_ticks_only_within_its_bound() {
        let mut registry = EntityRegistry::new();
        let entity = registry.spawn(Transform::default());
        let mut poses = RiderMoverPoses::default();
        let first = 100u32;
        for offset in 0..=HOST_MOVER_POSE_TICKS as u32 {
            poses.record(3, entity, first + offset, at(offset as f32));
        }
        // A repeat for the newest tick replaces it.
        let newest = first + HOST_MOVER_POSE_TICKS as u32;
        poses.record(3, entity, newest, at(-1.0));
        assert_eq!(poses.pose_at(3, newest), Some(at(-1.0)));
        assert_eq!(poses.pose_at(3, first + 1), Some(at(1.0)));
        assert_eq!(poses.pose_at(3, first), None, "evicted past the bound");
        assert_eq!(poses.pose_at(3, newest + 1), None, "never extrapolated");
        assert_eq!(poses.pose_at(4, newest), None);
        assert_eq!(poses.entity(3), Some(entity));
        assert_eq!(poses.entity(4), None);
    }
}

mod carry_yaw {
    use glam::{EulerRot, Quat, Vec3};
    use postretro_entities::Transform;
    use postretro_foundation::GroundRef;

    use super::{LOCAL_POINT, MOVER, mover_at, point_on};
    use crate::interpolation::{RiderBracket, RiderSample};
    use crate::rider_rebase::{PresentedMover, rebase_rider};

    const PITCH: f32 = 0.2;
    const ROLL: f32 = -0.1;

    /// A rider at mover yaw + `relative`, with its own aim pitch and a roll.
    fn sample(tick: u32, mover_yaw: f32, relative: f32) -> RiderSample {
        let mover = mover_at(mover_yaw, 0.0);
        RiderSample {
            server_tick: tick,
            transform: Transform {
                position: point_on(&mover, LOCAL_POINT),
                rotation: Quat::from_euler(EulerRot::YXZ, mover_yaw + relative, PITCH, ROLL),
                scale: Vec3::ONE,
            },
            ground: Some(GroundRef::Mover(MOVER)),
        }
    }

    fn bracket() -> RiderBracket {
        RiderBracket {
            // The mover turns half a radian; the rider turns 0.1 on it.
            from: sample(10, 0.0, 0.3),
            to: sample(12, 0.5, 0.4),
            alpha: 0.5,
        }
    }

    fn mover_yaw_at(tick: u32) -> f32 {
        if tick == 10 { 0.0 } else { 0.5 }
    }

    fn blended(bracket: &RiderBracket) -> Transform {
        Transform {
            position: Vec3::ZERO,
            rotation: bracket
                .from
                .transform
                .rotation
                .slerp(bracket.to.transform.rotation, bracket.alpha),
            scale: Vec3::ONE,
        }
    }

    fn rebase(drawn: Quat, carry_yaw: bool) -> crate::rider_rebase::RebasedRider {
        let bracket = bracket();
        rebase_rider(
            &bracket,
            &blended(&bracket),
            |_, tick| Some(mover_at(mover_yaw_at(tick), 0.0)),
            |_| {
                Some(PresentedMover {
                    transform: Transform {
                        position: Vec3::new(0.2, 0.0, 0.0),
                        rotation: drawn,
                        scale: Vec3::ONE,
                    },
                    carry_yaw,
                })
            },
        )
        .expect("both samples stand on the mover")
    }

    fn yaw_pitch_roll(rotation: Quat) -> (f32, f32, f32) {
        rotation.to_euler(EulerRot::YXZ)
    }

    #[test]
    fn carried_yaw_is_relative_blend_plus_drawn_mover_yaw_exactly_once() {
        let rebased = rebase(Quat::from_rotation_y(1.0), true);
        let (yaw, pitch, roll) = yaw_pitch_roll(rebased.transform.rotation);
        assert!((yaw - 1.35).abs() < 1.0e-5, "yaw {yaw}");
        let (_, blended_pitch, blended_roll) = yaw_pitch_roll(blended(&bracket()).rotation);
        assert!((pitch - blended_pitch).abs() < 1.0e-5 && (roll - blended_roll).abs() < 1.0e-5);
        // The same frame sampled twice gives the same yaw.
        assert_eq!(rebase(Quat::from_rotation_y(1.0), true), rebased);
    }

    #[test]
    fn mover_pitch_or_roll_never_enters_rider_orientation() {
        for tilt in [Quat::from_rotation_x(0.4), Quat::from_rotation_z(0.3)] {
            let tilted = Quat::from_rotation_y(1.0) * tilt;
            let carried = rebase(tilted, true);
            let (yaw, pitch, roll) = yaw_pitch_roll(carried.transform.rotation);
            let (_, blended_pitch, blended_roll) = yaw_pitch_roll(blended(&bracket()).rotation);
            assert!((yaw - 1.35).abs() < 1.0e-5, "world-up yaw only: {yaw}");
            assert!((pitch - blended_pitch).abs() < 1.0e-5 && (roll - blended_roll).abs() < 1.0e-5);

            let uncarried = rebase(tilted, false);
            assert_eq!(uncarried.transform.rotation, blended(&bracket()).rotation);
            assert_eq!(uncarried.yaw_turn, 0.0);
        }
    }

    #[test]
    fn without_carry_yaw_mover_rotation_adds_no_yaw() {
        let rebased = rebase(Quat::from_rotation_y(2.5), false);
        assert_eq!(rebased.transform.rotation, blended(&bracket()).rotation);
        assert_eq!(rebased.yaw_turn, 0.0);
        assert_eq!(rebased.turn(Vec3::X), Vec3::X);
    }

    #[test]
    fn legs_turn_with_the_body_so_heading_relative_to_aim_is_kept() {
        let rebased = rebase(Quat::from_rotation_y(1.0), true);
        let velocity = Vec3::new(0.3, 0.0, -1.2);
        let heading = |v: Vec3| v.x.atan2(v.z);
        let (blended_yaw, _, _) = yaw_pitch_roll(blended(&bracket()).rotation);
        let (aim, _, _) = yaw_pitch_roll(rebased.transform.rotation);
        let before = heading(velocity) - blended_yaw;
        let after = heading(rebased.turn(velocity)) - aim;
        let wrap = |a: f32| (a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU);
        assert!((wrap(before) - wrap(after)).abs() < 1.0e-5);
        assert!((rebased.turn(velocity).length() - velocity.length()).abs() < 1.0e-6);
    }
}

mod hot_path {
    use std::time::Instant;

    use glam::{Quat, Vec3};
    use postretro_entities::Transform;
    use postretro_foundation::GroundRef;
    use postretro_sim::alloc_probe::AllocSnapshot;

    use crate::interpolation::{RiderBracket, RiderSample};
    use crate::rider_rebase::{PresentedMover, RiderMoverPoses, rebase_rider};

    // The counting allocator is this binary's global allocator: a real heap
    // allocation is counted (positive control for the proofs below).
    #[test]
    fn counting_allocator_counts_a_real_allocation() {
        let probe = AllocSnapshot::arm();
        let boxed = std::hint::black_box(Box::new([0u8; 64]));
        drop(boxed);
        assert!(probe.allocs_since() > 0);
    }

    const RIDERS: u32 = 32;
    const MOVERS: u32 = 8;

    // Every remote riding, across several movers: placing them allocates
    // nothing once the host's mover poses are warm, and costs well under a
    // microsecond each.
    #[test]
    fn rebasing_every_rider_allocates_nothing_per_frame() {
        let mut poses = RiderMoverPoses::default();
        let registry_entity = {
            let mut registry = postretro_entities::EntityRegistry::new();
            registry.spawn(Transform::default())
        };
        let pose = |mover: u32, tick: u32| Transform {
            position: Vec3::new(mover as f32, 0.0, tick as f32 * 0.01),
            rotation: Quat::from_rotation_y(tick as f32 * 0.02 + mover as f32),
            scale: Vec3::ONE,
        };
        for tick in 0..20 {
            for mover in 0..MOVERS {
                poses.record(mover, registry_entity, tick, pose(mover, tick));
            }
        }
        let brackets: Vec<RiderBracket> = (0..RIDERS)
            .map(|rider| {
                let mover = rider % MOVERS;
                let sample = |tick: u32| RiderSample {
                    server_tick: tick,
                    transform: pose(mover, tick),
                    ground: Some(GroundRef::Mover(mover)),
                };
                RiderBracket {
                    from: sample(16),
                    to: sample(18),
                    alpha: 0.4,
                }
            })
            .collect();
        let place = |bracket: &RiderBracket| {
            rebase_rider(
                bracket,
                &bracket.from.transform,
                |mover, tick| poses.pose_at(mover, tick),
                |mover| {
                    poses.entity(mover).map(|_| PresentedMover {
                        transform: pose(mover, 19),
                        carry_yaw: true,
                    })
                },
            )
        };
        for bracket in &brackets {
            assert!(place(bracket).is_some());
        }
        let probe = AllocSnapshot::arm();
        const FRAMES: u32 = 200;
        let started = Instant::now();
        for _ in 0..FRAMES {
            for bracket in &brackets {
                std::hint::black_box(place(bracket));
            }
        }
        let elapsed = started.elapsed();
        assert_eq!(probe.allocs_since(), 0, "rebasing allocated");
        eprintln!(
            "[riders] rebase cost: {:.2} µs per frame for {RIDERS} riders on {MOVERS} movers",
            elapsed.as_secs_f64() * 1.0e6 / f64::from(FRAMES)
        );
    }
}
