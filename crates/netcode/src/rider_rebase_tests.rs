use glam::{Quat, Vec3};
use postretro_entities::Transform;
use postretro_foundation::GroundRef;

use super::rebase_rider;
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

fn presented(mover_id: u32) -> Option<Transform> {
    (mover_id == MOVER).then_some(PRESENTED)
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
        (placed.position - expected).length() < 1.0e-5,
        "placed {:?} expected {expected:?}",
        placed.position
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
    assert_eq!(placed.rotation, world.rotation);
    assert_eq!(placed.scale, world.scale);
}

#[test]
fn rebase_attaches_only_when_both_samples_name_one_mover() {
    let cases = [
        (Some(GroundRef::Mover(MOVER)), Some(GroundRef::Mover(MOVER + 1))),
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
    assert_eq!(rebase_rider(&bracket, &world, missing_from, presented), None);
    assert_eq!(rebase_rider(&bracket, &world, missing_to, presented), None);
    assert_eq!(rebase_rider(&bracket, &world, authoritative, |_| None), None);
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
            rebase_rider(&bracket, &world, authoritative, |_| Some(frame)),
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
    assert!((placed.position - point_on(&PRESENTED, LOCAL_POINT)).length() < 1.0e-5);
}
