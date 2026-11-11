// Engine-level attack-turn proofs on synthetic token tables: arbitration
// order, cost fit, head-of-line reservations, the turn-free fact, and the
// refresh ledger and pool profiles at the runtime seam. Through-the-tick
// proofs live in `tick/`, mounted beside the AI test fixtures.
// See: context/lib/entity_model.md §7c (Attack budget)

mod arbitration;
mod facts;
mod refresh;

use postretro_entities::components::brain::{
    AttackPoolId, AttackTokenId, ResolvedDraw, ResolvedDraws,
};
use postretro_entities::{EntityId, EntityRegistry, Transform};

use super::arbitration::{Arbiter, GroupKey, WaitKey};
use super::{BudgetTable, TokenBudget};

const MELEE: u16 = 0;
const HEAVY: u16 = 1;
const RANGED: u16 = 2;

/// A table of `limit`s by token id, none refreshing.
fn table(limits: &[u32]) -> BudgetTable {
    BudgetTable::new(
        limits
            .iter()
            .map(|&limit| TokenBudget {
                limit,
                refresh_ms: 0.0,
            })
            .collect(),
    )
}

fn draws(list: &[(u16, u32)]) -> ResolvedDraws {
    let list: Vec<ResolvedDraw> = list
        .iter()
        .map(|&(token, cost)| ResolvedDraw {
            token: AttackTokenId(token),
            cost,
        })
        .collect();
    ResolvedDraws::from_slice(&list).expect("test draw lists fit")
}

/// Live targets to key allowances by.
fn targets(count: usize) -> Vec<EntityId> {
    let mut registry = EntityRegistry::new();
    (0..count)
        .map(|_| registry.spawn(Transform::default()))
        .collect()
}

/// Nothing refreshing anywhere.
fn no_refresh(_group: GroupKey, _token: AttackTokenId) -> u32 {
    0
}

fn group(target: EntityId) -> GroupKey {
    (AttackPoolId::Default, target)
}

fn key(since_tick: u64, slot: u32) -> WaitKey {
    WaitKey { since_tick, slot }
}

/// One arbitration on `table` for a single pool; returns granted tags sorted.
fn grants(arbiter: &mut Arbiter, table: &BudgetTable) -> Vec<usize> {
    let mut granted = arbiter.arbitrate(|_| table, no_refresh).to_vec();
    granted.sort_unstable();
    granted
}

// Hot path: once its scratch has grown, a steady-state turn pass (refresh
// pruning, the previous-tick line, turn ends found on the roster and recorded
// in the refresh ledger, arbitration over a wave on three tokens with
// refreshing counts, the turn facts, and the roster rebuild) performs no heap
// allocation. Shapes: a wave spread over several targets, and the 64- and
// 128-enemy waves on one target that the arbitration stress row times
// (`tick/arbitration_stress.rs`).
#[test]
fn steady_state_turn_passes_allocate_nothing() {
    // Every fourth enemy still holds, all of them on the first target.
    steady_turn_pass_allocates_nothing(128, 4, |slot| slot % 4 == 0);
    // One ranged-and-heavy holder leaves melee room, so a melee waiter is
    // granted on the shared target.
    steady_turn_pass_allocates_nothing(64, 1, |slot| slot == 2);
    steady_turn_pass_allocates_nothing(128, 1, |slot| slot == 2);
}

