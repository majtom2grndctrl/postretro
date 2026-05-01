// Per-particle simulation state. Carried by each live particle ECS entity.
// See: context/lib/scripting.md §11 (Emitter and Particles)

use serde::{Deserialize, Serialize};

use crate::scripting::registry::EntityId;

/// Per-particle simulation state. The particle simulation reads / writes this
/// each tick. Curve data and `buoyancy` / `drag` are *copied* from the parent
/// emitter at spawn so a particle survives unchanged after its emitter
/// despawns; `emitter` is a back-reference for diagnostics only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ParticleState {
    pub(crate) velocity: [f32; 3],
    pub(crate) age: f32,
    pub(crate) lifetime: f32,
    pub(crate) buoyancy: f32,
    pub(crate) drag: f32,
    pub(crate) size_curve: Vec<f32>,
    pub(crate) opacity_curve: Vec<f32>,
    pub(crate) emitter: Option<EntityId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn particle_state_serde_round_trip_with_curves_and_emitter_ref() {
        let value = ParticleState {
            velocity: [0.5, 1.5, -0.25],
            age: 0.4,
            lifetime: 2.5,
            buoyancy: -1.0,
            drag: 0.3,
            size_curve: vec![0.2, 1.0, 0.5],
            opacity_curve: vec![0.0, 1.0, 0.0],
            emitter: Some(EntityId::from_raw(0x0001_0002)),
        };
        let json = serde_json::to_string(&value).unwrap();
        let back: ParticleState = serde_json::from_str(&json).unwrap();
        assert_eq!(value, back);
    }

    #[test]
    fn particle_state_serde_round_trip_with_no_emitter_back_reference() {
        let value = ParticleState {
            velocity: [0.0, 0.0, 0.0],
            age: 0.0,
            lifetime: 1.0,
            buoyancy: 0.0,
            drag: 0.0,
            size_curve: vec![1.0],
            opacity_curve: vec![1.0],
            emitter: None,
        };
        let json = serde_json::to_string(&value).unwrap();
        let back: ParticleState = serde_json::from_str(&json).unwrap();
        assert_eq!(value, back);
    }
}
