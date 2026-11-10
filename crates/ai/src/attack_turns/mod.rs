// Attack turns: the per-target attack budget that makes a group take turns.
// Requests, turn lifetimes, host-only arbitration, the hard gate and the turn
// facts guards read.
// See: context/lib/entity_model.md §7c (Attack budget) · context/lib/scripting.md §11

mod arbitration;
mod budget;
mod facts;
mod lifecycle;

#[cfg(test)]
mod tests;

pub(crate) use budget::{AttackBudgets, resolve_attack_draws};
pub(crate) use facts::{AttackTurnRead, LineEntry, PreviousTickLine, TurnFactReader};
pub(crate) use lifecycle::{admit, arbitrate_attack_turns, release_unstarted_grant};

#[cfg(test)]
pub(crate) use budget::{BudgetTable, TokenBudget};

use std::sync::Arc;

use postretro_entities::EntityId;
use postretro_entities::components::brain::{AttackPoolId, AttackTokenId, BrainComponent};
use postretro_foundation::BehaviorGraphDescriptor;
use postretro_foundation::attack_budget::AttackBudgetVocabulary;

use arbitration::{Arbiter, GroupKey};

/// The attack-turn state the AI runtime keeps across ticks: the tick clock
/// wait clocks read, the budgets, and reused scratch. Turns themselves live
/// on brains.
pub(crate) struct AttackTurnRuntime {
    pub(crate) tick: u64,
    pub(crate) budgets: AttackBudgets,
    /// The committed vocabulary `budgets` was built from.
    vocabulary: Arc<AttackBudgetVocabulary>,
    pub(crate) previous: PreviousTickLine,
    arbiter: Arbiter,
}

impl Default for AttackTurnRuntime {
    fn default() -> Self {
        let vocabulary = AttackBudgetVocabulary::shared_default();
        Self {
            tick: 0,
            budgets: AttackBudgets::from_vocabulary(&vocabulary),
            vocabulary,
            previous: PreviousTickLine::default(),
            arbiter: Arbiter::default(),
        }
    }
}

impl AttackTurnRuntime {
    /// Rebuild the budgets when a new vocabulary has been committed. A commit
    /// replaces the pointer, so an unchanged one costs a pointer compare.
    pub(crate) fn sync_budgets(&mut self, committed: &Arc<AttackBudgetVocabulary>) {
        if !Arc::ptr_eq(&self.vocabulary, committed) {
            self.budgets = AttackBudgets::from_vocabulary(committed);
            self.vocabulary = Arc::clone(committed);
        }
    }
}

/// The pool an enemy takes turns in. Every enemy is in the default pool until
/// `ai_attack_pool` assigns members.
pub(crate) fn attack_pool_of(_enemy: EntityId) -> AttackPoolId {
    AttackPoolId::Default
}

/// Draws still counted after their turn ended, per pool, target and token.
/// Nothing refreshes until tokens carry `refreshMs`.
pub(crate) fn refreshing_count(_group: GroupKey, _token: AttackTokenId) -> u32 {
    0
}

/// An attack's index in its graph's `attacks`, the identity turns store.
pub(crate) fn attack_index(graph: &BehaviorGraphDescriptor, attack: &str) -> Option<u16> {
    graph
        .attacks
        .keys()
        .position(|name| name == attack)
        .and_then(|index| u16::try_from(index).ok())
}

pub(crate) fn attack_name(graph: &BehaviorGraphDescriptor, attack: u16) -> Option<&str> {
    graph
        .attacks
        .keys()
        .nth(attack as usize)
        .map(String::as_str)
}

/// The attack marked on the active path. Load guarantees every mark on one
/// path names the same attack.
pub(crate) fn path_mark(brain: &BrainComponent) -> Option<u16> {
    (0..brain.active_depth())
        .filter_map(|depth| brain.activity_at_depth(depth))
        .find_map(|(_, activity)| activity.attack_turn.as_deref())
        .and_then(|attack| attack_index(&brain.graph, attack))
}
