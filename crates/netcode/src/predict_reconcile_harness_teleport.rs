// Teleporter co-op proofs over the conditioned-link harness: prediction, host
// arming, refusal and divergence, stamped command frames under loss, join and
// re-baseline adoption. Every scenario drives the production predicted tick,
// host command resolution, host movement and client reconcile.
// See: context/plans/done/teleporters/index.md (Acceptance) ·
//      context/lib/networking.md §Host input command queue

#![cfg(test)]

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

use glam::Vec3;

use super::command_queue::ResolutionSource;
use super::predict_reconcile_harness_teleport_rig::{
    Cue, FORWARD, IDLE, Rig, Spec, clean_link, forward_of, region, yaw_gap,
};
use super::predict_reconcile_harness_test_fixtures::{CLIENT_ID, mandated_link};
use super::prediction::{CorrectionClass, ORDINARY_CORRECTION_MAX_M, TELEPORT_CORRECTION_MIN_M};

const YAW_EPS: f32 = 1.0e-4;
/// A correction this small is numerical noise, not a correction.
const SETTLED_M: f32 = 1.0e-3;

/// The entry region straight ahead of the spawn: the body point crosses
/// `z = -6` walking forward.
fn entry() -> (Vec3, Vec3) {
    region((-3.0, 3.0), (-8.0, -6.0))
}

fn classic_entry() -> Spec {
    Spec::classic(entry(), Vec3::new(40.0, 1.25, 0.0), FRAC_PI_2)
}

/// A 90° seamless shift whose landmark sits in the entry region.
fn seamless_entry() -> Spec {
    Spec::seamless(
        entry(),
        (Vec3::new(0.0, 0.0, -7.0), 0.0),
        (Vec3::new(100.0, 0.0, 0.0), FRAC_PI_2),
    )
}

fn assert_yaw(actual: f32, expected: f32, what: &str) {
    assert!(
        yaw_gap(actual, expected) < YAW_EPS,
        "{what}: yaw {actual} != {expected}"
    );
}

/// Final agreement after the run settles: same identity, same view and avatar yaw.
fn assert_converged(rig: &Rig) {
    let client = rig.client_teleport();
    let host = rig.host_teleport();
    assert_eq!(client.generation, host.generation, "teleport generation");
    assert_eq!(
        client.last_teleporter, host.last_teleporter,
        "last teleporter"
    );
    assert!(
        (client.accumulated_yaw - host.accumulated_yaw).abs() < YAW_EPS,
        "accumulated yaw {} != {}",
        client.accumulated_yaw,
        host.accumulated_yaw
    );
    assert_yaw(rig.camera_yaw, rig.host_yaw(), "client view vs host avatar");
    assert!(
        rig.h.position_error() < 0.05,
        "client and host converge, error {}",
        rig.h.position_error()
    );
}

// A walk-in on a clean 100 ms link teleports host and client on the same
// client tick with no correction past the ordinary band; the view ends on the
// host's avatar yaw. Classic and seamless both.
#[test]
fn teleport_walkin_conditioned_link_clean_same_tick_ordinary_band() {
    for spec in [classic_entry(), seamless_entry()] {
        let mut rig = Rig::new(clean_link(), &[spec]);
        rig.join();
        let at = rig.walk_until_predicted_teleport(120);
        let predicted_tick = rig.ticks[at].client_tick;
        for _ in 0..30 {
            rig.step(FORWARD);
        }
        rig.idle(60);

        let host = rig.host_teleports();
        assert_eq!(host.len(), 1, "the host teleports once");
        assert_eq!(
            host[0]
                .resolved
                .as_ref()
                .map(|resolved| resolved.client_tick),
            Some(predicted_tick),
            "host and client teleport on the same client tick"
        );
        assert_eq!(
            rig.ticks
                .iter()
                .filter(|tick| tick.teleport.is_some())
                .count(),
            1
        );
        assert!(rig.adoptions.is_empty(), "no adoption cut");
        assert!(!rig.corrections.is_empty(), "the client reconciled");
        for correction in &rig.corrections {
            assert_eq!(correction.class, CorrectionClass::Ordinary);
            assert!(correction.magnitude <= ORDINARY_CORRECTION_MAX_M);
        }
        assert_eq!(rig.cues, vec![Cue::Predicted(0)]);
        assert_converged(&rig);
    }
}

// Under the harness's lossy profile corrections stay in the ordinary band
// and the view still ends on the host's yaw.
#[test]
fn teleport_walkin_conditioned_link_lossy_ordinary_band() {
    for spec in [classic_entry(), seamless_entry()] {
        let mut rig = Rig::new(mandated_link(), &[spec]);
        rig.join();
        rig.walk_until_predicted_teleport(120);
        for _ in 0..30 {
            rig.step(FORWARD);
        }
        rig.idle(90);

        assert_eq!(rig.host_teleports().len(), 1, "the host teleports once");
        assert!(rig.adoptions.is_empty(), "no adoption cut");
        assert!(!rig.corrections.is_empty(), "the client reconciled");
        for correction in &rig.corrections {
            assert_eq!(
                correction.class,
                CorrectionClass::Ordinary,
                "{correction:?}"
            );
            assert!(correction.magnitude <= ORDINARY_CORRECTION_MAX_M);
        }
        assert!(
            rig.h.to_server.dropped() > 0 || rig.h.to_client.dropped() > 0,
            "the lossy profile dropped packets"
        );
        assert_converged(&rig);
    }
}

/// The room the client stands in when a script arms it: a 90° seamless shift
/// to a copy at `x = 200`. Large enough to walk forward in throughout.
fn shift_room() -> Spec {
    Spec::seamless(
        region((-10.0, 10.0), (-40.0, 5.0)),
        (Vec3::ZERO, 0.0),
        (Vec3::new(200.0, 0.0, 0.0), FRAC_PI_2),
    )
    .disarmed()
}

