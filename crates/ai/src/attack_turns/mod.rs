// Attack turns: the per-target attack budget that makes a group take turns.
// Requests, turn lifetimes, host-only arbitration, the hard gate and the turn
// facts guards read.
// See: context/lib/entity_model.md §7c (Attack budget) · context/lib/scripting.md §11

mod arbitration;
mod budget;
mod draws;
mod facts;
mod lifecycle;
mod pools;
mod refresh;
mod roster;
mod token_remap;

#[cfg(test)]
mod tests;

pub(crate) use budget::AttackBudgets;
pub(crate) use draws::{live_behavior, resolve_graph_draws};
pub(crate) use facts::{AttackTurnRead, LineEntry, PreviousTickLine, TurnFactReader};
pub(crate) use lifecycle::{admit, arbitrate_attack_turns};
pub(crate) use pools::RunningPool;
pub(crate) use refresh::RefreshView;

#[cfg(test)]
pub(crate) use budget::{BudgetTable, TokenBudget};

use std::sync::Arc;

use postretro_entities::components::brain::{AttackPoolId, AttackTurnState, BrainComponent};
use postretro_entities::{ComponentKind, ComponentValue, EntityId, EntityRegistry};
use postretro_foundation::BehaviorGraphDescriptor;
use postretro_foundation::attack_budget::AttackBudgetVocabulary;

use arbitration::Arbiter;
use refresh::RefreshLedger;
use roster::TurnRoster;
use token_remap::TokenRemap;

/// The attack-turn state the AI runtime keeps across ticks: the tick clock
/// wait clocks read, the refresh clock, the budgets, the refresh ledger, and
/// reused scratch. Turns themselves live on brains.
pub(crate) struct AttackTurnRuntime {
    pub(crate) tick: u64,
    /// Elapsed AI time at this tick and at the previous one. Refresh delays
    /// are measured on it; the turn facts read the previous tick's.
    now_ms: f64,
    previous_ms: f64,
    pub(crate) budgets: AttackBudgets,
    /// The committed vocabulary `budgets` was built from.
    vocabulary: Arc<AttackBudgetVocabulary>,
    /// Old to new token ids when this tick's commit shifted them.
    remap: Option<TokenRemap>,
    pub(crate) previous: PreviousTickLine,
    /// Draws still counted after their turn ended, per pool, target and
    /// token. Level-scoped: it names level entities.
    ledger: RefreshLedger,
    /// The turns standing at the end of the last tick.
    roster: TurnRoster,
    arbiter: Arbiter,
}

impl Default for AttackTurnRuntime {
    fn default() -> Self {
        let vocabulary = AttackBudgetVocabulary::shared_default();
        Self {
            tick: 0,
            now_ms: 0.0,
            previous_ms: 0.0,
            budgets: AttackBudgets::from_vocabulary(&vocabulary),
            vocabulary,
            remap: None,
            previous: PreviousTickLine::default(),
            ledger: RefreshLedger::default(),
            roster: TurnRoster::default(),
            arbiter: Arbiter::default(),
        }
    }
}

impl AttackTurnRuntime {
    /// Advance both clocks, adopt a newly committed vocabulary, then drop
    /// refreshing counts that lapsed by the previous tick or whose target
    /// left the world. Runs before anything reads turns; [`Self::sync_pools`]
    /// follows it.
    pub(crate) fn begin_tick(&mut self, dt_ms: f32, registry: &EntityRegistry) {
        self.tick = self.tick.wrapping_add(1);
        self.previous_ms = self.now_ms;
        self.now_ms += f64::from(dt_ms);
        self.sync_budgets(registry.attack_budgets());
        self.ledger
            .prune(self.previous_ms, &self.budgets, |target| {
                registry.exists(target)
            });
    }

