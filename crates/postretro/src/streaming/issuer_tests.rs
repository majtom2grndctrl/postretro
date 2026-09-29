//! Threaded tests for the shared read issuer over recording, per-range
//! holding routes for SH and lightmap blocks. See: context/lib/testing_guide.md

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{JoinHandle, ThreadId};
use std::time::{Duration, Instant};

use postretro_level_loader::PrlLoadError;

use super::*;
use crate::streaming::request::{ReadIdentity, ReadRanges, ReadTier};
use crate::streaming::schedule::COALESCE_MAX_GAP_BYTES;
use crate::streaming::target_bitset::TargetBitset;

/// Far enough apart that no two test ranges coalesce.
const FAR: u64 = 4 * COALESCE_MAX_GAP_BYTES;
const LEN: u64 = 64;
const KEY_COUNT: u32 = 64;

fn at(slot: u64) -> Range<u64> {
    slot * FAR..slot * FAR + LEN
}

fn pattern(range: &Range<u64>) -> Vec<u8> {
    range.clone().map(|offset| (offset % 251) as u8).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Delivered {
    Read(Vec<Vec<u8>>),
    Failed,
    Cancelled,
}

/// Everything both routes record, in one order: physical reads with their
/// thread, deliveries, and the net bytes charged but not released.
#[derive(Default)]
struct Log {
    reads: Mutex<Vec<(StreamResource, Range<u64>, ThreadId)>>,
    deliveries: Mutex<Vec<(StreamResource, u32, Delivered)>>,
    /// Reads starting at these offsets block until released.
    held: Mutex<BTreeSet<u64>>,
    gate: Condvar,
    /// Reads starting at these offsets fail.
    failing: Mutex<BTreeSet<u64>>,
    charged: AtomicU64,
}

impl Log {
    fn hold(&self, range: &Range<u64>) {
        self.held.lock().unwrap().insert(range.start);
    }

    fn release(&self, range: &Range<u64>) {
        self.held.lock().unwrap().remove(&range.start);
        self.gate.notify_all();
    }

    fn release_all(&self) {
        self.held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.gate.notify_all();
    }

    fn wait_until(&self, what: &str, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::yield_now();
        }
    }

    fn wait_for_reads(&self, count: usize) {
        self.wait_until("reads", |log| log.reads.lock().unwrap().len() >= count);
    }

    fn wait_for_deliveries(&self, count: usize) {
        self.wait_until("deliveries", |log| {
            log.deliveries.lock().unwrap().len() >= count
        });
    }

    fn read_spans(&self) -> Vec<(StreamResource, Range<u64>)> {
        self.reads
            .lock()
            .unwrap()
            .iter()
            .map(|(resource, span, _)| (*resource, span.clone()))
            .collect()
    }

    fn deliveries(&self) -> Vec<(StreamResource, u32, Delivered)> {
        self.deliveries.lock().unwrap().clone()
    }
}

struct TestRoute {
    resource: StreamResource,
    targets: Arc<TargetBitset>,
    log: Arc<Log>,
}

impl ReadRoute for TestRoute {
    fn is_targeted(&self, key: u32) -> bool {
        self.targets.contains(key)
    }

    fn read_span(&self, span: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        self.log.reads.lock().unwrap().push((
            self.resource,
            span.clone(),
            std::thread::current().id(),
        ));
        let mut held = self.log.held.lock().unwrap();
        while held.contains(&span.start) {
            held = self.log.gate.wait(held).unwrap();
        }
        drop(held);
        if self.log.failing.lock().unwrap().contains(&span.start) {
            return Err(PrlLoadError::SectionValidation {
                section: "test route",
                message: "read failed".into(),
            });
        }
        Ok(pattern(&span))
    }

    fn charge_read_bytes(&self, bytes: u64) {
        self.log.charged.fetch_add(bytes, Ordering::SeqCst);
    }

    fn release_read_bytes(&self, bytes: u64) {
        self.log.charged.fetch_sub(bytes, Ordering::SeqCst);
    }

