// Two-peer ghost-audio replication: the level-scoped `setGhostAudio` record, rebase, joins, reloads and resets.
// See: context/lib/networking.md §Current contract · context/lib/audio.md §1 (Ghost bus)

use glam::Vec3;
use log::Level;
use postretro_foundation::GhostAudioValues;
use postretro_net::wire::GHOST_AUDIO_VOLUME;
use postretro_scripting_core::runtime::ModGhostAudio;
use postretro_test_log_capture::LogCapture;
use serde_json::json;

use super::test_fixtures::*;
use crate::ghost_links::bus::CROSSED_PROFILE_WARNING;

/// One snapshot interval, in presented frames: snapshots go out every second
/// host tick.
const SNAPSHOT_INTERVAL_FRAMES: usize = 2 * FRAMES_PER_TICK;
const EPS: f32 = 1e-4;

/// The host's manifest profile.
const HOST: ModGhostAudio = ModGhostAudio {
    volume: 0.5,
    high_pass_hz: 250.0,
    low_pass_hz: 3000.0,
    drive: 0.2,
};

/// The client's own manifest: differs from the host's in every field.
const CLIENT: ModGhostAudio = ModGhostAudio {
    volume: 0.4,
    high_pass_hz: 150.0,
    low_pass_hz: 6000.0,
    drive: 0.1,
};

fn values(profile: ModGhostAudio) -> GhostAudioValues {
    crate::startup::audio_profile::ghost_audio_values(profile)
}

fn close(a: f32, b: f32, tolerance: f32) -> bool {
    (a - b).abs() <= tolerance
}

/// A session at rest with both peers on their own manifests and the client
/// joined: `ticks` ticks of nothing.
fn joined(ticks: usize) -> Loopback {
    let mut run = Loopback::new(Scenario {
        links_armed: false,
        pawns: [Vec3::new(-50.0, 0.0, 0.0), Vec3::new(50.0, 0.0, 0.0)],
        barrier: false,
    });
    run.host_bus.set_manifest(HOST);
    run.client_bus.set_manifest(CLIENT);
    run.run(ticks);
    run
}

/// The first frame from `from` on where `reached` holds, or `None`.
fn first_frame(
    frames: &[PresentedFrame],
    from: usize,
    reached: impl Fn(&GhostAudioValues) -> bool,
) -> Option<usize> {
    frames[from..]
        .iter()
        .position(|frame| reached(&frame.ghost_audio))
        .map(|offset| from + offset)
}

/// Volume change per presented frame for a ramp of `delta` over `period_ms`.
fn slope_per_frame(period_ms: f32, delta: f32) -> f32 {
    delta / (period_ms / 1000.0) * FRAME_SECONDS
}

// Audio: host `setGhostAudio` reaches a connected client at realistic
// lag, and the client's ramp ends within one snapshot interval of the host's.
// Only the fields the call set cross the wire: the client keeps its own
// manifest's cutoffs and drive, and presents the ramp, not a jump.
#[test]
fn host_set_ghost_audio_reaches_the_client_and_its_ramp_ends_with_the_hosts() {
    let mut run = joined(40);
    assert_eq!(
        run.client_frames.last().unwrap().ghost_audio,
        values(CLIENT)
    );
    assert!(
        run.client_audio_received
            .iter()
            .all(|record| record.fields == 0),
        "the always-on record rides empty before any script sets a field"
    );
    assert!(!run.client_audio_received.is_empty());
    let start = run.host_frames.len();
    let received = run.client_audio_received.len();
    run.host_set_ghost_audio(json!({ "volume": 0.9, "periodMs": 500 }));
    run.run(120);

    let ended = |audio: &GhostAudioValues| close(audio.volume, 0.9, EPS);
    let host_end = first_frame(&run.host_frames, start, ended).expect("the host ramp ends");
    let client_end = first_frame(&run.client_frames, start, ended).expect("the client ramp ends");
    assert!(
        host_end.abs_diff(client_end) <= SNAPSHOT_INTERVAL_FRAMES,
        "client ramp ended at frame {client_end}, host at {host_end}"
    );
    let moved = first_frame(&run.client_frames, start, |audio| {
        !close(audio.volume, CLIENT.volume, EPS)
    })
    .unwrap();
    let first = run.client_frames[moved].ghost_audio.volume;
    assert!(
        first < 0.9 - 0.05,
        "the client presents the ramp, not a jump to its target: {first}"
    );
    assert!(
        run.client_audio_received[received..]
            .iter()
            .all(|record| record.fields & !GHOST_AUDIO_VOLUME == 0),
        "only volume crosses the wire"
    );
    let last = run.client_frames.last().unwrap().ghost_audio;
    assert_eq!(
        (last.high_pass_hz, last.low_pass_hz, last.drive),
        (CLIENT.high_pass_hz, CLIENT.low_pass_hz, CLIENT.drive),
        "unset fields stay the client's own"
    );
    // The host's ramp starts from its own unclamped value: its manifest's.
    let ramp = run.host_bus.script_fields().volume.unwrap();
    assert_eq!((ramp.from, ramp.to), (HOST.volume, 0.9));
}

