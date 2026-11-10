// Turn lifetimes and the per-tick arbitration pass: releases, requests, wait
// clocks, grants and the hard gate, between combat slots and apply.
// See: context/lib/entity_model.md §7c (Attack budget)

use std::collections::BTreeMap;

use postretro_entities::EntityId;
use postretro_entities::components::brain::{
    AttackPoolId, AttackTurnState, BrainComponent, HeldTurn, TurnRequest, WaitClock,
};
use postretro_foundation::BehaviorGraphDescriptor;

use super::arbitration::WaitKey;
use super::{AttackTurnRuntime, attack_index, attack_name, attack_pool_of, path_mark};
use crate::brain_programs::{BrainEntityPrograms, BrainPrograms};
use crate::weapon_controller::AiWeaponControllers;
use crate::{EnemyOutcome, PendingAttack};

/// Admit a brain into this tick's AI set. A brain the previous tick did not
/// evaluate left the set (quiescence, a queued despawn, inventory ownership,
/// death): it returns holding nothing, requesting nothing, with no clock.
pub(crate) fn admit(turns: &mut AttackTurnState, tick: u64) {
    if turns.seen_tick.wrapping_add(1) != tick {
        turns.clear();
    }
    turns.seen_tick = tick;
}

/// The tick's turn pass. Every release and request of the tick is known
/// before any grant: releases run first for every enemy, then arbitration
/// grants per pool and target, then the hard gate drops every pending attack
/// whose enemy holds no turn for it. A dropped proposal committed nothing, so
/// its once-per-entry fire stays open for the first granted tick.
///
/// Every turn that ended since the last tick, including a holder's that left
/// the AI set, starts its refresh before any grant, in the pool and on the
/// target it counted against when it ended.
pub(crate) fn arbitrate_attack_turns(
    outcomes: &mut [EnemyOutcome],
    programs: &BrainPrograms,
    runtime: &mut AttackTurnRuntime,
    weapons: &mut AiWeaponControllers,
) {
    let AttackTurnRuntime {
        tick,
        now_ms,
        budgets,
        ledger,
        roster,
        arbiter,
        ..
    } = runtime;
    let now_ms = *now_ms;
    arbiter.begin();

    for (slot, outcome) in outcomes.iter_mut().enumerate() {
        let target = outcome.target.map(|target| target.entity);
        let pool = attack_pool_of(budgets, &outcome.brain);
        // Closing aggro takes the enemy out of the AI set.
        if !outcome.brain.aggro_armed {
            outcome.brain.attack_turns.clear();
        }
        let mark = path_mark(&outcome.brain);
        let programs_entry = programs.get(outcome.id);
        let executing = weapons
            .executing_attack(outcome.id)
            .and_then(|name| attack_index(&outcome.brain.graph, name));
        let BrainComponent {
            attack_turns: turns,
            graph,
            attack_cooldown_remaining_ms: cooldowns,
            ..
        } = &mut outcome.brain;

        let mut cancel_execution = false;
        turns.retain_held(|turn| {
            if Some(turn.target) != target {
                cancel_execution |= executing == Some(turn.attack);
                return false;
            }
            turn.pool = pool;
            // A reload that changed what the attack draws ends its turn; it
            // re-requests under the new draws.
            let draws_unchanged = programs_entry
                .and_then(|entry| entry.attack_draws(turn.attack))
                .is_none_or(|draws| draws == turn.draws);
            draws_unchanged
                && (mark == Some(turn.attack)
                    || executing == Some(turn.attack)
                    || (turn.until_cooldown && cooling_down(graph, cooldowns, turn.attack)))
        });
        if cancel_execution {
            weapons.cancel_actor(outcome.id);
        }
        roster.ended_for(outcome.id, turns, |turn| {
            ledger.record(
                (pool, turn.target),
                &turn.draws,
                budgets.for_pool(pool),
                now_ms,
            );
        });

        let request = target.and_then(|target| {
            next_request(
                turns,
                graph,
                mark,
                outcome.attack.as_ref(),
                programs_entry,
                target,
                pool,
            )
        });
        match (request, turns.wait) {
            (Some((request, _)), Some(wait)) if wait.target == request.target => {}
            (Some((request, _)), _) => {
                turns.wait = Some(WaitClock {
                    since_tick: *tick,
                    target: request.target,
                });
            }
            // A gap in requesting keeps the clock.
            (None, _) => {}
        }
        turns.request = request.map(|(request, _)| request);

        for turn in turns.held() {
            arbiter.hold((pool, turn.target), turn.draws);
        }
        if let (Some((request, grantable)), Some(wait)) = (request, turns.wait) {
            arbiter.request(
                (pool, request.target),
                WaitKey {
                    since_tick: wait.since_tick,
                    slot: slot as u32,
                },
                request.draws,
                grantable,
                slot,
            );
        }
    }

    roster.departed(|turn| {
        ledger.record(
            (turn.pool, turn.target),
            &turn.draws,
            budgets.for_pool(turn.pool),
            now_ms,
        );
    });

    let refreshing = |group: (AttackPoolId, EntityId), token| {
        let refresh_ms = budgets.for_pool(group.0).refresh_ms(token);
        ledger.count(group, token, now_ms, refresh_ms)
    };
    for &slot in arbiter.arbitrate(|pool| budgets.for_pool(pool), refreshing) {
        let outcome = &mut outcomes[slot];
        let turns = &mut outcome.brain.attack_turns;
        let Some(request) = turns.request.take() else {
            continue;
        };
        let held = turns.hold(HeldTurn {
            attack: request.attack,
            target: request.target,
            pool: request.pool,
            draws: request.draws,
            until_cooldown: !request.marked,
        });
        debug_assert!(held, "a request is made only with a free turn slot");
        turns.wait = None;
        if !request.marked {
            outcome.fresh_unmarked_grant = Some(request.attack);
        }
    }

    roster.rebuild(
        outcomes
            .iter()
            .map(|outcome| (outcome.id, &outcome.brain.attack_turns)),
    );

    for outcome in outcomes.iter_mut() {
        if let Some(pending) = outcome.attack.as_ref()
            && !holds_turn_for(&outcome.brain, pending, outcome.target.map(|t| t.entity))
        {
            outcome.attack = None;
        }
    }
}

