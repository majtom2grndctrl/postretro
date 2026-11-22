//! The local eye rides the drawn deck: both tick endpoints on one mover blend
//! in its frame and place through its presented pose; anything else blends in
//! world space exactly as before.

use glam::{Quat, Vec3};
use postretro_entities::{EntityId, EntityRegistry, Transform};

use super::{EyeBody, EyeMover, InterpolableState, present_eye};

const EYE_HEIGHT: f32 = 0.7;
const RADIUS: f32 = 4.0;
const LIFT: u32 = 2;

fn mover_entity() -> EntityId {
    let mut registry = EntityRegistry::new();
    registry.spawn(Transform::default())
}

fn turned(yaw: f32) -> Transform {
    Transform {
        rotation: Quat::from_rotation_y(yaw),
        ..Transform::default()
    }
}

/// An endpoint for a pawn planted `RADIUS` out on a mover at `pose`.
fn riding(entity: EntityId, pose: Transform, offset: Vec3) -> InterpolableState {
    let centre = pose.position + pose.rotation * Vec3::new(RADIUS, 1.0, 0.0);
    InterpolableState::following(
        centre + offset + Vec3::Y * EYE_HEIGHT,
        EyeBody {
            centre,
            offset,
            mover: Some(EyeMover {
                mover_id: LIFT,
                entity,
                pose,
            }),
        },
    )
}

fn slerped(from: &Transform, to: &Transform, alpha: f32) -> Transform {
    Transform {
        position: from.position.lerp(to.position, alpha),
        rotation: from.rotation.slerp(to.rotation, alpha),
        scale: Vec3::ONE,
    }
}

