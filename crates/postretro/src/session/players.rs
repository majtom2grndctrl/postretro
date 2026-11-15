// Session player facts: per-seat status, the shared player counts, and when a
// level counts as a session level.
// See: context/lib/scripting.md §5 (Session players)

use postretro_entities::EntityRegistry;
use postretro_entities::slot_table::SlotTable;
use postretro_foundation::Seat;

use crate::netcode::SeatTable;
use crate::netcode::session_players::{
    CapturedPawns, PlayerCounts, SeatStatus, derive_seat_statuses, write_player_counts,
};

/// Host-side session player state. Clients receive the counts through slot
/// replication and only zero them when their level ends.
#[derive(Debug, Default)]
pub(crate) struct SessionPlayers {
    /// A level a session plays is installed. A frontend backdrop is not one, nor
    /// is the gap between unload and the next install.
    session_level: bool,
    /// Every seat's latest status, in seat order; reused across ticks.
    statuses: Vec<(Seat, SeatStatus)>,
}

impl SessionPlayers {
    #[cfg(test)]
    pub(crate) fn session_level(&self) -> bool {
        self.session_level
    }

    #[cfg(test)]
    pub(crate) fn statuses(&self) -> &[(Seat, SeatStatus)] {
        &self.statuses
    }

    /// A level installs. On the host and in single player, a session level
    /// writes its counts from the seats before crossings initialize: no pawn
    /// has a tick capture yet, so every connected seat reads loading.
    pub(crate) fn begin_level(
        &mut self,
        session_level: bool,
        seats: Option<&SeatTable>,
        registry: &EntityRegistry,
        slot_table: &mut SlotTable,
    ) {
        self.session_level = session_level;
        self.publish(seats, registry, CapturedPawns::NONE, slot_table);
    }

    /// One authoritative tick, after the trigger stage settles and before
    /// accumulators and crossing detection. Host and single player only.
    pub(crate) fn publish_tick(
        &mut self,
        seats: Option<&SeatTable>,
        registry: &EntityRegistry,
        pawns: CapturedPawns<'_>,
        slot_table: &mut SlotTable,
    ) {
        self.publish(seats, registry, pawns, slot_table);
    }

    /// The level ends, on every role: counts read 0 until the next install
    /// writes them, and on a machine left with no session level.
    pub(crate) fn end_level(&mut self, slot_table: &mut SlotTable) {
        self.session_level = false;
        self.statuses.clear();
        write_player_counts(slot_table, PlayerCounts::default());
    }

    fn publish(
        &mut self,
        seats: Option<&SeatTable>,
        registry: &EntityRegistry,
        pawns: CapturedPawns<'_>,
        slot_table: &mut SlotTable,
    ) {
        let counts = match seats {
            Some(seats) if self.session_level => {
                derive_seat_statuses(seats, registry, pawns, &mut self.statuses)
            }
            _ => {
                self.statuses.clear();
                PlayerCounts::default()
            }
        };
        write_player_counts(slot_table, counts);
    }
}
