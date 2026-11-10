// The three attack-turn facts guards read: `hasAttackTurn`, `attackTurnHeld`
// and `attackTurnFree`, all from the previous tick's turns, counts and
// reservations.
// See: context/lib/entity_model.md §7c (Attack budget) · context/lib/scripting.md §11

use postretro_entities::EntityId;
use postretro_entities::components::brain::{AttackPoolId, AttackTurnState, ResolvedDraws};
use postretro_foundation::BehaviorGraphDescriptor;

use super::arbitration::{GroupKey, WaitKey};
use super::budget::{AttackBudgets, BudgetTable};
use super::{attack_index, refreshing_count};
use crate::brain_programs::BrainEntityPrograms;

/// Which named attack-turn read a guard binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttackTurnRead {
    Held,
    Free,
}

/// A waiter as it stood after last tick's arbitration.
#[derive(Clone, Copy)]
struct LineWaiter {
    key: WaitKey,
    cost: u32,
    entity: EntityId,
}

/// One token on one target: what was counted, and the two longest waiters on
/// it, so a reader that is itself the head still sees who is behind it.
#[derive(Clone, Copy, Default)]
struct TokenLine {
    held: u32,
    heads: [Option<LineWaiter>; 2],
}

impl TokenLine {
    fn admit(&mut self, waiter: LineWaiter) {
        match self.heads {
            [Some(first), _] if waiter.key < first.key => {
                self.heads = [Some(waiter), Some(first)];
            }
            [Some(_), Some(second)] if waiter.key >= second.key => {}
            [Some(first), _] => self.heads = [Some(first), Some(waiter)],
            [None, _] => self.heads[0] = Some(waiter),
        }
    }

    /// The longest waiter other than `reader`.
    fn head_excluding(&self, reader: EntityId) -> Option<LineWaiter> {
        self.heads
            .iter()
            .flatten()
            .copied()
            .find(|waiter| waiter.entity != reader)
    }
}

/// The previous tick's turns and line per pool, target and token, rebuilt
/// from the brains as they entered this tick. Reused scratch.
#[derive(Default)]
pub(crate) struct PreviousTickLine {
    groups: Vec<GroupKey>,
    lines: Vec<TokenLine>,
    stride: usize,
}

/// One brain as the line builder reads it: its slot, pool and turn state.
pub(crate) struct LineEntry<'a> {
    pub(crate) entity: EntityId,
    pub(crate) slot: u32,
    pub(crate) pool: AttackPoolId,
    pub(crate) turns: &'a AttackTurnState,
}

impl PreviousTickLine {
    pub(crate) fn rebuild<'a>(
        &mut self,
        budgets: &AttackBudgets,
        entries: impl Iterator<Item = LineEntry<'a>> + Clone,
    ) {
        self.groups.clear();
        for entry in entries.clone() {
            for turn in entry.turns.held() {
                self.groups.push((entry.pool, turn.target));
            }
            if let Some(request) = entry.turns.request {
                self.groups.push((entry.pool, request.target));
            }
        }
        self.groups.sort_unstable();
        self.groups.dedup();
        self.stride = budgets.token_count();
        self.lines.clear();
        self.lines
            .resize(self.groups.len() * self.stride, TokenLine::default());

        for entry in entries {
            for turn in entry.turns.held() {
                for draw in turn.draws.as_slice() {
                    if let Some(line) = self.line_mut((entry.pool, turn.target), draw.token.0) {
                        line.held = line.held.saturating_add(draw.cost);
                    }
                }
            }
            if let (Some(request), Some(wait)) = (entry.turns.request, entry.turns.wait) {
                let key = WaitKey {
                    since_tick: wait.since_tick,
                    slot: entry.slot,
                };
                for draw in request.draws.as_slice() {
                    if let Some(line) = self.line_mut((entry.pool, request.target), draw.token.0) {
                        line.admit(LineWaiter {
                            key,
                            cost: draw.cost,
                            entity: entry.entity,
                        });
                    }
                }
            }
        }
    }

    fn line_index(&self, group: GroupKey, token: u16) -> Option<usize> {
        let token = token as usize;
        if token >= self.stride {
            return None;
        }
        let group = self.groups.binary_search(&group).ok()?;
        Some(group * self.stride + token)
    }

    fn line_mut(&mut self, group: GroupKey, token: u16) -> Option<&mut TokenLine> {
        let index = self.line_index(group, token)?;
        self.lines.get_mut(index)
    }

    /// Whether `draws`, requested by `reader` at `key`, would be granted
    /// against this line, leaving out the reader's own request.
    pub(super) fn would_grant(
        &self,
        group: GroupKey,
        draws: &ResolvedDraws,
        table: &BudgetTable,
        reader: EntityId,
        key: WaitKey,
    ) -> bool {
        draws.as_slice().iter().all(|draw| {
            let Some(limit) = table.limit(draw.token) else {
                return true;
            };
            let line = self
                .line_index(group, draw.token.0)
                .and_then(|index| self.lines.get(index))
                .copied()
                .unwrap_or_default();
            let reserved = line
                .head_excluding(reader)
                .filter(|head| head.key < key)
                .map_or(0, |head| head.cost.min(limit));
            let counted = line
                .held
                .saturating_add(refreshing_count(group, draw.token))
                .saturating_add(reserved);
            counted == 0 || counted.saturating_add(draw.cost) <= limit
        })
    }
}

/// Answers the named reads for one enemy this tick.
pub(crate) struct TurnFactReader<'a> {
    pub(crate) line: &'a PreviousTickLine,
    pub(crate) budgets: &'a AttackBudgets,
    pub(crate) entity: EntityId,
    pub(crate) slot: u32,
    pub(crate) pool: AttackPoolId,
    /// The target selected this tick.
    pub(crate) target: Option<EntityId>,
    pub(crate) turns: &'a AttackTurnState,
    pub(crate) graph: &'a BehaviorGraphDescriptor,
    pub(crate) programs: &'a BrainEntityPrograms,
}

impl TurnFactReader<'_> {
    pub(crate) fn read(&self, attack: &str, read: AttackTurnRead) -> bool {
        let Some(attack) = attack_index(self.graph, attack) else {
            return false;
        };
        match read {
            AttackTurnRead::Held => self.turns.holds_attack(attack),
            AttackTurnRead::Free => self.free(attack),
        }
    }

    fn free(&self, attack: u16) -> bool {
        let Some(target) = self.target else {
            return false;
        };
        let Some(draws) = self.programs.attack_draws(attack) else {
            return false;
        };
        if self
            .turns
            .held()
            .any(|turn| turn.draws.shares_token(&draws))
        {
            return false;
        }
        // The reader's own clock places it in line; a wait on another target
        // would restart, so it reads as a fresh request.
        let key = match self.turns.wait {
            Some(wait) if wait.target == target => WaitKey {
                since_tick: wait.since_tick,
                slot: self.slot,
            },
            _ => WaitKey::fresh(self.slot),
        };
        self.line.would_grant(
            (self.pool, target),
            &draws,
            self.budgets.for_pool(self.pool),
            self.entity,
            key,
        )
    }
}
