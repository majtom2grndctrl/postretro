// Session player facts: per-seat status, the shared player counts, and when a
// level counts as a session level.
// See: context/lib/scripting.md §5 (Session players)

use postretro_entities::EntityRegistry;
use postretro_entities::slot_table::SlotTable;
use postretro_foundation::Seat;

use crate::netcode::session_players::{
    CapturedPawns, PlayerCounts, PlayerRowsWriter, SeatStatus, derive_seat_statuses, roster_rows,
    seat_rows, write_player_counts,
};
use crate::netcode::{ClientSessionStatus, SeatTable};

/// Session player status, counts and the `session.players` rows. The host and
/// single player derive all three; a client receives the counts through slot
/// replication and fills its rows from the host's roster.
#[derive(Debug, Default)]
pub(crate) struct SessionPlayers {
    /// A level a session plays is installed. A frontend backdrop is not one, nor
    /// is the gap between unload and the next install.
    session_level: bool,
    /// Every seat's latest status, in seat order; reused across ticks.
    statuses: Vec<(Seat, SeatStatus)>,
    rows: PlayerRowsWriter,
}

impl SessionPlayers {
    /// A level a session plays is installed.
    pub(crate) fn session_level(&self) -> bool {
        self.session_level
    }

    #[cfg(test)]
    pub(crate) fn statuses(&self) -> &[(Seat, SeatStatus)] {
        &self.statuses
    }

    /// A level installs. On the host and in single player, a session level
    /// writes its counts and rows from the seats before crossings initialize:
    /// no pawn has a tick capture yet, so every connected seat reads loading.
    /// A client's session level shows the roster it already holds.
    pub(crate) fn begin_level(
        &mut self,
        session_level: bool,
        seats: Option<&mut SeatTable>,
        client: Option<&ClientSessionStatus>,
        registry: &EntityRegistry,
        slot_table: &mut SlotTable,
    ) {
        self.session_level = session_level;
        match (seats, client) {
            (Some(seats), _) => self.publish(seats, registry, CapturedPawns::NONE, slot_table),
            (None, Some(client)) => self.apply_roster(client, slot_table),
            (None, None) => {}
        }
    }

    /// One authoritative tick, after the trigger stage settles and before
    /// accumulators and crossing detection. Host and single player only.
    pub(crate) fn publish_tick(
        &mut self,
        seats: Option<&mut SeatTable>,
        registry: &EntityRegistry,
        pawns: CapturedPawns<'_>,
        slot_table: &mut SlotTable,
    ) {
        if let Some(seats) = seats {
            self.publish(seats, registry, pawns, slot_table);
        }
    }

    /// A client received the host's roster: show it while a session level is
    /// installed.
    pub(crate) fn apply_roster(&mut self, client: &ClientSessionStatus, slot_table: &mut SlotTable) {
        if self.session_level {
            self.rows
                .write(slot_table, roster_rows(client.entries(), client.local_seat()));
        }
    }

    /// The level ends, on every role: counts read 0 and rows read absent until
    /// the next install writes them, and on a machine left with no session
    /// level. The host's roster drops back to connection-only statuses.
    pub(crate) fn end_level(&mut self, seats: Option<&mut SeatTable>, slot_table: &mut SlotTable) {
        self.session_level = false;
        self.statuses.clear();
        if let Some(seats) = seats {
            seats.clear_statuses();
        }
        write_player_counts(slot_table, PlayerCounts::default());
        self.rows.clear(slot_table);
    }

    fn publish(
        &mut self,
        seats: &mut SeatTable,
        registry: &EntityRegistry,
        pawns: CapturedPawns<'_>,
        slot_table: &mut SlotTable,
    ) {
        if !self.session_level {
            self.statuses.clear();
            write_player_counts(slot_table, PlayerCounts::default());
            self.rows.clear(slot_table);
            return;
        }
        let counts = derive_seat_statuses(seats, registry, pawns, &mut self.statuses);
        write_player_counts(slot_table, counts);
        seats.record_statuses(&self.statuses);
        self.rows.write(slot_table, seat_rows(seats, &self.statuses));
    }
}
