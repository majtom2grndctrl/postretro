// Arbitration on synthetic token tables: wait order, cost fit, per-token
// head-of-line reservations, and fit against this tick's earlier grants.

use super::*;

const T0: u64 = 0;

// ---------------------------------------------------------------------------
// A multi-tick line: synthetic enemies on one target, each holding its turn
// for a fixed number of ticks and requesting through a window of ticks.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Agent {
    draws: ResolvedDraws,
    hold_ticks: u64,
    requests_from: u64,
}

fn agent(list: &[(u16, u32)], hold_ticks: u64, requests_from: u64) -> Agent {
    Agent {
        draws: draws(list),
        hold_ticks,
        requests_from,
    }
}

struct Line {
    target: EntityId,
    agents: Vec<Agent>,
    held_until: Vec<Option<u64>>,
    wait: Vec<Option<u64>>,
    grants: Vec<Vec<u64>>,
    arbiter: Arbiter,
}

impl Line {
    fn new(agents: Vec<Agent>) -> Self {
        let count = agents.len();
        Self {
            target: targets(1)[0],
            agents,
            held_until: vec![None; count],
            wait: vec![None; count],
            grants: vec![Vec::new(); count],
            arbiter: Arbiter::default(),
        }
    }

    /// Agent `index` starts the run holding a turn until `until`.
    fn holding(mut self, index: usize, until: u64) -> Self {
        self.held_until[index] = Some(until);
        self
    }

    fn tick(&mut self, table: &BudgetTable, tick: u64) -> Vec<usize> {
        for held in &mut self.held_until {
            if held.is_some_and(|until| until <= tick) {
                *held = None;
            }
        }
        self.arbiter.begin();
        for (index, agent) in self.agents.iter().enumerate() {
            if self.held_until[index].is_some() {
                self.arbiter.hold(group(self.target), agent.draws);
            } else if tick >= agent.requests_from {
                let since = *self.wait[index].get_or_insert(tick);
                self.arbiter.request(
                    group(self.target),
                    key(since, index as u32),
                    agent.draws,
                    true,
                    index,
                );
            }
        }
        let granted = grants(&mut self.arbiter, table);
        for &index in &granted {
            self.held_until[index] = Some(tick + self.agents[index].hold_ticks);
            self.wait[index] = None;
            self.grants[index].push(tick);
        }
        granted
    }

    fn held_cost(&self, token: u16) -> u32 {
        self.agents
            .iter()
            .zip(&self.held_until)
            .filter(|(_, held)| held.is_some())
            .flat_map(|(agent, _)| agent.draws.as_slice().iter().copied())
            .filter(|draw| draw.token.0 == token)
            .map(|draw| draw.cost)
            .sum()
    }

    fn first_grant(&self, index: usize) -> Option<u64> {
        self.grants[index].first().copied()
    }
}

// ---------------------------------------------------------------------------
// Single-tick arbitration
// ---------------------------------------------------------------------------

// P1: the longest waiter wins even when a fresher requester has the earlier
// slot index.
#[test]
fn longest_waiter_wins_over_an_earlier_slot() {
    let table = table(&[1]);
    let target = targets(1)[0];
    let mut arbiter = Arbiter::default();
    arbiter.begin();
    arbiter.request(group(target), key(5, 0), draws(&[(MELEE, 1)]), true, 0);
    arbiter.request(group(target), key(2, 9), draws(&[(MELEE, 1)]), true, 1);
    assert_eq!(grants(&mut arbiter, &table), vec![1]);
}

// B3: three requesters on one tick with one turn free: exactly one, and a
// replay in any feed order grants the same one.
#[test]
fn one_free_turn_grants_exactly_one_deterministically() {
    let table = table(&[3]);
    let target = targets(1)[0];
    let run = |order: &[(u32, usize)]| {
        let mut arbiter = Arbiter::default();
        arbiter.begin();
        arbiter.hold(group(target), draws(&[(MELEE, 1)]));
        arbiter.hold(group(target), draws(&[(MELEE, 1)]));
        for &(slot, tag) in order {
            arbiter.request(group(target), key(7, slot), draws(&[(MELEE, 1)]), true, tag);
        }
        grants(&mut arbiter, &table)
    };
    let first = run(&[(5, 0), (3, 1), (7, 2)]);
    assert_eq!(first, vec![1], "equal waits go to the earlier slot index");
    assert_eq!(
        run(&[(7, 2), (5, 0), (3, 1)]),
        first,
        "a replay grants the same one"
    );
}

