// `attackTurnFree` against the previous tick's line: the reader's own request
// left out, its own clock placing it in line.

use super::*;
use crate::attack_turns::{AttackBudgets, LineEntry, PreviousTickLine};
use postretro_entities::components::brain::{AttackTurnState, HeldTurn, TurnRequest, WaitClock};

fn holding(target: EntityId, cost: u32) -> AttackTurnState {
    let mut turns = AttackTurnState::default();
    assert!(turns.hold(HeldTurn {
        attack: 0,
        target,
        pool: AttackPoolId::Default,
        draws: draws(&[(MELEE, cost)]),
        until_cooldown: true,
    }));
    turns
}

fn waiting(target: EntityId, cost: u32, since_tick: u64) -> AttackTurnState {
    let mut turns = AttackTurnState::default();
    turns.request = Some(TurnRequest {
        attack: 0,
        draws: draws(&[(MELEE, cost)]),
        marked: false,
        target,
        pool: AttackPoolId::Default,
    });
    turns.wait = Some(WaitClock { since_tick, target });
    turns
}

fn line(budgets: &AttackBudgets, brains: &[(EntityId, &AttackTurnState)]) -> PreviousTickLine {
    let mut line = PreviousTickLine::default();
    line.rebuild(
        budgets,
        brains
            .iter()
            .enumerate()
            .map(|(slot, (entity, turns))| LineEntry {
                entity: *entity,
                slot: slot as u32,
                pool: AttackPoolId::Default,
                turns,
            }),
    );
    line
}

// P23: limit 3; X holds cost 1; B's cost-3 slam has waited longest and
// reserves the token; C, a fresher cost-1 requester, waits behind it. Both
// read whether a cost-1 jab would be granted.
#[test]
fn turn_free_leaves_out_the_readers_own_request_and_applies_its_clock() {
    let ids = targets(5);
    let (target, x, b, c, d) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
    let budgets = AttackBudgets::new(table(&[3]));
    let (x_turns, b_turns, c_turns) = (
        holding(target, 1),
        waiting(target, 3, 10),
        waiting(target, 1, 12),
    );
    let line = line(&budgets, &[(x, &x_turns), (b, &b_turns), (c, &c_turns)]);
    let jab = draws(&[(MELEE, 1)]);
    let table = budgets.for_pool(AttackPoolId::Default);

    assert!(
        line.would_grant(group(target), &jab, table, b, key(10, 1)),
        "B's own slam is left out and B's clock heads the line"
    );
    assert!(
        !line.would_grant(group(target), &jab, table, c, key(12, 2)),
        "C reads false while B's slam reserves the token"
    );
    assert!(
        !line.would_grant(group(target), &jab, table, d, WaitKey::fresh(3)),
        "a fresh reader queues behind B's reservation too"
    );
}

// P7: with room for one and no waiter on the token, several readers may see
// the turn free.
#[test]
fn turn_free_reads_true_for_every_reader_while_room_and_no_waiter() {
    let ids = targets(4);
    let (target, x) = (ids[0], ids[1]);
    let budgets = AttackBudgets::new(table(&[2]));
    let x_turns = holding(target, 1);
    let line = line(&budgets, &[(x, &x_turns)]);
    let table = budgets.for_pool(AttackPoolId::Default);
    let jab = draws(&[(MELEE, 1)]);
    for (slot, reader) in [(1, ids[2]), (2, ids[3])] {
        assert!(line.would_grant(group(target), &jab, table, reader, WaitKey::fresh(slot)));
    }
    assert!(
        !line.would_grant(
            group(target),
            &draws(&[(MELEE, 2)]),
            table,
            ids[2],
            WaitKey::fresh(1)
        ),
        "a cost-2 draw does not fit beside the holder"
    );
    assert!(
        line.would_grant(
            group(ids[2]),
            &draws(&[(MELEE, 2)]),
            table,
            ids[3],
            WaitKey::fresh(2)
        ),
        "another target's allowance is empty"
    );
}
