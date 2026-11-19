// AI compute pass: evaluate each enemy snapshot into an outcome — target
// selection, guard facts, transitions, steering and the attack proposal.
// See: context/lib/entity_model.md §7c · context/lib/scripting.md §11

use super::brain_scope::SelectorChoices;
use glam::Vec3;
use postretro_entities::{EntityId, EntityRegistry, EntityStateComponent, LiveFactionSentiment};

use super::{FACTION_STATE_FIELD, heading, perception};
use crate::ai_host::{AiStageFrame, ai_stage};
use crate::sim::cpu_stages::SimStage;

pub(super) fn entity_faction(registry: &EntityRegistry, entity: EntityId) -> f32 {
    registry
        .get_component::<EntityStateComponent>(entity)
        .map_or(0.0, |state| state.get(FACTION_STATE_FIELD))
}

use super::brain_scope::BrainFacts;
use super::engine_floor::SteeringIntent;
use super::graph_eval::{
    ResolvedMotion, action_for_path, engages_active, engages_path, resolved_motion,
    select_transitions,
};
use super::steering::position_goal_steering;
use super::targeting::{
    TargetSelection, is_hostile, select_target_with_attacker_ledger, selected_target_alive,
    target_candidate, target_distance, target_offers,
};
use super::{AttackOutcome, EnemyOutcome, EnteredActivity, PendingAttack};
use super::{attack_turns, clip_clock, engaged_motion, leap};
use crate::nav::{NavScratch, NavStart, find_path_from_in};
use crate::weapon_controller::{WeaponAim, WeaponAttackRequest};
use postretro_entities::components::agent::AgentComponent;
use postretro_foundation::{ActionVerb, BRAIN_NO_TARGET_DISTANCE, GroundRef, MotionVerb};

