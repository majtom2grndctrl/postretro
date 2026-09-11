// Enemy target-pawn selection: nearest/retained candidate ranking with switch
// hysteresis, feeding the brain tick's per-enemy target.
// See: context/lib/entity_model.md §7c (enemy brain component)

use glam::Vec3;

use super::engine_floor::{is_meaningfully_closer, think_stride_for_distance};
use super::perception::RawTargetPerception;
use crate::nav::distance_xz;
use postretro_entities::ComponentKind;
use postretro_entities::components::brain::{
    BrainComponent, RECENT_ATTACKER_LEDGER_CAPACITY, RecentAttacker,
};
#[cfg(test)]
use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::player_movement::PlayerMovementComponent;
use postretro_entities::{
    EntityId, EntityRegistry, EntityStateComponent, LiveFactionSentiment, Transform,
};
use postretro_foundation::{BoundProgram, IrValue, RetaliationDescriptor, eval_value};

use super::candidate_scope::{CandidateFacts, CandidateRefreshContext, CandidateScope};

#[cfg(test)]
const EMPTY_RECENT_ATTACKERS: [Option<RecentAttacker>; RECENT_ATTACKER_LEDGER_CAPACITY] =
    [None; RECENT_ATTACKER_LEDGER_CAPACITY];

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TargetPawn {
    pub(crate) entity: EntityId,
    pub(crate) position: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TargetCandidate {
    pub(super) target: TargetPawn,
    pub(super) distance: f32,
}

/// This tick's selected pawn plus the raw LOS already computed when a fresh
/// candidate won the scan. Retained selections carry no fresh verdict and run
/// the normal per-tick perception query.
#[derive(Debug, Clone, Copy)]
pub(super) struct TargetSelection {
    pub(super) target: TargetPawn,
    pub(super) fresh_perception: Option<RawTargetPerception>,
    /// The target whose current retention is owned by the engine retaliation
    /// term. `None` leaves ordinary pure-distance retention untouched.
    pub(super) retaliation_acquired_target: Option<EntityId>,
}

/// Pure nearest sentiment-hostile candidate plus the stack-only scan context
/// needed to replay raw candidates on a due acquisition tick. The replay keeps
/// the hot path allocation-free; only `nearest` prices acquisition stride.
#[derive(Debug, Clone, Copy)]
pub(super) struct TargetOffers {
    pub(super) nearest: Option<TargetCandidate>,
    from: Vec3,
    exclude: Option<EntityId>,
}

/// A stable engine-owned retaliation preference. Its scalar score lets content
/// tune damage versus recency, while deterministic damage and distance
/// tiebreaks keep equal scores from depending on registry iteration order.
#[derive(Clone, Copy, Debug)]
struct RetaliationRank {
    score: f32,
    accumulated_damage: f32,
    distance: f32,
}

impl RetaliationRank {
    fn from_facts(facts: CandidateFacts, distance: f32, tuning: RetaliationDescriptor) -> Self {
        Self {
            score: facts.accumulated_damage * tuning.damage_weight
                - facts.time_since_damage_ms * tuning.recency_weight,
            accumulated_damage: facts.accumulated_damage,
            distance,
        }
    }

    fn is_preferred_to(self, other: Self) -> bool {
        self.score
            .total_cmp(&other.score)
            .then_with(|| self.accumulated_damage.total_cmp(&other.accumulated_damage))
            .then_with(|| other.distance.total_cmp(&self.distance))
            .is_gt()
    }

    fn exceeds_margin_over(self, held: Self) -> bool {
        self.score > held.score + RETALIATION_PREFERENCE_MARGIN
    }
}

/// A challenger must improve this engine-owned retaliation score strictly by
/// one whole preference point before it transfers a retained retaliation mark.
/// The fixed margin blocks two similar provokers from leapfrogging every tick.
const RETALIATION_PREFERENCE_MARGIN: f32 = 1.0;

/// Return the rank only when a candidate is presently eligible for retaliation.
/// `window_ms <= 0` and sub-tick windows short-circuit before the age check so
/// a same-tick age of zero can never accidentally activate the term.
fn retaliation_preference(
    facts: CandidateFacts,
    distance: f32,
    tuning: RetaliationDescriptor,
    tick_ms: f32,
) -> Option<RetaliationRank> {
    (tuning.window_ms > 0.0
        && tuning.window_ms >= tick_ms
        && facts.has_attacker_record
        && facts.time_since_damage_ms <= tuning.window_ms
        && facts.accumulated_damage > facts.tolerance)
        .then(|| RetaliationRank::from_facts(facts, distance, tuning))
}

pub(super) fn target_candidate(
    registry: &EntityRegistry,
    entity: EntityId,
    from: Vec3,
) -> Option<TargetCandidate> {
    let is_pawn = registry
        .get_component::<PlayerMovementComponent>(entity)
        .is_ok()
        || registry.get_component::<BrainComponent>(entity).is_ok();
    is_pawn.then_some(())?;
    let position = registry.get_component::<Transform>(entity).ok()?.position;
    Some(TargetCandidate {
        target: TargetPawn { entity, position },
        distance: distance_xz(position, from),
    })
}

/// Iterate targetable pawns without applying authored or engine-floor
/// eligibility. Player-movement holders come first, followed by brain-only
/// holders, matching the established deterministic scan order.
fn target_candidates(
    registry: &EntityRegistry,
    from: Vec3,
    evaluating_enemy: Option<EntityId>,
    exclude: Option<EntityId>,
) -> impl Iterator<Item = TargetCandidate> + '_ {
    let movement_holders = registry.iter_with_kind(ComponentKind::PlayerMovement);
    let brain_only_holders =
        registry
            .iter_with_kind(ComponentKind::Brain)
            .filter(move |(entity, _)| {
                registry
                    .get_component::<PlayerMovementComponent>(*entity)
                    .is_err()
            });
    movement_holders
        .chain(brain_only_holders)
        .filter_map(move |(entity, _)| {
            if evaluating_enemy == Some(entity) || exclude == Some(entity) {
                return None;
            }
            target_candidate(registry, entity, from)
        })
}