/// Host ticks after its teleport whose command still carries the stamp the
/// client sampled before it knew of the teleport.
fn host_ticks_after_teleport(
    rig: &Rig,
) -> Vec<super::predict_reconcile_harness_test_fixtures::HostTickTrace> {
    let teleport_tick = rig.host_teleports()[0].server_tick;
    rig.h
        .host_trace
        .iter()
        .filter(|entry| entry.server_tick > teleport_tick)
        .cloned()
        .collect()
}

/// The run of one scripted shift with the arm landing `offset` ticks into the
/// client's forward walk.
fn scripted_shift(offset: usize) -> Rig {
    let mut rig = Rig::new(clean_link(), &[shift_room()]);
    rig.join();
    for _ in 0..15 + offset {
        rig.step(FORWARD);
    }
    rig.host_set_armed(&[(0, true)]);
    for _ in 0..40 {
        rig.step(FORWARD);
    }
    rig.idle(40);
    rig
}

// At 100 ms a script arms the occupied seamless room at a 90°
// landmark delta while the client holds forward. On the host every command the
// client sampled before it knew of the shift faces and moves along the rotated
// frame. On the client the shift lands once — as one teleport-class cut when the
// pawn record carrying it applies with (or before) the replicated armed state —
// with the view turned 90°, one cue, and no later correction.
//
// Each entity's delta applies on its own newest-wins baseline, so the armed
// record (an unchanged baseline until the arm) can apply a snapshot before the
// moving pawn's record does. The client then predicts the shift itself from
// replicated armed state; a seamless shift is an isometry, so that path agrees
// with the host exactly. Both paths are asserted; the cut path must occur.
#[test]
fn teleport_scripted_shift_inflight_input_moves_along_new_facing_with_one_cut() {
    let mut cut_paths = 0;
    for offset in 0..6 {
        let rig = scripted_shift(offset);

        // Host: one shift; every later tick faces and moves in the rotated frame,
        // including commands stamped before the client knew of the shift.
        let teleports = rig.host_teleports();
        assert_eq!(teleports.len(), 1, "the host shifts once");
        let after = host_ticks_after_teleport(&rig);
        assert!(
            after.iter().any(|entry| entry
                .resolved
                .as_ref()
                .is_some_and(|resolved| resolved.teleport_yaw_stamp == 0.0)),
            "commands sampled before the client knew reached the host after the shift"
        );
        let mut previous = teleports[0].position;
        for entry in after.iter().take(40) {
            assert_yaw(entry.yaw, FRAC_PI_2, "host avatar after the shift");
            let resolved = entry.resolved.as_ref().unwrap();
            assert_yaw(
                resolved.command.movement.facing_yaw,
                FRAC_PI_2,
                "host resolved facing",
            );
            let step = entry.position - previous;
            let horizontal = Vec3::new(step.x, 0.0, step.z);
            if horizontal.length() > 0.01 {
                assert!(
                    horizontal.normalize().dot(forward_of(FRAC_PI_2)) > 0.999,
                    "host moves along the new facing, step {step:?}"
                );
            }
            previous = entry.position;
        }

        // Client: the shift lands exactly once, with one cue and the 90° turn.
        let predicted: Vec<_> = rig.ticks.iter().filter_map(|tick| tick.teleport).collect();
        let cuts = rig.teleport_corrections();
        assert_eq!(predicted.len() + rig.adoptions.len(), 1, "offset {offset}");
        assert_eq!(rig.cues.len(), 1, "the cue plays once");
        let landed_after = if let Some(adoption) = rig.adoptions.first() {
            cut_paths += 1;
            assert_eq!(cuts.len(), 1, "one teleport-class cut");
            assert!(adoption.camera_cut);
            assert!((adoption.yaw_delta - FRAC_PI_2).abs() < YAW_EPS);
            assert_eq!(rig.cues, vec![Cue::Adopted(0)]);
            cuts[0].after_client_ticks
        } else {
            assert!(cuts.is_empty());
            assert!((predicted[0].yaw_delta - FRAC_PI_2).abs() < YAW_EPS);
            assert_eq!(rig.cues, vec![Cue::Predicted(0)]);
            rig.ticks
                .iter()
                .position(|tick| tick.teleport.is_some())
                .unwrap()
                + 1
        };
        // In-flight commands replayed in the rotated frame exactly as the host
        // ran them: no later snapshot corrects the client.
        for correction in rig
            .corrections
            .iter()
            .filter(|correction| correction.after_client_ticks > landed_after)
        {
            assert_eq!(correction.class, CorrectionClass::Ordinary);
            assert!(
                correction.magnitude < SETTLED_M,
                "offset {offset}: {correction:?}"
            );
        }
        assert_converged(&rig);
        assert_yaw(rig.camera_yaw, FRAC_PI_2, "client view");
    }
    assert!(cut_paths > 0, "some arm timing reaches the client as a cut");
}

/// A two-way seamless pair sharing its two points: `era_a` (yaw 0) inside
/// region A and `era_b` at `x = 100` (yaw 90°). Region B is region A's image
/// under A's shift, so each direction's arrival lands inside the other.
fn two_way_pair() -> [Spec; 2] {
    let era_a = (Vec3::new(0.0, 0.0, -7.0), 0.0);
    let era_b = (Vec3::new(100.0, 0.0, 0.0), FRAC_PI_2);
    [
        Spec::seamless(region((-3.0, 3.0), (-12.0, -6.0)), era_a, era_b),
        Spec::seamless(region((95.0, 101.0), (-3.0, 3.0)), era_b, era_a),
    ]
}

