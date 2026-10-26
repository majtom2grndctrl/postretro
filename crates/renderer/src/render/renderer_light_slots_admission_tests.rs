// Admission rank at the renderer's ranking seam: tags, incumbency and promoted seats (AC 22, O7, O15).
// See: context/lib/rendering_pipeline.md §7.3a (Pool rank)

use super::*;
use postretro_lighting::NO_SHADOW_SLOT;
use postretro_lighting::shadow_ranking::slot_score;

const NEAR: f32 = 0.1;

/// A spot `distance` metres down +x from the camera at the origin, of
/// `range`: its slot score is `(range / distance)²`.
fn spot(distance: f64, range: f32, dynamic: bool) -> MapLight {
    MapLight {
        origin: [distance, 0.0, 0.0],
        light_type: postretro_level_loader::LightType::Spot,
        intensity: 1.0,
        color: [1.0; 3],
        falloff_model: postretro_level_loader::FalloffModel::Linear,
        falloff_range: range,
        cone_angle_inner: 0.3,
        cone_angle_outer: 0.4,
        cone_direction: [0.0, 0.0, -1.0],
        is_dynamic: dynamic,
        casts_entity_shadows: true,
        animated_slot: None,
        tags: vec![],
        cell_index: 0,
        shadow_type: postretro_level_loader::ShadowType::StaticLightMap,
    }
}

fn score(light: &MapLight) -> f32 {
    slot_score(light.falloff_range, light.origin[0] as f32, NEAR)
}

/// A promotion holding `slot` at weight `weight`, its gate failed this frame
/// but its sticky window still open.
fn promoted(slot: u32, weight: f32) -> PromotedBakedLightState {
    PromotedBakedLightState {
        weight,
        sticky_remaining: 0.2,
        pool_kind: Some(PromotedShadowPoolKind::Spot),
        slot,
        last_score: 1.0,
    }
}

/// One frame's renderer-side ranking inputs.
struct Frame<'a> {
    lights: &'a [MapLight],
    selection: &'a [Option<usize>],
    animated: &'a [Option<usize>],
    static_states: &'a [PromotedBakedLightState],
    animated_states: &'a [PromotedBakedLightState],
    prior: &'a [u32],
    eligible: &'a [bool],
    mirror_only: &'a [bool],
    held: &'a [bool],
    capacity: usize,
    promoted_cap: usize,
}

impl Frame<'_> {
    fn rank(&self) -> Vec<u32> {
        let lookup = build_promotion_candidate_lookup(
            self.selection,
            self.animated,
            self.static_states.len(),
            self.animated_states.len(),
        );
        assign_shadow_pool_slots_with_promoted_baked(
            ShadowRankingState {
                candidate_lights: self.lights,
                selection_indices: self.selection,
                animated_baked_indices: self.animated,
                promoted_static_states: self.static_states,
                promoted_animated_states: self.animated_states,
                prior_assignment: self.prior,
            },
            ShadowPoolSlotAssignmentInputs {
                pool_kind: PromotedShadowPoolKind::Spot,
                camera_position: Vec3::ZERO,
                camera_near_clip: NEAR,
                eligible_lights: self.eligible,
                capacity: self.capacity,
                promoted_cap: self.promoted_cap,
                promotion_candidate_lookup: &lookup,
                admission: AdmissionTags {
                    mirror_only: self.mirror_only,
                    held_mirror_only: self.held,
                },
            },
        )
    }
}

fn dynamic_frame<'a>(
    lights: &'a [MapLight],
    prior: &'a [u32],
    eligible: &'a [bool],
    mirror_only: &'a [bool],
    held: &'a [bool],
    capacity: usize,
) -> Frame<'a> {
    Frame {
        lights,
        selection: &[],
        animated: &[],
        static_states: &[],
        animated_states: &[],
        prior,
        eligible,
        mirror_only,
        held,
        capacity,
        promoted_cap: 0,
    }
}

