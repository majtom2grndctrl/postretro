// Emission helpers for the simulation's gameplay events. The emission types
// live in `postretro-entities` so the sim/AI seam shares one definition.
// See: context/lib/audio.md §4

use glam::Vec3;
use postretro_entities::provenance::DescriptorProvenance;
use postretro_entities::{EntityId, EntityRegistry, Transform};

pub use postretro_entities::{
    AiCue, AiEmission, ContactHit, Emitter, ImpactContact, MovementEmission, WeaponEmission,
};

/// Emitter for an entity at its current transform. An entity without one
/// (never expected for an emitter) reports the world origin.
pub fn entity_emitter(registry: &EntityRegistry, id: EntityId) -> Emitter {
    let origin = registry
        .get_component::<Transform>(id)
        .map_or(Vec3::ZERO, |transform| transform.position);
    Emitter::Entity { id, origin }
}

/// Canonical name of the descriptor a spawned entity came from, if it has one.
/// For a weapon instance this is the weapon descriptor its sounds resolve by.
pub fn descriptor_name(registry: &EntityRegistry, id: EntityId) -> Option<String> {
    registry
        .get_component::<DescriptorProvenance>(id)
        .ok()
        .map(|provenance| provenance.canonical_name.clone())
}

/// The addresses a batch of weapon emissions fires, in order.
#[cfg(test)]
pub(crate) fn weapon_addresses(emissions: &[WeaponEmission]) -> Vec<&'static str> {
    emissions.iter().map(|emission| emission.address).collect()
}
