//! Refusal outcomes through session accounting, logging and real loader reads.

use super::*;
use postretro_test_log_capture::LogCapture;
use std::time::Duration;

fn session(manifest: Arc<ShStreamManifest>) -> ShStreamingSession {
    let hints = Arc::new(ClusterHints::decode(manifest.cluster_directory()).unwrap());
    ShStreamingSession::from_snapshot(
        manifest,
        ShResidencySnapshot {
            effective_floor_bytes: 1024 * 1024,
            ..ShResidencySnapshot::default()
        },
        hints,
    )
    .unwrap()
}

fn refuse_ready(session: &mut ShStreamingSession) {
    let batch = session.controller.take_drain_batch().unwrap();
    assert_eq!(batch.ready.len(), 1);
    session
        .apply_controller_outcome(ShDrainOutcome {
            refused: vec![batch.ready[0].chunk.cluster_id],
            ..Default::default()
        })
        .unwrap();
}

// Regression: a refusal must not spend the read-failure warning or retry.
#[test]
fn device_limit_refusal_then_corrupt_reread_warns_for_both_causes_once() {
    use std::io::{Read, Seek, SeekFrom, Write};

    let (_temp, path) = sync_manifest_test_fixture::write_one_cluster_prl();
    let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
    let mut session = session(Arc::clone(world.sh_stream_manifest().unwrap()));
    let capture = LogCapture::start();
    let visible = VisibleCells::Culled(vec![0]);
    let empty = VisibleCells::Culled(vec![]);
    session.update_targets(&visible, None, 0.0).unwrap();
    assert!(matches!(
        session.read_one_sync().unwrap(),
        SyncReadResult::Prepared(0)
    ));
    refuse_ready(&mut session);
    capture.assert_logged_once(log::Level::Warn, "device limit refused cluster 0");
    capture.assert_not_logged(log::Level::Warn, "read/decode failed");

    // Keep the PRL and chunk size intact, but change a byte in the chunk's
    // atlas body. The manifest's recorded hash now rejects the real reread.
    let index = &session.manifest.payloads().index[0];
    let range = session.manifest.chunk_file_range(0).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let offset = range.start + index.payload_len - 1;
    file.seek(SeekFrom::Start(offset)).unwrap();
    let mut byte = [0];
    file.read_exact(&mut byte).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(&[byte[0] ^ 1]).unwrap();

    session.mode = ShStreamingMode::Async;
    session.start_async_workers().unwrap();
    let await_failure = |session: &mut ShStreamingSession, time| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let batch = session.prepare_async_batch(&visible, None, time).unwrap();
            assert!(
                batch.ready.is_empty(),
                "hash mismatch cannot yield ready bytes"
            );
            if session.controller.state(0)
                == Some(crate::sh_streaming::controller::ClusterResidencyState::Failed)
                && session.controller.permits_in_use() == 0
            {
                break;
            }
            assert!(Instant::now() < deadline, "hash failure did not arrive");
            std::thread::yield_now();
        }
    };
    session.prepare_async_batch(&empty, None, 0.1).unwrap();
    session.prepare_async_batch(&empty, None, 2.2).unwrap();
    session.prepare_async_batch(&visible, None, 2.3).unwrap();
    await_failure(&mut session, 2.3);
    capture.assert_logged_once(log::Level::Warn, "cluster 0 read/decode failed:");
    capture.assert_logged(log::Level::Warn, "ClusterShPayloadsHashMismatch");
    session.prepare_async_batch(&empty, None, 2.4).unwrap();
    session.prepare_async_batch(&empty, None, 4.5).unwrap();
    session.prepare_async_batch(&visible, None, 4.6).unwrap();
    await_failure(&mut session, 4.6);
    capture.assert_logged_once(log::Level::Warn, "cluster 0 read/decode failed:");
    capture.assert_logged_once(log::Level::Warn, "device limit refused cluster 0");
}

#[test]
fn invalid_refusal_outcome_does_not_spend_level_warning() {
    let (_temp, path) = sync_manifest_test_fixture::write_one_cluster_prl();
    let world = postretro_level_loader::load_prl(path.to_str().unwrap()).unwrap();
    let mut session = session(Arc::clone(world.sh_stream_manifest().unwrap()));
    let capture = LogCapture::start();
    assert!(
        session
            .apply_controller_outcome(ShDrainOutcome {
                refused: vec![0],
                ..Default::default()
            })
            .is_err()
    );
    assert!(!session.device_limit_warned.load(Ordering::Relaxed));
    capture.assert_not_logged(log::Level::Warn, "device limit");
}

#[test]
fn device_limit_warning_names_only_first_cluster_per_level() {
    let capture = LogCapture::start();
    let level_warned = AtomicBool::new(false);
    warn_device_limit_once(&level_warned, None);
    warn_device_limit_once(&level_warned, Some(7));
    warn_device_limit_once(&level_warned, Some(2));
    warn_device_limit_once(&level_warned, Some(7));
    capture.assert_logged_once(log::Level::Warn, "device limit refused cluster");
    capture.assert_logged_once(log::Level::Warn, "device limit refused cluster 7");
    capture.assert_not_logged(log::Level::Warn, "device limit refused cluster 2");
    capture.assert_not_logged(log::Level::Warn, "read/decode failed");
    capture.clear();
    warn_device_limit_once(&AtomicBool::new(false), Some(2));
    capture.assert_logged_once(log::Level::Warn, "device limit refused cluster 2");
}
