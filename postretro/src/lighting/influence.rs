// Per-light influence volumes: runtime struct and GPU packing.
// See: context/lib/rendering_pipeline.md §4

use glam::Vec3;

/// Runtime influence volume for one light. Deserialized from the
/// `LightInfluence` PRL section (ID 21).
#[derive(Debug, Clone)]
pub struct LightInfluence {
    /// World-space sphere center. Unused for directional lights.
    pub center: Vec3,
    /// Sphere radius in meters. `f32::MAX` = always active (directional).
    pub radius: f32,
}

impl LightInfluence {
    /// Check if a point (e.g., camera) is within the influence volume.
    /// For directional lights (radius == f32::MAX), always returns true.
    #[allow(dead_code)]
    pub fn is_in_frustum_approx(&self, point: Vec3) -> bool {
        if self.radius >= f32::MAX / 2.0 {
            // Directional light: always in scope.
            return true;
        }
        let dist_sq = (self.center - point).length_squared();
        dist_sq <= self.radius * self.radius
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

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
