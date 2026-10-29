//! Registry-side life capability gates and the immutable Triggers-stage view.
//! See: context/lib/entity_model.md §5 and context/lib/scripting.md §12.

use std::collections::{BTreeMap, HashSet};

use postretro_entities::components::{brain::BrainComponent, health::HealthComponent};
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::{Capability, PlayerMovementComponent, player_capability};

use crate::trigger_system::{AuthoritativePlayer, PlayerId, canonical_player_pawns};

pub fn pawn_capability(
    registry: &EntityRegistry,
    pawn: EntityId,
    capability: Capability,
) -> Option<bool> {
    registry
        .get_component::<PlayerMovementComponent>(pawn)
        .ok()
        .and_then(|movement| player_capability(movement, capability))
}

pub fn permits(registry: &EntityRegistry, pawn: EntityId, capability: Capability) -> bool {
    pawn_capability(registry, pawn, capability).unwrap_or(true)
}

/// This adapter is used only by AI apply. Brain-bearing actors keep their
/// original health gate even if an unusual descriptor also carries movement.
pub fn ai_is_quiescent(registry: &EntityRegistry, pawn: EntityId) -> bool {
    if registry.get_component::<BrainComponent>(pawn).is_err()
        && let Some(damageable) = pawn_capability(registry, pawn, Capability::Damageable)
    {
        return !damageable
            || registry.is_inventory_owned(pawn)
            || crate::scripting_systems::health::is_terminally_committed_to_removal(
                registry, pawn,
            );
    }
    crate::scripting_systems::health::is_quiescent(registry, pawn)
}

#[derive(Debug, Clone, Copy, Default)]
struct TriggerCapabilities {
    use_enabled: Option<bool>,
    touch: Option<bool>,
    occupancy: Option<bool>,
}

/// Captured once before any trigger reaction. Membership remains physical;
/// only activation and effective counts consult this snapshot.
#[derive(Debug, Default)]
pub struct TriggerStageCapabilities {
    pub(crate) pawns: BTreeMap<PlayerId, EntityId>,
    capabilities: BTreeMap<PlayerId, TriggerCapabilities>,
    pub(crate) occupants: HashSet<PlayerId>,
}

impl TriggerStageCapabilities {
    pub fn capture(registry: &EntityRegistry, players: &[AuthoritativePlayer]) -> Self {
        let pawns = canonical_player_pawns(registry, players);
        let capabilities: BTreeMap<_, _> = pawns
            .iter()
            .map(|(&player, &pawn)| {
                (
                    player,
                    TriggerCapabilities {
                        use_enabled: pawn_capability(registry, pawn, Capability::Use),
                        touch: pawn_capability(registry, pawn, Capability::Touch),
                        occupancy: pawn_capability(registry, pawn, Capability::Occupancy),
                    },
                )
            })
            .collect();
        let occupants = pawns
            .iter()
            .filter_map(|(&player, &pawn)| {
                capabilities[&player]
                    .occupancy
                    .unwrap_or_else(|| {
                        registry
                            .get_component::<HealthComponent>(pawn)
                            .map_or(true, |health| {
                                !crate::scripting_systems::health::is_depleted(health)
                            })
                    })
                    .then_some(player)
            })
            .collect();
        Self {
            pawns,
            capabilities,
            occupants,
        }
    }

    pub(crate) fn effective_occupants(&self, fallback: &HashSet<PlayerId>) -> HashSet<PlayerId> {
        self.pawns
            .keys()
            .copied()
            .filter(|player| {
                self.capabilities[player]
                    .occupancy
                    .unwrap_or_else(|| fallback.contains(player))
            })
            .collect()
    }

    pub fn permits_use(&self, player: PlayerId) -> bool {
        self.capabilities
            .get(&player)
            .and_then(|gates| gates.use_enabled)
            .unwrap_or(true)
    }

    pub fn permits_touch(&self, player: PlayerId) -> bool {
        self.capabilities
            .get(&player)
            .and_then(|gates| gates.touch)
            .unwrap_or(true)
    }
}