// B4: under limit 2, cost 2 is granted alone and refused beside a cost-1
// holder; cost 3 is granted when nothing is counted.
#[test]
fn cost_fits_against_limit_and_oversized_draw_admits_when_empty() {
    let table = table(&[2]);
    let target = targets(1)[0];
    let mut arbiter = Arbiter::default();

    arbiter.begin();
    arbiter.request(group(target), key(T0, 0), draws(&[(MELEE, 2)]), true, 0);
    assert_eq!(
        grants(&mut arbiter, &table),
        vec![0],
        "cost 2 alone fits limit 2"
    );

    arbiter.begin();
    arbiter.hold(group(target), draws(&[(MELEE, 1)]));
    arbiter.request(group(target), key(T0, 0), draws(&[(MELEE, 2)]), true, 0);
    assert!(
        grants(&mut arbiter, &table).is_empty(),
        "cost 2 beside a cost-1 holder overflows limit 2"
    );

    arbiter.begin();
    arbiter.request(group(target), key(T0, 0), draws(&[(MELEE, 3)]), true, 0);
    assert_eq!(
        grants(&mut arbiter, &table),
        vec![0],
        "a token with nothing counted admits any cost"
    );
}

// B4 / P8: an attack drawing nothing is always granted, never counts, and
// never blocks an oversized waiter.
#[test]
fn undrawn_attack_never_counts_or_blocks() {
    let table = table(&[2]);
    let target = targets(1)[0];
    let mut arbiter = Arbiter::default();

    // A cost-3 request waits beside a cost-1 holder; the undrawn one lands.
    arbiter.begin();
    arbiter.hold(group(target), draws(&[(MELEE, 1)]));
    arbiter.request(group(target), key(1, 0), draws(&[(MELEE, 3)]), true, 0);
    arbiter.request(group(target), key(2, 1), ResolvedDraws::NONE, true, 1);
    assert_eq!(grants(&mut arbiter, &table), vec![1]);

    // The undrawn turn is held; the cost-1 holder released: cost 3 lands.
    arbiter.begin();
    arbiter.hold(group(target), ResolvedDraws::NONE);
    arbiter.request(group(target), key(1, 0), draws(&[(MELEE, 3)]), true, 0);
    assert_eq!(
        grants(&mut arbiter, &table),
        vec![0],
        "an undrawn turn counts nothing against the token"
    );
}

// B10 / P31: fit and reservations are judged against counts that include
// this tick's earlier grants and reservations; a replay grants the same set.
#[test]
fn grants_within_a_tick_see_earlier_grants_and_reservations() {
    let table = table(&[3, 1, 2]);
    let target = targets(1)[0];
    let run = |reverse: bool| {
        let mut requests = vec![
            (key(1, 0), draws(&[(MELEE, 2)]), 0),
            (key(2, 1), draws(&[(MELEE, 2)]), 1),
            (key(3, 2), draws(&[(MELEE, 1)]), 2),
            (key(4, 3), draws(&[(RANGED, 1)]), 3),
            (key(5, 4), draws(&[(RANGED, 1)]), 4),
            (key(6, 5), draws(&[(RANGED, 1)]), 5),
        ];
        if reverse {
            requests.reverse();
        }
        let mut arbiter = Arbiter::default();
        arbiter.begin();
        for (key, draws, tag) in requests {
            arbiter.request(group(target), key, draws, true, tag);
        }
        grants(&mut arbiter, &table)
    };
    let granted = run(false);
    // 0 takes two melee; 1 cannot fit beside it and reserves two; 2 would fit
    // the raw count (2 + 1) but not beside 1's reservation. Ranged admits two.
    assert_eq!(granted, vec![0, 3, 4]);
    assert_eq!(run(true), granted, "a replay grants the same set");
}

