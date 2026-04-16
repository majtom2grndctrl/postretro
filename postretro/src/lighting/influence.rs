// Per-light influence volumes: runtime struct, GPU packing, and CPU
// sphere-vs-frustum test for shadow-slot allocation.
// See: context/plans/in-progress/lighting-foundation/4-light-influence-volumes.md

use glam::Vec3;

use crate::visibility::Frustum;

/// Runtime influence volume for one light. Deserialized from the
/// `LightInfluence` PRL section (ID 21).
#[derive(Debug, Clone)]
pub struct LightInfluence {
    /// World-space sphere center. Unused for directional lights.
    pub center: Vec3,
    /// Sphere radius in meters. `f32::MAX` = always active (directional).
    pub radius: f32,
}

/// Infinity sentinel threshold. The compiler writes `f32::MAX` (~3.4e38)
/// for directional lights; the shader and CPU test both use `> 1.0e30` as
/// the "skip spatial test" signal. Any physical falloff range is well under
/// this threshold.
pub const INFINITY_THRESHOLD: f32 = 1.0e30;

/// Pack influence records into a contiguous `[f32; 4]` array suitable for
/// GPU upload as `array<vec4<f32>>`.
pub fn pack_influence(records: &[LightInfluence]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(records.len() * 16);
    for r in records {
        bytes.extend_from_slice(&r.center.x.to_ne_bytes());
        bytes.extend_from_slice(&r.center.y.to_ne_bytes());
        bytes.extend_from_slice(&r.center.z.to_ne_bytes());
        bytes.extend_from_slice(&r.radius.to_ne_bytes());
    }
    bytes
}

/// Test each light's influence volume against the camera frustum and return
/// the indices of lights whose volumes intersect (i.e., are potentially
/// visible this frame). Directional lights (radius > INFINITY_THRESHOLD)
/// are always included.
///
/// Uses the inward-normal `FrustumPlane` convention from `visibility.rs`:
/// `plane.normal.dot(center) + plane.dist < -radius` means the sphere is
/// fully outside that plane.
pub fn visible_lights(influences: &[LightInfluence], frustum: &Frustum) -> Vec<u32> {
    let mut result = Vec::new();
    for (i, influence) in influences.iter().enumerate() {
        if light_affects_frame(influence, frustum) {
            result.push(i as u32);
        }
    }
    result
}

fn light_affects_frame(light: &LightInfluence, frustum: &Frustum) -> bool {
    if light.radius > INFINITY_THRESHOLD {
        return true; // directional lights always contribute
    }
    for plane in &frustum.planes {
        if plane.normal.dot(light.center) + plane.dist < -light.radius {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a 6-plane frustum looking down -Z from `position`.
    fn test_frustum(position: Vec3) -> Frustum {
        use glam::Mat4;
        let view = Mat4::look_at_rh(position, position + Vec3::NEG_Z, Vec3::Y);
        let proj = Mat4::perspective_rh(
            std::f32::consts::FRAC_PI_2,
            16.0 / 9.0,
            0.1,
            4096.0,
        );
        crate::visibility::extract_frustum_planes(proj * view)
    }

    #[test]
    fn light_at_camera_origin_is_visible() {
        let frustum = test_frustum(Vec3::ZERO);
        let influences = vec![LightInfluence {
            center: Vec3::new(0.0, 0.0, -5.0),
            radius: 10.0,
        }];
        let result = visible_lights(&influences, &frustum);
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn light_behind_far_plane_is_not_visible() {
        let frustum = test_frustum(Vec3::ZERO);
        let influences = vec![LightInfluence {
            center: Vec3::new(0.0, 0.0, -5000.0),
            radius: 10.0,
        }];
        let result = visible_lights(&influences, &frustum);
        assert!(result.is_empty());
    }

    #[test]
    fn light_straddling_side_plane_is_visible() {
        let frustum = test_frustum(Vec3::ZERO);
        // Large sphere that straddles the left edge of the frustum
        let influences = vec![LightInfluence {
            center: Vec3::new(-50.0, 0.0, -50.0),
            radius: 100.0,
        }];
        let result = visible_lights(&influences, &frustum);
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn directional_light_always_visible() {
        let frustum = test_frustum(Vec3::ZERO);
        let influences = vec![LightInfluence {
            center: Vec3::ZERO,
            radius: f32::MAX,
        }];
        let result = visible_lights(&influences, &frustum);
        assert_eq!(result, vec![0]);
    }

    #[test]
    fn light_behind_camera_is_not_visible() {
        let frustum = test_frustum(Vec3::ZERO);
        let influences = vec![LightInfluence {
            center: Vec3::new(0.0, 0.0, 50.0),
            radius: 5.0,
        }];
        let result = visible_lights(&influences, &frustum);
        assert!(result.is_empty());
    }

    #[test]
    fn mixed_lights_filters_correctly() {
        let frustum = test_frustum(Vec3::ZERO);
        let influences = vec![
            // Visible: in front of camera
            LightInfluence {
                center: Vec3::new(0.0, 0.0, -10.0),
                radius: 20.0,
            },
            // Not visible: behind camera
            LightInfluence {
                center: Vec3::new(0.0, 0.0, 100.0),
                radius: 5.0,
            },
            // Always visible: directional
            LightInfluence {
                center: Vec3::ZERO,
                radius: f32::MAX,
            },
        ];
        let result = visible_lights(&influences, &frustum);
        assert_eq!(result, vec![0, 2]);
    }

    #[test]
    fn pack_influence_produces_correct_bytes() {
        let records = vec![LightInfluence {
            center: Vec3::new(1.0, 2.0, 3.0),
            radius: 10.0,
        }];
        let bytes = pack_influence(&records);
        assert_eq!(bytes.len(), 16);
        let x = f32::from_ne_bytes(bytes[0..4].try_into().unwrap());
        let y = f32::from_ne_bytes(bytes[4..8].try_into().unwrap());
        let z = f32::from_ne_bytes(bytes[8..12].try_into().unwrap());
        let r = f32::from_ne_bytes(bytes[12..16].try_into().unwrap());
        assert_eq!(x, 1.0);
        assert_eq!(y, 2.0);
        assert_eq!(z, 3.0);
        assert_eq!(r, 10.0);
    }
}