// One call arms both directions of a two-way seamless pair while
// the remote client stands latched in B. The host fires once (B to A, latched
// against A on arrival); the client shifts once — by adoption, since it never
// clears a latch on an observed armed change — with no ping-pong, no cut back
// and one cue.
#[test]
fn teleport_arm_pair_latched_remote_shifts_once_without_ping_pong() {
    let mut rig = Rig::new(clean_link(), &two_way_pair());
    rig.join();
    rig.walk_until_predicted_teleport(120);
    rig.idle(30);
    assert_eq!(
        rig.client_teleport().arrival_latches,
        vec![1],
        "latched in B"
    );
    assert_eq!(rig.host_teleport().arrival_latches, vec![1]);
    assert_eq!(rig.host_teleport().generation, 1);

    rig.step_with(IDLE, true, &[(0, false), (1, false)]);
    rig.idle(20);
    assert!(!rig.client_armed(0) && !rig.client_armed(1));
    assert_eq!(
        rig.client_teleport().arrival_latches,
        vec![1],
        "a disarm keeps the latch"
    );
    let cues_before = rig.cues.len();
    let ticks_before = rig.ticks.len();

    rig.step_with(IDLE, true, &[(0, true), (1, true)]);
    rig.idle(60);

    let host = rig.host_teleport();
    assert_eq!(host.generation, 2, "the host fires exactly once on the arm");
    assert_eq!(host.last_teleporter, 1, "B fired");
    assert_eq!(
        host.arrival_latches,
        vec![0],
        "latched against A on arrival"
    );
    assert!(rig.client_armed(0) && rig.client_armed(1));
    let predicted_after = rig.ticks[ticks_before..]
        .iter()
        .filter(|tick| tick.teleport.is_some())
        .count();
    let cuts_after = rig
        .teleport_corrections()
        .iter()
        .filter(|cut| cut.after_client_ticks > ticks_before)
        .count();
    assert_eq!(
        predicted_after + rig.adoptions.len(),
        1,
        "the client shifts once"
    );
    assert!(cuts_after <= 1, "no cut back");
    assert_eq!(rig.cues.len() - cues_before, 1, "one cue, no stray sound");
    assert_eq!(rig.client_teleport().arrival_latches, vec![0]);
    assert_converged(&rig);
}

// The predicting client's cue comes from forward prediction once; the
// reconciles that replay the teleport tick, and the one that adopts the host's
// agreeing record, add no adoption and no cue.
#[test]
fn teleport_prediction_sound_once_agreeing_reconcile_adds_no_cue() {
    let mut rig = Rig::new(clean_link(), &[classic_entry()]);
    rig.join();
    let at = rig.walk_until_predicted_teleport(120);
    let teleport_tick = rig.ticks[at].client_tick;
    rig.idle(60);
    let replays = rig
        .corrections
        .iter()
        .filter(|correction| {
            correction.after_client_ticks > at
                && correction.acked.is_some_and(|acked| acked < teleport_tick)
        })
        .count();
    let adopting = rig
        .corrections
        .iter()
        .filter(|correction| correction.acked.is_some_and(|acked| acked >= teleport_tick))
        .count();
    assert!(replays > 0, "a reconcile replayed the teleport tick");
    assert!(
        adopting > 0,
        "a reconcile adopted the host's teleported record"
    );
    assert!(rig.adoptions.is_empty());
    assert_eq!(rig.cues, vec![Cue::Predicted(0)]);
    assert_converged(&rig);
}

/// A classic entry close to the spawn whose destination lies 1 m from the
/// crossing point, turning the view 90°.
fn short_classic() -> Spec {
    Spec::classic(
        region((-3.0, 3.0), (-3.0, -1.0)),
        Vec3::new(1.0, 1.25, -1.0),
        FRAC_PI_2,
    )
}

/// Walk into teleporter `index`'s region; on the tick the client predicts it,
/// the host disarms it before resolving that command. Returns the predicted
/// tick's index.
fn walk_in_and_host_disarms(rig: &mut Rig, index: usize, keep_walking: bool) -> usize {
    let at = rig.walk_until_predicted_teleport(120);
    assert_eq!(rig.ticks[at].teleport.unwrap().teleporter, index as u16);
    assert!(
        rig.h
            .command_queues
            .resolved_cursor(CLIENT_ID)
            .is_some_and(|cursor| cursor < rig.ticks[at].client_tick),
        "the host has not resolved the crossing yet"
    );
    rig.host_set_armed(&[(index, false)]);
    let wish = if keep_walking { FORWARD } else { IDLE };
    for _ in 0..30 {
        rig.step(wish);
    }
    rig.idle(60);
    at
}

/// The predicted tick's arrival relative to the client's pose before it.
fn predicted_teleport_distance(rig: &Rig, at: usize) -> f32 {
    rig.ticks[at]
        .teleport
        .unwrap()
        .arrival
        .distance(rig.ticks[at - 1].position_after)
}

fn assert_no_correction_after(rig: &Rig, client_ticks: usize) {
    for correction in rig
        .corrections
        .iter()
        .filter(|correction| correction.after_client_ticks > client_ticks)
    {
        assert_eq!(
            correction.class,
            CorrectionClass::Ordinary,
            "{correction:?}"
        );
        assert!(correction.magnitude < SETTLED_M, "{correction:?}");
    }
}

/// Host ticks never present the avatar facing `yaw`.
fn assert_host_never_faces(rig: &Rig, yaw: f32) {
    for entry in &rig.h.host_trace {
        assert!(
            yaw_gap(entry.yaw, yaw) > 0.1,
            "host avatar showed the predicted facing at server tick {}",
            entry.server_tick
        );
    }
}

