//! SH's phase ledger over the shared decode pool: a read chunk is charged at
//! its decoded size from the moment it is queued, never its stored size.
//! See: context/lib/rendering_pipeline.md §4 (Lossless decode)

use std::ops::Range;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_level_loader::PrlLoadError;

use super::*;
use crate::sh_streaming::controller::ShClusterRequest;

const STORED: u64 = 64;
/// A coded chunk decodes to several times its stored size.
const DECODED: u64 = 4 * STORED;
const WAIT: Duration = Duration::from_secs(5);

/// Two far-apart clusters of `STORED` stored bytes, each decoding to
/// `DECODED` bytes. A decode blocks until the test opens the gate, so the test can watch the
/// ledger while the chunk sits on the pool.
struct GatedDecodeSource {
    gate: (Mutex<bool>, Condvar),
    decoding: (Mutex<bool>, Condvar),
    fail: bool,
}

impl GatedDecodeSource {
    fn new(fail: bool) -> Arc<Self> {
        Arc::new(Self {
            gate: (Mutex::new(false), Condvar::new()),
            decoding: (Mutex::new(false), Condvar::new()),
            fail,
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
        2
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
    ShClusterRequest {
        generation: 1,
        content_tag: [1; 32],
        cluster_id,
        chunk_hash: [1; 32],
        mandatory: true,
    }
}

fn phases(workers: &ShAsyncWorkers) -> crate::sh_streaming::budget::CpuPhaseLedger {
    *workers.shared.phases.lock().unwrap()
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
        workers.publish_targets(&[0].into_iter().collect());
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
    workers.publish_targets(&[0, 1].into_iter().collect());
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
    assert_eq!(
        phases(&workers).decoding.current_bytes,
        DECODED,
        "the queued job's charge is released as the pool retires"
    );
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
