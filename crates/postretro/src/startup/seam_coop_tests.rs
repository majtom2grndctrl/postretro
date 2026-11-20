//! Co-op seam crossings at the host: each remote seat's arrival through the
//! spawn-pose resolver, its hint, its participation-entry spawn, and the
//! fallbacks and re-entries around it.
//! See: context/lib/entity_model.md §7e · context/lib/networking.md §Slot lifecycle ·
//! context/plans/in-progress/E25--landmark-level-change/research.md §6

use super::lifecycle_test_fixtures::script_ctx;
use super::seam_test_fixtures::{
    DESTINATION, DESTINATION_LANDMARK, DESTINATION_YAW, DOOR, THIRD, destination_body,
    destination_door, seam_world, spawn_points,
};
use super::spawn_pose_handoff_fixtures::{Relay, standing_on};
use super::*;
use crate::seam_crossing::SeamCrossing;
use crate::spawn_pose::ResolvedSpawn;
use glam::Quat;
use postretro_entities::EntityId;
use postretro_foundation::PlayerMovementComponent;
use postretro_net::wire::SpawnPoseHint;
use postretro_physics::movement::seam_arrival::LandmarkRelative;
use postretro_sim::scripting::map_entity::MapEntity;
use postretro_test_log_capture::LogCapture;

const INSIDE: u64 = 41;
const OUTSIDE: u64 = 42;
const SCOUT_EYE: f32 = 0.9;

/// Four starts; every joiner start spawns a `scout`.
fn points() -> Vec<MapEntity> {
    let mut points = spawn_points(4);
    for point in &mut points[1..] {
        point
            .key_values
            .insert("entity_class".into(), "scout".into());
    }
    points
}

/// A crossing client's motion: turned, pitched and moving.
fn motion() -> LandmarkRelative {
    let mut motion = LandmarkRelative::at_rest(Vec3::new(1.0, 1.2, -0.5), 0.3, 0.15);
    motion.velocity = Vec3::new(0.0, 0.0, 4.0);
    motion
}

/// A crossing client's motion whose arrival body lands on `body`.
fn motion_onto(body: Vec3) -> LandmarkRelative {
    let offset = Quat::from_rotation_y(-DESTINATION_YAW) * (body - DESTINATION_LANDMARK);
    LandmarkRelative::at_rest(offset, 0.3, 0.15)
}

/// A host with `clients` admitted, whose unload harvested `crossing` for the
/// seat of `INSIDE`, installing the destination.
fn crossed(clients: &[u64], arrival: LandmarkRelative) -> (Relay, EntityId) {
    let mut relay = Relay::admitted(clients);
    let seat = relay.seat(INSIDE).unwrap();
    let mut crossing = SeamCrossing::empty(DOOR);
    crossing.arrivals.insert(seat, arrival);
    relay.app.seam_crossing = Some(crossing);
    let (host, _) = relay
        .install_seam_level(
            DESTINATION,
            seam_world(vec![destination_door()], &points()),
            points(),
        )
        .unwrap();
    (relay, host)
}

/// The hint and pose an arrival through the destination door presents.
fn arrival_eye(arrival: &LandmarkRelative) -> Vec3 {
    destination_body(arrival) + Vec3::Y * SCOUT_EYE
}

#[track_caller]
fn assert_hint_at_arrival(hint: &SpawnPoseHint, arrival: &LandmarkRelative) {
    assert!(
        Vec3::from_array(hint.eye).distance(arrival_eye(arrival)) < 1e-4,
        "hint eye {:?} is the arrival eye {:?}",
        hint.eye,
        arrival_eye(arrival)
    );
    let yaw = postretro_foundation::wrap_teleport_yaw(arrival.view_yaw + DESTINATION_YAW);
    assert!((hint.yaw - yaw).abs() < 1e-5, "arrival yaw");
    assert!(
        (hint.pitch - arrival.view_pitch).abs() < 1e-6,
        "arrival pitch"
    );
}

#[track_caller]
fn assert_hint_not_at_arrival(hint: &SpawnPoseHint, arrival: &LandmarkRelative) {
    assert!(
        Vec3::from_array(hint.eye).distance(arrival_eye(arrival)) > 1.0,
        "hint {:?} names a start, not the arrival",
        hint.eye
    );
}

fn pawn_movement(relay: &Relay, pawn: EntityId) -> PlayerMovementComponent {
    script_ctx(&relay.app)
        .registry
        .borrow()
        .get_component::<PlayerMovementComponent>(pawn)
        .unwrap()
        .clone()
}

fn eye(relay: &Relay, pawn: EntityId) -> Vec3 {
    relay.eye(pawn)
}

/// Reveal `client_id` and the host, and wait for its pawn.
fn promote(relay: &mut Relay, client_id: u64) -> EntityId {
    relay.reveal(client_id);
    relay.host_reveal();
    relay.await_pawn(client_id)
}