/// Find the pure nearest hostile target without retaining a heap-backed copy
/// of the candidate set. A due selection replays [`target_candidates`], so a
/// later retaliation admission can never alter this stride-price input.
pub(super) fn target_offers(
    registry: &EntityRegistry,
    factions: &LiveFactionSentiment<'_>,
    from: Vec3,
    enemy_faction: f32,
    evaluating_enemy: Option<EntityId>,
    exclude: Option<EntityId>,
) -> TargetOffers {
    let mut nearest = None;
    for candidate in target_candidates(registry, from, evaluating_enemy, exclude) {
        // Hostility alone contributes to the pure-distance stride price. The
        // non-hostile pawn remains in the raw scan so selection can admit it
        // only if its one refreshed candidate scope proves over tolerance.
        // Retained lookup stays above this scan and deliberately never re-gates
        // its target on hostility.
        let candidate_faction = registry
            .get_component::<EntityStateComponent>(candidate.target.entity)
            .map_or(0.0, |state| state.get(super::FACTION_STATE_FIELD));
        let hostile = is_hostile(factions, enemy_faction, candidate_faction);
        if hostile
            && nearest.is_none_or(|current: TargetCandidate| {
                candidate.distance.total_cmp(&current.distance).is_lt()
            })
        {
            nearest = Some(candidate);
        }
    }
    TargetOffers {
        nearest,
        from,
        exclude,
    }
}

/// The one directional hostility decision shared by offer filtering and the
/// durable brain fact. A strict negative value is hostile; zero remains
/// neutral and positive allied.
pub(super) fn is_hostile(
    factions: &LiveFactionSentiment<'_>,
    from_faction: f32,
    to_faction: f32,
) -> bool {
    factions.sentiment(from_faction, to_faction) < 0.0
}

pub(super) fn target_distance(target: TargetPawn, from: Vec3) -> f32 {
    distance_xz(target.position, from)
}

pub(super) fn acquisition_due(brain: &BrainComponent, distance: Option<f32>) -> bool {
    distance
        .map(|distance| {
            let stride = think_stride_for_distance(distance);
            stride <= 1 || brain.think_stride_counter.wrapping_add(1) % stride == 0
        })
        .unwrap_or(true)
}

pub(super) fn selected_target_alive(registry: &EntityRegistry, target: EntityId) -> bool {
    crate::scripting_systems::health::is_damage_target_eligible(registry, target)
}

/// Choose from a raw offer set on an acquisition tick. Authored candidacy and
/// engine-floor LOS both narrow fresh eligibility, while `offers.nearest` stays
/// untouched for stride pricing. The retained candidate is deliberately supplied
/// separately and never passes either fresh-acquisition gate.
#[cfg(test)]
pub(super) fn select_target(
    retained: Option<TargetCandidate>,
    offers: &TargetOffers,
    registry: &EntityRegistry,
    factions: &LiveFactionSentiment<'_>,
    evaluating_faction: f32,
    candidate_filter: Option<&BoundProgram<CandidateScope>>,
    candidate_scope: &mut CandidateScope,
    candidate_perception: &mut dyn FnMut(TargetPawn) -> Option<RawTargetPerception>,
) -> Option<TargetSelection> {
    select_target_with_attacker_ledger(
        retained,
        offers,
        registry,
        factions,
        None,
        evaluating_faction,
        candidate_filter,
        candidate_scope,
        &EMPTY_RECENT_ATTACKERS,
        None,
        RetaliationDescriptor::default(),
        1_000.0 / 60.0,
        candidate_perception,
    )
}

