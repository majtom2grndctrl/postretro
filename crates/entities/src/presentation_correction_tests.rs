use glam::{Quat, Vec3};

use super::PoseCorrection;
use crate::{EntityId, EntityRegistry, Transform};

const EPS: f32 = 1.0e-5;
const ALPHAS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];
/// A point out on the deck, so rotation errors show as distance.
const DECK_POINT: Vec3 = Vec3::new(2.0, 0.5, -1.0);

fn pose(x: f32, yaw: f32) -> Transform {
    Transform {
        position: Vec3::new(x, 0.0, 0.0),
        rotation: Quat::from_rotation_y(yaw),
        scale: Vec3::ONE,
    }
}

fn deck(transform: &Transform) -> Vec3 {
    transform.position + transform.rotation * (transform.scale * DECK_POINT)
}

fn blend(previous: &Transform, current: &Transform, alpha: f32) -> Transform {
    Transform {
        position: previous.position.lerp(current.position, alpha),
        rotation: previous.rotation.slerp(current.rotation, alpha),
        scale: previous.scale.lerp(current.scale, alpha),
    }
}

/// A mover mid-tick: previous at the tick start, current at its end.
fn mover() -> (EntityRegistry, EntityId) {
    let mut registry = EntityRegistry::new();
    let id = registry.spawn(pose(0.0, 0.0));
    registry.snapshot_transforms();
    registry.set_component(id, pose(0.1, 0.05)).unwrap();
    (registry, id)
}

fn assert_presents(registry: &EntityRegistry, id: EntityId, expected: impl Fn(f32) -> Transform) {
    for alpha in ALPHAS {
        let presented = registry.interpolated_transform(id, alpha).unwrap();
        let expected = expected(alpha);
        assert!(
            (deck(&presented) - deck(&expected)).length() < EPS
                && (presented.position - expected.position).length() < EPS,
            "alpha {alpha}: presented {presented:?} expected {expected:?}"
        );
    }
}

/// Advance one fixed tick on both the registry and a reference path: the
/// mover moves 0.1 m and 0.05 rad from wherever it is.
fn tick(registry: &mut EntityRegistry, id: EntityId) {
    registry.snapshot_transforms();
    let current = *registry.get_component::<Transform>(id).unwrap();
    registry.set_component(id, step(&current)).unwrap();
}

fn step(transform: &Transform) -> Transform {
    Transform {
        position: transform.position + Vec3::new(0.1, 0.0, 0.0),
        rotation: Quat::from_rotation_y(0.05) * transform.rotation,
        scale: transform.scale,
    }
}

#[test]
fn gliding_correction_presents_the_uncorrected_pose_at_every_alpha_and_across_the_next_tick() {
    let (mut registry, id) = mover();
    let before_previous = pose(0.0, 0.0);
    let before_current = pose(0.1, 0.05);
    registry
        .correct_transform(id, pose(0.4, 0.35), true)
        .unwrap();
    // Sim truth takes the correction at once.
    assert_eq!(
        *registry.get_component::<Transform>(id).unwrap(),
        pose(0.4, 0.35)
    );
    // Zero-tick frame: the drawn pose has not moved (pin P1).
    assert_presents(&registry, id, |alpha| {
        blend(&before_previous, &before_current, alpha)
    });
    // First ticking frame: still the old path plus its own motion.
    tick(&mut registry, id);
    let next = step(&before_current);
    assert_presents(&registry, id, |alpha| blend(&before_current, &next, alpha));
}

#[test]
fn two_corrections_on_one_frame_compose() {
    let (mut registry, id) = mover();
    registry
        .correct_transform(id, pose(0.4, 0.35), true)
        .unwrap();
    registry
        .correct_transform(id, pose(-0.2, -0.4), true)
        .unwrap();
    assert_presents(&registry, id, |alpha| {
        blend(&pose(0.0, 0.0), &pose(0.1, 0.05), alpha)
    });
}

