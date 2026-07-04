use glam::Vec3;
use postretro_level_loader::{LightType, MapLight};

use crate::NO_SHADOW_SLOT;

/// The one shadow-slot score formula, shared by every ranker so the spot, cube,
/// and promotion paths cannot drift. `distance` is the light→camera distance;
/// `near_clip` floors the denominator so a light sitting on the camera does not
/// divide by zero. Larger reach and closer lights score higher.
pub fn slot_score(falloff_range: f32, distance: f32, near_clip: f32) -> f32 {
    let denom = distance.max(near_clip);
    (falloff_range / denom).powi(2)
}

/// Shared scoring/drop core for shadow-slot rankers. Takes pre-filtered,
/// pre-scored candidates — each `(light_index, influence_score)` — plus the
/// pool `capacity` and the total light count, and returns a `slot_assignment`
/// Vec indexed by light index: each entry is a slot (`0..capacity`) or
/// [`NO_SHADOW_SLOT`].
///
/// Sorts by score descending, breaking ties by ascending light index for
/// determinism, then assigns the top `capacity` to dense slots `0..capacity`;
/// every lower-ranked candidate keeps `NO_SHADOW_SLOT` (dropped gracefully).
///
/// `candidates` is taken by value so the caller does not pay a clone.
pub fn assign_ranked_slots(
    mut candidates: Vec<(usize, f32)>,
    capacity: usize,
    light_count: usize,
) -> Vec<u32> {
    let mut slot_assignment = vec![NO_SHADOW_SLOT; light_count];

    candidates.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });

    for (slot, (light_idx, _score)) in candidates.iter().take(capacity).enumerate() {
        slot_assignment[*light_idx] = slot as u32;
    }

    slot_assignment
}

/// Rank dynamic spot lights into a fixed-capacity shadow pool.
///
/// Pool eligibility is `is_dynamic && Spot`. The caller owns the renderer pool
/// capacity and the per-light visibility/brightness gate; an empty or short
/// `eligible_lights` slice treats missing entries as eligible.
pub fn rank_spot_lights(
    lights: &[MapLight],
    camera_position: Vec3,
    camera_near_clip: f32,
    eligible_lights: &[bool],
    capacity: usize,
) -> Vec<u32> {
    let candidates = rank_candidates(
        lights,
        camera_position,
        camera_near_clip,
        eligible_lights,
        LightType::Spot,
    );

    if candidates.len() > capacity {
        log::debug!(
            "[ShadowPool] {} pool-eligible spot lights visible; {} assigned to slots, {} unshadowed",
            candidates.len(),
            capacity,
            candidates.len() - capacity
        );
    }

    assign_ranked_slots(candidates, capacity, lights.len())
}

/// Rank dynamic point lights into a fixed-capacity cube shadow pool.
///
/// Pool eligibility is `is_dynamic && Point`. The scoring and overflow policy
/// intentionally match [`rank_spot_lights`] so spot and point pools cannot drift.
pub fn rank_point_lights(
    lights: &[MapLight],
    camera_position: Vec3,
    camera_near_clip: f32,
    eligible_lights: &[bool],
    capacity: usize,
) -> Vec<u32> {
    let candidates = rank_candidates(
        lights,
        camera_position,
        camera_near_clip,
        eligible_lights,
        LightType::Point,
    );

    if candidates.len() > capacity {
        log::debug!(
            "[CubeShadowPool] {} pool-eligible point lights; {} assigned to cube slots, {} unshadowed",
            candidates.len(),
            capacity,
            candidates.len() - capacity
        );
    }

    assign_ranked_slots(candidates, capacity, lights.len())
}

fn rank_candidates(
    lights: &[MapLight],
    camera_position: Vec3,
    camera_near_clip: f32,
    eligible_lights: &[bool],
    light_type: LightType,
) -> Vec<(usize, f32)> {
    lights
        .iter()
        .enumerate()
        .filter_map(|(idx, light)| {
            if !light.is_dynamic || light.light_type != light_type {
                return None;
            }
            if idx < eligible_lights.len() && !eligible_lights[idx] {
                return None;
            }

            let light_pos = Vec3::new(
                light.origin[0] as f32,
                light.origin[1] as f32,
                light.origin[2] as f32,
            );
            let dist = (light_pos - camera_position).length();
            let score = slot_score(light.falloff_range, dist, camera_near_clip);
            Some((idx, score))
        })
        .collect()
}