// O7, dynamic: on frame N−1 a mirror-only light held a slot beside a
// main-view one in a two-slot pool; on frame N a second main-view light
// needs a slot, scoring under the eviction margin over the mirror-only
// incumbent (and under it). It takes the slot, and the frame's assignment is
// the one the planar-off renderer makes, where the mirror-only light never
// held a slot.
#[test]
fn mirror_only_dynamic_incumbent_yields_to_a_main_light_under_the_margin() {
    let lights = [
        spot(2.0, 10.0, true),
        spot(4.0, 10.0, true),
        spot(3.8, 10.0, true),
    ];
    assert!(score(&lights[1]) < score(&lights[2]) * EVICTION_MARGIN);
    assert!(score(&lights[1]) < score(&lights[2]));

    let planar = dynamic_frame(
        &lights,
        &[0, NO_SHADOW_SLOT, 1],
        &[true, true, true],
        &[false, false, true],
        &[false, false, true],
        2,
    )
    .rank();
    let planar_off = dynamic_frame(
        &lights,
        &[0, NO_SHADOW_SLOT, NO_SHADOW_SLOT],
        &[true, true, false],
        &[],
        &[],
        2,
    )
    .rank();

    assert_eq!(planar, vec![0, 1, NO_SHADOW_SLOT]);
    assert_eq!(
        planar, planar_off,
        "the main view's assignment is planar-off's"
    );

    // Control: the same light held as the main view's keeps the slot.
    let main_held = dynamic_frame(
        &lights,
        &[0, NO_SHADOW_SLOT, 1],
        &[true, true, true],
        &[],
        &[],
        2,
    )
    .rank();
    assert_eq!(main_held, vec![0, NO_SHADOW_SLOT, 1]);
}

// O15: a light that held a main-view slot on frame N−1 and turned
// mirror-only on frame N keeps no incumbency against a main-view light,
// which takes the slot under the margin; and a light that held a mirror-only
// slot on frame N−1 and turned main-view on frame N competes as a fresh
// candidate, exactly as without the mirror, where it held nothing.
#[test]
fn reach_changes_between_frames_rank_by_frame_n_reaches() {
    let lights = [spot(2.0, 10.0, true), spot(2.1, 10.0, true)];
    assert!(score(&lights[1]) * EVICTION_MARGIN > score(&lights[0]));

    // Light 0 was the main view's, is the mirror's now.
    let turned_mirror_only = dynamic_frame(
        &lights,
        &[0, NO_SHADOW_SLOT],
        &[true, true],
        &[true, false],
        &[false, false],
        1,
    )
    .rank();
    assert_eq!(turned_mirror_only, vec![NO_SHADOW_SLOT, 0]);

    // Light 1 was the mirror's, is the main view's now; light 0 is new and
    // stronger within the margin.
    let lights = [spot(2.0, 10.0, true), spot(2.1, 10.0, true)];
    let turned_main = dynamic_frame(
        &lights,
        &[NO_SHADOW_SLOT, 0],
        &[true, true],
        &[false, false],
        &[false, true],
        1,
    )
    .rank();
    let planar_off = dynamic_frame(
        &lights,
        &[NO_SHADOW_SLOT, NO_SHADOW_SLOT],
        &[true, true],
        &[],
        &[],
        1,
    )
    .rank();
    assert_eq!(turned_main, planar_off);
    assert_eq!(turned_main, vec![0, NO_SHADOW_SLOT]);
}

// O7, promoted-static inside its sticky window and animated-baked: a baked
// light the mirror alone held from frame N−1 — no longer a candidate, its
// weight still above zero — yields its slot to a weaker main-view dynamic
// light. Held as the main view's, the same sticky light keeps it.
#[test]
fn mirror_only_sticky_promoted_and_animated_incumbents_yield_to_main_lights() {
    let lights = [spot(2.0, 10.0, false), spot(4.0, 10.0, true)];
    for animated in [false, true] {
        let (selection, animated_indices): (&[Option<usize>], &[Option<usize>]) = if animated {
            (&[None, None], &[Some(0), None])
        } else {
            (&[Some(0), None], &[None, None])
        };
        let states = [promoted(0, 0.5)];
        let frame = |held: &'static [bool]| Frame {
            lights: &lights,
            selection,
            animated: animated_indices,
            static_states: if animated { &states[..0] } else { &states[..] },
            animated_states: if animated { &states[..] } else { &states[..0] },
            prior: &[0, NO_SHADOW_SLOT],
            eligible: &[false, true],
            mirror_only: &[false, false],
            held,
            capacity: 1,
            promoted_cap: 1,
        };
        assert_eq!(
            frame(&[true, false]).rank(),
            vec![NO_SHADOW_SLOT, 0],
            "animated {animated}: the main-view light takes the mirror's seat"
        );
        assert_eq!(
            frame(&[false, false]).rank(),
            vec![0, NO_SHADOW_SLOT],
            "animated {animated}: the main view's own sticky light keeps it"
        );
    }
}