// Every target has its own allowance, and one pool never blocks another.
#[test]
fn allowances_are_per_pool_and_per_target() {
    let table = table(&[1]);
    let ids = targets(3);
    let (a, b, pool_entity) = (ids[0], ids[1], ids[2]);
    let other_pool = (AttackPoolId::Pool(pool_entity), a);
    let mut arbiter = Arbiter::default();
    arbiter.begin();
    arbiter.hold(group(a), draws(&[(MELEE, 1)]));
    // A head-of-line waiter on A in the default pool, refused and reserving.
    arbiter.request(group(a), key(1, 0), draws(&[(MELEE, 1)]), true, 0);
    arbiter.request(group(b), key(9, 1), draws(&[(MELEE, 1)]), true, 1);
    arbiter.request(other_pool, key(9, 2), draws(&[(MELEE, 1)]), true, 2);
    assert_eq!(grants(&mut arbiter, &table), vec![1, 2]);
}

// P20: a request its own held turn blocks waits in line and heads its token
// until it can be granted.
#[test]
fn a_waiter_its_own_turn_blocks_still_heads_its_token() {
    let table = table(&[2]);
    let target = targets(1)[0];
    let mut arbiter = Arbiter::default();
    arbiter.begin();
    arbiter.hold(group(target), draws(&[(MELEE, 1)]));
    arbiter.request(group(target), key(1, 0), draws(&[(MELEE, 1)]), false, 0);
    arbiter.request(group(target), key(2, 1), draws(&[(MELEE, 1)]), true, 1);
    assert!(
        grants(&mut arbiter, &table).is_empty(),
        "the blocked head is not granted and its reservation fills the token"
    );
}

// ---------------------------------------------------------------------------
// Multi-tick lines
// ---------------------------------------------------------------------------

// B5 / P12: a cost-3 waiter under limit 2, among cost-1 enemies that would
// keep a turn held, reserves the whole token: nobody is granted past it, and
// it lands once nothing is counted.
#[test]
fn oversized_head_of_line_waiter_is_granted_within_bounded_wait() {
    let table = table(&[2]);
    let mut line = Line::new(vec![
        agent(&[(MELEE, 1)], 4, 99),
        agent(&[(MELEE, 1)], 3, 0),
        agent(&[(MELEE, 1)], 3, 2),
        agent(&[(MELEE, 1)], 3, 2),
        agent(&[(MELEE, 3)], 2, 1),
    ])
    .holding(0, 4);
    let oversized = 4;
    for tick in 0..20 {
        line.tick(&table, tick);
    }
    let granted_at = line
        .first_grant(oversized)
        .expect("the oversized waiter is granted");
    assert_eq!(
        granted_at, 4,
        "granted the tick the last cost-1 holder ends"
    );
    for cost_one in 1..oversized {
        assert!(
            !line.grants[cost_one]
                .iter()
                .any(|&tick| (1..granted_at).contains(&tick)),
            "no cost-1 enemy is granted past the waiting cost-3 draw"
        );
    }
    assert_eq!(
        line.first_grant(1),
        Some(0),
        "fixture: one turn rotates in first"
    );
}

// B6 / P14: `melee` at 4, `heavy` at 1. A waiting slam reserves one `melee`
// and the `heavy`; jabs fill `melee` beside it; it is granted the tick the
// holding slam releases, ahead of a later slam.
#[test]
fn waiting_slam_reserves_melee_and_heavy_while_jabs_pass() {
    let table = table(&[4, 1]);
    let slam = [(MELEE, 1), (HEAVY, 1)];
    let mut line = Line::new(vec![
        agent(&slam, 10, 99),
        agent(&slam, 3, 1),
        agent(&[(MELEE, 1)], 20, 2),
        agent(&[(MELEE, 1)], 20, 2),
        agent(&[(MELEE, 1)], 20, 2),
        agent(&slam, 3, 3),
    ])
    .holding(0, 6);
    for tick in 0..7 {
        line.tick(&table, tick);
        if (2..6).contains(&tick) {
            assert_eq!(
                line.held_cost(MELEE),
                3,
                "jabs fill `melee` to 4 with the slam's reservation, tick {tick}"
            );
        }
    }
    assert_eq!(line.first_grant(2), Some(2));
    assert_eq!(line.first_grant(3), Some(2));
    assert_eq!(
        line.first_grant(4),
        Some(6),
        "a third jab finds no room beside the reservation while the slam waits"
    );
    assert_eq!(
        line.first_grant(1),
        Some(6),
        "the waiting slam lands as the holder releases"
    );
    assert_eq!(line.first_grant(5), None, "a later slam waits behind it");
}

