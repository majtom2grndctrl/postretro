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
/// its once-per-entry fire stays open for the first granted tick. The gate
/// never drops an undrawn attack; past its capacity it fires holding no turn.
///
/// Every turn that ended since the last tick starts its refresh before any
/// grant, on its target, in its holder's current pool: a turn counts in that
/// pool while it holds (P13). A holder that left the AI set records in the
/// turn's last pool. If that pool stopped this tick, the refresh lands in the
/// default pool, where its members now count (P13); if it began a new run
/// this tick, the refresh is dropped, because a new run begins open (P30).
///
/// Within arbitration, executions stop in one place, before any grant, so
/// only executions begun on earlier ticks are seen: a grant starts its
/// execution in apply, after this pass. Outside it, apply cancels one with no
/// aim or on a weapon switch, and a weapon sync refresh cancels one too.
/// Three rules cancel one here. An ended turn whose attack is still executing
/// cancels it, whatever ended the turn, undrawn turns included. After a
/// reseat the ended turn's index may name the graph it left, so a reseat
/// cancels any execution in flight outright: it ends every turn, so no turn's
/// burst outlives it. And an executing attack, resolved by name against the
/// current graph, is cancelled when the name no longer resolves, or when it
/// is not known to be undrawn and its enemy holds no turn for it. The last
/// rule covers what no ended turn reports: a burst a held enemy kept, on its
/// drop with its turns cleared.
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
        // The execution in flight, by name, resolved against the current
        // graph: `None` inside `Some` when the name no longer resolves.
        let executing = weapons
            .executing_attack(outcome.id)
            .map(|name| attack_index(&outcome.brain.graph, name));
        let graph_reseated = outcome.graph_reseated;
        let BrainComponent {
            attack_turns: turns,
            graph,
            attack_cooldown_remaining_ms: cooldowns,
            ..
        } = &mut outcome.brain;

        let executing_attack = executing.flatten();
        turns.retain_held(|turn| {
            // Switching to another target ends a turn. No target this tick
            // (a path that neither engages nor is due to acquire) is no
            // switch: it ends only a turn its mark alone held, since a mark
            // must sit under an engaged path. A turn can survive such a
            // tick, but its burst still needs aim to continue: apply passes
            // none, so the weapon controller cancels the execution.
            if target.is_some_and(|target| target != turn.target) {
                return false;
            }
            turn.pool = pool;
            // A reload that changed what the attack draws ends its turn; it
            // re-requests under the new draws.
            let draws_unchanged = programs_entry
                .and_then(|entry| entry.attack_draws(turn.attack))
                .is_none_or(|draws| draws.same_draws(&turn.draws));
            draws_unchanged
                && ((target.is_some() && mark == Some(turn.attack))
                    || executing_attack == Some(turn.attack)
                    || (turn.until_cooldown && cooling_down(graph, cooldowns, turn.attack)))
        });
        // Every turn an evaluated holder lost since last tick is reported
        // here, whatever ended it (a target switch, a draws change, a reseat,
        // closing aggro), so this is where its execution stops. A reported
        // turn names an attack of the graph it was granted under; after a
        // reseat that may not be the current graph, so a reseat cancels
        // whatever executes. An invalid-path reseat keeps the graph, so the
        // name check below would spare an undrawn burst.
        let mut cancel_execution = graph_reseated && executing.is_some();
        roster.ended_for(outcome.id, turns, |turn| {
            cancel_execution |= executing_attack == Some(turn.attack);
            ledger.record(
                (pool, turn.target),
                &turn.draws,
                budgets.for_pool(pool),
                now_ms,
            );
        });
        // No drawn attack executes without a turn, whether or not a reported
        // turn ended: an undrawn one may, past its capacity.
        cancel_execution |= executing.is_some_and(|attack| {
            attack.is_none_or(|attack| {
                !turns.holds_attack(attack) && !attack_draws_nothing(programs_entry, attack)
            })
        });
        if cancel_execution {
            weapons.cancel_actor(outcome.id);
        }

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

    roster.departed(|_holder, turn| {
        // A departed holder's execution is not cancelled here. A held (suspended)
        // enemy keeps its private execution by the held-ownership contract
        // (`entity_model.md`), and a despawned one loses its controller with it.
        // A held enemy's kept drawn burst stops on drop, by the name check
        // above, and does not resume even if a turn is granted that same
        // tick: the cancel runs before any grant. A kept undrawn burst may
        // resume, since an undrawn attack fires holding no turn past its
        // capacity anyway.
        if let Some(pool) = budgets.departed_refresh_pool(turn.pool) {
            ledger.record(
                (pool, turn.target),
                &turn.draws,
                budgets.for_pool(pool),
                now_ms,
            );
        }
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
            && !draws_nothing(programs.get(outcome.id), &outcome.brain.graph, pending)
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
    if turns.holds_attack(attack) {
        return None;
    }
    let draws = programs?.attack_draws(attack)?;
    // With no room, the request could never be granted. An undrawn attack
    // fires anyway; a drawn one waits for a held turn to end.
    if !turns.has_room_for(&draws) {
        return None;
    }
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

/// An attack drawing nothing is always granted: past its turn capacity it
/// fires holding no turn.
fn draws_nothing(
    programs: Option<&BrainEntityPrograms>,
    graph: &BehaviorGraphDescriptor,
    pending: &PendingAttack,
) -> bool {
    attack_index(graph, &pending.attack_name)
        .is_some_and(|attack| attack_draws_nothing(programs, attack))
}

/// Whether the attack at `attack` resolves to no draws. Unresolved draws
/// count as drawn: such an attack is never granted, so it needs a turn.
fn attack_draws_nothing(programs: Option<&BrainEntityPrograms>, attack: u16) -> bool {
    programs
        .and_then(|programs| programs.attack_draws(attack))
        .is_some_and(|draws| draws.is_empty())
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