fn horizontal(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

// A quarter turn in one tick draws the eye on the arc, not the chord, and keeps
// its distance from the axis at every alpha (single-player, host and client
// share this placement).
#[test]
fn quarter_turn_draws_the_arc_midpoint_and_keeps_its_radius() {
    let entity = mover_entity();
    let (from, to) = (turned(0.0), turned(std::f32::consts::FRAC_PI_2));
    let previous = riding(entity, from, Vec3::ZERO);
    let current = riding(entity, to, Vec3::ZERO);
    for alpha in [0.0, 0.1, 0.25, 0.5, 0.75, 1.0] {
        let eye = present_eye(&previous, &current, alpha, |_| {
            Some(slerped(&from, &to, alpha))
        });
        assert!(eye.riding.is_some());
        assert!(
            (horizontal(eye.eye).length() - RADIUS).abs() < 1.0e-4,
            "alpha {alpha}: radius {}",
            horizontal(eye.eye).length()
        );
    }
    let mid = present_eye(&previous, &current, 0.5, |_| Some(slerped(&from, &to, 0.5)));
    let arc = Quat::from_rotation_y(std::f32::consts::FRAC_PI_4) * Vec3::new(RADIUS, 0.0, 0.0);
    assert!(
        (horizontal(mid.eye) - arc).length() < 1.0e-4,
        "{:?}",
        mid.eye
    );
    let chord = horizontal(previous.position.lerp(current.position, 0.5));
    assert!((horizontal(mid.eye) - chord).length() > 1.0);
}

// Mover pitch and roll never tilt the eye: it sits exactly eye height above the
// placed capsule centre, along world-up.
#[test]
fn pitched_or_rolled_mover_keeps_eye_height_along_world_up() {
    let entity = mover_entity();
    let tilted = Transform {
        rotation: Quat::from_rotation_x(0.3) * Quat::from_rotation_z(-0.2),
        ..Transform::default()
    };
    let endpoint = riding(entity, tilted, Vec3::ZERO);
    let eye = present_eye(&endpoint, &riding(entity, tilted, Vec3::ZERO), 0.5, |_| {
        Some(tilted)
    });
    let body = eye.body.unwrap();
    assert!((eye.eye - (body + Vec3::Y * EYE_HEIGHT)).length() < 1.0e-5);
}

// Pin P17: the world offset is added along world axes after placement, never
// turned by the mover's spin.
#[test]
fn world_offset_rides_world_axes_while_the_deck_turns() {
    let entity = mover_entity();
    let offset = Vec3::new(0.05, 0.0, -0.02);
    let (from, to) = (turned(0.0), turned(0.6));
    let previous = riding(entity, from, offset);
    let current = riding(entity, to, offset);
    let eye = present_eye(&previous, &current, 0.5, |_| Some(slerped(&from, &to, 0.5)));
    let on_deck = slerped(&from, &to, 0.5).rotation * Vec3::new(RADIUS, 1.0, 0.0);
    assert!((eye.eye - (on_deck + offset + Vec3::Y * EYE_HEIGHT)).length() < 1.0e-5);
    assert!((eye.body.unwrap() - (on_deck + offset)).length() < 1.0e-5);
}

// Pin P19 and the transitions: landing, leaving, changing movers, a hold, a
// world-only eye and a missing mover pose each draw exactly today's blend.
#[test]
fn intervals_that_do_not_share_a_mover_draw_the_world_blend_bit_for_bit() {
    let entity = mover_entity();
    let other = {
        let mut registry = EntityRegistry::new();
        registry.spawn(Transform::default());
        registry.spawn(Transform::default())
    };
    let on = |yaw: f32| riding(entity, turned(yaw), Vec3::ZERO);
    let off = |state: InterpolableState| {
        let body = state.body.map(|body| EyeBody {
            mover: None,
            ..body
        });
        InterpolableState {
            position: state.position,
            body,
        }
    };
    let changed = |state: InterpolableState| {
        let body = state.body.map(|body| EyeBody {
            mover: body.mover.map(|mover| EyeMover {
                mover_id: LIFT + 1,
                entity: other,
                ..mover
            }),
            ..body
        });
        InterpolableState {
            position: state.position,
            body,
        }
    };
    let cases: [(&str, InterpolableState, InterpolableState); 6] = [
        ("landing", off(on(0.0)), on(0.2)),
        ("leaving", on(0.0), off(on(0.2))),
        ("mover change", on(0.0), changed(on(0.2))),
        ("hold", InterpolableState::new(on(0.0).position), on(0.2)),
        (
            "fly camera",
            InterpolableState::new(Vec3::ONE),
            InterpolableState::new(Vec3::X),
        ),
        (
            "no pawn either end",
            InterpolableState::new(on(0.0).position),
            InterpolableState::new(on(0.2).position),
        ),
    ];
    for (name, previous, current) in cases {
        for alpha in [0.0, 0.3, 1.0] {
            let eye = present_eye(&previous, &current, alpha, |_| Some(turned(5.0)));
            assert_eq!(
                eye.eye,
                previous.lerp(&current, alpha).position,
                "{name} alpha {alpha}"
            );
            assert!(eye.riding.is_none(), "{name}");
        }
    }
    // A mover with no presented pose this frame falls back too.
    let (previous, current) = (on(0.0), on(0.2));
    let eye = present_eye(&previous, &current, 0.5, |_| None);
    assert_eq!(eye.eye, previous.lerp(&current, 0.5).position);
    assert!(eye.riding.is_none());
}

// A pushed interval keeps each endpoint's attachment: push_state moves the
// whole endpoint, while a hold never attaches.
#[test]
fn push_keeps_the_attachment_and_hold_drops_it() {
    let entity = mover_entity();
    let mut timing = super::FrameTiming::new(InterpolableState::new(Vec3::ZERO));
    timing.push_state(riding(entity, turned(0.0), Vec3::ZERO));
    timing.push_state(riding(entity, turned(0.3), Vec3::ZERO));
    assert!(
        timing
            .previous_state
            .body
            .is_some_and(|body| body.mover.is_some())
    );
    assert!(timing.presented_eye(|_| Some(turned(0.1))).riding.is_some());
    timing.hold_state(InterpolableState::new(Vec3::ONE));
    assert!(timing.previous_state.body.is_none());
    assert!(timing.presented_eye(|_| Some(turned(0.1))).riding.is_none());
}
