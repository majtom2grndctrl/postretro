//! Candidate placement and scoring for combat slots.

use glam::Vec3;

use super::{CombatCandidate, CombatQuery, path_length};
use crate::collision::{
    CapsulePlacement, SKIN_DISTANCE, capsule_static_placement_center, line_of_sight,
};
use crate::nav::{NavGraph, distance_xz, find_path};

pub(super) fn capsule_placement(nav_graph: &NavGraph) -> Option<CapsulePlacement> {
    let params = nav_graph.agent_params();
    if !params.radius.is_finite()
        || !params.height.is_finite()
        || !params.step_height.is_finite()
        || params.radius <= 0.0
        || params.height < params.radius * 2.0
        || params.step_height <= 0.0
    {
        return None;
    }

    Some(CapsulePlacement {
        radius: params.radius,
        half_height: (params.height - params.radius * 2.0) * 0.5,
        step_height: params.step_height,
    })
}

pub(super) fn score_candidate(
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
    // Slot visibility is a positioning heuristic only. The following tick's
    // actual-position fire gate remains authoritative for damage.
    if !line_of_sight(
        position + query.enemy_eye_offset,
        query.target_aim,
        query.collision_world,
    ) {
        return None;
    }
    let path = find_path(query.nav_graph, query.agent_pos, position)?;
    let target_distance = distance_xz(position, query.target_pos);
    if !has_tactically_direct_target_path(query, placement, position, target_distance) {
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
    query: &CombatQuery<'_>,
    placement: CapsulePlacement,
    position: Vec3,
    target_distance: f32,
) -> bool {
    let Some(target_path) = find_path(query.nav_graph, position, query.target_pos) else {
        // The funnel can conservatively refuse a route when a moving endpoint
        // grazes a clearance disk, even while its regions remain connected.
        // Keep a reachable slot in that narrow case so pursuit keeps moving;
        // a disconnected corral still has no shared component and is rejected.
        return query
            .nav_graph
            .endpoints_are_topologically_connected(position, query.target_pos);
    };

    // A local engagement may bend around one clearance disk at a nearby portal
    // endpoint. One capsule-clearance diameter (including the collision skin)
    // admits that bounded repair, but not a route that walks around a wall whose
    // far end is remote from the candidate-to-target engagement segment.
    let max_path_length = tactically_direct_path_length_limit(placement, target_distance);
    path_length(&target_path) <= max_path_length
}

pub(super) fn tactically_direct_path_length_limit(
    placement: CapsulePlacement,
    target_distance: f32,
) -> f32 {
    target_distance + 2.0 * (placement.radius + SKIN_DISTANCE)
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

pub(super) fn dynamic_min_spacing(placement: CapsulePlacement) -> f32 {
    placement.radius * 2.0
}
