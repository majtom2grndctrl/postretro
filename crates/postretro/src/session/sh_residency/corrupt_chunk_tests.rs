//! Coded id-50 chunks through a real session: a coded chunk installs as its
//! raw form, and a corrupt frame takes the hash-failure path.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency"

use super::*;
use crate::sh_streaming::controller::ClusterResidencyState;
use crate::sh_streaming::sync_manifest_test_fixture::StoredChunk;
use postretro_test_log_capture::LogCapture;
use std::time::{Duration, Instant};

fn session(stored: StoredChunk) -> (tempfile::TempDir, ShStreamingSession) {
    let (temp, path) = sync_manifest_test_fixture::write_one_cluster_prl_stored(stored);
    let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
    let manifest = Arc::clone(world.sh_stream_manifest().expect("id 50 selects streaming"));
    let hints = Arc::new(ClusterHints::decode(manifest.cluster_directory()).unwrap());
    let session = ShStreamingSession::from_manifest(manifest, hints).unwrap();
    (temp, session)
}

/// The one ready chunk a sync-proof read prepares.
fn sync_ready_chunk(stored: StoredChunk) -> Vec<u8> {
    let (_temp, mut session) = session(stored);
    session
        .update_targets(&VisibleCells::Culled(vec![0]), None, 0.0)
        .unwrap();
    assert!(matches!(
        session.read_one_sync().unwrap(),
        SyncReadResult::Prepared(0)
    ));
    let mut batch = session.controller.take_drain_batch().unwrap();
    batch.ready.pop().expect("one ready chunk").chunk.bytes
}

// A coded chunk is read at its stored size and installs as its raw form.
#[test]
fn a_coded_chunk_installs_the_raw_chunk_through_sync_proof() {
    let raw = sync_ready_chunk(StoredChunk::Raw);
    assert_eq!(sync_ready_chunk(StoredChunk::Coded), raw);
}

// Sync-proof: the read returns the named decode error and fails the cluster.
#[test]
fn a_sync_proof_read_of_a_corrupt_frame_returns_the_named_error() {
    let (_temp, mut session) = session(StoredChunk::CorruptFrame);
    session
        .update_targets(&VisibleCells::Culled(vec![0]), None, 0.0)
        .unwrap();
    let message = format!("{:#}", session.read_one_sync().unwrap_err());
    assert!(
        message.contains("ClusterShPayloadsChunkDecodeFailed: cluster 0"),
        "{message}"
    );
    assert_eq!(
        session.controller.state(0),
        Some(ClusterResidencyState::Failed)
    );
    assert_eq!(session.controller.permits_in_use(), 0);
}

// Async: the decode fails on the pool, the cluster is marked failed with one
// warning naming the decode, it is retried once after leaving and
// re-entering its targets, and never again.
#[test]
fn an_async_corrupt_frame_fails_with_one_warning_and_one_retry() {
    let (_temp, mut session) = session(StoredChunk::CorruptFrame);
    session.mode = ShStreamingMode::Async;
    session.start_async_workers().unwrap();
    let capture = LogCapture::start();
    let visible = VisibleCells::Culled(vec![0]);
    let empty = VisibleCells::Culled(Vec::new());

    let wait_for_failure = |session: &mut ShStreamingSession, time| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let batch = session.prepare_async_batch(&visible, None, time).unwrap();
            assert!(
                batch.ready.is_empty(),
                "a corrupt frame yields no ready bytes"
            );
            if session.controller.state(0) == Some(ClusterResidencyState::Failed)
                && session.controller.permits_in_use() == 0
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the decode failure did not arrive"
            );
            std::thread::yield_now();
        }
    };
    wait_for_failure(&mut session, 0.0);
    capture.assert_logged_once(log::Level::Warn, "cluster 0 read/decode failed:");
    capture.assert_logged(
        log::Level::Warn,
        "ClusterShPayloadsChunkDecodeFailed: cluster 0",
    );
    assert_eq!(session.controller.counters().retries, 0);

    // Leaving and re-entering the targets earns the one retry.
    session.prepare_async_batch(&empty, None, 0.1).unwrap();
    session.prepare_async_batch(&empty, None, 2.1).unwrap();
    session.prepare_async_batch(&visible, None, 2.2).unwrap();
    assert_eq!(session.controller.counters().retries, 1);
    wait_for_failure(&mut session, 2.2);
    capture.assert_logged_once(log::Level::Warn, "cluster 0 read/decode failed:");

    // The retry is spent: re-entering again reads nothing.
    session.prepare_async_batch(&empty, None, 2.3).unwrap();
    session.prepare_async_batch(&empty, None, 4.4).unwrap();
    session.prepare_async_batch(&visible, None, 4.5).unwrap();
    assert_eq!(session.controller.counters().retries, 1);
    assert_eq!(
        session.controller.state(0),
        Some(ClusterResidencyState::Failed)
    );
    assert_eq!(session.controller.permits_in_use(), 0);
    capture.assert_logged_once(log::Level::Warn, "cluster 0 read/decode failed:");
}
