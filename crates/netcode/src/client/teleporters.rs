// Bind replicated teleporter arming to the installed local movement region.
use postretro_entities::{ComponentKind, EntityId, EntityRegistry, TeleporterComponent};
use postretro_net::wire::{ComponentPayload, WireTeleporterState};

pub(super) fn first_state(components: &[ComponentPayload]) -> Option<WireTeleporterState> {
    components.iter().find_map(|payload| match payload {
        ComponentPayload::TeleporterState(state) => Some(*state),
        _ => None,
    })
}

pub(super) fn find_loaded(
    registry: &EntityRegistry,
    wire: WireTeleporterState,
) -> Option<EntityId> {
    registry
        .iter_with_kind(ComponentKind::Teleporter)
        .find_map(|(id, _)| {
            registry
                .get_component::<TeleporterComponent>(id)
                .ok()
                .filter(|local| {
                    local.index == wire.index && local.source_entity == wire.source_entity
                })
                .map(|_| id)
        })
}

pub(super) fn matches(
    registry: &EntityRegistry,
    entity: EntityId,
    wire: WireTeleporterState,
) -> bool {
    registry
        .get_component::<TeleporterComponent>(entity)
        .is_ok_and(|local| local.index == wire.index && local.source_entity == wire.source_entity)
}

pub(super) fn apply(registry: &mut EntityRegistry, entity: EntityId, wire: WireTeleporterState) {
    if let Ok(local) = registry.get_component::<TeleporterComponent>(entity) {
        let mut updated = local.clone();
        updated.armed = wire.armed;
        let _ = registry.set_component(entity, updated);
    }
}
