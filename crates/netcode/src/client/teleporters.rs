// Bind replicated teleporter arming to the installed local movement region.
// See: context/lib/networking.md §Current contract
use postretro_entities::{ComponentKind, EntityId, EntityRegistry, TeleporterComponent};
use postretro_net::wire::{ComponentPayload, NetworkId, WireTeleporterState};

use super::{ApplyOutcome, ClientReplication, IgnoredPayload, RegionBinding};

impl ClientReplication {
    /// Bind an unmapped teleporter full baseline to the installed region it names
    /// and apply its armed state. A teleporter is never spawned from a record: one
    /// that matches no installed region is ignored, and one whose region is
    /// already bound, or whose payload fails to apply, leaves no mapping behind.
    /// `false` means not acked, so a later baseline can bind it.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_teleporter_baseline(
        &mut self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        baseline_id: u32,
        state: WireTeleporterState,
        server_tick: u32,
        mover_target_tick: u32,
        mover_tick_dt: f32,
        components: &[ComponentPayload],
        outcome: &mut ApplyOutcome,
    ) -> bool {
        let Some(id) = find_loaded(registry, state) else {
            log::warn!(
                "[Net] teleporter baseline for {network_id:?} names unknown index/source {}/{}; not binding",
                state.index,
                state.source_entity
            );
            outcome.ignored.push(IgnoredPayload::UnknownTeleporter {
                network_id: network_id.0,
                index: state.index,
                source_entity: state.source_entity,
            });
            return false;
        };
        if self.reverse_map.contains_key(&id) {
            log::warn!(
                "[Net] teleporter {} is already bound; not binding {network_id:?}",
                state.index
            );
            return false;
        }
        self.insert_mapping(network_id, id);
        self.baselines.insert(network_id, baseline_id);
        self.pending_repairs.remove(&network_id);
        self.bindings
            .region_network_ids
            .insert(network_id, RegionBinding::Teleporter(state.index));
        self.interp.forget(network_id);
        if self.apply_components_to(
            registry,
            network_id,
            server_tick,
            mover_target_tick,
            mover_tick_dt,
            id,
            components,
            outcome,
        ) {
            return true;
        }
        self.remove_mapping(network_id);
        self.baselines.remove(&network_id);
        self.bindings.region_network_ids.remove(&network_id);
        false
    }
}

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