/// Pass 2: evaluate each immutable enemy snapshot into an outcome.
#[allow(clippy::too_many_arguments)] // orchestration keeps each compute-stage input explicit.
pub(super) fn evaluate(
    registry: &postretro_entities::EntityRegistry,
    snapshots: Vec<super::EnemySnapshot>,
    programs: &mut super::brain_programs::BrainPrograms,
    reseat_warned: &mut std::collections::HashSet<EntityId>,
    los_grace: &mut std::collections::HashMap<EntityId, super::perception::LosGraceState>,
    tick_dt: f32,
    dt_ms: f32,
    nav_graph: Option<&crate::nav::NavGraph>,
    gate: crate::nav::NavGate<'_>,
    collision_world: Option<&crate::collision::CollisionWorld>,
    factions: &LiveFactionSentiment<'_>,
    turns: &attack_turns::AttackTurnRuntime,
    clips: Option<&crate::scripting_systems::hit_zones::HitZoneStore>,
    cpu: Option<&AiStageFrame>,
    nav_scratch: &mut NavScratch,
    pawns: &super::pawn_index::PawnIndex,
    selector_memo: &mut std::collections::HashMap<EntityId, SelectorChoices>,
    decide_interval: u64,
) -> Vec<super::EnemyOutcome> {
    let mut outcomes: Vec<EnemyOutcome> = Vec::with_capacity(snapshots.len());
    for (slot, snap) in snapshots.into_iter().enumerate() {
        // A brain the schedule skipped since its last run advances every
        // clock by all the ticks it missed: its whole tick runs with its step
        // scaled.
        let span = snap.span;
        let tick_dt = tick_dt * span as f32;
        let one_tick_dt_ms = dt_ms;
        let dt_ms = dt_ms * span as f32;
        let mut brain = snap.brain;
        let mut graph_reseated = programs.take_reseat(snap.id);
        if graph_reseated {
            brain.reseat_to_initial();
        }
        // Validate the complete restored path before any consumer walks it.
        // Checking only the root misses a stale child index or a descendant
        // retained beneath a leaf; trusting an over-cap length also panics in
        // timer/counter slice operations.
        if !brain.has_valid_active_path() {
            if reseat_warned.insert(snap.id) {
                log::warn!(
                    "[AI] enemy {} carried an invalid behavior activity path; re-seating it \
                     to `{}`. Warned once per enemy.",
                    snap.id,
                    brain.graph.envelope.initial,
                );
            }
            graph_reseated |= brain.reseat_to_initial();
        }
        // A reseat takes the enemy out of the AI set: its turns, request and
        // clock name attacks of the graph it left.
        if graph_reseated {
            brain.attack_turns.clear();
        }
        // A decide tick runs acquisition, guards, selectors and combat-slot
        // choice; the ticks between act on its choices. Each enemy decides on
        // its own phase of the 15 Hz cycle, keyed on its identity, and at once
        // whenever its path moved without a decide (a reseat).
        // A dormant run decides on every fourth dormant run instead.
        let decide = snap.dormant_decide.unwrap_or_else(|| {
            crate::decide::decides_on(
                turns.tick,
                postretro_sim::npc_tiers::schedule_identity(&brain, snap.id),
                decide_interval,
            )
        }) || graph_reseated
            || selector_memo
                .get(&snap.id)
                .is_none_or(|choices| !choices.is_for(brain.active_path()));
        // The guard scope is shared across enemies: every enemy starts out
        // evaluating, and only an armed acting pass recalls its own choices.
        programs.scope_mut().begin_decide(brain.active_path());
        // Candidate refresh runs during selection below, before the established
        // brain damage-fact aging site. Advance the paired attacker ledger here
        // exactly once so `@candidate.timeSinceDamageFromCandidate` sees the
        // same post-tick age that `@brain.timeSinceDamageMs` later publishes.
        brain.age_recent_attackers_over(dt_ms, one_tick_dt_ms);
        // Read the evaluating enemy's mutable faction once for the whole
        // compute pass. Candidate comparison consumes this scalar only on a
        // fresh scan; retained target lookup deliberately does not see it.
        let enemy_faction = entity_faction(registry, snap.id);
        // A home-distance guard is about the evaluating enemy alone, not its
        // target or the acquisition stride. Compute it once from this tick's
        // immutable position snapshot before either branch can suppress target
        // work.
        let distance_from_anchor = crate::nav::distance_xz(snap.position, brain.home_anchor);
        let prior_acquired_target = brain.acquired_target;
        let targeting_stage = ai_stage(cpu, SimStage::AiTargeting);
        let (target_selection, evaluate_acquisition) = if brain.aggro_armed {
            // A target is retained across ticks only while the brain is engaged
            // — chasing one, or acting on one. A resting brain re-ranks
            // candidates instead of honoring a stale acquired id.
            let retained_target = engages_active(&brain)
                .then_some(brain.acquired_target)
                .flatten();
            let retained = retained_target
                .and_then(|entity| target_candidate(registry, entity, snap.position));
            let (target, evaluate_acquisition) = if let Some(retained) = retained {
                // A retained target alone prices the stride from its raw
                // distance. A due tick may still run the normal hysteresis scan
                // below, but neither its candidate filter nor its result can
                // alter this cost input.
                let evaluate_acquisition = decide;
                let target = if evaluate_acquisition {
                    let (candidate_filter, candidate_scope) =
                        programs.candidate_filter_context(snap.id);
                    let offers = target_offers(snap.position, Some(retained.target.entity));
                    let enemy_eye =
                        perception::enemy_eye(registry, snap.id, snap.position, nav_graph);
                    let mut candidate_perception = |candidate| {
                        perception::raw_target_perception(
                            registry,
                            enemy_eye,
                            candidate,
                            collision_world,
                        )
                    };
                    select_target_with_attacker_ledger(
                        Some(retained),
                        &offers,
                        pawns,
                        registry,
                        factions,
                        Some(snap.id),
                        enemy_faction,
                        candidate_filter,
                        candidate_scope,
                        &brain.recent_attackers,
                        brain.retaliation_acquired_target,
                        brain.graph.retaliation(),
                        dt_ms,
                        &mut candidate_perception,
                    )
                } else {
                    Some(TargetSelection {
                        target: retained.target,
                        fresh_perception: None,
                        retaliation_acquired_target: brain
                            .retaliation_acquired_target
                            .filter(|target| *target == retained.target.entity),
                    })
                };
                (target, evaluate_acquisition)
            } else if !decide {
                // Between decides an enemy without a retained target acquires
                // nothing and scans nothing.
                (None, false)
            } else {
                let offers = target_offers(snap.position, None);
                let evaluate_acquisition = true;
                let (candidate_filter, candidate_scope) =
                    programs.candidate_filter_context(snap.id);
                let target = evaluate_acquisition.then(|| {
                    let enemy_eye =
                        perception::enemy_eye(registry, snap.id, snap.position, nav_graph);
                    let mut candidate_perception = |candidate| {
                        perception::raw_target_perception(
                            registry,
                            enemy_eye,
                            candidate,
                            collision_world,
                        )
                    };
                    select_target_with_attacker_ledger(
                        None,
                        &offers,
                        pawns,
                        registry,
                        factions,
                        Some(snap.id),
                        enemy_faction,
                        candidate_filter,
                        candidate_scope,
                        &brain.recent_attackers,
                        brain.retaliation_acquired_target,
                        brain.graph.retaliation(),
                        dt_ms,
                        &mut candidate_perception,
                    )
                });
                (target.flatten(), evaluate_acquisition)
            };
            (target, evaluate_acquisition)
        } else {
            (None, false)
        };
        drop(targeting_stage);
        let graph_stage = ai_stage(cpu, SimStage::AiGraph);
        let target = target_selection.map(|selection| selection.target);

        // (1) Every named cooldown ticks down before the aggro gate and before
        // any guard reads its selected attack's value. Entries do not freeze
        // while another attack is current, nor disappear on a graph reseat.
        for remaining_ms in brain.attack_cooldown_remaining_ms.values_mut() {
            *remaining_ms = (*remaining_ms - dt_ms).max(0.0);
        }
        // Damage recency advances even while an activity entry is pending or
        // aggro is closed, matching the unconditional named-cooldown clock.
        // A hit since the last run ages one tick, never the run's whole span.
        brain.time_since_damage_ms = postretro_entities::components::brain::age_damage_clock(
            brain.time_since_damage_ms,
            dt_ms,
            one_tick_dt_ms,
        );
        // An entry edge observes a freshly zeroed activity clock. Once it has
        // been consumed, subsequent ticks advance the active clocks before
        // evaluating their transition rows. A transition later in this pass
        // resets its new suffix again, so no newly entered activity inherits a
        // fraction of its predecessor's tick.
        if !brain.entry_pending {
            brain.tick_activity_timers(dt_ms);
        }
        let clip_source = clip_clock::ClipSource {
            registry,
            clips,
            id: snap.id,
        };
        clip_clock::begin_tick(&mut brain, &clip_source, tick_dt);
        // Read before transitions: a `"*"` or ancestor row, or the flinch
        // route itself on a reaction already played out, can leave the
        // flinched activity this tick and clear its stagger.
        let flinched = brain.clip_clock.stagger.is_some();
        let mut animation_done = clip_clock::animation_done(&brain.clip_clock, &clip_source);
        // A path seated before this tick's rows run (a reseat) evaluates them
        // against the clip it is about to restart, not its predecessor's.
        if brain.entry_pending
            && let Some(entered) = clip_clock::entered_animation_done(&brain, &clip_source)
        {
            animation_done = entered;
        }

        drop(graph_stage);
        let perception_stage = target_selection
            .is_some()
            .then(|| ai_stage(cpu, SimStage::AiPerception))
            .flatten();
        // The FINALLY selected pawn's identity and distance, or `None` with no
        // target. This one binding feeds the guard facts and attack range gate,
        // so neither can disagree about which target they describe.
        let target_perception = target_selection.and_then(|selection| {
            perception::perceive_target(
                registry,
                los_grace,
                perception::TargetPerceptionQuery {
                    enemy: snap.id,
                    enemy_position: snap.position,
                    target: selection.target,
                    nav_graph,
                    collision_world,
                    fresh: selection.fresh_perception,
                    span,
                },
            )
        });
        drop(perception_stage);
        let _graph_stage = ai_stage(cpu, SimStage::AiGraph);
        if target.is_none() {
            los_grace.remove(&snap.id);
        }
        // The graph fact and the engine-floor fire gate consume this one
        // already-debounced perception result. Keep the fire gate independent
        // of authoring below; it still calls `perception::fire_gate` directly.
        let target_visible = target_perception.is_some_and(|perception| perception.visible);
        let enemy_eye_offset = target_perception
            .map(|perception| perception.enemy_eye - snap.position)
            .unwrap_or(Vec3::ZERO);
        let target_aim = target_perception.map(|perception| perception.target_aim);

        let selected_target = target.map(|target| {
            (
                target.entity,
                target_distance(target, snap.position),
                target.position,
            )
        });
        // Sight memory is a host-only fact about the selected target's last
        // visible position. The shared debounced verdict is authoritative: a
        // visible target refreshes this cache, overwriting the prior-tick damage
        // seed. An unseen target preserves that seed for authored investigation.
        if target_visible {
            if let Some((_, _, target_position)) = selected_target {
                brain.last_known_target_pos = Some(target_position);
                brain.time_since_target_visible = 0.0;
            }
        } else {
            brain.time_since_target_visible =
                (brain.time_since_target_visible + dt_ms).clamp(0.0, BRAIN_NO_TARGET_DISTANCE);
        }
        let distance_to_last_known = brain
            .last_known_target_pos
            .map(|position| crate::nav::distance_xz(snap.position, position))
            .unwrap_or(BRAIN_NO_TARGET_DISTANCE);
        let target_hostile = selected_target.is_some_and(|(target, _, _)| {
            is_hostile(factions, enemy_faction, entity_faction(registry, target))
        });
        // Reachability is the nav floor's pathfinder verdict, cached on the
        // existing acquisition stride. It deliberately mirrors the gated route
        // query chase consumes, from the agent's own ground, rather than
        // claiming a stronger ground-truth answer. An absent nav graph has no
        // route query, so it is immediately unreachable even if a restored
        // brain retained a cached result from an earlier map or acquisition
        // stride.
        let target_reachable = match (target, nav_graph) {
            (Some(_), None) => {
                brain.target_reachable = false;
                false
            }
            (Some(target), Some(graph)) if evaluate_acquisition => {
                let agent = registry.get_component::<AgentComponent>(snap.id).ok();
                let ground = agent.map_or(GroundRef::World, |agent| agent.ground);
                let height = agent.map_or(graph.agent_params().height, |agent| agent.height);
                // A rider's route is the ride, so the fact holds its boarding
                // value until it steps off. A body carried between docks
                // before steering starts its ride has no route start until
                // the mover docks, so its last verdict holds too.
                let holds = agent.is_some_and(|agent| agent.ride.is_some())
                    || graph.between_docks(gate, ground);
                if !holds {
                    let from = NavStart {
                        position: snap.position,
                        ground,
                        height,
                    };
                    brain.target_reachable =
                        find_path_from_in(nav_scratch, graph, gate, from, target.position)
                            .is_some();
                }
                brain.target_reachable
            }
            (Some(_), Some(_)) => brain.target_reachable,
            (None, _) => {
                brain.target_reachable = false;
                false
            }
        };
        let selected_distance = selected_target.map(|(_, distance, _)| distance);
        // Read before transitions: a stance release that resolved without
        // striking, left on this tick, starts its attack's cooldown below.
        let whiffed_release = clip_clock::whiffed_release(&brain);
        let mut prior_standoff_distance = brain.graph.standoff_distance_for_action(None);
        // Set on a leap's entry whose spot found no walkable ground.
        let mut leap_spot_off_mesh = false;
        let (transitioned, resolved, steering, queued_attack) = if !brain.aggro_armed {
            // THE AGGRO GATE, and the only thing that suppresses evaluation. Its
            // v1 disengage policy is hold: a closed brain consults neither target
            // selection nor its guards, and standing down clears steering outright
            // rather than deferring to the resting state's motion verb. Clearing
            // the destination sends the agent through steering's destination-less
            // idle-settle path, which has no separation push.
            let transitioned = if !brain.is_seated_at_initial() {
                brain.reseat_to_initial()
            } else {
                false
            };
            (transitioned, None, SteeringIntent::Clear, None)
        } else {
            // On a decide tick, facts refresh and the guard set runs: with no
            // target the facts still refresh (`hasTarget` false,
            // `targetDistance` at its sentinel), because an armed brain
            // evaluates its whole guard set whether or not it has a pawn. The
            // ticks between act on the path and selector rows it chose.
            let (transitioned, queued) = if decide {
                programs.scope_mut().begin_decide(brain.active_path());
                // A queued row opens on a turn granted since the previous
                // decide.
                brain.attack_turns.decide_span = selector_memo
                    .get(&snap.id)
                    .and_then(|choices| u32::try_from(turns.tick - choices.tick()).ok())
                    .unwrap_or(1)
                    .max(1);
                brain.attack_turns.last_decide_tick = turns.tick;
                // Turn facts read the previous tick's turns and line, against the
                // target selected now.
                let has_attack_turn = brain.attack_turns.holds_any();
                programs.with_entry_scope(snap.id, |bound, scope| {
                    let reader = attack_turns::TurnFactReader {
                        line: &turns.previous,
                        budgets: &turns.budgets,
                        refresh: turns.previous_refresh(),
                        entity: snap.id,
                        slot: slot as u32,
                        pool: attack_turns::attack_pool_of(&turns.budgets, &brain),
                        target: target.map(|target| target.entity),
                        turns: &brain.attack_turns,
                        graph: &brain.graph,
                        programs: bound,
                    };
                    scope.refresh_attack_turns(|attack, read| reader.read(attack, read));
                });
                // Selector guards may choose the current action, but their input
                // snapshot has not yet been refreshed for this enemy. Seed a
                // type-correct zero cooldown first, resolve that action without
                // cloning it, then refresh the one action-relative cooldown fact
                // the transition planner is allowed to observe.
                let facts = BrainFacts {
                    target: selected_target,
                    attack_cooldown_ms: 0.0,
                    time_since_damage_ms: brain.time_since_damage_ms,
                    time_since_target_visible: brain.time_since_target_visible,
                    distance_to_last_known,
                    damage_bearing: brain.damage_bearing,
                    damage_source_known: brain.damage_source_known,
                    acquisition_due: evaluate_acquisition,
                    distance_from_anchor,
                    target_hostile,
                    target_reachable,
                    target_visible,
                    attacks_fired_in_activity: brain.activity_attack_count(0).unwrap_or(0),
                    has_attack_turn,
                    animation_done,
                };
                programs.scope_mut().refresh(registry, snap.id, facts);
                let attack_cooldown_ms = programs
                    .with_entry_scope(snap.id, |bound, scope| {
                        action_for_path(bound, scope, &brain).and_then(|action| match action {
                            ActionVerb::Attack { name, .. } => {
                                brain.attack_cooldown_remaining_ms.get(name).copied()
                            }
                        })
                    })
                    .flatten()
                    .unwrap_or(0.0);
                let facts = BrainFacts {
                    attack_cooldown_ms,
                    ..facts
                };
                programs.scope_mut().refresh(registry, snap.id, facts);
                prior_standoff_distance = programs
                    .with_entry_scope(snap.id, |bound, scope| {
                        action_for_path(bound, scope, &brain)
                            .map(|action| brain.graph.standoff_distance_for_action(Some(action)))
                    })
                    .flatten()
                    .unwrap_or_else(|| brain.graph.standoff_distance_for_action(None));
                // The pass also names the attack a standing rest's first open
                // queued row waits on; arbitration turns it into this tick's
                // request. A hold's release row reads its turn against the target
                // selected now, as the turn facts do.
                let selected = target.map(|target| target.entity);
                let pass = programs
                    .with_entry_scope(snap.id, |bound, scope| {
                        select_transitions(bound, scope, &mut brain, selected)
                    })
                    .unwrap_or_default();
                let transitioned = pass.transitioned;
                // The entered path's selectors read the clip `settle` is about to
                // restart, not its predecessor's; a path back onto the tracked
                // state reads the clock's own value, which a reseat's rows did not.
                // The rows above read one value and the selectors below another,
                // one per evaluation pass.
                if transitioned {
                    let entered = clip_clock::entered_animation_done(&brain, &clip_source)
                        .unwrap_or_else(|| {
                            clip_clock::animation_done(&brain.clip_clock, &clip_source)
                        });
                    if entered != facts.animation_done {
                        programs.scope_mut().refresh(
                            registry,
                            snap.id,
                            BrainFacts {
                                animation_done: entered,
                                ..facts
                            },
                        );
                    }
                }
                // Selectors record against the path the rows left.
                programs.scope_mut().begin_decide(brain.active_path());
                (transitioned, pass.queued)
            } else {
                let recalled = programs
                    .scope_mut()
                    .begin_recall(selector_memo.get(&snap.id).copied(), brain.active_path());
                debug_assert!(recalled, "a decide follows any path move");
                prior_standoff_distance = programs
                    .with_entry_scope(snap.id, |bound, scope| {
                        action_for_path(bound, scope, &brain)
                            .map(|action| brain.graph.standoff_distance_for_action(Some(action)))
                    })
                    .flatten()
                    .unwrap_or_else(|| brain.graph.standoff_distance_for_action(None));
                (
                    false,
                    selector_memo
                        .get(&snap.id)
                        .and_then(SelectorChoices::queued),
                )
            };
            let resolved = programs
                .with_entry_scope(snap.id, |bound, scope| {
                    resolved_motion(bound, scope, &brain)
                })
                .flatten();
            let steering = resolved
                .map(|motion| position_goal_steering(motion.verb, &mut brain, snap.position))
                .unwrap_or(SteeringIntent::Clear);
            // A chase with nothing to chase degrades to a stand-down: with no
            // target there is nothing to move relative to, and leaving the intent
            // as Chase would keep the agent walking to the last destination it
            // was given.
            let steering = match (steering, target) {
                (SteeringIntent::Chase, None) => SteeringIntent::Clear,
                (steering, _) => steering,
            };
            // A leap travels to the spot it fixed on entry, never re-aiming.
            let (steering, off_mesh) = leap::steer(
                &mut brain,
                resolved,
                target,
                snap.position,
                steering,
                nav_graph,
            );
            leap_spot_off_mesh = off_mesh;
            // A flinch over a release or recover plants the enemy for the
            // rest of the activity, whatever its motion: a staggered charge
            // stops where it was hit.
            let (resolved, steering) = if brain.clip_clock.stagger.is_some() {
                (
                    resolved.map(|motion| ResolvedMotion {
                        verb: MotionVerb::Hold,
                        ..motion
                    }),
                    SteeringIntent::Clear,
                )
            } else {
                (resolved, steering)
            };
            (transitioned, resolved, steering, queued)
        };
        let motion = resolved.map(|motion| motion.verb);

        // The acquired id is the cross-tick retention marker. Keep it while the
        // active path remains capable of engagement, even when an in-range move
        // selector currently resolves to `hold` and the committed leaf has no
        // action. A transition to a genuinely idle or position-goal path still
        // clears it here.
        let resolved_position_goal = motion.is_some_and(MotionVerb::is_position_goal);
        let retains_target = target.is_some() && !resolved_position_goal && engages_active(&brain);
        brain.acquired_target = match target {
            Some(target) if retains_target => Some(target.entity),
            _ => None,
        };
        brain.retaliation_acquired_target = match target_selection {
            Some(selection)
                if retains_target
                    && selection.retaliation_acquired_target == Some(selection.target.entity) =>
            {
                Some(selection.target.entity)
            }
            _ => None,
        };

        // Resolved engagement remains the facing policy for ordinary chase and
        // action paths. Committed actionless aim is handled in `heading`.
        let engaged = target.is_some()
            && !resolved_position_goal
            && programs
                .with_entry_scope(snap.id, |bound, scope| engages_path(bound, scope, &brain))
                .unwrap_or(false);

        let tick_heading = heading::resolve(
            registry,
            &mut brain,
            heading::HeadingQuery {
                id: snap.id,
                rotation: snap.rotation,
                motion,
                steering,
                engaged,
                retains_target,
                aim_direction: target_perception
                    .map(|perception| perception.target_aim - perception.enemy_eye),
                tick_dt,
                span,
            },
        );
        let weapon_aim = target_perception.and_then(|perception| {
            Some(WeaponAim {
                origin: perception.enemy_eye,
                direction: (perception.target_aim - perception.enemy_eye).try_normalize()?,
            })
        });

        // (4) Attack: the active firing leaf latches one graph-wide attack on
        // its first clear dwell tick. Its own cooldown must have elapsed, the
        // SELECTED target must be inside its effective range, and it must still
        // be alive. The LOS and facing gates read this tick's shared debounced
        // perception and post-slew heading respectively.
        // The range gate lets a graph declare the action without making it
        // connect from across the room.
        // An unresolved action name configures no range and no damage, so it
        // never attacks.
        // Gating on the selected target's damage eligibility stops attack/event
        // spam against an already-dead or removal-committed pawn and prevents
        // damaging a different co-op pawn than the one this enemy chose.
        // A clip-timed attack proposes only on its moment's tick, its one
        // chance per entry: a gate failing there is a whiff. A flinch over a
        // release or recover fires nothing further that entry.
        let entry_depth = brain.take_entry_pending();
        let moment = clip_clock::settle(
            &mut brain,
            &clip_source,
            entry_depth,
            tick_heading.travel_variant,
        );
        // A leap arrives at its spot by the moment the clock now reads, and
        // stands once the latch it steered on has closed.
        let (steering, leap_travel) = leap::pace(&brain, &clip_source, motion, steering);
        // A repeating release strikes once per pass at its moment, under one
        // turn: a later pass ignores the cooldown its first hit started, and
        // each hit restarts it, so it runs from the release's last hit.
        let strike_limit = clip_clock::strike_limit(&brain, moment);
        let leaf_strikes = brain
            .active_depth()
            .checked_sub(1)
            .and_then(|depth| brain.activity_attack_count(depth));
        let later_pass = strike_limit > 1 && leaf_strikes.is_some_and(|strikes| strikes > 0);
        let mut attack_outcome = None;
        if let Some(firing_leaf_depth) = brain.active_depth().checked_sub(1)
            && moment.allows_proposal()
            && brain.clip_clock.stagger.is_none()
            && let (Some(target), Some(distance)) = (target, selected_distance)
            && brain
                .activity_attack_count(firing_leaf_depth)
                .is_some_and(|strikes| strikes < strike_limit)
            && selected_target_alive(registry, target.entity)
            && perception::fire_gate(target_perception)
            && tick_heading.within_attack_tolerance
            && let Some((attack_name, cooldown_ms, outcome)) = programs
                .with_entry_scope(snap.id, |bound, scope| {
                    let action = action_for_path(bound, scope, &brain)?;
                    let ActionVerb::Attack { name, .. } = action;
                    let attack = brain.graph.attacks.get(name)?;

                    if !later_pass
                        && brain
                            .attack_cooldown_remaining_ms
                            .get(name)
                            .copied()
                            .unwrap_or(0.0)
                            > 0.0
                    {
                        return None;
                    }

                    if attack.weapon.is_some() {
                        let resolved = bound.resolved_projectile_attack(name)?;
                        if distance > resolved.range() {
                            return None;
                        }

                        weapon_aim?;
                        Some((
                            name.clone(),
                            resolved.cooldown_ms(),
                            AttackOutcome::Projectile {
                                request: WeaponAttackRequest {
                                    canonical_weapon: resolved.canonical_weapon_name().to_string(),
                                    attack_name: name.clone(),
                                },
                            },
                        ))
                    } else {
                        let (Some(damage), Some(max_range), Some(cooldown_ms)) =
                            (attack.damage, attack.max_range, attack.cooldown_ms)
                        else {
                            return None;
                        };
                        if distance > max_range {
                            return None;
                        }
                        let damage_type = attack
                            .damage_type
                            .as_ref()
                            .and_then(|handle| handle.handle("damageType").ok())
                            .map(|handle| handle.name().to_string());
                        Some((
                            name.clone(),
                            cooldown_ms,
                            AttackOutcome::Contact {
                                damage,
                                damage_type,
                            },
                        ))
                    }
                })
                .flatten()
        {
            // This is only an immutable fire proposal. A target can become
            // dead or disappear while an earlier outcome applies, so the
            // mutable cooldown/count commit belongs beside the effect in apply.
            attack_outcome = Some(PendingAttack {
                attack_name,
                cooldown_ms,
                effect: outcome,
            });
        }
        // The enemy committed: a stance release left having struck nothing,
        // every moment whiffed, starts its attack's cooldown as a hit would,
        // so a whiffed leap is not queued again at once. A release a flinch
        // staggered is not a whiff. Set after this tick's proposal, which the
        // entered path formed.
        if let Some((depth, attack)) = whiffed_release
            && entry_depth.is_some_and(|entry| entry <= depth)
            && let Some((name, cooldown_ms)) = attack_turns::attack_name(&brain.graph, attack)
                .and_then(|name| {
                    let params = brain.graph.attacks.get(name)?;
                    let cooldown_ms = if params.weapon.is_some() {
                        programs
                            .with_entry_scope(snap.id, |bound, _| {
                                bound
                                    .resolved_projectile_attack(name)
                                    .map(|resolved| resolved.cooldown_ms())
                            })
                            .flatten()?
                    } else {
                        params.cooldown_ms?
                    };
                    Some((name.to_string(), cooldown_ms))
                })
        {
            brain.attack_cooldown_remaining_ms.insert(name, cooldown_ms);
        }

        let state_changed = graph_reseated || transitioned || entry_depth.is_some();
        let announce_entry = brain.take_entry_event_pending() && entry_depth.is_some();
        // An announced entry is an emission even without an authored
        // `on_enter`: the activity's entry sound plays regardless. The path
        // names the entered leaf in the graph the brain holds now.
        let entered = announce_entry.then(|| EnteredActivity {
            on_enter: brain
                .active_depth()
                .checked_sub(1)
                .and_then(|depth| brain.activity_at_depth(depth))
                .and_then(|(_, activity)| activity.on_enter.clone()),
            path: brain.active_path().to_vec(),
        });
        let standoff_distance = programs
            .with_entry_scope(snap.id, |bound, scope| {
                action_for_path(bound, scope, &brain)
                    .map(|action| brain.graph.standoff_distance_for_action(Some(action)))
            })
            .flatten()
            .unwrap_or_else(|| brain.graph.standoff_distance_for_action(None));
        // A strafer re-aims, and a reposition picks its ring, each decide.
        let engaged_ring = decide
            .then(|| {
                engaged_motion::plan(
                    &mut brain,
                    resolved.filter(|_| engaged),
                    entry_depth,
                    target,
                    snap.position,
                    standoff_distance,
                )
            })
            .flatten();
        if decide && brain.aggro_armed {
            let scope = programs.scope_mut();
            scope.record_decide(turns.tick, queued_attack);
            selector_memo.insert(snap.id, scope.selector_choices());
        }
        outcomes.push(EnemyOutcome {
            id: snap.id,
            span,
            decide,
            position: snap.position,
            target,
            enemy_eye_offset,
            target_aim,
            prior_acquired_target,
            graph_reseated,
            state_changed,
            attack: attack_outcome,
            weapon_aim,
            prior_standoff_distance,
            standoff_distance,
            entered,
            steering,
            leap: leap_travel,
            leap_spot_off_mesh,
            engaged,
            facing_direction: tick_heading.facing_direction,
            travel_variant: tick_heading.travel_variant,
            combat_slot: None,
            engaged_ring,
            queued_attack,
            fresh_unmarked_grant: None,
            flinched,
            brain,
        });
    }

    outcomes
}
