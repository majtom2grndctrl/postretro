//! Lightmap blocks' route on the shared read issuer: targets, bytes, decode handoff.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use postretro_level_format::lightmap::LightmapBlockPayload;
use postretro_level_loader::PrlLoadError;

use super::controller::LIGHTMAP_COMPLETION_CAPACITY;
use super::decode_job::LightmapDecodeJob;
use super::source::LightmapBlockSource;
use crate::streaming::decode_pool::DecodeQueue;
use crate::streaming::drain_budget::DrainClass;
use crate::streaming::issuer::{ReadOutcome, ReadRecord, ReadRoute};
use crate::streaming::latency_histogram::LatencyHistogram;
use crate::streaming::request::ReadRequest;
use crate::streaming::target_set::TargetSet;

/// A block pair as the controller receives it. The issuer only reads; the
/// level's decode pool decodes each read pair and splits it per plane, so the
/// frame thread admits it without touching a payload byte.
#[derive(Debug)]
pub(crate) enum LightmapReadResult {
    /// Read and decoded: every plane in its own buffer.
    Decoded(LightmapBlockPayload),
    Failed(PrlLoadError),
    /// Read whole, but a plane failed its decode; the pair is never uploaded.
    DecodeFailed(PrlLoadError),
    /// The block left the target set before every range was read, or
    /// before its decode began.
    Cancelled,
}

#[derive(Debug)]
pub(crate) struct LightmapCompletion {
    pub(crate) request: ReadRequest,
    pub(crate) result: LightmapReadResult,
    /// Bytes charged to the route ledger for this completion until its
    /// consumer releases them: a decoded pair's decoded size, otherwise 0.
    pub(crate) charged_bytes: u64,
}

/// Per-pair decode timings, for measurement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LightmapDecodeStats {
    /// Per decoded pair: submission to the decode pool to its decode
    /// starting.
    pub(crate) queue_wait: LatencyHistogram,
    /// Per decoded pair: decode time on a pool thread.
    pub(crate) decode_time: LatencyHistogram,
}

/// Route-side counters shared with the frame thread.
#[derive(Debug, Default)]
pub(crate) struct LightmapRouteLedger {
    /// Payload bytes in hand and not yet consumed by the controller: stored
    /// bytes while the issuer reads a pair, then the pair's decoded size from
    /// the moment it is queued for decode until the frame admits or drops it.
    in_memory_bytes: AtomicU64,
    physical_reads: AtomicU64,
    span_bytes: AtomicU64,
    /// Bytes read only to bridge ranges; lightmap reads merge only
    /// byte-contiguous ranges, so this stays 0.
    gap_bytes: AtomicU64,
    /// Held only to record one pair's two samples; a poisoned lock is
    /// recovered, since a sample is whole or absent.
    decode_stats: Mutex<LightmapDecodeStats>,
}

impl LightmapRouteLedger {
    pub(crate) fn in_memory_bytes(&self) -> u64 {
        self.in_memory_bytes.load(Ordering::Acquire)
    }

    /// Positional reads the issuer performed for this route; a coalesced
    /// read of several pairs counts once.
    pub(crate) fn physical_reads(&self) -> u64 {
        self.physical_reads.load(Ordering::Acquire)
    }

    /// Bytes the issuer read for this route and discarded between ranges.
    #[cfg(test)]
    pub(crate) fn gap_bytes(&self) -> u64 {
        self.gap_bytes.load(Ordering::Acquire)
    }

    pub(super) fn charge(&self, bytes: u64) {
        self.in_memory_bytes.fetch_add(bytes, Ordering::AcqRel);
    }

    /// The frame thread consumed `bytes` of delivered payload, or a job
    /// returned its charge undelivered.
    pub(crate) fn release(&self, bytes: u64) {
        self.in_memory_bytes.fetch_sub(bytes, Ordering::AcqRel);
    }

    pub(super) fn record_decode(&self, queue_wait: Duration, decode_time: Duration) {
        let mut stats = self
            .decode_stats
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        stats.queue_wait.record(queue_wait);
        stats.decode_time.record(decode_time);
    }

