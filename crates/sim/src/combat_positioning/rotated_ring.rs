//! Per-query rotated combat ring: the candidate set `strafe` and `reposition`
//! generate in place of the shared world-axis ring.
//!
//! See: context/lib/entity_model.md §7c (Engaged motion and facing).

use glam::{Quat, Vec3};

use super::scoring::score_candidate;
use super::{CombatCandidate, CombatQuery, compare_candidates};
use crate::collision::CapsulePlacement;
use crate::nav::distance_xz;

/// Slots on a rotated ring, one every 45°.
pub const RING_SLOT_COUNT: u8 = 8;
const RING_STEP_RADIANS: f32 = std::f32::consts::FRAC_PI_4;
const RING_STEP_DEG: f32 = 45.0;
/// A keep-away is inclusive at its distance: a neighbor slot sits exactly one
/// spacing from its sibling, and float rotation must not push it out.
const KEEP_AWAY_TOLERANCE: f32 = 1.0e-3;

/// One slot spacing: the chord between neighboring ring slots at `standoff`.
pub fn slot_spacing(standoff: f32) -> f32 {
    2.0 * standoff * (RING_STEP_RADIANS * 0.5).sin()
}

/// Degrees between ring index `index` and the ring bearing, folded to either
/// side: 0 is the bearing itself, 180 the far side.
fn ring_index_arc_deg(index: u8) -> f32 {
    let index = index % RING_SLOT_COUNT;
    let steps = index.min(RING_SLOT_COUNT - index);
    RING_STEP_DEG * f32::from(steps)
}

/// Ring indices whose generated angle lies within `[min, max]` degrees, on
/// both sides of the bearing, as a bitmask (bit `i` is index `i`). The test
/// reads the index's exact generated angle, so a bound on a ring direction
/// includes it.
pub fn arc_slot_mask(arc_deg: [f32; 2]) -> u8 {
    let [min, max] = arc_deg;
    (0..RING_SLOT_COUNT)
        .filter(|&index| {
            let angle = ring_index_arc_deg(index);
            min <= angle && angle <= max
        })
        .fold(0, |mask, index| mask | (1 << index))
}

/// How a rotated ring orders the candidates it proposes to the batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RingOrder {
    /// Best score first, ties broken by ring index.
    ByScore,
    /// The listed index order: a preference the caller already decided, such
    /// as a strafer's travel direction before its reverse.
    Listed,
}

/// Slots closer than `distance` (XZ) to `point` never qualify.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeepAway {
    pub point: Vec3,
    pub distance: f32,
}

/// Per-query ring and constraints. The ring is centered on the query's
/// `target_pos` at its `engagement_radius` (radius multiplier 1.0 only, "at
/// standoff"), and ring index `i` lies `i * 45°` counter-clockwise seen from
/// above (+yaw) from `bearing`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlotRing {
    /// Unit XZ direction, from the target, of ring index 0.
    bearing: Vec3,
    slots: [u8; RING_SLOT_COUNT as usize],
    len: u8,
    order: RingOrder,
    keep_away: [Option<KeepAway>; 2],
}

impl SlotRing {
    /// Propose `indices` in the listed order. Indices past the ring and
    /// repeats are dropped.
    pub fn listed(bearing: Vec3, indices: &[u8]) -> Self {
        let mut ring = Self::empty(bearing, RingOrder::Listed);
        for &index in indices {
            ring.push(index);
        }
        ring
    }

    /// Propose every index set in `mask` (bit `i` is index `i`), best score
    /// first.
    pub fn by_score(bearing: Vec3, mask: u8) -> Self {
        let mut ring = Self::empty(bearing, RingOrder::ByScore);
        for index in (0..RING_SLOT_COUNT).filter(|index| mask & (1 << index) != 0) {
            ring.push(index);
        }
        ring
    }

    /// Add a keep-away. A ring carries at most two; a third is ignored.
    pub fn with_keep_away(mut self, keep_away: KeepAway) -> Self {
        if let Some(free) = self.keep_away.iter_mut().find(|slot| slot.is_none()) {
            *free = Some(keep_away);
        }
        self
    }

    /// The proposed ring indices, in generation order.
    pub fn slots(&self) -> &[u8] {
        &self.slots[..usize::from(self.len)]
    }

    /// Ring index `index`'s world position around `center` at `radius`.
    pub fn slot_position(&self, center: Vec3, radius: f32, index: u8) -> Vec3 {
        let rotation =
            Quat::from_rotation_y(f32::from(index % RING_SLOT_COUNT) * RING_STEP_RADIANS);
        center + rotation * self.bearing * radius
    }

    fn empty(bearing: Vec3, order: RingOrder) -> Self {
        Self {
            bearing: Vec3::new(bearing.x, 0.0, bearing.z),
            slots: [0; RING_SLOT_COUNT as usize],
            len: 0,
            order,
            keep_away: [None; 2],
        }
    }

