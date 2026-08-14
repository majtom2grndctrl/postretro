// Host-side join-seed buffering across parity and participation.
// See: context/lib/networking.md

use std::collections::{BTreeMap, HashMap, HashSet};

use postretro_net::slots::SlotEvent;
use postretro_net::transport::ServerPoll;
use postretro_net::wire::JoinSeedValue;

/// Per-connection join seeds held until the host knows the player's durable
/// seat. The buffer belongs in the binary because only engine code can validate
/// durable keys and mutate per-seat slot values.
#[derive(Debug, Default)]
pub(crate) struct HostJoinSeeds {
    pending: HashMap<u64, BTreeMap<String, JoinSeedValue>>,
    consumed: HashSet<u64>,
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
    /// Route one transport poll through the seed lifecycle before the caller
    /// applies participation edges. Treat an entry edge as not-yet-participating
    /// so its seed stays buffered until the admitted seat is known.
    pub(crate) fn route_poll(
        &mut self,
        poll: &ServerPoll,
        mut is_participating: impl FnMut(u64) -> bool,
    ) -> Vec<(u64, JoinSeedArrival)> {
        self.prepare_lifecycle(&poll.lifecycle);
        poll.join_seeds
            .iter()
            .map(|(client_id, slots)| {
                let entering_participation = poll.lifecycle.iter().any(|event| {
                    matches!(
                        event,
                        SlotEvent::Participating {
                            client_id: entering_client
                        } if entering_client == client_id
                    )
                });
                let arrival = self.receive(
                    *client_id,
                    slots.clone(),
                    is_participating(*client_id) && !entering_participation,
                );
                (*client_id, arrival)
            })
            .collect()
    }

