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

/// Resolve a departing pawn against the authority's current mod table.
/// The caller distinguishes an absent movement source from an undeclared one.
pub fn life_state_name(
    registry: &EntityRegistry,
    pawn: EntityId,
    names: &[String],
) -> Option<String> {
    let movement = registry
        .get_component::<PlayerMovementComponent>(pawn)
        .ok()?;
    if movement.life_state == postretro_foundation::UNDECLARED_LIFE_STATE {
        return None;
    }
    names.get(usize::from(movement.life_state)).cloned()
}

/// Materialization runs before a net-slot pawn is bound to its seat. Resolve
/// through its lowered descriptor directly, without a membership/role query.
pub fn restore_carried_life_state(
    carried: Option<&postretro_combat_model::CarriedState>,
    registry: &mut EntityRegistry,
    pawn: EntityId,
    descriptor: &postretro_entities::EntityTypeDescriptor,
) {
    let Some(states) = descriptor.life_states.as_ref() else {
        return;
    };
    let target = carried
        .and_then(|carried| carried.life_state.as_deref())
        .filter(|name| states.states.contains_key(*name))
        .and_then(|name| states.rejoin.get(name))
        .and_then(|target| states.state(target));
    let (id, profile) = target.unwrap_or_else(|| states.initial_state());
    let delta = match registry
        .get_component_value_mut(pawn, postretro_entities::ComponentKind::PlayerMovement)
    {
        Ok(postretro_entities::ComponentValue::PlayerMovement(movement)) => {
            postretro_foundation::enter_life_state(movement, id, profile, states.table_revision);
            apply_spawn_life_posture(movement)
        }
        _ => return,
    };
    if delta != 0.0
        && let Ok(transform) = registry.get_component::<postretro_entities::Transform>(pawn)
    {
        let mut pose = *transform;
        pose.position.y += delta;
        let _ = registry.set_presentation_transform(pawn, pose);
    }
}

/// A new pawn/respawn has no presented transition to ease. Its geometry starts
/// in the target posture even before the first remote command, with feet held.
pub(crate) fn apply_spawn_life_posture(movement: &mut PlayerMovementComponent) -> f32 {
    let previous_height = movement.capsule.half_height;
    let crouch = if movement.life_posture == postretro_foundation::LifeStatePosture::Crouch {
        movement.crouch.as_ref()
    } else {
        None
    };
    if let Some(crouch) = crouch {
        movement.capsule.half_height = crouch.half_height;
        movement.capsule.eye_height = crouch.eye_height;
        movement.movement_state = postretro_foundation::MovementState::Crouching {
            eye_current: crouch.eye_height,
        };
    } else {
        movement.capsule.half_height = movement.standing_half_height;
        movement.capsule.eye_height = movement.standing_eye_height;
        movement.movement_state = postretro_foundation::MovementState::Normal;
    }
    movement.capsule.half_height - previous_height
}

#[cfg(test)]
#[path = "life_state_restore_tests.rs"]
mod restore_tests;
