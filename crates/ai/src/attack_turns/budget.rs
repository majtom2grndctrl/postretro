// Attack token budgets per pool, and the one chokepoint that resolves what an
// attack draws.
// See: context/lib/entity_model.md §7c (Attack budget)

use postretro_entities::components::brain::{
    AttackPoolId, AttackTokenId, ResolvedDraw, ResolvedDraws,
};
use postretro_foundation::AttackParams;

/// The SDK's `defaultToken`: what an attack without `draws` draws.
pub(crate) const DEFAULT_TOKEN: AttackTokenId = AttackTokenId(0);
/// `defineAttackToken`'s default `limit`, which `defaultToken` declares.
pub(crate) const DEFAULT_TOKEN_LIMIT: u32 = 3;

/// One token's budget on one target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TokenBudget {
    /// How much of the token may be drawn on one target at once.
    pub(crate) limit: u32,
    /// How long a spent draw stays counted after its turn ends. Carried as
    /// data; refresh counting lands with the budget ledger.
    pub(crate) refresh_ms: f32,
}

/// One profile's budgets, indexed by token id.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BudgetTable {
    tokens: Vec<TokenBudget>,
}

impl BudgetTable {
    pub(crate) fn new(tokens: Vec<TokenBudget>) -> Self {
        Self { tokens }
    }

    /// The budgets a mod with no declarations runs under: `defaultToken`
    /// alone, at the SDK default.
    pub(crate) fn sdk_default() -> Self {
        Self::new(vec![TokenBudget {
            limit: DEFAULT_TOKEN_LIMIT,
            refresh_ms: 0.0,
        }])
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
        Self::new(BudgetTable::sdk_default())
    }
}

/// THE draw-resolution chokepoint: what one attack draws. Until mods declare
/// tokens and attacks list `draws`, every attack draws one `defaultToken`.
pub(crate) fn resolve_attack_draws(_attack: &AttackParams) -> ResolvedDraws {
    ResolvedDraws::from_slice(&[ResolvedDraw {
        token: DEFAULT_TOKEN,
        cost: 1,
    }])
    .expect("one draw fits the inline draw list")
}
