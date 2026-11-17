//! Coded pairs through the route and the decode pool: decoded-size charges,
//! pre-split admission, corrupt blocks, and a raised pair's second decode.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
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
use crate::streaming::decode_pool::DecodePool;
use crate::streaming::issuer::{ReadIssuer, ReadRoutes};
use crate::streaming::request::ReadRequest;

const RATIO: u64 = 4;

/// The corridor with every pair coded: four decoded bytes per stored byte.
fn coded_corridor(half_bytes: u64) -> Vec<BlockSpec> {
    corridor_blocks(half_bytes, true)
        .into_iter()
        .map(|block| block.coded(RATIO))
        .collect()
}

// AC 17: a pair is charged at its decoded size from its permit on: the
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

// AC 16: a decoded pair reaches the controller split per plane, and
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
            gate: Default::default(),
        })
    }

    fn decodes(&self, block: u32) -> usize {
        self.decodes[block as usize].load(Ordering::SeqCst)
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
            Arc::clone(rig.controller.target_bitset()),
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

// AC 23: retiring the level's workers while one pair decodes and others wait
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

// AC 2 (streaming): a pair whose plane fails its decode completes failed with
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

// AC 21 / P4: a band pair raised to mandatory after its read was delivered,
// while its first decode is still running, is read and decoded a second time
// at the mandatory tier (at most twice), keeps one permit and one in-hand
// charge, and its duplicate completion is released.
#[test]
fn a_pair_raised_while_decoding_decodes_at_most_twice_and_is_charged_once() {
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
    let in_hand = rig.controller.in_hand_bytes();

    // Camera 2 makes 3 and 4 mandatory while 3 decodes and 4 is queued.
    let batch = rig.portal(2, &[]).unwrap();
    rig.install_all(&batch, headroom(8));
    let raised: Vec<(u32, ReadTier)> = rig
        .requests
        .iter()
        .map(|request| (request.key, request.tier))
        .filter(|&(key, _)| key == 3 || key == 4)
        .collect();
    assert_eq!(
        raised,
        vec![(3, ReadTier::Mandatory), (4, ReadTier::Mandatory)]
    );
    let others = rig.requests.len() - raised.len();
    pipeline.submit(std::mem::take(&mut rig.requests));
    source.release();

    pipeline.admit(&mut rig, 4 + others);
    assert_eq!(
        source.decodes(3),
        2,
        "the raise reads and decodes once more"
    );
    assert!(source.decodes(4) <= 2);
    assert_eq!(rig.controller.counters().duplicate_completions, 2);
    assert_eq!(
        rig.controller.in_hand_bytes() - in_hand,
        others as u64 * 2 * 64 * RATIO,
        "the raised pairs keep their one charge; only camera 2's new reads add"
    );
    assert_eq!(
        pipeline.ledger.in_memory_bytes(),
        0,
        "every duplicate released"
    );
    let batch = rig.portal(2, &[]).unwrap();
    let ready: Vec<u32> = batch.ready.iter().map(|prepared| prepared.block).collect();
    assert!(ready.contains(&3) && ready.contains(&4), "{ready:?}");
}
