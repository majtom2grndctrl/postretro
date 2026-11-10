// Attack token budgets per pool, and the one chokepoint that resolves what an
// attack draws.
// See: context/lib/entity_model.md §7c (Attack budget)

use std::collections::HashSet;

use postretro_entities::components::brain::{
    AttackPoolId, AttackTokenId, MAX_ATTACK_DRAWS, ResolvedDraw, ResolvedDraws,
};
use postretro_foundation::AttackParams;
use postretro_foundation::attack_budget::AttackBudgetVocabulary;
pub(crate) use postretro_foundation::attack_budget::TokenBudget;

/// One profile's budgets, indexed by token id.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BudgetTable {
    tokens: Vec<TokenBudget>,
}

impl BudgetTable {
    pub(crate) fn new(tokens: Vec<TokenBudget>) -> Self {
        Self { tokens }
    }

    /// Each registered token at its declared budget.
    pub(crate) fn declared(vocabulary: &AttackBudgetVocabulary) -> Self {
        Self::new(vocabulary.declared_budgets().collect())
    }

    pub(crate) fn token_count(&self) -> usize {
        self.tokens.len()
    }

    /// A token outside the table has no budget and never limits a grant.
    pub(crate) fn limit(&self, token: AttackTokenId) -> Option<u32> {
        self.tokens.get(token.0 as usize).map(|budget| budget.limit)
    }
}

/// The budget table each pool arbitrates under. Every pool shares one token
/// vocabulary, so tables differ only in their values.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AttackBudgets {
    default: BudgetTable,
}

impl AttackBudgets {
    pub(crate) fn new(default: BudgetTable) -> Self {
        Self { default }
    }

    /// The default pool runs at the declared budgets until profiles apply.
    pub(crate) fn from_vocabulary(vocabulary: &AttackBudgetVocabulary) -> Self {
        Self::new(BudgetTable::declared(vocabulary))
    }

    /// Only the default pool's `default` profile exists until attack pools
    /// name profiles.
    pub(crate) fn for_pool(&self, _pool: AttackPoolId) -> &BudgetTable {
        &self.default
    }

    /// Tokens in the shared vocabulary.
    pub(crate) fn token_count(&self) -> usize {
        self.default.token_count()
    }
}

impl Default for AttackBudgets {
    fn default() -> Self {
        Self::from_vocabulary(&AttackBudgetVocabulary::default())
    }
}

/// THE draw-resolution chokepoint: what one attack draws, as token ids in the
/// registered vocabulary. An attack without `draws` draws one `defaultToken`.
/// Runs at bind time, never per tick. A reference naming no registered token
/// (stale content load validation never saw) warns once and is left out.
pub(crate) fn resolve_attack_draws(
    attack: &AttackParams,
    vocabulary: &AttackBudgetVocabulary,
    warned: &mut HashSet<String>,
) -> ResolvedDraws {
    let resolved: Vec<ResolvedDraw> = vocabulary
        .resolve_draws(attack.draws.as_deref(), |token| {
            if warned.insert(format!("attack-draw:{token}")) {
                log::warn!(
                    "[AI] an attack draws unregistered attack token `{token}`; the draw is ignored"
                );
            }
        })
        .into_iter()
        .take(MAX_ATTACK_DRAWS)
        .map(|(token, cost)| ResolvedDraw {
            token: AttackTokenId(token),
            cost,
        })
        .collect();
    ResolvedDraws::from_slice(&resolved).expect("draw lists are capped at MAX_ATTACK_DRAWS")
}
