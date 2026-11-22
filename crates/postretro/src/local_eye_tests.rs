//! Eye hand-offs between the drawn deck and the world offset: the eye moves
//! only by its own motion when a glide is picked up, dropped or lost, and cuts
//! only when the deck snaps.

use glam::{Quat, Vec3};
use postretro_entities::{EntityId, EntityRegistry, Transform};

use super::{
    apply_authoritative_mover_corrections, drawn_deck_view_yaw, hand_off_eye_across_apply,
    next_view_yaw_handoff, push_eye_endpoint,
};
use crate::frame_timing::{EyeBody, EyeMover, FrameTiming, InterpolableState, present_eye};

const MOVER: u32 = 4;
const EYE_HEIGHT: f32 = 0.7;
const LOCAL: Vec3 = Vec3::new(2.0, 1.0, 0.0);
const ALPHAS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

fn at(x: f32, yaw: f32) -> Transform {
    Transform {
        position: Vec3::new(x, 0.0, 0.0),
        rotation: Quat::from_rotation_y(yaw),
        scale: Vec3::ONE,
    }
}

fn on(pose: &Transform) -> Vec3 {
    pose.position + pose.rotation * LOCAL
}

/// A mover one tick into a move, gliding onto a correction of 0.2 m and
/// 0.1 rad, as a connected client draws it after the snapshot lands.
fn gliding_mover() -> (EntityRegistry, EntityId) {
    let mut registry = EntityRegistry::new();
    let mover = registry.spawn(at(0.0, 0.0));
    registry.snapshot_transforms();
    registry.set_component(mover, at(0.1, 0.05)).unwrap();
    registry
        .correct_transform(mover, at(0.3, 0.15), true)
        .unwrap();
    (registry, mover)
}

/// An eye endpoint for a pawn standing at `LOCAL` on the mover at `pose`, or
/// at that point with no mover underfoot.
fn endpoint(mover: Option<(EntityId, Transform)>, centre: Vec3, offset: Vec3) -> InterpolableState {
    InterpolableState::following(
        centre + offset + Vec3::Y * EYE_HEIGHT,
        EyeBody {
            centre,
            offset,
            mover: mover.map(|(entity, pose)| EyeMover {
                mover_id: MOVER,
                entity,
                pose,
            }),
        },
    )
}

fn timing(previous: InterpolableState, current: InterpolableState) -> FrameTiming {
    let mut timing = FrameTiming::new(InterpolableState::new(Vec3::ZERO));
    timing.push_state(previous);
    timing.push_state(current);
    timing
}

/// The eye the old interval ends on at a tick boundary, with the mover at its
/// previous slot: the pose that interval ends on.
fn boundary_end(timing: &FrameTiming, registry: &EntityRegistry) -> (Vec3, Option<Vec3>) {
    let eye = present_eye(
        &timing.previous_state,
        &timing.current_state,
        1.0,
        |mover| registry.interpolated_transform(mover, 0.0).ok(),
    );
    (eye.eye, eye.body)
}

fn eye_at(timing: &FrameTiming, registry: &EntityRegistry, alpha: f32) -> (Vec3, Option<Vec3>) {
    let eye = present_eye(
        &timing.previous_state,
        &timing.current_state,
        alpha,
        |mover| registry.interpolated_transform(mover, alpha).ok(),
    );
    (eye.eye, eye.body)
}

// Pin P3: a pawn standing on a gliding mover keeps its eye on its point of the
// drawn deck at every alpha, glide frame after glide frame, with no fixed tick.
#[test]
fn eye_stays_on_the_drawn_deck_through_a_glide() {
    let (mut registry, mover) = gliding_mover();
    // Endpoints pushed before the correction, on the uncorrected deck.
    let timing = timing(
        endpoint(Some((mover, at(0.0, 0.0))), on(&at(0.0, 0.0)), Vec3::ZERO),
        endpoint(Some((mover, at(0.1, 0.05))), on(&at(0.1, 0.05)), Vec3::ZERO),
    );
    for _ in 0..4 {
        for alpha in ALPHAS {
            let (eye, body) = eye_at(&timing, &registry, alpha);
            let deck = registry.interpolated_transform(mover, alpha).unwrap();
            assert!((eye - (on(&deck) + Vec3::Y * EYE_HEIGHT)).length() < 1.0e-4);
            assert!(
                (body.unwrap() - on(&deck)).length() < 1.0e-4,
                "mesh on the deck"
            );
        }
        crate::netcode::advance_mover_correction_glides(&mut registry, 1.0 / 60.0);
    }
}

