//! Candidate placement and scoring for combat slots.
//!
//! See: context/lib/entity_model.md §7c.

use glam::Vec3;

use super::{CombatCandidate, CombatQuery, path_length};
use crate::collision::{CapsulePlacement, SKIN_DISTANCE, capsule_static_placement_center};
use crate::nav::{
    NavGraph, NavScratch, NavStart, distance_xz, find_path_from_in, find_path_gated_in,
};

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

/// Where a candidate's placement work comes from. [`Direct`] computes it for
/// the one query; a batch shares the target-side half across its enemies
/// (`placement_cache`). Every source returns what [`Direct`] returns.
pub(super) trait PlacementSource {
    /// The candidate grounded on its region's floor, if it stays there.
    fn grounded(
        &mut self,
        query: &CombatQuery<'_>,
        placement: CapsulePlacement,
        position: Vec3,
    ) -> Option<Vec3>;

    /// No other agent stands within spacing of the grounded slot.
    fn clear(
        &mut self,
        query: &CombatQuery<'_>,
        placement: CapsulePlacement,
        position: Vec3,
    ) -> bool;

    /// A docked mover overlaps a capsule at the grounded slot.
    fn overlaps_docked_mover(
        &mut self,
        query: &CombatQuery<'_>,
        placement: CapsulePlacement,
        position: Vec3,
    ) -> bool;

    /// The slot reaches the target by a route about as short as the line.
    fn tactically_direct(
        &mut self,
        scratch: &mut NavScratch,
        query: &CombatQuery<'_>,
        placement: CapsulePlacement,
        position: Vec3,
        target_distance: f32,
    ) -> bool;
}

/// Placement work computed for one query alone.
pub(super) struct Direct;

impl PlacementSource for Direct {
    fn grounded(
        &mut self,
        query: &CombatQuery<'_>,
        placement: CapsulePlacement,
        position: Vec3,
    ) -> Option<Vec3> {
        grounded_candidate_position(query, placement, position)
    }

    fn clear(
        &mut self,
        query: &CombatQuery<'_>,
        placement: CapsulePlacement,
        position: Vec3,
    ) -> bool {
        dynamic_placement_is_clear(query, placement, position)
    }

    fn overlaps_docked_mover(
        &mut self,
        query: &CombatQuery<'_>,
        placement: CapsulePlacement,
        position: Vec3,
    ) -> bool {
        overlaps_docked_mover(query, placement, position)
    }

    fn tactically_direct(
        &mut self,
        scratch: &mut NavScratch,
        query: &CombatQuery<'_>,
        placement: CapsulePlacement,
        position: Vec3,
        target_distance: f32,
    ) -> bool {
        has_tactically_direct_target_path(scratch, query, placement, position, target_distance)
    }
}

pub(super) fn score_candidate(
    scratch: &mut NavScratch,
    query: &CombatQuery<'_>,
    placement: CapsulePlacement,
    position: Vec3,
    generation_index: usize,
    is_incumbent: bool,
) -> Option<CombatCandidate> {
    score_candidate_with(
        &mut Direct,
        scratch,
        query,
        placement,
        position,
        generation_index,
        is_incumbent,
    )
}

/// Score one candidate, taking its placement work from `source`. Every
/// check is pure, so where a source gets an answer cannot change it.
pub(super) fn score_candidate_with(
    source: &mut impl PlacementSource,
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

    let position = source.grounded(query, placement, position)?;
    if !source.clear(query, placement, position) {
        return None;
    }
    // Gated routes never gate regions, so only this keeps a slot out of a door.
    if source.overlaps_docked_mover(query, placement, position) {
        return None;
    }
    // Slot visibility is a positioning heuristic only. The following tick's
    // actual-position fire gate remains authoritative for damage. It casts
    // from this enemy's own eye, so it is never shared.
    if !query.slot_sees_target(position + query.enemy_eye_offset) {
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
    if !source.tactically_direct(scratch, query, placement, position, target_distance) {
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

/// Placement work done at the scoring seam on this thread, for the proofs
/// that a batch shares it.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PlacementWork {
    pub(crate) groundings: usize,
    pub(crate) dock_tests: usize,
    pub(crate) target_routes: usize,
    pub(crate) peers_examined: usize,
    pub(crate) clearance_tests: usize,
    /// The most agents one clearance test examined.
    pub(crate) most_peers_per_test: usize,
}

#[cfg(test)]
thread_local! {
    pub(crate) static PLACEMENT_WORK: std::cell::Cell<PlacementWork> =
        const {
            std::cell::Cell::new(PlacementWork {
                groundings: 0,
                dock_tests: 0,
                target_routes: 0,
                peers_examined: 0,
                clearance_tests: 0,
                most_peers_per_test: 0,
            })
        };
}

/// Record one clearance test that examined `peers` agents.
#[cfg(test)]
pub(super) fn count_clearance(peers: usize) {
    count_work(|work| {
        work.clearance_tests += 1;
        work.peers_examined += peers;
        work.most_peers_per_test = work.most_peers_per_test.max(peers);
    });
}

#[cfg(test)]
pub(super) fn count_work(update: impl FnOnce(&mut PlacementWork)) {
    PLACEMENT_WORK.with(|work| {
        let mut value = work.get();
        update(&mut value);
        work.set(value);
    });
}

fn overlaps_docked_mover(
    query: &CombatQuery<'_>,
    placement: CapsulePlacement,
    position: Vec3,
) -> bool {
    #[cfg(test)]
    count_work(|work| work.dock_tests += 1);
    let capsule = crate::collision::CollisionCapsule::new(placement.radius, placement.half_height);
    query.mover_nav.overlaps_docked_mover(position, capsule)
}

fn has_tactically_direct_target_path(
    scratch: &mut NavScratch,
    query: &CombatQuery<'_>,
    placement: CapsulePlacement,
    position: Vec3,
    target_distance: f32,
) -> bool {
    #[cfg(test)]
    count_work(|work| work.target_routes += 1);
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
    #[cfg(test)]
    count_work(|work| work.groundings += 1);
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
    #[cfg(test)]
    let mut examined = 0;
    let clear = query.other_agents.iter().all(|other| {
        #[cfg(test)]
        {
            examined += 1;
        }
        peer_is_clear(query, other, position, min_spacing)
    });
    #[cfg(test)]
    count_clearance(examined);
    clear
}

/// The spacing rule against one other agent: never against the claimant
/// itself or a non-finite position.
pub(super) fn peer_is_clear(
    query: &CombatQuery<'_>,
    other: &super::CombatAgentSnapshot,
    position: Vec3,
    min_spacing: f32,
) -> bool {
    other.claimant_id == query.claimant_id
        || !(other.position.is_finite() && distance_xz(position, other.position) < min_spacing)
}

pub(super) fn dynamic_min_spacing(placement: CapsulePlacement) -> f32 {
    placement.radius * 2.0
}
