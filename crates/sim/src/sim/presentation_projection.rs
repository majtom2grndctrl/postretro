//! CPU projection for world-anchored presentation instances.

use glam::{Mat4, Vec2, Vec3};

/// Projects a world-space position into device pixels within the viewport.
///
/// Positions behind the camera, outside the view frustum, or with non-finite
/// projection values have no usable screen-space anchor.
pub(crate) fn project_world_to_screen(
    world_position: Vec3,
    view_projection: Mat4,
    viewport_size: [u32; 2],
) -> Option<Vec2> {
    let [width, height] = viewport_size;
    if width == 0 || height == 0 {
        return None;
    }

    let clip = view_projection * world_position.extend(1.0);
    if clip.w <= 0.0 || !clip.is_finite() {
        return None;
    }

    let ndc = clip.truncate() / clip.w;
    if !ndc.is_finite()
        || ndc.x < -1.0
        || ndc.x > 1.0
        || ndc.y < -1.0
        || ndc.y > 1.0
        || ndc.z < 0.0
        || ndc.z > 1.0
    {
        return None;
    }

    Some(Vec2::new(
        (ndc.x + 1.0) * 0.5 * width as f32,
        (1.0 - ndc.y) * 0.5 * height as f32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 1.0e-4;

    fn camera_view_projection() -> Mat4 {
        Mat4::perspective_rh(90.0_f32.to_radians(), 4.0 / 3.0, 0.1, 100.0)
            * Mat4::look_at_rh(Vec3::ZERO, -Vec3::Z, Vec3::Y)
    }

    #[test]
    fn projector_maps_in_front_point_to_device_pixels() {
        let screen = project_world_to_screen(
            Vec3::new(0.0, 0.0, -2.0),
            camera_view_projection(),
            [800, 600],
        )
        .expect("point in front of the camera should project into the viewport");

        assert!((screen.x - 400.0).abs() < EPSILON);
        assert!((screen.y - 300.0).abs() < EPSILON);
    }

    #[test]
    fn projector_culls_behind_and_offscreen_points() {
        let view_projection = camera_view_projection();

        assert_eq!(
            project_world_to_screen(Vec3::new(0.0, 0.0, 2.0), view_projection, [800, 600]),
            None
        );
        assert_eq!(
            project_world_to_screen(Vec3::new(100.0, 0.0, -2.0), view_projection, [800, 600]),
            None
        );
    }
}
