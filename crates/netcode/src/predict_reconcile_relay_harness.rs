// Own-pawn movement under the dev lag profile, over the production channel layout.
// See: context/lib/networking.md §Testing the conditioned link · §Host input command queue

use std::f32::consts::TAU;
use std::fmt::Write as _;

use glam::Vec2;
use postretro_foundation::MovementStateKind;
use postretro_net::harness::LinkConfig;

use super::command_queue::ResolutionSource;
use super::predict_reconcile_harness_test_fixtures::{forward_command, idle_command};
use super::predict_reconcile_relay_test_fixtures::{ClientCorrection, RelayHarness, dev_profile};
use super::prediction::{CorrectionClass, ORDINARY_CORRECTION_MAX_M};
use crate::sim::SimCommand;

/// Seed of the to-host link; the to-client link takes the next one, so the two
/// directions lose and jitter independently.
const STEADY_RUN_SEED: u64 = 0x1502;
/// Frames the client may take to join and start predicting.
const JOIN_FRAMES_MAX: u64 = 600;
/// A minute of the course at 60 Hz.
const COURSE_FRAMES: u64 = 60 * 60;
/// One course cycle: run, slide, strafe one way, strafe the other.
const CYCLE_FRAMES: u64 = 240;
/// The facing turns once every 20 s, so a run at 11 m/s circles about 35 m
/// across and stays on the floor for any length.
const TURN_PER_FRAME: f32 = TAU / 1200.0;

/// The course at `frame`: sustained running on a steady turn, with a slide
/// and both strafes every four seconds.
fn course_command(frame: u64) -> SimCommand {
    let mut command = forward_command(false);
    command.movement.facing_yaw = (frame as f32 * TURN_PER_FRAME) % TAU;
    match frame % CYCLE_FRAMES {
        0..90 => {}
        90..120 => command.movement.crouch_intent = true,
        120..180 => command.movement.wish_dir = Vec2::new(-1.0, 1.0).normalize(),
        _ => command.movement.wish_dir = Vec2::new(1.0, 1.0).normalize(),
    }
    command
}

fn dev_links(seed: u64) -> (LinkConfig, LinkConfig) {
    (dev_profile(seed), dev_profile(seed.wrapping_add(1)))
}

/// One course run: the reconciles the client ran during it (the join's
/// first-record adoption excluded), and a summary for failure messages.
struct SteadyRun {
    corrections: Vec<ClientCorrection>,
    summary: String,
}

fn steady_run(to_server: LinkConfig, to_client: LinkConfig) -> SteadyRun {
    let mut h = RelayHarness::new(to_server, to_client);
    assert!(
        h.run_until_predicting(&idle_command(), JOIN_FRAMES_MAX),
        "the client joins and predicts within {JOIN_FRAMES_MAX} frames"
    );
    let course_start = h.frame + 1;
    let trace_start = h.host_trace.len();
    for frame in 0..COURSE_FRAMES {
        h.frame(&course_command(frame));
    }

    let course_trace = &h.host_trace[trace_start..];
    assert!(
        course_trace
            .iter()
            .any(|trace| trace.movement_state == MovementStateKind::Slide),
        "the course slides on the host"
    );
    let corrections: Vec<ClientCorrection> = h
        .corrections
        .iter()
        .filter(|correction| {
            correction.frame >= course_start && !correction.sample.first_record_adoption
        })
        .copied()
        .collect();

    let mut summary = String::new();
    let classes = [
        CorrectionClass::Ordinary,
        CorrectionClass::Dash,
        CorrectionClass::OversizedSmoothed,
        CorrectionClass::Teleport,
    ];
    for class in classes {
        let of_class = corrections
            .iter()
            .filter(|correction| correction.sample.class == class);
        let (count, max) = of_class.fold((0, 0.0_f32), |(count, max), correction| {
            (count + 1, max.max(correction.sample.magnitude))
        });
        let _ = write!(summary, "{class:?}: {count} (max {max:.3} m); ");
    }
    let source_count = |source: ResolutionSource| {
        course_trace
            .iter()
            .filter(|trace| {
                trace
                    .resolved
                    .as_ref()
                    .is_some_and(|resolved| resolved.source == source)
            })
            .count()
    };
    let _ = write!(
        summary,
        "host ticks Real {} / Held {} / Neutral {} of {}; dropped to host {}, to client {}",
        source_count(ResolutionSource::Real),
        source_count(ResolutionSource::Held),
        source_count(ResolutionSource::Neutral),
        course_trace.len(),
        h.to_server.dropped(),
        h.to_client.dropped(),
    );
    eprintln!("[steady run] seed {:#x}: {summary}", to_server.seed);
    SteadyRun {
        corrections,
        summary,
    }
}

/// Every correction above the ordinary band, one per line.
fn above_ordinary_band(run: &SteadyRun) -> Vec<String> {
    run.corrections
        .iter()
        .filter(|correction| correction.sample.magnitude > ORDINARY_CORRECTION_MAX_M)
        .map(|correction| {
            format!(
                "frame {} server tick {} acked client tick {:?}: {:.3} m {:?} in {:?}",
                correction.frame,
                correction.sample.server_tick,
                correction.sample.acked_tick,
                correction.sample.magnitude,
                correction.sample.class,
                correction.movement_state,
            )
        })
        .collect()
}

// Regression: a connected client's own pawn rubber-bands under loss, because one lost
// datagram stalls reliable-ordered Input for the 300 ms resend.
#[test]
fn steady_run_stays_in_ordinary_band_under_dev_profile() {
    let (to_server, to_client) = dev_links(STEADY_RUN_SEED);
    let run = steady_run(to_server, to_client);
    let oversized = above_ordinary_band(&run);
    assert!(
        oversized.is_empty(),
        "{} corrections above the {ORDINARY_CORRECTION_MAX_M} m ordinary band ({}):\n{}",
        oversized.len(),
        run.summary,
        oversized.join("\n"),
    );
}

// The loss-off control: if this records a correction above the band, jitter rather
// than loss is the cause.
#[test]
fn steady_run_loss_off_records_no_oversized_correction() {
    let (to_server, to_client) = dev_links(STEADY_RUN_SEED);
    let lossless = |link: LinkConfig| LinkConfig {
        loss_probability: 0.0,
        ..link
    };
    let run = steady_run(lossless(to_server), lossless(to_client));
    let oversized = above_ordinary_band(&run);
    assert!(
        oversized.is_empty(),
        "{} corrections above the {ORDINARY_CORRECTION_MAX_M} m ordinary band with loss off ({}):\n{}",
        oversized.len(),
        run.summary,
        oversized.join("\n"),
    );
}
