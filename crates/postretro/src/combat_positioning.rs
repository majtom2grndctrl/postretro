//! Pure combat-position candidate selection.
//!
//! This module is deliberately decoupled from AI steering state. Callers pass a
//! target position directly, plus the nav/collision query surfaces required to
//! prove each candidate is reachable and statically occupiable.

use std::cmp::Ordering;

use glam::Vec3;

use crate::collision::{CapsulePlacement, CollisionWorld, capsule_static_placement_center};
use crate::nav::{NavGraph, distance_xz, find_path};

const RING_DIRECTIONS: [Vec3; 8] = [
    Vec3::new(1.0, 0.0, 0.0),
    Vec3::new(0.70710677, 0.0, 0.70710677),
    Vec3::new(0.0, 0.0, 1.0),
    Vec3::new(-0.70710677, 0.0, 0.70710677),
    Vec3::new(-1.0, 0.0, 0.0),
    Vec3::new(-0.70710677, 0.0, -0.70710677),
    Vec3::new(0.0, 0.0, -1.0),
    Vec3::new(0.70710677, 0.0, -0.70710677),
];
const RADIAL_MULTIPLIERS: [f32; 3] = [1.0, 0.75, 1.25];
pub(crate) const PATH_LENGTH_SCORE_WEIGHT: f32 = 0.05;
pub(crate) const COMBAT_SLOT_SWITCH_MARGIN: f32 = 1.0;

