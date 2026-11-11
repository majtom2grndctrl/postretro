//! A connected client's unarmed stand-in takes its host's spawn-pose hint: at
//! install, and on arrival during Settling or while Running with its pawn
//! unarmed, except on the reveal frame, which holds it for the next frame.
//! The hint never gates anything, and arming moves position only. Hints come
//! from the real host over relay; the client App drains them through its
//! Control router, one drain per frame, before that frame's snapshots.
//! See: context/lib/networking.md §Slot lifecycle · context/lib/boot_sequence.md §1

use super::spawn_pose_handoff_fixtures::{
    LEVEL, OfferOne, Relay, hint_pose_of, spawn_points, standing_on,
};
use super::spawn_pose_stand_in_fixtures::{
    ClientView, OTHER_LEVEL, assert_pose, hint_pose, host_reinstalls, without_hints,
};
use super::*;
use crate::session::level_streaming::settle::{ResourceSettle, SettleReport};
use crate::spawn_pose::{SpawnPose, UnbackedSpawn};
use crate::startup::presented_pose::PresentedPose;
use crate::startup::settling::SETTLE_TIMEOUT;
use postretro_net::wire::{DivergenceReason, RevealedLevel, ServerControlMessage, SpawnPoseHint};
use postretro_sim::scripting::map_entity::MapEntity;
use std::time::{Duration, Instant};

const CLIENT: u64 = 41;

fn is_hint(message: &ServerControlMessage) -> bool {
    matches!(message, ServerControlMessage::SpawnPoseHint(_))
}

fn hints_in(messages: &[ServerControlMessage]) -> Vec<SpawnPoseHint> {
    messages
        .iter()
        .filter_map(|message| match message {
            ServerControlMessage::SpawnPoseHint(hint) => Some(hint.clone()),
            _ => None,
        })
        .collect()
}

/// A host with one seated client and `points` installed, and that client's
/// App, which drained the host's Relevel and hint at its frontend.
fn joined(points: Vec<MapEntity>) -> (Relay, ClientView, SpawnPoseHint) {
    let mut relay = Relay::admitted(&[CLIENT]);
    relay.install(points.clone());
    let hint = relay.await_hint(CLIENT);
    let mut client = ClientView::new(points);
    client.follow(&mut relay, CLIENT);
    (relay, client, hint)
}

// C1, nonzero pitch.
#[test]
fn matching_hint_installs_settles_and_reveals_the_client_at_the_hinted_eye_yaw_and_pitch() {
    let points = spawn_points(3);
    let (_relay, mut client, hint) = joined(points.clone());
    assert!(hint.pitch != 0.0, "fixture: the joiner's start pitches");
    let hinted = hint_pose(&hint);

    client.install();
    assert_eq!(client.app.published_level_identity.as_deref(), Some(LEVEL));
    client.assert_at(hinted, "install's stand-in");
    assert_pose(
        client.app.presented_pose(),
        hinted,
        "Settling streams from the hinted pose",
    );
    assert!(
        client.first_spawn_pose().position.distance(hinted.position) > 1.0,
        "the hint is not the first player's start"
    );

    client.reveal();
    client.assert_at(hinted, "the reveal frame");
    assert_eq!(client.app.camera.pitch, hint.pitch, "the revealed pitch");
}

// C2.
#[test]
fn client_whose_player_spawn_order_differs_from_the_hosts_settles_at_the_hosts_pose() {
    let host_points = spawn_points(3);
    let mut relay = Relay::admitted(&[CLIENT]);
    relay.install(host_points.clone());
    let hint = relay.await_hint(CLIENT);
    // The same three starts, in another map order on the client.
    let client_points = vec![
        host_points[2].clone(),
        host_points[0].clone(),
        host_points[1].clone(),
    ];
    let mut client = ClientView::new(client_points.clone());
    client.follow(&mut relay, CLIENT);

    client.install();
    client.reveal();

    let (host_eye, yaw, pitch) = hint_pose_of(&host_points, 1);
    let host_pose = PresentedPose {
        position: Vec3::from_array(host_eye),
        yaw,
        pitch,
    };
    assert_pose(
        hint_pose(&hint),
        host_pose,
        "fixture: the host names its start 1",
    );
    client.assert_at(host_pose, "the host's pose for this seat");
    let (local_index_eye, _, _) = hint_pose_of(&client_points, 1);
    assert!(
        Vec3::from_array(local_index_eye).distance(client.camera().position) > 1.0,
        "not the client's own start at the host's index"
    );
    assert!(
        client
            .first_spawn_pose()
            .position
            .distance(client.camera().position)
            > 1.0,
        "not the client's own first start"
    );
}

