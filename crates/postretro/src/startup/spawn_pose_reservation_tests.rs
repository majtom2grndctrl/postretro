//! A resolved seat's reservation through respawn, re-promotion, demotion,
//! holds and live overlap, and the hints each re-resolution sends.
//! See: context/lib/networking.md §Slot lifecycle · context/lib/scripting.md §12

use super::lifecycle_test_fixtures::script_ctx;
use super::spawn_pose_handoff_fixtures::{
    Naming, OfferOne, Relay, hint_pose_of, pose, spawn_points, standing_on,
};
use super::*;
use crate::spawn_pose::{SpawnPose, UnbackedSpawn};
use postretro_entities::RespawnRequest;
use postretro_net::wire::SpawnPoseHint;
use postretro_sim::scripting::map_entity::MapEntity;
use std::time::Duration;

fn assert_hint_names(hint: &SpawnPoseHint, points: &[MapEntity], index: usize) {
    let (eye, yaw, pitch) = hint_pose_of(points, index);
    assert!(
        Vec3::from_array(hint.eye).distance(Vec3::from_array(eye)) < 1e-5,
        "hint eye {:?} is start {index}'s eye {eye:?}",
        hint.eye
    );
    assert_eq!((hint.yaw, hint.pitch), (yaw, pitch));
}

fn scout_pose(single_use: bool) -> UnbackedSpawn {
    UnbackedSpawn {
        pose: SpawnPose {
            eye: Vec3::new(20.0, 3.0, -9.0),
            yaw: 0.8,
            pitch: 0.25,
        },
        entity_class: "scout".into(),
        single_use,
    }
}

fn by_seat(relay: &Relay, a: u64, b: u64) -> (u64, u64) {
    if relay.seat(a) < relay.seat(b) {
        (a, b)
    } else {
        (b, a)
    }
}

fn last_position(naming: &[Naming], wanted: impl Fn(&Naming) -> bool) -> usize {
    naming
        .iter()
        .rposition(wanted)
        .unwrap_or_else(|| panic!("arrival missing from {naming:?}"))
}

fn is_marker(naming: &Naming) -> bool {
    matches!(naming, Naming::Marker)
}

// R3.
#[test]
fn respawn_skips_a_resolved_unpromoted_seats_start_and_a_seat_holding_none_takes_the_first_free() {
    let mut relay = Relay::admitted(&[41, 42]);
    let (offered, waiting) = by_seat(&relay, 41, 42);
    let offered_seat = relay.seat(offered).unwrap();
    relay
        .app
        .spawn_poses
        .set_unbacked_source(Some(Box::new(OfferOne {
            seat: offered_seat,
            spawn: scout_pose(false),
            consumed: Default::default(),
        })));
    let points = spawn_points(4);
    relay.install(points.clone());
    relay.reveal(offered);
    relay.host_reveal();
    let pawn = relay.await_pawn(offered);
    // `waiting` was resolved at install and never reveals.
    assert_eq!(relay.placement(waiting), Some(1));
    assert!(!relay.participating(waiting));
    assert_eq!(relay.placement(offered), None);

    let ctx = script_ctx(&relay.app);
    ctx.pending_respawns.borrow_mut().insert(
        pawn,
        RespawnRequest {
            pawn,
            health: None,
            state: None,
        },
    );
    let _ = postretro_sim::sim::begin_tick(&ctx.registry, Some(&ctx));

    // The scout capsule's half extent is 0.3 + 0.6.
    let expected = points[2].origin + Vec3::Y * 0.9;
    let position = relay.transform(pawn).position;
    assert!(
        position.distance(expected) < 1e-4,
        "past the host's start and the waiting seat's, the first free one: {position:?}"
    );
}

// R4, O6.
#[test]
fn single_use_pose_consumed_by_its_spawn_rehints_a_start_before_re_promotion_and_rejoin() {
    let mut relay = Relay::admitted(&[41]);
    let seat = relay.seat(41).unwrap();
    relay
        .app
        .spawn_poses
        .set_unbacked_source(Some(Box::new(OfferOne {
            seat,
            spawn: scout_pose(true),
            consumed: Default::default(),
        })));
    let points = spawn_points(3);
    relay.install(points.clone());
    let unbacked = relay.await_hint(41);
    assert_eq!(pose(&unbacked).0, scout_pose(true).pose.eye.to_array());
    relay.reveal(41);
    relay.host_reveal();
    let pawn = relay.await_pawn(41);
    relay.assert_pawn_at_hint(pawn, &unbacked);

    // The spawn consumed the pose: the seat resolves to a start, reserves
    // it, and its client is told before any later promotion.
    relay.pump_until(|relay| relay.hints(41).len() == 2);
    let start = relay.hints(41).pop().unwrap();
    assert_hint_names(&start, &points, 1);
    assert_eq!(relay.placement(41), Some(1));

    // Re-promotion without reload: the start's hint precedes its marker.
    relay.demote_by_mod_digest(&[41]);
    relay.restore_mod_digest();
    let repromoted = relay.await_pawn(41);
    relay.pump_until(|relay| {
        relay
            .naming(41)
            .iter()
            .filter(|naming| is_marker(naming))
            .count()
            == 2
    });
    let naming = relay.naming(41);
    assert!(
        last_position(&naming, |naming| *naming == Naming::Hint(start.clone()))
            < last_position(&naming, is_marker),
        "{naming:?}"
    );
    relay.assert_pawn_at_hint(repromoted, &start);
    assert_eq!(
        relay.hints(41).len(),
        2,
        "a sticky start needs no fresh hint"
    );

    // Rejoin within the hold resolves to the same start.
    relay.disconnect(41);
    relay.connect_as(43, 41);
    relay.pump_until(|relay| relay.seat(43) == Some(seat));
    let rejoin = relay.await_hint(43);
    assert_eq!(pose(&rejoin), pose(&start));
    relay.reveal(43);
    let rejoined = relay.await_pawn(43);
    relay.assert_pawn_at_hint(rejoined, &rejoin);
    assert_eq!(relay.placement(43), Some(1));
}

