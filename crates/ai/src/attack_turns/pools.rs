// Running attack pools as the budget sees them: each pool's profile, its run,
// and the table that profile resolves to.
// See: context/lib/entity_model.md §7c (Attack budget, Attack pools)

use postretro_entities::EntityId;
use postretro_foundation::attack_budget::{AttackBudgetVocabulary, DEFAULT_PROFILE_NAME};

use super::budget::BudgetTable;

/// One running `ai_attack_pool` as the AI reads it each tick.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RunningPool<'a> {
    pub(crate) pool: EntityId,
    /// The pool's `budget_profile`; empty means `default`.
    pub(crate) profile: &'a str,
    /// Counts the pool's starts from stopped. A `start` on a running pool
    /// keeps it, so its refreshing counts stand; a new run begins open.
    pub(crate) run: u64,
}

#[derive(Clone, Debug, PartialEq)]
struct PoolBudget {
    pool: EntityId,
    profile: String,
    run: u64,
    /// `None` while the profile is `default` or one the mod does not declare:
    /// the pool reads the default pool's table, so a reload changing
    /// `default` reaches it.
    table: Option<BudgetTable>,
    seen: bool,
    /// Started from stopped, or began a new run, at this tick's sync.
    opened: bool,
}

/// Every running attack pool's budget, by pool entity.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PoolBudgets {
    pools: Vec<PoolBudget>,
}

impl PoolBudgets {
    /// Whether `pool` was running at this tick's sync.
    pub(super) fn is_running(&self, pool: EntityId) -> bool {
        self.index(pool).is_ok()
    }

    /// Whether `pool` started or began a new run at this tick's sync. A pool
    /// that is not running did neither.
    pub(super) fn opened(&self, pool: EntityId) -> bool {
        self.index(pool).is_ok_and(|index| self.pools[index].opened)
    }

    pub(super) fn table(&self, pool: EntityId) -> Option<&BudgetTable> {
        let index = self.index(pool).ok()?;
        self.pools[index].table.as_ref()
    }

    /// Match the running set. `closed` hears each pool that stopped or began
    /// a new run since the last sync, so its refreshing counts drop. Resolves
    /// a table only for a pool that starts or changes profile.
    pub(super) fn sync<'a>(
        &mut self,
        running: impl Iterator<Item = RunningPool<'a>>,
        vocabulary: &AttackBudgetVocabulary,
        mut closed: impl FnMut(EntityId),
    ) {
        for pool in &mut self.pools {
            pool.seen = false;
            pool.opened = false;
        }
        for running in running {
            let profile = profile_name(running.profile);
            match self.index(running.pool) {
                Ok(index) => {
                    let entry = &mut self.pools[index];
                    if entry.seen {
                        continue;
                    }
                    entry.seen = true;
                    if entry.run != running.run {
                        entry.run = running.run;
                        entry.opened = true;
                        closed(running.pool);
                    }
                    if entry.profile != profile {
                        entry.profile.clear();
                        entry.profile.push_str(profile);
                        entry.table = resolve_table(vocabulary, profile);
                    }
                }
                Err(index) => {
                    closed(running.pool);
                    self.pools.insert(
                        index,
                        PoolBudget {
                            pool: running.pool,
                            profile: profile.to_string(),
                            run: running.run,
                            table: resolve_table(vocabulary, profile),
                            seen: true,
                            opened: true,
                        },
                    );
                }
            }
        }
        self.pools.retain(|pool| {
            if !pool.seen {
                closed(pool.pool);
            }
            pool.seen
        });
    }

    /// Re-resolve every pool's profile against a newly committed vocabulary.
    pub(super) fn resolve(&mut self, vocabulary: &AttackBudgetVocabulary) {
        for pool in &mut self.pools {
            pool.table = resolve_table(vocabulary, &pool.profile);
        }
    }

    pub(super) fn clear(&mut self) {
        self.pools.clear();
    }

    fn index(&self, pool: EntityId) -> Result<usize, usize> {
        self.pools.binary_search_by_key(&pool, |entry| entry.pool)
    }
}

fn profile_name(profile: &str) -> &str {
    if profile.is_empty() {
        DEFAULT_PROFILE_NAME
    } else {
        profile
    }
}

/// A declared profile's own table; `default` and an undeclared name read the
/// default pool's.
fn resolve_table(vocabulary: &AttackBudgetVocabulary, profile: &str) -> Option<BudgetTable> {
    if profile == DEFAULT_PROFILE_NAME {
        return None;
    }
    BudgetTable::profile(vocabulary, profile)
}
