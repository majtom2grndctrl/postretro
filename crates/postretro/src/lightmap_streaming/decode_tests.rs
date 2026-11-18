//! Coded pairs through the route and the decode pool: decoded-size charges,
//! pre-split admission, corrupt blocks, raised and departed pairs, and the
//! queue bounds.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use postretro_level_format::lightmap::LightmapBlockPayload;
use postretro_level_loader::{LightmapBlockFileRanges, PrlLoadError};

use super::*;
use crate::lightmap_streaming::route::{
    LightmapCompletion, LightmapReadResult, LightmapRouteLedger, lightmap_route,
};
use crate::lightmap_streaming::source::BlockSummary;
use crate::lightmap_streaming::test_fixtures::*;
use crate::streaming::decode_pool::{DecodeJob, DecodePool, DecodeRank};
use crate::streaming::drain_budget::DrainClass;
use crate::streaming::issuer::{ReadIssuer, ReadRoutes};
use crate::streaming::request::ReadRequest;
use crate::streaming::target_set::TargetSet;

const RATIO: u64 = 4;

/// The corridor with every pair coded: four decoded bytes per stored byte.
fn coded_corridor(half_bytes: u64) -> Vec<BlockSpec> {
    corridor_blocks(half_bytes, true)
        .into_iter()
        .map(|block| block.coded(RATIO))
        .collect()
}

// A pair is charged at its decoded size from its permit on: the
// in-hand caps and the shared drain count decoded bytes, never stored ones,
// so a coded build admits exactly what an uncoded build of the same decoded
// sizes admits.
#[test]
fn pairs_are_charged_at_decoded_size_in_hand_and_in_the_drain() {
    let mut coded = Rig::new(coded_corridor(64), corridor_set(), None);
    let mut raw = Rig::new(corridor_blocks(64 * RATIO, true), corridor_set(), None);
    for rig in [&mut coded, &mut raw] {
        let batch = rig.portal(0, &[]).unwrap();
        rig.install_all(&batch, headroom(8));
        assert_eq!(rig.requested_blocks(), vec![0, 1, 2]);
    }
    let decoded_pair = 2 * 64 * RATIO;
    assert_eq!(coded.controller.in_hand_bytes(), 3 * decoded_pair);
    assert_eq!(
        coded.controller.in_hand_bytes(),
        raw.controller.in_hand_bytes()
    );

    for rig in [&mut coded, &mut raw] {
        rig.complete_all();
        rig.drain.begin();
        rig.controller.offer_ready(&mut rig.drain).unwrap();
        // Room for two decoded pairs: a stored-size charge would admit all
        // three.
        rig.drain.admit_within(2 * decoded_pair).unwrap();
    }
    let admitted = |rig: &Rig| rig.drain.admission(StreamResource::LightmapBlock);
    assert_eq!(admitted(&coded).admitted, 2);
    assert_eq!(admitted(&coded).bytes, 2 * decoded_pair);
    assert_eq!(admitted(&coded), admitted(&raw));
}

// A decoded pair reaches the controller split per plane, and
// admitting it moves its buffers: the batch hands the renderer the very
// allocations the decode produced.
#[test]
fn a_decoded_pair_is_admitted_without_copying_a_payload_byte() {
    let mut rig = Rig::new(coded_corridor(64), corridor_set(), None);
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    let mut pointers = Vec::new();
    for request in std::mem::take(&mut rig.requests) {
        let completion = rig.source.completion(request);
        let LightmapReadResult::Decoded(payload) = &completion.result else {
            panic!("a decoded completion");
        };
        let [group_a, group_b] = payload.shadowmask.as_ref().unwrap();
        pointers.push((
            request.key,
            [
                payload.irradiance.as_ptr(),
                payload.direction.as_ptr(),
                group_a.as_ptr(),
                group_b.as_ptr(),
            ],
        ));
        rig.controller.admit_completion(completion).unwrap();
    }
    let batch = rig.portal(0, &[]).unwrap();
    assert_eq!(batch.ready.len(), 3);
    for prepared in &batch.ready {
        let payload = &prepared.payload;
        let [group_a, group_b] = payload.shadowmask.as_ref().unwrap();
        let expected = pointers
            .iter()
            .find(|(block, _)| *block == prepared.block)
            .unwrap()
            .1;
        assert_eq!(
            [
                payload.irradiance.as_ptr(),
                payload.direction.as_ptr(),
                group_a.as_ptr(),
                group_b.as_ptr(),
            ],
            expected,
            "block {}'s planes moved, never copied",
            prepared.block
        );
    }
}

