//! Threaded tests for the SH issuer and decode pool, over injectable sources
//! that record read spans and thread names.
//! See: context/lib/testing_guide.md.

use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_level_loader::PrlLoadError;

use super::schedule::COALESCE_MAX_GAP_BYTES;
use super::*;

const MIB: u64 = 1024 * 1024;

fn pattern(range: &Range<u64>) -> Vec<u8> {
    range.clone().map(|offset| (offset % 251) as u8).collect()
}

/// Serves synthetic bytes for per-cluster ranges. With `hold_first_read`, the
/// first read blocks until the test opens the gate, so the test can queue
/// work behind it and observe the issuer's next choices deterministically.
struct RecordingSource {
    ranges: Vec<Range<u64>>,
    reads: Mutex<Vec<Range<u64>>>,
    read_threads: Mutex<Vec<String>>,
    decode_threads: Mutex<Vec<String>>,
    hold_first_read: bool,
    /// Every physical read fails after it is recorded.
    fail_reads: bool,
    gate: (Mutex<bool>, Condvar),
}

impl RecordingSource {
    fn new(ranges: Vec<Range<u64>>, hold_first_read: bool) -> Arc<Self> {
        Self::with_reads_failing(ranges, hold_first_read, false)
    }

    fn with_reads_failing(
        ranges: Vec<Range<u64>>,
        hold_first_read: bool,
        fail_reads: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            ranges,
            reads: Mutex::new(Vec::new()),
            read_threads: Mutex::new(Vec::new()),
            decode_threads: Mutex::new(Vec::new()),
            hold_first_read,
            fail_reads,
            gate: (Mutex::new(false), Condvar::new()),
        })
    }

    fn open_gate(&self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }

    fn wait_for_reads(&self, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.reads.lock().unwrap().len() < count {
            assert!(
                Instant::now() < deadline,
                "issuer never started read {count}"
            );
            std::thread::yield_now();
        }
    }

    fn reads(&self) -> Vec<Range<u64>> {
        self.reads.lock().unwrap().clone()
    }
}

fn thread_name() -> String {
    std::thread::current()
        .name()
        .unwrap_or("<unnamed>")
        .to_owned()
}

impl ShWorkerSource for RecordingSource {
    fn cluster_count(&self) -> u32 {
        self.ranges.len() as u32
    }

    fn chunk_file_range(&self, cluster_id: u32) -> Result<Range<u64>, PrlLoadError> {
        Ok(self.ranges[cluster_id as usize].clone())
    }

    fn decoded_bytes(&self, cluster_id: u32) -> u64 {
        let range = &self.ranges[cluster_id as usize];
        range.end - range.start
    }

    fn read_file_span(&self, range: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        let first = {
            let mut reads = self.reads.lock().unwrap();
            reads.push(range.clone());
            reads.len() == 1
        };
        self.read_threads.lock().unwrap().push(thread_name());
        if first && self.hold_first_read {
            let mut open = self.gate.0.lock().unwrap();
            while !*open {
                open = self.gate.1.wait(open).unwrap();
            }
        }
        if self.fail_reads {
            return Err(PrlLoadError::SectionValidation {
                section: "test source",
                message: "read failed".into(),
            });
        }
        Ok(pattern(&range))
    }

    fn decode(
        &self,
        cluster_id: u32,
        bytes: Vec<u8>,
    ) -> Result<DecodedClusterShPayload, PrlLoadError> {
        self.decode_threads.lock().unwrap().push(thread_name());
        Ok(DecodedClusterShPayload {
            cluster_id,
            bytes,
            blocks: Vec::new(),
        })
    }
}

fn request(cluster_id: u32, mandatory: bool) -> ShClusterRequest {
    ShClusterRequest {
        generation: 3,
        content_tag: [3; 32],
        cluster_id,
        chunk_hash: [cluster_id as u8; 32],
        mandatory,
    }
}