// B7: a slam and a rocket from two archetypes, both drawing `heavy` at
// limit 1, never hold two turns on one target at once; each gets a turn.
#[test]
fn heavy_draws_from_two_archetypes_never_overlap() {
    let table = table(&[3, 1, 2]);
    let slam = [(MELEE, 1), (HEAVY, 1)];
    let rocket = [(RANGED, 1), (HEAVY, 1)];
    let mut line = Line::new(vec![
        agent(&slam, 3, 0),
        agent(&rocket, 3, 0),
        agent(&slam, 3, 0),
        agent(&rocket, 3, 0),
    ]);
    for tick in 0..40 {
        line.tick(&table, tick);
        assert!(line.held_cost(HEAVY) <= 1, "two heavy turns at tick {tick}");
    }
    for index in 0..4 {
        assert!(
            line.grants[index].len() >= 2,
            "every heavy attacker keeps getting turns"
        );
    }
}

// B8 / P18 / P32: `melee` at 3, `heavy` at 1, one slam holding. The second
// slam heads both tokens and reserves one of each; a cost-3 `melee` draw
// behind it reserves nothing yet, so a cost-1 request is granted beside the
// slam's reservation. Once the slam is granted, the cost-3 draw heads `melee`
// and reserves the whole token.
#[test]
fn head_of_line_reserves_per_token_in_wait_order() {
    let table = table(&[3, 1]);
    let slam = [(MELEE, 1), (HEAVY, 1)];
    let mut line = Line::new(vec![
        agent(&slam, 20, 99),
        agent(&slam, 6, 1),
        agent(&[(MELEE, 3)], 2, 2),
        agent(&[(MELEE, 1)], 8, 3),
        agent(&[(MELEE, 1)], 2, 11),
    ])
    .holding(0, 10);
    for tick in 0..30 {
        line.tick(&table, tick);
    }
    assert_eq!(
        line.first_grant(3),
        Some(3),
        "the cost-1 request fits beside the slam's reservation"
    );
    assert_eq!(
        line.first_grant(1),
        Some(10),
        "the slam lands as the holder ends"
    );
    let cost_three = line.first_grant(2).expect("the cost-3 draw is granted");
    assert_eq!(cost_three, 16, "granted once `melee` empties");
    assert_eq!(
        line.first_grant(4),
        Some(18),
        "a later cost-1 request never passes the cost-3 head"
    );
}

// B9 / P27: a slam (`melee` ×2, `heavy`) waits among jabs and pellets that
// keep turns cycling, while a rocket holds `heavy`. It reserves two `melee`
// and the `heavy` from its first request; jabs use only the remaining
// `melee`, no rocket takes `heavy`, and it is granted the tick the holding
// rocket's `heavy` frees.
#[test]
fn multi_token_waiter_is_granted_among_cycling_single_token_traffic() {
    let table = table(&[3, 1, 2]);
    let rocket = [(RANGED, 1), (HEAVY, 1)];
    let mut line = Line::new(vec![
        agent(&rocket, 4, 99),
        agent(&[(MELEE, 1)], 2, 99),
        agent(&[(MELEE, 2), (HEAVY, 1)], 3, 1),
        agent(&[(MELEE, 1)], 2, 2),
        agent(&[(MELEE, 1)], 2, 2),
        agent(&[(MELEE, 1)], 2, 2),
        agent(&[(RANGED, 1)], 2, 0),
        agent(&[(RANGED, 1)], 2, 0),
        agent(&rocket, 4, 2),
    ])
    .holding(0, 8)
    .holding(1, 6);
    let slam = 2;
    for tick in 0..8 {
        line.tick(&table, tick);
        if (1..8).contains(&tick) {
            assert!(
                line.held_cost(MELEE) <= 1,
                "jabs fit only beside the slam's two-melee reservation, tick {tick}"
            );
        }
    }
    assert!(
        line.grants[3..6].iter().any(|grants| !grants.is_empty()),
        "jabs keep cycling"
    );
    assert!(
        line.grants[6..8].iter().all(|grants| !grants.is_empty()),
        "pellets keep cycling"
    );
    line.tick(&table, 8);
    assert_eq!(
        line.first_grant(slam),
        Some(8),
        "granted as the rocket's `heavy` frees"
    );
    assert_eq!(
        line.first_grant(8),
        None,
        "no later rocket takes `heavy` past it"
    );
}