/// The mover's next fixed tick on the corrected path: previous takes the
/// corrected pose, current moves 0.2 m and 0.1 rad on; the glide stays.
fn tick(registry: &mut EntityRegistry, mover: EntityId, next: Transform) {
    registry.snapshot_transforms();
    registry.set_component(mover, next).unwrap();
}

// Pins P15 and P4: stepping off a gliding deck, or onto one, moves the eye and
// the pawn mesh by no more than their own motion, and the eye keeps the drawn
// deck's place as the hand-off decays.
#[test]
fn leaving_or_landing_on_a_gliding_deck_hands_the_glide_to_the_world_offset() {
    let riding = |mover, pose: Transform| endpoint(Some((mover, pose)), on(&pose), Vec3::ZERO);
    let afoot = |centre: Vec3| endpoint(None, centre, Vec3::ZERO);

    // Leaving: the last interval on the deck was pushed on the corrected path.
    let (mut registry, mover) = gliding_mover();
    let mut leaving = timing(riding(mover, at(0.1, 0.05)), riding(mover, at(0.3, 0.15)));
    tick(&mut registry, mover, at(0.5, 0.25));
    let before = boundary_end(&leaving, &registry);
    let handed = push_eye_endpoint(&mut leaving, afoot(on(&at(0.5, 0.25))), &registry);
    assert!(handed.length() > 0.05, "the glide was in flight: {handed}");
    let after = eye_at(&leaving, &registry, 0.0);
    assert!(
        (after.0 - before.0).length() < 1.0e-4,
        "eye {:?} vs {:?}",
        after.0,
        before.0
    );
    assert!(
        (after.1.unwrap() - before.1.unwrap()).length() < 1.0e-4,
        "mesh"
    );
    // A tick on, the eye stands where the deck now draws that point.
    let drawn = registry.interpolated_transform(mover, 1.0).unwrap();
    let end = eye_at(&leaving, &registry, 1.0).0;
    assert!(
        (end - (on(&drawn) + Vec3::Y * EYE_HEIGHT)).length() < 0.02,
        "{end}"
    );

    // Landing: the first interval with the deck on both ends attaches.
    let (mut registry, mover) = gliding_mover();
    let mut landing = timing(afoot(on(&at(0.1, 0.05))), riding(mover, at(0.3, 0.15)));
    tick(&mut registry, mover, at(0.5, 0.25));
    let before = boundary_end(&landing, &registry);
    let handed = push_eye_endpoint(&mut landing, riding(mover, at(0.5, 0.25)), &registry);
    assert!(handed.length() > 0.05);
    let after = eye_at(&landing, &registry, 0.0);
    assert!((after.0 - before.0).length() < 1.0e-4);
    assert!((after.1.unwrap() - before.1.unwrap()).length() < 1.0e-4);
}

// With no glide in flight, attaching meets exactly: nothing is handed off.
#[test]
fn attaching_without_a_glide_hands_off_nothing() {
    let mut registry = EntityRegistry::new();
    let mover = registry.spawn(at(0.0, 0.0));
    tick(&mut registry, mover, at(0.1, 0.05));
    let riding = |pose: Transform| endpoint(Some((mover, pose)), on(&pose), Vec3::ZERO);
    let mut timing = timing(
        endpoint(None, on(&at(0.0, 0.0)), Vec3::ZERO),
        riding(at(0.1, 0.05)),
    );
    tick(&mut registry, mover, at(0.2, 0.1));
    let handed = push_eye_endpoint(&mut timing, riding(at(0.2, 0.1)), &registry);
    assert!(handed.length() < 1.0e-5, "{handed}");
}