// R5.
#[test]
fn spawn_stays_at_its_hinted_pose_when_other_seats_free_and_take_starts_before_promotion() {
    let mut relay = Relay::admitted(&[41, 42]);
    let (lower, higher) = by_seat(&relay, 41, 42);
    let points = spawn_points(4);
    relay.install(points.clone());
    let hint = relay.await_hint(higher);
    assert_hint_names(&hint, &points, 2);

    // The lower seat's hold expires, freeing start 1; a joiner then takes it.
    relay.disconnect(lower);
    relay
        .seats_mut()
        .advance_hold_clock(Duration::from_secs(3600));
    relay.step();
    assert!(
        relay
            .seats()
            .assigned_placements()
            .all(|(_, placement)| placement != 1),
        "start 1 is free"
    );
    relay.admit(&[43]);
    assert_eq!(relay.placement(43), Some(1));

    relay.reveal(higher);
    relay.host_reveal();
    let pawn = relay.await_pawn(higher);
    relay.settle();

    relay.assert_pawn_at_hint(pawn, &hint);
    assert_eq!(relay.placement(higher), Some(2));
    assert_eq!(relay.hints(higher), [hint]);
}

// R6, O5.
#[test]
fn promotion_moves_off_a_reserved_start_a_live_pawn_stands_on_and_rehints_it() {
    let mut relay = Relay::admitted(&[41]);
    let points = spawn_points(3);
    let (host_pawn, _) = relay.install(points.clone()).unwrap();
    let reserved = relay.await_hint(41);
    assert_hint_names(&reserved, &points, 1);
    relay.move_pawn(host_pawn, standing_on(&points, 1));

    relay.reveal(41);
    relay.host_reveal();
    let pawn = relay.await_pawn(41);
    relay.pump_until(|relay| relay.hints(41).len() == 2);

    let moved = relay.hints(41).pop().unwrap();
    assert_hint_names(&moved, &points, 2);
    assert_eq!(relay.placement(41), Some(2));
    relay.assert_pawn_at_hint(pawn, &moved);
    let naming = relay.naming(41);
    assert!(
        last_position(&naming, is_marker)
            < last_position(&naming, |naming| *naming == Naming::Hint(moved.clone())),
        "the fresh hint follows the marker: {naming:?}"
    );
}

// R8, O10: demotion without reload.
#[test]
fn demoted_seats_keep_their_starts_and_return_to_their_hinted_pose_with_no_fresh_hint() {
    let mut relay = Relay::admitted(&[41, 42]);
    let points = spawn_points(4);
    relay.install(points.clone());
    let first = relay.await_hint(41);
    let second = relay.await_hint(42);
    relay.reveal(41);
    relay.reveal(42);
    relay.host_reveal();
    relay.await_pawn(41);
    relay.await_pawn(42);
    let (start_41, start_42) = (relay.placement(41), relay.placement(42));

    relay.demote_by_mod_digest(&[41, 42]);
    relay.admit(&[43]);
    assert_eq!(
        relay.placement(43),
        Some(3),
        "a joiner skips the demoted seats' starts"
    );
    relay.restore_mod_digest();
    let back_41 = relay.await_pawn(41);
    let back_42 = relay.await_pawn(42);
    relay.settle();

    relay.assert_pawn_at_hint(back_41, &first);
    relay.assert_pawn_at_hint(back_42, &second);
    assert_eq!(
        (relay.placement(41), relay.placement(42)),
        (start_41, start_42)
    );
    assert_eq!(relay.hints(41), [first], "no fresh hint");
    assert_eq!(relay.hints(42), [second], "no fresh hint");
}

// R8: a held seat.
#[test]
fn held_seat_keeps_its_start_and_rejoins_at_its_hinted_pose() {
    let mut relay = Relay::admitted(&[41]);
    let points = spawn_points(3);
    relay.install(points.clone());
    let hint = relay.await_hint(41);
    relay.reveal(41);
    relay.host_reveal();
    relay.await_pawn(41);
    let seat = relay.seat(41).unwrap();

    relay.disconnect(41);
    assert_eq!(relay.seats().placement_for_seat(seat), Some(1));
    relay.admit(&[42]);
    assert_eq!(
        relay.placement(42),
        Some(2),
        "a joiner skips the held start"
    );

    relay.connect_as(43, 41);
    relay.pump_until(|relay| relay.seat(43) == Some(seat));
    let rejoin = relay.await_hint(43);
    assert_eq!(pose(&rejoin), pose(&hint));
    relay.reveal(43);
    let pawn = relay.await_pawn(43);
    relay.assert_pawn_at_hint(pawn, &rejoin);
}