// C3.
#[test]
fn hint_in_the_same_drain_as_relevel_applies_to_the_new_level_after_the_old_one_unloads() {
    let points = spawn_points(3);
    let mut relay = Relay::admitted(&[CLIENT]);
    relay.install_from(Some(OTHER_LEVEL), points.clone());
    relay.await_hint(CLIENT);
    let mut client = ClientView::new(points.clone());
    client.follow(&mut relay, CLIENT);
    client.install();
    client.reveal();
    assert_eq!(
        client.app.published_level_identity.as_deref(),
        Some(OTHER_LEVEL)
    );
    let running = client.camera();

    let messages = host_reinstalls(&mut relay, CLIENT, LEVEL, &points);
    let relevel = messages
        .iter()
        .position(|message| matches!(message, ServerControlMessage::Relevel { .. }))
        .expect("the Relevel naming the next level");
    let hint_at = messages.iter().position(is_hint).unwrap();
    assert!(relevel < hint_at, "one drain, Relevel first: {messages:?}");
    let hint = hints_in(&messages).pop().unwrap();
    assert_eq!(hint.level_identity, LEVEL);
    client.drain(messages);
    client.drain(Vec::new());
    assert_pose(
        client.camera(),
        running,
        "a hint for the next level leaves the running one",
    );

    // The old level unloads (the level-scoped reset included), then the new
    // one installs.
    client.install();
    assert_eq!(client.app.published_level_identity.as_deref(), Some(LEVEL));
    assert_eq!(client.stored_hint(), Some(hint.clone()));
    client.assert_at(hint_pose(&hint), "the new level's stand-in");
    client.reveal();
    client.assert_at(hint_pose(&hint), "the new level's reveal");
}

// C4, O4.
#[test]
fn holding_notice_after_the_hint_leaves_it_and_the_client_settles_at_it() {
    let points = spawn_points(3);
    let mut relay = Relay::admitted(&[CLIENT]);
    relay.install(points.clone());
    let hint = relay.await_hint(CLIENT);
    let joining = relay.forward(CLIENT);
    // The client installs and declares parity; the host holds it until both
    // peers reveal: the ordinary revealed hold.
    let holds_before = relay.holding_notices(CLIENT);
    relay.declare_parity(CLIENT);
    relay.pump_until(|relay| relay.holding_notices(CLIENT) > holds_before);
    let holding = relay.forward(CLIENT);
    assert!(
        holding.iter().any(|message| matches!(
            message,
            ServerControlMessage::Divergence(DivergenceReason::Holding(cause))
                if cause.is_revealed_hold()
        )),
        "a revealed hold: {holding:?}"
    );

    // The notice reaches a Settling client after its hint.
    let mut settling = ClientView::new(points.clone());
    settling.drain(joining.clone());
    settling.install();
    settling.drain(holding.clone());
    assert_eq!(settling.app.boot_state, BootState::Settling);
    assert_eq!(settling.stored_hint(), Some(hint.clone()));
    settling.assert_at(hint_pose(&hint), "Settling after the hold");
    settling.reveal();
    settling.assert_at(hint_pose(&hint), "revealed after the hold");

    // The notice reaches a client still loading, before its install.
    let mut loading = ClientView::new(points);
    loading.drain(joining);
    loading.drain(holding);
    assert_eq!(loading.stored_hint(), Some(hint.clone()));
    loading.install();
    loading.assert_at(hint_pose(&hint), "installed after the hold");
}