    /// Rebuild the budgets when a new vocabulary has been committed. A commit
    /// replaces the pointer, so an unchanged one costs a pointer compare.
    /// Held turns and limits are untouched: a lowered budget revokes nothing.
    fn sync_budgets(&mut self, committed: &Arc<AttackBudgetVocabulary>) {
        if Arc::ptr_eq(&self.vocabulary, committed) {
            self.remap = None;
            return;
        }
        self.remap = TokenRemap::between(&self.vocabulary, committed);
        if let Some(remap) = &self.remap {
            self.ledger.remap_tokens(remap);
            self.roster.remap_tokens(remap);
        }
        self.budgets.resolve(committed);
        self.vocabulary = Arc::clone(committed);
    }

    /// Adopt the running attack pools: each pool's profile resolves by name,
    /// and a pool that stopped or began a new run drops its refreshing counts.
    pub(crate) fn sync_pools<'a>(&mut self, running: impl Iterator<Item = RunningPool<'a>>) {
        let Self {
            budgets,
            vocabulary,
            ledger,
            ..
        } = self;
        budgets.pools.sync(running, vocabulary, |pool| {
            ledger.drop_pool(AttackPoolId::Pool(pool));
        });
    }

    /// Carry one brain's turns across this tick's token-id shift, if any.
    pub(crate) fn remap_brain(&self, turns: &mut AttackTurnState) {
        if let Some(remap) = &self.remap {
            remap.turns(turns);
        }
    }

    /// The refresh ledger as the turn facts read it: at the previous tick.
    pub(crate) fn previous_refresh(&self) -> RefreshView<'_> {
        RefreshView {
            ledger: &self.ledger,
            at_ms: self.previous_ms,
        }
    }

    /// Release a granted unmarked turn whose attack did not start on its
    /// grant tick: nothing fired and no execution began. It ended all the
    /// same, so its refresh starts now.
    pub(crate) fn release_unstarted_grant(
        &mut self,
        entity: EntityId,
        turns: &mut AttackTurnState,
        attack: u16,
    ) {
        let Self {
            now_ms,
            budgets,
            ledger,
            roster,
            ..
        } = self;
        turns.retain_held(|turn| {
            if turn.attack != attack {
                return true;
            }
            ledger.record(
                (turn.pool, turn.target),
                &turn.draws,
                budgets.for_pool(turn.pool),
                *now_ms,
            );
            roster.remove(entity, attack);
            false
        });
    }

    /// Drop everything keyed by the unloading level's entities: refreshing
    /// counts, the turn roster, and the running pools' budgets.
    pub(crate) fn clear_level_state(&mut self) {
        self.ledger.clear();
        self.roster.clear();
        self.budgets.pools.clear();
    }

    /// Refreshing counts held, lapsed or not.
    #[cfg(test)]
    pub(crate) fn refreshing_entries(&self) -> usize {
        self.ledger.len()
    }

    /// How much of `token` is refreshing on `group` now.
    #[cfg(test)]
    pub(crate) fn refreshing_now(
        &self,
        group: arbitration::GroupKey,
        token: postretro_entities::components::brain::AttackTokenId,
    ) -> u32 {
        self.ledger.count(
            group,
            token,
            self.now_ms,
            self.budgets.for_pool(group.0).refresh_ms(token),
        )
    }
}

/// The pool an enemy takes turns in: the running attack pool a start or a
/// tagged spawn assigned it, else the default pool. An assignment naming a
/// pool that is not running this tick reads as the default pool.
pub(crate) fn attack_pool_of(budgets: &AttackBudgets, brain: &BrainComponent) -> AttackPoolId {
    match brain.attack_pool {
        Some(pool) if budgets.pools.is_running(pool) => AttackPoolId::Pool(pool),
        _ => AttackPoolId::Default,
    }
}

/// The attack pools running this tick, with their profiles, in slot order.
/// Reads the pool column in place; nothing allocates.
pub(crate) fn running_attack_pools(
    registry: &EntityRegistry,
) -> impl Iterator<Item = RunningPool<'_>> {
    registry
        .iter_with_kind(ComponentKind::AttackPool)
        .filter_map(|(pool, value)| match value {
            ComponentValue::AttackPool(component) if component.running => Some(RunningPool {
                pool,
                profile: &component.budget_profile,
                run: component.run,
            }),
            _ => None,
        })
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
