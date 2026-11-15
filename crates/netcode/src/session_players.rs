// Per-seat player status and the session player counts derived from it.
// See: context/lib/scripting.md §5 (Session players)

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

#[cfg(test)]
#[path = "session_players_test.rs"]
mod tests;