/// The test source with decode instrumented: counts each block's decodes,
/// fails one block's, and holds one block's first decode until released.
struct DecodeProbe {
    inner: Arc<TestBlockSource>,
    corrupt: Option<u32>,
    held: Option<u32>,
    decodes: Vec<AtomicUsize>,
    /// Blocks in the order their decodes began.
    order: Mutex<Vec<u32>>,
    gate: (Mutex<GateState>, Condvar),
}

#[derive(Default)]
struct GateState {
    started: bool,
    released: bool,
}

impl DecodeProbe {
    fn new(inner: Arc<TestBlockSource>, corrupt: Option<u32>, held: Option<u32>) -> Arc<Self> {
        let decodes = (0..inner.block_count())
            .map(|_| AtomicUsize::new(0))
            .collect();
        Arc::new(Self {
            inner,
            corrupt,
            held,
            decodes,
            order: Mutex::default(),
            gate: Default::default(),
        })
    }

    fn decodes(&self, block: u32) -> usize {
        self.decodes[block as usize].load(Ordering::SeqCst)
    }

    fn order(&self) -> Vec<u32> {
        self.order.lock().unwrap().clone()
    }

    fn wait_for_held_decode(&self) {
        let (state, changed) = &self.gate;
        let mut state = state.lock().unwrap();
        while !state.started {
            state = changed.wait(state).unwrap();
        }
    }

    fn release(&self) {
        let (state, changed) = &self.gate;
        state.lock().unwrap().released = true;
        changed.notify_all();
    }
}

impl LightmapBlockSource for DecodeProbe {
    fn block_count(&self) -> u32 {
        self.inner.block_count()
    }

    fn block_summary(&self, block: u32) -> Option<BlockSummary> {
        self.inner.block_summary(block)
    }

    fn block_alignment(&self) -> u32 {
        self.inner.block_alignment()
    }

    fn block_file_ranges(&self, block: u32) -> Result<LightmapBlockFileRanges, PrlLoadError> {
        self.inner.block_file_ranges(block)
    }

    fn read_file_span(&self, range: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        self.inner.read_file_span(range)
    }

    fn payload_from_pair_bytes(
        &self,
        block: u32,
        lightmap: Vec<u8>,
        shadowmask: Option<Vec<u8>>,
    ) -> Result<LightmapBlockPayload, PrlLoadError> {
        self.order.lock().unwrap().push(block);
        let first = self.decodes[block as usize].fetch_add(1, Ordering::SeqCst) == 0;
        if self.held == Some(block) && first {
            let (state, changed) = &self.gate;
            let mut state = state.lock().unwrap();
            state.started = true;
            changed.notify_all();
            while !state.released {
                state = changed.wait(state).unwrap();
            }
        }
        if self.corrupt == Some(block) {
            return Err(PrlLoadError::SectionValidation {
                section: "lightmap streaming",
                message: format!("lightmap block {block} irradiance plane failed to decode"),
            });
        }
        self.inner
            .payload_from_pair_bytes(block, lightmap, shadowmask)
    }

    fn content_tag(&self) -> [u8; 32] {
        self.inner.content_tag()
    }
}

/// The rig's route on a real issuer over a one-thread decode pool.
struct Pipeline {
    ledger: Arc<LightmapRouteLedger>,
    completions: Receiver<LightmapCompletion>,
    issuer: Option<ReadIssuer>,
    handle: Option<JoinHandle<()>>,
    decode: Option<DecodePool>,
}