// The host refuses an entry the client
// predicted (a disarm it had not seen). The client cuts back once, through the
// teleport identity although the arrival was 1 m away, with its yaw restored.
// In-flight commands, stamped in the predicted frame, run in the pawn's actual
// frame on the host and in replay; the host avatar never shows the predicted
// facing, and no correction follows the cut. The cut back plays no cue.
#[test]
fn teleport_refused_entry_short_distance_cuts_back_with_yaw_restored() {
    let mut rig = Rig::new(clean_link(), &[short_classic()]);
    rig.join();
    let at = walk_in_and_host_disarms(&mut rig, 0, false);
    assert!(
        predicted_teleport_distance(&rig, at) < 1.2,
        "a 1 m teleport"
    );

    assert!(rig.host_teleports().is_empty(), "the host refused");
    let cuts = rig.teleport_corrections();
    assert_eq!(cuts.len(), 1, "one cut back");
    assert!(
        cuts[0].magnitude < TELEPORT_CORRECTION_MIN_M,
        "distance alone would not cut: {}",
        cuts[0].magnitude
    );
    assert_eq!(rig.adoptions.len(), 1);
    assert!((rig.adoptions[0].yaw_delta + FRAC_PI_2).abs() < YAW_EPS);
    assert_eq!(rig.adoptions[0].cue_teleporter, None);
    assert_eq!(rig.cues, vec![Cue::Predicted(0)], "the cut back plays none");
    assert_yaw(rig.camera_yaw, 0.0, "view restored");
    let in_flight: Vec<_> = rig
        .h
        .host_trace
        .iter()
        .filter_map(|entry| entry.resolved.as_ref())
        .filter(|resolved| resolved.teleport_yaw_stamp != 0.0)
        .collect();
    assert!(
        !in_flight.is_empty(),
        "commands stamped in the predicted frame reached the host"
    );
    for resolved in in_flight {
        assert_yaw(
            resolved.command.movement.facing_yaw,
            0.0,
            "host runs the actual frame",
        );
    }
    assert_host_never_faces(&rig, FRAC_PI_2);
    assert_no_correction_after(&rig, cuts[0].after_client_ticks);
    assert_converged(&rig);
}

/// Two classic teleporters inside one round trip: T1 turns the view 90° and
/// lands the pawn just short of T2, which turns it another 90°.
fn back_to_back() -> [Spec; 2] {
    [
        Spec::classic(
            region((-3.0, 3.0), (-3.0, -1.0)),
            Vec3::new(0.0, 1.25, -20.0),
            FRAC_PI_2,
        ),
        Spec::classic(
            region((-3.0, -0.2), (-23.0, -17.0)),
            Vec3::new(0.0, 1.25, -60.0),
            PI,
        ),
    ]
}

fn walk_through_back_to_back(rig: &mut Rig, refuse_second: bool) -> (usize, usize) {
    rig.join();
    let first = rig.walk_until_predicted_teleport(120);
    let second = rig.walk_until_predicted_teleport(20);
    if refuse_second {
        rig.host_set_armed(&[(1, false)]);
    }
    for _ in 0..20 {
        rig.step(FORWARD);
    }
    rig.idle(60);
    (first, second)
}

// Two predicted teleports inside one round trip. With the second
// refused the client takes exactly one cut and its yaw ends at the host's;
// with both agreed it takes none.
#[test]
fn teleport_reconcile_identity_sequences_two_predicted_in_one_round_trip() {
    let mut refused = Rig::new(clean_link(), &back_to_back());
    let (first, second) = walk_through_back_to_back(&mut refused, true);
    assert!(
        refused.ticks[second].client_tick - refused.ticks[first].client_tick < 6,
        "both inside one 100 ms round trip"
    );
    assert_eq!(
        refused.host_teleports().len(),
        1,
        "the host took only the first"
    );
    assert_eq!(refused.teleport_corrections().len(), 1, "exactly one cut");
    assert_eq!(refused.cues, vec![Cue::Predicted(0), Cue::Predicted(1)]);
    assert_converged(&refused);

    let mut agreed = Rig::new(clean_link(), &back_to_back());
    walk_through_back_to_back(&mut agreed, false);
    assert_eq!(agreed.host_teleports().len(), 2);
    assert!(
        agreed.teleport_corrections().is_empty(),
        "agreement takes no cut"
    );
    assert!(agreed.adoptions.is_empty());
    assert_converged(&agreed);
}

/// Two disarmed seamless rooms: S1 holds the spawn and shifts it 90° into S2,
/// which shifts it another 90°.
fn chained_rooms() -> [Spec; 2] {
    [
        Spec::seamless(
            region((-5.0, 5.0), (-5.0, 5.0)),
            (Vec3::ZERO, 0.0),
            (Vec3::new(100.0, 0.0, 0.0), FRAC_PI_2),
        )
        .disarmed(),
        Spec::seamless(
            region((95.0, 105.0), (-5.0, 5.0)),
            (Vec3::new(100.0, 0.0, 0.0), 0.0),
            (Vec3::new(200.0, 0.0, 0.0), FRAC_PI_2),
        )
        .disarmed(),
    ]
}

// Two teleports the client never
// predicted reach it in one snapshot. Script arms S1, then S2 once the pawn
// stands in it, then disarms both, all inside one host frame, so the client
// never holds armed state to predict from. One adoption lands both: the view
// turns by their sum to the host's yaw and one cue plays, the last teleporter's.
#[test]
fn teleport_reconcile_identity_sequences_two_unpredicted_in_one_snapshot_coalesce() {
    let mut rig = Rig::new(clean_link(), &chained_rooms());
    rig.join();
    rig.predict(IDLE, true);
    rig.h.advance_clock();
    rig.host_ticks_arming(
        4,
        &[&[(0, true)], &[], &[(1, true)], &[(0, false), (1, false)]],
    );
    rig.receive();
    let host = rig.host_teleport();
    assert_eq!(host.generation, 2, "two host teleports in one frame");
    assert_eq!(host.last_teleporter, 1);
    rig.idle(60);

    assert!(rig.ticks.iter().all(|tick| tick.teleport.is_none()));
    assert_eq!(rig.adoptions.len(), 1, "one adoption lands both");
    assert_yaw(
        rig.adoptions[0].yaw_delta,
        PI,
        "the view turns by both deltas",
    );
    assert_eq!(
        rig.cues,
        vec![Cue::Adopted(1)],
        "one cue, the last teleporter's"
    );
    assert_converged(&rig);
    assert_yaw(rig.camera_yaw, PI, "client view");
}

/// Two overlapping seamless teleporters sharing an entry face: X (smaller,
/// wins while armed) and Y (larger). Y's target lies 1.5 m from X's.
fn overlapping(y_target_yaw: f32) -> [Spec; 2] {
    let landmark = (Vec3::new(0.0, 0.0, -7.0), 0.0);
    [
        Spec::seamless(entry(), landmark, (Vec3::new(100.0, 0.0, 0.0), FRAC_PI_2)),
        Spec::seamless(
            (Vec3::new(-4.0, 0.0, -8.0), Vec3::new(4.0, 3.5, -6.0)),
            landmark,
            (Vec3::new(101.5, 0.0, 0.0), y_target_yaw),
        ),
    ]
}

