// Admission history for first entity baselines overtaken by newer snapshots.
// See: context/lib/networking.md

use std::collections::HashSet;

use postretro_net::wire::NetworkId;

/// Bound additional ordering memory independently of entity churn. Once the set
/// fills, a newer snapshot must carry any still-missing baseline: forgetting an
/// identity must never make its older baseline eligible again.
pub(super) const MAX_REMEMBERED_IDENTITIES: usize = 4096;

#[derive(Debug, Default)]
pub(super) struct LateBaselineHistory {
    floor: Option<u32>,
    seen: HashSet<NetworkId>,
}

impl LateBaselineHistory {
    pub(super) fn permits(&self, sequence: u32, network_id: NetworkId) -> bool {
        self.floor.is_none_or(|floor| sequence > floor) && !self.seen.contains(&network_id)
    }

    pub(super) fn remember(&mut self, network_id: NetworkId, latest_sequence: u32) {
        if self.seen.contains(&network_id) {
            return;
        }
        if self.seen.len() == MAX_REMEMBERED_IDENTITIES {
            self.reset(Some(latest_sequence));
        }
        self.seen.insert(network_id);
    }

    pub(super) fn reset(&mut self, latest_sequence: Option<u32>) {
        self.floor = latest_sequence;
        self.seen.clear();
    }
}