/// The one request an enemy makes this tick, and whether it can be granted
/// yet. A marked path requests its mark until granted; otherwise the attack
/// the fire step resolved requests, only on a tick it is ready to fire.
fn next_request(
    turns: &AttackTurnState,
    graph: &BehaviorGraphDescriptor,
    mark: Option<u16>,
    pending: Option<&PendingAttack>,
    programs: Option<&BrainEntityPrograms>,
    target: EntityId,
    pool: AttackPoolId,
) -> Option<(TurnRequest, bool)> {
    let (attack, marked) = match mark {
        Some(mark) => (mark, true),
        None => (attack_index(graph, &pending?.attack_name)?, false),
    };
    if turns.holds_attack(attack) || !turns.has_room() {
        return None;
    }
    let draws = programs?.attack_draws(attack)?;
    // No two turns of one enemy share a token. A ready unmarked attack waits
    // to request until that turn ends; a marked activity's request stands
    // from entry and waits in line until it can be granted.
    let shares_token = turns.held().any(|turn| turn.draws.shares_token(&draws));
    if shares_token && !marked {
        return None;
    }
    Some((
        TurnRequest {
            attack,
            draws,
            marked,
            target,
            pool,
        },
        !shares_token,
    ))
}

fn cooling_down(
    graph: &BehaviorGraphDescriptor,
    cooldowns: &BTreeMap<String, f32>,
    attack: u16,
) -> bool {
    attack_name(graph, attack)
        .and_then(|name| cooldowns.get(name))
        .is_some_and(|remaining| *remaining > 0.0)
}

fn holds_turn_for(
    brain: &BrainComponent,
    pending: &PendingAttack,
    target: Option<EntityId>,
) -> bool {
    attack_index(&brain.graph, &pending.attack_name).is_some_and(|attack| {
        brain
            .attack_turns
            .held()
            .any(|turn| turn.attack == attack && Some(turn.target) == target)
    })
}