// C5.
#[test]
fn hint_naming_another_level_or_an_earlier_life_is_ignored_and_the_stand_in_stays_at_the_first_spawn()
 {
    let points = spawn_points(3);
    let (mut relay, mut client, first_life) = joined(points.clone());
    client.install();
    client.reveal();

    // A host restart: the new life's hint is still in flight when the client
    // reinstalls, so the store holds the previous life's.
    let restart = host_reinstalls(&mut relay, CLIENT, LEVEL, &points);
    let restart_life = restart
        .iter()
        .find_map(|message| match message {
            ServerControlMessage::Relevel { level_life, .. } => Some(*level_life),
            _ => None,
        })
        .expect("the restart's Relevel");
    assert_ne!(Some(restart_life), first_life.level_life);
    client.drain(without_hints(&restart));
    client.install();
    assert_eq!(client.app.followed_level_life(), Some(restart_life));
    assert_eq!(client.stored_hint(), Some(first_life.clone()));
    client.assert_at(client.first_spawn_pose(), "an earlier life is ignored");

    // The host moves to another level; the hint naming it is still in flight.
    let moved = host_reinstalls(&mut relay, CLIENT, OTHER_LEVEL, &points);
    client.drain(without_hints(&moved));
    client.install();
    assert_eq!(
        client.app.published_level_identity.as_deref(),
        Some(OTHER_LEVEL)
    );
    assert_eq!(client.stored_hint().unwrap().level_identity, LEVEL);
    client.assert_at(client.first_spawn_pose(), "another level is ignored");
    client.reveal();
    client.assert_at(client.first_spawn_pose(), "and reveals there");
}

// C6.
#[test]
fn host_same_map_restart_reloads_the_client_and_settles_it_at_the_new_lifes_hint() {
    let points = spawn_points(3);
    let (mut relay, mut client, first_life) = joined(points.clone());
    client.install();
    client.reveal();

    let restart = host_reinstalls(&mut relay, CLIENT, LEVEL, &points);
    let new_life = hints_in(&restart).pop().unwrap();
    assert_eq!(new_life.level_identity, LEVEL);
    assert_ne!(new_life.level_life, first_life.level_life, "a new life");
    client.drain(restart);
    assert!(
        !client.app.level_requests.is_empty(),
        "the restart reloads the client"
    );

    client.install();
    client.assert_at(hint_pose(&new_life), "the new life's stand-in");
    client.reveal();
    client.assert_at(hint_pose(&new_life), "the new life's reveal");
    assert_eq!(
        client.declared_reveal(),
        Some(RevealedLevel::new(LEVEL, new_life.level_life)),
        "the reveal names the life the hint named"
    );
}

// C7, O20.
#[test]
fn hint_during_settling_retargets_the_settle_view_without_resetting_the_settling_timer() {
    let points = spawn_points(3);
    let mut relay = Relay::admitted(&[CLIENT]);
    relay.install(points.clone());
    let hint = relay.await_hint(CLIENT);
    let joining = relay.forward(CLIENT);
    let mut client = ClientView::new(points);
    // The client is already Settling when the hint lands.
    client.drain(without_hints(&joining));
    client.install();
    client.assert_at(client.first_spawn_pose(), "the first-spawn stand-in");
    let started = Instant::now() - Duration::from_millis(9_500);
    client.app.enter_settling(started);

    client.drain(vec![ServerControlMessage::SpawnPoseHint(hint.clone())]);
    assert_eq!(client.app.boot_state, BootState::Settling);
    assert_pose(
        client.app.presented_pose(),
        hint_pose(&hint),
        "Settling now streams from the hint",
    );
    client.assert_at(hint_pose(&hint), "the retargeted settle view");

    // The entry's deadline still counts from when it entered Settling: a
    // reset timer would hold this unsettled frame for another 10 s.
    let unsettled = SettleReport {
        sh: ResourceSettle::Unsettled(1),
        lightmap: ResourceSettle::NotStreamed,
    };
    client
        .app
        .advance_settle(unsettled, started + SETTLE_TIMEOUT);
    assert!(
        client.app.reveal_if_released(),
        "the original deadline releases"
    );
    client.assert_at(hint_pose(&hint), "the timed-out reveal");
}

