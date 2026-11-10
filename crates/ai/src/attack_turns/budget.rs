// Attack token budgets: one table per profile, and the table each pool
// arbitrates under.
// See: context/lib/entity_model.md §7c (Attack budget)

use postretro_entities::components::brain::{AttackPoolId, AttackTokenId};
pub(crate) use postretro_foundation::attack_budget::TokenBudget;
use postretro_foundation::attack_budget::{AttackBudgetVocabulary, DEFAULT_PROFILE_NAME};

use super::pools::PoolBudgets;

/// One profile's budgets, indexed by token id.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BudgetTable {
    tokens: Vec<TokenBudget>,
}

impl BudgetTable {
    pub(crate) fn new(tokens: Vec<TokenBudget>) -> Self {
        Self { tokens }
    }

    /// Profile `name`'s budgets: the declared budgets with its named fields
    /// applied. `None` when the mod declares no profile by that name.
    pub(crate) fn profile(vocabulary: &AttackBudgetVocabulary, name: &str) -> Option<Self> {
        vocabulary.profile_budgets(name).map(Self::new)
    }

    /// The `default` profile: the declared budgets unless the mod overrides
    /// `default`.
    pub(crate) fn default_profile(vocabulary: &AttackBudgetVocabulary) -> Self {
        Self::profile(vocabulary, DEFAULT_PROFILE_NAME)
            .expect("the `default` profile always resolves")
    }

    pub(crate) fn token_count(&self) -> usize {
        self.tokens.len()
    }

    /// A token outside the table has no budget and never limits a grant.
    pub(crate) fn limit(&self, token: AttackTokenId) -> Option<u32> {
        self.tokens.get(token.0 as usize).map(|budget| budget.limit)
    }

    /// How long a draw on `token` stays counted after its turn ends. A token
    /// outside the table never refreshes.
    pub(crate) fn refresh_ms(&self, token: AttackTokenId) -> f32 {
        self.tokens
            .get(token.0 as usize)
            .map_or(0.0, |budget| budget.refresh_ms)
    }
}

/// The budget table each pool arbitrates under: the default pool's `default`
/// profile, and each running attack pool's own. Every pool shares one token
/// vocabulary, so tables differ only in their values.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AttackBudgets {
    default: BudgetTable,
    pub(super) pools: PoolBudgets,
}

impl AttackBudgets {
    /// The default pool at `default`, with no attack pool running.
    pub(crate) fn new(default: BudgetTable) -> Self {
        Self {
            default,
            pools: PoolBudgets::default(),
        }
    }

    pub(crate) fn from_vocabulary(vocabulary: &AttackBudgetVocabulary) -> Self {
        Self::new(BudgetTable::default_profile(vocabulary))
    }

    /// Re-resolve every table against a newly committed vocabulary. A running
    /// pool keeps its profile name, so a reload that removes the profile
    /// drops it to `default` and one that changes `default` reaches it.
    pub(crate) fn resolve(&mut self, vocabulary: &AttackBudgetVocabulary) {
        self.default = BudgetTable::default_profile(vocabulary);
        self.pools.resolve(vocabulary);
    }

    /// The table `pool` arbitrates under. A pool not running, or running a
    /// profile the mod does not declare, uses `default`.
    pub(crate) fn for_pool(&self, pool: AttackPoolId) -> &BudgetTable {
        match pool {
            AttackPoolId::Default => &self.default,
            AttackPoolId::Pool(entity) => self.pools.table(entity).unwrap_or(&self.default),
        }
    }

    /// Where a departed holder's refresh lands for a turn last counted in
    /// `pool`. A pool still on last tick's run keeps it. A pool that stopped
    /// this tick hands it to the default pool, where its members now count
    /// (P13). A pool that began a new run this tick drops it: a new run
    /// begins open (P30).
    pub(crate) fn departed_refresh_pool(&self, pool: AttackPoolId) -> Option<AttackPoolId> {
        match pool {
            AttackPoolId::Pool(entity) if !self.pools.is_running(entity) => {
                Some(AttackPoolId::Default)
            }
            AttackPoolId::Pool(entity) if self.pools.opened(entity) => None,
            pool => Some(pool),
        }
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
