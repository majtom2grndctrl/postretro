// Effective-binding refresh: relevance facts from the entity registry and
// co-op host tuning, rebuilt into the input system when either changes.
// See: context/lib/input.md §2 (Relevance) · context/lib/networking.md §What gates

use postretro_combat_model::TuningPayload;
use postretro_entities::EntityTypeDescriptor;
use postretro_foundation::{PlayerMovementDescriptor, WeaponResource};

use crate::input::{BindingSources, RelevanceFacts};
use crate::*;

fn movement_facts(movement: &PlayerMovementDescriptor) -> RelevanceFacts {
    RelevanceFacts {
        dash: movement.dash.is_some(),
        crouch: movement.crouch.is_some(),
        ..RelevanceFacts::default()
    }
}

/// Relevance facts from the mod's registered entity types.
pub(crate) fn registry_facts(entities: &[EntityTypeDescriptor]) -> RelevanceFacts {
    entities
        .iter()
        .fold(RelevanceFacts::default(), |facts, entity| {
            let mut entity_facts = entity
                .movement
                .as_ref()
                .map(movement_facts)
                .unwrap_or_default();
            if let Some(weapon) = &entity.weapon {
                entity_facts.magazine = matches!(weapon.resource, Some(WeaponResource::Ammo(_)));
                entity_facts.secondary = weapon.secondary.is_some();
            }
            facts.union(entity_facts)
        })
}

/// Relevance facts from installed host tuning: the pawn's movement and its
/// carried wieldables. Tuning sites keep no local fallback, so a host value
/// the local registry lacks must still bind its command.
pub(crate) fn tuning_facts(tuning: &TuningPayload) -> RelevanceFacts {
    let movement = tuning
        .movement
        .as_ref()
        .map(movement_facts)
        .unwrap_or_default();
    tuning
        .wieldables
        .iter()
        .flatten()
        .fold(movement, |facts, wieldable| {
            facts.union(RelevanceFacts {
                magazine: matches!(wieldable.resource, Some(WeaponResource::Ammo(_))),
                secondary: wieldable.secondary.is_some(),
                ..RelevanceFacts::default()
            })
        })
}

impl App {
    /// Rebuild the effective binding table when the registry, the layers, or
    /// participation tuning changed since the last build. Cheap when nothing
    /// did: one generation read and a key compare. Runs before each frame's
    /// input snapshot, gameplay and frontend alike, so the controls panel
    /// never reads a table derived before mod init committed (P6).
    pub(crate) fn refresh_effective_bindings(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let registry = session.scripting.script_ctx.data_registry.borrow();
        let tuning = match session.net_endpoint.as_ref() {
            Some(netcode::NetEndpoint::Client {
                tuning,
                tuning_generation,
                ..
            }) => tuning.as_deref().map(|tuning| (tuning, *tuning_generation)),
            _ => None,
        };
        let sources = BindingSources {
            entity_types_generation: registry.entity_types_generation(),
            tuning: tuning.map(|(_, generation)| generation),
        };
        if !session.bindings.needs_rebuild(sources) {
            return;
        }
        let local = registry_facts(&registry.entities);
        let facts = match tuning {
            Some((tuning, _)) => local.union(tuning_facts(tuning)),
            None => local,
        };
        drop(registry);
        session
            .bindings
            .rebuild(sources, facts, &mut session.input_system);
    }
}

#[cfg(test)]
#[path = "bindings_tests.rs"]
mod tests;