    fn record_read(&self, _read: &ReadRecord<'_>) {}

    fn deliver(&self, request: ReadRequest, outcome: ReadOutcome) -> bool {
        let delivered = match outcome {
            ReadOutcome::Read(parts) => Delivered::Read(parts.into_iter().collect()),
            ReadOutcome::Failed(_) => Delivered::Failed,
            ReadOutcome::Cancelled => Delivered::Cancelled,
        };
        self.log
            .deliveries
            .lock()
            .unwrap()
            .push((self.resource, request.key, delivered));
        true
    }
}

/// One issuer serving both resources. Dropping it releases every held read
/// and joins the thread, so a failed assertion cannot hang the suite.
struct Harness {
    issuer: Option<ReadIssuer>,
    handle: Option<JoinHandle<()>>,
    log: Arc<Log>,
    targets: BTreeMap<StreamResource, Arc<TargetBitset>>,
}

impl Harness {
    fn new() -> Self {
        let log = Arc::new(Log::default());
        let mut targets = BTreeMap::new();
        let mut routes = ReadRoutes::default();
        for resource in [StreamResource::Sh, StreamResource::LightmapBlock] {
            let bitset = Arc::new(TargetBitset::new(KEY_COUNT));
            bitset.publish(&(0..KEY_COUNT).collect());
            routes = routes.with(
                resource,
                Box::new(TestRoute {
                    resource,
                    targets: Arc::clone(&bitset),
                    log: Arc::clone(&log),
                }),
            );
            targets.insert(resource, bitset);
        }
        let (issuer, handle) = ReadIssuer::spawn(routes, 64).unwrap();
        Self {
            issuer: Some(issuer),
            handle: Some(handle),
            log,
            targets,
        }
    }

    fn submit(&self, request: ReadRequest) {
        self.issuer.as_ref().unwrap().submit(request).unwrap();
    }

    fn untarget(&self, resource: StreamResource, key: u32) {
        let keys = (0..KEY_COUNT).filter(|&other| other != key).collect();
        self.targets[&resource].publish(&keys);
    }

