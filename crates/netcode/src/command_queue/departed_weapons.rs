// Host-wide memory of where each departed weapon's credited recovery ends.
// See: context/lib/networking.md §Combat authority

use postretro_entities::EntityId;

use super::activation_cadence::CATCH_UP_ALLOWANCE_TICKS;

/// Cadence records leave with their weapon (drop, hand-over, despawn) or with
/// their client (disconnect, demotion). The weapon's own cooldown carries the
/// ticks the record still owed; this remembers, in host simulation ticks, when
/// that owed recovery ends. A later unrecorded start on the weapon, by any
/// client, claims no credit before that end, so leaving and returning never
/// restarts the chain early. An end older than the catch-up allowance can no
/// longer bound any credit and is forgotten.
#[derive(Debug, Default)]
pub(super) struct DepartedWeapons {
    /// Host simulation ticks, counted once per `advance`.
    tick: u32,
    /// Records of clients that left, charged on the next `advance`.
    pending: Vec<(EntityId, u32)>,
    /// Host tick at which each recently departed weapon's owed recovery ends.
    chain_ends: Vec<(EntityId, u32)>,
}

impl DepartedWeapons {
    /// One host simulation tick passed. Returns the departures of clients that
    /// left since the last tick, for the caller to charge with this tick's.
    pub fn advance(&mut self) -> Vec<(EntityId, u32)> {
        self.tick = self.tick.wrapping_add(1);
        let now = self.tick;
        self.chain_ends
            .retain(|&(_, end)| (now.wrapping_sub(end) as i32) < CATCH_UP_ALLOWANCE_TICKS as i32);
        std::mem::take(&mut self.pending)
    }

    /// Departures of a client that left, charged on the next `advance`. Owed
    /// ticks counted from its last playout tick only grow by that wait.
    pub fn defer(&mut self, departures: impl IntoIterator<Item = (EntityId, u32)>) {
        self.pending.extend(departures);
    }

    /// `weapon` left with `owed_ticks` of its credited recovery still owed.
    pub fn depart(&mut self, weapon: EntityId, owed_ticks: u32) {
        let end = self.tick.wrapping_add(owed_ticks);
        match self.chain_ends.iter_mut().find(|(held, _)| *held == weapon) {
            Some(entry) if (end.wrapping_sub(entry.1) as i32) > 0 => entry.1 = end,
            Some(_) => {}
            None => self.chain_ends.push((weapon, end)),
        }
    }

    /// Host ticks since `weapon`'s departed recovery ended: zero while it still
    /// owes, and the allowance once nothing remembered can bound its credit.
    pub fn cool_ticks(&self, weapon: EntityId) -> u32 {
        self.chain_ends
            .iter()
            .find(|(held, _)| *held == weapon)
            .map_or(CATCH_UP_ALLOWANCE_TICKS, |&(_, end)| {
                (self.tick.wrapping_sub(end) as i32)
                    .clamp(0, CATCH_UP_ALLOWANCE_TICKS as i32)
                    .unsigned_abs()
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn departed_weapon_cools_from_the_end_of_its_owed_recovery_and_is_then_forgotten() {
        let rifle = EntityId::from_raw(9);
        let mut departed = DepartedWeapons::default();
        assert_eq!(departed.cool_ticks(rifle), CATCH_UP_ALLOWANCE_TICKS);
        departed.advance();
        departed.depart(rifle, 5);
        assert_eq!(departed.cool_ticks(rifle), 0, "still owed");
        for _ in 0..5 {
            departed.advance();
        }
        assert_eq!(departed.cool_ticks(rifle), 0, "owed recovery just ended");
        departed.advance();
        assert_eq!(departed.cool_ticks(rifle), 1);
        departed.depart(rifle, 0);
        assert_eq!(
            departed.cool_ticks(rifle),
            0,
            "a later departure moves the end"
        );
        for _ in 0..CATCH_UP_ALLOWANCE_TICKS {
            departed.advance();
        }
        assert!(
            departed.chain_ends.is_empty(),
            "an end past the allowance is forgotten"
        );
        assert_eq!(departed.cool_ticks(rifle), CATCH_UP_ALLOWANCE_TICKS);
    }

    #[test]
    fn departed_client_records_are_charged_on_the_next_tick() {
        let rifle = EntityId::from_raw(9);
        let mut departed = DepartedWeapons::default();
        departed.defer([(rifle, 4)]);
        assert_eq!(departed.advance(), vec![(rifle, 4)]);
        assert!(departed.advance().is_empty());
    }
}
