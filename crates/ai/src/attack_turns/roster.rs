// Every turn standing at the end of the last tick. A turn missing from its
// holder this tick ended, however it ended: released by its own rule, cleared
// by a reseat or a reload, or its holder gone from the AI set or the world.
// That is the one place a turn's refresh starts.
// See: context/lib/entity_model.md §7c (Attack budget)

use postretro_entities::EntityId;
use postretro_entities::components::brain::{AttackTurnState, HeldTurn};

use super::token_remap::TokenRemap;

#[derive(Clone, Copy)]
struct RosterEntry {
    entity: EntityId,
    turn: HeldTurn,
    visited: bool,
}

/// Reused storage, sorted by holder.
#[derive(Default)]
pub(crate) struct TurnRoster {
    entries: Vec<RosterEntry>,
}

impl TurnRoster {
    /// Each turn `entity` held at the end of last tick that `turns` no longer
    /// holds. A turn is its attack on its target.
    pub(crate) fn ended_for(
        &mut self,
        entity: EntityId,
        turns: &AttackTurnState,
        mut ended: impl FnMut(&HeldTurn),
    ) {
        let start = self.entries.partition_point(|entry| entry.entity < entity);
        for entry in self.entries[start..]
            .iter_mut()
            .take_while(|entry| entry.entity == entity)
        {
            entry.visited = true;
            let still_held = turns
                .held()
                .any(|turn| turn.attack == entry.turn.attack && turn.target == entry.turn.target);
            if !still_held {
                ended(&entry.turn);
            }
        }
    }

    /// Each turn whose holder this tick never reached: it left the AI set or
    /// the world.
    pub(crate) fn departed(&self, mut ended: impl FnMut(&HeldTurn)) {
        for entry in self.entries.iter().filter(|entry| !entry.visited) {
            ended(&entry.turn);
        }
    }

    /// Record the turns standing now.
    pub(crate) fn rebuild<'a>(
        &mut self,
        holders: impl Iterator<Item = (EntityId, &'a AttackTurnState)>,
    ) {
        self.entries.clear();
        for (entity, turns) in holders {
            for turn in turns.held() {
                self.entries.push(RosterEntry {
                    entity,
                    turn: *turn,
                    visited: false,
                });
            }
        }
        self.entries.sort_unstable_by_key(|entry| entry.entity);
    }

    /// A turn that ended after the roster was taken, already accounted for.
    pub(crate) fn remove(&mut self, entity: EntityId, attack: u16) {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.entity == entity && entry.turn.attack == attack)
        {
            self.entries.remove(index);
        }
    }

    pub(crate) fn remap_tokens(&mut self, remap: &TokenRemap) {
        for entry in &mut self.entries {
            entry.turn.draws = remap.draws(&entry.turn.draws);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}