impl Pipeline {
    fn over(rig: &Rig, source: &Arc<DecodeProbe>) -> Self {
        let ledger = Arc::new(LightmapRouteLedger::default());
        let (route, completions) = lightmap_route(
            Arc::clone(source) as Arc<dyn LightmapBlockSource>,
            Arc::clone(rig.controller.target_set()),
            Arc::clone(&ledger),
        );
        let decode = DecodePool::spawn(1, LIGHTMAP_QUEUE_CAPACITY).unwrap();
        let (issuer, handle) = ReadIssuer::spawn(
            ReadRoutes::default().with(
                StreamResource::LightmapBlock,
                Box::new(route.attach(decode.queue())),
            ),
            LIGHTMAP_QUEUE_CAPACITY,
        )
        .unwrap();
        Self {
            ledger,
            completions,
            issuer: Some(issuer),
            handle: Some(handle),
            decode: Some(decode),
        }
    }

    fn submit(&self, requests: impl IntoIterator<Item = ReadRequest>) {
        for request in requests {
            self.issuer.as_ref().unwrap().submit(request).unwrap();
        }
    }

    /// The next `count` completions, each admitted as the session admits
    /// it: its charge released once the controller has consumed it. Returns
    /// each one's block and charge.
    fn admit(&self, rig: &mut Rig, count: usize) -> Vec<(u32, u64)> {
        let mut admitted = Vec::new();
        for _ in 0..count {
            let completion = self
                .completions
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("a completion");
            let charged = completion.charged_bytes;
            admitted.push((completion.request.key, charged));
            rig.controller.admit_completion(completion).unwrap();
            self.ledger.release(charged);
        }
        admitted
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        drop(self.issuer.take());
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
        drop(self.decode.take());
    }
}

// Retiring the level's workers while one pair decodes and others wait
// in the pool returns every charge: queued pairs are discarded, the running
// one releases instead of delivering, and nothing reaches the frame.
#[test]
fn retiring_mid_decode_returns_every_charge_and_delivers_nothing() {
    let mut rig = Rig::new(coded_corridor(64), corridor_set(), None);
    let source = DecodeProbe::new(Arc::clone(&rig.source), None, Some(0));
    let mut pipeline = Pipeline::over(&rig, &source);
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    assert_eq!(rig.requested_blocks(), vec![0, 1, 2]);
    pipeline.submit(std::mem::take(&mut rig.requests));
    source.wait_for_held_decode();
    let decoded_pair = 2 * 64 * RATIO;
    wait_until("every pair read and charged at decoded size", || {
        pipeline.ledger.in_memory_bytes() == 3 * decoded_pair
    });

    drop(pipeline.issuer.take());
    pipeline.handle.take().unwrap().join().unwrap();
    let threads = pipeline.decode.take().unwrap().begin_retirement();
    assert_eq!(
        pipeline.ledger.in_memory_bytes(),
        decoded_pair,
        "the queued pairs released at once"
    );
    source.release();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(pipeline.ledger.in_memory_bytes(), 0);
    assert!(
        pipeline.completions.try_recv().is_err(),
        "nothing delivered"
    );
    assert_eq!(source.decodes(1) + source.decodes(2), 0);
}

// Streaming: a pair whose plane fails its decode completes failed with
// no charge left behind, is never offered to the renderer, and fails like a
// failed read: one warning, `Failed` while targeted.
#[test]
fn a_corrupt_pair_fails_its_decode_and_is_never_uploaded() {
    let mut rig = Rig::new(coded_corridor(64), corridor_set(), None);
    let source = DecodeProbe::new(Arc::clone(&rig.source), Some(1), None);
    let pipeline = Pipeline::over(&rig, &source);
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    pipeline.submit(std::mem::take(&mut rig.requests));
    let completions = pipeline.admit(&mut rig, 3);
    let failed: Vec<(u32, u64)> = completions
        .into_iter()
        .filter(|&(block, _)| block == 1)
        .collect();
    assert_eq!(failed, vec![(1, 0)], "a failed decode returns its charge");
    assert_eq!(pipeline.ledger.in_memory_bytes(), 0);
    assert_eq!(rig.controller.phase(1), BlockPhase::Failed);
    assert_eq!(rig.controller.counters().failed_reads, 1);
    let batch = rig.portal(0, &[]).unwrap();
    let ready: Vec<u32> = batch.ready.iter().map(|prepared| prepared.block).collect();
    assert_eq!(ready, vec![0, 2], "the corrupt block is never uploaded");
}

