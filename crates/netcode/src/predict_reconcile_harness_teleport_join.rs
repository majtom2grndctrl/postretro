// Teleporter join-gate proofs over the conditioned-link harness: how often a
// joiner whose first pawn record cannot merge asks for a fresh baseline, and the
// camera hold its first written record raises. Every scenario drives the
// production predicted tick, host resolution, host movement and client reconcile.
// See: context/lib/networking.md §Snapshot apply ordering · §Current contract
//      (Join baseline budget)

#![cfg(test)]

use std::f32::consts::FRAC_PI_2;

use glam::{Vec2, Vec3};
use postretro_entities::Transform;

use postretro_entities::EntityRegistry;
use postretro_net::wire::{
    ComponentPayload, EntityRecord, NetworkId, SnapshotMessage, WireTransform,
};

use super::client::{ClientReplication, REFRESH_REASON_UNMERGED_LOCAL_PAWN};
use super::movement_state::movement_state_to_wire;
use super::predict_reconcile_harness_teleport_rig::{FORWARD, IDLE, Rig, Spec, clean_link, region};
use super::predict_reconcile_harness_test_fixtures::{DT, component};

/// The resend cadence the pending-repair set paces retries at.
const RESEND_MS: f32 = 200.0;

/// A classic teleporter straight ahead of the spawn, turning its traveller 90°.
fn entry_ahead() -> Spec {
    Spec::classic(
        region((-3.0, 3.0), (-8.0, -6.0)),
        Vec3::new(40.0, 1.25, 0.0),
        FRAC_PI_2,
    )
}

/// Hold the host tuning on the Control lane, as a lost reliable send waits for
/// its retransmit.
fn hold_tuning(rig: &mut Rig) {
    rig.h.hold_tuning();
}

/// Land the held tuning between frames, in a frame that carries no snapshot.
fn land_tuning(rig: &mut Rig) {
    rig.h.land_tuning();
    assert!(rig.h.client_tuning.is_some(), "the tuning installed");
}

/// Step until the client arms its pawn, with the tuning still held.
fn arm_without_tuning(rig: &mut Rig, wish: Vec2) {
    for _ in 0..200 {
        if rig.h.client_pawn.is_some() {
            break;
        }
        rig.step(wish);
    }
    assert!(rig.h.client_pawn.is_some(), "the client armed its pawn");
    assert!(rig.h.client_tuning.is_none(), "armed before its tuning");
}

/// One frame through the production client and host paths, without the rig's
/// per-receive bookkeeping, which expects every local reconcile to apply.
fn frame(rig: &mut Rig, wish: Vec2) {
    rig.predict(wish, true);
    rig.h.advance_clock();
    rig.host_ticks(1);
    rig.h.client_receive();
}

fn unmerged_requests(rig: &Rig) -> usize {
    rig.h
        .sent_refresh_requests
        .iter()
        .filter(|request| request.reason == REFRESH_REASON_UNMERGED_LOCAL_PAWN)
        .count()
}

fn client_pose(rig: &Rig) -> Vec3 {
    rig.h
        .client_registry
        .get_component::<Transform>(rig.h.client_pawn.unwrap())
        .unwrap()
        .position
}

// No merge can succeed without host tuning, so a joiner waiting on it asks for no
// baseline; once the tuning lands, one immediate request fetches the record that
// merges. A forced refresh bypasses the join baseline budget, so asking every
// round trip while the tuning is missing would flood the joiner.
#[test]
fn teleport_join_unmerged_refresh_waits_for_tuning_then_asks_once() {
    let mut rig = Rig::new(clean_link(), &[entry_ahead()]);
    hold_tuning(&mut rig);
    arm_without_tuning(&mut rig, IDLE);
    rig.idle(120);
    assert_eq!(
        unmerged_requests(&rig),
        0,
        "two seconds without tuning ask for nothing"
    );

    land_tuning(&mut rig);
    for _ in 0..60 {
        if !rig.h.prediction.first_record_pending() {
            break;
        }
        rig.step(IDLE);
    }
    assert!(
        !rig.h.prediction.first_record_pending(),
        "the first record merged"
    );
    rig.idle(60);
    assert!(
        unmerged_requests(&rig) <= 1,
        "at most the one immediate request: {}",
        unmerged_requests(&rig)
    );
}

// The host's tuning gives the pawn no movement, which leaves it inert with no
// movement component: no record can ever merge, so the joiner asks for none.
#[test]
fn teleport_join_unmerged_refresh_stops_while_tuning_has_no_movement() {
    let mut rig = Rig::new(clean_link(), &[entry_ahead()]);
    hold_tuning(&mut rig);
    arm_without_tuning(&mut rig, IDLE);
    land_tuning(&mut rig);
    rig.h.client_tuning.as_mut().unwrap().movement = None;
    rig.h.client_rebuild_movement_from_installed_tuning();
    assert!(
        rig.h
            .client_registry
            .get_component::<postretro_foundation::PlayerMovementComponent>(
                rig.h.client_pawn.unwrap()
            )
            .is_err(),
        "fixture: the pawn is inert"
    );
    for _ in 0..120 {
        frame(&mut rig, IDLE);
    }
    assert!(rig.h.prediction.first_record_pending(), "nothing merged");
    assert_eq!(unmerged_requests(&rig), 0, "no request while inert");
}

