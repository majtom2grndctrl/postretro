//! Lightmap blocks' route on the shared read issuer: targets, bytes, decode handoff.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use postretro_level_format::lightmap::LightmapBlockPayload;
use postretro_level_loader::PrlLoadError;

use super::controller::LIGHTMAP_QUEUE_CAPACITY;
use super::decode_job::LightmapDecodeJob;
use super::source::LightmapBlockSource;
use crate::streaming::decode_pool::DecodeQueue;
use crate::streaming::issuer::{ReadOutcome, ReadRecord, ReadRoute};
use crate::streaming::request::ReadRequest;
use crate::streaming::target_bitset::TargetBitset;

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
    /// The block left the target set before every range was read.
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
}

/// The session's half of a route before the level's issuer and decode pool
/// exist: the completion sender the route and its decode jobs deliver into.
pub(crate) struct PendingLightmapRoute {
    source: Arc<dyn LightmapBlockSource>,
    targets: Arc<TargetBitset>,
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

/// Reads through the level's source, checks the block target bitset, and
/// hands each read pair to the decode pool; cancellations and failed reads
/// complete directly.
pub(crate) struct LightmapReadRoute {
    source: Arc<dyn LightmapBlockSource>,
    targets: Arc<TargetBitset>,
    ledger: Arc<LightmapRouteLedger>,
    completed: SyncSender<LightmapCompletion>,
    decode: DecodeQueue,
}

/// A pending route plus the frame side's completion receiver. The queue
/// holds every completion the controller's submissions can produce (see
/// [`LIGHTMAP_QUEUE_CAPACITY`]), so neither the issuer nor a decode thread
/// ever waits on the frame.
pub(crate) fn lightmap_route(
    source: Arc<dyn LightmapBlockSource>,
    targets: Arc<TargetBitset>,
    ledger: Arc<LightmapRouteLedger>,
) -> (PendingLightmapRoute, Receiver<LightmapCompletion>) {
    let (completed, receiver) = sync_channel(LIGHTMAP_QUEUE_CAPACITY);
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
    /// bytes released, and queued for decode at its request's tier. False
    /// when the session or the decode pool is gone; the issuer then exits.
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
                    ledger: Arc::clone(&self.ledger),
                    completed: self.completed.clone(),
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
