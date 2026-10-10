//! The per-tick snapshot evaluation: every (event, player) condition reads the
//! settled tick before any fire applies.
//! See: context/lib/scripting.md §12 (Player events)

use postretro_entities::{EntityId, EntityRegistry, SlotTable};
use postretro_foundation::{IrValue, eval_value};
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

/// Buffers reused every tick, so steady-state evaluation allocates nothing.
#[derive(Default)]
pub(super) struct Scratch {
    pawns: Vec<EntityId>,
    players: Vec<(PlayerKey, EntityId)>,
    /// Row-major by event: `None` while the player is unobserved for it.
    values: Vec<Option<bool>>,
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
            .resize(self.events.len() * player_count, None);
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
                let holds = matches!(eval_value(&event.program, scope), IrValue::Bool(true));
                scratch.values[event_index * player_count + player_index] = Some(holds);
            }
        }

        for (event_index, event) in self.events.iter_mut().enumerate() {
            scratch.memory.clear();
            for (player_index, &(key, pawn)) in scratch.players.iter().enumerate() {
                // Unobserved: the player drops out of memory and reads as
                // false at its next observation. Becoming unobserved fires
                // nothing.
                let Some(holds) = scratch.values[event_index * player_count + player_index] else {
                    continue;
                };
                let held = event
                    .memory
                    .iter()
                    .find(|(remembered, _)| *remembered == key)
                    .is_some_and(|(_, held)| *held);
                let fires = match event.edge {
                    PlayerEventEdge::Becomes => !held && holds,
                    PlayerEventEdge::Ceases => held && !holds,
                };
                if fires {
                    scratch.fires.push(super::evaluate::Fire {
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