// A band pair that turns mandatory after its read was delivered is never
// read or decoded again, and nothing is submitted for it: its block's live
// class lifts its queued decode ahead of every pair of the same or a less
// urgent class read after it; the pair already decoding finishes as it was.
// Each keeps one permit and one charge, and completes once.
#[test]
fn a_pair_raised_after_its_read_decodes_once_and_is_charged_once() {
    let mut rig = Rig::new(coded_corridor(64), corridor_set(), None);
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    rig.complete_all();
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    assert_eq!(rig.requested_blocks(), vec![3, 4], "the band reads");

    let source = DecodeProbe::new(Arc::clone(&rig.source), None, Some(3));
    let pipeline = Pipeline::over(&rig, &source);
    pipeline.submit(std::mem::take(&mut rig.requests));
    source.wait_for_held_decode();
    let decoded_pair = 2 * 64 * RATIO;
    wait_until("both band pairs read and queued for decode", || {
        pipeline.ledger.in_memory_bytes() == 2 * decoded_pair
    });
    let in_hand = rig.controller.in_hand_bytes();

    // Camera 2 makes 3 and 4 mandatory while 3 decodes and 4 is queued.
    let batch = rig.portal(2, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    assert!(
        rig.requests
            .iter()
            .all(|request| request.key != 3 && request.key != 4),
        "nothing is submitted for the raised pairs"
    );
    let raised_class = rig.controller.target_set().class(4).unwrap();
    assert_eq!(raised_class.tier(), ReadTier::Mandatory);
    let others: Vec<(u32, DrainClass)> = rig
        .requests
        .iter()
        .map(|request| (request.key, request.class))
        .collect();
    assert!(!others.is_empty(), "camera 2 reads pairs of its own");
    pipeline.submit(std::mem::take(&mut rig.requests));
    wait_until(
        "camera 2's new pairs read and queued behind block 4",
        || pipeline.ledger.in_memory_bytes() == (2 + others.len() as u64) * decoded_pair,
    );
    source.release();

    pipeline.admit(&mut rig, 2 + others.len());
    assert_eq!((source.decodes(3), source.decodes(4)), (1, 1));
    let order = source.order();
    let position = |block: u32| order.iter().position(|&b| b == block).unwrap();
    assert_eq!(order[0], 3, "the running decode finishes first");
    for &(block, class) in &others {
        if class >= raised_class {
            assert!(
                position(4) < position(block),
                "raised block 4 ({raised_class:?}) decodes before block {block} \
                 ({class:?}) read after it: {order:?}"
            );
        }
    }
    assert_eq!(rig.controller.counters().duplicate_completions, 0);
    assert!(
        pipeline
            .completions
            .recv_timeout(std::time::Duration::from_millis(100))
            .is_err(),
        "no second completion"
    );
    assert_eq!(
        rig.controller.in_hand_bytes() - in_hand,
        others.len() as u64 * decoded_pair,
        "the raised pairs keep their one charge; only camera 2's new reads add"
    );
    assert_eq!(pipeline.ledger.in_memory_bytes(), 0);
    let batch = rig.portal(2, &[]).unwrap();
    let ready: Vec<u32> = batch.ready.iter().map(|prepared| prepared.block).collect();
    assert!(ready.contains(&3) && ready.contains(&4), "{ready:?}");
}

/// Camera 0's three mandatory pairs on the pipeline, block 0's decode held
/// and blocks 1 and 2 read and queued behind it.
fn three_pairs_queued_behind_a_held_decode() -> (Rig, Arc<DecodeProbe>, Pipeline) {
    let mut rig = Rig::new(coded_corridor(64), corridor_set(), None);
    let source = DecodeProbe::new(Arc::clone(&rig.source), None, Some(0));
    let pipeline = Pipeline::over(&rig, &source);
    let batch = rig.portal(0, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    assert_eq!(rig.requested_blocks(), vec![0, 1, 2]);
    pipeline.submit(std::mem::take(&mut rig.requests));
    source.wait_for_held_decode();
    wait_until("every pair read and queued for decode", || {
        pipeline.ledger.in_memory_bytes() == 3 * 2 * 64 * RATIO
    });
    (rig, source, pipeline)
}

// A pair whose block leaves its targets while its decode is
// queued is not decoded: it completes cancelled with no charge, and the
// controller frees its permit and returns it to Absent.
#[test]
fn a_departed_pair_is_cancelled_without_decoding_and_its_charge_released() {
    let (mut rig, source, pipeline) = three_pairs_queued_behind_a_held_decode();
    // Camera 5's set holds none of blocks 0..=2.
    rig.portal(5, &[]);
    rig.requests.clear();
    source.release();
    let mut admitted = pipeline.admit(&mut rig, 3);
    admitted.sort_unstable();
    assert_eq!(
        admitted,
        vec![(0, 2 * 64 * RATIO), (1, 0), (2, 0)],
        "the departed queued pairs complete uncharged"
    );
    assert_eq!((source.decodes(1), source.decodes(2)), (0, 0));
    for block in 0..3 {
        assert_eq!(rig.controller.phase(block), BlockPhase::Absent, "{block}");
    }
    assert_eq!(rig.controller.counters().cancelled_reads, 2);
    assert_eq!(
        rig.controller.counters().departed_reads,
        1,
        "block 0 decoded"
    );
    assert_eq!(pipeline.ledger.in_memory_bytes(), 0);
}

// A block that leaves its targets and returns before its queued decode
// runs still decodes and becomes ready.
#[test]
fn a_pair_whose_target_returned_before_its_decode_still_decodes() {
    let (mut rig, source, pipeline) = three_pairs_queued_behind_a_held_decode();
    for camera in [5, 0] {
        let batch = rig.portal(camera, &[]).unwrap();
        rig.install_all(&batch, headroom(8));
    }
    rig.requests.clear();
    source.release();
    pipeline.admit(&mut rig, 3);
    assert_eq!((source.decodes(1), source.decodes(2)), (1, 1));
    let batch = rig.portal(0, &[]).unwrap();
    let ready: Vec<u32> = batch.ready.iter().map(|prepared| prepared.block).collect();
    assert_eq!(ready, vec![0, 1, 2]);
    assert_eq!(rig.controller.counters().cancelled_reads, 0);
}

/// Another resource's Visible decode: reports when the pool runs it.
struct OtherResourceJob(std::sync::mpsc::Sender<()>);

impl DecodeJob for OtherResourceJob {
    fn rank(&self) -> DecodeRank {
        DecodeRank::Decode {
            class: DrainClass::Visible,
            resource: StreamResource::Sh,
        }
    }

    fn run(&mut self, _: &AtomicBool, _: std::time::Duration) {
        self.0.send(()).unwrap();
    }

    fn discard(self: Box<Self>) {}

    fn panicked(self: Box<Self>, _: &AtomicBool) {}
}

/// An optional job of the least urgent rank: taken after every other job.
struct OptionalJob(OtherResourceJob);

impl DecodeJob for OptionalJob {
    fn rank(&self) -> DecodeRank {
        DecodeRank::Decode {
            class: DrainClass::Hysteresis,
            resource: StreamResource::LightmapBlock,
        }
    }

    fn run(&mut self, retiring: &AtomicBool, queue_wait: std::time::Duration) {
        self.0.run(retiring, queue_wait);
    }

    fn discard(self: Box<Self>) {}

    fn panicked(self: Box<Self>, _: &AtomicBool) {}
}

// A mandatory decode of the other resource queued behind
// departed lightmap pairs on a one-thread pool waits for the running decode
// only, never for theirs.
#[test]
fn a_mandatory_job_behind_departed_pairs_runs_without_waiting_for_their_decodes() {
    let (mut rig, source, pipeline) = three_pairs_queued_behind_a_held_decode();
    let (ran, other_ran) = std::sync::mpsc::channel();
    let queue = pipeline.decode.as_ref().unwrap().queue();
    assert!(queue.submit(Box::new(OtherResourceJob(ran))));
    rig.portal(5, &[]);
    rig.requests.clear();
    source.release();
    other_ran
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the other resource's decode runs");
    assert_eq!(
        source.order(),
        vec![0],
        "only the decode already running went first"
    );
    pipeline.admit(&mut rig, 3);
    assert_eq!(pipeline.ledger.in_memory_bytes(), 0);
}

// The queue bounds: every permit's read submitted at once fits the issuer's
// queue, and with nothing admitted every completion fits the completion
// queue, so the issuer and the decode thread finish every read and exit
// without waiting on the frame. Promoting every read meanwhile writes only
// the target set.
#[test]
fn every_permit_read_and_promoted_fits_both_queues_undrained() {
    let blocks = MAX_LIGHTMAP_PERMITS as u32;
    let source = TestBlockSource::new(
        (0..blocks)
            .map(|block| BlockSpec::standard(block, block, 64, true).coded(RATIO))
            .collect(),
    );
    let targets = Arc::new(TargetSet::new(blocks));
    targets.publish((0..blocks).map(|block| (block, DrainClass::Prefetch)));
    let ledger = Arc::new(LightmapRouteLedger::default());
    let (route, completions) = lightmap_route(
        Arc::clone(&source) as Arc<dyn LightmapBlockSource>,
        Arc::clone(&targets),
        Arc::clone(&ledger),
    );
    let decode = DecodePool::spawn(1, LIGHTMAP_QUEUE_CAPACITY).unwrap();
    let (issuer, handle) = ReadIssuer::spawn(
        ReadRoutes::default().with(
            StreamResource::LightmapBlock,
            Box::new(route.attach(decode.queue())),
        ),
        LIGHTMAP_QUEUE_CAPACITY,
    )
    .unwrap();
    let request = |block: u32| {
        let spec = source.spec(block);
        ReadRequest {
            resource: StreamResource::LightmapBlock,
            key: block,
            class: DrainClass::Prefetch,
            identity: crate::streaming::request::ReadIdentity {
                generation: 1,
                content_tag: [0; 32],
                item_hash: [0; 32],
            },
            ranges: crate::streaming::request::ReadRanges::pair(
                spec.lightmap.clone(),
                spec.shadowmask.clone().unwrap(),
            ),
        }
    };
    for block in 0..blocks {
        issuer.submit(request(block)).unwrap();
        targets.set(block, Some(DrainClass::Visible));
    }
    let decoded_pair = 2 * 64 * RATIO;
    wait_until("every read delivered to the decode pool", || {
        ledger.in_memory_bytes() == u64::from(blocks) * decoded_pair
    });
    // Queued behind every pair's decode: it runs only once each has sent its
    // completion.
    let (ran, last_ran) = std::sync::mpsc::channel();
    assert!(
        decode
            .queue()
            .submit(Box::new(OptionalJob(OtherResourceJob(ran))))
    );
    last_ran
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the decode thread never waits on the undrained queue");
    drop(issuer);
    handle.join().unwrap();
    drop(decode);
    let mut delivered: Vec<u32> = completions
        .try_iter()
        .map(|completion| {
            assert!(matches!(completion.result, LightmapReadResult::Decoded(_)));
            ledger.release(completion.charged_bytes);
            completion.request.key
        })
        .collect();
    delivered.sort_unstable();
    assert_eq!(
        delivered,
        (0..blocks).collect::<Vec<_>>(),
        "one completion per read"
    );
    assert_eq!(ledger.in_memory_bytes(), 0);
}

/// Holds the pool's one thread until its gate opens.
struct BlockingJob {
    started: std::sync::mpsc::Sender<()>,
    gate: Receiver<()>,
}

impl DecodeJob for BlockingJob {
    fn rank(&self) -> DecodeRank {
        DecodeRank::Decode {
            class: DrainClass::Visible,
            resource: StreamResource::Sh,
        }
    }

    fn run(&mut self, _: &AtomicBool, _: std::time::Duration) {
        self.started.send(()).unwrap();
        self.gate.recv().unwrap();
    }

    fn discard(self: Box<Self>) {}

    fn panicked(self: Box<Self>, _: &AtomicBool) {}
}

/// A mandatory decode that reports the route ledger's charge when it runs.
struct LedgerProbeJob {
    ledger: Arc<LightmapRouteLedger>,
    seen: std::sync::mpsc::Sender<u64>,
}

impl DecodeJob for LedgerProbeJob {
    fn rank(&self) -> DecodeRank {
        DecodeRank::Decode {
            class: DrainClass::Visible,
            resource: StreamResource::LightmapBlock,
        }
    }

    fn run(&mut self, _: &AtomicBool, _: std::time::Duration) {
        self.seen.send(self.ledger.in_memory_bytes()).unwrap();
    }

    fn discard(self: Box<Self>) {}

    fn panicked(self: Box<Self>, _: &AtomicBool) {}
}

// An optional pair whose block has left its targets is taken before older
// mandatory decodes: it returns its charge and completes cancelled without
// decoding, so its permit comes back without waiting behind them.
#[test]
fn a_departed_optional_pair_is_taken_before_older_mandatory_work_and_completes_cancelled() {
    let source = DecodeProbe::new(
        TestBlockSource::new(vec![BlockSpec::standard(0, 0, 64, true).coded(RATIO)]),
        None,
        None,
    );
    let ledger = Arc::new(LightmapRouteLedger::default());
    let pool = DecodePool::spawn(1, 8).unwrap();
    let queue = pool.queue();
    let (started, running) = std::sync::mpsc::channel();
    let (open, gate) = std::sync::mpsc::channel();
    assert!(queue.submit(Box::new(BlockingJob { started, gate })));
    running.recv().unwrap();

    let (seen, probes) = std::sync::mpsc::channel();
    for _ in 0..2 {
        assert!(queue.submit(Box::new(LedgerProbeJob {
            ledger: Arc::clone(&ledger),
            seen: seen.clone(),
        })));
    }
    ledger.charge(512);
    let (completed, completions) = std::sync::mpsc::sync_channel(1);
    assert!(queue.submit(Box::new(
        crate::lightmap_streaming::decode_job::LightmapDecodeJob {
            request: ReadRequest {
                resource: StreamResource::LightmapBlock,
                key: 0,
                class: DrainClass::Prefetch,
                identity: crate::streaming::request::ReadIdentity {
                    generation: 1,
                    content_tag: [0; 32],
                    item_hash: [0; 32],
                },
                ranges: crate::streaming::request::ReadRanges::one(0..0),
            },
            lightmap: Vec::new(),
            shadowmask: None,
            charged_bytes: 512,
            source: source.clone() as Arc<dyn LightmapBlockSource>,
            // Block 0 is not a member: it has left its targets.
            targets: Arc::new(TargetSet::new(1)),
            ledger: Arc::clone(&ledger),
            completed: Some(completed),
        }
    )));
    open.send(()).unwrap();
    let wait = std::time::Duration::from_secs(10);
    for _ in 0..2 {
        assert_eq!(
            probes.recv_timeout(wait).unwrap(),
            0,
            "the departed pair ran and returned its charge before each older mandatory job"
        );
    }
    let completion = completions.recv_timeout(wait).unwrap();
    assert!(matches!(completion.result, LightmapReadResult::Cancelled));
    assert_eq!(completion.charged_bytes, 0);
    assert_eq!(source.decodes(0), 0, "never decoded");
    drop(pool);
}

// A lightmap decode job dropped while still holding its charge (its pool's
// `panicked` cleanup panicked too) returns that charge.
#[test]
fn a_decode_job_dropped_holding_its_charge_returns_it() {
    let source = TestBlockSource::new(vec![BlockSpec::standard(0, 0, 64, true).coded(RATIO)]);
    let ledger = Arc::new(LightmapRouteLedger::default());
    ledger.charge(512);
    let (completed, _completions) = std::sync::mpsc::sync_channel(1);
    let job = crate::lightmap_streaming::decode_job::LightmapDecodeJob {
        request: ReadRequest {
            resource: StreamResource::LightmapBlock,
            key: 0,
            class: DrainClass::Visible,
            identity: crate::streaming::request::ReadIdentity {
                generation: 1,
                content_tag: [0; 32],
                item_hash: [0; 32],
            },
            ranges: crate::streaming::request::ReadRanges::one(0..0),
        },
        lightmap: Vec::new(),
        shadowmask: None,
        charged_bytes: 512,
        source: source as Arc<dyn LightmapBlockSource>,
        targets: Arc::new(TargetSet::new(1)),
        ledger: Arc::clone(&ledger),
        completed: Some(completed),
    };
    drop(job);
    assert_eq!(ledger.in_memory_bytes(), 0);
}
