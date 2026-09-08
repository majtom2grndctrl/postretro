use glam::Vec3;
use postretro_entities::{EntityId, EntityRegistry, EntityStateComponent, FactionRegistry};

use super::{FACTION_STATE_FIELD, LocomotionIntent, perception};

/// Resolve the one direction this tick's apply pass will slew toward. A
/// committed aim holds its movement destination but still turns toward the
/// shared eye-to-target vector; other engaged states keep the established
/// velocity-first facing behavior.
pub(super) fn facing_direction(
    steering: SteeringIntent,
    engaged: bool,
    committed_aim: bool,
    target_perception: Option<perception::EnemyTargetPerception>,
    path_state: Option<crate::agent_steering::AgentPathState>,
) -> Option<Vec3> {
    let moving_velocity = path_state
        .map(|path| path.velocity)
        .filter(|velocity| LocomotionIntent::from_velocity(*velocity).moving);
    match steering {
        SteeringIntent::MoveTo(_) if let Some(velocity) = moving_velocity => Some(velocity),
        SteeringIntent::MoveTo(_) => None,
        _ if committed_aim => {
            target_perception.map(|perception| perception.target_aim - perception.enemy_eye)
        }
        _ if engaged && let Some(velocity) = moving_velocity => Some(velocity),
        _ if engaged => {
            target_perception.map(|perception| perception.target_aim - perception.enemy_eye)
        }
        _ => None,
    }
}

pub(super) fn entity_faction(registry: &EntityRegistry, entity: EntityId) -> f32 {
    registry
        .get_component::<EntityStateComponent>(entity)
        .map_or(0.0, |state| state.get(FACTION_STATE_FIELD))
}