// C8, O7.
#[test]
fn hint_first_arriving_on_the_reveal_frame_leaves_it_at_the_settled_pose_and_moves_the_camera_on_a_later_unarmed_frame()
 {
    let points = spawn_points(3);
    let mut relay = Relay::admitted(&[CLIENT]);
    relay.install(points.clone());
    let hint = relay.await_hint(CLIENT);
    let joining = relay.forward(CLIENT);
    let mut client = ClientView::new(points);
    client.drain(without_hints(&joining));
    client.install();
    let settled = client.first_spawn_pose();

    // The deciding Settling frame, then the reveal edge on the next redraw,
    // whose world poll brings the (resent) hint.
    client.reveal_edge();
    client.drain(vec![ServerControlMessage::SpawnPoseHint(hint.clone())]);
    client.assert_at(settled, "the reveal frame presents the settled pose");

    // The next frame's drain brings nothing new.
    client.drain(Vec::new());
    client.assert_at(hint_pose(&hint), "a later unarmed frame");
}

// C9, with the overlap re-resolution's hint (R6).
#[test]
fn fresh_hint_moves_a_running_unarmed_client_before_its_pawn_arms_there() {
    let points = spawn_points(3);
    let mut relay = Relay::admitted(&[CLIENT]);
    let (host_pawn, _) = relay.install(points.clone()).unwrap();
    let reserved = relay.await_hint(CLIENT);
    let mut client = ClientView::new(points.clone());
    client.follow(&mut relay, CLIENT);
    client.install();
    client.reveal();
    client.assert_at(hint_pose(&reserved), "revealed at the reserved start");

    relay.move_pawn(host_pawn, standing_on(&points, 1));
    relay.reveal(CLIENT);
    relay.host_reveal();
    let pawn = relay.await_pawn(CLIENT);
    relay.pump_until(|relay| relay.hints(CLIENT).len() == 2);
    let moved = relay.hints(CLIENT).pop().unwrap();
    assert!(
        (moved.yaw, moved.pitch) != (reserved.yaw, reserved.pitch),
        "fixture: the fresh hint faces another way"
    );

    // One frame brings the fresh hint and the arming baseline: its Control
    // drain runs first, then the snapshot that arms the pawn.
    client.follow(&mut relay, CLIENT);
    client.assert_at(
        hint_pose(&moved),
        "unarmed, the drain moves the camera to the fresh hint",
    );
    client.arm(&relay, pawn);
    assert_pose(
        client.camera(),
        hint_pose(&moved),
        "the pawn arms at the fresh hint's eye, yaw and pitch",
    );
}

// C10, O5.
#[test]
fn hint_after_the_pawn_arms_moves_neither_the_camera_position_nor_its_facing() {
    let points = spawn_points(3);
    let mut relay = Relay::admitted(&[CLIENT]);
    let (host_pawn, _) = relay.install(points.clone()).unwrap();
    let reserved = relay.await_hint(CLIENT);
    let mut client = ClientView::new(points.clone());
    client.follow(&mut relay, CLIENT);
    client.install();
    client.reveal();

    relay.move_pawn(host_pawn, standing_on(&points, 1));
    relay.reveal(CLIENT);
    relay.host_reveal();
    let pawn = relay.await_pawn(CLIENT);
    relay.pump_until(|relay| relay.hints(CLIENT).len() == 2);
    let after_marker = relay.forward(CLIENT);
    let moved = hints_in(&after_marker).pop().unwrap();

    // A snapshot arms the pawn before the fresh hint drains.
    client.arm(&relay, pawn);
    let armed = client.camera();
    assert_pose(
        armed,
        PresentedPose {
            position: hint_pose(&moved).position,
            yaw: reserved.yaw,
            pitch: reserved.pitch,
        },
        "arming moved the camera to the pawn's eye and kept its facing",
    );
    client.drain(after_marker);
    client.drain(Vec::new());
    assert_pose(client.camera(), armed, "the late hint moves nothing");
}