fn publish(workers: &ShAsyncWorkers, targets: impl IntoIterator<Item = u32>) {
    workers.publish_targets(&targets.into_iter().collect::<BTreeSet<_>>());
}

fn wait_completions(workers: &ShAsyncWorkers, count: usize) -> Vec<ShWorkerCompletion> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut completions = Vec::new();
    while completions.len() < count {
        assert!(
            Instant::now() < deadline,
            "only {} completions",
            completions.len()
        );
        match workers.try_completion().unwrap() {
            Some(completion) => completions.push(completion),
            None => std::thread::yield_now(),
        }
    }
    completions
}

fn prepared_bytes(completions: &[ShWorkerCompletion], cluster_id: u32) -> &[u8] {
    let completion = completions
        .iter()
        .find(|completion| completion.request.cluster_id == cluster_id)
        .expect("completion for cluster");
    match &completion.result {
        ShWorkerResult::Prepared(chunk) => &chunk.bytes,
        other => panic!("cluster {cluster_id} completed {other:?}"),
    }
}

fn assert_phases_drained(workers: &ShAsyncWorkers) {
    let phases = *workers.shared.phases.lock().unwrap();
    assert_eq!(phases.encoded.current_bytes, 0);
    assert_eq!(phases.decoding.current_bytes, 0);
    assert_eq!(phases.ready.current_bytes, 0);
}

#[test]
fn decode_pool_size_is_half_the_cores_between_one_and_three() {
    for (available, expected) in [(0, 1), (1, 1), (2, 1), (4, 2), (6, 3), (64, 3)] {
        assert_eq!(decode_pool_size(available), expected, "{available} cores");
    }
}

#[test]
fn shuffled_submissions_read_mandatory_first_then_optional_each_ascending() {
    let far = 4 * COALESCE_MAX_GAP_BYTES;
    let ranges: Vec<_> = (0..8).map(|i| i * far..i * far + 64).collect();
    let source = RecordingSource::new(ranges.clone(), true);
    let mut workers = ShAsyncWorkers::with_source(source.clone(), 2).unwrap();
    publish(&workers, 0..8);

    workers.submit(request(7, true)).unwrap();
    source.wait_for_reads(1);
    for (cluster_id, mandatory) in [
        (2, false),
        (5, true),
        (0, false),
        (1, true),
        (6, false),
        (3, true),
        (4, false),
    ] {
        workers.submit(request(cluster_id, mandatory)).unwrap();
    }
    source.open_gate();
    let completions = wait_completions(&workers, 8);

    let order: Vec<_> = [7, 1, 3, 5, 0, 2, 4, 6]
        .iter()
        .map(|&cluster| ranges[cluster].clone())
        .collect();
    assert_eq!(source.reads(), order);
    for cluster_id in 0..8u32 {
        assert_eq!(
            prepared_bytes(&completions, cluster_id),
            pattern(&ranges[cluster_id as usize])
        );
    }
    assert_phases_drained(&workers);
    workers.stop();
}

#[test]
fn chunks_within_the_gap_cap_merge_into_one_read_and_split_into_exact_bytes() {
    let first = 0..1000;
    let second = 1000 + COALESCE_MAX_GAP_BYTES..1500 + COALESCE_MAX_GAP_BYTES;
    let third = second.end + 1..second.end + 301;
    let blocker = 100 * MIB..100 * MIB + 10;
    let ranges = vec![
        first.clone(),
        second.clone(),
        third.clone(),
        blocker.clone(),
    ];
    let source = RecordingSource::new(ranges.clone(), true);
    let mut workers = ShAsyncWorkers::with_source(source.clone(), 2).unwrap();
    publish(&workers, 0..4);

    workers.submit(request(3, true)).unwrap();
    source.wait_for_reads(1);
    for cluster_id in [2, 0, 1] {
        workers.submit(request(cluster_id, true)).unwrap();
    }
    source.open_gate();
    let completions = wait_completions(&workers, 4);

    assert_eq!(
        source.reads(),
        vec![blocker.clone(), first.start..third.end]
    );
    for (cluster_id, range) in ranges.iter().enumerate() {
        assert_eq!(
            prepared_bytes(&completions, cluster_id as u32),
            pattern(range)
        );
    }
    let stats = workers.stats().unwrap();
    assert_eq!(stats.reads_issued, 2);
    assert_eq!(stats.coalesced_reads, 1);
    assert_eq!(stats.gap_bytes, COALESCE_MAX_GAP_BYTES + 1);
    assert_eq!(stats.read_bytes, 10 + (third.end - first.start));
    assert!(stats.read_latency.max_ms() > 0.0);
    assert_phases_drained(&workers);
    workers.stop();
}