use super::brain_scope::BrainFacts;
use super::engine_floor::SteeringIntent;
use super::facing::{
    FACING_TURN_RATE, slewed_yaw_toward, yaw_from_rotation, yaw_within_attack_tolerance,
};
use super::graph_eval::{
    action_for_path, engages_active, engages_path, motion_for_path, select_transition_path,
};
use super::steering::position_goal_steering;
use super::targeting::{
    TargetSelection, acquisition_due, is_hostile, select_target_with_attacker_ledger,
    selected_target_alive, target_candidate, target_distance, target_offers,
};
use super::{AttackOutcome, EnemyOutcome, PendingAttack};
use crate::agent_steering;
use crate::nav::find_path;
use crate::weapon::ProjectileLaunch;
use postretro_foundation::{ActionVerb, BRAIN_NO_TARGET_DISTANCE, MotionVerb};

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
    collision_world: Option<&crate::collision::CollisionWorld>,
    factions: &FactionRegistry,
) -> Vec<super::EnemyOutcome> {
    let mut outcomes: Vec<EnemyOutcome> = Vec::with_capacity(snapshots.len());
    for snap in snapshots {
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
        // Candidate refresh runs during selection below, before the established
        // brain damage-fact aging site. Advance the paired attacker ledger here
        // exactly once so `@candidate.timeSinceDamageFromCandidate` sees the
        // same post-tick age that `@brain.timeSinceDamageMs` later publishes.
        brain.age_recent_attackers(dt_ms);
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
                let evaluate_acquisition = acquisition_due(&brain, Some(retained.distance));
                let target = if evaluate_acquisition {
                    let (candidate_filter, candidate_scope) =
                        programs.candidate_filter_context(snap.id);
                    let offers = target_offers(
                        registry,
                        factions,
                        snap.position,
                        enemy_faction,
                        Some(snap.id),
                        Some(retained.target.entity),
                    );
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
            } else {
                let offers = target_offers(
                    registry,
                    factions,
                    snap.position,
                    enemy_faction,
                    Some(snap.id),
                    None,
                );
                let evaluate_acquisition =
                    acquisition_due(&brain, offers.nearest.map(|candidate| candidate.distance));
                let (candidate_filter, candidate_scope) =
                    programs.candidate_filter_context(snap.id);
                // The raw nearest hostile offer prices the stride. A
                // graph- and LOS-filtered selection becomes a target only on a
                // due tick; otherwise `BrainFacts` stay untargeted rather than
                // borrowing it. The offer set avoids a second registry walk
                // while keeping exact candidate raycasts off non-due ticks.
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
        let target = target_selection.map(|selection| selection.target);

        // (1) Every named cooldown ticks down before the aggro gate and before
        // any guard reads its selected attack's value. Entries do not freeze
        // while another attack is current, nor disappear on a graph reseat.
        for remaining_ms in brain.attack_cooldown_remaining_ms.values_mut() {
            *remaining_ms = (*remaining_ms - dt_ms).max(0.0);
        }
        // Damage recency advances even while an activity entry is pending or
        // aggro is closed, matching the unconditional named-cooldown clock.
        brain.time_since_damage_ms =
            (brain.time_since_damage_ms + dt_ms).clamp(0.0, BRAIN_NO_TARGET_DISTANCE);
        // An entry edge observes a freshly zeroed activity clock. Once it has
        // been consumed, subsequent ticks advance the active clocks before
        // evaluating their transition rows. A transition later in this pass
        // resets its new suffix again, so no newly entered activity inherits a
        // fraction of its predecessor's tick.
        if !brain.entry_pending {
            brain.tick_activity_timers(dt_ms);
        }

        // Stride bookkeeping advances every tick so the gate is deterministic.
        brain.think_stride_counter = brain.think_stride_counter.wrapping_add(1);

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
                },
            )
        });
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
        // existing acquisition stride. It deliberately mirrors the same
        // `find_path` capability chase consumes, rather than claiming a
        // stronger ground-truth answer. An absent nav graph has no route query,
        // so it is immediately unreachable even if a restored brain retained a
        // cached result from an earlier map or acquisition stride.
        let target_reachable = match (target, nav_graph) {
            (Some(_), None) => {
                brain.target_reachable = false;
                false
            }
            (Some(target), Some(graph)) if evaluate_acquisition => {
                let reachable = find_path(graph, snap.position, target.position).is_some();
                brain.target_reachable = reachable;
                reachable
            }
            (Some(_), Some(_)) => brain.target_reachable,
            (None, _) => {
                brain.target_reachable = false;
                false
            }
        };
        let selected_distance = selected_target.map(|(_, distance, _)| distance);
        let mut prior_standoff_distance = brain.graph.standoff_distance_for_action(None);
        let (transitioned, motion, steering) = if !brain.aggro_armed {
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
            (transitioned, None, SteeringIntent::Clear)
        } else {
            // The think stride is derived from the CURRENT player distance; the
            // gate fires when the per-enemy counter aligns with the band's
            // divisor, and reaches the guards as `@brain.acquisitionDue` for the
            // edges that opt into it. With no target the facts still refresh —
            // `hasTarget` false, `targetDistance` at its sentinel — because an
            // armed brain evaluates its whole guard set whether or not it has a
            // pawn: that is how an interrupt reaches an enemy nobody is standing
            // in front of.
            // Selector guards may choose the current action, but their input
            // snapshot has not yet been refreshed for this enemy. Seed a
            // type-correct zero cooldown first, resolve that action without
            // cloning it, then refresh the one action-relative cooldown fact
            // the transition planner is allowed to observe.
            programs.scope_mut().refresh(
                registry,
                snap.id,
                BrainFacts {
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
                },
            );
            let attack_cooldown_ms = programs
                .with_entry_scope(snap.id, |bound, scope| {
                    action_for_path(bound, scope, &brain).and_then(|action| match action {
                        ActionVerb::Attack(name) => {
                            brain.attack_cooldown_remaining_ms.get(name).copied()
                        }
                    })
                })
                .flatten()
                .unwrap_or(0.0);
            programs.scope_mut().refresh(
                registry,
                snap.id,
                BrainFacts {
                    target: selected_target,
                    attack_cooldown_ms,
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
                },
            );
            prior_standoff_distance = programs
                .with_entry_scope(snap.id, |bound, scope| {
                    action_for_path(bound, scope, &brain)
                        .map(|action| brain.graph.standoff_distance_for_action(Some(action)))
                })
                .flatten()
                .unwrap_or_else(|| brain.graph.standoff_distance_for_action(None));
            let transitioned = programs
                .with_entry_scope(snap.id, |bound, scope| {
                    select_transition_path(bound, scope, &mut brain)
                })
                .unwrap_or(false);
            let motion = programs
                .with_entry_scope(snap.id, |bound, scope| {
                    motion_for_path(bound, scope, &brain)
                })
                .flatten();
            let steering = motion
                .map(|motion| position_goal_steering(motion, &mut brain, snap.position))
                .unwrap_or(SteeringIntent::Clear);
            // A chase with nothing to chase degrades to a stand-down: with no
            // target there is nothing to move relative to, and leaving the intent
            // as Chase would keep the agent walking to the last destination it
            // was given.
            let steering = match (steering, target) {
                (SteeringIntent::Chase, None) => SteeringIntent::Clear,
                (steering, _) => steering,
            };
            (transitioned, motion, steering)
        };

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
        // action paths. Committed actionless aim is handled separately below.
        let engaged = target.is_some()
            && !resolved_position_goal
            && programs
                .with_entry_scope(snap.id, |bound, scope| engages_path(bound, scope, &brain))
                .unwrap_or(false);

        // A nested offense phase can intentionally hold at an authored standoff
        // before its leaf exposes an action. It is still committed to the
        // target: keep its aim moving even though its resolved current verb is
        // `hold`, which `engages_path` correctly leaves false.
        let committed_aim = retains_target && matches!(motion, Some(MotionVerb::Hold));
        let facing_direction = facing_direction(
            steering,
            engaged,
            committed_aim,
            target_perception,
            agent_steering::path_state(registry, snap.id),
        );
        // If no horizontal yaw is derivable (for example, melee contact's
        // vertical eye-to-aim segment), apply leaves the transform untouched;
        // the firing check therefore reads that unchanged heading.
        let post_slew_yaw = facing_direction
            .and_then(|direction| {
                slewed_yaw_toward(snap.rotation, direction, FACING_TURN_RATE * tick_dt)
            })
            .unwrap_or_else(|| yaw_from_rotation(snap.rotation));
        let post_slew_facing_is_within_tolerance = target_perception.is_some_and(|perception| {
            yaw_within_attack_tolerance(post_slew_yaw, perception.target_aim - perception.enemy_eye)
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
        let entered = brain.take_entry_pending();
        let mut attack_outcome = None;
        if let Some(firing_leaf_depth) = brain.active_depth().checked_sub(1)
            && let (Some(target), Some(distance)) = (target, selected_distance)
            && brain.activity_attack_count(firing_leaf_depth) == Some(0)
            && selected_target_alive(registry, target.entity)
            && perception::fire_gate(target_perception)
            && post_slew_facing_is_within_tolerance
            && let Some((attack_name, cooldown_ms, outcome)) = programs
                .with_entry_scope(snap.id, |bound, scope| {
                    let action = action_for_path(bound, scope, &brain)?;
                    let ActionVerb::Attack(name) = action;
                    let attack = brain.graph.attacks.get(name)?;

                    if brain
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

                        let perception = target_perception?;
                        let origin = perception.enemy_eye;
                        let direction = (perception.target_aim - origin).try_normalize()?;
                        let credit_source = resolved
                            .credit_source()
                            .unwrap_or_else(|| resolved.canonical_weapon_name())
                            .to_string();
                        Some((
                            name.clone(),
                            resolved.cooldown_ms(),
                            AttackOutcome::Projectile {
                                launch: Box::new(ProjectileLaunch {
                                    origin,
                                    direction,
                                    speed: resolved.projectile().speed,
                                    radius: resolved.projectile().radius,
                                    range: resolved.range(),
                                    lifetime: resolved.projectile().lifetime_ms / 1000.0,
                                    damage: resolved.damage(),
                                    credit_source,
                                    descriptor: resolved.projectile().clone(),
                                }),
                                descriptor_class: resolved.canonical_weapon_name().to_string(),
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
                        Some((name.clone(), cooldown_ms, AttackOutcome::Contact { damage }))
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

        let state_changed = graph_reseated || transitioned || entered.is_some();
        let announce_entry = brain.take_entry_event_pending() && entered.is_some();
        let on_enter = if announce_entry {
            brain
                .active_depth()
                .checked_sub(1)
                .and_then(|depth| brain.activity_at_depth(depth))
                .and_then(|(_, activity)| activity.on_enter.clone())
        } else {
            None
        };
        let standoff_distance = programs
            .with_entry_scope(snap.id, |bound, scope| {
                action_for_path(bound, scope, &brain)
                    .map(|action| brain.graph.standoff_distance_for_action(Some(action)))
            })
            .flatten()
            .unwrap_or_else(|| brain.graph.standoff_distance_for_action(None));
        outcomes.push(EnemyOutcome {
            id: snap.id,
            position: snap.position,
            target,
            enemy_eye_offset,
            target_aim,
            prior_acquired_target,
            graph_reseated,
            state_changed,
            attack: attack_outcome,
            prior_standoff_distance,
            standoff_distance,
            on_enter,
            steering,
            engaged,
            facing_direction,
            combat_slot: None,
            brain,
        });
    }

    outcomes
}
