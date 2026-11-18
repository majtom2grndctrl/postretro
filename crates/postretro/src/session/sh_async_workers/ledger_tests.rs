//! SH's phase ledger and rank over the shared decode pool: a read chunk is
//! charged at its decoded size from the moment it is queued, never its
//! stored size, and is taken in its cluster's live class order.
//! See: context/lib/rendering_pipeline.md §4 (Lossless decode)

use std::ops::Range;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_level_loader::PrlLoadError;

use super::*;
use crate::sh_streaming::controller::ShClusterRequest;
use crate::streaming::drain_budget::DrainClass;

const STORED: u64 = 64;
/// A coded chunk decodes to several times its stored size.
const DECODED: u64 = 4 * STORED;
const WAIT: Duration = Duration::from_secs(5);

/// Far-apart clusters of `STORED` stored bytes, each decoding to `DECODED`
/// bytes. A decode blocks until the test opens the gate, so the test can
/// watch the ledger while the chunk sits on the pool. Decodes record their
/// clusters in the order they begin.
struct GatedDecodeSource {
    gate: (Mutex<bool>, Condvar),
    decoding: (Mutex<bool>, Condvar),
    fail: bool,
    clusters: u32,
    order: Mutex<Vec<u32>>,
}

impl GatedDecodeSource {
    fn new(fail: bool) -> Arc<Self> {
        Self::with_clusters(fail, 2)
    }

    fn with_clusters(fail: bool, clusters: u32) -> Arc<Self> {
        Arc::new(Self {
            gate: (Mutex::new(false), Condvar::new()),
            decoding: (Mutex::new(false), Condvar::new()),
            fail,
            clusters,
            order: Mutex::default(),
        })
    }

    fn open(&self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }

    fn wait_decoding(&self) {
        let (started, _) = self
            .decoding
            .1
            .wait_timeout_while(self.decoding.0.lock().unwrap(), WAIT, |started| !*started)
            .unwrap();
        assert!(*started, "the decode never started");
    }
}

impl ShWorkerSource for GatedDecodeSource {
    fn cluster_count(&self) -> u32 {
        self.clusters
    }

    fn chunk_file_range(&self, cluster_id: u32) -> Result<Range<u64>, PrlLoadError> {
        let start = u64::from(cluster_id) * 1_000_000;
        Ok(start..start + STORED)
    }

    fn decoded_bytes(&self, _cluster_id: u32) -> u64 {
        DECODED
    }

    fn read_file_span(&self, range: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        Ok(vec![7; (range.end - range.start) as usize])
    }

    fn decode(
        &self,
        cluster_id: u32,
        _bytes: Vec<u8>,
    ) -> Result<DecodedClusterShPayload, PrlLoadError> {
        self.order.lock().unwrap().push(cluster_id);
        *self.decoding.0.lock().unwrap() = true;
        self.decoding.1.notify_all();
        let (open, _) = self
            .gate
            .1
            .wait_timeout_while(self.gate.0.lock().unwrap(), WAIT, |open| !*open)
            .unwrap();
        assert!(*open, "the test never opened the decode gate");
        if self.fail {
            return Err(PrlLoadError::SectionValidation {
                section: "test source",
                message: "frame failed to decode".into(),
            });
        }
        Ok(DecodedClusterShPayload {
            cluster_id,
            bytes: vec![7; DECODED as usize],
            blocks: Vec::new(),
        })
    }
}

fn request() -> ShClusterRequest {
    request_for(0)
}

fn request_for(cluster_id: u32) -> ShClusterRequest {
    request_at(cluster_id, DrainClass::Visible)
}

fn request_at(cluster_id: u32, class: DrainClass) -> ShClusterRequest {
    ShClusterRequest {
        generation: 1,
        content_tag: [1; 32],
        cluster_id,
        chunk_hash: [1; 32],
        class,
    }
}

/// Waits until `bytes` are charged to the decoding phase.
fn wait_decoding_bytes(workers: &ShAsyncWorkers, bytes: u64) {
    let deadline = Instant::now() + WAIT;
    while phases(workers).decoding.current_bytes != bytes {
        assert!(Instant::now() < deadline, "the chunks never queued");
        std::thread::yield_now();
    }
}