/// A shadow-slot candidate in the unified (dynamic + promoted-static) ranking.
/// `candidate_index` indexes the renderer's shadow-candidate light list;
/// `is_promoted_static` marks a compiler-selected static light competing for a
/// promoted slot (subject to `promoted_cap`) — a dynamic-tier light has it
/// `false`. Both tiers score through [`slot_score`] and compete on that score
/// alone; no tier is reserved ahead of the sort.
#[derive(Clone, Copy, Debug)]
pub struct SlotCandidate {
    pub candidate_index: usize,
    pub score: f32,
    pub is_promoted_static: bool,
}

/// The previous frame's occupant of a shadow slot, supplied so eviction
/// hysteresis is tier-neutral: a challenger takes a held slot only when it
/// out-scores the incumbent by the eviction margin, in BOTH directions
/// (dynamic⇄static and dynamic⇄dynamic). Static incumbents are the promoted
/// lights still holding a slot with weight `w > 0` (including the demote sticky
/// window, when the light is no longer a candidate); dynamic incumbents are the
/// still-eligible lights that held a slot last frame. `score` is the incumbent's
/// CURRENT-frame score so a camera jump is reflected before the comparison.
#[derive(Clone, Copy, Debug)]
pub struct SlotIncumbent {
    pub slot: usize,
    pub candidate_index: usize,
    pub score: f32,
    pub is_promoted_static: bool,
}

#[derive(Clone, Copy)]
struct SlotOccupant {
    candidate_index: usize,
    score: f32,
    is_promoted_static: bool,
}

