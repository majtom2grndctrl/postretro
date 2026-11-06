//! Session-level SH streaming tests: worker failures, periodic log, sync-proof
//! counters, budget inputs, and mode gates. See: context/lib/testing_guide.md

use super::*;
use postretro_test_log_capture::LogCapture;
use std::time::{Duration, Instant};

/// The level's id 49, decoded as level scope does.
fn level_hints(world: &postretro_level_loader::LevelWorld) -> Arc<ClusterHints> {
    let directory = world
        .cluster_directory()
        .expect("streamed SH carries id 49");
    Arc::new(ClusterHints::decode(directory).unwrap())
}

// Regression: the permitted retry for one failed worker request emitted a duplicate warning.
#[test]
fn async_worker_failure_warns_once_across_same_identity_retry() {
    let (_temp, path) = sync_manifest_test_fixture::write_one_cluster_prl();
    let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
    let manifest = Arc::clone(world.sh_stream_manifest().expect("id 50 selects streaming"));
    let mut session = ShStreamingSession::from_manifest(manifest, level_hints(&world)).unwrap();
    session.mode = ShStreamingMode::Async;
    session.start_async_workers().unwrap();

    // The manifest retains an open file. Truncating that same file after
    // load makes both real positional worker reads fail.
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(0)
        .unwrap();
    let capture = LogCapture::start();
    let visible = VisibleCells::Culled(vec![0]);
    let empty = VisibleCells::Culled(Vec::new());
    session.prepare_async_batch(&visible, None, 0.0).unwrap();

    let wait_for_failure = |session: &mut ShStreamingSession, time| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.controller.state(0)
            != Some(crate::sh_streaming::controller::ClusterResidencyState::Failed)
        {
            assert!(Instant::now() < deadline, "worker failure did not arrive");
            std::thread::yield_now();
            session.prepare_async_batch(&visible, None, time).unwrap();
        }
    };
    wait_for_failure(&mut session, 0.0);
    capture.assert_logged_once(log::Level::Warn, "cluster 0 read/decode failed:");

    // No reach stands for the camera leaving; the visible cluster departs.
    session.prepare_async_batch(&empty, None, 0.1).unwrap();
    session.prepare_async_batch(&empty, None, 2.1).unwrap();
    session.prepare_async_batch(&visible, None, 2.2).unwrap();
    assert_eq!(session.controller.counters().retries, 1);
    wait_for_failure(&mut session, 2.2);
    assert_eq!(session.controller.permits_in_use(), 0);
    capture.assert_logged_once(log::Level::Warn, "cluster 0 read/decode failed:");
}

#[test]
fn periodic_log_line_appears_once_per_active_interval_and_never_when_idle() {
    let (_temp, path) = sync_manifest_test_fixture::write_one_cluster_prl();
    let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
    let manifest = Arc::clone(world.sh_stream_manifest().expect("id 50 selects streaming"));
    let renderer = ShResidencySnapshot::default();
    let mut session = ShStreamingSession::from_manifest(manifest, level_hints(&world)).unwrap();
    session.mode = ShStreamingMode::Async;
    session.start_async_workers().unwrap();
    let capture = LogCapture::start();
    let visible = VisibleCells::Culled(vec![0]);
    // One frame as the windowed loop runs it, with the renderer accepting
    // every ready cluster.
    let frame = |session: &mut ShStreamingSession, seconds: f64| {
        let batch = session
            .prepare_async_batch(&visible, None, seconds)
            .unwrap();
        let accepted = batch
            .ready
            .iter()
            .map(|prepared| prepared.chunk.cluster_id)
            .collect();
        session
            .controller
            .apply_drain_outcome(ShDrainOutcome {
                accepted,
                ..ShDrainOutcome::default()
            })
            .unwrap();
        session.refresh_diagnostics(Some(&renderer)).unwrap();
    };
    let lines = |capture: &LogCapture| {
        capture
            .records()
            .iter()
            .filter(|record| record.message.starts_with("[SH streaming] last"))
            .count()
    };

    // The one cluster is read and installed while render time stands at 0.
    let deadline = Instant::now() + Duration::from_secs(5);
    while session.live.installs == 0 {
        assert!(Instant::now() < deadline, "worker read did not arrive");
        std::thread::yield_now();
        frame(&mut session, 0.0);
    }
    assert_eq!(session.live.reads_issued, 1);
    assert_eq!(lines(&capture), 0, "the first interval has not elapsed");

    // 60 Hz for 20 s: the activity is reported once, at the first
    // interval boundary; the idle intervals after it stay silent.
    for step in 1..=1200u32 {
        frame(&mut session, f64::from(step) / 60.0);
    }
    assert_eq!(lines(&capture), 1);
    capture.assert_logged_once(log::Level::Info, "[SH streaming] last 5.0 s: 1 reads");
}

// Regression: capture runs sync-proof, whose frame-thread reads bypassed
// the worker counters and left every capture's read fields at zero.
#[test]
fn sync_proof_reads_fill_the_read_counters() {
    let (_temp, path) = sync_manifest_test_fixture::write_one_cluster_prl();
    let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
    let manifest = Arc::clone(world.sh_stream_manifest().expect("id 50 selects streaming"));
    let encoded_bytes = manifest.payloads().index[0].payload_len;
    let mut session = ShStreamingSession::from_manifest(manifest, level_hints(&world)).unwrap();
    assert_eq!(session.mode, ShStreamingMode::SyncProof);

    session
        .update_targets(&VisibleCells::Culled(vec![0]), None, 0.0)
        .unwrap();
    assert_eq!(
        session.read_one_sync().unwrap(),
        SyncReadResult::Prepared(0)
    );

    let stats = session
        .read_stats()
        .unwrap()
        .expect("sync-proof reports reads");
    assert!(encoded_bytes > 0);
    assert_eq!(stats.reads_issued, 1);
    assert_eq!(stats.coalesced_reads, 0);
    assert_eq!(stats.read_bytes, encoded_bytes);
    assert_eq!(stats.gap_bytes, 0);
}

#[test]
fn capture_mode_gate_requires_sync_proof() {
    assert!(require_sync_proof_mode(ShStreamingMode::SyncProof).is_ok());
    let async_error = require_sync_proof_mode(ShStreamingMode::Async).unwrap_err();
    assert!(async_error.to_string().contains("static capture requires"));
    let late_off_error = require_sync_proof_mode(ShStreamingMode::Off).unwrap_err();
    assert!(late_off_error.to_string().contains("changed to off"));
    assert!(require_loaded_streaming_mode(ShStreamingMode::Async).is_ok());
}

#[test]
fn session_submission_latch_defers_promotion_until_the_next_frame_boundary() {
    let mut prior_compose_submitted = false;

    // Frame N accepted an install, but a later acquire/scene failure did
    // not submit compose work. Frame N+1 must retain it uncomposed.
    assert!(!consume_prior_compose_submission(
        &mut prior_compose_submitted
    ));

    // A successful Frame N+1 compose submission can only be consumed at
    // the following pre-compose seam, and exactly once.
    prior_compose_submitted = true;
    assert!(consume_prior_compose_submission(
        &mut prior_compose_submitted
    ));
    assert!(
        !consume_prior_compose_submission(&mut prior_compose_submitted),
        "one completed frame cannot publish more than once"
    );
}
