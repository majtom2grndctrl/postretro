// Free-fly camera: position, orientation, projection, and view matrix computation.
// See: context/lib/input.md

#[cfg(test)]
use glam::Mat4;
use glam::Vec3;

/// Horizontal field of view in radians (100 degrees).
pub const HFOV: f32 = 100.0 * std::f32::consts::PI / 180.0;

pub const NEAR: f32 = 0.1;
pub const FAR: f32 = 4096.0;

/// Maximum pitch angle in radians (+/- 89 degrees from horizontal).
const PITCH_LIMIT: f32 = 89.0 * std::f32::consts::PI / 180.0;

/// Base movement speed in units per second (Quake player speed).
pub const MOVE_SPEED: f32 = 320.0;

pub const SPRINT_MULTIPLIER: f32 = 2.0;

/// Free-fly camera with Euler angle orientation and perspective projection.
pub struct Camera {
    pub position: Vec3,
    /// Yaw angle in radians (rotation around world Y axis). Zero faces -Z.
    pub yaw: f32,
    /// Pitch angle in radians (rotation around camera local X axis).
    /// Positive looks up, negative looks down.
    pub pitch: f32,
    /// Window aspect ratio (width / height).
    aspect: f32,
}

impl Camera {
    /// Create a new camera at the given position with initial orientation.
    pub fn new(position: Vec3, yaw: f32, pitch: f32) -> Self {
        Self {
            position,
            yaw,
            pitch: pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT),
            aspect: 16.0 / 9.0,
        }
    }

    /// Update the aspect ratio from window dimensions. Skips zero-sized dimensions.
    pub fn update_aspect(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.aspect = width as f32 / height as f32;
        }
    }

    /// Current aspect ratio, for use by interpolation rendering.
    pub fn aspect(&self) -> f32 {
        self.aspect
    }

    /// Apply yaw and pitch deltas from mouse input. Pitch is clamped to +/- 89 degrees.
    pub fn rotate(&mut self, yaw_delta: f32, pitch_delta: f32) {
        self.yaw += yaw_delta;
        self.pitch = (self.pitch + pitch_delta).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }

    /// Yaw-only forward vector in the XZ plane (no pitch component).
    /// Looking down doesn't slow forward movement.
    pub fn forward(&self) -> Vec3 {
        Vec3::new(-self.yaw.sin(), 0.0, -self.yaw.cos())
    }

    pub fn right(&self) -> Vec3 {
        Vec3::new(self.yaw.cos(), 0.0, -self.yaw.sin())
    }

    /// Combined view-projection matrix. Used by tests; production rendering
    /// uses `InterpolableState::view_projection` for interpolated state.
    #[cfg(test)]
    pub fn view_projection(&self) -> Mat4 {
        let view = self.view_matrix();
        let projection = self.projection_matrix();
        projection * view
    }

    #[cfg(test)]
    fn view_matrix(&self) -> Mat4 {
        let look_dir = Vec3::new(
            -self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            -self.yaw.cos() * self.pitch.cos(),
        );
        let target = self.position + look_dir;
        Mat4::look_at_rh(self.position, target, Vec3::Y)
    }

    #[cfg(test)]
    fn projection_matrix(&self) -> Mat4 {
        let vfov = 2.0 * ((HFOV / 2.0).tan() / self.aspect.max(0.001)).atan();
        Mat4::perspective_rh(vfov, self.aspect, NEAR, FAR)
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    const EPSILON: f32 = 1e-5;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < EPSILON
    }

    fn assert_vec3_approx(actual: Vec3, expected: Vec3) {
        assert!(
            approx_eq(actual.x, expected.x)
                && approx_eq(actual.y, expected.y)
                && approx_eq(actual.z, expected.z),
            "expected ({:.5}, {:.5}, {:.5}), got ({:.5}, {:.5}, {:.5})",
            expected.x,
            expected.y,
            expected.z,
            actual.x,
            actual.y,
            actual.z,
        );
    }

    // -- Construction --

    #[test]
    fn new_camera_has_correct_initial_state() {
        let cam = Camera::new(Vec3::new(1.0, 2.0, 3.0), 0.0, 0.0);
        assert_vec3_approx(cam.position, Vec3::new(1.0, 2.0, 3.0));
        assert!(approx_eq(cam.yaw, 0.0));
        assert!(approx_eq(cam.pitch, 0.0));
    }

    #[test]
    fn new_camera_clamps_excessive_pitch() {
        let cam = Camera::new(Vec3::ZERO, 0.0, PI); // 180 degrees, way over limit
        assert!(cam.pitch <= PITCH_LIMIT + EPSILON);

        let cam = Camera::new(Vec3::ZERO, 0.0, -PI);
        assert!(cam.pitch >= -PITCH_LIMIT - EPSILON);
    }

    // -- Forward vector --

    #[test]
    fn forward_at_zero_yaw_faces_negative_z() {
        let cam = Camera::new(Vec3::ZERO, 0.0, 0.0);
        assert_vec3_approx(cam.forward(), Vec3::new(0.0, 0.0, -1.0));
    }

    #[test]
    fn forward_at_90_degree_yaw_faces_negative_x() {
        let cam = Camera::new(Vec3::ZERO, PI / 2.0, 0.0);
        assert_vec3_approx(cam.forward(), Vec3::new(-1.0, 0.0, 0.0));
    }

    #[test]
    fn forward_has_no_y_component() {
        // Even with pitch, forward is always in the XZ plane.
        let cam = Camera::new(Vec3::ZERO, 0.5, 0.8);
        assert!(approx_eq(cam.forward().y, 0.0));
    }

    #[test]
    fn forward_is_unit_length() {
        let cam = Camera::new(Vec3::ZERO, 1.23, 0.45);
        assert!(approx_eq(cam.forward().length(), 1.0));
    }

    // -- Right vector --

    #[test]
    fn right_at_zero_yaw_faces_positive_x() {
        let cam = Camera::new(Vec3::ZERO, 0.0, 0.0);
        assert_vec3_approx(cam.right(), Vec3::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn right_at_90_degree_yaw() {
        let cam = Camera::new(Vec3::ZERO, PI / 2.0, 0.0);
        assert_vec3_approx(cam.right(), Vec3::new(0.0, 0.0, -1.0));
    }

    #[test]
    fn right_is_perpendicular_to_forward() {
        let cam = Camera::new(Vec3::ZERO, 1.0, 0.3);
        let dot = cam.forward().dot(cam.right());
        assert!(approx_eq(dot, 0.0));
    }

    #[test]
    fn right_is_unit_length() {
        let cam = Camera::new(Vec3::ZERO, 2.5, -0.7);
        assert!(approx_eq(cam.right().length(), 1.0));
    }

    // -- Rotation --

    #[test]
    fn rotate_applies_yaw_delta() {
        let mut cam = Camera::new(Vec3::ZERO, 0.0, 0.0);
        cam.rotate(0.5, 0.0);
        assert!(approx_eq(cam.yaw, 0.5));
    }

    #[test]
    fn rotate_applies_pitch_delta() {
        let mut cam = Camera::new(Vec3::ZERO, 0.0, 0.0);
        cam.rotate(0.0, 0.3);
        assert!(approx_eq(cam.pitch, 0.3));
    }

    #[test]
    fn rotate_clamps_pitch_at_positive_limit() {
        let mut cam = Camera::new(Vec3::ZERO, 0.0, 0.0);
        cam.rotate(0.0, PI); // Try to look past straight up
        assert!(cam.pitch <= PITCH_LIMIT + EPSILON);
    }

    #[test]
    fn rotate_clamps_pitch_at_negative_limit() {
        let mut cam = Camera::new(Vec3::ZERO, 0.0, 0.0);
        cam.rotate(0.0, -PI); // Try to look past straight down
        assert!(cam.pitch >= -PITCH_LIMIT - EPSILON);
    }

    #[test]
    fn yaw_is_unconstrained() {
        let mut cam = Camera::new(Vec3::ZERO, 0.0, 0.0);
        cam.rotate(10.0 * PI, 0.0);
        assert!(approx_eq(cam.yaw, 10.0 * PI));
    }

    // -- Aspect ratio --

    #[test]
    fn update_aspect_changes_ratio() {
        let mut cam = Camera::new(Vec3::ZERO, 0.0, 0.0);
        cam.update_aspect(1920, 1080);
        assert!(approx_eq(cam.aspect, 1920.0 / 1080.0));
    }

    #[test]
    fn update_aspect_ignores_zero_width() {
        let mut cam = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let original_aspect = cam.aspect;
        cam.update_aspect(0, 1080);
        assert!(approx_eq(cam.aspect, original_aspect));
    }

    #[test]
    fn update_aspect_ignores_zero_height() {
        let mut cam = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let original_aspect = cam.aspect;
        cam.update_aspect(1920, 0);
        assert!(approx_eq(cam.aspect, original_aspect));
    }

    // -- View-projection matrix --

    #[test]
    fn view_projection_is_finite() {
        let cam = Camera::new(Vec3::new(0.0, 200.0, 500.0), 0.0, 0.0);
        let vp = cam.view_projection();
        let cols = vp.to_cols_array();
        for (i, val) in cols.iter().enumerate() {
            assert!(val.is_finite(), "view_proj[{i}] is not finite: {val}");
        }
    }

    #[test]
    fn view_projection_changes_with_position() {
        let cam1 = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let cam2 = Camera::new(Vec3::new(100.0, 0.0, 0.0), 0.0, 0.0);
        let vp1 = cam1.view_projection();
        let vp2 = cam2.view_projection();
        assert_ne!(vp1, vp2);
    }

    #[test]
    fn view_projection_changes_with_yaw() {
        let cam1 = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let cam2 = Camera::new(Vec3::ZERO, PI / 4.0, 0.0);
        let vp1 = cam1.view_projection();
        let vp2 = cam2.view_projection();
        assert_ne!(vp1, vp2);
    }

    #[test]
    fn view_projection_changes_with_pitch() {
        let cam1 = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let cam2 = Camera::new(Vec3::ZERO, 0.0, 0.5);
        let vp1 = cam1.view_projection();
        let vp2 = cam2.view_projection();
        assert_ne!(vp1, vp2);
    }

    #[test]
    fn view_projection_changes_with_aspect() {
        let mut cam = Camera::new(Vec3::ZERO, 0.0, 0.0);
        cam.update_aspect(1920, 1080);
        let vp1 = cam.view_projection();
        cam.update_aspect(800, 600);
        let vp2 = cam.view_projection();
        assert_ne!(vp1, vp2);
    }

    // -- FOV conversion --

    #[test]
    fn vertical_fov_is_less_than_horizontal_for_widescreen() {
        // For 16:9, vertical FOV should be narrower than horizontal.
        let cam = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let vfov = 2.0 * ((HFOV / 2.0).tan() / cam.aspect).atan();
        assert!(
            vfov < HFOV,
            "vfov ({vfov}) should be less than hfov ({HFOV})"
        );
    }

    #[test]
    fn vertical_fov_equals_horizontal_for_square_aspect() {
        let mut cam = Camera::new(Vec3::ZERO, 0.0, 0.0);
        cam.update_aspect(100, 100);
        let vfov = 2.0 * ((HFOV / 2.0).tan() / cam.aspect).atan();
        assert!(approx_eq(vfov, HFOV));
    }

    // -- View matrix correctness --

    #[test]
    fn view_matrix_at_origin_looking_at_negative_z() {
        let cam = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let view = cam.view_matrix();

        // A point directly in front of the camera (negative Z) should have
        // negative Z in view space (right-handed: camera looks down -Z).
        let world_point = glam::Vec4::new(0.0, 0.0, -10.0, 1.0);
        let view_point = view * world_point;
        assert!(
            view_point.z < 0.0,
            "point in front of camera should have negative view-space Z, got {:.3}",
            view_point.z,
        );
    }

    #[test]
    fn view_matrix_with_pitch_up_sees_above() {
        let cam = Camera::new(Vec3::ZERO, 0.0, 0.5); // Looking up
        let view = cam.view_matrix();

        // A point above and in front should be visible (negative Z in view space).
        let world_point = glam::Vec4::new(0.0, 10.0, -10.0, 1.0);
        let view_point = view * world_point;
        assert!(
            view_point.z < 0.0,
            "point above and ahead should be in front of camera looking up",
        );
    }
}