    /// Decode timings of every pair decoded so far.
    #[cfg(test)]
    pub(crate) fn decode_stats(&self) -> LightmapDecodeStats {
        *self
            .decode_stats
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// The session's half of a route before the level's issuer and decode pool
/// exist: the completion sender the route and its decode jobs deliver into.
pub(crate) struct PendingLightmapRoute {
    source: Arc<dyn LightmapBlockSource>,
    targets: Arc<TargetSet>,
    ledger: Arc<LightmapRouteLedger>,
    completed: SyncSender<LightmapCompletion>,
}

impl PendingLightmapRoute {
    /// The route over the level's decode pool.
    pub(crate) fn attach(self, decode: DecodeQueue) -> LightmapReadRoute {
        LightmapReadRoute {
            source: self.source,
            targets: self.targets,
            ledger: self.ledger,
            completed: self.completed,
            decode,
        }
    }
}

/// Reads through the level's source, checks the controller's block targets,
/// and hands each read pair to the decode pool, ranked by its block's live
/// class; cancellations and failed reads complete directly.
pub(crate) struct LightmapReadRoute {
    source: Arc<dyn LightmapBlockSource>,
    targets: Arc<TargetSet>,
    ledger: Arc<LightmapRouteLedger>,
    completed: SyncSender<LightmapCompletion>,
    decode: DecodeQueue,
}

/// A pending route plus the frame side's completion receiver. The queue
/// holds every completion the controller's permits can have outstanding (see
/// [`LIGHTMAP_COMPLETION_CAPACITY`]), so neither the issuer nor a decode
/// thread ever waits on the frame.
pub(crate) fn lightmap_route(
    source: Arc<dyn LightmapBlockSource>,
    targets: Arc<TargetSet>,
    ledger: Arc<LightmapRouteLedger>,
) -> (PendingLightmapRoute, Receiver<LightmapCompletion>) {
    let (completed, receiver) = sync_channel(LIGHTMAP_COMPLETION_CAPACITY);
    (
        PendingLightmapRoute {
            source,
            targets,
            ledger,
            completed,
        },
        receiver,
    )
}

impl LightmapReadRoute {
    fn complete(&self, request: ReadRequest, result: LightmapReadResult) -> bool {
        self.completed
            .send(LightmapCompletion {
                request,
                result,
                charged_bytes: 0,
            })
            .is_ok()
    }
}

impl ReadRoute for LightmapReadRoute {
    fn is_targeted(&self, key: u32) -> bool {
        self.targets.contains(key)
    }

    fn live_class(&self, key: u32) -> Option<DrainClass> {
        self.targets.class(key)
    }

    fn read_span(&self, span: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        self.source.read_file_span(span)
    }

    fn charge_read_bytes(&self, bytes: u64) {
        self.ledger.charge(bytes);
    }

    fn release_read_bytes(&self, bytes: u64) {
        self.ledger.release(bytes);
    }

    fn record_read(&self, read: &ReadRecord<'_>) {
        self.ledger.physical_reads.fetch_add(1, Ordering::AcqRel);
        self.ledger
            .span_bytes
            .fetch_add(read.span_bytes, Ordering::AcqRel);
        self.ledger
            .gap_bytes
            .fetch_add(read.gap_bytes, Ordering::AcqRel);
    }

    /// A read pair is charged at its decoded size from here on, its stored
    /// bytes released, and queued for decode, ranked by its block's live
    /// class. False when the session or the decode pool is gone; the issuer
    /// then exits.
    fn deliver(&self, request: ReadRequest, outcome: ReadOutcome) -> bool {
        match outcome {
            ReadOutcome::Read(parts) => {
                let mut parts = parts.into_iter();
                let lightmap = parts.next().unwrap_or_default();
                // A two-range request is a pair: its second range is id 42.
                let shadowmask =
                    (request.ranges.len() > 1).then(|| parts.next().unwrap_or_default());
                let stored =
                    lightmap.len() as u64 + shadowmask.as_ref().map_or(0, |b| b.len() as u64);
                let decoded = self.source.decoded_pair_bytes(request.key);
                self.ledger.charge(decoded);
                self.ledger.release(stored);
                self.decode.submit(Box::new(LightmapDecodeJob {
                    request,
                    lightmap,
                    shadowmask,
                    charged_bytes: decoded,
                    source: Arc::clone(&self.source),
                    targets: Arc::clone(&self.targets),
                    ledger: Arc::clone(&self.ledger),
                    completed: Some(self.completed.clone()),
                }))
            }
            ReadOutcome::Failed(error) => self.complete(request, LightmapReadResult::Failed(error)),
            ReadOutcome::Cancelled => self.complete(request, LightmapReadResult::Cancelled),
        }
    }
}

#[cfg(test)]
#[path = "route_tests.rs"]
mod tests;