// O7, promoted cap: mirror-only baked lights hold the promoted cap from frame
// N−1; a weaker main-view baked light still takes a promoted seat on frame N,
// and the mirror keeps only the seats the main view leaves.
#[test]
fn main_baked_light_takes_a_promoted_seat_mirror_only_baked_lights_hold() {
    let lights = [
        spot(2.0, 10.0, false),
        spot(2.0, 10.0, false),
        spot(6.0, 10.0, false),
    ];
    let states = [
        promoted(0, 1.0),
        promoted(1, 1.0),
        PromotedBakedLightState::default(),
    ];
    let assignment = Frame {
        lights: &lights,
        selection: &[Some(0), Some(1), Some(2)],
        animated: &[None, None, None],
        static_states: &states,
        animated_states: &[],
        prior: &[0, 1, NO_SHADOW_SLOT],
        eligible: &[true, true, true],
        mirror_only: &[true, true, false],
        held: &[true, true, false],
        capacity: 4,
        promoted_cap: 2,
    }
    .rank();

    assert_ne!(
        assignment[2], NO_SHADOW_SLOT,
        "the main-view baked light promotes"
    );
    assert_eq!(
        assignment[..2]
            .iter()
            .filter(|&&slot| slot != NO_SHADOW_SLOT)
            .count(),
        1,
        "one promoted seat is left for the mirror"
    );
}

// AC 22: a pool full of main-view lights, a main-view shadow caster among
// them, is slot for slot the planar-off assignment when stronger mirror-only
// lights join; with room, a mirror-only light takes the free slot.
#[test]
fn full_pool_of_main_view_lights_matches_planar_off_and_room_admits_the_mirror() {
    let lights: Vec<MapLight> = (0..5)
        .map(|index| spot(2.0 + index as f64, 10.0, true))
        .chain([spot(1.0, 10.0, true), spot(1.1, 10.0, true)])
        .collect();
    let eligible = [true; 7];
    let mirror_only = [false, false, false, false, false, true, true];
    let prior = [2, NO_SHADOW_SLOT, 0, NO_SHADOW_SLOT, 1, 3, 4];
    let held = [false, false, false, false, false, true, true];
    for capacity in [5, 6] {
        let planar =
            dynamic_frame(&lights, &prior, &eligible, &mirror_only, &held, capacity).rank();
        let planar_off = dynamic_frame(
            &lights[..5],
            &prior[..5],
            &eligible[..5],
            &[],
            &[],
            capacity,
        )
        .rank();
        assert_eq!(&planar[..5], planar_off.as_slice(), "capacity {capacity}");
        let mirror_seats = planar[5..]
            .iter()
            .filter(|&&slot| slot != NO_SHADOW_SLOT)
            .count();
        assert_eq!(mirror_seats, capacity - 5, "capacity {capacity}");
    }
}

// The seats held after ranking: a main-view light holds its slot as the main
// view's, a mirror-only light as the mirror's, and a sticky light neither
// view reaches at the rank it held.
#[test]
fn held_seats_follow_frame_n_reach_and_keep_a_sticky_lights_rank() {
    let spot_assignment = [0, 1, 2, 3, NO_SHADOW_SLOT];
    let cube_assignment = [NO_SHADOW_SLOT; 5];
    let eligible = [true, true, false, false, true];
    let mirror_only = [false, true, false, false, true];
    let mut held = vec![true, false, true, false, true];
    update_held_as_mirror_only(
        &spot_assignment,
        &cube_assignment,
        &eligible,
        &mirror_only,
        &mut held,
    );
    assert_eq!(held, [false, true, true, false, false]);
}