// R12, O11: the client half.
#[test]
fn client_revealed_before_connecting_applies_its_post_marker_hint_while_unarmed_or_not_at_all_once_armed()
 {
    let points = spawn_points(3);
    let mut relay = Relay::host();
    relay.install_from(None, points.clone());
    relay.host_reveal();
    let identity = relay.app.published_level_identity.clone().unwrap();
    // Booted on the same uncatalogued map and revealed before connecting.
    let mut unarmed = ClientView::new(points.clone());
    let mut armed = ClientView::new(points.clone());
    for client in [&mut unarmed, &mut armed] {
        client.install();
        client.reveal();
        assert_eq!(client.app.published_level_identity, Some(identity.clone()));
        client.assert_at(client.first_spawn_pose(), "no hint yet");
    }

    relay.connect_as(CLIENT, CLIENT);
    relay.reveal_as(CLIENT, &identity, None);
    let pawn = relay.await_pawn(CLIENT);
    let hint = relay.await_hint(CLIENT);
    let after_marker = relay.forward(CLIENT);

    let first_spawn = unarmed.first_spawn_pose();
    assert!(
        (hint.yaw, hint.pitch) != (first_spawn.yaw, first_spawn.pitch),
        "fixture: the hint faces another way than the first spawn"
    );

    // The post-marker hint and the arming baseline land in one frame: its
    // Control drain runs first, then the snapshot that arms the pawn.
    unarmed.drain(after_marker.clone());
    unarmed.assert_at(hint_pose(&hint), "the drain moves the unarmed stand-in");
    unarmed.arm(&relay, pawn);
    assert_pose(
        unarmed.camera(),
        hint_pose(&hint),
        "the pawn arms at the hint's eye, yaw and pitch",
    );

    armed.arm(&relay, pawn);
    let at_arm = armed.camera();
    assert_eq!(
        (at_arm.yaw, at_arm.pitch),
        (armed.first_spawn_pose().yaw, armed.first_spawn_pose().pitch),
        "arming kept the stand-in's facing"
    );
    armed.drain(after_marker);
    armed.drain(Vec::new());
    assert_pose(
        armed.camera(),
        at_arm,
        "armed first, the hint moves nothing",
    );
}

// C11.
#[test]
fn client_with_no_hint_reveals_at_the_first_spawn_stand_in_with_no_added_hold() {
    let mut client = ClientView::new(spawn_points(3));
    client.install();
    assert_eq!(client.stored_hint(), None);
    let stand_in = client.first_spawn_pose();
    client.assert_at(stand_in, "the first-spawn stand-in");
    // The first settled check releases it: `reveal` asserts the edge fires.
    client.reveal();
    client.assert_at(stand_in, "revealed at the stand-in");
}

// C12, O14.
#[test]
fn non_finite_hint_is_rejected_before_it_moves_the_stand_in() {
    let points = spawn_points(3);
    let mut relay = Relay::admitted(&[CLIENT]);
    relay.install(points.clone());
    let hint = relay.await_hint(CLIENT);
    let joining = relay.forward(CLIENT);
    let mut poisoned = hint.clone();
    poisoned.eye[1] = f32::NAN;
    poisoned.yaw += 1.0;

    // Settling at a finite hint: a finite arrival would retarget now.
    let mut settling = ClientView::new(points.clone());
    settling.drain(joining.clone());
    settling.install();
    settling.drain(vec![ServerControlMessage::SpawnPoseHint(poisoned.clone())]);
    assert_eq!(settling.stored_hint(), Some(hint.clone()));
    settling.assert_at(hint_pose(&hint), "the finite hint stands");

    // The only hint is non-finite: install keeps the first-spawn stand-in.
    let mut unhinted = ClientView::new(points);
    let mut messages = without_hints(&joining);
    messages.push(ServerControlMessage::SpawnPoseHint(poisoned));
    unhinted.drain(messages);
    assert_eq!(unhinted.stored_hint(), None);
    unhinted.install();
    unhinted.assert_at(unhinted.first_spawn_pose(), "no hint was stored");
}

