// Bind replicated ghost-link state to its link for presentation history; hold the level-scoped ghost-audio record.
// See: context/lib/networking.md §Current contract
use std::collections::{HashMap, VecDeque};

use postretro_entities::{ComponentKind, EntityId, EntityRegistry, GhostLinkComponent};
use postretro_net::wire::{ComponentPayload, NetworkId, WireGhostAudioState, WireGhostLinkState};

use super::{ApplyOutcome, ClientReplication, IgnoredPayload, RegionBinding};

/// Queue length past which received updates coalesce per link. Every applied
/// snapshot is drained the frame it lands, so only a burst (a joiner's first
/// baseline, the re-baseline after an unload) or a stalled drain reaches it.
const MAX_PENDING_UPDATES: usize = 256;

/// One received link state and the host time it describes: the server tick of
/// the snapshot that carried it, the post-increment tick its end-of-tick state
/// is stamped with. The presentation side pushes it into the link's history.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GhostLinkUpdate {
    pub server_tick: u32,
    pub state: WireGhostLinkState,
}

/// Received link states held between frames, in apply order.
///
/// Over the limit the queue coalesces per link, keeping each link's newest
/// state and dropping only states a newer one for the same link supersedes.
/// It never drops a link's newest state: the host stops resending an acked,
/// unchanged link, so that state may be its only copy. A queue holding one
/// state for each of many links therefore grows to the link count, and the
/// limit rises with it so coalescing stays rare.
#[derive(Debug, Default)]
pub(super) struct PendingGhostLinkUpdates {
    updates: VecDeque<GhostLinkUpdate>,
    /// Length past which the next push coalesces, beyond the
    /// [`MAX_PENDING_UPDATES`] floor: twice the last coalesced length.
    limit: usize,
    /// Coalescing scratch: queued states per link index.
    counts: HashMap<u16, u32>,
}

impl PendingGhostLinkUpdates {
    fn push(&mut self, update: GhostLinkUpdate) {
        self.updates.push_back(update);
        if self.updates.len() > self.limit.max(MAX_PENDING_UPDATES) {
            self.coalesce();
        }
    }

    /// Keep only each link's newest queued state, in apply order.
    fn coalesce(&mut self) {
        let before = self.updates.len();
        self.counts.clear();
        for update in &self.updates {
            *self.counts.entry(update.state.index).or_default() += 1;
        }
        let counts = &mut self.counts;
        self.updates.retain(|update| {
            let Some(count) = counts.get_mut(&update.state.index) else {
                return true;
            };
            *count -= 1;
            *count == 0
        });
        self.limit = self.updates.len().saturating_mul(2);
        let dropped = before - self.updates.len();
        if dropped > 0 {
            log::warn!(
                "[Net] ghost link update queue past {before} states; dropped {dropped} superseded, kept each link's newest"
            );
        }
    }

    pub(super) fn clear(&mut self) {
        self.updates.clear();
        self.limit = 0;
    }

    fn drain(&mut self) -> std::collections::vec_deque::Drain<'_, GhostLinkUpdate> {
        self.limit = 0;
        self.updates.drain(..)
    }
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
        self.ghost_link_updates
            .push(GhostLinkUpdate { server_tick, state });
    }

    /// Every link state received since the last drain, in apply order. The
    /// installed link component is never written by replication: it stays this
    /// client's local spawn state, and presentation history takes these instead,
    /// so a state that lasted between two frames is not lost.
    pub fn drain_ghost_link_updates(&mut self) -> impl Iterator<Item = GhostLinkUpdate> + '_ {
        self.ghost_link_updates.drain()
    }

    /// Hold the newest snapshot's ghost-audio record. The record is level-scoped
    /// and whole, so the newest supersedes any untaken one; an envelope without
    /// a record leaves the held one alone.
    pub(super) fn receive_ghost_audio(&mut self, record: Option<WireGhostAudioState>) {
        if record.is_some() {
            self.ghost_audio = record;
        }
    }

    /// The newest ghost-audio record received since the last take, if any. The
    /// presentation side rebases it onto this client's script clock.
    pub fn take_ghost_audio(&mut self) -> Option<WireGhostAudioState> {
        self.ghost_audio.take()
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