// Pins P12 and P16: a mover that despawns under a standing pawn mid-glide, on
// a frame that runs no tick, hands the glide at the eye into the world offset
// before it leaves, so the eye does not step at any alpha.
#[test]
fn losing_a_gliding_mover_hands_its_glide_to_the_eye_at_every_alpha() {
    for alpha in ALPHAS {
        let (mut registry, mover) = gliding_mover();
        let mut timing = timing(
            endpoint(Some((mover, at(0.0, 0.0))), on(&at(0.0, 0.0)), Vec3::ZERO),
            endpoint(Some((mover, at(0.1, 0.05))), on(&at(0.1, 0.05)), Vec3::ZERO),
        );
        let presented = |timing: &FrameTiming, registry: &EntityRegistry| {
            present_eye(&timing.previous_state, &timing.current_state, alpha, |m| {
                registry.interpolated_transform(m, alpha).ok()
            })
        };
        let before = presented(&timing, &registry);
        registry.despawn(mover).unwrap();
        let after = presented(&timing, &registry);
        let mut prediction = crate::netcode::ClientPrediction::default();
        hand_off_eye_across_apply(&mut timing, &mut prediction, before, after, &[]);
        let now = presented(&timing, &registry);
        assert!((now.eye - before.eye).length() < 1.0e-5, "alpha {alpha}");
        assert!((prediction.presentation_offset() - (before.eye - after.eye)).length() < 1.0e-6);
    }
}

// Pin P7: a deck that snaps under the eye cuts the eye to the snapped deck on
// the same frame, with nothing left to blend from.
#[test]
fn snapping_deck_cuts_the_eye() {
    let (mut registry, mover) = gliding_mover();
    let mut timing = timing(
        endpoint(Some((mover, at(0.0, 0.0))), on(&at(0.0, 0.0)), Vec3::ZERO),
        endpoint(Some((mover, at(0.1, 0.05))), on(&at(0.1, 0.05)), Vec3::ZERO),
    );
    let presented = |timing: &FrameTiming, registry: &EntityRegistry| {
        present_eye(&timing.previous_state, &timing.current_state, 0.5, |m| {
            registry.interpolated_transform(m, 0.5).ok()
        })
    };
    let before = presented(&timing, &registry);
    registry.clear_presentation_correction(mover);
    let after = presented(&timing, &registry);
    let mut prediction = crate::netcode::ClientPrediction::default();
    hand_off_eye_across_apply(&mut timing, &mut prediction, before, after, &[MOVER]);
    assert!(timing.previous_state.body.is_none() && timing.current_state.body.is_none());
    assert_eq!(timing.current_state.position, after.eye);
    assert_eq!(prediction.presentation_offset(), Vec3::ZERO);
}

// Pin P20: with carry_yaw, leaving, landing on or losing a gliding deck moves
// the glide yaw the view drew into the render-only hand-off, so the drawn view
// does not turn; staying on one deck hands off nothing, and it decays.
#[test]
fn view_yaw_hands_off_a_gliding_carry_yaw_deck() {
    let dt = 1.0 / 60.0;
    // Leaving: drawn glide 0.2 rad on mover 4, none now.
    let left = next_view_yaw_handoff(0.0, (Some(4), 0.2), (None, 0.0, false), dt);
    assert!((left - 0.2).abs() < 1.0e-6);
    // Landing on a deck gliding 0.15 rad.
    let landed = next_view_yaw_handoff(0.0, (None, 0.0), (Some(4), 0.15, true), dt);
    assert!((landed + 0.15).abs() < 1.0e-6);
    // Same deck, its own glide shrinking: no hand-off.
    assert_eq!(
        next_view_yaw_handoff(0.0, (Some(4), 0.2), (Some(4), 0.1, true), dt),
        0.0
    );
    // Same deck, its glide gone (despawned, rebound): the drawn yaw hands off.
    let lost = next_view_yaw_handoff(0.0, (Some(4), 0.2), (Some(4), 0.0, false), dt);
    assert!((lost - 0.2).abs() < 1.0e-6);
    // In flight, it decays.
    let decayed = next_view_yaw_handoff(0.2, (None, 0.0), (None, 0.0, false), dt);
    assert!(decayed < 0.2 && decayed > 0.0);
}