// The client predicts X while the host,
// having disarmed X unseen, takes Y on the same client tick with the same yaw
// delta and arrivals 1.5 m apart. Generation and yaw agree, the last teleporter
// differs: one teleport-class cut, no view turn, and no second cue.
#[test]
fn teleport_reconcile_identity_sequences_same_tick_different_teleporters_cut() {
    let mut rig = Rig::new(clean_link(), &overlapping(FRAC_PI_2));
    rig.join();
    let at = walk_in_and_host_disarms(&mut rig, 0, true);
    let host = rig.host_teleports();
    assert_eq!(host.len(), 1);
    assert_eq!(host[0].teleport.last_teleporter, 1, "the host took Y");
    assert_eq!(
        host[0].resolved.as_ref().unwrap().client_tick,
        rig.ticks[at].client_tick,
        "on the client's tick"
    );
    let cuts = rig.teleport_corrections();
    assert_eq!(cuts.len(), 1, "different teleporters cut");
    assert!(cuts[0].magnitude < TELEPORT_CORRECTION_MIN_M);
    assert_eq!(rig.adoptions.len(), 1);
    assert!(rig.adoptions[0].yaw_delta.abs() < YAW_EPS);
    assert_eq!(
        rig.adoptions[0].cue_teleporter, None,
        "the speculative cue stands"
    );
    assert_eq!(rig.cues, vec![Cue::Predicted(0)]);
    assert_converged(&rig);
}

// As above, but Y turns 45° where X turned 90°. Commands
// stamped in X's frame run in Y's on the host and in replay, the host avatar
// never shows X's facing, and nothing corrects the client after the cut.
#[test]
fn teleport_divergent_command_frames_different_teleporters_run_in_actual_frame() {
    let mut rig = Rig::new(clean_link(), &overlapping(FRAC_PI_4));
    rig.join();
    walk_in_and_host_disarms(&mut rig, 0, true);
    let cuts = rig.teleport_corrections();
    assert_eq!(cuts.len(), 1);
    assert!((rig.adoptions[0].yaw_delta + FRAC_PI_4).abs() < YAW_EPS);
    let teleport_tick = rig.host_teleports()[0].server_tick;
    let mut stamped_in_x = 0;
    for entry in rig
        .h
        .host_trace
        .iter()
        .filter(|entry| entry.server_tick >= teleport_tick)
    {
        assert_yaw(entry.yaw, FRAC_PI_4, "host avatar in Y's frame");
        if entry
            .resolved
            .as_ref()
            .is_some_and(|resolved| (resolved.teleport_yaw_stamp - FRAC_PI_2).abs() < YAW_EPS)
        {
            stamped_in_x += 1;
        }
    }
    assert!(
        stamped_in_x > 0,
        "commands stamped in X's frame reached the host"
    );
    assert_host_never_faces(&rig, FRAC_PI_2);
    assert_no_correction_after(&rig, cuts[0].after_client_ticks);
    assert_converged(&rig);
    assert_yaw(rig.camera_yaw, FRAC_PI_4, "client view");
}

// The client predicts a classic entry one
// tick before the host takes it (a host-only shove the client never predicted
// holds the host pawn back). The command after the client's crossing, stamped
// in the arrival frame, runs on the host in the source frame, so the host
// arrives with the same yaw: no cut, the avatar never shows the predicted
// facing early, nothing corrects the client once it reconciles past the
// crossing, and the crossing sounds once.
#[test]
fn teleport_divergent_command_frames_one_tick_skew_takes_no_cut_or_second_cue() {
    let mut rig = Rig::new(clean_link(), &[classic_entry()]);
    rig.join();
    let at = rig.walk_until_predicted_teleport(120);
    let crossing = rig.ticks[at].client_tick;
    let shove = rig.ticks[at - 1].position_after - rig.ticks[at - 2].position_after;
    let pawn = rig.h.host_pawn;
    let mut transform = *rig
        .h
        .host_registry
        .get_component::<postretro_entities::Transform>(pawn)
        .unwrap();
    transform.position -= shove;
    rig.h.host_registry.set_component(pawn, transform).unwrap();
    for _ in 0..20 {
        rig.step(FORWARD);
    }
    rig.idle(60);

    let host = rig.host_teleports();
    assert_eq!(host.len(), 1);
    let host_crossing = host[0].resolved.as_ref().unwrap();
    assert_eq!(
        host_crossing.client_tick,
        crossing + 1,
        "the host crosses one tick later"
    );
    assert!(
        (host_crossing.teleport_yaw_stamp - FRAC_PI_2).abs() < YAW_EPS,
        "its command was stamped in the arrival frame"
    );
    assert_yaw(
        host_crossing.command.movement.facing_yaw,
        0.0,
        "and ran in the source frame",
    );
    for entry in rig
        .h
        .host_trace
        .iter()
        .filter(|entry| entry.server_tick < host[0].server_tick)
    {
        assert_yaw(entry.yaw, 0.0, "host avatar before its crossing");
    }
    assert!(rig.teleport_corrections().is_empty(), "no cut");
    assert!(rig.adoptions.is_empty());
    assert_eq!(
        rig.cues,
        vec![Cue::Predicted(0)],
        "the skewed crossing sounds once"
    );
    let reconciled_past = rig
        .corrections
        .iter()
        .find(|correction| correction.acked.is_some_and(|acked| acked > crossing))
        .unwrap()
        .after_client_ticks;
    assert_no_correction_after(&rig, reconciled_past);
    assert_converged(&rig);
}

