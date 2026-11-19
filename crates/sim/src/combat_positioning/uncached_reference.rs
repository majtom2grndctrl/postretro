// Test-only reference: combat-slot scoring as it ran before the per-batch
// placement cache, grounding, testing and routing every candidate for every
// enemy. The batch run with this scorer is the uncached batch the live one
// must match.
// See: context/plans/in-progress/perf-ai-tick-cost (positioning cache)

use glam::Vec3;

use super::scoring::tactically_direct_path_length_limit;
use super::{
    CombatAssignment, CombatCandidate, CombatQuery, combat_candidates_with, dynamic_min_spacing,
    path_length, select_batch_with,
};
use crate::collision::{CapsulePlacement, capsule_static_placement_center, line_of_sight};
use crate::nav::{NavScratch, NavStart, distance_xz, find_path_from_in, find_path_gated_in};

/// The uncached batch.
pub(crate) fn select_combat_positions_batch_uncached(
    queries: &[CombatQuery<'_>],
    scratch: &mut NavScratch,
) -> Vec<CombatAssignment> {
    select_batch_with(queries, |query| {
        combat_candidates_with(
            query,
            &mut |query, placement, position, generation, incumbent| {
                score_candidate_uncached(scratch, query, placement, position, generation, incumbent)
            },
        )
    })
}

pub(super) fn score_candidate_uncached(
    scratch: &mut NavScratch,
    query: &CombatQuery<'_>,
    placement: CapsulePlacement,
    position: Vec3,
    generation_index: usize,
    is_incumbent: bool,
) -> Option<CombatCandidate> {
    if !position.is_finite() {
        return None;
    }

    let position = grounded_candidate_position(query, placement, position)?;
    if !dynamic_placement_is_clear(query, placement, position) {
        return None;
    }
    // Gated routes never gate regions, so only this keeps a slot out of a door.
    let capsule = crate::collision::CollisionCapsule::new(placement.radius, placement.half_height);
    if query.mover_nav.overlaps_docked_mover(position, capsule) {
        return None;
    }
    // Slot visibility is a positioning heuristic only. The following tick's
    // actual-position fire gate remains authoritative for damage.
    if !line_of_sight(
        position + query.enemy_eye_offset,
        query.target_aim,
        query.collision_world,
    ) {
        return None;
    }
    let gate = query.mover_nav.gate;
    let from = NavStart {
        position: query.agent_pos,
        ground: query.agent_ground,
        height: query.agent_height,
    };
    let path = find_path_from_in(scratch, query.nav_graph, gate, from, position)?;
    let target_distance = distance_xz(position, query.target_pos);
    if !has_tactically_direct_target_path(scratch, query, placement, position, target_distance) {
        return None;
    }
    let path_cost = path_length(&path);
    let attack_band_error = (target_distance - query.engagement_radius).abs();
    let score = attack_band_error + path_cost * query.path_length_score_weight;
    Some(CombatCandidate {
        position,
        score,
        attack_band_error,
        path_cost,
        generation_index,
        is_incumbent,
    })
}

fn has_tactically_direct_target_path(
    scratch: &mut NavScratch,
    query: &CombatQuery<'_>,
    placement: CapsulePlacement,
    position: Vec3,
    target_distance: f32,
) -> bool {
    let gate = query.mover_nav.gate;
    // The slot is a floor placement, never a mover top, so it starts on world
    // ground.
    let Some(target_path) =
        find_path_gated_in(scratch, query.nav_graph, gate, position, query.target_pos)
    else {
        // The funnel can conservatively refuse a route when a moving endpoint
        // grazes a clearance disk, even while its regions remain connected.
        // Keep a reachable slot in that narrow case so pursuit keeps moving;
        // a disconnected corral still has no shared component and is rejected.
        return query.nav_graph.endpoints_are_topologically_connected(
            gate,
            position,
            query.target_pos,
        );
    };

    // A local engagement may bend around one clearance disk at a nearby portal
    // endpoint. One capsule-clearance diameter (including the collision skin)
    // admits that bounded repair, but not a route that walks around a wall whose
    // far end is remote from the candidate-to-target engagement segment.
    let max_path_length = tactically_direct_path_length_limit(placement, target_distance);
    path_length(&target_path) <= max_path_length
}

fn grounded_candidate_position(
    query: &CombatQuery<'_>,
    placement: CapsulePlacement,
    position: Vec3,
) -> Option<Vec3> {
    let region_index = query.nav_graph.region_at(position)?;
    let region = query.nav_graph.region(region_index)?;
    let probe = Vec3::new(
        position.x,
        region.floor_y_max + placement.rest_offset(),
        position.z,
    );
    let grounded = capsule_static_placement_center(query.collision_world, probe, placement)?;
    (query.nav_graph.region_at(grounded) == Some(region_index)).then_some(grounded)
}

fn dynamic_placement_is_clear(
    query: &CombatQuery<'_>,
    placement: CapsulePlacement,
    position: Vec3,
) -> bool {
    let min_spacing = dynamic_min_spacing(placement);
    query.other_agents.iter().all(|other| {
        if other.claimant_id == query.claimant_id {
            return true;
        }
        if other.position.is_finite() && distance_xz(position, other.position) < min_spacing {
            return false;
        }
        true
    })
}
