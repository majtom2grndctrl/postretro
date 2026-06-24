// Host serialize-side derivation of a replicable pawn's descriptor-class metadata
// (the snapshot `entity_class`), read from its `DescriptorProvenance`.
// See: context/lib/networking.md

use postretro_net::wire::ComponentPayload;

use crate::scripting::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use crate::scripting::registry::{EntityId, EntityRegistry};

/// The descriptor class a replicable movement pawn was materialized from, for the
/// snapshot's `entity_class` (M15 Phase 3 Task 7). `None` unless the entity both
/// carries a `PlayerMovementState` payload (the wire only allows `entity_class` on a
/// movement record) AND was spawned through the net-slot descriptor path
/// (`DescriptorSpawnPath::NetworkSlot`), in which case its `DescriptorProvenance`
/// `canonical_name` is exactly the resolved `entity_class` (default `"player"`). A
/// Transform-only fixture / demo mover / map-start pawn returns `None`.
pub(super) fn movement_entity_class(
    registry: &EntityRegistry,
    id: EntityId,
    components: &[ComponentPayload],
) -> Option<String> {
    let carries_movement = components
        .iter()
        .any(|c| matches!(c, ComponentPayload::PlayerMovementState(_)));
    if !carries_movement {
        return None;
    }
    let provenance = registry.get_component::<DescriptorProvenance>(id).ok()?;
    if provenance.spawn_path != DescriptorSpawnPath::NetworkSlot {
        return None;
    }
    Some(provenance.canonical_name.clone())
}
