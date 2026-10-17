// Observer teleport cues: one per accepted remote sample whose teleport generation
// changed, held in arrival order until the frame drains them.
// See: context/lib/networking.md §Host-side remote-pawn presentation (Teleport cuts)

use postretro_entities::EntityId;
use postretro_net::wire::NetworkId;

use super::{ObservedTeleport, TransformSample};

/// Pending observer cues. Every generation change between two accepted samples
/// is one cue: several teleports the observer never sampled between them
/// coalesce into it, carrying the newer sample's last teleporter, while two
/// changes accepted in one frame stay two cues. Storage is reused across frames.
#[derive(Debug, Default)]
pub(super) struct PendingObservedTeleports {
    pending: Vec<(NetworkId, TransformSample)>,
}

impl PendingObservedTeleports {
    /// Queue the accepted sample that changed `network_id`'s generation. A
    /// same-tick replacement of the sample just queued replaces its cue rather
    /// than adding a second one for one server tick.
    pub(super) fn push(&mut self, network_id: NetworkId, sample: TransformSample) {
        if let Some(last) = self.pending.last_mut()
            && last.0 == network_id
            && last.1.server_tick == sample.server_tick
        {
            last.1 = sample;
            return;
        }
        self.pending.push((network_id, sample));
    }

    /// Drop a forgotten identity's cues; a new identity's first sample is silent.
    pub(super) fn forget(&mut self, network_id: NetworkId) {
        self.pending.retain(|(pending, _)| *pending != network_id);
    }

    /// Drain in network-id order, each pawn's cues in the order they were
    /// accepted. Allocates nothing when no generation changed.
    pub(super) fn take(
        &mut self,
        mut resolve: impl FnMut(NetworkId) -> Option<EntityId>,
    ) -> Vec<ObservedTeleport> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        // A stable sort keeps each pawn's changes in acceptance order.
        self.pending.sort_by_key(|(network_id, _)| network_id.0);
        self.pending
            .drain(..)
            .filter_map(|(network_id, sample)| {
                let teleport = sample.teleport?;
                Some(ObservedTeleport {
                    entity_id: resolve(network_id)?,
                    generation: teleport.generation,
                    last_teleporter: teleport.last_teleporter,
                    arrival: sample.transform.position,
                })
            })
            .collect()
    }
}
