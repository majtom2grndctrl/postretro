// Local entity bindings used while applying host snapshots. The client keeps
// stable PRL mover identities here so mover phase is applied to the loaded copy.

use std::collections::HashMap;

use postretro_entities::{
    ComponentKind, ComponentValue, EntityId, EntityRegistry, KinematicMoverComponent,
};
use postretro_net::wire::{NetworkId, WireKinematicMoverState};

#[derive(Debug, Default)]
pub(super) struct ClientBindings {
    /// Network ids bound to PRL-loaded local kinematic movers, with their stable
    /// compile-time mover id. These entities are predicted/reconciled in place and
    /// must never be remote-interpolated or materialized from a baseline.
    pub(super) mover_network_ids: HashMap<NetworkId, u32>,
}

pub(super) fn find_loaded_mover_entity(
    registry: &EntityRegistry,
    mover_id: u32,
) -> Option<EntityId> {
    registry
        .iter_with_kind(ComponentKind::KinematicMover)
        .find_map(|(id, value)| {
            let ComponentValue::KinematicMover(mover) = value else {
                return None;
            };
            (mover.mover_id == mover_id).then_some(id)
        })
}

pub(super) fn validate_kinematic_mover_state_binding(
    registry: &EntityRegistry,
    id: EntityId,
    network_id: NetworkId,
    bound_mover_id: Option<u32>,
    wire: &WireKinematicMoverState,
) -> Option<KinematicMoverComponent> {
    let Ok(mover) = registry
        .get_component::<KinematicMoverComponent>(id)
        .cloned()
    else {
        log::warn!(
            "[Net] KinematicMoverState for mover_id {} applied to entity without KinematicMover",
            wire.mover_id
        );
        return None;
    };
    if let Some(bound) = bound_mover_id
        && bound != wire.mover_id
    {
        log::warn!(
            "[Net] KinematicMoverState mover_id {} would rebind {network_id:?} from mover_id {}; dropping phase",
            wire.mover_id,
            bound
        );
        return None;
    }
    if mover.mover_id != wire.mover_id {
        log::warn!(
            "[Net] KinematicMoverState mover_id {} does not match local mover_id {}; dropping phase",
            wire.mover_id,
            mover.mover_id
        );
        return None;
    }
    Some(mover)
}
