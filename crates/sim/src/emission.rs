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

/// Append one movement tick's emissions on its pawn, in dispatch order. Each
/// swim edge carries the tick's swim fluid; one tick's swim edges always share
/// it, since a swim state never changes fluid. Shared by the host or offline
/// local tick and a client's forward prediction, the only two routes into the
/// movement drain, so a reconcile replay can never emit.
pub fn append_movement_emissions(
    events: &crate::movement::MovementEvents,
    emitter: &Emitter,
    out: &mut Vec<MovementEmission>,
) {
    let mut addresses = Vec::new();
    events.append_named_events(&mut addresses);
    out.extend(addresses.into_iter().map(|address| {
        MovementEmission {
            address,
            emitter: emitter.clone(),
            fluid: matches!(address, "swim_started" | "swim_ended")
                .then_some(events.swim_fluid)
                .flatten(),
        }
    }));
}

/// Canonical name of the descriptor a spawned entity came from, if it has one.
/// For a weapon instance this is the weapon descriptor its sounds resolve by.
pub fn descriptor_name(registry: &EntityRegistry, id: EntityId) -> Option<String> {
    registry
        .get_component::<DescriptorProvenance>(id)
        .ok()
        .map(|provenance| provenance.canonical_name.clone())
}

/// A reload outcome stamped for presentation where the weapon stage produced
/// it: anchored at the reloading pawn, naming the reloading weapon's
/// descriptor. Stamped at the tick like every other emission, so a pawn gone or
/// a weapon dropped before the frame drain still sounds where, and as, it
/// reloaded.
pub(crate) fn reload_emission(
    registry: &EntityRegistry,
    delivery: &crate::sim::ReloadDelivery,
) -> WeaponEmission {
    WeaponEmission {
        sounds: None,
        action: None,
        shot_id: None,
        address: delivery.outcome.event_name(),
        emitter: entity_emitter(registry, delivery.pawn()),
        weapon: descriptor_name(registry, delivery.weapon()),
    }
}

/// The addresses a batch of weapon emissions fires, in order.
#[cfg(test)]
pub(crate) fn weapon_addresses(emissions: &[WeaponEmission]) -> Vec<&'static str> {
    emissions.iter().map(|emission| emission.address).collect()
}