#[test]
fn span_cap_is_respected_and_an_oversized_chunk_is_read_alone() {
    let whole_cap = 0..16 * MIB;
    let low = 0..8 * MIB;
    let high = 8 * MIB..16 * MIB;
    let past_cap = 16 * MIB..16 * MIB + 1024;
    let oversized = 32 * MIB..48 * MIB + 1;
    let blocker = 100 * MIB..100 * MIB + 10;
    let ranges = vec![
        low,
        high,
        past_cap.clone(),
        oversized.clone(),
        blocker.clone(),
    ];
    let source = RecordingSource::new(ranges, true);
    let mut workers = ShAsyncWorkers::with_source(source.clone(), 2).unwrap();
    publish(&workers, 0..5);

    workers.submit(request(4, true)).unwrap();
    source.wait_for_reads(1);
    for cluster_id in [3, 2, 1, 0] {
        workers.submit(request(cluster_id, true)).unwrap();
    }
    source.open_gate();
    let completions = wait_completions(&workers, 5);

    assert_eq!(
        source.reads(),
        vec![blocker, whole_cap, past_cap, oversized]
    );
    assert!(
        completions
            .iter()
            .all(|completion| matches!(completion.result, ShWorkerResult::Prepared(_)))
    );
    assert_eq!(workers.stats().unwrap().coalesced_reads, 1);
    workers.stop();
}

#[test]
fn a_request_cleared_from_the_target_bitset_is_never_read_and_completes_cancelled() {
    let ranges = vec![0..64, 10 * MIB..10 * MIB + 64, 20 * MIB..20 * MIB + 64];
    let source = RecordingSource::new(ranges.clone(), true);
    let mut workers = ShAsyncWorkers::with_source(source.clone(), 1).unwrap();
    publish(&workers, [0, 1]);

    workers.submit(request(0, true)).unwrap();
    source.wait_for_reads(1);
    // Cluster 1 queues behind the held read, then departs before it is read.
    workers.submit(request(1, true)).unwrap();
    // Cluster 2 was never targeted.
    workers.submit(request(2, false)).unwrap();
    publish(&workers, [0]);
    source.open_gate();
    let completions = wait_completions(&workers, 3);

    assert_eq!(source.reads(), vec![ranges[0].clone()]);
    for cluster_id in [1, 2] {
        let completion = completions
            .iter()
            .find(|completion| completion.request.cluster_id == cluster_id)
            .unwrap();
        assert!(
            matches!(completion.result, ShWorkerResult::Cancelled),
            "cluster {cluster_id}: {:?}",
            completion.result
        );
    }
    assert_eq!(prepared_bytes(&completions, 0), pattern(&ranges[0]));
    workers.stop();
}