// Under the lossy profile, with the uplink cut for six ticks across the
// client's seamless teleport tick, its commands there never reach the host.
// The host's held and neutral commands (reusing the last real, pre-teleport
// facing and stamp) and the late real commands after the gap all face and
// move in the post-teleport frame; the avatar the host replicates never turns
// back to the old facing.
#[test]
fn teleport_gap_command_stamp_held_neutral_late_run_in_post_teleport_frame() {
    let mut rig = Rig::new(mandated_link(), &[seamless_entry()]);
    rig.join();
    let mut dropped = 0;
    let mut teleported = false;
    for _ in 0..200 {
        // The gap opens within the held-command budget's reach of the trigger
        // at z = -6, so the host's held forward commands carry it across.
        let near = rig.ticks.last().unwrap().position_after.z <= -5.8;
        let drop = !teleported && near || (dropped > 0 && dropped < 6);
        teleported |= rig.step_with(FORWARD, !drop, &[]).is_some();
        if drop {
            dropped += 1;
        }
        if teleported && dropped >= 6 {
            break;
        }
    }
    assert!(
        teleported && dropped == 6,
        "the teleport tick fell inside the lost window"
    );
    for _ in 0..30 {
        rig.step(FORWARD);
    }
    rig.idle(90);

    let teleports = rig.host_teleports();
    assert_eq!(teleports.len(), 1);
    let mut sources = Vec::new();
    let mut previous = teleports[0].position;
    for entry in host_ticks_after_teleport(&rig).iter().take(60) {
        assert_yaw(entry.yaw, FRAC_PI_2, "host avatar never turns back");
        let resolved = entry.resolved.as_ref().unwrap();
        assert_yaw(
            resolved.command.movement.facing_yaw,
            FRAC_PI_2,
            "resolved facing",
        );
        let step = entry.position - previous;
        let horizontal = Vec3::new(step.x, 0.0, step.z);
        if horizontal.length() > 0.01 {
            assert!(
                horizontal.normalize().dot(forward_of(FRAC_PI_2)) > 0.999,
                "{:?} moves along the post-teleport frame, step {step:?}",
                resolved.source
            );
        }
        previous = entry.position;
        sources.push((resolved.source, resolved.teleport_yaw_stamp));
    }
    for source in [ResolutionSource::Held, ResolutionSource::Neutral] {
        assert!(
            sources
                .iter()
                .any(|&(seen, stamp)| seen == source && stamp == 0.0),
            "a {source:?} command with the pre-teleport stamp ran after the teleport: {sources:?}"
        );
    }
    assert!(
        sources
            .iter()
            .any(|&(seen, stamp)| seen == ResolutionSource::Real && stamp != 0.0),
        "late real commands ran after the gap"
    );
    assert_converged(&rig);
}

/// Walk in three-tick client frames (three predicted sends, then the host
/// catches up and one receive) after `lead` single steps. Returns the frame's
/// first tick index and the teleport's offset in it, if not the last tick.
fn multi_tick_walk(spec: Spec, lead: usize) -> Option<(Rig, usize, usize)> {
    let mut rig = Rig::new(clean_link(), &[spec]);
    rig.join();
    for _ in 0..lead {
        rig.step(FORWARD);
    }
    for _ in 0..60 {
        let first = rig.ticks.len();
        for _ in 0..3 {
            rig.predict(FORWARD, true);
        }
        for _ in 0..3 {
            rig.h.advance_clock();
            rig.host_ticks(1);
        }
        rig.receive();
        if let Some(offset) = rig.ticks[first..]
            .iter()
            .position(|tick| tick.teleport.is_some())
        {
            return (offset < 2).then_some((rig, first, offset));
        }
    }
    None
}

// Commands sampled on a later tick of a multi-tick client frame, and
// a command sent on a zero-tick frame, carry the teleport frame their facing was
// sampled in, so the host runs them exactly as forward prediction did.
#[test]
fn teleport_input_sampling_seams_multi_tick_and_zero_tick_frames() {
    for spec in [classic_entry(), seamless_entry()] {
        // Phase the walk so the teleport is not the last tick of its frame.
        let (mut rig, first, offset) = (0..3)
            .find_map(|lead| multi_tick_walk(spec, lead))
            .expect("some phase predicts the teleport before a frame's last tick");
        // A zero-tick frame right after: one extra send at the turned view.
        let zero_facing = rig.camera_yaw;
        let zero_tick = rig.h.client_send_zero_tick(&rig.command(IDLE));
        for _ in 0..30 {
            rig.step(FORWARD);
        }

        let arrival_yaw = rig.camera_yaw;
        for tick in &rig.ticks[first + offset..first + 3] {
            let host = rig
                .h
                .host_trace
                .iter()
                .find(|entry| {
                    entry
                        .resolved
                        .as_ref()
                        .is_some_and(|r| r.client_tick == tick.client_tick)
                })
                .expect("the host resolved the frame's command");
            assert_yaw(host.yaw, tick.yaw_after, "host faces as predicted");
            assert!(
                host.position.distance(tick.position_after) < SETTLED_M,
                "host moves as predicted: {} vs {}",
                host.position,
                tick.position_after
            );
        }
        let later = &rig.ticks[first + offset + 1];
        assert_yaw(
            later.facing,
            arrival_yaw,
            "the later tick sampled the turned view",
        );
        let zero = rig
            .h
            .host_trace
            .iter()
            .filter_map(|entry| entry.resolved.as_ref())
            .find(|resolved| resolved.client_tick == zero_tick)
            .expect("the host resolved the zero-tick command");
        assert_yaw(
            zero.command.movement.facing_yaw,
            zero_facing,
            "zero-tick facing",
        );
        assert!(
            (zero.teleport_yaw_stamp - rig.client_teleport().accumulated_yaw).abs() < YAW_EPS,
            "stamped with the frame its facing was sampled in"
        );
    }
}

