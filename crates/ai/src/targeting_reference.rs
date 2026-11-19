// Test-only reference: enemy target selection as it ran before the
// faction-first pawn index, walking every pawn per enemy. The live selection
// must return what this returns.
// See: context/plans/in-progress/perf-ai-tick-cost (pawn index)

use super::*;
use postretro_entities::{ComponentKind, EntityStateComponent};

/// Iterate targetable pawns without applying authored or engine-floor
/// eligibility. Player-movement holders come first, followed by brain-only
/// holders, matching the established deterministic scan order.
pub(crate) fn target_candidates(
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
pub(crate) fn target_offers(
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
            .map_or(0.0, |state| state.get(crate::FACTION_STATE_FIELD));
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

/// As [`select_target`], with the evaluating brain's immutable, already-aged
/// attacker ledger available to the per-candidate scope. The compatibility
/// wrapper above keeps focused target-ranking tests independent of ledger
/// setup; live AI evaluation always uses this entry point.
#[allow(clippy::too_many_arguments)]
pub(crate) fn select_target_with_attacker_ledger(
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

        if let Some(preferred) = preferred_eligible
            && (preferred.retaliation.is_some()
                || (preferred.retaliation.is_none()
                    && is_meaningfully_closer(preferred.candidate.distance, retained.distance)))
        {
            let retaliation_acquired_target = preferred
                .retaliation
                .map(|_| preferred.candidate.target.entity);
            return Some(selection_for(preferred, retaliation_acquired_target));
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