    fn push(&mut self, index: u8) {
        if index < RING_SLOT_COUNT && !self.slots().contains(&index) {
            self.slots[usize::from(self.len)] = index;
            self.len += 1;
        }
    }

    fn keeps_away(&self, position: Vec3) -> bool {
        self.keep_away.iter().flatten().all(|keep_away| {
            distance_xz(position, keep_away.point) + KEEP_AWAY_TOLERANCE >= keep_away.distance
        })
    }
}

/// Score the ring's slots. Each candidate's `generation_index` is its ring
/// index. Keep-aways reject before the scorer pays for routing.
pub(super) fn ring_candidates(
    query: &CombatQuery<'_>,
    ring: &SlotRing,
    placement: CapsulePlacement,
) -> Vec<CombatCandidate> {
    let bearing_len_sq = ring.bearing.length_squared();
    if !bearing_len_sq.is_finite() || (bearing_len_sq - 1.0).abs() > 1.0e-3 {
        return Vec::new();
    }
    let mut candidates = Vec::with_capacity(usize::from(ring.len));
    for &index in ring.slots() {
        let position = ring.slot_position(query.target_pos, query.engagement_radius, index);
        if !ring.keeps_away(position) {
            continue;
        }
        if let Some(candidate) =
            score_candidate(query, placement, position, usize::from(index), false)
        {
            candidates.push(candidate);
        }
    }
    if ring.order == RingOrder::ByScore {
        candidates.sort_by(compare_candidates);
    }
    candidates
}

#[cfg(test)]
mod tests {
    use super::super::combat_candidates;
    use super::*;
    use crate::collision::CollisionWorld;
    use crate::nav::NavGraph;
    use postretro_foundation::GroundRef;
    use postretro_level_format::navmesh::{NAVMESH_VERSION, NavMeshSection, NavRegion};

    const REST_Y: f32 = 1.8 * 0.5 + crate::collision::SKIN_DISTANCE;
    const EYE: Vec3 = Vec3::new(0.0, 1.0, 0.0);
    const TARGET: Vec3 = Vec3::new(16.0, REST_Y, 16.0);
    const RADIUS: f32 = 4.0;

    fn nav_graph() -> NavGraph {
        NavGraph::from_section(&NavMeshSection {
            version: NAVMESH_VERSION,
            origin: [0.0, 0.0, 0.0],
            cell_size: 1.0,
            dim_x: 32,
            dim_z: 32,
            agent_radius: 0.3,
            agent_height: 1.8,
            step_height: 0.4,
            max_slope_deg: 45.0,
            regions: vec![NavRegion {
                x0: 0,
                z0: 0,
                x1: 32,
                z1: 32,
                floor_y_min: 0.0,
                floor_y_max: 0.25,
            }],
            portals: vec![],
            gates: Vec::new(),
            links: Vec::new(),
        })
    }

    fn floor() -> CollisionWorld {
        CollisionWorld::from_triangles_for_test(
            vec![
                Vec3::new(-4.0, 0.0, -4.0),
                Vec3::new(36.0, 0.0, -4.0),
                Vec3::new(36.0, 0.0, 36.0),
                Vec3::new(-4.0, 0.0, 36.0),
            ],
            vec![[0u32, 1, 2], [0, 2, 3]],
        )
    }

