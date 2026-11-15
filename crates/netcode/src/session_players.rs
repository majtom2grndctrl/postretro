// Per-seat player status and the session player counts derived from it.
// See: context/lib/scripting.md §5 (Session players)

use postretro_entities::engine_state_collections::{LIST_PRESENT_FIELD, SESSION_PLAYERS_LIST};
use postretro_entities::slot_table::{SlotTable, SlotValue};
use postretro_entities::{EntityId, EntityRegistry};
use postretro_foundation::Seat;

use crate::SeatTable;

/// One seat's place in the session, derived on the host each tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SeatStatus {
    /// Connected with no live pawn: loading, never loaded, or demoted mid-level.
    Loading,
    /// Live pawn the effective occupancy rule counts.
    InPlay,
    /// Live pawn the effective occupancy rule does not count.
    Down,
    /// Remote seat held after a disconnect.
    Reconnecting,
}

impl SeatStatus {
    /// Script-facing status name, the enum value a `session.players` row reads.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Loading => "loading",
            Self::InPlay => "inPlay",
            Self::Down => "down",
            Self::Reconnecting => "reconnecting",
        }
    }

    /// The roster's `status` byte.
    #[must_use]
    pub const fn roster_code(self) -> u8 {
        use postretro_net::wire::roster_status as code;
        match self {
            Self::Loading => code::LOADING,
            Self::InPlay => code::IN_PLAY,
            Self::Down => code::DOWN,
            Self::Reconnecting => code::RECONNECTING,
        }
    }

    /// The status a roster byte names; `None` for a byte no status uses.
    #[must_use]
    pub const fn from_roster_code(code: u8) -> Option<Self> {
        use postretro_net::wire::roster_status as status;
        match code {
            status::LOADING => Some(Self::Loading),
            status::IN_PLAY => Some(Self::InPlay),
            status::DOWN => Some(Self::Down),
            status::RECONNECTING => Some(Self::Reconnecting),
            _ => None,
        }
    }
}

/// Every seat's status by kind. `connected` excludes reconnecting seats.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlayerCounts {
    pub loading: u32,
    pub in_play: u32,
    pub down: u32,
    pub reconnecting: u32,
}

impl PlayerCounts {
    #[must_use]
    pub const fn connected(&self) -> u32 {
        self.loading + self.in_play + self.down
    }

    fn add(&mut self, status: SeatStatus) {
        let count = match status {
            SeatStatus::Loading => &mut self.loading,
            SeatStatus::InPlay => &mut self.in_play,
            SeatStatus::Down => &mut self.down,
            SeatStatus::Reconnecting => &mut self.reconnecting,
        };
        *count += 1;
    }
}

/// The trigger stage's view of player pawns for one tick: the pawns it
/// captured and, of those, the ones the effective occupancy rule counts.
/// Status reads this capture, never a later health read, so a player killed
/// after the capture still counts for the tick its occupancy came from.
#[derive(Debug, Clone, Copy)]
pub struct CapturedPawns<'a> {
    pub captured: &'a [EntityId],
    pub counted: &'a [EntityId],
}

impl CapturedPawns<'static> {
    /// No capture: install, before the first authoritative tick. Every seat with
    /// a live pawn reads loading until a tick captures it.
    pub const NONE: Self = Self {
        captured: &[],
        counted: &[],
    };
}

/// Derive one seat's status. The live pawn comes from the registry, not the
/// seat table's pawn binding, which outlives a demotion's despawn.
#[must_use]
pub fn seat_status(
    seats: &SeatTable,
    registry: &EntityRegistry,
    pawns: CapturedPawns<'_>,
    seat: Seat,
) -> SeatStatus {
    if seat != Seat(0) && seats.client_for_seat(seat).is_none() {
        return SeatStatus::Reconnecting;
    }
    let Some(pawn) = registry
        .pawn_for_seat(seat)
        .filter(|&pawn| registry.exists(pawn))
    else {
        return SeatStatus::Loading;
    };
    if pawns.counted.contains(&pawn) {
        SeatStatus::InPlay
    } else if pawns.captured.contains(&pawn) {
        SeatStatus::Down
    } else {
        // Spawned after this tick's capture: it is not in play until a tick
        // captures it.
        SeatStatus::Loading
    }
}

/// Derive every seat's status into `out`, in seat order, and return the counts.
/// `out` is cleared first and reused across ticks.
pub fn derive_seat_statuses(
    seats: &SeatTable,
    registry: &EntityRegistry,
    pawns: CapturedPawns<'_>,
    out: &mut Vec<(Seat, SeatStatus)>,
) -> PlayerCounts {
    out.clear();
    let mut counts = PlayerCounts::default();
    seats.for_each_seat(|seat| {
        let status = seat_status(seats, registry, pawns, seat);
        counts.add(status);
        out.push((seat, status));
    });
    counts
}

const PLAYER_COUNT_SLOTS: [&str; 5] = [
    "session.playerCounts.connected",
    "session.playerCounts.loading",
    "session.playerCounts.inPlay",
    "session.playerCounts.down",
    "session.playerCounts.reconnecting",
];

