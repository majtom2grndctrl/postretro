// Runtime light influence volumes shared by the PRL loader and renderer.
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
