// Per-particle simulation state. Carried by each live particle ECS entity.
// See: context/lib/scripting.md §10.1 (Emitter and Particles)

use serde::{Deserialize, Serialize};

use crate::scripting::components::billboard_emitter::LifetimeCurve;
use crate::scripting::registry::EntityId;

/// Per-particle simulation state. The particle simulation reads / writes this
/// each tick. `buoyancy` / `drag` are copied from the parent emitter at spawn;
/// the size / opacity curves are *shared* via a cheap `Arc<[f32]>` handle
/// ([`LifetimeCurve`]) rather than deep-cloned, so spawning a particle and
/// snapshotting it each tick only bump a refcount. Curves are immutable once
/// authored (a reaction that changes one installs a fresh `Arc`), so a particle
/// survives unchanged after its emitter despawns; `emitter` is a back-reference
/// to the parent emitter entity whose **only** runtime role is spin-rate lookup
/// in the sim tick — it is **not** consulted for render-collect culling. Each
/// billboard is culled by the BSP leaf of *its own* world position (see
/// `scripting/systems/particle_render.rs`). When the emitter has despawned the
/// back-reference is stale, and the orphaned particle is culled or drawn by its
/// own leaf exactly like any other particle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ParticleState {
    pub(crate) velocity: [f32; 3],
    pub(crate) age: f32,
    pub(crate) lifetime: f32,
    pub(crate) buoyancy: f32,
    pub(crate) drag: f32,
    pub(crate) size_curve: LifetimeCurve,
    pub(crate) opacity_curve: LifetimeCurve,
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
            size_curve: [0.2, 1.0, 0.5].into(),
            opacity_curve: [0.0, 1.0, 0.0].into(),
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
            size_curve: [1.0].into(),
            opacity_curve: [1.0].into(),
            emitter: None,
        };
        let json = serde_json::to_string(&value).unwrap();
        let back: ParticleState = serde_json::from_str(&json).unwrap();
        assert_eq!(value, back);
    }
}