// Coop 1, Coop 2 and Comp 2: a client inside the seam at the harvest resolves
// to its arrival, holds no start, is hinted the arrival eye, yaw and pitch, and
// spawns there at participation entry as the class of the start it would
// otherwise take, with its motion, latched, its `arrive` queued. A client
// outside takes its start.
#[test]
fn seam_client_inside_arrives_at_participation_entry_and_outside_takes_its_start() {
    let arrival = motion();
    let (mut relay, _) = crossed(&[INSIDE, OUTSIDE], arrival.clone());
    let inside_seat = relay.seat(INSIDE).unwrap();

    let hint = relay.await_hint(INSIDE);
    assert_hint_at_arrival(&hint, &arrival);
    assert_eq!(relay.placement(INSIDE), None, "an arrival holds no start");
    let Some(ResolvedSpawn::Unbacked(unbacked)) = relay.app.spawn_poses.recorded(inside_seat)
    else {
        panic!("the arriving seat resolves to an unbacked pose");
    };
    assert_eq!(
        unbacked.entity_class, "scout",
        "the class of the start it would otherwise take"
    );
    let outside_hint = relay.await_hint(OUTSIDE);
    assert_hint_not_at_arrival(&outside_hint, &arrival);
    assert_eq!(relay.placement(OUTSIDE), Some(1));

    let pawn = promote(&mut relay, INSIDE);
    assert!(eye(&relay, pawn).distance(arrival_eye(&arrival)) < 1e-4);
    let moved = pawn_movement(&relay, pawn);
    assert!((moved.capsule.radius - 0.3).abs() < 1e-6, "a scout pawn");
    let rotated = Quat::from_rotation_y(DESTINATION_YAW) * Vec3::new(0.0, 0.0, 4.0);
    assert!(
        moved.velocity.distance(rotated) < 1e-4,
        "the client's motion turns by the landmark yaw: {:?}",
        moved.velocity
    );
    assert!(relay.app.seam_stage.is_latched(pawn, 0));
    assert!(relay.app.seam_stage.take_arrivals().contains(&(0, pawn)));
    assert_eq!(relay.placement(INSIDE), None);

    let outside = promote(&mut relay, OUTSIDE);
    relay.assert_pawn_at_hint(outside, &outside_hint);
}

// Coop 3 / P13 (resolve) and Coop 7 / P17 (a): an arrival a live pawn blocks
// when the seat resolves falls back to a start with one warning, and its hint
// names that start; after a demotion and re-promotion it re-enters at a start,
// never its arrival.
#[test]
fn seam_client_arrival_blocked_at_resolve_takes_a_start_for_good() {
    let capture = LogCapture::start();
    let blocked = motion_onto(standing_on(&points(), 0));
    let (mut relay, _) = crossed(&[INSIDE], blocked.clone());

    let hint = relay.await_hint(INSIDE);
    assert_hint_not_at_arrival(&hint, &blocked);
    assert_eq!(relay.placement(INSIDE), Some(1));
    capture.assert_logged_once(log::Level::Warn, "seam `door` arrival blocked");

    let pawn = promote(&mut relay, INSIDE);
    relay.assert_pawn_at_hint(pawn, &hint);
    assert!(
        relay.app.seam_stage.take_arrivals().is_empty(),
        "no `arrive`"
    );

    relay.demote_by_mod_digest(&[INSIDE]);
    relay.restore_mod_digest();
    let repromoted = relay.await_pawn(INSIDE);
    relay.assert_pawn_at_hint(repromoted, &hint);
    capture.assert_logged_once(log::Level::Warn, "seam `door` arrival blocked");
}

// Coop 3 / P13 (promotion) and Coop 7 / P17 (b): an arrival that fit when the
// seat resolved but that the host's pawn stands on at promotion spawns at a
// start chosen then, with one warning and a resent hint naming it; a later
// re-promotion stays at a start.
#[test]
fn seam_client_arrival_blocked_at_promotion_spawns_at_a_start_and_rehints() {
    let capture = LogCapture::start();
    let arrival = motion();
    let (mut relay, host) = crossed(&[INSIDE], arrival.clone());
    let hint = relay.await_hint(INSIDE);
    assert_hint_at_arrival(&hint, &arrival);
    relay.move_pawn(host, destination_body(&arrival));

    let pawn = promote(&mut relay, INSIDE);
    relay.pump_until(|relay| relay.hints(INSIDE).len() == 2);
    let fallback = relay.hints(INSIDE).pop().unwrap();
    assert_hint_not_at_arrival(&fallback, &arrival);
    relay.assert_pawn_at_hint(pawn, &fallback);
    assert!(
        relay.placement(INSIDE).is_some(),
        "the fallback holds a start"
    );
    capture.assert_logged_once(log::Level::Warn, "seam `door` arrival blocked");
    assert!(
        relay.app.seam_stage.take_arrivals().is_empty(),
        "no `arrive`"
    );

    relay.demote_by_mod_digest(&[INSIDE]);
    relay.restore_mod_digest();
    let repromoted = relay.await_pawn(INSIDE);
    assert!(
        eye(&relay, repromoted).distance(arrival_eye(&arrival)) > 1.0,
        "re-promotion never returns to the arrival"
    );
    capture.assert_logged_once(log::Level::Warn, "seam `door` arrival blocked");
}