    fn ring_query<'a>(
        nav_graph: &'a NavGraph,
        world: &'a CollisionWorld,
        agent_pos: Vec3,
        ring: SlotRing,
    ) -> CombatQuery<'a> {
        CombatQuery {
            claimant_id: 1,
            agent_pos,
            agent_ground: GroundRef::World,
            agent_height: nav_graph.agent_params().height,
            engagement_radius: RADIUS,
            target_pos: TARGET,
            enemy_eye_offset: EYE,
            target_aim: TARGET + EYE,
            combat_slot: None,
            scan_challengers: true,
            other_agents: &[],
            nav_graph,
            mover_nav: crate::nav::MoverNav::none(),
            collision_world: world,
            path_length_score_weight: super::super::PATH_LENGTH_SCORE_WEIGHT,
            ring: Some(ring),
        }
    }

    fn indices(candidates: &[CombatCandidate]) -> Vec<usize> {
        candidates.iter().map(|c| c.generation_index).collect()
    }

    #[test]
    fn slot_spacing_is_the_ring_neighbor_chord_at_standoff() {
        let ring = SlotRing::listed(Vec3::X, &[0, 1]);
        let chord = distance_xz(
            ring.slot_position(TARGET, RADIUS, 0),
            ring.slot_position(TARGET, RADIUS, 1),
        );
        assert!((slot_spacing(RADIUS) - chord).abs() <= 1.0e-5);
    }

    #[test]
    fn ring_indices_step_counter_clockwise_from_the_bearing_at_standoff() {
        let bearing = Vec3::new(0.6, 0.0, 0.8);
        let ring = SlotRing::listed(bearing, &[0, 1, 2, 3, 4, 5, 6, 7]);
        let bearing_yaw = bearing.x.atan2(bearing.z);
        for index in 0..RING_SLOT_COUNT {
            let offset = ring.slot_position(TARGET, RADIUS, index) - TARGET;
            assert!((distance_xz(offset, Vec3::ZERO) - RADIUS).abs() <= 1.0e-4);
            let yaw = offset.x.atan2(offset.z);
            let expected = bearing_yaw + f32::from(index) * RING_STEP_RADIANS;
            let residual = (yaw - expected + std::f32::consts::PI)
                .rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            assert!(residual.abs() <= 1.0e-4, "index {index} at yaw {yaw}");
        }
    }

    // Row M4: arc bounds test each ring index's generated angle, both sides.
    #[test]
    fn arc_mask_selects_generated_ring_angles_on_both_sides() {
        let set = |mask: u8| -> Vec<u8> { (0..8).filter(|i| mask & (1 << i) != 0).collect() };
        assert_eq!(set(arc_slot_mask([45.0, 135.0])), [1, 2, 3, 5, 6, 7]);
        assert_eq!(set(arc_slot_mask([90.0, 90.0])), [2, 6]);
        assert_eq!(set(arc_slot_mask([180.0, 180.0])), [4]);
        assert_eq!(set(arc_slot_mask([0.0, 180.0])), [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(set(arc_slot_mask([0.0, 0.0])), [0]);
    }

    // Row M6: `minTravel: 8` refuses a slot 7 m from the entry position and
    // accepts one 9 m from it.
    #[test]
    fn reposition_min_travel_refuses_near_and_accepts_far_slot() {
        let nav = nav_graph();
        let world = floor();
        let ring = SlotRing::listed(Vec3::X, &[0, 4]);
        let near = ring.slot_position(TARGET, RADIUS, 0);
        let far = ring.slot_position(TARGET, RADIUS, 4);
        // The one point 7 m from slot 0 and 9 m from slot 4, off the axis.
        let entry = Vec3::new(18.0, REST_Y, 16.0 + 45.0_f32.sqrt());
        assert!((distance_xz(entry, near) - 7.0).abs() <= 1.0e-3);
        assert!((distance_xz(entry, far) - 9.0).abs() <= 1.0e-3);

        let ring = ring.with_keep_away(KeepAway {
            point: entry,
            distance: 8.0,
        });
        let candidates = combat_candidates(&ring_query(&nav, &world, entry, ring));

        assert_eq!(indices(&candidates), [4], "{candidates:?}");
    }

    #[test]
    fn keep_away_admits_a_neighbor_exactly_one_spacing_away() {
        let nav = nav_graph();
        let world = floor();
        let ring = SlotRing::by_score(Vec3::X, 0b1000_0011);
        let held = ring.slot_position(TARGET, RADIUS, 0);
        let ring = ring.with_keep_away(KeepAway {
            point: held,
            distance: slot_spacing(RADIUS),
        });
        let candidates = combat_candidates(&ring_query(&nav, &world, held, ring));

        let mut found = indices(&candidates);
        found.sort_unstable();
        assert_eq!(found, [1, 7], "the held slot alone is within a spacing");
    }

    #[test]
    fn listed_order_survives_scoring_and_by_score_sorts() {
        let nav = nav_graph();
        let world = floor();
        // An agent near index 1 prices index 7's route longer.
        let agent = Vec3::new(TARGET.x + 3.0, REST_Y, TARGET.z - 3.0);
        let listed = combat_candidates(&ring_query(
            &nav,
            &world,
            agent,
            SlotRing::listed(Vec3::X, &[7, 1]),
        ));
        assert_eq!(indices(&listed), [7, 1]);

        let scored = combat_candidates(&ring_query(
            &nav,
            &world,
            agent,
            SlotRing::by_score(Vec3::X, 0b1000_0010),
        ));
        assert_eq!(indices(&scored), [1, 7]);
    }

    #[test]
    fn rotated_ring_generates_only_its_slots_at_standoff() {
        let nav = nav_graph();
        let world = floor();
        let ring = SlotRing::by_score(Vec3::Z, 0xFF);
        let candidates = combat_candidates(&ring_query(&nav, &world, TARGET + Vec3::Z * 6.0, ring));

        assert_eq!(candidates.len(), 8);
        for candidate in &candidates {
            assert!(
                (distance_xz(candidate.position, TARGET) - RADIUS).abs() <= 1.0e-3,
                "{candidate:?} sits off the standoff ring"
            );
        }
    }
}