fn phases(workers: &ShAsyncWorkers) -> crate::sh_streaming::budget::CpuPhaseLedger {
    *workers.shared.phases()
}

fn completion(workers: &ShAsyncWorkers) -> ShWorkerCompletion {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(completion) = workers.try_completion().unwrap() {
            return completion;
        }
        assert!(Instant::now() < deadline, "no completion arrived");
        std::thread::yield_now();
    }
}

// While a chunk is queued and decoding it is charged at its decoded size,
// its stored bytes released; the frame's take of the completion returns the
// ledger to zero, a delivered chunk through the ready phase at that size.
#[test]
fn a_queued_chunk_is_charged_at_its_decoded_size_until_the_frame_takes_it() {
    for fail in [false, true] {
        let source = GatedDecodeSource::new(fail);
        let mut workers = ShAsyncWorkers::with_source(source.clone(), 1).unwrap();
        workers.publish_target_keys(&[0].into_iter().collect());
        workers.submit(request()).unwrap();
        source.wait_decoding();
        let held = phases(&workers);
        assert_eq!(held.encoded.current_bytes, 0, "stored bytes released");
        assert_eq!(held.encoded.high_water_bytes, STORED, "the read held them");
        assert_eq!(held.decoding.current_bytes, DECODED);
        assert_eq!(held.ready.current_bytes, 0);

        source.open();
        let completion = completion(&workers);
        match (&completion.result, fail) {
            (ShWorkerResult::Prepared(chunk), false) => {
                assert_eq!(chunk.bytes.len() as u64, DECODED);
                assert_eq!(completion.ready_bytes, DECODED);
                assert_eq!(phases(&workers).ready.high_water_bytes, DECODED);
            }
            (ShWorkerResult::Failed(_), true) => assert_eq!(completion.ready_bytes, 0),
            (other, _) => panic!("fail={fail}: unexpected {other:?}"),
        }
        let drained = phases(&workers);
        assert_eq!(drained.encoded.current_bytes, 0);
        assert_eq!(drained.decoding.current_bytes, 0);
        assert_eq!(drained.ready.current_bytes, 0);
        workers.stop();
    }
}

// A job the pool discards at retirement releases its decoded-size charge,
// and the job decoding at the time releases its own once it returns.
#[test]
fn a_chunk_discarded_on_the_pool_releases_its_decoded_charge() {
    let source = GatedDecodeSource::new(false);
    let mut workers = ShAsyncWorkers::with_source(source.clone(), 1).unwrap();
    workers.publish_target_keys(&[0, 1].into_iter().collect());
    // The pool's one thread takes cluster 0; cluster 1 queues behind it.
    workers.submit(request_for(0)).unwrap();
    source.wait_decoding();
    workers.submit(request_for(1)).unwrap();
    let deadline = Instant::now() + WAIT;
    while phases(&workers).decoding.current_bytes != 2 * DECODED {
        assert!(Instant::now() < deadline, "cluster 1 never queued");
        std::thread::yield_now();
    }
    let mut retirement = workers.begin_retirement();
    // The route charges cluster 1 just before it submits the job, so the job
    // may meet the closed pool in `submit` rather than in the queue; either
    // discard releases its charge while cluster 0 is still decoding.
    let deadline = Instant::now() + WAIT;
    while phases(&workers).decoding.current_bytes != DECODED {
        assert!(
            Instant::now() < deadline,
            "the queued job's charge is released as the pool retires"
        );
        std::thread::yield_now();
    }
    source.open();
    let deadline = Instant::now() + WAIT;
    while !retirement.try_finish() {
        assert!(Instant::now() < deadline, "the pool never retired");
        std::thread::yield_now();
    }
    let drained = phases(&workers);
    assert_eq!(drained.encoded.current_bytes, 0);
    assert_eq!(drained.decoding.current_bytes, 0);
    assert_eq!(drained.ready.current_bytes, 0);
}