#[derive(Debug, Clone, Copy)]
pub(crate) struct CombatQuery<'a> {
    pub(crate) claimant_id: u32,
    pub(crate) agent_pos: Vec3,
    pub(crate) engagement_radius: f32,
    pub(crate) target_pos: Vec3,
    pub(crate) combat_slot: Option<Vec3>,
    pub(crate) scan_challengers: bool,
    pub(crate) other_agents: &'a [CombatAgentSnapshot],
    pub(crate) nav_graph: &'a NavGraph,
    pub(crate) collision_world: &'a CollisionWorld,
    pub(crate) path_length_score_weight: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CombatAgentSnapshot {
    pub(crate) claimant_id: u32,
    pub(crate) position: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CombatCandidate {
    pub(crate) position: Vec3,
    pub(crate) score: f32,
    pub(crate) attack_band_error: f32,
    pub(crate) path_cost: f32,
    pub(crate) generation_index: usize,
    pub(crate) is_incumbent: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CombatAssignment {
    pub(crate) claimant_id: u32,
    pub(crate) candidate: Option<CombatCandidate>,
}

#[derive(Clone, Copy)]
struct CombatProposal {
    pending_index: usize,
    claimant_id: u32,
    min_spacing: f32,
    candidate: CombatCandidate,
}

#[cfg(test)]
pub(crate) fn select_combat_position(query: &CombatQuery<'_>) -> Option<CombatCandidate> {
    combat_candidates(query).into_iter().next()
}

pub(crate) fn select_combat_positions_batch(queries: &[CombatQuery<'_>]) -> Vec<CombatAssignment> {
    struct Pending {
        query_index: usize,
        claimant_id: u32,
        min_spacing: f32,
        candidates: Vec<CombatCandidate>,
        next_candidate: usize,
        accepted: Option<CombatCandidate>,
    }

    #[derive(Clone, Copy)]
    struct AcceptedSlot {
        position: Vec3,
        min_spacing: f32,
    }

    let mut pending: Vec<Pending> = queries
        .iter()
        .enumerate()
        .map(|(query_index, query)| Pending {
            query_index,
            claimant_id: query.claimant_id,
            min_spacing: capsule_placement(query.nav_graph)
                .map(dynamic_min_spacing)
                .unwrap_or(0.0),
            candidates: combat_candidates(query),
            next_candidate: 0,
            accepted: None,
        })
        .collect();
    pending.sort_by(|a, b| {
        a.claimant_id
            .cmp(&b.claimant_id)
            .then_with(|| a.query_index.cmp(&b.query_index))
    });

    let mut accepted_slots: Vec<AcceptedSlot> = Vec::new();
    loop {
        let mut proposals = Vec::new();
        for (pending_index, item) in pending.iter().enumerate() {
            if item.accepted.is_some() {
                continue;
            }
            if let Some(&candidate) = item.candidates.get(item.next_candidate) {
                proposals.push(CombatProposal {
                    pending_index,
                    claimant_id: item.claimant_id,
                    min_spacing: item.min_spacing,
                    candidate,
                });
            }
        }

        if proposals.is_empty() {
            break;
        }

        proposals.sort_by(compare_proposals);
        let mut made_progress = false;
        for proposal in proposals {
            let item = &mut pending[proposal.pending_index];
            if item.accepted.is_some()
                || item.candidates.get(item.next_candidate) != Some(&proposal.candidate)
            {
                continue;
            }

            if accepted_slots.iter().any(|accepted| {
                distance_xz(proposal.candidate.position, accepted.position)
                    < proposal.min_spacing.max(accepted.min_spacing)
            }) {
                item.next_candidate += 1;
                made_progress = true;
                continue;
            }

            item.accepted = Some(proposal.candidate);
            accepted_slots.push(AcceptedSlot {
                position: proposal.candidate.position,
                min_spacing: proposal.min_spacing,
            });
            made_progress = true;
        }

        if !made_progress {
            break;
        }
    }

    pending
        .into_iter()
        .map(|item| CombatAssignment {
            claimant_id: item.claimant_id,
            candidate: item.accepted,
        })
        .collect()
}

pub(crate) fn combat_candidates(query: &CombatQuery<'_>) -> Vec<CombatCandidate> {
    if !query.agent_pos.is_finite()
        || !query.target_pos.is_finite()
        || !query.engagement_radius.is_finite()
        || !query.path_length_score_weight.is_finite()
        || query.engagement_radius <= 0.0
        || query.path_length_score_weight < 0.0
    {
        return Vec::new();
    }

    let placement = match capsule_placement(query.nav_graph) {
        Some(placement) => placement,
        None => return Vec::new(),
    };

    let mut challengers = Vec::new();
    if query.scan_challengers {
        for (generation_index, position) in
            generated_positions(query.target_pos, query.engagement_radius)
                .into_iter()
                .enumerate()
        {
            if let Some(candidate) =
                score_candidate(query, placement, position, generation_index, false)
            {
                challengers.push(candidate);
            }
        }
        challengers.sort_by(compare_candidates);
    }

    let incumbent = query
        .combat_slot
        .and_then(|slot| score_candidate(query, placement, slot, usize::MAX, true));
    apply_hysteresis(incumbent, challengers)
}

fn generated_positions(target_pos: Vec3, engagement_radius: f32) -> Vec<Vec3> {
    let mut positions = Vec::with_capacity(RING_DIRECTIONS.len() * RADIAL_MULTIPLIERS.len());
    for multiplier in RADIAL_MULTIPLIERS {
        for direction in RING_DIRECTIONS {
            let position = target_pos + direction * (engagement_radius * multiplier);
            if !positions
                .iter()
                .any(|existing| same_position_bits(*existing, position))
            {
                positions.push(position);
            }
        }
    }
    positions
}

fn same_position_bits(a: Vec3, b: Vec3) -> bool {
    a.x.to_bits() == b.x.to_bits()
        && a.y.to_bits() == b.y.to_bits()
        && a.z.to_bits() == b.z.to_bits()
}

fn capsule_placement(nav_graph: &NavGraph) -> Option<CapsulePlacement> {
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

fn score_candidate(
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
    let path = find_path(query.nav_graph, query.agent_pos, position)?;
    let path_cost = path_length(&path);
    let attack_band_error =
        (distance_xz(position, query.target_pos) - query.engagement_radius).abs();
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

fn dynamic_min_spacing(placement: CapsulePlacement) -> f32 {
    placement.radius * 2.0
}

fn apply_hysteresis(
    incumbent: Option<CombatCandidate>,
    mut challengers: Vec<CombatCandidate>,
) -> Vec<CombatCandidate> {
    let Some(incumbent) = incumbent else {
        return challengers;
    };

    challengers.retain(|candidate| !same_position_bits(candidate.position, incumbent.position));
    let challenger_beats_margin = challengers
        .first()
        .is_some_and(|candidate| candidate.score + COMBAT_SLOT_SWITCH_MARGIN < incumbent.score);

    if !challenger_beats_margin {
        let mut candidates = Vec::with_capacity(challengers.len() + 1);
        candidates.push(incumbent);
        candidates.extend(challengers);
        return candidates;
    }

    challengers.push(incumbent);
    challengers.sort_by(compare_candidates);
    challengers
}

fn path_length(path: &[Vec3]) -> f32 {
    path.windows(2)
        .map(|segment| distance_xz(segment[0], segment[1]))
        .sum()
}

fn compare_candidates(a: &CombatCandidate, b: &CombatCandidate) -> Ordering {
    a.score
        .total_cmp(&b.score)
        .then_with(|| a.attack_band_error.total_cmp(&b.attack_band_error))
        .then_with(|| a.path_cost.total_cmp(&b.path_cost))
        .then_with(|| a.is_incumbent.cmp(&b.is_incumbent).reverse())
        .then_with(|| a.generation_index.cmp(&b.generation_index))
        .then_with(|| a.position.x.total_cmp(&b.position.x))
        .then_with(|| a.position.y.total_cmp(&b.position.y))
        .then_with(|| a.position.z.total_cmp(&b.position.z))
}

fn compare_proposals(a: &CombatProposal, b: &CombatProposal) -> Ordering {
    a.candidate
        .score
        .total_cmp(&b.candidate.score)
        .then_with(|| {
            a.candidate
                .attack_band_error
                .total_cmp(&b.candidate.attack_band_error)
        })
        .then_with(|| a.candidate.path_cost.total_cmp(&b.candidate.path_cost))
        .then_with(|| {
            a.candidate
                .is_incumbent
                .cmp(&b.candidate.is_incumbent)
                .reverse()
        })
        .then_with(|| {
            a.candidate
                .generation_index
                .cmp(&b.candidate.generation_index)
        })
        .then_with(|| a.candidate.position.x.total_cmp(&b.candidate.position.x))
        .then_with(|| a.candidate.position.y.total_cmp(&b.candidate.position.y))
        .then_with(|| a.candidate.position.z.total_cmp(&b.candidate.position.z))
        .then_with(|| a.claimant_id.cmp(&b.claimant_id))
        .then_with(|| a.pending_index.cmp(&b.pending_index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use parry3d::math::{Isometry, Point};
    use parry3d::shape::TriMesh;
    use postretro_level_format::navmesh::{NAVMESH_VERSION, NavMeshSection, NavPortal, NavRegion};

    const EPS: f32 = 1.0e-4;
    const AGENT_RADIUS: f32 = 0.3;
    const AGENT_HEIGHT: f32 = 1.8;
    const STEP_HEIGHT: f32 = 0.4;
    const REST_Y: f32 = AGENT_HEIGHT * 0.5 + crate::collision::SKIN_DISTANCE;

    fn nav_region(x0: u32, z0: u32, x1: u32, z1: u32) -> NavRegion {
        NavRegion {
            x0,
            z0,
            x1,
            z1,
            floor_y_min: 0.0,
            floor_y_max: 0.25,
        }
    }

    fn nav_section(regions: Vec<NavRegion>, portals: Vec<NavPortal>) -> NavMeshSection {
        NavMeshSection {
            version: NAVMESH_VERSION,
            origin: [0.0, 0.0, 0.0],
            cell_size: 1.0,
            dim_x: 32,
            dim_z: 32,
            agent_radius: AGENT_RADIUS,
            agent_height: AGENT_HEIGHT,
            step_height: STEP_HEIGHT,
            max_slope_deg: 45.0,
            regions,
            portals,
        }
    }

    fn open_nav_graph() -> NavGraph {
        NavGraph::from_section(&nav_section(vec![nav_region(0, 0, 12, 12)], vec![]))
    }

    fn floor_world() -> CollisionWorld {
        let points = vec![
            Point::new(-20.0, 0.0, -20.0),
            Point::new(20.0, 0.0, -20.0),
            Point::new(20.0, 0.0, 20.0),
            Point::new(-20.0, 0.0, 20.0),
        ];
        let triangles = vec![[0u32, 1, 2], [0, 2, 3]];
        CollisionWorld {
            mesh: TriMesh::new(points, triangles),
            isometry: Isometry::identity(),
        }
    }

    fn floor_with_wall_at_west_candidate() -> CollisionWorld {
        let mut points = vec![
            Point::new(-20.0, 0.0, -20.0),
            Point::new(20.0, 0.0, -20.0),
            Point::new(20.0, 0.0, 20.0),
            Point::new(-20.0, 0.0, 20.0),
            Point::new(3.0, 0.0, 4.0),
            Point::new(3.0, 2.4, 4.0),
            Point::new(3.0, 2.4, 6.0),
            Point::new(3.0, 0.0, 6.0),
        ];
        let triangles = vec![[0u32, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]];
        CollisionWorld {
            mesh: TriMesh::new(std::mem::take(&mut points), triangles),
            isometry: Isometry::identity(),
        }
    }

    fn query<'a>(
        nav_graph: &'a NavGraph,
        collision_world: &'a CollisionWorld,
        agent_pos: Vec3,
        target_pos: Vec3,
    ) -> CombatQuery<'a> {
        CombatQuery {
            claimant_id: 1,
            agent_pos,
            engagement_radius: 2.0,
            target_pos,
            combat_slot: None,
            scan_challengers: true,
            other_agents: &[],
            nav_graph,
            collision_world,
            path_length_score_weight: PATH_LENGTH_SCORE_WEIGHT,
        }
    }

    fn query_with<'a>(
        claimant_id: u32,
        nav_graph: &'a NavGraph,
        collision_world: &'a CollisionWorld,
        agent_pos: Vec3,
        target_pos: Vec3,
        combat_slot: Option<Vec3>,
        other_agents: &'a [CombatAgentSnapshot],
    ) -> CombatQuery<'a> {
        CombatQuery {
            claimant_id,
            agent_pos,
            engagement_radius: 2.0,
            target_pos,
            combat_slot,
            scan_challengers: true,
            other_agents,
            nav_graph,
            collision_world,
            path_length_score_weight: PATH_LENGTH_SCORE_WEIGHT,
        }
    }

    fn snapshot(claimant_id: u32, position: Vec3) -> CombatAgentSnapshot {
        CombatAgentSnapshot {
            claimant_id,
            position,
        }
    }

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() <= EPS
    }

    fn approx_xz(a: Vec3, b: Vec3) -> bool {
        approx_eq(a.x, b.x) && approx_eq(a.z, b.z)
    }

    #[test]
    fn combat_candidates_returns_distinct_ordered_points_around_target() {
        let nav_graph = open_nav_graph();
        let world = floor_world();
        let target = Vec3::new(5.0, REST_Y, 5.0);
        let candidates = combat_candidates(&query(
            &nav_graph,
            &world,
            Vec3::new(1.0, REST_Y, 5.0),
            target,
        ));
        let repeated = combat_candidates(&query(
            &nav_graph,
            &world,
            Vec3::new(1.0, REST_Y, 5.0),
            target,
        ));

        assert!(!candidates.is_empty());
        assert_eq!(candidates, repeated);
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.position != target)
        );
        for (i, candidate) in candidates.iter().enumerate() {
            assert!(
                candidates[..i]
                    .iter()
                    .all(|previous| !same_position_bits(previous.position, candidate.position)),
                "duplicate candidate at {:?}",
                candidate.position
            );
        }
    }

    #[test]
    fn combat_candidates_rejects_unreachable_nav_regions() {
        let nav_graph = NavGraph::from_section(&nav_section(
            vec![nav_region(0, 0, 4, 4), nav_region(8, 8, 12, 12)],
            vec![],
        ));
        let world = floor_world();
        let candidates = combat_candidates(&CombatQuery {
            claimant_id: 1,
            agent_pos: Vec3::new(2.0, REST_Y, 2.0),
            engagement_radius: 1.0,
            target_pos: Vec3::new(10.0, REST_Y, 10.0),
            combat_slot: None,
            scan_challengers: true,
            other_agents: &[],
            nav_graph: &nav_graph,
            collision_world: &world,
            path_length_score_weight: PATH_LENGTH_SCORE_WEIGHT,
        });

        assert!(
            candidates.is_empty(),
            "disconnected candidates must be filtered out: {candidates:?}"
        );
    }

    #[test]
    fn combat_candidates_skip_static_capsule_occupancy_failures() {
        let nav_graph = open_nav_graph();
        let world = floor_with_wall_at_west_candidate();
        let target = Vec3::new(5.0, REST_Y, 5.0);
        let west_candidate = Vec3::new(3.0, REST_Y, 5.0);
        assert!(
            nav_graph.region_at(west_candidate).is_some()
                && find_path(&nav_graph, Vec3::new(1.0, REST_Y, 5.0), west_candidate).is_some(),
            "fixture candidate must be nav-accepted so this test isolates static occupancy"
        );
        let candidates = combat_candidates(&query(
            &nav_graph,
            &world,
            Vec3::new(1.0, REST_Y, 5.0),
            target,
        ));

        assert!(!candidates.is_empty());
        assert!(
            candidates
                .iter()
                .all(|candidate| !approx_xz(candidate.position, west_candidate)),
            "wall-overlapping candidate should be skipped: {candidates:?}"
        );
        assert!(
            !approx_xz(candidates[0].position, west_candidate),
            "selector should fall through to the next valid candidate"
        );
    }

    #[test]
    fn combat_candidates_ground_ring_slots_when_target_center_y_is_offset() {
        let nav_graph = open_nav_graph();
        let world = floor_world();
        let target = Vec3::new(5.0, REST_Y + 1.5, 5.0);

        let selected = select_combat_position(&query(
            &nav_graph,
            &world,
            Vec3::new(1.0, REST_Y, 5.0),
            target,
        ))
        .expect("offset target center should still yield a grounded ring slot");

        assert!(
            (selected.position.y - REST_Y).abs() <= EPS,
            "combat slot should use grounded capsule center, got {:?}",
            selected.position
        );
    }

    #[test]
    fn capsule_placement_allows_sphere_shaped_nav_agents() {
        let graph = NavGraph::from_section(&nav_section(vec![nav_region(0, 0, 12, 12)], vec![]));
        let mut section = nav_section(vec![nav_region(0, 0, 12, 12)], vec![]);
        section.agent_height = section.agent_radius * 2.0;
        let sphere_graph = NavGraph::from_section(&section);

        assert!(capsule_placement(&graph).is_some());
        assert_eq!(
            capsule_placement(&sphere_graph).map(|placement| placement.half_height),
            Some(0.0)
        );
    }

    #[test]
    fn select_combat_position_is_deterministic_for_identical_inputs() {
        let nav_graph = open_nav_graph();
        let world = floor_world();
        let query = query(
            &nav_graph,
            &world,
            Vec3::new(1.0, REST_Y, 5.0),
            Vec3::new(5.0, REST_Y, 5.0),
        );

        let first = select_combat_position(&query);
        let second = select_combat_position(&query);

        assert_eq!(first, second);
        assert!(first.is_some());
    }

    #[test]
    fn combat_candidates_do_not_treat_unvalidated_snapshot_slots_as_blockers() {
        let nav_graph = open_nav_graph();
        let world = floor_world();
        let target = Vec3::new(5.0, REST_Y, 5.0);
        let claimed_slot = Vec3::new(7.0, REST_Y, 5.0);
        let blockers = [snapshot(2, Vec3::new(1.0, REST_Y, 1.0))];

        let candidates = combat_candidates(&query_with(
            1,
            &nav_graph,
            &world,
            Vec3::new(1.0, REST_Y, 5.0),
            target,
            None,
            &blockers,
        ));

        assert!(!candidates.is_empty());
        assert!(
            candidates
                .iter()
                .any(|candidate| approx_xz(candidate.position, claimed_slot)),
            "snapshot combat slots are proposals, not hard blockers: {candidates:?}"
        );
    }

    #[test]
    fn batch_assignment_is_order_independent_and_distinct() {
        let nav_graph = open_nav_graph();
        let world = floor_world();
        let target = Vec3::new(5.0, REST_Y, 5.0);
        let agent_pos = Vec3::new(1.0, REST_Y, 1.0);
        let frozen = [
            snapshot(30, agent_pos),
            snapshot(10, agent_pos),
            snapshot(20, agent_pos),
        ];
        let queries = [
            query_with(30, &nav_graph, &world, agent_pos, target, None, &frozen),
            query_with(10, &nav_graph, &world, agent_pos, target, None, &frozen),
            query_with(20, &nav_graph, &world, agent_pos, target, None, &frozen),
        ];
        let reversed = [queries[2], queries[1], queries[0]];

        let first = select_combat_positions_batch(&queries);
        let second = select_combat_positions_batch(&reversed);

        assert_eq!(first, second);
        assert!(
            first
                .iter()
                .all(|assignment| assignment.candidate.is_some())
        );
        for (i, assignment) in first.iter().enumerate() {
            let position = assignment.candidate.unwrap().position;
            assert!(
                first[..i]
                    .iter()
                    .all(|previous| { !approx_xz(previous.candidate.unwrap().position, position) }),
                "accepted slots should be distinct: {first:?}"
            );
        }
    }

    #[test]
    fn scarce_slots_leave_extra_agents_unassigned() {
        let nav_graph = open_nav_graph();
        let world = floor_world();
        let target = Vec3::new(5.0, REST_Y, 5.0);
        let agent_pos = Vec3::new(1.0, REST_Y, 1.0);
        let open_slot = Vec3::new(target.x + 2.0, REST_Y, target.z);
        let frozen = [
            snapshot(10, agent_pos),
            snapshot(20, agent_pos),
            snapshot(30, agent_pos),
        ];
        let mut queries = [
            query_with(
                10,
                &nav_graph,
                &world,
                agent_pos,
                target,
                Some(open_slot),
                &frozen,
            ),
            query_with(
                20,
                &nav_graph,
                &world,
                agent_pos,
                target,
                Some(open_slot),
                &frozen,
            ),
            query_with(
                30,
                &nav_graph,
                &world,
                agent_pos,
                target,
                Some(open_slot),
                &frozen,
            ),
        ];
        for query in &mut queries {
            query.scan_challengers = false;
        }

        let assignments = select_combat_positions_batch(&queries);
        let assigned: Vec<_> = assignments
            .iter()
            .filter_map(|assignment| assignment.candidate)
            .collect();

        assert_eq!(assigned.len(), 1, "only one slot should remain claimable");
        assert!(approx_xz(assigned[0].position, open_slot));
        assert_eq!(
            assignments
                .iter()
                .filter(|assignment| assignment.candidate.is_none())
                .count(),
            2
        );
    }

    #[test]
    fn hysteresis_keeps_incumbent_inside_switch_margin() {
        let nav_graph = open_nav_graph();
        let world = floor_world();
        let target = Vec3::new(5.0, REST_Y, 5.0);
        let incumbent = Vec3::new(
            target.x + 2.0 + COMBAT_SLOT_SWITCH_MARGIN - 0.1,
            REST_Y,
            target.z,
        );
        let mut query = query_with(1, &nav_graph, &world, target, target, Some(incumbent), &[]);
        query.path_length_score_weight = 0.0;

        let selected = select_combat_position(&query).expect("incumbent should be valid");

        assert!(selected.is_incumbent);
        assert!(approx_xz(selected.position, incumbent));
    }

    #[test]
    fn hysteresis_switches_when_challenger_beats_margin() {
        let nav_graph = open_nav_graph();
        let world = floor_world();
        let target = Vec3::new(5.0, REST_Y, 5.0);
        let incumbent = Vec3::new(
            target.x + 2.0 + COMBAT_SLOT_SWITCH_MARGIN + 0.1,
            REST_Y,
            target.z,
        );
        let mut query = query_with(1, &nav_graph, &world, target, target, Some(incumbent), &[]);
        query.path_length_score_weight = 0.0;

        let selected = select_combat_position(&query).expect("challenger should be valid");

        assert!(!selected.is_incumbent);
        assert!(!approx_xz(selected.position, incumbent));
        assert!(selected.score + COMBAT_SLOT_SWITCH_MARGIN < COMBAT_SLOT_SWITCH_MARGIN + 0.1);
    }
}