#[test]
fn glide_converges_onto_the_corrected_path_without_drift() {
    let (mut registry, id) = mover();
    registry
        .correct_transform(id, pose(0.4, 0.35), true)
        .unwrap();
    // The first advance only marks the seeded correction as drawn.
    registry.advance_presentation_corrections(0.9, 1.0e-6);
    let seeded = registry.presentation_correction(id).unwrap();
    registry.advance_presentation_corrections(0.9, 1.0e-6);
    let shrunk = registry.presentation_correction(id).unwrap();
    assert!((shrunk.translation - seeded.translation * 0.9).length() < 1.0e-6);
    for _ in 0..400 {
        registry.advance_presentation_corrections(0.9, 1.0e-6);
    }
    assert_eq!(registry.presentation_correction(id), None);
    assert!(!registry.has_presentation_corrections());
    let corrected = PoseCorrection::between(&pose(0.1, 0.05), &pose(0.4, 0.35));
    assert_presents(&registry, id, |alpha| {
        corrected.apply(&blend(&pose(0.0, 0.0), &pose(0.1, 0.05), alpha))
    });
}

// Pin P18: a snap lands whole on the frame it is applied, at any alpha, even on
// a frame that runs no fixed tick, and drops a glide already in flight.
#[test]
fn snapping_correction_lands_whole_at_every_alpha_and_drops_the_glide() {
    let (mut registry, id) = mover();
    registry
        .correct_transform(id, pose(0.4, 0.35), true)
        .unwrap();
    let snap = registry
        .correct_transform(id, pose(3.0, 1.5), false)
        .unwrap();
    assert_eq!(registry.presentation_correction(id), None);
    let first = PoseCorrection::between(&pose(0.1, 0.05), &pose(0.4, 0.35));
    assert_presents(&registry, id, |alpha| {
        snap.apply(&first.apply(&blend(&pose(0.0, 0.0), &pose(0.1, 0.05), alpha)))
    });
    let at_one = registry.interpolated_transform(id, 1.0).unwrap();
    assert!((deck(&at_one) - deck(&pose(3.0, 1.5))).length() < EPS);
}

#[test]
fn despawn_clear_and_non_finite_input_drop_the_glide() {
    let (mut registry, id) = mover();
    registry
        .correct_transform(id, pose(0.4, 0.35), true)
        .unwrap();
    registry.clear_presentation_correction(id);
    assert_eq!(registry.presentation_correction(id), None);

    registry
        .correct_transform(id, pose(0.6, 0.35), true)
        .unwrap();
    let truth = *registry.get_component::<Transform>(id).unwrap();
    let broken = Transform {
        position: Vec3::new(f32::NAN, 0.0, 0.0),
        ..pose(0.0, 0.0)
    };
    registry.correct_transform(id, broken, true).unwrap();
    assert_eq!(registry.presentation_correction(id), None);
    assert_eq!(*registry.get_component::<Transform>(id).unwrap(), truth);

    registry
        .correct_transform(id, pose(0.9, 0.35), true)
        .unwrap();
    let other = registry.spawn(pose(5.0, 0.0));
    registry.despawn(id).unwrap();
    assert!(!registry.has_presentation_corrections());
    assert_eq!(
        registry.interpolated_transform(other, 0.5).unwrap(),
        pose(5.0, 0.0)
    );
}

#[test]
fn entities_without_a_correction_present_their_blend_bit_for_bit() {
    let (mut registry, id) = mover();
    let other = registry.spawn(pose(5.0, 0.0));
    registry.snapshot_transforms();
    registry.set_component(other, pose(5.5, 0.3)).unwrap();
    let before: Vec<Transform> = ALPHAS
        .iter()
        .map(|alpha| registry.interpolated_transform(other, *alpha).unwrap())
        .collect();
    registry
        .correct_transform(id, pose(0.4, 0.35), true)
        .unwrap();
    let after: Vec<Transform> = ALPHAS
        .iter()
        .map(|alpha| registry.interpolated_transform(other, *alpha).unwrap())
        .collect();
    assert_eq!(before, after);
}