// For SH: a chunk whose cluster leaves the targets while its decode is
// queued is not decoded; it completes cancelled and releases its charge. One
// that returns before its decode runs still decodes.
#[test]
fn a_departed_queued_chunk_completes_cancelled_without_its_decode() {
    for returns in [false, true] {
        let source = GatedDecodeSource::new(false);
        let mut workers = ShAsyncWorkers::with_source(source.clone(), 1).unwrap();
        workers.publish_target_keys(&[0, 1].into_iter().collect());
        workers.submit(request_for(0)).unwrap();
        source.wait_decoding();
        workers.submit(request_for(1)).unwrap();
        let deadline = Instant::now() + WAIT;
        while phases(&workers).decoding.current_bytes != 2 * DECODED {
            assert!(Instant::now() < deadline, "cluster 1 never queued");
            std::thread::yield_now();
        }
        workers.publish_target_keys(&[0].into_iter().collect());
        if returns {
            workers.publish_target_keys(&[0, 1].into_iter().collect());
        }
        source.open();
        let completions = [completion(&workers), completion(&workers)];
        let second = completions
            .iter()
            .find(|completion| completion.request.cluster_id == 1)
            .expect("cluster 1 completes");
        match (&second.result, returns) {
            (ShWorkerResult::Cancelled, false) => assert_eq!(second.ready_bytes, 0),
            (ShWorkerResult::Prepared(_), true) => assert_eq!(second.ready_bytes, DECODED),
            (other, _) => panic!("returns={returns}: unexpected {other:?}"),
        }
        let drained = phases(&workers);
        assert_eq!(drained.decoding.current_bytes, 0);
        assert_eq!(drained.ready.current_bytes, 0);
        workers.stop();
    }
}

// An SH chunk requested optional whose cluster turns mandatory while its
// decode is queued is taken before older optional chunks, in its live class
// order: the session's target publish carries each cluster's class, and the
// job reads it at every pickup.
#[test]
fn an_sh_chunk_raised_while_queued_is_taken_in_its_live_class_order() {
    use DrainClass::{Lead, Prefetch, Visible};
    let source = GatedDecodeSource::with_clusters(false, 5);
    let mut workers = ShAsyncWorkers::with_source(source.clone(), 1).unwrap();
    workers.publish_targets((0..5).map(|cluster| (cluster, Prefetch)));
    workers.submit(request_at(0, Prefetch)).unwrap();
    source.wait_decoding();
    for cluster in 1..5 {
        workers.submit(request_at(cluster, Prefetch)).unwrap();
    }
    wait_decoding_bytes(&workers, 5 * DECODED);
    workers.publish_targets([
        (0, Prefetch),
        (1, Prefetch),
        (2, Lead),
        (3, Prefetch),
        (4, Visible),
    ]);
    source.open();
    for _ in 0..5 {
        completion(&workers);
    }
    assert_eq!(
        *source.order.lock().unwrap(),
        [0, 4, 2, 1, 3],
        "the running decode, then Visible, Lead, and the optional chunks oldest first"
    );
    workers.stop();
}

// The phase ledger is never left poisoned: a panic while it is held (here
// forced by the test) leaves later charges, deliveries and the frame's take
// working, with the ledger exact.
#[test]
fn a_poisoned_phase_ledger_is_recovered_and_stays_exact() {
    let source = GatedDecodeSource::new(false);
    let mut workers = ShAsyncWorkers::with_source(source.clone(), 1).unwrap();
    let shared = Arc::clone(&workers.shared);
    let poisoner = std::thread::spawn(move || {
        let _guard = shared.phases.lock().unwrap();
        panic!("a panic while the phase ledger is held");
    });
    assert!(poisoner.join().is_err());
    assert!(workers.shared.phases.is_poisoned());

    workers.publish_target_keys(&[0].into_iter().collect());
    source.open();
    workers.submit(request()).unwrap();
    let delivered = completion(&workers);
    assert!(matches!(delivered.result, ShWorkerResult::Prepared(_)));
    let drained = *workers.shared.phases();
    assert_eq!(drained.encoded.current_bytes, 0);
    assert_eq!(drained.decoding.current_bytes, 0);
    assert_eq!(drained.ready.current_bytes, 0);
    workers.stop();
}
