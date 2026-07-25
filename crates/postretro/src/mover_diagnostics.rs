//! App-side debug geometry for rotating kinematic movers.
//!
//! The frame loop owns debug-line lifetime. This module only reads mover state
//! and appends geometry while the existing dev diagnostics panel is visible.

use glam::{Quat, Vec3};
use postretro_entities::{
    ComponentKind, ComponentValue, EntityRegistry, KinematicMoverComponent, Transform,
};

use crate::render::Renderer;

const AXIS_HALF_LENGTH_M: f32 = 0.4;
const ORIENTATION_LENGTH_M: f32 = 0.32;
const COLOR_SPIN_AXIS: [u8; 4] = [255, 220, 80, 255];
const COLOR_LOCAL_X: [u8; 4] = [255, 90, 90, 255];
const COLOR_LOCAL_Z: [u8; 4] = [90, 180, 255, 255];

#[derive(Debug, Clone, Copy, PartialEq)]
struct MoverOverlaySegment {
    start: Vec3,
    end: Vec3,
    color: [u8; 4],
}

/// Collect spin-axis and current-orientation lines for rotating movers only.
///
/// A zero axis denotes a linear-only mover, so it has no angular diagnostic to
/// draw. The local X/Z arms reveal the transform orientation around every
/// supported spin axis.
fn mover_overlay_segments(
    transform: Transform,
    mover: &KinematicMoverComponent,
) -> Vec<MoverOverlaySegment> {
    let axis = mover.spin_axis.normalize_or_zero();
    if axis == Vec3::ZERO || !transform.position.is_finite() {
        return Vec::new();
    }

    let rotation = normalized_rotation(transform.rotation);
    let local_x = rotation * Vec3::X;
    let local_z = rotation * Vec3::Z;
    vec![
        MoverOverlaySegment {
            start: transform.position - axis * AXIS_HALF_LENGTH_M,
            end: transform.position + axis * AXIS_HALF_LENGTH_M,
            color: COLOR_SPIN_AXIS,
        },
        MoverOverlaySegment {
            start: transform.position,
            end: transform.position + local_x * ORIENTATION_LENGTH_M,
            color: COLOR_LOCAL_X,
        },
        MoverOverlaySegment {
            start: transform.position,
            end: transform.position + local_z * ORIENTATION_LENGTH_M,
            color: COLOR_LOCAL_Z,
        },
    ]
}

fn normalized_rotation(rotation: Quat) -> Quat {
    (rotation.is_finite() && rotation.length_squared() > f32::EPSILON)
        .then_some(rotation.normalize())
        .unwrap_or(Quat::IDENTITY)
}

/// Append current rotating-mover diagnostics to the established debug-line
/// buffer. The caller applies the existing dev-overlay visibility gate.
pub(crate) fn emit(renderer: &mut Renderer, registry: &EntityRegistry) {
    for (entity, value) in registry.iter_with_kind(ComponentKind::KinematicMover) {
        let ComponentValue::KinematicMover(mover) = value else {
            continue;
        };
        let Ok(transform) = registry.get_component::<Transform>(entity) else {
            continue;
        };
        for segment in mover_overlay_segments(*transform, mover) {
            renderer.push_debug_line(segment.start, segment.end, segment.color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_entities::KinematicMoverMode;

    fn rotating_mover(axis: Vec3) -> KinematicMoverComponent {
        KinematicMoverComponent::new(
            7,
            vec![Vec3::ZERO],
            vec!["carousel".to_string()],
            1.0,
            0.0,
            KinematicMoverMode::Once,
            true,
            axis,
            1.0,
            0.0,
            true,
        )
    }

    fn assert_vec3_near(actual: Vec3, expected: Vec3) {
        const EPSILON: f32 = 1.0e-6;
        assert!(
            (actual - expected).abs().cmple(Vec3::splat(EPSILON)).all(),
            "expected {actual:?} to be within {EPSILON} of {expected:?}",
        );
    }

    #[test]
    fn overlay_geometry_draws_spin_axis_and_current_orientation() {
        let transform = Transform {
            position: Vec3::new(2.0, 3.0, 4.0),
            rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            scale: Vec3::ONE,
        };
        let segments = mover_overlay_segments(transform, &rotating_mover(Vec3::Y));

        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0].color, COLOR_SPIN_AXIS);
        assert_vec3_near(segments[0].start, Vec3::new(2.0, 2.6, 4.0));
        assert_vec3_near(segments[0].end, Vec3::new(2.0, 3.4, 4.0));
        assert_eq!(segments[1].color, COLOR_LOCAL_X);
        assert_vec3_near(segments[1].end, Vec3::new(2.0, 3.0, 3.68));
        assert_eq!(segments[2].color, COLOR_LOCAL_Z);
        assert_vec3_near(segments[2].end, Vec3::new(2.32, 3.0, 4.0));
    }

    #[test]
    fn overlay_geometry_skips_linear_only_mover() {
        assert!(
            mover_overlay_segments(Transform::default(), &rotating_mover(Vec3::ZERO)).is_empty()
        );
    }
}