#[test]
fn a_failed_coalesced_read_fails_every_member_and_releases_its_encoded_bytes() {
    let first = 0..64;
    let second = 64..128;
    let blocker = 100 * MIB..100 * MIB + 10;
    let ranges = vec![first.clone(), second.clone(), blocker.clone()];
    let source = RecordingSource::with_reads_failing(ranges, true, true);
    let mut workers = ShAsyncWorkers::with_source(source.clone(), 2).unwrap();
    publish(&workers, 0..3);

    workers.submit(request(2, true)).unwrap();
    source.wait_for_reads(1);
    for cluster_id in [1, 0] {
        workers.submit(request(cluster_id, true)).unwrap();
    }
    source.open_gate();
    let completions = wait_completions(&workers, 3);

    // The two adjacent chunks still shared one physical read.
    assert_eq!(source.reads(), vec![blocker, first.start..second.end]);
    for completion in &completions {
        assert!(
            matches!(completion.result, ShWorkerResult::Failed(_)),
            "cluster {}: {:?}",
            completion.request.cluster_id,
            completion.result
        );
    }
    assert_eq!(workers.stats().unwrap().reads_issued, 0);
    assert!(source.decode_threads.lock().unwrap().is_empty());
    assert_phases_drained(&workers);
    workers.stop();
}

#[test]
fn a_canonical_empty_cluster_is_decoded_without_a_read() {
    let empty: Range<u64> = 64..64;
    let source = RecordingSource::new(std::iter::once(empty).collect(), false);
    let mut workers = ShAsyncWorkers::with_source(source.clone(), 1).unwrap();
    publish(&workers, [0]);
    workers.submit(request(0, false)).unwrap();
    let completions = wait_completions(&workers, 1);
    assert!(prepared_bytes(&completions, 0).is_empty());
    assert!(source.reads().is_empty());
    assert_eq!(workers.stats().unwrap().reads_issued, 0);
    workers.stop();
}

#[test]
fn decode_runs_on_a_pool_thread_never_the_issuer() {
    let ranges: Vec<_> = (0..4).map(|i| i * 10 * MIB..i * 10 * MIB + 64).collect();
    let source = RecordingSource::new(ranges, false);
    let mut workers = ShAsyncWorkers::with_source(source.clone(), 2).unwrap();
    publish(&workers, 0..4);
    for cluster_id in 0..4 {
        workers
            .submit(request(cluster_id, cluster_id % 2 == 0))
            .unwrap();
    }
    wait_completions(&workers, 4);
    workers.stop();

    let read_threads = source.read_threads.lock().unwrap().clone();
    let decode_threads = source.decode_threads.lock().unwrap().clone();
    assert_eq!(read_threads.len(), 4);
    assert!(read_threads.iter().all(|name| name == "sh-probe-read"));
    assert_eq!(decode_threads.len(), 4);
    assert!(
        decode_threads
            .iter()
            .all(|name| name.starts_with("sh-probe-decode-")),
        "{decode_threads:?}"
    );
    let test_thread = thread_name();
    assert!(!decode_threads.contains(&test_thread));
}

#[cfg(unix)]
mod positional {
    use super::*;
    use std::os::unix::fs::FileExt;

    /// Reads "payload" out of "headpayloadtail" after a delay, through a
    /// real positional read.
    struct DelayedPositionalSource {
        file: std::fs::File,
        delay: Duration,
        executions: Arc<AtomicUsize>,
    }

    impl ShWorkerSource for DelayedPositionalSource {
        fn cluster_count(&self) -> u32 {
            MAX_STREAM_PERMITS as u32
        }

        fn chunk_file_range(&self, _cluster_id: u32) -> Result<Range<u64>, PrlLoadError> {
            Ok(4..11)
        }

        fn decoded_bytes(&self, _cluster_id: u32) -> u64 {
            7
        }

        fn read_file_span(&self, range: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
            self.executions.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            let mut bytes = vec![0; (range.end - range.start) as usize];
            self.file.read_at(&mut bytes, range.start).unwrap();
            Ok(bytes)
        }

        fn decode(
            &self,
            cluster_id: u32,
            bytes: Vec<u8>,
        ) -> Result<DecodedClusterShPayload, PrlLoadError> {
            Ok(DecodedClusterShPayload {
                cluster_id,
                bytes,
                blocks: Vec::new(),
            })
        }
    }