/// Run a walk into `spec` turning the view `turn` on the crossing command and
/// losing exactly that command, so the host crosses on the held previous one.
/// `crossing` comes from an identical undisturbed probe run.
fn held_crossing(spec: Spec, turn: f32) -> Rig {
    let mut probe = Rig::new(clean_link(), &[spec]);
    probe.join();
    let at = probe.walk_until_predicted_teleport(120);
    let crossing = probe.ticks[at].client_tick;

    let mut rig = Rig::new(clean_link(), &[spec]);
    rig.join();
    while rig.h.prediction.peek_next_client_tick() < crossing {
        assert!(rig.step(FORWARD).is_none());
    }
    rig.camera_yaw += turn;
    assert!(
        rig.step_with(FORWARD, false, &[]).is_some(),
        "the turned command still crosses"
    );
    for _ in 0..20 {
        rig.step(FORWARD);
    }
    rig.idle(60);
    let host = rig.host_teleports();
    assert_eq!(host.len(), 1);
    assert_eq!(
        host[0].resolved.as_ref().unwrap().source,
        ResolutionSource::Held,
        "the host crossed on the held command"
    );
    rig
}

// The client turns on the very command that crosses a classic
// teleporter, and that command is lost, so the host crosses on the held earlier
// facing. Adoption turns the view by exactly the turn that command missed; a
// seamless crossing, whose delta ignores facing, takes no extra turn.
#[test]
fn teleport_held_classic_view_turn_adopts_missed_turn_and_seamless_takes_none() {
    let turn = 0.3;
    let classic = held_crossing(classic_entry(), turn);
    assert_eq!(classic.adoptions.len(), 1);
    assert!((classic.adoptions[0].yaw_delta - turn).abs() < YAW_EPS);
    assert_converged(&classic);

    let seamless = held_crossing(seamless_entry(), turn);
    assert!(
        seamless
            .adoptions
            .iter()
            .all(|adoption| adoption.yaw_delta.abs() < YAW_EPS)
    );
    assert!(seamless.teleport_corrections().is_empty());
    assert_converged(&seamless);
}

/// A deep classic entry: a refused crossing stops inside it.
fn deep_classic() -> Spec {
    Spec::classic(
        region((-3.0, 3.0), (-14.0, -6.0)),
        Vec3::new(40.0, 1.25, 0.0),
        FRAC_PI_2,
    )
}

// A joiner whose teleporter a script re-armed (or disarmed) before
// it joined predicts from `enabled_on_spawn` until the armed record arrives —
// held back here as the join baseline budget defers it — and converges with
// one cut once it does.
#[test]
fn teleport_join_first_record_and_armed_binding_predicts_spawn_state_until_armed_arrives() {
    // Re-armed: spawn-disarmed locally, so the client never predicts the host's
    // teleport; one adoption cut converges it.
    let mut rearmed =
        Rig::with_unreplicated_teleporters(clean_link(), &[classic_entry().disarmed()]);
    rearmed.host_set_armed(&[(0, true)]);
    rearmed.join();
    for _ in 0..60 {
        rearmed.step(FORWARD);
    }
    assert!(!rearmed.client_armed(0), "still the spawn state");
    assert_eq!(rearmed.host_teleports().len(), 1);
    rearmed.replicate_teleporters();
    rearmed.idle(40);
    assert!(rearmed.client_armed(0), "the armed record arrived");
    assert!(rearmed.ticks.iter().all(|tick| tick.teleport.is_none()));
    assert_eq!(rearmed.teleport_corrections().len(), 1, "one cut");
    assert_converged(&rearmed);

    // Disarmed: spawn-armed locally, so the client predicts the entry the host
    // refuses, and keeps re-predicting it in replay until the armed record
    // arrives; then one cut takes it back.
    let mut disarmed = Rig::with_unreplicated_teleporters(clean_link(), &[deep_classic()]);
    disarmed.host_set_armed(&[(0, false)]);
    disarmed.join();
    disarmed.walk_until_predicted_teleport(120);
    disarmed.idle(40);
    assert!(disarmed.client_armed(0), "still the spawn state");
    assert!(
        disarmed.teleport_corrections().is_empty(),
        "no cut before the record"
    );
    let arrived_at = disarmed.ticks.len();
    disarmed.replicate_teleporters();
    disarmed.idle(60);
    assert!(!disarmed.client_armed(0));
    let cuts = disarmed.teleport_corrections();
    assert_eq!(cuts.len(), 1, "one cut once the armed record arrives");
    assert!(cuts[0].after_client_ticks > arrived_at);
    assert!(disarmed.host_teleports().is_empty());
    assert_converged(&disarmed);
}

/// A classic teleporter around the spawn, turning its traveller 1 rad.
fn spawn_room() -> Spec {
    Spec::classic(
        region((-3.0, 3.0), (-3.0, 3.0)),
        Vec3::new(40.0, 1.25, 0.0),
        1.0,
    )
}

/// One host movement tick before the client has joined: the pawn standing in
/// [`spawn_room`] teleports on the host alone.
fn teleport_host_pawn_before_join(rig: &mut Rig) {
    let h = &mut rig.h;
    let collision = crate::collision::moving::CombinedCollisionWorld::new(
        &h.world,
        &h.mover_colliders,
        &h.host_mover_states,
    );
    let environment = postretro_collision::EnvironmentView::new(
        h.host_level_gravity,
        &h.host_environment_volumes,
        h.host_fluid_table.viscosities(),
    );
    let mut input = rig_idle_input();
    input.facing_yaw = 0.0;
    crate::sim::run_host_movement_tick(
        &mut h.host_registry,
        &collision,
        environment,
        &[(h.host_pawn, input)],
        super::predict_reconcile_harness_test_fixtures::DT,
    );
    assert_eq!(
        rig.host_teleport().generation,
        1,
        "the pawn teleported before the client joined"
    );
}

