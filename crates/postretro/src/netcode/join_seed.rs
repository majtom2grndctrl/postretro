// Host-side join-seed buffering across parity and participation.
// See: context/lib/networking.md

use std::collections::{BTreeMap, HashMap, HashSet};

use postretro_net::wire::JoinSeedValue;

/// Per-connection join seeds held until the host knows the player's durable
/// seat. The buffer belongs in the binary because only engine code can validate
/// durable keys and mutate per-seat slot values.
#[derive(Debug, Default)]
pub(crate) struct HostJoinSeeds {
    pending: HashMap<u64, BTreeMap<String, JoinSeedValue>>,
    applied: HashSet<u64>,
    reclaimed: HashSet<u64>,
}

pub(crate) enum JoinSeedArrival {
    Buffered,
    Apply(BTreeMap<String, JoinSeedValue>),
    DroppedConsumed,
    DroppedReclaimed,
}

pub(crate) enum ParticipationSeed {
    None,
    Apply(BTreeMap<String, JoinSeedValue>),
    DroppedReclaimed,
}

impl HostJoinSeeds {
    /// Mark an admission that reclaimed a held seat. That seat's carried live
    /// state wins over every persisted seed from the reconnecting client.
    pub(crate) fn mark_reclaimed(&mut self, client_id: u64) {
        self.pending.remove(&client_id);
        self.reclaimed.insert(client_id);
    }

    /// Buffer a seed while parity is held, or return it for immediate late
    /// application once the connection already participates.
    pub(crate) fn receive(
        &mut self,
        client_id: u64,
        slots: BTreeMap<String, JoinSeedValue>,
        participating: bool,
    ) -> JoinSeedArrival {
        if self.reclaimed.contains(&client_id) {
            self.pending.remove(&client_id);
            self.applied.insert(client_id);
            return JoinSeedArrival::DroppedReclaimed;
        }
        if self.applied.contains(&client_id) {
            return JoinSeedArrival::DroppedConsumed;
        }
        if participating {
            self.applied.insert(client_id);
            return JoinSeedArrival::Apply(slots);
        }
        self.pending.insert(client_id, slots);
        JoinSeedArrival::Buffered
    }

    /// Consume the buffered seed at the participation seam, before the pawn
    /// materializes. No seed leaves a single late-arrival opportunity open: the
    /// first real seed received after defaults is still applied, then duplicates
    /// are rejected.
    pub(crate) fn on_participating(&mut self, client_id: u64) -> ParticipationSeed {
        if self.reclaimed.contains(&client_id) {
            self.pending.remove(&client_id);
            self.applied.insert(client_id);
            return ParticipationSeed::DroppedReclaimed;
        }
        let Some(slots) = self.pending.remove(&client_id) else {
            return ParticipationSeed::None;
        };
        self.applied.insert(client_id);
        ParticipationSeed::Apply(slots)
    }

    /// A level transition begins a new parity/participation generation.
    pub(crate) fn clear_generation(&mut self, client_id: u64) {
        self.pending.remove(&client_id);
        self.applied.remove(&client_id);
        self.reclaimed.remove(&client_id);
    }

    /// Transport ids are short-lived. Never retain their seed state after close.
    pub(crate) fn remove_client(&mut self, client_id: u64) {
        self.clear_generation(client_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(value: f64) -> BTreeMap<String, JoinSeedValue> {
        BTreeMap::from([(
            "kplayer0000000001".to_string(),
            JoinSeedValue::Number(value),
        )])
    }

    #[test]
    fn early_seed_buffers_then_applies_on_participation() {
        let mut seeds = HostJoinSeeds::default();

        assert!(matches!(
            seeds.receive(7, seed(42.0), false),
            JoinSeedArrival::Buffered
        ));
        assert!(matches!(
            seeds.on_participating(7),
            ParticipationSeed::Apply(values)
                if values == seed(42.0)
        ));
    }

    #[test]
    fn late_first_seed_applies_after_participation_defaults() {
        let mut seeds = HostJoinSeeds::default();

        assert!(matches!(seeds.on_participating(7), ParticipationSeed::None));
        assert!(matches!(
            seeds.receive(7, seed(42.0), true),
            JoinSeedArrival::Apply(values)
                if values == seed(42.0)
        ));
    }

    #[test]
    fn reclaimed_seat_discards_seed_without_replacing_live_values() {
        let mut seeds = HostJoinSeeds::default();
        seeds.mark_reclaimed(7);

        assert!(matches!(
            seeds.receive(7, seed(42.0), false),
            JoinSeedArrival::DroppedReclaimed
        ));
        assert!(matches!(
            seeds.on_participating(7),
            ParticipationSeed::DroppedReclaimed
        ));
    }

    #[test]
    fn seed_after_apply_is_discarded_until_the_next_generation() {
        let mut seeds = HostJoinSeeds::default();

        assert!(matches!(
            seeds.receive(7, seed(42.0), true),
            JoinSeedArrival::Apply(_)
        ));
        assert!(matches!(
            seeds.receive(7, seed(99.0), true),
            JoinSeedArrival::DroppedConsumed
        ));
        seeds.clear_generation(7);
        assert!(matches!(
            seeds.receive(7, seed(99.0), false),
            JoinSeedArrival::Buffered
        ));
    }
}
