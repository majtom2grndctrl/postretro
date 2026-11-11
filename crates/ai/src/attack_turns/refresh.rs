// The refresh ledger: draws still counted after their turn ended, per pool,
// target and token, each measured every tick against its token's current
// `refreshMs` in that pool.
// See: context/lib/entity_model.md §7c (Attack budget)

use postretro_entities::EntityId;
use postretro_entities::components::brain::{AttackPoolId, AttackTokenId, ResolvedDraws};

use super::arbitration::GroupKey;
use super::budget::{AttackBudgets, BudgetTable};
use super::token_remap::TokenRemap;

/// Rounding slack on the refresh clock, far below one tick, so a delay that
/// is a whole number of ticks lapses on that tick.
const CLOCK_TOLERANCE_MS: f64 = 1e-3;

#[derive(Clone, Copy, Debug)]
struct Refreshing {
    group: GroupKey,
    token: AttackTokenId,
    cost: u32,
    ended_ms: f64,
}

impl Refreshing {
    fn key(&self) -> (GroupKey, AttackTokenId) {
        (self.group, self.token)
    }

    fn counts_at(&self, now_ms: f64, refresh_ms: f32) -> bool {
        now_ms - self.ended_ms + CLOCK_TOLERANCE_MS < f64::from(refresh_ms)
    }
}

/// Every refreshing count, kept sorted by pool, target and token so a lookup
/// is a binary search. Reused storage: steady state allocates nothing.
#[derive(Default)]
pub(crate) struct RefreshLedger {
    entries: Vec<Refreshing>,
}

impl RefreshLedger {
    /// A turn drawing `draws` ended at `now_ms`, counted in `group` under
    /// `table`. Each draw on a token that refreshes stays counted.
    pub(crate) fn record(
        &mut self,
        group: GroupKey,
        draws: &ResolvedDraws,
        table: &BudgetTable,
        now_ms: f64,
    ) {
        for draw in draws.as_slice() {
            if draw.cost == 0 || table.refresh_ms(draw.token) <= 0.0 {
                continue;
            }
            let key = (group, draw.token);
            let at = self.entries.partition_point(|entry| entry.key() <= key);
            self.entries.insert(
                at,
                Refreshing {
                    group,
                    token: draw.token,
                    cost: draw.cost,
                    ended_ms: now_ms,
                },
            );
        }
    }

    /// How much of `token` is still refreshing on `group` at `now_ms`, under
    /// that token's current `refresh_ms` there.
    pub(crate) fn count(
        &self,
        group: GroupKey,
        token: AttackTokenId,
        now_ms: f64,
        refresh_ms: f32,
    ) -> u32 {
        let key = (group, token);
        let start = self.entries.partition_point(|entry| entry.key() < key);
        self.entries[start..]
            .iter()
            .take_while(|entry| entry.key() == key)
            .filter(|entry| entry.counts_at(now_ms, refresh_ms))
            .fold(0, |total, entry| total.saturating_add(entry.cost))
    }

    /// Drop each count lapsed by `now_ms` under its pool's current budget,
    /// and every tally on a target that left the world.
    pub(crate) fn prune(
        &mut self,
        now_ms: f64,
        budgets: &AttackBudgets,
        target_live: impl Fn(EntityId) -> bool,
    ) {
        self.entries.retain(|entry| {
            target_live(entry.group.1)
                && entry.counts_at(
                    now_ms,
                    budgets.for_pool(entry.group.0).refresh_ms(entry.token),
                )
        });
    }

    /// A stopped pool's refreshing counts drop, so a restart begins open.
    pub(crate) fn drop_pool(&mut self, pool: AttackPoolId) {
        self.entries.retain(|entry| entry.group.0 != pool);
    }

    /// Re-key counts by a new vocabulary's token ids; a token it no longer
    /// registers drops.
    pub(crate) fn remap_tokens(&mut self, remap: &TokenRemap) {
        self.entries
            .retain_mut(|entry| match remap.token(entry.token) {
                Some(token) => {
                    entry.token = token;
                    true
                }
                None => false,
            });
        self.entries.sort_unstable_by(|a, b| {
            a.key()
                .cmp(&b.key())
                .then(a.ended_ms.total_cmp(&b.ended_ms))
        });
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// The ledger read at one clock, as the turn facts see it.
#[derive(Clone, Copy)]
pub(crate) struct RefreshView<'a> {
    pub(crate) ledger: &'a RefreshLedger,
    pub(crate) at_ms: f64,
}

impl RefreshView<'_> {
    pub(crate) fn count(&self, group: GroupKey, token: AttackTokenId, table: &BudgetTable) -> u32 {
        self.ledger
            .count(group, token, self.at_ms, table.refresh_ms(token))
    }
}
