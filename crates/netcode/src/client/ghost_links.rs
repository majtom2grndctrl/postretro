// Bind replicated ghost-link state to the installed link and queue it for presentation history.
// See: context/lib/networking.md §Current contract
use postretro_entities::{ComponentKind, EntityId, EntityRegistry, GhostLinkComponent};
use postretro_net::wire::{ComponentPayload, NetworkId, WireGhostLinkState};

use super::{ApplyOutcome, ClientReplication, IgnoredPayload, RegionBinding};

/// Received updates held between frames. A link changes at most once per host
/// tick and every applied snapshot is drained the frame it lands, so this only
/// bounds memory if the presentation side stops draining.
const MAX_PENDING_UPDATES: usize = 256;

/// One received link state and the host time it describes: the server tick of
/// the snapshot that carried it, the post-increment tick its end-of-tick state
/// is stamped with. The presentation side pushes it into the link's history.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GhostLinkUpdate {
    pub server_tick: u32,
    pub state: WireGhostLinkState,
}

impl ClientReplication {
    /// Bind an unmapped ghost-link full baseline to the installed link it names,
    /// as a teleporter baseline binds. A link is never spawned from a record: one
    /// naming no installed link is ignored and not acked, so a later baseline can
    /// bind it once the link installs.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn bind_ghost_link_baseline(
        &mut self,
        registry: &mut EntityRegistry,
        network_id: NetworkId,
        baseline_id: u32,
        state: WireGhostLinkState,
        server_tick: u32,
        mover_target_tick: u32,
        mover_tick_dt: f32,
        components: &[ComponentPayload],
        outcome: &mut ApplyOutcome,
    ) -> bool {
        let Some(id) = find_loaded(registry, state) else {
            log::warn!(
                "[Net] ghost link baseline for {network_id:?} names unknown index/source {}/{}; not binding",
                state.index,
                state.source_entity
            );
            outcome.ignored.push(IgnoredPayload::UnknownGhostLink {
                network_id: network_id.0,
                index: state.index,
                source_entity: state.source_entity,
            });
            return false;
        };
        if self.reverse_map.contains_key(&id) {
            log::warn!(
                "[Net] ghost link {} is already bound; not binding {network_id:?}",
                state.index
            );
            return false;
        }
        self.insert_mapping(network_id, id);
        self.baselines.insert(network_id, baseline_id);
        self.pending_repairs.remove(&network_id);
        self.bindings
            .region_network_ids
            .insert(network_id, RegionBinding::GhostLink(state.index));
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

    pub(super) fn queue_ghost_link_update(&mut self, server_tick: u32, state: WireGhostLinkState) {
        if self.ghost_link_updates.len() >= MAX_PENDING_UPDATES {
            log::warn!("[Net] ghost link update queue full; dropping the oldest update");
            self.ghost_link_updates.pop_front();
        }
        self.ghost_link_updates
            .push_back(GhostLinkUpdate { server_tick, state });
    }

    /// Every link state received since the last drain, in apply order. The
    /// installed link component is never written by replication: it stays this
    /// client's local spawn state, and presentation history takes these instead,
    /// so a state that lasted between two frames is not lost.
    pub fn drain_ghost_link_updates(&mut self) -> impl Iterator<Item = GhostLinkUpdate> + '_ {
        self.ghost_link_updates.drain(..)
    }
}

pub(super) fn first_state(components: &[ComponentPayload]) -> Option<WireGhostLinkState> {
    components.iter().find_map(|payload| match payload {
        ComponentPayload::GhostLinkState(state) => Some(*state),
        _ => None,
    })
}

pub(super) fn find_loaded(registry: &EntityRegistry, wire: WireGhostLinkState) -> Option<EntityId> {
    registry
        .iter_with_kind(ComponentKind::GhostLink)
        .find_map(|(id, _)| matches(registry, id, wire).then_some(id))
}

pub(super) fn matches(
    registry: &EntityRegistry,
    entity: EntityId,
    wire: WireGhostLinkState,
) -> bool {
    registry
        .get_component::<GhostLinkComponent>(entity)
        .is_ok_and(|local| local.index == wire.index && local.source_entity == wire.source_entity)
}
