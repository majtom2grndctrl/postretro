// The arbitration core: one tick's held turns and requests in, grants out, per
// pool and target, by longest wait with per-token head-of-line reservations.
// Pure and allocation-free once its scratch has grown.
// See: context/lib/entity_model.md §7c (Attack budget)

use postretro_entities::EntityId;
use postretro_entities::components::brain::{AttackPoolId, AttackTokenId, ResolvedDraws};

use super::budget::BudgetTable;

/// Every allowance is per pool and per target.
pub(crate) type GroupKey = (AttackPoolId, EntityId);

/// A waiter's place in line: its wait start, then its brain slot index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct WaitKey {
    pub(crate) since_tick: u64,
    pub(crate) slot: u32,
}

impl WaitKey {
    /// The key of a request first made now, behind every existing waiter.
    pub(crate) fn fresh(slot: u32) -> Self {
        Self {
            since_tick: u64::MAX,
            slot,
        }
    }
}

struct Holding {
    group: GroupKey,
    draws: ResolvedDraws,
}

struct Waiter {
    group: GroupKey,
    key: WaitKey,
    draws: ResolvedDraws,
    /// False while the requester holds a turn sharing a token with this
    /// request: it waits in line but cannot be granted yet.
    grantable: bool,
    tag: usize,
}

/// The head-of-line waiter's reservation on one token.
#[derive(Clone, Copy, Default)]
struct Reservation {
    amount: u32,
    taken: bool,
}

/// Reused arbitration scratch. Feed it the tick's holdings and requests, then
/// [`Arbiter::arbitrate`].
#[derive(Default)]
pub(crate) struct Arbiter {
    holdings: Vec<Holding>,
    waiters: Vec<Waiter>,
    granted: Vec<usize>,
    counts: Vec<u32>,
    reservations: Vec<Reservation>,
}

impl Arbiter {
    pub(crate) fn begin(&mut self) {
        self.holdings.clear();
        self.waiters.clear();
        self.granted.clear();
    }

    /// A turn held this tick, after this tick's releases.
    pub(crate) fn hold(&mut self, group: GroupKey, draws: ResolvedDraws) {
        if !draws.is_empty() {
            self.holdings.push(Holding { group, draws });
        }
    }

    /// One requester's request this tick. `tag` names it in the grants.
    pub(crate) fn request(
        &mut self,
        group: GroupKey,
        key: WaitKey,
        draws: ResolvedDraws,
        grantable: bool,
        tag: usize,
    ) {
        self.waiters.push(Waiter {
            group,
            key,
            draws,
            grantable,
            tag,
        });
    }

    /// Grant this tick's requests and return the granted tags.
    ///
    /// Per pool and target, longest wait first (slot index breaks ties). A
    /// request fits a token when held plus refreshing counts, plus the
    /// reservation of an earlier waiter heading that token, plus its cost
    /// stay within the limit, or when nothing at all is counted or reserved
    /// there. It is granted only when every token fits, and then counts at
    /// once against later requests. Refused, it reserves its cost (the whole
    /// token past the limit) on each token no earlier waiter heads.
    pub(crate) fn arbitrate<'b>(
        &mut self,
        budgets: impl Fn(AttackPoolId) -> &'b BudgetTable,
        refreshing: impl Fn(GroupKey, AttackTokenId) -> u32,
    ) -> &[usize] {
        let Self {
            holdings,
            waiters,
            granted,
            counts,
            reservations,
        } = self;
        holdings.sort_unstable_by_key(|holding| holding.group);
        waiters.sort_unstable_by_key(|waiter| (waiter.group, waiter.key));

        let mut next_holding = 0;
        let mut start = 0;
        while start < waiters.len() {
            let group = waiters[start].group;
            let end = start
                + waiters[start..]
                    .iter()
                    .take_while(|waiter| waiter.group == group)
                    .count();
            let table = budgets(group.0);
            counts.clear();
            counts.resize(table.token_count(), 0);
            reservations.clear();
            reservations.resize(table.token_count(), Reservation::default());

            while next_holding < holdings.len() && holdings[next_holding].group < group {
                next_holding += 1;
            }
            while next_holding < holdings.len() && holdings[next_holding].group == group {
                for draw in holdings[next_holding].draws.as_slice() {
                    if let Some(count) = counts.get_mut(draw.token.0 as usize) {
                        *count = count.saturating_add(draw.cost);
                    }
                }
                next_holding += 1;
            }
            for (token, count) in counts.iter_mut().enumerate() {
                *count = count.saturating_add(refreshing(group, AttackTokenId(token as u16)));
            }

            for waiter in &waiters[start..end] {
                let fits = waiter.grantable
                    && waiter.draws.as_slice().iter().all(|draw| {
                        let token = draw.token.0 as usize;
                        let (Some(limit), Some(&count)) =
                            (table.limit(draw.token), counts.get(token))
                        else {
                            return true;
                        };
                        let counted = count.saturating_add(reservations[token].amount);
                        counted == 0 || counted.saturating_add(draw.cost) <= limit
                    });
                if fits {
                    for draw in waiter.draws.as_slice() {
                        if let Some(count) = counts.get_mut(draw.token.0 as usize) {
                            *count = count.saturating_add(draw.cost);
                        }
                    }
                    granted.push(waiter.tag);
                } else {
                    for draw in waiter.draws.as_slice() {
                        let token = draw.token.0 as usize;
                        let (Some(limit), Some(reservation)) =
                            (table.limit(draw.token), reservations.get_mut(token))
                        else {
                            continue;
                        };
                        if !reservation.taken {
                            *reservation = Reservation {
                                amount: draw.cost.min(limit),
                                taken: true,
                            };
                        }
                    }
                }
            }
            start = end;
        }
        granted
    }
}
