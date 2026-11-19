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
use super::{
    AttackTurnRuntime, SkippedHolder, attack_index, attack_name, attack_pool_of, path_mark,
};
use crate::brain_programs::{BrainEntityPrograms, BrainPrograms};
use crate::weapon_controller::AiWeaponControllers;
use crate::{EnemyOutcome, PendingAttack};

/// Admit a brain into this tick's run, and return the ticks since its last
/// run. A brain the previous tick neither evaluated nor skipped left the set
/// (quiescence, a queued despawn, inventory ownership, death): it returns
/// holding nothing, requesting nothing, with no clock, and spans one tick. A
/// brain the schedule skipped kept everything, and spans every tick since it
/// last ran.
pub(crate) fn admit(turns: &mut AttackTurnState, tick: u64, present_last_tick: bool) -> u32 {
    let stayed = present_last_tick || turns.seen_tick.wrapping_add(1) == tick;
    let span = if stayed && turns.seen_tick < tick {
        u32::try_from(tick - turns.seen_tick).unwrap_or(u32::MAX)
    } else {
        if !stayed {
            turns.clear();
        }
        1
    };
    turns.seen_tick = tick;
    span
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
///
/// A holder the schedule skipped this tick (`skipped`) stays: its turns hold
/// and count against their pools, none ends or starts a refresh, and it
/// requests nothing until it runs again.
pub(crate) fn arbitrate_attack_turns(
    outcomes: &mut [EnemyOutcome],
    skipped: &[SkippedHolder],
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
        // One turn covers a repeating release's passes: once it has struck, a
        // turn it lost (a target switch, a tick with no target) is not
        // requested again, so the passes left whiff on the hard gate below
        // rather than strike under a second turn.
        let spent_release = crate::clip_clock::repeating_release_struck(&outcome.brain);
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
            // A rest's queued grant is kept by nothing here but the mark: on
            // the evaluation after its grant, its row passes and the hold's
            // mark names it, or another row (or none) leaves the rest and the
            // turn ends with its refresh. A queued turn neither waits out a
            // cooldown nor executes before its release. A burst can still
            // outlast its recover and carry its turn into the rest; with no
            // mark on the rest's path naming it, that turn opens no queued
            // row, which passes only on the evaluation after a grant, so it
            // ends here with its refresh once the burst does. An ancestor
            // marked for the stance's attack keeps its turn across the rest,
            // and the row opens on it.
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

        let request = target.filter(|_| !spent_release).and_then(|target| {
            next_request(
                turns,
                graph,
                RequestSources {
                    mark,
                    queued: outcome.queued_attack,
                    pending: outcome.attack.as_ref(),
                },
                programs_entry,
                target,
                pool,
            )
        });
        // The clock is per target, not per attack: a change of requested
        // attack, a gap in requesting (leaving a rest and re-entering it),
        // and a move between a rest's request and its hold's mark all keep
        // the enemy's place in line on one target.
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

    for holder in skipped {
        roster.ended_for(holder.id, &holder.turns, |turn| {
            debug_assert!(false, "a skipped holder ended turn {}", turn.attack);
        });
        for turn in holder.turns.held() {
            arbiter.hold((holder.pool, turn.target), turn.draws);
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
            granted_tick: *tick,
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
            .map(|outcome| (outcome.id, &outcome.brain.attack_turns))
            .chain(skipped.iter().map(|holder| (holder.id, &holder.turns))),
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

/// Where an enemy's one request can come from this tick, highest first.
#[derive(Clone, Copy)]
pub(crate) struct RequestSources<'a> {
    /// The attack marked on the active path.
    pub(crate) mark: Option<u16>,
    /// The attack a standing rest's first open queued row waits on.
    pub(crate) queued: Option<u16>,
    /// The unmarked attack the fire step resolved this tick.
    pub(crate) pending: Option<&'a PendingAttack>,
}

/// The one request an enemy makes this tick, and whether it can be granted
/// yet. A marked path requests its mark until granted. Else a rest's queued
/// row requests its attack while the row waits on the turn alone. Else the
/// attack the fire step resolved requests, only on a tick it is ready to
/// fire. A queued request takes a mark's lifetime: it stands from the rest
/// and waits in line, and its grant is held to the next evaluation, where
/// only the hold's mark keeps it.
pub(crate) fn next_request(
    turns: &AttackTurnState,
    graph: &BehaviorGraphDescriptor,
    sources: RequestSources<'_>,
    programs: Option<&BrainEntityPrograms>,
    target: EntityId,
    pool: AttackPoolId,
) -> Option<(TurnRequest, bool)> {
    let (attack, marked) = match (sources.mark, sources.queued) {
        (Some(mark), _) => (mark, true),
        (None, Some(queued)) => (queued, true),
        (None, None) => (attack_index(graph, &sources.pending?.attack_name)?, false),
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