    /// Retire lifecycle state before ingesting this poll's seeds. A demotion
    /// preserves the durable seat and its live values, so it closes any late
    /// seed opportunity instead of opening another one. Only a closed transport
    /// id can later start with fresh seed state.
    fn prepare_lifecycle(&mut self, events: &[SlotEvent]) {
        for event in events {
            match event {
                SlotEvent::Demoted { client_id, .. } => {
                    self.pending.remove(client_id);
                    self.consumed.insert(*client_id);
                }
                SlotEvent::Closed { client_id, .. } => self.remove_client(*client_id),
                SlotEvent::Participating { .. } => {}
            }
        }
    }

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
            self.consumed.insert(client_id);
            return JoinSeedArrival::DroppedReclaimed;
        }
        if self.consumed.contains(&client_id) {
            return JoinSeedArrival::DroppedConsumed;
        }
        if participating {
            self.consumed.insert(client_id);
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
            self.consumed.insert(client_id);
            return ParticipationSeed::DroppedReclaimed;
        }
        let Some(slots) = self.pending.remove(&client_id) else {
            return ParticipationSeed::None;
        };
        self.consumed.insert(client_id);
        ParticipationSeed::Apply(slots)
    }

    /// Transport ids are short-lived. Never retain their seed state after close.
    pub(crate) fn remove_client(&mut self, client_id: u64) {
        self.pending.remove(&client_id);
        self.consumed.remove(&client_id);
        self.reclaimed.remove(&client_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use crate::scripting::state_persistence::apply_join_seed;
    use postretro_entities::slot_table::{
        ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotTable, SlotType, SlotValue,
    };
    use postretro_foundation::Seat;
    use postretro_scripting_core::store_identity::StoreIdentityLedger;

    fn seed(value: f64) -> BTreeMap<String, JoinSeedValue> {
        BTreeMap::from([(
            "kplayer0000000001".to_string(),
            JoinSeedValue::Number(value),
        )])
    }

    fn persistent_per_owner_fixture() -> (SlotTable, StoreIdentityLedger, BTreeSet<String>) {
        let mut table = SlotTable::new();
        table
            .insert_namespace(
                "game",
                vec![(
                    "xp".to_string(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(0.0)),
                        range: None,
                        persist: true,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                        network: ReplicationScope::OwnerPrivatePlayer,
                        per_owner: true,
                        accumulate: None,
                    }),
                )],
            )
            .expect("declare persistent per-owner fixture");
        let identity = StoreIdentityLedger {
            version: 1,
            slots: BTreeMap::from([("game.xp".to_string(), "kplayer0000000001".to_string())]),
        };
        let membership = BTreeSet::from(["game.xp".to_string()]);
        (table, identity, membership)
    }

    fn collect_poll_arrivals(
        seeds: &mut HostJoinSeeds,
        poll: &ServerPoll,
    ) -> Vec<(u64, JoinSeedArrival)> {
        seeds.route_poll(poll, |_| true)
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
    fn seed_after_apply_is_discarded_until_the_connection_closes() {
        let mut seeds = HostJoinSeeds::default();

        assert!(matches!(
            seeds.receive(7, seed(42.0), true),
            JoinSeedArrival::Apply(_)
        ));
        assert!(matches!(
            seeds.receive(7, seed(99.0), true),
            JoinSeedArrival::DroppedConsumed
        ));
        seeds.remove_client(7);
        assert!(matches!(
            seeds.receive(7, seed(99.0), false),
            JoinSeedArrival::Buffered
        ));
    }

    // Regression: demotion reopened seed application for a seat whose live values survived.
    #[test]
    fn same_poll_demotion_then_participation_drops_reprepared_seed() {
        use postretro_net::wire::HoldingCause;

        let mut seeds = HostJoinSeeds::default();
        assert!(matches!(
            seeds.receive(7, seed(1.0), true),
            JoinSeedArrival::Apply(_)
        ));

        let poll = ServerPoll {
            lifecycle: vec![
                SlotEvent::Demoted {
                    client_id: 7,
                    cause: HoldingCause::LevelIdentity {
                        expected: "new-map".to_string(),
                        received: "old-map".to_string(),
                    },
                },
                SlotEvent::Participating { client_id: 7 },
            ],
            join_seeds: vec![(7, seed(2.0))],
            ..ServerPoll::default()
        };
        let routed = collect_poll_arrivals(&mut seeds, &poll);
        assert!(matches!(
            routed.as_slice(),
            [(7, JoinSeedArrival::DroppedConsumed)]
        ));
        assert!(matches!(seeds.on_participating(7), ParticipationSeed::None));
    }

    #[test]
    fn poll_route_buffers_entry_seed_and_routes_late_first_seed_immediately() {
        let mut seeds = HostJoinSeeds::default();
        let entry = ServerPoll {
            lifecycle: vec![SlotEvent::Participating { client_id: 7 }],
            join_seeds: vec![(7, seed(42.0))],
            ..ServerPoll::default()
        };

        let routed = collect_poll_arrivals(&mut seeds, &entry);
        assert!(matches!(
            routed.as_slice(),
            [(7, JoinSeedArrival::Buffered)]
        ));
        assert!(matches!(
            seeds.on_participating(7),
            ParticipationSeed::Apply(values) if values == seed(42.0)
        ));

        let mut late_seeds = HostJoinSeeds::default();
        let no_seed_entry = ServerPoll {
            lifecycle: vec![SlotEvent::Participating { client_id: 8 }],
            ..ServerPoll::default()
        };
        assert!(collect_poll_arrivals(&mut late_seeds, &no_seed_entry).is_empty());
        assert!(matches!(
            late_seeds.on_participating(8),
            ParticipationSeed::None
        ));

        let late = ServerPoll {
            join_seeds: vec![(8, seed(77.0))],
            ..ServerPoll::default()
        };
        let routed = collect_poll_arrivals(&mut late_seeds, &late);
        assert!(matches!(
            routed.as_slice(),
            [(8, JoinSeedArrival::Apply(values))] if values == &seed(77.0)
        ));
    }

    // Regression: level re-entry reapplied the client's boot seed over live seat progress.
    #[test]
    fn level_transition_reprepared_seed_cannot_overwrite_live_per_owner_value() {
        use postretro_net::wire::HoldingCause;

        let client_id = 7;
        let seat = Seat(2);
        let (mut table, identity, membership) = persistent_per_owner_fixture();
        let mut seeds = HostJoinSeeds::default();
        let initial_poll = ServerPoll {
            lifecycle: vec![SlotEvent::Participating { client_id }],
            join_seeds: vec![(client_id, seed(10.0))],
            ..ServerPoll::default()
        };

        assert!(matches!(
            collect_poll_arrivals(&mut seeds, &initial_poll).as_slice(),
            [(7, JoinSeedArrival::Buffered)]
        ));
        let ParticipationSeed::Apply(values) = seeds.on_participating(client_id) else {
            panic!("initial poll must apply its buffered seed at participation");
        };
        assert!(
            apply_join_seed(&mut table, Some(&identity), &membership, seat, values,).is_empty()
        );
        table
            .get_mut("game.xp")
            .expect("xp fixture")
            .set_per_seat_value(seat, SlotValue::Number(99.0));

        let level_reentry = ServerPoll {
            lifecycle: vec![
                SlotEvent::Demoted {
                    client_id,
                    cause: HoldingCause::LevelIdentity {
                        expected: "new-map".to_string(),
                        received: "old-map".to_string(),
                    },
                },
                SlotEvent::Participating { client_id },
            ],
            join_seeds: vec![(client_id, seed(10.0))],
            ..ServerPoll::default()
        };
        assert!(matches!(
            collect_poll_arrivals(&mut seeds, &level_reentry).as_slice(),
            [(7, JoinSeedArrival::DroppedConsumed)]
        ));
        assert!(matches!(
            seeds.on_participating(client_id),
            ParticipationSeed::None
        ));
        assert_eq!(
            table
                .get("game.xp")
                .expect("xp fixture")
                .per_seat_value(seat),
            Some(&SlotValue::Number(99.0))
        );
    }

    #[test]
    fn late_poll_seed_routes_through_host_state_and_applies_to_admitted_seat() {
        let client_id = 8;
        let seat = Seat(3);
        let (mut table, identity, membership) = persistent_per_owner_fixture();
        let mut seeds = HostJoinSeeds::default();
        let entry_without_seed = ServerPoll {
            lifecycle: vec![SlotEvent::Participating { client_id }],
            ..ServerPoll::default()
        };

        assert!(collect_poll_arrivals(&mut seeds, &entry_without_seed).is_empty());
        assert!(matches!(
            seeds.on_participating(client_id),
            ParticipationSeed::None
        ));

        let late_poll = ServerPoll {
            join_seeds: vec![(client_id, seed(77.0))],
            ..ServerPoll::default()
        };
        let mut routed = collect_poll_arrivals(&mut seeds, &late_poll);
        let (routed_client, JoinSeedArrival::Apply(values)) = routed
            .pop()
            .expect("late poll must produce one routed seed")
        else {
            panic!("late poll must apply the first seed immediately");
        };
        assert_eq!(routed_client, client_id);
        assert!(
            apply_join_seed(&mut table, Some(&identity), &membership, seat, values,).is_empty()
        );
        assert_eq!(
            table
                .get("game.xp")
                .expect("xp fixture")
                .per_seat_value(seat),
            Some(&SlotValue::Number(77.0))
        );
    }
}
