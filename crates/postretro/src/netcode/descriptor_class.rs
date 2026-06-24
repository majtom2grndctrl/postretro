// Host serialize-side derivation of a replicable entity's descriptor-class metadata
// (the snapshot `entity_class`), read from its `DescriptorProvenance`, plus the shared
// "is this a networked AI map enemy" predicate that gates host registration and
// connected-client spawn suppression.
// See: context/lib/networking.md · context/lib/entity_model.md §6

use postretro_net::wire::ComponentPayload;

use crate::scripting::provenance::{DescriptorProvenance, DescriptorSpawnPath};
use crate::scripting::registry::{ComponentKind, EntityId, EntityRegistry};

/// Shared predicate: is `id` a map-placed descriptor AI enemy that the host owns as an
/// authoritative networked entity (E10 — networked enemy authority)?
///
/// True iff the entity is alive and:
/// - its `DescriptorProvenance.spawn_path == DescriptorSpawnPath::MapPlacement` (a
///   map-authored placement, not a player-start / net-slot / default-weapon spawn), AND
/// - its **live** registry columns carry BOTH `ComponentKind::Brain` AND
///   `ComponentKind::Agent` (the engine-owned AI brain + navigation agent an `ai`
///   descriptor block materializes together — see `data_archetype::attach_descriptor_components`).
///
/// Contract notes for importers (this is a deliverable other tasks depend on):
/// - It reads the **live component columns**, NOT `DescriptorProvenance.owned_components`.
///   `owned_components` only tracks the modder-declarable `DescriptorComponentKind` set
///   (weapon/movement/light/emitter/mesh/health) and never includes the AI components,
///   so it cannot be used to detect an AI enemy.
/// - It is registry-blind about role: it does not check host/client. Host registration
///   (this task) gates on the role separately; connected-client spawn suppression (Task 5)
///   imports the same predicate to decide which descriptor placements NOT to spawn locally.
/// - It is reachable from both the netcode/registration path and the
///   descriptor-dispatch/lifecycle path via `crate::netcode::is_networked_ai_map_enemy`.
///
/// A stale/despawned id (`get_component` errors) returns `false`.
pub(crate) fn is_networked_ai_map_enemy(registry: &EntityRegistry, id: EntityId) -> bool {
    let Ok(provenance) = registry.get_component::<DescriptorProvenance>(id) else {
        return false;
    };
    if provenance.spawn_path != DescriptorSpawnPath::MapPlacement {
        return false;
    }
    matches!(
        registry.has_component_kind(id, ComponentKind::Brain),
        Ok(true)
    ) && matches!(
        registry.has_component_kind(id, ComponentKind::Agent),
        Ok(true)
    )
}

/// The descriptor class a replicable entity was materialized from, for the snapshot's
/// `entity_class` (M15 Phase 3 Task 7 / E10 Task 4). The recipient uses it to materialize
/// the matching descriptor-backed presentation locally. `None` unless the entity is one of:
///
/// - a **movement pawn** (carries a `PlayerMovementState` wire payload) spawned through
///   the net-slot descriptor path (`DescriptorSpawnPath::NetworkSlot`) — its
///   `canonical_name` is the resolved `entity_class` (default `"player"`); or
/// - a **map-placed AI enemy** ([`is_networked_ai_map_enemy`]) — its `canonical_name` is
///   the descriptor class the host registered it under.
///
/// The wire allows `entity_class` on any non-despawn finite-`Transform` record (E10
/// Task 3 relaxed it off the movement-only gate), so an enemy's class rides its
/// Transform-only snapshot. A Transform-only fixture / demo mover / map-start pawn that
/// is neither of the above returns `None`.
pub(super) fn descriptor_entity_class(
    registry: &EntityRegistry,
    id: EntityId,
    components: &[ComponentPayload],
) -> Option<String> {
    let provenance = registry.get_component::<DescriptorProvenance>(id).ok()?;

    // A net-slot movement pawn: the wire historically gated `entity_class` on a movement
    // record, and this remains the host's own player / accepted-client pawn case.
    let carries_movement = components
        .iter()
        .any(|c| matches!(c, ComponentPayload::PlayerMovementState(_)));
    if carries_movement && provenance.spawn_path == DescriptorSpawnPath::NetworkSlot {
        return Some(provenance.canonical_name.clone());
    }

    // A map-placed AI enemy (Brain + Agent, MapPlacement): stamp its descriptor class so
    // the client materializes the remote-enemy presentation from a Transform-only record.
    if is_networked_ai_map_enemy(registry, id) {
        return Some(provenance.canonical_name.clone());
    }

    None
}