// The always-on record repeats the same ramp every snapshot. With
// the ramp received, a client whose RTT estimate swings presents exactly what
// one with a steady estimate presents: rebased once and never again.
#[test]
fn repeated_identical_ghost_audio_records_do_not_move_the_presented_bus_as_rtt_changes() {
    let presented = |swing: bool| {
        let mut run = joined(40);
        run.host_set_ghost_audio(json!({ "volume": 1.0, "lowPassHz": 1000, "periodMs": 1500 }));
        run.run(12);
        assert!(
            run.client_audio_received
                .last()
                .is_some_and(|record| record.fields & GHOST_AUDIO_VOLUME != 0),
            "the ramp record has landed before the RTT swings"
        );
        let from = run.client_frames.len();
        let records = run.client_audio_received.len();
        for tick in 0..100 {
            if swing {
                run.client_rtt_micros = Some(if tick % 2 == 0 { 40_000.0 } else { 300_000.0 });
            }
            run.run(1);
        }
        let repeats = run.client_audio_received.len() - records;
        assert!(repeats >= 10, "the ramp rode {repeats} more records");
        run.client_frames[from..]
            .iter()
            .map(|frame| frame.ghost_audio)
            .collect::<Vec<_>>()
    };
    let steady = presented(false);
    let swinging = presented(true);
    assert!(steady.iter().any(|audio| audio.volume < 1.0 - EPS));
    assert_eq!(steady, swinging);
}

// A client joining mid-`setGhostAudio` ramp presents its own
// manifest until its first snapshot lands, then the rebased current value of
// the host's ramp, which tracks the host's and ends with it.
#[test]
fn a_late_joiner_mid_ghost_audio_ramp_presents_the_hosts_current_value_and_ends_with_it() {
    let mut run = Loopback::new(Scenario {
        links_armed: false,
        pawns: [Vec3::new(-50.0, 0.0, 0.0), Vec3::new(50.0, 0.0, 0.0)],
        barrier: false,
    });
    run.host_bus.set_manifest(HOST);
    run.client_bus.set_manifest(CLIENT);
    run.host_set_ghost_audio(json!({ "volume": 1.0, "periodMs": 1500 }));
    // Host-only for 0.6 s of the 1.5 s ramp: the client has seen nothing.
    run.run_host_only(36);
    let joined = run.host_frames.len();
    run.run(120);

    let client = &run.client_frames;
    let host = &run.host_frames[joined..];
    assert_eq!(client[0].ghost_audio, values(CLIENT), "local spawn state");
    let landed =
        first_frame(client, 0, |audio| *audio != values(CLIENT)).expect("the first record lands");
    let first = client[landed].ghost_audio.volume;
    assert!(
        first > HOST.volume + 0.1 && first < 1.0 - 0.1,
        "the joiner lands mid-ramp, not at the from-value: {first}"
    );
    let tolerance = slope_per_frame(1500.0, 1.0 - HOST.volume) * SNAPSHOT_INTERVAL_FRAMES as f32;
    for (index, (client, host)) in client.iter().zip(host).enumerate().skip(landed) {
        let (client, host) = (client.ghost_audio.volume, host.ghost_audio.volume);
        assert!(
            close(client, host, tolerance),
            "frame {index}: client {client} vs host {host}"
        );
    }
    let ended = |audio: &GhostAudioValues| close(audio.volume, 1.0, EPS);
    let client_end = first_frame(client, 0, ended).expect("the client ramp ends");
    let host_end = first_frame(host, 0, ended).expect("the host ramp ends");
    assert!(client_end.abs_diff(host_end) <= SNAPSHOT_INTERVAL_FRAMES);
}

