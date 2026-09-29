//! Session-level SH streaming tests: worker failures, periodic log, sync-proof
//! counters, budget inputs, and mode gates. See: context/lib/testing_guide.md

use super::*;
use postretro_test_log_capture::LogCapture;
use std::time::{Duration, Instant};

// Regression: the permitted retry for one failed worker request emitted a duplicate warning.
#[test]
fn async_worker_failure_warns_once_across_same_identity_retry() {
    let (_temp, path) = sync_manifest_test_fixture::write_one_cluster_prl();
    let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
    let manifest = Arc::clone(world.sh_stream_manifest().expect("id 50 selects streaming"));
    let mut session = ShStreamingSession::from_snapshot(
        manifest,
        ShResidencySnapshot {
            effective_floor_bytes: 1024 * 1024,
            ..ShResidencySnapshot::default()
        },
        world.cell_visibility.as_ref(),
    )
    .unwrap();
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
    session.prepare_async_batch(&visible, Some(0), 0.0).unwrap();

    let wait_for_failure = |session: &mut ShStreamingSession, time| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.controller.state(0)
            != Some(crate::sh_streaming::controller::ClusterResidencyState::Failed)
        {
            assert!(Instant::now() < deadline, "worker failure did not arrive");
            std::thread::yield_now();
            session
                .prepare_async_batch(&visible, Some(0), time)
                .unwrap();
        }
    };
    wait_for_failure(&mut session, 0.0);
    capture.assert_logged_once(log::Level::Warn, "cluster 0 read/decode failed:");

    // No camera cell stands for the camera leaving; its warm set departs.
    session.prepare_async_batch(&empty, None, 0.1).unwrap();
    session.prepare_async_batch(&empty, None, 2.1).unwrap();
    session.prepare_async_batch(&visible, Some(0), 2.2).unwrap();
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
    let renderer = ShResidencySnapshot {
        effective_floor_bytes: 1024 * 1024,
        ..ShResidencySnapshot::default()
    };
    let mut session =
        ShStreamingSession::from_snapshot(manifest, renderer, world.cell_visibility.as_ref())
            .unwrap();
    session.mode = ShStreamingMode::Async;
    session.start_async_workers().unwrap();
    let capture = LogCapture::start();
    let visible = VisibleCells::Culled(vec![0]);
    // One frame as the windowed loop runs it, with the renderer accepting
    // every ready cluster.
    let frame = |session: &mut ShStreamingSession, seconds: f64| {
        let batch = session
            .prepare_async_batch(&visible, Some(0), seconds)
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
    let mut session = ShStreamingSession::from_snapshot(
        manifest,
        ShResidencySnapshot {
            effective_floor_bytes: 1024 * 1024,
            ..ShResidencySnapshot::default()
        },
        world.cell_visibility.as_ref(),
    )
    .unwrap();
    assert_eq!(session.mode, ShStreamingMode::SyncProof);

    session
        .update_targets(&VisibleCells::Culled(vec![0]), Some(0), 0.0)
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
fn snapshot_budget_uses_real_renderer_pool_figures() {
    let snapshot = ShResidencySnapshot {
        fixed_metadata_bytes: 11,
        whole_resident_scatter_bytes: 13,
        active_capacity_bytes: 17,
        effective_floor_bytes: 41,
        dense_group_minimum_bytes: Some(5),
        indirect_delta_minimum_bytes: Some(2),
        direct_delta_minimum_bytes: None,
        animated_direct_delta_minimum_bytes: Some(3),
        ..ShResidencySnapshot::default()
    };

    let inputs = budget_inputs(snapshot);
    assert_eq!(inputs.fixed.fixed_metadata_bytes, 11);
    assert_eq!(inputs.fixed.whole_resident_scatter_bytes, 13);
    assert_eq!(inputs.fixed.active_pool_capacity_bytes, 17);
    assert_eq!(inputs.pool_minima.dense_group_bytes, Some(5));
    assert_eq!(inputs.pool_minima.indirect_delta_bytes, Some(2));
    assert_eq!(inputs.pool_minima.direct_delta_bytes, None);
    assert_eq!(inputs.pool_minima.animated_direct_delta_bytes, Some(3));
    assert_eq!(inputs.renderer_effective_floor_bytes, Some(41));
    // Regression: a level whose entire SH allocation is below the 256 MiB
    // requested floor must still accept the renderer's exact physical floor.
    let accounting = crate::sh_streaming::budget::ShResidencyAccounting::new(inputs).unwrap();
    assert_eq!(accounting.effective_floor_bytes().unwrap(), 41);
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