// A merge that keeps failing keeps asking, but only the first request goes out at
// once: the answering baseline clears the entry, and the retries wait on the resend
// cadence instead of repeating every round trip. The host is simulated, answering
// each request one 100 ms round trip later with a fresh local-player baseline.
#[test]
fn teleport_join_unmerged_refresh_paces_failing_merges_at_the_resend_cadence() {
    let mut registry = EntityRegistry::new();
    let mut client = ClientReplication::new();
    let mut sequence = 0;
    let mut answer = |client: &mut ClientReplication, registry: &mut EntityRegistry| {
        sequence += 1;
        client.apply_snapshot(
            registry,
            &SnapshotMessage {
                sequence,
                server_tick: sequence,
                records: vec![EntityRecord::FullBaseline {
                    network_id: 7,
                    baseline_id: sequence,
                    last_processed_client_tick: None,
                    local_player: true,
                    entity_class: Some("player".to_string()),
                    active_weapon_archetype: None,
                    projectile_presentation: None,
                    presentation_binding: None,
                    components: vec![
                        ComponentPayload::Transform(WireTransform {
                            position: [0.0; 3],
                            rotation: [0.0, 0.0, 0.0, 1.0],
                            scale: [1.0; 3],
                        }),
                        ComponentPayload::PlayerMovementState(movement_state_to_wire(
                            &component(),
                            0.0,
                        )),
                    ],
                }],
                state_schema_fingerprint: [0; 32],
                state_records: Vec::new(),
                faction_sentiment_record: None,
                server_script_time: 0.0,
            },
        )
    };
    answer(&mut client, &mut registry);
    assert_eq!(client.local_pawn_network_id(), Some(NetworkId(7)));

    const ROUND_TRIP_FRAMES: usize = 6;
    let frames = 120;
    let mut answers_due = Vec::new();
    let mut sent = Vec::new();
    for frame in 0..frames {
        if answers_due.first() == Some(&frame) {
            answers_due.remove(0);
            answer(&mut client, &mut registry);
        }
        let immediate = client.refresh_unmerged_local_pawn(true);
        assert!(
            immediate.is_none() || frame == 0,
            "only the first request sends at once (frame {frame})"
        );
        let resent = client.tick_pending_repairs(DT * 1000.0);
        for request in immediate.into_iter().chain(resent) {
            assert_eq!(request.reason, REFRESH_REASON_UNMERGED_LOCAL_PAWN);
            sent.push(frame);
            answers_due.push(frame + ROUND_TRIP_FRAMES);
        }
    }
    let window_ms = frames as f32 * DT * 1000.0;
    let cadence_bound = (window_ms / RESEND_MS).ceil() as usize + 1;
    assert!(sent.len() >= 2, "it keeps asking: {sent:?}");
    assert!(
        sent.len() <= cadence_bound,
        "{} requests in {window_ms} ms exceed the {RESEND_MS} ms cadence: {sent:?}",
        sent.len()
    );
    for pair in sent.windows(2) {
        let gap_ms = (pair[1] - pair[0]) as f32 * DT * 1000.0;
        assert!(
            gap_ms + DT * 1000.0 >= RESEND_MS,
            "retries {pair:?} closer than the cadence"
        );
    }

    // Once the record merges, the pending request is dropped and nothing resends.
    assert!(client.refresh_unmerged_local_pawn(false).is_none());
    assert!(!client.is_pending_repair(NetworkId(7)));
    assert!(client.tick_pending_repairs(1000.0).is_empty());
}

// The joiner's pawn walks and teleports on the host while its first record cannot
// merge, so the client still shows the stale arming pose. The receive that first
// writes the authoritative pose raises a camera hold, so the eye snaps to it rather
// than blending from the stale pose; it plays no cue and turns no view.
#[test]
fn teleport_join_gated_first_record_holds_camera_without_cue_or_turn() {
    let mut rig = Rig::new(clean_link(), &[entry_ahead()]);
    hold_tuning(&mut rig);
    arm_without_tuning(&mut rig, IDLE);
    let stale = client_pose(&rig);
    for _ in 0..240 {
        if !rig.host_teleports().is_empty() {
            break;
        }
        rig.step(FORWARD);
    }
    assert_eq!(rig.host_teleports().len(), 1, "teleported during the gate");
    rig.idle(10);
    assert_eq!(
        client_pose(&rig),
        stale,
        "the gated pawn shows its stale pose"
    );
    assert!(rig.holds.is_empty(), "no hold before any write");

    land_tuning(&mut rig);
    let mut written_at = None;
    for _ in 0..60 {
        rig.step(IDLE);
        if written_at.is_none() && client_pose(&rig) != stale {
            written_at = Some(rig.ticks.len());
        }
        if !rig.h.prediction.first_record_pending() {
            break;
        }
    }
    let written_at = written_at.expect("the authoritative pose was written");
    assert!(
        (client_pose(&rig) - stale).length() > 10.0,
        "fixture: the merged pose is far from the stale one"
    );
    assert!(
        !rig.h.prediction.first_record_pending(),
        "the record merged"
    );
    assert!(
        rig.holds.contains(&written_at),
        "the receive that wrote the pose holds the camera: {:?} vs {written_at}",
        rig.holds
    );
    let holds = rig.holds.len();
    rig.idle(30);
    assert_eq!(rig.holds.len(), holds, "no hold once the record merged");
    assert!(rig.adoptions.is_empty(), "no cut or view turn");
    assert!(rig.cues.is_empty(), "no cue");
    assert_eq!(rig.camera_yaw, 0.0);
    assert!(rig.teleport_corrections().is_empty());
}