// The joiner's pawn teleported on the host before the client's first record.
// That record is adopted silently — no cue, cut or view turn. Commands sampled
// before it carry a zero stamp (rotated on the host by the whole accumulated
// yaw); those after carry the record's yaw (unrotated).
#[test]
fn teleport_join_first_record_and_armed_binding_adopts_prior_teleport_silently() {
    let mut rig = Rig::new(clean_link(), &[spawn_room()]);
    teleport_host_pawn_before_join(&mut rig);
    rig.join();

    assert_eq!(
        rig.client_teleport(),
        rig.host_teleport(),
        "the record was adopted"
    );
    assert!(rig.adoptions.is_empty(), "no cut or view turn");
    assert!(rig.cues.is_empty(), "no cue");
    assert_eq!(rig.camera_yaw, 0.0);
    assert_eq!(
        rig.corrections[0].class,
        CorrectionClass::Ordinary,
        "first record"
    );
    let first_record = rig.first_record_client_tick.unwrap();
    let (mut before, mut after) = (0, 0);
    for resolved in rig
        .h
        .host_trace
        .iter()
        .filter_map(|entry| entry.resolved.as_ref())
    {
        if resolved.client_tick < first_record {
            before += 1;
            assert_eq!(resolved.teleport_yaw_stamp, 0.0);
            assert_yaw(
                resolved.command.movement.facing_yaw,
                1.0,
                "rotated by the full yaw",
            );
        } else {
            after += 1;
            assert!((resolved.teleport_yaw_stamp - 1.0).abs() < YAW_EPS);
            assert_yaw(
                resolved.command.movement.facing_yaw,
                0.0,
                "not rotated by that teleport",
            );
        }
    }
    assert!(
        before > 0 && after > 0,
        "commands on both sides of the record: {before}/{after}"
    );
}

fn rig_idle_input() -> crate::movement::MovementInput {
    super::predict_reconcile_harness_test_fixtures::idle_command().movement
}

// The joiner's host tuning installs after its arming baseline: the reliable
// Control payload was lost and retransmitted, and lands in a frame that
// carries no snapshot. The arming record could not merge without tuning, and
// the idle pawn's later records arrive only while its cursor advances. The pawn stands where a
// teleport before the join left it, latched in a second armed region. The
// client must neither predict from its empty record (which would fire that
// region and then cut back) nor wait for movement to change: it asks for a
// full baseline, whose record is the first to merge and adopts silently.
#[test]
fn teleport_join_unmerged_arming_record_rebaselines_before_predicting() {
    // The spawn room sends its traveller into a second region's interior.
    let arrival_region = Spec::classic(
        region((37.0, 43.0), (-3.0, 3.0)),
        Vec3::new(80.0, 1.25, 0.0),
        0.0,
    );
    let mut rig = Rig::new(clean_link(), &[spawn_room(), arrival_region]);
    teleport_host_pawn_before_join(&mut rig);
    assert_eq!(&rig.host_teleport().arrival_latches[..], &[1]);
    // The tuning send is lost: it waits on the Control lane until retransmitted.
    rig.h.hold_tuning();
    for _ in 0..200 {
        if rig.h.client_pawn.is_some() {
            break;
        }
        rig.step(IDLE);
    }
    assert!(rig.h.client_pawn.is_some(), "the client armed its pawn");
    rig.idle(3);
    assert!(rig.h.client_tuning.is_none(), "armed before its tuning");
    assert!(rig.corrections.is_empty(), "nothing merged without tuning");

    // The retransmit lands in a frame with no snapshot.
    rig.h.land_tuning();
    assert!(rig.h.client_tuning.is_some());
    for _ in 0..60 {
        if rig.client_teleport() == rig.host_teleport() {
            break;
        }
        rig.step(IDLE);
        assert!(
            rig.h.prediction.history().is_empty(),
            "no prediction before the first record merges"
        );
    }
    assert_eq!(
        rig.client_teleport(),
        rig.host_teleport(),
        "a full baseline merged the record, latch included"
    );
    rig.idle(60);

    assert!(
        rig.ticks.iter().all(|tick| tick.teleport.is_none()),
        "the latched region never fires in prediction"
    );
    assert!(rig.host_teleports().is_empty(), "nor on the host");
    assert!(rig.adoptions.is_empty(), "no cut or view turn");
    assert!(rig.cues.is_empty(), "no cue");
    assert_eq!(rig.camera_yaw, 0.0);
    assert!(rig.teleport_corrections().is_empty());
    assert_converged(&rig);
}

// A scripted shift arrives in a periodic full re-baseline of the
// client's own pawn. Only the first record for a new pawn is silent: this one
// plays the cue and turns the view.
#[test]
fn teleport_periodic_rebaseline_adoption_plays_cue_and_turns_view() {
    let mut rig = Rig::new(clean_link(), &[shift_room()]);
    rig.join();
    assert!(
        rig.adoptions.is_empty() && rig.cues.is_empty(),
        "the first record is silent"
    );
    let pawn = rig.h.host_pawn_network_id.0;
    rig.h.server_replication.request_refresh(CLIENT_ID, pawn, 0);
    rig.host_set_armed(&[(0, true)]);
    let mut adopted_in_baseline = None;
    for _ in 0..20 {
        rig.step(IDLE);
        if adopted_in_baseline.is_none() && !rig.adoptions.is_empty() {
            adopted_in_baseline = Some(rig.h.latest_full_baselines.contains(&pawn));
        }
    }
    assert_eq!(
        adopted_in_baseline,
        Some(true),
        "adopted from a full re-baseline"
    );
    assert_eq!(rig.adoptions.len(), 1);
    assert!((rig.adoptions[0].yaw_delta - FRAC_PI_2).abs() < YAW_EPS);
    assert_eq!(rig.cues, vec![Cue::Adopted(0)]);
    rig.idle(30);
    assert_converged(&rig);
}

// A correction still settling (the host shoves the pawn a little
// every tick, so every reconcile seeds a presentation offset) when a predicted
// seamless teleport fires. The teleport tick drops the offset: the presented eye
// is the arrival pose, never the arrival plus a source-room-frame vector.
#[test]
fn teleport_clears_prior_correction_offset_on_predicted_seamless_teleport() {
    let mut rig = Rig::new(clean_link(), &[seamless_entry()]);
    rig.join();
    rig.host_nudge = Vec3::new(0.01, 0.0, 0.0);
    let at = rig.walk_until_predicted_teleport(120);
    let tick = &rig.ticks[at];
    assert!(
        tick.offset_before.length() > 1.0e-3,
        "an offset was settling: {:?}",
        tick.offset_before
    );
    assert_eq!(tick.offset_after, Vec3::ZERO);
    assert!(tick.position_after.distance(tick.teleport.unwrap().arrival) < 1.0e-5);
}