    /// Holds `blocker`'s read open so the requests submitted next all queue
    /// behind it, then releases it.
    fn behind_held_read(&self, blocker: ReadRequest, queue: impl FnOnce(&Self)) {
        let range = blocker.ranges.get(0);
        self.log.hold(&range);
        let reads_before = self.log.reads.lock().unwrap().len();
        self.submit(blocker);
        self.log.wait_for_reads(reads_before + 1);
        queue(self);
        self.log.release(&range);
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.log.release_all();
        drop(self.issuer.take());
        if let Some(handle) = self.handle.take() {
            handle.join().unwrap();
        }
    }
}

fn request(resource: StreamResource, key: u32, tier: ReadTier, ranges: ReadRanges) -> ReadRequest {
    ReadRequest {
        resource,
        key,
        tier,
        identity: ReadIdentity {
            generation: 5,
            content_tag: [5; 32],
            item_hash: [key as u8; 32],
        },
        ranges,
    }
}

fn sh(key: u32, tier: ReadTier, slot: u64) -> ReadRequest {
    request(StreamResource::Sh, key, tier, ReadRanges::one(at(slot)))
}

fn block_pair(key: u32, tier: ReadTier, first: u64, second: u64) -> ReadRequest {
    request(
        StreamResource::LightmapBlock,
        key,
        tier,
        ReadRanges::pair(at(first), at(second)),
    )
}

use ReadTier::{Mandatory as M, Optional as O};
use StreamResource::{LightmapBlock as Lm, Sh};

// AC 19: with SH and lightmap block demand both pending, one issuer thread
// performs every read, mandatory before optional across both resources,
// each tier in ascending file offset.
#[test]
fn shared_issuer_reads_both_resources_mandatory_first_in_ascending_offset_on_one_thread() {
    let harness = Harness::new();
    harness.behind_held_read(sh(0, M, 20), |harness| {
        harness.submit(sh(1, O, 2));
        harness.submit(block_pair(10, O, 0, 7));
        harness.submit(sh(2, M, 3));
        harness.submit(block_pair(11, M, 8, 4));
        harness.submit(sh(3, O, 6));
        harness.submit(sh(4, M, 1));
    });
    harness.log.wait_for_deliveries(7);

    assert_eq!(
        harness.log.read_spans(),
        vec![
            (Sh, at(20)),
            // Mandatory tier across both resources, ascending.
            (Sh, at(1)),
            (Sh, at(3)),
            (Lm, at(4)),
            (Lm, at(8)),
            // Optional tier across both resources, ascending.
            (Lm, at(0)),
            (Sh, at(2)),
            (Sh, at(6)),
            (Lm, at(7)),
        ]
    );
    let threads: HashSet<ThreadId> = harness
        .log
        .reads
        .lock()
        .unwrap()
        .iter()
        .map(|(_, _, thread)| *thread)
        .collect();
    assert_eq!(threads.len(), 1, "one issuer thread performs every read");
    assert!(!threads.contains(&std::thread::current().id()));

    // Each pair completes once, after its later range, with its parts in the
    // request's range order rather than read order.
    let deliveries = harness.log.deliveries();
    assert_eq!(deliveries.len(), 7);
    let pair = |key| {
        let matching: Vec<_> = deliveries
            .iter()
            .filter(|(resource, delivered_key, _)| *resource == Lm && *delivered_key == key)
            .collect();
        assert_eq!(matching.len(), 1, "block {key} completes exactly once");
        matching[0].2.clone()
    };
    assert_eq!(
        pair(11),
        Delivered::Read(vec![pattern(&at(8)), pattern(&at(4))])
    );
    assert_eq!(
        pair(10),
        Delivered::Read(vec![pattern(&at(0)), pattern(&at(7))])
    );
    let order: Vec<_> = deliveries.iter().map(|(r, key, _)| (*r, *key)).collect();
    assert_eq!(
        order,
        vec![
            (Sh, 0),
            (Sh, 4),
            (Sh, 2),
            (Lm, 11),
            (Sh, 1),
            (Sh, 3),
            (Lm, 10)
        ],
        "a pair is delivered when its last range lands, not its first"
    );
}

#[test]
fn shared_issuer_cancels_a_two_range_request_whole_after_its_first_range() {
    let harness = Harness::new();
    let first = at(1);
    harness.log.hold(&first);
    harness.submit(block_pair(9, M, 1, 5));
    harness.log.wait_for_reads(1);
    // The block leaves its targets while its first range is being read.
    harness.untarget(Lm, 9);
    harness.log.release(&first);
    harness.log.wait_for_deliveries(1);

    assert_eq!(harness.log.read_spans(), vec![(Lm, first)]);
    assert_eq!(
        harness.log.deliveries(),
        vec![(Lm, 9, Delivered::Cancelled)]
    );
    assert_eq!(
        harness.log.charged.load(Ordering::SeqCst),
        0,
        "the first range's bytes are released with the cancelled request"
    );
}

#[test]
fn shared_issuer_fails_a_two_range_request_whole_when_its_second_range_fails() {
    let harness = Harness::new();
    harness.log.failing.lock().unwrap().insert(at(5).start);
    harness.submit(block_pair(9, M, 1, 5));
    harness.log.wait_for_deliveries(1);

    assert_eq!(harness.log.read_spans(), vec![(Lm, at(1)), (Lm, at(5))]);
    assert_eq!(harness.log.deliveries(), vec![(Lm, 9, Delivered::Failed)]);
    assert_eq!(harness.log.charged.load(Ordering::SeqCst), 0);
}

// P5 (issuer half): N submissions for one key while its request is pending
// produce one read and one completion. A mandatory duplicate raises the
// pending request's tier.
#[test]
fn shared_issuer_absorbs_repeat_submissions_of_a_pending_key_into_one_read() {
    let harness = Harness::new();
    harness.behind_held_read(sh(0, M, 20), |harness| {
        harness.submit(sh(1, O, 2));
        harness.submit(block_pair(7, O, 4, 6));
        harness.submit(block_pair(7, O, 4, 6));
        harness.submit(block_pair(7, M, 4, 6));
        harness.submit(block_pair(7, O, 4, 6));
    });
    harness.log.wait_for_deliveries(3);
    // Nothing else is queued; give a stray second read time to show.
    std::thread::sleep(Duration::from_millis(50));

    assert_eq!(
        harness.log.read_spans(),
        vec![(Sh, at(20)), (Lm, at(4)), (Lm, at(6)), (Sh, at(2))],
        "the block reads once, raised to the mandatory tier"
    );
    let block: Vec<_> = harness
        .log
        .deliveries()
        .into_iter()
        .filter(|(resource, _, _)| *resource == Lm)
        .collect();
    assert_eq!(block.len(), 1);
}