// Coop 6 / P7 (a) and (b): the arrival spawn consumed it. Demoted, the seat is
// hinted a start before it re-promotes there; disconnected and rejoined within
// the hold, it resolves a start.
#[test]
fn seam_client_leaving_participation_after_its_arrival_spawn_re_enters_at_a_start() {
    let arrival = motion();
    let (mut relay, _) = crossed(&[INSIDE], arrival.clone());
    relay.await_hint(INSIDE);
    promote(&mut relay, INSIDE);

    relay.demote_by_mod_digest(&[INSIDE]);
    relay.pump_until(|relay| relay.hints(INSIDE).len() == 2);
    let start = relay.hints(INSIDE).pop().unwrap();
    assert_hint_not_at_arrival(&start, &arrival);
    relay.restore_mod_digest();
    let repromoted = relay.await_pawn(INSIDE);
    relay.assert_pawn_at_hint(repromoted, &start);

    // (b) Rejoin within the hold.
    let arrival = motion();
    let (mut relay, _) = crossed(&[INSIDE], arrival.clone());
    let seat = relay.seat(INSIDE).unwrap();
    relay.await_hint(INSIDE);
    promote(&mut relay, INSIDE);
    relay.disconnect(INSIDE);
    relay.connect_as(43, INSIDE);
    relay.pump_until(|relay| relay.seat(43) == Some(seat));
    let rejoin = relay.await_hint(43);
    assert_hint_not_at_arrival(&rejoin, &arrival);
    let pawn = promote(&mut relay, 43);
    relay.assert_pawn_at_hint(pawn, &rejoin);
}

// Coop 6 / P7 (c): a client that disconnects while it loads the destination
// and rejoins within the hold before any spawn there still arrives.
#[test]
fn seam_client_reconnecting_before_its_spawn_still_arrives() {
    let arrival = motion();
    let (mut relay, _) = crossed(&[INSIDE], arrival.clone());
    let seat = relay.seat(INSIDE).unwrap();
    relay.await_hint(INSIDE);
    relay.disconnect(INSIDE);
    relay.connect_as(43, INSIDE);
    relay.pump_until(|relay| relay.seat(43) == Some(seat));
    let rejoin = relay.await_hint(43);
    assert_hint_at_arrival(&rejoin, &arrival);
    let pawn = promote(&mut relay, 43);
    assert!(eye(&relay, pawn).distance(arrival_eye(&arrival)) < 1e-4);
    assert!(relay.app.seam_stage.take_arrivals().contains(&(0, pawn)));
}

// Coop 5 / P5 and P20: a host level change before the crossing client spawns
// clears its arrival, even a seam into a map holding a seam of the first
// crossing's name: it arrives at a start with no warning.
#[test]
fn seam_host_level_change_before_the_client_spawns_clears_its_arrival() {
    for by_seam in [false, true] {
        let capture = LogCapture::start();
        let arrival = motion();
        let (mut relay, _) = crossed(&[INSIDE], arrival.clone());
        relay.await_hint(INSIDE);
        let hints = relay.hints(INSIDE).len();

        // The host crosses on while the client still loads: no pawn of its
        // stands in the seam, so the harvest gives it nothing.
        if by_seam {
            relay.app.unload_level_crossing(Some(DOOR));
            assert!(
                relay
                    .app
                    .seam_crossing
                    .as_ref()
                    .is_some_and(|crossing| crossing.arrivals.is_empty()),
                "no arrival for a seat with no pawn"
            );
        } else {
            relay.app.unload_level();
        }
        relay.install_seam_level(
            THIRD,
            seam_world(vec![destination_door()], &points()),
            points(),
        );
        relay.pump_until(|relay| relay.hints(INSIDE).len() > hints);
        let hint = relay.hints(INSIDE).pop().unwrap();
        assert_hint_not_at_arrival(&hint, &arrival);
        let pawn = promote(&mut relay, INSIDE);
        relay.assert_pawn_at_hint(pawn, &hint);
        capture.assert_not_logged(log::Level::Warn, "seam `door`");
    }
}

// Carry 5 / P6 (c): a suspend while a crossing client still loads the
// destination clears its arrival: after the resume the host's next install
// places it at a start.
#[test]
fn seam_suspend_while_a_crossing_client_loads_clears_its_arrival() {
    let capture = LogCapture::start();
    let arrival = motion();
    let (mut relay, _) = crossed(&[INSIDE], arrival.clone());
    let first = relay.await_hint(INSIDE);
    assert_hint_at_arrival(&first, &arrival);
    let hints = relay.hints(INSIDE).len();

    relay.app.suspend_platform();
    assert!(relay.app.seam_crossing.is_none());
    relay.install_seam_level(
        DESTINATION,
        seam_world(vec![destination_door()], &points()),
        points(),
    );
    relay.pump_until(|relay| relay.hints(INSIDE).len() > hints);
    let hint = relay.hints(INSIDE).pop().unwrap();
    assert_hint_not_at_arrival(&hint, &arrival);
    let pawn = promote(&mut relay, INSIDE);
    relay.assert_pawn_at_hint(pawn, &hint);
    capture.assert_not_logged(log::Level::Warn, "seam `door`");
}