    fn delayed_source(
        delay: Duration,
        executions: Arc<AtomicUsize>,
    ) -> (tempfile::TempDir, Arc<dyn ShWorkerSource>) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("encoded.bin");
        std::fs::write(&path, b"headpayloadtail").unwrap();
        let source = DelayedPositionalSource {
            file: std::fs::File::open(path).unwrap(),
            delay,
            executions,
        };
        (temp, Arc::new(source))
    }

    fn all_targets(workers: &ShAsyncWorkers) {
        publish(workers, 0..MAX_STREAM_PERMITS as u32);
    }

    #[test]
    fn delayed_positional_reader_never_blocks_completion_drain_or_submission() {
        let executions = Arc::new(AtomicUsize::new(0));
        let (_temp, source) = delayed_source(Duration::from_millis(250), Arc::clone(&executions));
        let mut workers = ShAsyncWorkers::with_source(source, 2).unwrap();
        all_targets(&workers);
        let request = ShClusterRequest {
            generation: 17,
            content_tag: [4; 32],
            cluster_id: 0,
            chunk_hash: [7; 32],
            mandatory: true,
        };
        let start = Instant::now();
        workers.submit(request).unwrap();
        assert!(workers.try_completion().unwrap().is_none());
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "frame-side submission/drain waited for I/O"
        );
        std::thread::sleep(Duration::from_millis(300));
        let completion = wait_completions(&workers, 1).pop().unwrap();
        assert_eq!(completion.request, request);
        match completion.result {
            ShWorkerResult::Prepared(chunk) => assert_eq!(chunk.bytes, b"payload"),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        workers.stop();
    }

    #[test]
    fn delayed_reload_retires_without_blocking_and_cannot_deliver_stale_generation() {
        let executions = Arc::new(AtomicUsize::new(0));
        let (_old_temp, old_source) =
            delayed_source(Duration::from_millis(250), Arc::clone(&executions));
        let mut workers = ShAsyncWorkers::with_source(old_source, 2).unwrap();
        assert_eq!(workers.handles.len(), 3, "one issuer plus two decoders");
        all_targets(&workers);
        let old_request = |cluster_id| ShClusterRequest {
            generation: 1,
            content_tag: [1; 32],
            cluster_id,
            chunk_hash: [1; 32],
            mandatory: true,
        };
        for cluster_id in 0..MAX_STREAM_PERMITS as u32 {
            workers.submit(old_request(cluster_id)).unwrap();
        }
        while executions.load(Ordering::SeqCst) < 1 {
            std::thread::yield_now();
        }

        let start = Instant::now();
        let mut retirement = workers.begin_retirement();
        assert!(start.elapsed() < Duration::from_millis(100));
        assert!(workers.handles.is_empty());
        assert!(
            !retirement.try_finish(),
            "delayed positional read is still live"
        );

        std::thread::sleep(Duration::from_millis(300));
        assert!(retirement.try_finish());
        assert!(retirement.handles.is_empty());
        assert_eq!(
            executions.load(Ordering::SeqCst),
            1,
            "cancellation stops the issuer before its next read"
        );
        assert!(
            workers.try_completion().is_err(),
            "cancelled generation must not publish a ready completion"
        );

        // Only after every old handle has joined may the next generation
        // start, so lifecycle permits never overlap generations.
        let (_new_temp, new_source) = delayed_source(Duration::ZERO, Arc::new(AtomicUsize::new(0)));
        let mut replacement = ShAsyncWorkers::with_source(new_source, 2).unwrap();
        all_targets(&replacement);
        let new_request = ShClusterRequest {
            generation: 2,
            content_tag: [2; 32],
            cluster_id: 0,
            chunk_hash: [2; 32],
            mandatory: true,
        };
        replacement.submit(new_request).unwrap();
        let completion = wait_completions(&replacement, 1).pop().unwrap();
        assert_eq!(completion.request, new_request);
        assert_ne!(completion.request.generation, old_request(0).generation);
        replacement.stop();
    }
}
