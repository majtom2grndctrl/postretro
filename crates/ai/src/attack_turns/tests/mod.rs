// Engine-level attack-turn proofs on synthetic token tables: arbitration
// order, cost fit, head-of-line reservations, and the turn-free fact.
// Through-the-tick proofs live in `tick_*.rs`, beside the AI test fixtures.
// See: context/lib/entity_model.md §7c (Attack budget)

mod arbitration;
mod facts;

use postretro_entities::components::brain::{
    AttackPoolId, AttackTokenId, ResolvedDraw, ResolvedDraws,
};
use postretro_entities::{EntityId, EntityRegistry, Transform};

use super::arbitration::{Arbiter, GroupKey, WaitKey};
use super::{BudgetTable, TokenBudget, refreshing_count};

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

fn group(target: EntityId) -> GroupKey {
    (AttackPoolId::Default, target)
}

fn key(since_tick: u64, slot: u32) -> WaitKey {
    WaitKey { since_tick, slot }
}

/// One arbitration on `table` for a single pool; returns granted tags sorted.
fn grants(arbiter: &mut Arbiter, table: &BudgetTable) -> Vec<usize> {
    let mut granted = arbiter.arbitrate(|_| table, refreshing_count).to_vec();
    granted.sort_unstable();
    granted
}

// Hot path: once its scratch has grown, a steady-state turn pass (the
// previous-tick line, then arbitration over a wave on three tokens and
// several targets) performs no heap allocation.
#[test]
fn steady_state_turn_passes_allocate_nothing() {
    use super::{AttackBudgets, LineEntry, PreviousTickLine};
    use crate::alloc_probe::AllocSnapshot;
    use postretro_entities::components::brain::{
        AttackTurnState, HeldTurn, TurnRequest, WaitClock,
    };

    let table = table(&[3, 1, 2]);
    let budgets = AttackBudgets::new(table.clone());
    let ids = targets(132);
    let (targets, enemies) = ids.split_at(4);
    let kinds = [
        draws(&[(MELEE, 1)]),
        draws(&[(MELEE, 2), (HEAVY, 1)]),
        draws(&[(RANGED, 1), (HEAVY, 1)]),
    ];
    let brains: Vec<AttackTurnState> = enemies
        .iter()
        .enumerate()
        .map(|(slot, _)| {
            let target = targets[slot % targets.len()];
            let draws = kinds[slot % kinds.len()];
            let mut turns = AttackTurnState::default();
            if slot % 4 == 0 {
                turns.hold(HeldTurn {
                    attack: 0,
                    target,
                    pool: AttackPoolId::Default,
                    draws,
                    until_cooldown: true,
                });
            } else {
                turns.request = Some(TurnRequest {
                    attack: 0,
                    draws,
                    marked: false,
                    target,
                    pool: AttackPoolId::Default,
                });
                turns.wait = Some(WaitClock {
                    since_tick: slot as u64 % 7,
                    target,
                });
            }
            turns
        })
        .collect();

    let mut line = PreviousTickLine::default();
    let mut arbiter = Arbiter::default();
    let pass = |line: &mut PreviousTickLine, arbiter: &mut Arbiter| {
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
        arbiter.begin();
        for (slot, turns) in brains.iter().enumerate() {
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
        arbiter.arbitrate(|_| &table, refreshing_count).len()
    };
    let warm = pass(&mut line, &mut arbiter);

    let snapshot = AllocSnapshot::arm();
    let granted = pass(&mut line, &mut arbiter);
    let allocs = snapshot.allocs_since();
    assert_eq!(granted, warm, "the same inputs grant the same set");
    assert!(granted > 0, "fixture: some requests are granted");
    assert_eq!(allocs, 0, "a steady-state turn pass must not allocate");
}