/// As [`select_target`], with the evaluating brain's immutable, already-aged
/// attacker ledger available to the per-candidate scope. The compatibility
/// wrapper above keeps focused target-ranking tests independent of ledger
/// setup; live AI evaluation always uses this entry point.
#[allow(clippy::too_many_arguments)]
pub(super) fn select_target_with_attacker_ledger(
    retained: Option<TargetCandidate>,
    offers: &TargetOffers,
    registry: &EntityRegistry,
    factions: &LiveFactionSentiment<'_>,
    evaluating_enemy: Option<EntityId>,
    evaluating_faction: f32,
    candidate_filter: Option<&BoundProgram<CandidateScope>>,
    candidate_scope: &mut CandidateScope,
    recent_attackers: &[Option<RecentAttacker>; RECENT_ATTACKER_LEDGER_CAPACITY],
    retaliation_acquired_target: Option<EntityId>,
    retaliation: RetaliationDescriptor,
    tick_ms: f32,
    candidate_perception: &mut dyn FnMut(TargetPawn) -> Option<RawTargetPerception>,
) -> Option<TargetSelection> {
    #[derive(Clone, Copy)]
    struct EligibleCandidate {
        candidate: TargetCandidate,
        perception: RawTargetPerception,
        retaliation: Option<RetaliationRank>,
    }

    let refresh_context = CandidateRefreshContext::new(
        registry,
        factions,
        evaluating_enemy,
        evaluating_faction,
        recent_attackers,
    );
    let mut nearest_distance_eligible: Option<EligibleCandidate> = None;
    let mut preferred_eligible: Option<EligibleCandidate> = None;
    for candidate in target_candidates(registry, offers.from, evaluating_enemy, offers.exclude) {
        let facts =
            candidate_scope.refresh(refresh_context, candidate.target.entity, candidate.distance);
        let retaliation_preference =
            retaliation_preference(facts, candidate.distance, retaliation, tick_ms);
        let hostile = facts.sentiment < 0.0;
        // This is the only non-hostile admission path. Candidate guards remain
        // narrowing-only and are deliberately not evaluated for an engine
        // candidate that did not first pass the offer floor.
        if !hostile && retaliation_preference.is_none() {
            continue;
        }
        let filter_allows = candidate_filter
            .is_none_or(|filter| eval_value(filter, candidate_scope) == IrValue::Bool(true));
        let Some(perception) = filter_allows
            .then(|| candidate_perception(candidate.target))
            .flatten()
            .filter(|perception| perception.visible)
        else {
            continue;
        };
        let eligible = EligibleCandidate {
            candidate,
            perception,
            retaliation: retaliation_preference,
        };
        if hostile
            && nearest_distance_eligible.is_none_or(|current| {
                candidate
                    .distance
                    .total_cmp(&current.candidate.distance)
                    .is_lt()
            })
        {
            nearest_distance_eligible = Some(eligible);
        }
        if preferred_eligible.is_none_or(|current| {
            match (eligible.retaliation, current.retaliation) {
                (Some(candidate_rank), Some(current_rank)) => {
                    candidate_rank.is_preferred_to(current_rank)
                }
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => candidate
                    .distance
                    .total_cmp(&current.candidate.distance)
                    .is_lt(),
            }
        }) {
            preferred_eligible = Some(eligible);
        }
    }

    let selection_for =
        |eligible: EligibleCandidate, retaliation_acquired_target| TargetSelection {
            target: eligible.candidate.target,
            fresh_perception: Some(eligible.perception),
            retaliation_acquired_target,
        };

    let retained_is_retaliation = retained
        .is_some_and(|retained| retaliation_acquired_target == Some(retained.target.entity));
    if let Some(retained) = retained {
        if retained_is_retaliation {
            let held_facts =
                candidate_scope.refresh(refresh_context, retained.target.entity, retained.distance);
            let held_rank = RetaliationRank::from_facts(held_facts, retained.distance, retaliation);
            if let Some(challenger) = preferred_eligible
                .filter(|candidate| candidate.retaliation.is_some())
                .filter(|candidate| {
                    candidate
                        .retaliation
                        .expect("filtered retaliation candidate")
                        .exceeds_margin_over(held_rank)
                })
            {
                return Some(selection_for(
                    challenger,
                    Some(challenger.candidate.target.entity),
                ));
            }
            return Some(TargetSelection {
                target: retained.target,
                fresh_perception: None,
                retaliation_acquired_target: Some(retained.target.entity),
            });
        }

        if let Some(preferred) = preferred_eligible {
            if preferred.retaliation.is_some()
                || (preferred.retaliation.is_none()
                    && is_meaningfully_closer(preferred.candidate.distance, retained.distance))
            {
                let retaliation_acquired_target = preferred
                    .retaliation
                    .map(|_| preferred.candidate.target.entity);
                return Some(selection_for(preferred, retaliation_acquired_target));
            }
        }
        return Some(TargetSelection {
            target: retained.target,
            fresh_perception: None,
            retaliation_acquired_target: None,
        });
    }

    preferred_eligible.map(|preferred| {
        // A fresh retaliation winner needs the latch only when the term changed
        // what pure hostile distance ranking would have selected. A nearest
        // hostile attacker already chosen by distance retains ordinary behavior.
        let retaliation_acquired_target = preferred.retaliation.and_then(|_| {
            (nearest_distance_eligible.map(|candidate| candidate.candidate.target.entity)
                != Some(preferred.candidate.target.entity))
            .then_some(preferred.candidate.target.entity)
        });
        selection_for(preferred, retaliation_acquired_target)
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use super::*;
    use crate::alloc_probe::AllocSnapshot;
    use postretro_entities::FactionRegistry;
    use postretro_entities::data_descriptors::{
        BehaviorActivityDescriptor, BehaviorGraphDescriptor, BehaviorGraphEnvelope,
    };
    use postretro_foundation::{
        AirParams, BRAIN_NO_TARGET_DISTANCE, CapsuleParams, FallParams, GroundParams,
        PlayerMovementDescriptor, SpeedParams,
    };

    fn movement() -> PlayerMovementComponent {
        PlayerMovementComponent::from_descriptor(&PlayerMovementDescriptor {
            capsule: CapsuleParams {
                radius: 0.35,
                half_height: 0.9,
                eye_height: 1.1,
            },
            ground: GroundParams {
                speed: SpeedParams {
                    walk: 4.0,
                    run: 6.0,
                    crouch: 2.0,
                },
                accel: 20.0,
                step_height: 0.4,
                max_slope: 45.0,
            },
            air: AirParams {
                forward_steer: 0.0,
                accel: 1.0,
                max_control_speed: 1.0,
                bunny_hop: false,
                jumps: 0,
                jump_velocity: 5.0,
                jump_ceiling: 0.0,
            },
            fall: FallParams {
                terminal_velocity: 40.0,
            },
            stuck_stop_enabled: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_ENABLED,
            stuck_stop_threshold: PlayerMovementDescriptor::DEFAULT_STUCK_STOP_THRESHOLD,
            dash: None,
            forgiveness: None,
            crouch: None,
            slide: None,
            view_feel: None,
        })
    }

    fn pawn(registry: &mut EntityRegistry, x: f32) -> EntityId {
        let entity = registry.spawn(Transform {
            position: Vec3::new(x, 0.0, 0.0),
            ..Transform::default()
        });
        registry.set_component(entity, movement()).unwrap();
        entity
    }

    fn brain(registry: &mut EntityRegistry, x: f32) -> EntityId {
        let entity = registry.spawn(Transform {
            position: Vec3::new(x, 0.0, 0.0),
            ..Transform::default()
        });
        let graph = BehaviorGraphDescriptor {
            envelope: BehaviorGraphEnvelope {
                initial: "idle".to_string(),
                activities: BTreeMap::from([(
                    "idle".to_string(),
                    BehaviorActivityDescriptor {
                        animation: None,
                        motion: None,
                        action: None,
                        on_enter: None,
                        layers: BTreeMap::new(),
                    },
                )]),
                transitions: BTreeMap::new(),
            },
            candidate_filter: None,
            retaliation: None,
            patrol: None,
            attacks: BTreeMap::new(),
            engagement_radius: None,
            move_speed: 0.0,
        };
        registry
            .set_component(entity, BrainComponent::from_graph(&graph))
            .expect("fresh enemy is live");
        entity
    }

    fn select_target_for_test(
        registry: &EntityRegistry,
        from: Vec3,
        enemy_faction: f32,
        retained_target: Option<EntityId>,
        candidate_filter: Option<&BoundProgram<CandidateScope>>,
        candidate_scope: &mut CandidateScope,
    ) -> (Option<TargetCandidate>, Option<TargetPawn>) {
        let factions = FactionRegistry::default();
        let retained = retained_target.and_then(|entity| target_candidate(registry, entity, from));
        let offers = target_offers(
            registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            from,
            enemy_faction,
            None,
            retained_target,
        );
        let mut candidate_perception = |target: TargetPawn| {
            Some(RawTargetPerception {
                target: target.entity,
                visible: true,
                enemy_eye: from,
                target_aim: target.position,
            })
        };
        let selected = select_target(
            retained,
            &offers,
            registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            enemy_faction,
            candidate_filter,
            candidate_scope,
            &mut candidate_perception,
        )
        .map(|selection| selection.target);
        (offers.nearest, selected)
    }

    #[allow(clippy::too_many_arguments)]
    fn select_with_retaliation_for_test(
        registry: &EntityRegistry,
        factions: &LiveFactionSentiment<'_>,
        evaluating_enemy: EntityId,
        enemy_faction: f32,
        retained_target: Option<EntityId>,
        recent_attackers: &[Option<RecentAttacker>; RECENT_ATTACKER_LEDGER_CAPACITY],
        retaliation_acquired_target: Option<EntityId>,
        retaliation: RetaliationDescriptor,
        tick_ms: f32,
    ) -> (Option<TargetCandidate>, Option<TargetSelection>) {
        let retained =
            retained_target.and_then(|entity| target_candidate(registry, entity, Vec3::ZERO));
        let offers = target_offers(
            registry,
            factions,
            Vec3::ZERO,
            enemy_faction,
            Some(evaluating_enemy),
            retained_target,
        );
        let nearest = offers.nearest;
        let mut candidate_perception = |target: TargetPawn| {
            Some(RawTargetPerception {
                target: target.entity,
                visible: true,
                enemy_eye: Vec3::ZERO,
                target_aim: target.position,
            })
        };
        let selection = select_target_with_attacker_ledger(
            retained,
            &offers,
            registry,
            factions,
            Some(evaluating_enemy),
            enemy_faction,
            None,
            &mut CandidateScope::for_validation(),
            recent_attackers,
            retaliation_acquired_target,
            retaliation,
            tick_ms,
            &mut candidate_perception,
        );
        (nearest, selection)
    }

    fn ledger_for(
        attacker: EntityId,
        accumulated_damage: f32,
        time_since_damage_ms: f32,
    ) -> [Option<RecentAttacker>; RECENT_ATTACKER_LEDGER_CAPACITY] {
        let mut ledger = EMPTY_RECENT_ATTACKERS;
        ledger[0] = Some(RecentAttacker {
            attacker,
            accumulated_damage,
            time_since_damage_ms,
        });
        ledger
    }

    fn retaliation_fixture() -> (EntityRegistry, EntityId, EntityId, EntityId) {
        let mut registry = EntityRegistry::new();
        let enemy = brain(&mut registry, 0.0);
        let player = pawn(&mut registry, 2.0);
        registry
            .entity_state_mut(player)
            .expect("player has state")
            .set(super::super::FACTION_STATE_FIELD, 1.0);
        let attacker = brain(&mut registry, 10.0);
        registry
            .entity_state_mut(enemy)
            .expect("enemy has state")
            .set(super::super::ARCHETYPE_TOLERANCE_STATE_FIELD, 5.0);
        (registry, enemy, player, attacker)
    }

    #[test]
    fn brain_holders_are_walked_but_default_faction_peers_remain_non_hostile() {
        let mut registry = EntityRegistry::new();
        let evaluating_enemy = brain(&mut registry, 0.0);
        let peer = brain(&mut registry, 4.0);
        let inert_prop = registry.spawn(Transform {
            position: Vec3::new(2.0, 0.0, 0.0),
            ..Transform::default()
        });
        registry
            .set_component(
                inert_prop,
                HealthComponent {
                    max: 10.0,
                    current: 10.0,
                    hitbox: None,
                    death_handled: false,
                    pending_kill_credit: None,
                    zone_multipliers: HashMap::new(),
                    contributor_ledger: Default::default(),
                },
            )
            .expect("fresh prop is live");

        assert!(target_candidate(&registry, peer, Vec3::ZERO).is_some());
        assert!(target_candidate(&registry, inert_prop, Vec3::ZERO).is_none());

        registry
            .entity_state_mut(peer)
            .expect("fresh peer has entity state")
            .set(super::super::FACTION_STATE_FIELD, 1.0);
        let factions = FactionRegistry::default();
        let hostile_offers = target_offers(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            Vec3::ZERO,
            0.0,
            Some(evaluating_enemy),
            None,
        );
        assert_eq!(
            target_candidates(&registry, Vec3::ZERO, Some(evaluating_enemy), None,)
                .map(|candidate| candidate.target.entity)
                .collect::<Vec<_>>(),
            vec![peer],
            "the brain-bearing peer is included while the evaluating enemy and inert prop are not",
        );
        assert_eq!(
            hostile_offers
                .nearest
                .map(|candidate| candidate.target.entity),
            Some(peer),
            "the cross-faction peer prices the hostile stride",
        );

        registry
            .entity_state_mut(peer)
            .expect("fresh peer has entity state")
            .set(super::super::FACTION_STATE_FIELD, 0.0);
        let default_faction_offers = target_offers(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            Vec3::ZERO,
            0.0,
            Some(evaluating_enemy),
            None,
        );
        assert_eq!(
            target_candidates(&registry, Vec3::ZERO, Some(evaluating_enemy), None,)
                .map(|candidate| candidate.target.entity)
                .collect::<Vec<_>>(),
            vec![peer],
            "same-default-faction brain peers stay available only for the narrow retaliation scan",
        );
        assert!(
            default_faction_offers.nearest.is_none(),
            "a same-default-faction peer remains absent from the pure hostile stride price",
        );
    }

    #[test]
    fn directional_sentiment_controls_offers_while_defaults_preserve_faction_inequality() {
        use postretro_entities::{FactionDescriptor, FactionSentimentDescriptor};

        let factions = FactionRegistry::from_descriptors(vec![
            FactionDescriptor {
                name: "cabal".to_string(),
            },
            FactionDescriptor {
                name: "resistance".to_string(),
            },
        ])
        .expect("valid factions")
        .with_sentiments(vec![
            FactionSentimentDescriptor {
                from_faction: "cabal".to_string(),
                to_faction: "resistance".to_string(),
                sentiment: -1.0,
                tolerance: 1.0,
                decay: None,
            },
            FactionSentimentDescriptor {
                from_faction: "resistance".to_string(),
                to_faction: "cabal".to_string(),
                sentiment: 0.0,
                tolerance: 1.0,
                decay: None,
            },
            FactionSentimentDescriptor {
                from_faction: "cabal".to_string(),
                to_faction: "cabal".to_string(),
                sentiment: -0.5,
                tolerance: 1.0,
                decay: None,
            },
        ])
        .expect("directed entries resolve");
        let mut registry = EntityRegistry::new();
        let resistance = pawn(&mut registry, 4.0);
        registry
            .entity_state_mut(resistance)
            .expect("pawn has state")
            .set(super::super::FACTION_STATE_FIELD, 3.0);

        let cabal_offers = target_offers(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            Vec3::ZERO,
            2.0,
            None,
            None,
        );
        assert_eq!(
            cabal_offers
                .nearest
                .map(|candidate| candidate.target.entity),
            Some(resistance),
            "cabal's negative sentiment toward resistance offers it"
        );
        registry
            .entity_state_mut(resistance)
            .expect("pawn remains live")
            .set(super::super::FACTION_STATE_FIELD, 2.0);
        let resistance_offers = target_offers(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            Vec3::ZERO,
            3.0,
            None,
            None,
        );
        assert_eq!(
            target_candidates(&registry, Vec3::ZERO, None, None)
                .map(|candidate| candidate.target.entity)
                .collect::<Vec<_>>(),
            vec![resistance],
            "neutral candidates remain available only for the narrow retaliation scan"
        );
        assert!(
            resistance_offers.nearest.is_none(),
            "neutral sentiment cannot price the pure hostile stride"
        );
        assert_eq!(
            target_offers(
                &registry,
                &LiveFactionSentiment::with_empty_overlay(&factions),
                Vec3::ZERO,
                2.0,
                None,
                None
            )
            .nearest
            .map(|candidate| candidate.target.entity),
            Some(resistance),
            "an authored same-faction negative sentiment overrides the neutral default"
        );
        assert!(
            is_hostile(
                &LiveFactionSentiment::with_empty_overlay(&FactionRegistry::default()),
                1.0,
                0.0
            ) && !is_hostile(
                &LiveFactionSentiment::with_empty_overlay(&FactionRegistry::default()),
                1.0,
                1.0
            ),
            "unlisted pairs retain cross-faction hostile and same-faction neutral defaults"
        );
    }

    #[test]
    fn fresh_selection_carries_the_candidate_los_result_but_retention_does_not() {
        let mut registry = EntityRegistry::new();
        let pawn = pawn(&mut registry, 4.0);
        let factions = FactionRegistry::default();
        let offers = target_offers(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            Vec3::ZERO,
            1.0,
            None,
            None,
        );
        let expected = RawTargetPerception {
            target: pawn,
            visible: true,
            enemy_eye: Vec3::new(0.0, 1.0, 0.0),
            target_aim: Vec3::new(4.0, 1.1, 0.0),
        };
        let mut calls = 0;
        let mut candidate_perception = |_: TargetPawn| {
            calls += 1;
            Some(expected)
        };

        let selected = select_target(
            None,
            &offers,
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            1.0,
            None,
            &mut CandidateScope::for_validation(),
            &mut candidate_perception,
        )
        .expect("fresh target");

        assert_eq!(selected.target.entity, pawn);
        assert_eq!(selected.fresh_perception, Some(expected));

        let retained = target_candidate(&registry, pawn, Vec3::ZERO).expect("retained target");
        let empty_offers = target_offers(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            Vec3::ZERO,
            1.0,
            None,
            Some(pawn),
        );
        let retained_selection = select_target(
            Some(retained),
            &empty_offers,
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            1.0,
            None,
            &mut CandidateScope::for_validation(),
            &mut candidate_perception,
        )
        .expect("retained target");
        assert_eq!(calls, 1, "retention does not evaluate fresh-candidate LOS");
        assert_eq!(retained_selection.target.entity, pawn);
        assert_eq!(retained_selection.fresh_perception, None);
    }

    #[test]
    fn retained_due_switch_carries_the_challengers_los_result() {
        let mut registry = EntityRegistry::new();
        let retained_entity = pawn(&mut registry, 10.0);
        let challenger = pawn(&mut registry, 2.0);
        let retained =
            target_candidate(&registry, retained_entity, Vec3::ZERO).expect("retained target");
        let factions = FactionRegistry::default();
        let offers = target_offers(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            Vec3::ZERO,
            1.0,
            None,
            Some(retained_entity),
        );
        let mut candidate_perception = |target: TargetPawn| {
            Some(RawTargetPerception {
                target: target.entity,
                visible: true,
                enemy_eye: Vec3::Y,
                target_aim: target.position + Vec3::Y,
            })
        };

        let selected = select_target(
            Some(retained),
            &offers,
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            1.0,
            None,
            &mut CandidateScope::for_validation(),
            &mut candidate_perception,
        )
        .expect("closer fresh challenger");

        assert_eq!(selected.target.entity, challenger);
        assert_eq!(
            selected
                .fresh_perception
                .map(|perception| perception.target),
            Some(challenger),
            "the retained-due switch reuses the challenger's acquisition ray",
        );
    }

    #[test]
    fn selection_keeps_retained_target_until_a_fresh_candidate_beats_hysteresis() {
        let mut registry = EntityRegistry::new();
        let retained = pawn(&mut registry, 10.0);
        let near_but_not_meaningfully_closer = pawn(&mut registry, 9.5);
        let (_, selected) = select_target_for_test(
            &registry,
            Vec3::ZERO,
            1.0,
            Some(retained),
            None,
            &mut CandidateScope::for_validation(),
        );
        assert_eq!(selected.map(|target| target.entity), Some(retained));

        registry
            .set_component(
                near_but_not_meaningfully_closer,
                Transform {
                    position: Vec3::new(8.0, 0.0, 0.0),
                    ..Transform::default()
                },
            )
            .unwrap();
        let (_, selected) = select_target_for_test(
            &registry,
            Vec3::ZERO,
            1.0,
            Some(retained),
            None,
            &mut CandidateScope::for_validation(),
        );
        assert_eq!(
            selected.map(|target| target.entity),
            Some(near_but_not_meaningfully_closer)
        );
    }

    #[test]
    fn fresh_acquisition_skips_friendlies_so_they_do_not_mask_hostiles() {
        let mut registry = EntityRegistry::new();
        let friendly = pawn(&mut registry, 2.0);
        let hostile = pawn(&mut registry, 5.0);
        registry
            .entity_state_mut(friendly)
            .unwrap()
            .set(super::super::FACTION_STATE_FIELD, 1.0);

        let (nearest_for_stride, selected) = select_target_for_test(
            &registry,
            Vec3::ZERO,
            1.0,
            None,
            None,
            &mut CandidateScope::for_validation(),
        );
        assert_eq!(
            selected.map(|target| target.entity),
            Some(hostile),
            "a nearer friendly cannot mask a farther hostile candidate"
        );
        assert_eq!(
            nearest_for_stride.map(|candidate| candidate.target.entity),
            Some(hostile),
            "a friendly is not an offered candidate and cannot price the stride"
        );

        registry
            .entity_state_mut(hostile)
            .unwrap()
            .set(super::super::FACTION_STATE_FIELD, 1.0);
        let (_, selected) = select_target_for_test(
            &registry,
            Vec3::ZERO,
            1.0,
            None,
            None,
            &mut CandidateScope::for_validation(),
        );
        assert!(selected.is_none(), "a friendly is never freshly acquired");
    }

    #[test]
    fn retained_target_stays_selected_after_its_faction_turns_friendly() {
        let mut registry = EntityRegistry::new();
        let retained = pawn(&mut registry, 2.0);
        registry
            .entity_state_mut(retained)
            .unwrap()
            .set(super::super::FACTION_STATE_FIELD, 1.0);

        let (_, selected) = select_target_for_test(
            &registry,
            Vec3::ZERO,
            1.0,
            Some(retained),
            None,
            &mut CandidateScope::for_validation(),
        );
        assert_eq!(
            selected.map(|target| target.entity),
            Some(retained),
            "retention deliberately bypasses the fresh-acquisition hostility filter"
        );
    }

    #[test]
    fn over_tolerance_same_and_cross_faction_attackers_beat_nearer_hostile_without_pricing_stride()
    {
        for attacker_faction in [0.0, 2.0] {
            let (mut registry, enemy, player, attacker) = retaliation_fixture();
            registry
                .entity_state_mut(attacker)
                .expect("attacker has state")
                .set(super::super::FACTION_STATE_FIELD, attacker_faction);
            let factions = FactionRegistry::default();
            let ledger = ledger_for(attacker, 6.0, 0.0);
            let (nearest, selected) = select_with_retaliation_for_test(
                &registry,
                &LiveFactionSentiment::with_empty_overlay(&factions),
                enemy,
                0.0,
                None,
                &ledger,
                None,
                RetaliationDescriptor::default(),
                16.0,
            );

            assert_eq!(
                nearest.map(|candidate| candidate.target.entity),
                Some(player),
                "the far retaliation attacker must never price offers.nearest"
            );
            let selected = selected.expect("over-tolerance attacker is offered");
            assert_eq!(selected.target.entity, attacker);
            assert_eq!(selected.retaliation_acquired_target, Some(attacker));
        }
    }

    // Regression: a legal negative tolerance plus the missing-attacker
    // zero/sentinel fact defaults admitted neutral pawns that never dealt damage.
    #[test]
    fn negative_tolerance_requires_an_actual_attacker_record_for_retaliation_admission() {
        let (mut registry, enemy, player, non_attacker) = retaliation_fixture();
        registry
            .entity_state_mut(enemy)
            .expect("enemy has state")
            .set(super::super::ARCHETYPE_TOLERANCE_STATE_FIELD, -1.0);
        registry
            .set_component(
                non_attacker,
                Transform {
                    position: Vec3::X,
                    ..Transform::default()
                },
            )
            .expect("non-attacker remains live");

        let selected = select_with_retaliation_for_test(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&FactionRegistry::default()),
            enemy,
            0.0,
            None,
            &EMPTY_RECENT_ATTACKERS,
            None,
            RetaliationDescriptor {
                window_ms: BRAIN_NO_TARGET_DISTANCE,
                recency_weight: 0.0,
                ..RetaliationDescriptor::default()
            },
            16.0,
        )
        .1
        .expect("the hostile player remains eligible");

        assert_eq!(selected.target.entity, player);
        assert_eq!(selected.retaliation_acquired_target, None);

        let selected = select_with_retaliation_for_test(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&FactionRegistry::default()),
            enemy,
            0.0,
            None,
            &ledger_for(non_attacker, 0.25, 0.0),
            None,
            RetaliationDescriptor {
                window_ms: BRAIN_NO_TARGET_DISTANCE,
                recency_weight: 0.0,
                ..RetaliationDescriptor::default()
            },
            16.0,
        )
        .1
        .expect("a real low-damage attacker clears the authored negative tolerance");
        assert_eq!(selected.target.entity, non_attacker);
        assert_eq!(selected.retaliation_acquired_target, Some(non_attacker));
    }

    #[test]
    fn complete_due_acquisition_scan_performs_zero_heap_allocations() {
        let mut registry = EntityRegistry::new();
        let enemy = brain(&mut registry, 0.0);
        let player = pawn(&mut registry, 2.0);
        registry
            .entity_state_mut(player)
            .expect("player has state")
            .set(super::super::FACTION_STATE_FIELD, 1.0);
        let peer = brain(&mut registry, 4.0);
        let factions = FactionRegistry::default();
        let mut candidate_scope = CandidateScope::for_validation();
        let mut candidate_perception = |target: TargetPawn| {
            Some(RawTargetPerception {
                target: target.entity,
                visible: true,
                enemy_eye: Vec3::ZERO,
                target_aim: target.position,
            })
        };

        // Warm the exact path before measuring so the probe covers one normal
        // due scan rather than one-time test/TLS initialization.
        let warm_offers = target_offers(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            Vec3::ZERO,
            0.0,
            Some(enemy),
            None,
        );
        let _ = select_target_with_attacker_ledger(
            None,
            &warm_offers,
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            Some(enemy),
            0.0,
            None,
            &mut candidate_scope,
            &EMPTY_RECENT_ATTACKERS,
            None,
            RetaliationDescriptor::default(),
            16.0,
            &mut candidate_perception,
        );

        let snapshot = AllocSnapshot::arm();
        let offers = target_offers(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            Vec3::ZERO,
            0.0,
            Some(enemy),
            None,
        );
        let selected = select_target_with_attacker_ledger(
            None,
            &offers,
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            Some(enemy),
            0.0,
            None,
            &mut candidate_scope,
            &EMPTY_RECENT_ATTACKERS,
            None,
            RetaliationDescriptor::default(),
            16.0,
            &mut candidate_perception,
        );
        let allocations = snapshot.allocs_since();

        assert_eq!(
            offers.nearest.map(|candidate| candidate.target.entity),
            Some(player)
        );
        assert_eq!(
            selected.map(|selection| selection.target.entity),
            Some(player)
        );
        assert_ne!(
            peer, player,
            "fixture includes a brain-only peer in the replay"
        );
        assert_eq!(allocations, 0, "a due target-acquisition scan allocated");
    }

    #[test]
    fn max_default_tolerance_keeps_pure_distance_selection() {
        let (registry, enemy, _player, attacker) = retaliation_fixture();
        let factions = FactionRegistry::default();
        let ledger = ledger_for(attacker, 50.0, 0.0);
        let selected = select_with_retaliation_for_test(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            enemy,
            0.0,
            None,
            &ledger,
            None,
            RetaliationDescriptor::default(),
            16.0,
        )
        .1
        .expect("near hostile player is selected");
        assert_eq!(selected.target.entity, attacker, "fixture lowers tolerance");

        let mut max_tolerance_registry = EntityRegistry::new();
        let max_enemy = brain(&mut max_tolerance_registry, 0.0);
        let max_player = pawn(&mut max_tolerance_registry, 2.0);
        max_tolerance_registry
            .entity_state_mut(max_player)
            .expect("player has state")
            .set(super::super::FACTION_STATE_FIELD, 1.0);
        let max_attacker = brain(&mut max_tolerance_registry, 10.0);
        let max_ledger = ledger_for(max_attacker, 50.0, 0.0);
        let selected = select_with_retaliation_for_test(
            &max_tolerance_registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            max_enemy,
            0.0,
            None,
            &max_ledger,
            None,
            RetaliationDescriptor::default(),
            16.0,
        )
        .1
        .expect("near hostile player is selected");
        assert_eq!(selected.target.entity, max_player);
        assert_eq!(selected.retaliation_acquired_target, None);
    }

    #[test]
    fn retaliation_latch_holds_after_decay_and_transfers_only_past_margin() {
        let (mut registry, enemy, player, attacker_a) = retaliation_fixture();
        let attacker_b = brain(&mut registry, 12.0);
        let factions = FactionRegistry::default();
        let mut ledger = ledger_for(attacker_a, 10.0, 0.0);
        ledger[1] = Some(RecentAttacker {
            attacker: attacker_b,
            accumulated_damage: 10.0,
            time_since_damage_ms: 0.0,
        });
        let initial = select_with_retaliation_for_test(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            enemy,
            0.0,
            None,
            &ledger,
            None,
            RetaliationDescriptor::default(),
            16.0,
        )
        .1
        .expect("first attacker wins deterministic tie by distance");
        assert_eq!(initial.target.entity, attacker_a);
        assert_eq!(initial.retaliation_acquired_target, Some(attacker_a));

        ledger[1]
            .as_mut()
            .expect("attacker b ledger entry")
            .accumulated_damage = 11.0;
        let held_at_margin = select_with_retaliation_for_test(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            enemy,
            0.0,
            Some(attacker_a),
            &ledger,
            Some(attacker_a),
            RetaliationDescriptor::default(),
            16.0,
        )
        .1
        .expect("an exactly-one-point challenger does not transfer the latch");
        assert_eq!(held_at_margin.target.entity, attacker_a);
        assert_eq!(held_at_margin.retaliation_acquired_target, Some(attacker_a));

        ledger[1]
            .as_mut()
            .expect("attacker b ledger entry")
            .accumulated_damage = 12.1;
        let transferred = select_with_retaliation_for_test(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            enemy,
            0.0,
            Some(attacker_a),
            &ledger,
            Some(attacker_a),
            RetaliationDescriptor::default(),
            16.0,
        )
        .1
        .expect("strong enough challenger transfers latch");
        assert_eq!(transferred.target.entity, attacker_b);
        assert_eq!(transferred.retaliation_acquired_target, Some(attacker_b));

        let decayed_ledger = ledger_for(attacker_a, 10.0, 2_000.0);
        let held_after_decay = select_with_retaliation_for_test(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            enemy,
            0.0,
            Some(attacker_a),
            &decayed_ledger,
            Some(attacker_a),
            RetaliationDescriptor::default(),
            16.0,
        )
        .1
        .expect("retaliation latch persists while its ledger entry decays");
        assert_eq!(held_after_decay.target.entity, attacker_a);
        assert_eq!(
            held_after_decay.retaliation_acquired_target,
            Some(attacker_a),
            "ledger decay and the nearer player do not clear the latch",
        );
        assert_ne!(held_after_decay.target.entity, player);
    }

    #[test]
    fn zero_or_sub_tick_retaliation_window_is_explicitly_inert() {
        for window_ms in [0.0, 15.9] {
            let (registry, enemy, player, attacker) = retaliation_fixture();
            let factions = FactionRegistry::default();
            let selected = select_with_retaliation_for_test(
                &registry,
                &LiveFactionSentiment::with_empty_overlay(&factions),
                enemy,
                0.0,
                None,
                &ledger_for(attacker, 50.0, 0.0),
                None,
                RetaliationDescriptor {
                    window_ms,
                    ..RetaliationDescriptor::default()
                },
                16.0,
            )
            .1
            .expect("near hostile player is selected");
            assert_eq!(
                selected.target.entity, player,
                "window {window_ms} must disable retaliation"
            );
            assert_eq!(selected.retaliation_acquired_target, None);
        }
    }

    #[test]
    fn stale_retaliation_mark_clears_when_retained_target_despawns() {
        let (mut registry, enemy, player, attacker) = retaliation_fixture();
        registry.despawn(attacker).expect("attacker despawns");
        let factions = FactionRegistry::default();
        let selected = select_with_retaliation_for_test(
            &registry,
            &LiveFactionSentiment::with_empty_overlay(&factions),
            enemy,
            0.0,
            None,
            &EMPTY_RECENT_ATTACKERS,
            Some(attacker),
            RetaliationDescriptor::default(),
            16.0,
        )
        .1
        .expect("fresh scan selects player");
        assert_eq!(selected.target.entity, player);
        assert_eq!(selected.retaliation_acquired_target, None);
    }
}
