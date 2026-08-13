// Batch combat-slot allocation and incumbent hold tracking for engaged enemies.
// See: context/lib/entity_model.md §7c (enemy brain component)

use std::collections::HashMap;

use glam::Vec3;

use super::EnemyOutcome;
use crate::collision::CollisionWorld;
use crate::combat_positioning::{
    CombatAgentSnapshot, CombatCandidate, CombatQuery, PATH_LENGTH_SCORE_WEIGHT,
    select_combat_positions_batch,
};
use crate::nav::NavGraph;

/// How many ticks a resolved combat slot is held for its incumbent before the
/// batch solver is free to reassign it to a challenger.
pub(super) const COMBAT_SLOT_HOLD_TICKS: u32 = 8;

pub(super) fn resolve_combat_slots(
    outcomes: &mut [EnemyOutcome],
    nav_graph: Option<&NavGraph>,
    collision_world: Option<&CollisionWorld>,
) {
    for outcome in outcomes.iter_mut() {
        outcome.combat_slot = None;
        if !outcome.engaged {
            clear_combat_slot(outcome);
        }
    }

    let (Some(nav_graph), Some(collision_world)) = (nav_graph, collision_world) else {
        for outcome in outcomes.iter_mut() {
            clear_combat_slot(outcome);
        }
        return;
    };

    if !outcomes.iter().any(|outcome| outcome.engaged) {
        return;
    }

    let other_agents: Vec<CombatAgentSnapshot> = outcomes
        .iter()
        .map(|outcome| CombatAgentSnapshot {
            claimant_id: outcome.id.to_raw(),
            position: outcome.position,
        })
        .collect();

    let mut queries = Vec::new();
    for outcome in outcomes.iter() {
        if !outcome.engaged {
            continue;
        }
        let Some(target) = outcome.target else {
            continue;
        };
        let retained_slot = retained_combat_slot(outcome);
        queries.push(CombatQuery {
            claimant_id: outcome.id.to_raw(),
            agent_pos: outcome.position,
            engagement_radius: outcome.brain.graph.engagement_radius(),
            target_pos: target.position,
            combat_slot: retained_slot,
            scan_challengers: retained_slot.is_none(),
            other_agents: &other_agents,
            nav_graph,
            collision_world,
            path_length_score_weight: PATH_LENGTH_SCORE_WEIGHT,
        });
    }

    let assignments: HashMap<u32, Option<CombatCandidate>> =
        select_combat_positions_batch(&queries)
            .into_iter()
            .map(|assignment| (assignment.claimant_id, assignment.candidate))
            .collect();

    for outcome in outcomes.iter_mut() {
        if !outcome.engaged {
            clear_combat_slot(outcome);
            continue;
        }

        match assignments.get(&outcome.id.to_raw()).copied().flatten() {
            Some(candidate) => {
                outcome.combat_slot = Some(candidate.position);
                outcome.brain.combat_slot = Some(candidate.position);
                outcome.brain.combat_slot_hold_ticks = if candidate.is_incumbent {
                    outcome.brain.combat_slot_hold_ticks.saturating_sub(1)
                } else {
                    COMBAT_SLOT_HOLD_TICKS
                };
            }
            None => clear_combat_slot(outcome),
        }
    }
}

fn clear_combat_slot(outcome: &mut EnemyOutcome) {
    outcome.combat_slot = None;
    outcome.brain.combat_slot = None;
    outcome.brain.combat_slot_hold_ticks = 0;
}

/// The slot this enemy may re-present as an incumbent: the one it held while
/// already engaged with this same target, and only while its hold window is
/// open. Both slot fields are still the prior tick's here.
fn retained_combat_slot(outcome: &EnemyOutcome) -> Option<Vec3> {
    let target = outcome.target?;
    (outcome.engaged
        && outcome.prior_acquired_target == Some(target.entity)
        && outcome.brain.combat_slot_hold_ticks > 0)
        .then_some(outcome.brain.combat_slot)
        .flatten()
}
