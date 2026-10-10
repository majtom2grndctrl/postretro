//! The per-tick snapshot evaluation: every (event, player) condition reads the
//! settled tick before any fire applies.
//! See: context/lib/scripting.md §12 (Player events)

use postretro_entities::{EntityId, EntityRegistry, SlotTable};
use postretro_foundation::{IrValue, eval_value_checked};
use postretro_scripting_core::data_descriptors::PlayerEventEdge;
use postretro_scripting_core::group_resolution::extend_with_player_pawns;

use super::{PlayerEventTable, PlayerKey};

/// One edge to apply this tick: event in composed order, then player in the
/// `players()` group's resolution order.
#[derive(Debug, Clone, Copy)]
pub(super) struct Fire {
    pub(super) event: usize,
    pub(super) player: PlayerKey,
    pub(super) pawn: EntityId,
}

/// One player's condition for one event this tick.
#[derive(Debug, Clone, Copy, Default)]
enum Observation {
    /// No pawn, no seat, or no value for a slot the condition reads.
    #[default]
    Unobserved,
    /// The condition divided by zero or went non-finite: no observation, so
    /// the player keeps last tick's value and no edge fires.
    Invalid,
    Holds(bool),
}

/// Buffers reused every tick, so steady-state evaluation allocates nothing.
#[derive(Default)]
pub(super) struct Scratch {
    pawns: Vec<EntityId>,
    players: Vec<(PlayerKey, EntityId)>,
    /// Row-major by event.
    values: Vec<Observation>,
    memory: Vec<(PlayerKey, bool)>,
    pub(super) fires: Vec<Fire>,
}

impl PlayerEventTable {
    pub(super) fn evaluate(&mut self, registry: &EntityRegistry, slot_table: &SlotTable) {
        let Some(scope) = self.scope.as_ref() else {
            return;
        };
        let scratch = &mut self.scratch;
        scratch.fires.clear();

        // The group's player set, one pawn per player: a seat bound to two
        // pawns is observed through the first in registry slot order.
        scratch.pawns.clear();
        extend_with_player_pawns(registry, None, &mut scratch.pawns);
        scratch.players.clear();
        for &pawn in &scratch.pawns {
            let key = registry
                .seat_for_pawn(pawn)
                .map_or(PlayerKey::Unseated(pawn), PlayerKey::Seat);
            if !scratch.players.iter().any(|(seen, _)| *seen == key) {
                scratch.players.push((key, pawn));
            }
        }

        let player_count = scratch.players.len();
        scratch.values.clear();
        scratch
            .values
            .resize(self.events.len() * player_count, Observation::Unobserved);
        for (player_index, &(key, pawn)) in scratch.players.iter().enumerate() {
            let seat = match key {
                PlayerKey::Seat(seat) => Some(seat),
                PlayerKey::Unseated(_) => None,
            };
            scope.seed_player(registry, slot_table, pawn, seat);
            for (event_index, event) in self.events.iter().enumerate() {
                if !scope.observes(&event.inputs) {
                    continue;
                }
                // A non-finite or divide-by-zero result is not an observation:
                // the totalizing value (0) would read as a real `false` and
                // could fire a `ceases` edge, and dropping the player would
                // re-fire `becomes` once the value recovers.
                scratch.values[event_index * player_count + player_index] =
                    match eval_value_checked(&event.program, scope) {
                        Ok(value) => Observation::Holds(matches!(value, IrValue::Bool(true))),
                        Err(_) => Observation::Invalid,
                    };
            }
        }

        for (event_index, event) in self.events.iter_mut().enumerate() {
            scratch.memory.clear();
            for (player_index, &(key, pawn)) in scratch.players.iter().enumerate() {
                let remembered = event
                    .memory
                    .iter()
                    .find(|(remembered, _)| *remembered == key)
                    .map(|(_, held)| *held);
                let holds = match scratch.values[event_index * player_count + player_index] {
                    // Unobserved: the player drops out of memory and reads as
                    // false at its next observation. Becoming unobserved
                    // fires nothing.
                    Observation::Unobserved => continue,
                    // Invalid: keep what was remembered, fire nothing.
                    Observation::Invalid => {
                        if let Some(held) = remembered {
                            scratch.memory.push((key, held));
                        }
                        continue;
                    }
                    Observation::Holds(holds) => holds,
                };
                let held = remembered.unwrap_or(false);
                let fires = match event.edge {
                    PlayerEventEdge::Becomes => !held && holds,
                    PlayerEventEdge::Ceases => held && !holds,
                };
                if fires {
                    scratch.fires.push(Fire {
                        event: event_index,
                        player: key,
                        pawn,
                    });
                }
                scratch.memory.push((key, holds));
            }
            std::mem::swap(&mut event.memory, &mut scratch.memory);
        }
    }
}