// A1: a single-spawn level, and a client placed on a start that is not the
// first player's.
#[test]
fn arming_with_an_applied_hint_keeps_the_camera_within_a_centimetre_and_its_facing() {
    for count in [1, 3] {
        let points = spawn_points(count);
        let (mut relay, mut client, hint) = joined(points.clone());
        if count == 3 {
            let (eye, yaw, pitch) = hint_pose_of(&points, 1);
            assert_pose(
                hint_pose(&hint),
                PresentedPose {
                    position: Vec3::from_array(eye),
                    yaw,
                    pitch,
                },
                "fixture: the client's start is not the first player's",
            );
        }
        client.install();
        client.reveal();
        relay.reveal(CLIENT);
        relay.host_reveal();
        let pawn = relay.await_pawn(CLIENT);
        // Any hint promotion sends lands while the client is unarmed.
        relay.settle();
        client.follow(&mut relay, CLIENT);
        let stand_in = client.camera();

        client.arm(&relay, pawn);
        assert_pose(
            client.camera(),
            stand_in,
            &format!("{count} start(s): the arming frame"),
        );
    }
}

// A2. Regression guard: an applied hint must not reclaim the facing a player
// turned to before the pawn arms.
#[test]
fn view_turned_while_unarmed_keeps_its_yaw_and_pitch_when_the_pawn_arms() {
    let points = spawn_points(3);
    let (mut relay, mut client, hint) = joined(points);
    client.install();
    client.reveal();
    client.app.camera.rotate(0.6, -0.2);
    let turned = client.camera();
    client.drain(Vec::new());
    client.drain(Vec::new());
    assert_pose(client.camera(), turned, "later frames keep the turn");

    relay.reveal(CLIENT);
    relay.host_reveal();
    let pawn = relay.await_pawn(CLIENT);
    client.follow(&mut relay, CLIENT);
    client.arm(&relay, pawn);
    assert_pose(
        client.camera(),
        PresentedPose {
            position: hint_pose(&hint).position,
            yaw: turned.yaw,
            pitch: turned.pitch,
        },
        "arming moves position only",
    );
}

// R2 and R4: the client half. A pose no placement backs, single-use or not,
// drives the stand-in, and the pawn arms there with its yaw and pitch. A
// single-use pose's seat re-resolves only once it leaves participation, so no
// hint naming its next start reaches the client before the pawn arms.
#[test]
fn unbacked_test_pose_drives_the_stand_in_and_the_arming_frame_single_use_or_not() {
    for single_use in [false, true] {
        let scout = UnbackedSpawn {
            pose: SpawnPose {
                eye: Vec3::new(20.0, 3.0, -9.0),
                yaw: 0.8,
                pitch: 0.25,
            },
            entity_class: "scout".into(),
            single_use,
        };
        let points = spawn_points(3);
        let mut relay = Relay::admitted(&[CLIENT]);
        let seat = relay.seat(CLIENT).unwrap();
        relay
            .app
            .spawn_poses
            .set_unbacked_source(Some(Box::new(OfferOne {
                seat,
                spawn: scout.clone(),
                consumed: Default::default(),
            })));
        relay.install(points.clone());
        let offered = relay.await_hint(CLIENT);
        let mut client = ClientView::new(points);
        client.follow(&mut relay, CLIENT);
        let scout_pose = PresentedPose {
            position: scout.pose.eye,
            yaw: scout.pose.yaw,
            pitch: scout.pose.pitch,
        };
        let what = |step: &str| format!("single use {single_use}: {step}");

        client.install();
        client.assert_at(scout_pose, &what("the stand-in takes the unbacked pose"));
        client.reveal();
        relay.reveal(CLIENT);
        relay.host_reveal();
        let pawn = relay.await_pawn(CLIENT);
        // Any hint promotion sends lands before the first baseline.
        relay.settle();
        assert_eq!(
            relay.hints(CLIENT),
            vec![offered.clone()],
            "{}",
            what("no hint follows promotion while the seat participates")
        );
        client.follow(&mut relay, CLIENT);
        client.assert_at(scout_pose, &what("the stand-in stays at the unbacked pose"));
        client.arm(&relay, pawn);
        assert_pose(
            client.camera(),
            scout_pose,
            &what("the scout pawn arms there, with its yaw and pitch"),
        );
    }
}