// Pin P13: a carry_yaw correction that changes the spin rate lands on a frame
// that runs no fixed tick. The settled yaw takes the tick-start correction and
// the table the corrected tick turn, while the drawn deck glides on its
// predicted turn; the view keeps its yaw on the drawn deck at every alpha.
#[test]
fn carry_yaw_view_stays_on_the_drawn_deck_when_a_correction_changes_spin() {
    use crate::kinematic_mover::{MoverTickState, MoverTickStateTable};
    let state = |end_yaw: f32, turn: f32, entity| MoverTickState {
        entity,
        transform: at(0.0, end_yaw),
        linear_velocity: Vec3::ZERO,
        tick_delta: Vec3::ZERO,
        angular_velocity: Vec3::Y,
        tick_rotation_delta: Quat::from_rotation_y(turn),
        carry_yaw: true,
        tick_dt: 1.0 / 60.0,
    };
    let mut registry = EntityRegistry::new();
    let mover = registry.spawn(at(0.0, 0.0));
    registry.snapshot_transforms();
    // Predicted: +0.03 rad this tick. The host reversed it: -0.02 rad, ending
    // 0.05 rad back from the prediction.
    registry.set_component(mover, at(0.0, 0.03)).unwrap();
    let mut table = MoverTickStateTable::default();
    table.publish(MOVER, state(0.03, 0.03, mover));
    let mut camera = crate::camera::Camera::new(Vec3::ZERO, 1.0, 0.0);
    let deck_yaw = |registry: &EntityRegistry, alpha: f32| {
        super::world_up_yaw_delta(
            registry
                .interpolated_transform(mover, alpha)
                .unwrap()
                .rotation,
        )
    };
    let view_minus_deck = |camera: &crate::camera::Camera,
                           table: &MoverTickStateTable,
                           registry: &EntityRegistry,
                           alpha: f32| {
        let drawn = registry.interpolated_transform(mover, alpha).unwrap();
        drawn_deck_view_yaw(camera.yaw, table.get(MOVER).unwrap(), &drawn).unwrap()
            - deck_yaw(registry, alpha)
    };
    let before = view_minus_deck(&camera, &table, &registry, 0.3);
    registry
        .correct_transform(mover, at(0.0, -0.02), true)
        .unwrap();
    let correction = crate::netcode::MoverCorrection {
        pre_correction: at(0.0, 0.03),
        applied: None,
        network_id: postretro_net::wire::NetworkId(1),
        mover_id: MOVER,
        magnitude: 0.0,
        authoritative_state: state(-0.02, -0.02, mover),
    };
    apply_authoritative_mover_corrections(
        &mut camera,
        postretro_foundation::GroundRef::Mover(MOVER),
        &mut table,
        &[correction],
    );
    for alpha in ALPHAS {
        let after = view_minus_deck(&camera, &table, &registry, alpha);
        assert!(
            (after - before).abs() < 1.0e-5,
            "alpha {alpha}: {after} vs {before}"
        );
    }
}

// Pin P6: an apply that cut the eye (a teleport adopted while riding held both
// endpoints at the arrival) carries nothing across: no shift, no offset.
#[test]
fn apply_time_cut_while_riding_stays_a_cut() {
    let (registry, mover) = gliding_mover();
    let mut timing = timing(
        endpoint(Some((mover, at(0.0, 0.0))), on(&at(0.0, 0.0)), Vec3::ZERO),
        endpoint(Some((mover, at(0.1, 0.05))), on(&at(0.1, 0.05)), Vec3::ZERO),
    );
    let presented = |timing: &FrameTiming| {
        present_eye(&timing.previous_state, &timing.current_state, 0.5, |m| {
            registry.interpolated_transform(m, 0.5).ok()
        })
    };
    let before = presented(&timing);
    let arrival = Vec3::new(40.0, 2.0, -7.0);
    timing.hold_state(InterpolableState::new(arrival));
    let after = presented(&timing);
    let mut prediction = crate::netcode::ClientPrediction::default();
    hand_off_eye_across_apply(&mut timing, &mut prediction, before, after, &[]);
    assert_eq!(timing.current_state.position, arrival);
    assert_eq!(timing.previous_state.position, arrival);
    assert_eq!(prediction.presentation_offset(), Vec3::ZERO);
}