// A record landing before the presentation clock anchors (no RTT
// sample yet) defers its ramp: the client keeps presenting its own manifest.
// Once the clock anchors the ramp rebases and presents the host's current value.
#[test]
fn a_ghost_audio_record_before_the_presentation_clock_defers_the_ramp() {
    let mut run = Loopback::new(Scenario {
        links_armed: false,
        pawns: [Vec3::new(-50.0, 0.0, 0.0), Vec3::new(50.0, 0.0, 0.0)],
        barrier: false,
    });
    run.host_bus.set_manifest(HOST);
    run.client_bus.set_manifest(CLIENT);
    run.client_rtt_micros = None;
    run.run(20);
    run.host_set_ghost_audio(json!({ "volume": 0.9, "periodMs": 2000 }));
    run.run(40);
    assert!(
        run.client_audio_received
            .iter()
            .any(|record| record.fields == GHOST_AUDIO_VOLUME),
        "the record landed"
    );
    assert_eq!(
        run.client_frames.last().unwrap().ghost_audio,
        values(CLIENT),
        "the ramp waits for the clock"
    );

    run.client_rtt_micros = Some(120_000.0);
    let anchored = run.client_frames.len();
    run.run(10);
    let tolerance = slope_per_frame(2000.0, 0.4) * SNAPSHOT_INTERVAL_FRAMES as f32;
    for index in anchored..run.client_frames.len() {
        let (client, host) = (
            run.client_frames[index].ghost_audio.volume,
            run.host_frames[index].ghost_audio.volume,
        );
        assert!(
            client > HOST.volume + 0.05 && close(client, host, tolerance),
            "frame {index}: client {client} vs host {host}"
        );
    }
}

// Client half: host-set cutoffs valid against the host's manifest cross
// against the client's own `audio.ghost`. The client presents the floor
// cutoffs, warning once however many frames resolve, while the host presents
// its set high-pass.
#[test]
fn a_client_whose_own_manifest_crosses_host_set_cutoffs_presents_the_floor_cutoffs_and_warns_once()
{
    let capture = LogCapture::start();
    let mut run = joined(20);
    run.client_bus.set_manifest(ModGhostAudio {
        low_pass_hz: 2000.0,
        ..CLIENT
    });
    run.host_set_ghost_audio(json!({ "highPassHz": 2500 }));
    run.run(60);

    let host = run.host_frames.last().unwrap().ghost_audio;
    assert_eq!(
        (host.high_pass_hz, host.low_pass_hz),
        (2500.0, HOST.low_pass_hz)
    );
    let client = run.client_frames.last().unwrap().ghost_audio;
    assert_eq!(
        (client.high_pass_hz, client.low_pass_hz),
        (
            ModGhostAudio::FLOOR.high_pass_hz,
            ModGhostAudio::FLOOR.low_pass_hz
        )
    );
    assert_eq!(
        (client.volume, client.drive),
        (CLIENT.volume, CLIENT.drive),
        "only the cutoffs fall back"
    );
    capture.assert_logged_once(Level::Warn, CROSSED_PROFILE_WARNING);
}

// Both peers: a committed reload changing `audio.ghost` on either
// peer mid-ramp re-resolves that peer's unset fields, and the host's
// `setGhostAudio` ramp runs on to its target on both.
#[test]
fn a_staged_reload_on_either_peer_re_resolves_its_bus_and_the_host_fields_stand() {
    let mut run = joined(40);
    run.host_set_ghost_audio(json!({ "volume": 1.0, "periodMs": 1500 }));
    run.run(30);
    let before = [
        run.host_frames.last().unwrap().ghost_audio,
        run.client_frames.last().unwrap().ghost_audio,
    ];
    let host_reloaded = ModGhostAudio {
        volume: 0.1,
        low_pass_hz: 5000.0,
        ..HOST
    };
    let client_reloaded = ModGhostAudio {
        volume: 0.2,
        low_pass_hz: 8000.0,
        ..CLIENT
    };
    run.host_bus.set_manifest(host_reloaded);
    run.client_bus.set_manifest(client_reloaded);
    run.run(1);
    for ((frames, own), before) in [
        (&run.host_frames, host_reloaded),
        (&run.client_frames, client_reloaded),
    ]
    .into_iter()
    .zip(before)
    {
        let presented = frames.last().unwrap().ghost_audio;
        assert_eq!(
            presented.low_pass_hz, own.low_pass_hz,
            "an unset field follows"
        );
        assert!(
            presented.volume > before.volume && presented.volume < 1.0,
            "the script ramp runs on: {} after {}",
            presented.volume,
            before.volume
        );
    }
    run.run(90);
    assert!(close(
        run.host_frames.last().unwrap().ghost_audio.volume,
        1.0,
        EPS
    ));
    assert!(close(
        run.client_frames.last().unwrap().ghost_audio.volume,
        1.0,
        EPS
    ));
}