/// Write the counts into their shared engine slots, touching only the slots
/// whose value changed. Engine writes bypass the slots' script readonly flag.
pub fn write_player_counts(slot_table: &mut SlotTable, counts: PlayerCounts) {
    let values = [
        counts.connected(),
        counts.loading,
        counts.in_play,
        counts.down,
        counts.reconnecting,
    ];
    for (name, value) in PLAYER_COUNT_SLOTS.into_iter().zip(values) {
        let Some(slot) = slot_table.get_mut(name) else {
            continue;
        };
        // Counts stay far below 2^24, so the f32 slot holds them exactly.
        let value = SlotValue::Number(value as f32);
        if slot.value.as_ref() != Some(&value) {
            slot.write_value(Some(value));
        }
    }
}

/// One seat's row in `session.players`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlayerRow<'a> {
    pub seat: Seat,
    /// The display name; empty reads `Player N`.
    pub name: &'a str,
    pub status: SeatStatus,
    /// The row holding this machine's own seat.
    pub is_local: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WrittenRow {
    seat: Seat,
    name: String,
    status: SeatStatus,
    is_local: bool,
}

/// Writes `session.players` rows, touching only rows whose content changed.
/// Rows fill densely in the order given, up to the list's capacity; rows past
/// the last seat read `present` false.
#[derive(Debug, Default)]
pub struct PlayerRowsWriter {
    /// What each row index last showed; `None` for an absent row.
    written: Vec<Option<WrittenRow>>,
}

impl PlayerRowsWriter {
    pub fn write<'a>(
        &mut self,
        slot_table: &mut SlotTable,
        rows: impl IntoIterator<Item = PlayerRow<'a>>,
    ) {
        let list = &SESSION_PLAYERS_LIST;
        self.written.resize(list.capacity, None);
        let mut rows = rows.into_iter();
        for (index, written) in self.written.iter_mut().enumerate() {
            let row = rows.next();
            let unchanged = match (&*written, &row) {
                (None, None) => true,
                (Some(written), Some(row)) => {
                    written.seat == row.seat
                        && written.name == row.name
                        && written.status == row.status
                        && written.is_local == row.is_local
                }
                _ => false,
            };
            if unchanged {
                continue;
            }
            match row {
                Some(row) => {
                    let shown = if row.name.is_empty() {
                        // Seat numbers are stable for the session, so a row
                        // keeps its fallback when an earlier seat leaves.
                        format!("Player {}", u32::from(row.seat.0) + 1)
                    } else {
                        row.name.to_string()
                    };
                    write_row(
                        slot_table,
                        index,
                        true,
                        SlotValue::String(shown),
                        row.status.name(),
                        row.is_local,
                    );
                    *written = Some(WrittenRow {
                        seat: row.seat,
                        name: row.name.to_string(),
                        status: row.status,
                        is_local: row.is_local,
                    });
                }
                None => {
                    write_row(
                        slot_table,
                        index,
                        false,
                        SlotValue::String(String::new()),
                        SeatStatus::Loading.name(),
                        false,
                    );
                    *written = None;
                }
            }
        }
    }

    /// Every row reads absent.
    pub fn clear(&mut self, slot_table: &mut SlotTable) {
        self.write(slot_table, std::iter::empty());
    }
}

fn write_row(
    slot_table: &mut SlotTable,
    index: usize,
    present: bool,
    name: SlotValue,
    status: &str,
    is_local: bool,
) {
    let list = &SESSION_PLAYERS_LIST;
    for (field, value) in [
        (LIST_PRESENT_FIELD, SlotValue::Boolean(present)),
        ("name", name),
        ("status", SlotValue::Enum(status.to_string())),
        ("isLocal", SlotValue::Boolean(is_local)),
    ] {
        if let Some(slot) = slot_table.get_mut(&list.slot_name(index, field))
            && slot.value.as_ref() != Some(&value)
        {
            slot.write_value(Some(value));
        }
    }
}

/// The client's rows, from the host's roster: every entry in seat order,
/// `isLocal` on the entry holding `local_seat`. An entry with an unknown status
/// byte reads loading.
pub fn roster_rows(
    entries: &[postretro_net::wire::RosterEntry],
    local_seat: Option<Seat>,
) -> impl Iterator<Item = PlayerRow<'_>> {
    entries.iter().map(move |entry| PlayerRow {
        seat: Seat(entry.seat),
        name: &entry.name,
        status: SeatStatus::from_roster_code(entry.status).unwrap_or(SeatStatus::Loading),
        is_local: local_seat == Some(Seat(entry.seat)),
    })
}

/// The host's and single player's rows, from its derived statuses and its
/// seats' claims. The host's own seat is seat 0.
pub fn seat_rows<'a>(
    seats: &'a SeatTable,
    statuses: &'a [(Seat, SeatStatus)],
) -> impl Iterator<Item = PlayerRow<'a>> {
    statuses.iter().map(move |&(seat, status)| PlayerRow {
        seat,
        name: seats.display_name(seat),
        status,
        is_local: seat == Seat(0),
    })
}

#[cfg(test)]
#[path = "session_players_test.rs"]
mod tests;
