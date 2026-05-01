// Per-particle visual descriptor. Distinct from `ParticleState` so future
// non-particle sprite entities can reuse it.
// See: context/lib/scripting.md §11 (Emitter and Particles)

use serde::{Deserialize, Serialize};

/// Per-frame visual state of a sprite. Authored by the particle simulation
/// each tick and consumed by the billboard render integration. `tint` is
/// CPU-side only at this stage — the GPU `SpriteInstance` layout has no color
/// channel yet (documented as a non-goal in the plan).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SpriteVisual {
    pub(crate) sprite: String,
    pub(crate) size: f32,
    pub(crate) opacity: f32,
    pub(crate) rotation: f32,
    pub(crate) tint: [f32; 3],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprite_visual_serde_round_trip() {
        let value = SpriteVisual {
            sprite: "smoke".into(),
            size: 1.25,
            opacity: 0.5,
            rotation: 0.75,
            tint: [1.0, 0.6, 0.2],
        };
        let json = serde_json::to_string(&value).unwrap();
        let back: SpriteVisual = serde_json::from_str(&json).unwrap();
        assert_eq!(value, back);
    }
}