/// Assign shadow-pool slots from a unified candidate set with tier-neutral
/// eviction hysteresis and a promoted-static cap.
///
/// Dynamic and promoted-static candidates compete on `score` alone — the
/// pre-emptive static reservation is gone, so a weaker static no longer beats a
/// stronger dynamic for a slot. Prior-frame `incumbents` seed the slots: a held
/// slot stays the incumbent's until a challenger out-scores its occupant by
/// `eviction_margin`, applied regardless of tier and in both directions.
/// `promoted_cap` bounds how many promoted-static lights may occupy the pool at
/// once — a static challenger already at the cap may only swap into a slot
/// another static holds, never grow the static population by taking a free or
/// dynamic-held slot, so statics keep their budget even when they win on score.
///
/// The handoff errs dark: an evicted static simply loses its slot here; its
/// weight ramp-down (the SH-delta subtraction) is the renderer's job.
///
/// Returns a `candidate_count`-length Vec indexed by candidate index: each entry
/// is a slot (`0..capacity`) or [`NO_SHADOW_SLOT`].
pub fn assign_slots_with_hysteresis(
    candidates: &[SlotCandidate],
    incumbents: &[SlotIncumbent],
    capacity: usize,
    promoted_cap: usize,
    candidate_count: usize,
    eviction_margin: f32,
) -> Vec<u32> {
    let mut assignment = vec![NO_SHADOW_SLOT; candidate_count];
    let mut slots: Vec<Option<SlotOccupant>> = vec![None; capacity];
    let mut promoted_count = 0usize;

    // Seed prior-frame occupants. A held slot is the incumbent's until a stronger
    // challenger displaces it — this is the hysteresis. Static sticky-window
    // incumbents that are no longer candidates hold their slot here too (their
    // weight ramps down while the slot stays assigned).
    for inc in incumbents {
        if inc.slot >= capacity || inc.candidate_index >= candidate_count {
            continue;
        }
        if slots[inc.slot].is_some() || assignment[inc.candidate_index] != NO_SHADOW_SLOT {
            continue;
        }
        if inc.is_promoted_static && promoted_count >= promoted_cap {
            // Defensive: a prior frame already honoured the cap, so this should
            // not fire — but never seed more statics than the budget.
            continue;
        }
        slots[inc.slot] = Some(SlotOccupant {
            candidate_index: inc.candidate_index,
            score: inc.score,
            is_promoted_static: inc.is_promoted_static,
        });
        assignment[inc.candidate_index] = inc.slot as u32;
        if inc.is_promoted_static {
            promoted_count += 1;
        }
    }

    let mut ranked: Vec<SlotCandidate> = candidates.to_vec();
    ranked.sort_by(candidate_order);

    for cand in ranked {
        if cand.candidate_index >= candidate_count
            || assignment[cand.candidate_index] != NO_SHADOW_SLOT
        {
            // Already holds a slot (an incumbent) — hysteresis keeps it.
            continue;
        }
        let cand_static = cand.is_promoted_static;
        // A static already at the cap may not grow the static population; it can
        // only swap into another static's slot below.
        let may_grow_static = !cand_static || promoted_count < promoted_cap;

        if may_grow_static {
            if let Some(free) = slots.iter().position(|slot| slot.is_none()) {
                slots[free] = Some(SlotOccupant {
                    candidate_index: cand.candidate_index,
                    score: cand.score,
                    is_promoted_static: cand_static,
                });
                assignment[cand.candidate_index] = free as u32;
                if cand_static {
                    promoted_count += 1;
                }
                continue;
            }
        }

        // No free slot the candidate may take — try to evict the weakest
        // incumbent it is allowed to displace. A capped static may only take a
        // slot another static holds (swap; net count unchanged); every other
        // candidate may evict any tier.
        let static_only = cand_static && promoted_count >= promoted_cap;
        let target = slots
            .iter()
            .enumerate()
            .filter_map(|(slot, occ)| occ.as_ref().map(|occ| (slot, *occ)))
            .filter(|(_, occ)| !static_only || occ.is_promoted_static)
            .min_by(|(_, a), (_, b)| {
                a.score
                    .partial_cmp(&b.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        if let Some((slot, occ)) = target {
            if challenger_can_evict(cand.score, occ.score, eviction_margin) {
                assignment[occ.candidate_index] = NO_SHADOW_SLOT;
                if occ.is_promoted_static {
                    promoted_count -= 1;
                }
                slots[slot] = Some(SlotOccupant {
                    candidate_index: cand.candidate_index,
                    score: cand.score,
                    is_promoted_static: cand_static,
                });
                assignment[cand.candidate_index] = slot as u32;
                if cand_static {
                    promoted_count += 1;
                }
            }
        }
    }

    assignment
}

fn candidate_order(a: &SlotCandidate, b: &SlotCandidate) -> std::cmp::Ordering {
    b.score
        .partial_cmp(&a.score)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then(a.candidate_index.cmp(&b.candidate_index))
}

/// A challenger displaces an incumbent only when it out-scores it by `margin`.
/// The incumbent score is floored at zero so a degenerate negative score cannot
/// invert the comparison.
fn challenger_can_evict(challenger_score: f32, incumbent_score: f32, margin: f32) -> bool {
    challenger_score > incumbent_score.max(0.0) * margin
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_loader::{FalloffModel, ShadowType};
    use proptest::prelude::*;

    const SPOT_CAPACITY: usize = 96;
    const CUBE_CAPACITY: usize = 6;

    fn spot_light(origin: [f64; 3], falloff_range: f32, is_dynamic: bool) -> MapLight {
        MapLight {
            origin,
            light_type: LightType::Spot,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::Linear,
            falloff_range,
            cone_angle_inner: 0.3,
            cone_angle_outer: 0.6,
            cone_direction: [0.0, 0.0, -1.0],
            is_dynamic,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![],
            cell_index: 0,
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    fn point_light(origin: [f64; 3], falloff_range: f32, is_dynamic: bool) -> MapLight {
        MapLight {
            origin,
            light_type: LightType::Point,
            intensity: 1.0,
            color: [1.0, 1.0, 1.0],
            falloff_model: FalloffModel::InverseSquared,
            falloff_range,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, 0.0, 0.0],
            is_dynamic,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![],
            cell_index: 0,
            shadow_type: ShadowType::StaticLightMap,
        }
    }

    #[test]
    fn empty_light_list_produces_empty_spot_assignment() {
        let assignment = rank_spot_lights(&[], Vec3::ZERO, 0.1, &[], SPOT_CAPACITY);
        assert!(assignment.is_empty());
    }

    #[test]
    fn baked_spots_are_not_assigned() {
        let lights = vec![
            spot_light([0.0, 0.0, 0.0], 10.0, false),
            spot_light([10.0, 0.0, 0.0], 10.0, false),
        ];
        let assignment = rank_spot_lights(&lights, Vec3::ZERO, 0.1, &[], SPOT_CAPACITY);
        assert_eq!(assignment[0], NO_SHADOW_SLOT);
        assert_eq!(assignment[1], NO_SHADOW_SLOT);
    }

    #[test]
    fn dynamic_spot_qualifies_for_pool() {
        let lights = vec![spot_light([0.0, 0.0, 0.0], 10.0, true)];
        let assignment = rank_spot_lights(&lights, Vec3::ZERO, 0.1, &[], SPOT_CAPACITY);
        assert_ne!(assignment[0], NO_SHADOW_SLOT);
    }

    #[test]
    fn dynamic_point_light_is_not_assigned_to_spot_pool() {
        let mut light = spot_light([0.0, 0.0, 0.0], 10.0, true);
        light.light_type = LightType::Point;
        let lights = vec![light];
        let assignment = rank_spot_lights(&lights, Vec3::ZERO, 0.1, &[], SPOT_CAPACITY);
        assert_eq!(assignment[0], NO_SHADOW_SLOT);
    }

    #[test]
    fn two_dynamic_spots_both_assigned() {
        let lights = vec![
            spot_light([0.0, 0.0, 0.0], 10.0, true),
            spot_light([10.0, 0.0, 0.0], 10.0, true),
        ];
        let assignment = rank_spot_lights(&lights, Vec3::ZERO, 0.1, &[], SPOT_CAPACITY);
        assert_ne!(assignment[0], NO_SHADOW_SLOT);
        assert_ne!(assignment[1], NO_SHADOW_SLOT);
        assert_ne!(assignment[0], assignment[1]);
    }

    #[test]
    fn spot_pool_assigns_all_candidates_within_capacity() {
        let lights: Vec<MapLight> = (0..9)
            .map(|i| spot_light([i as f64 * 10.0, 0.0, 0.0], 10.0, true))
            .collect();
        let assignment = rank_spot_lights(&lights, Vec3::ZERO, 0.1, &[], SPOT_CAPACITY);

        let assigned_count = assignment.iter().filter(|&&s| s != NO_SHADOW_SLOT).count();
        assert_eq!(assigned_count, 9);
    }

    #[test]
    fn closer_spot_light_ranks_higher() {
        let lights = vec![
            spot_light([0.0, 0.0, 0.0], 10.0, true),
            spot_light([100.0, 0.0, 0.0], 10.0, true),
        ];
        let assignment = rank_spot_lights(&lights, Vec3::ZERO, 0.1, &[], SPOT_CAPACITY);
        assert_eq!(assignment[0], 0);
        assert_eq!(assignment[1], 1);
    }

    #[test]
    fn larger_spot_falloff_ranks_higher() {
        let lights = vec![
            spot_light([0.0, 0.0, -10.0], 20.0, true),
            spot_light([0.0, 0.0, -10.0], 10.0, true),
        ];
        let assignment = rank_spot_lights(&lights, Vec3::ZERO, 0.1, &[], SPOT_CAPACITY);
        assert_eq!(assignment[0], 0);
        assert_eq!(assignment[1], 1);
    }

    #[test]
    fn equal_spot_scores_break_ties_by_light_index() {
        let lights = vec![
            spot_light([10.0, 0.0, 0.0], 10.0, true),
            spot_light([10.0, 0.0, 0.0], 10.0, true),
        ];
        let assignment = rank_spot_lights(&lights, Vec3::ZERO, 0.1, &[], SPOT_CAPACITY);
        assert_eq!(assignment[0], 0);
        assert_eq!(assignment[1], 1);
    }

    #[test]
    fn spot_eligibility_slice_culls_ineligible_lights() {
        let lights = vec![
            spot_light([0.0, 0.0, -10.0], 10.0, true),
            spot_light([10.0, 0.0, -10.0], 10.0, true),
            spot_light([20.0, 0.0, -10.0], 10.0, true),
        ];
        let assignment = rank_spot_lights(
            &lights,
            Vec3::ZERO,
            0.1,
            &[true, false, true],
            SPOT_CAPACITY,
        );
        assert_ne!(assignment[0], NO_SHADOW_SLOT);
        assert_eq!(assignment[1], NO_SHADOW_SLOT);
        assert_ne!(assignment[2], NO_SHADOW_SLOT);
    }

    #[test]
    fn spot_eligibility_slice_can_hide_closest_light() {
        let mut lights = vec![spot_light([0.0, 0.0, -1.0], 10.0, true)];
        for i in 1..9 {
            lights.push(spot_light([i as f64 * 50.0, 0.0, -10.0], 10.0, true));
        }
        let mut eligible = vec![true; 9];
        eligible[0] = false;
        let assignment = rank_spot_lights(&lights, Vec3::ZERO, 0.1, &eligible, SPOT_CAPACITY);

        assert_eq!(assignment[0], NO_SHADOW_SLOT);
        let assigned_count = assignment[1..]
            .iter()
            .filter(|&&s| s != NO_SHADOW_SLOT)
            .count();
        assert_eq!(assigned_count, 8);
    }

    #[test]
    fn empty_eligibility_slice_treated_as_all_spots_eligible() {
        let lights = vec![
            spot_light([0.0, 0.0, -10.0], 10.0, true),
            spot_light([10.0, 0.0, -10.0], 10.0, true),
            spot_light([20.0, 0.0, -10.0], 10.0, true),
        ];
        let assignment = rank_spot_lights(&lights, Vec3::ZERO, 0.1, &[], SPOT_CAPACITY);
        assert_ne!(assignment[0], NO_SHADOW_SLOT);
        assert_ne!(assignment[1], NO_SHADOW_SLOT);
        assert_ne!(assignment[2], NO_SHADOW_SLOT);
    }

    #[test]
    fn camera_near_clip_clamps_spot_score_denominator() {
        let lights = vec![spot_light([0.001, 0.0, 0.0], 10.0, true)];
        let assignment = rank_spot_lights(&lights, Vec3::ZERO, 0.1, &[], SPOT_CAPACITY);
        assert_eq!(assignment[0], 0);
    }

    #[test]
    fn spot_ranking_is_deterministic_for_fixed_camera_position() {
        let lights = vec![
            spot_light([0.0, 0.0, -10.0], 10.0, true),
            spot_light([500.0, 0.0, -10.0], 10.0, true),
            spot_light([-300.0, 50.0, 200.0], 10.0, true),
            spot_light([0.0, 200.0, 0.0], 10.0, true),
        ];
        let eye = Vec3::ZERO;

        let first = rank_spot_lights(&lights, eye, 0.1, &[], SPOT_CAPACITY);
        let second = rank_spot_lights(&lights, eye, 0.1, &[], SPOT_CAPACITY);

        assert_eq!(first, second);
    }

    #[test]
    fn spot_ranking_follows_camera_position_when_pool_overflows() {
        let near_a = Vec3::ZERO;
        let near_b = Vec3::new(100.0, 0.0, 0.0);
        let lights = vec![
            spot_light([0.0, 0.0, 0.0], 10.0, true),
            spot_light([100.0, 0.0, 0.0], 10.0, true),
        ];

        let from_a = rank_spot_lights(&lights, near_a, 0.1, &[], SPOT_CAPACITY);
        assert_eq!(from_a[0], 0);
        assert_eq!(from_a[1], 1);

        let from_b = rank_spot_lights(&lights, near_b, 0.1, &[], SPOT_CAPACITY);
        assert_eq!(from_b[1], 0);
        assert_eq!(from_b[0], 1);

        let score = |light: &MapLight, cam: Vec3| -> f32 {
            let p = Vec3::new(
                light.origin[0] as f32,
                light.origin[1] as f32,
                light.origin[2] as f32,
            );
            (light.falloff_range / (p - cam).length().max(0.1)).powi(2)
        };
        let cands_a: Vec<(usize, f32)> = lights
            .iter()
            .enumerate()
            .map(|(i, l)| (i, score(l, near_a)))
            .collect();
        let overflow_a = assign_ranked_slots(cands_a, 1, lights.len());
        assert_eq!(overflow_a[0], 0);
        assert_eq!(overflow_a[1], NO_SHADOW_SLOT);
    }

    #[test]
    fn dynamic_point_qualifies_baked_point_and_spot_do_not() {
        let mut spot = point_light([0.0, 0.0, 0.0], 10.0, true);
        spot.light_type = LightType::Spot;
        let lights = vec![
            point_light([0.0, 0.0, 0.0], 10.0, true),
            point_light([5.0, 0.0, 0.0], 10.0, false),
            spot,
        ];
        let assignment = rank_point_lights(&lights, Vec3::ZERO, 0.1, &[], CUBE_CAPACITY);
        assert_ne!(assignment[0], NO_SHADOW_SLOT);
        assert_eq!(assignment[1], NO_SHADOW_SLOT);
        assert_eq!(assignment[2], NO_SHADOW_SLOT);
    }

    #[test]
    fn closer_point_light_ranks_higher() {
        let lights = vec![
            point_light([0.0, 0.0, 0.0], 10.0, true),
            point_light([100.0, 0.0, 0.0], 10.0, true),
        ];
        let assignment = rank_point_lights(&lights, Vec3::ZERO, 0.1, &[], CUBE_CAPACITY);
        assert_eq!(assignment[0], 0);
        assert_eq!(assignment[1], 1);
    }

    #[test]
    fn point_overflow_drops_lowest_ranked_within_capacity() {
        let overflow = 4;
        let total = CUBE_CAPACITY + overflow;
        let lights: Vec<MapLight> = (0..total)
            .map(|i| point_light([i as f64 * 10.0, 0.0, 0.0], 10.0, true))
            .collect();
        let assignment = rank_point_lights(&lights, Vec3::ZERO, 0.1, &[], CUBE_CAPACITY);

        let assigned: Vec<u32> = assignment
            .iter()
            .copied()
            .filter(|&s| s != NO_SHADOW_SLOT)
            .collect();
        assert_eq!(assigned.len(), CUBE_CAPACITY);

        let mut sorted = assigned.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), assigned.len());
        assert!(assigned.iter().all(|&s| (s as usize) < CUBE_CAPACITY));

        for (i, slot) in assignment.iter().enumerate() {
            if i < CUBE_CAPACITY {
                assert_ne!(*slot, NO_SHADOW_SLOT);
            } else {
                assert_eq!(*slot, NO_SHADOW_SLOT);
            }
        }
    }

    #[test]
    fn point_eligibility_slice_culls_ineligible_lights() {
        let lights = vec![
            point_light([0.0, 0.0, 0.0], 10.0, true),
            point_light([50.0, 0.0, 0.0], 10.0, true),
        ];
        let assignment = rank_point_lights(&lights, Vec3::ZERO, 0.1, &[false, true], CUBE_CAPACITY);
        assert_eq!(assignment[0], NO_SHADOW_SLOT);
        assert_ne!(assignment[1], NO_SHADOW_SLOT);
    }

    #[test]
    fn empty_light_list_produces_empty_point_assignment() {
        let assignment = rank_point_lights(&[], Vec3::ZERO, 0.1, &[], CUBE_CAPACITY);
        assert!(assignment.is_empty());
    }

    // --- Unified tier-neutral hysteresis ranking -----------------------------

    const EVICTION_MARGIN: f32 = 1.25;

    fn dynamic_candidate(candidate_index: usize, score: f32) -> SlotCandidate {
        SlotCandidate {
            candidate_index,
            score,
            is_promoted_static: false,
        }
    }

    fn static_candidate(candidate_index: usize, score: f32) -> SlotCandidate {
        SlotCandidate {
            candidate_index,
            score,
            is_promoted_static: true,
        }
    }

    #[test]
    fn eviction_requires_one_and_quarter_score_margin() {
        assert!(!challenger_can_evict(1.24, 1.0, EVICTION_MARGIN));
        assert!(!challenger_can_evict(1.25, 1.0, EVICTION_MARGIN));
        assert!(challenger_can_evict(1.251, 1.0, EVICTION_MARGIN));
    }

    #[test]
    fn cap_full_stronger_static_challenger_evicts_weaker_static_incumbent() {
        // Regression: the promoted-static cap must not truncate a challenger
        // before the hysteresis comparison — a cap-full pool can still swap a
        // weaker static incumbent for a stronger static challenger.
        let incumbents = [SlotIncumbent {
            slot: 0,
            candidate_index: 0,
            score: 1.0,
            is_promoted_static: true,
        }];
        let challengers = [static_candidate(1, 1.251)];

        let assignment =
            assign_slots_with_hysteresis(&challengers, &incumbents, 1, 1, 2, EVICTION_MARGIN);

        assert_eq!(assignment[0], NO_SHADOW_SLOT);
        assert_eq!(assignment[1], 0);
    }

    #[test]
    fn dynamic_challenger_evicts_weaker_static_incumbent_on_margin() {
        // Even competition: a stronger dynamic challenger displaces a weaker
        // static incumbent (no static reservation shields it). The incumbent
        // score is the current-frame value supplied by the caller, so a camera
        // jump is reflected before the margin test.
        let incumbents = [SlotIncumbent {
            slot: 0,
            candidate_index: 0,
            score: 1.0,
            is_promoted_static: true,
        }];
        let challengers = [dynamic_candidate(1, 1.251)];

        let assignment =
            assign_slots_with_hysteresis(&challengers, &incumbents, 1, 1, 2, EVICTION_MARGIN);

        assert_eq!(assignment[0], NO_SHADOW_SLOT);
        assert_eq!(assignment[1], 0);
    }

    #[test]
    fn static_challenger_evicts_weaker_dynamic_incumbent_on_margin() {
        // The new capability the reservation blocked: a static challenger CAN
        // take a dynamic-held slot, subject to the same tier-neutral margin.
        let incumbents = [SlotIncumbent {
            slot: 0,
            candidate_index: 0,
            score: 1.0,
            is_promoted_static: false,
        }];
        let challengers = [static_candidate(1, 1.251)];

        let assignment =
            assign_slots_with_hysteresis(&challengers, &incumbents, 1, 1, 2, EVICTION_MARGIN);

        assert_eq!(assignment[0], NO_SHADOW_SLOT);
        assert_eq!(assignment[1], 0);
    }

    #[test]
    fn dynamic_incumbent_survives_challenger_within_margin() {
        // Tier-neutral hysteresis now protects dynamic incumbents too: a
        // dynamic challenger only barely stronger than a dynamic incumbent does
        // not clear the 1.25x margin, so the incumbent keeps its slot.
        let incumbents = [SlotIncumbent {
            slot: 0,
            candidate_index: 0,
            score: 1.0,
            is_promoted_static: false,
        }];
        let challengers = [dynamic_candidate(1, 1.1)];

        let assignment =
            assign_slots_with_hysteresis(&challengers, &incumbents, 1, 1, 2, EVICTION_MARGIN);

        assert_eq!(
            assignment[0], 0,
            "dynamic incumbent keeps its slot within the margin"
        );
        assert_eq!(assignment[1], NO_SHADOW_SLOT);
    }

    #[test]
    fn promoted_cap_blocks_static_from_taking_a_dynamic_slot() {
        // A cap-full static population may not grow by evicting a dynamic, even
        // when the static challenger vastly out-scores it: statics keep their
        // budget. The challenger instead swaps the weaker static.
        let incumbents = [
            SlotIncumbent {
                slot: 0,
                candidate_index: 0,
                score: 1.0,
                is_promoted_static: true,
            },
            SlotIncumbent {
                slot: 1,
                candidate_index: 1,
                score: 0.5,
                is_promoted_static: false,
            },
        ];
        let challengers = [static_candidate(2, 5.0)];

        let assignment =
            assign_slots_with_hysteresis(&challengers, &incumbents, 2, 1, 3, EVICTION_MARGIN);

        assert_eq!(
            assignment[0], NO_SHADOW_SLOT,
            "weaker static incumbent swapped out"
        );
        assert_eq!(assignment[1], 1, "dynamic slot untouched — cap holds");
        assert_eq!(
            assignment[2], 0,
            "static challenger swaps into the static slot only"
        );
    }

    #[test]
    fn free_slots_assign_by_score_without_reservation() {
        // With free slots and no incumbents, dynamic and static candidates
        // simply take slots in score order — no tier is reserved.
        let candidates = [static_candidate(0, 1.0), dynamic_candidate(1, 2.0)];
        let assignment = assign_slots_with_hysteresis(&candidates, &[], 2, 1, 2, EVICTION_MARGIN);
        // Higher-scored dynamic takes the first free slot; static takes the next.
        assert_eq!(assignment[1], 0);
        assert_eq!(assignment[0], 1);
    }

    // --- Restored camera-orientation regressions (Task 1 move) ---------------

    /// Build a `view_proj` from an eye, pitch, and yaw (radians), varying only
    /// the looking direction while holding position fixed. Used to document the
    /// swept camera pose in the orientation-invariance tests.
    fn camera_view_proj(eye: Vec3, pitch: f32, yaw: f32) -> glam::Mat4 {
        let forward = Vec3::new(
            yaw.sin() * pitch.cos(),
            pitch.sin(),
            -yaw.cos() * pitch.cos(),
        )
        .normalize();
        let world_up = if forward.y.abs() > 0.99 {
            Vec3::Z
        } else {
            Vec3::Y
        };
        let view = glam::Mat4::look_at_rh(eye, eye + forward, world_up);
        let proj = glam::Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.1, 4096.0);
        proj * view
    }

    /// Regression: a dynamic spot lost its shadow slot when its cone AABB left
    /// the pitched camera frustum (floor shadow vanished on pitch-down). Slot
    /// eligibility must not depend on camera orientation — only on the
    /// position-based score — so the light keeps its slot even though its cone
    /// AABB sits outside the pitched frustum.
    #[test]
    fn dynamic_spot_keeps_slot_when_cone_aabb_outside_pitched_camera_frustum() {
        use crate::light_space_matrix;
        use postretro_render_data::cone_frustum::{
            aabb_intersects_frustum, cone_enclosing_aabb, extract_frustum_planes_for_gpu,
        };

        // Light far off to the +X side, aimed straight down -Z.
        let lights = vec![spot_light([500.0, 0.0, -10.0], 10.0, true)];
        let camera_eye = Vec3::ZERO;
        // Camera at the origin pitched sharply down — its frustum points at the
        // floor near x=0 and does not contain the light's cone AABB.
        let pitched_down = camera_view_proj(camera_eye, -std::f32::consts::FRAC_PI_2 + 0.2, 0.0);
        let cone_aabb = cone_enclosing_aabb(&light_space_matrix(&lights[0]));
        let planes: [glam::Vec4; 6] = extract_frustum_planes_for_gpu(&pitched_down)
            .map(|p| glam::Vec4::new(p[0], p[1], p[2], p[3]));
        assert!(
            !aabb_intersects_frustum(&cone_aabb, &planes),
            "test precondition: cone AABB must sit outside the pitched camera frustum"
        );

        let assignment = rank_spot_lights(&lights, camera_eye, 0.1, &[], SPOT_CAPACITY);
        assert_ne!(
            assignment[0], NO_SHADOW_SLOT,
            "dynamic spot must keep its shadow slot regardless of where the camera looks"
        );
    }

    proptest! {
        /// The SET of lights receiving a shadow slot is invariant under camera
        /// orientation. Fix the lights and the camera position, sweep pitch and
        /// yaw, and assert the assigned-slot set never changes — the ranker
        /// consumes only the eye position, so orientation cannot drop a slot
        /// (the pitch-down "shadow vanished" symptom).
        #[test]
        fn shadow_slot_set_invariant_under_camera_orientation(
            pitch in -1.55f32..1.55,
            yaw in -std::f32::consts::PI..std::f32::consts::PI,
        ) {
            let lights = vec![
                spot_light([0.0, 0.0, -10.0], 10.0, true),
                spot_light([500.0, 0.0, -10.0], 10.0, true),
                spot_light([-300.0, 50.0, 200.0], 10.0, true),
                spot_light([0.0, 200.0, 0.0], 10.0, true),
            ];
            let eye = Vec3::ZERO;

            let baseline = rank_spot_lights(&lights, eye, 0.1, &[], SPOT_CAPACITY);
            let baseline_set: std::collections::BTreeSet<usize> = baseline
                .iter()
                .enumerate()
                .filter(|&(_, s)| *s != NO_SHADOW_SLOT)
                .map(|(i, _)| i)
                .collect();

            // Orientation only changes the (unused) view direction; the eye is
            // the only camera input the ranker consumes.
            let _ = camera_view_proj(eye, pitch, yaw);
            let swept = rank_spot_lights(&lights, eye, 0.1, &[], SPOT_CAPACITY);
            let swept_set: std::collections::BTreeSet<usize> = swept
                .iter()
                .enumerate()
                .filter(|&(_, s)| *s != NO_SHADOW_SLOT)
                .map(|(i, _)| i)
                .collect();

            prop_assert_eq!(
                swept_set,
                baseline_set,
                "the set of shadow-slot lights must not change with camera orientation",
            );
        }
    }
}
