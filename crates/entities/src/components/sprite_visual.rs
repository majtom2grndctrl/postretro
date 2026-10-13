// Visual descriptor for individually addressed sprite entities, such as projectiles.
// Anonymous particles live in the presentation pool instead.
// See: context/lib/entity_model.md §8

use serde::{Deserialize, Serialize};

/// Per-frame visual state of a sprite entity, authored by projectile presentation
/// and consumed by the billboard render integration. `tint` is
/// CPU-side only at this stage — the GPU `SpriteInstance` layout has no color
/// channel yet (documented as a non-goal in the plan).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpriteVisual {
    /// Render collection id, derived from an asset reference and draw contract.
    /// It is not a texture path; the renderer resolves the asset at level load.
    pub collection: String,
    pub size: f32,
    pub opacity: f32,
    pub rotation: f32,
    pub tint: [f32; 3],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprite_visual_serde_round_trip() {
        let value = SpriteVisual {
            collection: "smoke|sprite-collection-v1|lifetime=none|frame_duration_ms=none|emissive=bits:00000000|spec_intensity=none|spec_exponent=none".into(),
            size: 1.25,
            opacity: 0.5,
            rotation: 0.75,
            tint: [1.0, 0.6, 0.2],
        };
        let json = serde_json::to_string(&value).unwrap();
        assert!(json.contains("\"collection\""));
        assert!(!json.contains("\"sprite\""));
        let back: SpriteVisual = serde_json::from_str(&json).unwrap();
        assert_eq!(value, back);
    }
}