/// One warm turn pass, then one probed steady pass over the same inputs:
/// `enemy_count` enemies spread over `target_count` targets, each holding a
/// turn last tick. This tick those `still_holds` picks keep theirs, the rest
/// wait, and the last eight left the AI set.
fn steady_turn_pass_allocates_nothing(
    enemy_count: usize,
    target_count: usize,
    still_holds: impl Fn(usize) -> bool,
) {
    use super::refresh::{RefreshLedger, RefreshView};
    use super::roster::TurnRoster;
    use super::{AttackBudgets, LineEntry, PreviousTickLine};
    use crate::alloc_probe::AllocSnapshot;
    use postretro_entities::components::brain::{
        AttackTurnState, HeldTurn, TurnRequest, WaitClock,
    };

    let table = BudgetTable::new(vec![
        TokenBudget {
            limit: 3,
            refresh_ms: 0.0,
        },
        TokenBudget {
            limit: 1,
            refresh_ms: 4000.0,
        },
        TokenBudget {
            limit: 2,
            refresh_ms: 0.0,
        },
    ]);
    let budgets = AttackBudgets::new(table.clone());
    let ids = targets(target_count + enemy_count);
    let (targets, enemies) = ids.split_at(target_count);
    let kinds = [
        draws(&[(MELEE, 1)]),
        draws(&[(MELEE, 2), (HEAVY, 1)]),
        draws(&[(RANGED, 1), (HEAVY, 1)]),
    ];
    let held = |slot: usize| HeldTurn {
        attack: 0,
        target: targets[slot % targets.len()],
        pool: AttackPoolId::Default,
        draws: kinds[slot % kinds.len()],
        until_cooldown: true,
    };
    let before: Vec<AttackTurnState> = (0..enemies.len())
        .map(|slot| {
            let mut turns = AttackTurnState::default();
            turns.hold(held(slot));
            turns
        })
        .collect();
    let present = enemies.len() - 8;
    let brains: Vec<AttackTurnState> = (0..present)
        .map(|slot| {
            let mut turns = AttackTurnState::default();
            if still_holds(slot) {
                turns.hold(held(slot));
            } else {
                let turn = held(slot);
                turns.request = Some(TurnRequest {
                    attack: 0,
                    draws: turn.draws,
                    marked: false,
                    target: turn.target,
                    pool: AttackPoolId::Default,
                });
                turns.wait = Some(WaitClock {
                    since_tick: slot as u64 % 7,
                    target: turn.target,
                });
            }
            turns
        })
        .collect();

    let mut line = PreviousTickLine::default();
    let mut arbiter = Arbiter::default();
    let mut ledger = RefreshLedger::default();
    let mut roster = TurnRoster::default();
    let mut pass = |now_ms: f64| {
        roster.rebuild(enemies.iter().copied().zip(&before));
        ledger.prune(now_ms - 16.0, &budgets, |_| true);
        line.rebuild(
            &budgets,
            enemies
                .iter()
                .zip(&brains)
                .enumerate()
                .map(|(slot, (entity, turns))| LineEntry {
                    entity: *entity,
                    slot: slot as u32,
                    pool: AttackPoolId::Default,
                    turns,
                }),
        );
        let view = RefreshView {
            ledger: &ledger,
            at_ms: now_ms - 16.0,
        };
        let free = enemies[..present]
            .iter()
            .enumerate()
            .filter(|&(slot, entity)| {
                let group = group(targets[slot % targets.len()]);
                line.would_grant(
                    group,
                    &kinds[1],
                    &table,
                    *entity,
                    WaitKey::fresh(slot as u32),
                    |token| view.count(group, token, &table),
                )
            })
            .count();
        arbiter.begin();
        for (slot, (entity, turns)) in enemies.iter().zip(&brains).enumerate() {
            roster.ended_for(*entity, turns, |turn| {
                ledger.record(group(turn.target), &turn.draws, &table, now_ms);
            });
            for turn in turns.held() {
                arbiter.hold(group(turn.target), turn.draws);
            }
            if let (Some(request), Some(wait)) = (turns.request, turns.wait) {
                arbiter.request(
                    group(request.target),
                    key(wait.since_tick, slot as u32),
                    request.draws,
                    true,
                    slot,
                );
            }
        }
        roster.departed(|_, turn| {
            ledger.record(group(turn.target), &turn.draws, &table, now_ms);
        });
        let granted = arbiter
            .arbitrate(
                |_| &table,
                |group, token| ledger.count(group, token, now_ms, table.refresh_ms(token)),
            )
            .len();
        roster.rebuild(enemies.iter().copied().zip(&brains));
        (granted, free, ledger.len())
    };
    let warm = pass(0.0);

    let snapshot = AllocSnapshot::arm();
    let steady = pass(5000.0);
    let allocs = snapshot.allocs_since();
    let shape = format!("{enemy_count} enemies on {target_count} targets");
    assert_eq!(steady, warm, "{shape}: the same inputs grant the same set");
    assert!(warm.0 > 0, "{shape}: fixture: some requests are granted");
    assert!(warm.2 > 0, "{shape}: fixture: ended turns are refreshing");
    assert_eq!(
        allocs, 0,
        "{shape}: a steady-state turn pass must not allocate"
    );
}
