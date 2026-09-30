//! Lightmap blocks' route on the shared read issuer: targets, bytes, completions.
//! See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use postretro_level_loader::PrlLoadError;

use super::controller::LIGHTMAP_QUEUE_CAPACITY;
use super::source::LightmapBlockSource;
use crate::streaming::issuer::{ReadOutcome, ReadRecord, ReadRoute};
use crate::streaming::request::ReadRequest;
use crate::streaming::target_bitset::TargetBitset;

/// A block pair's bytes as read: id 22's range, then id 42's when the level
/// keeps it. Blocks upload as stored, so nothing is decoded.
#[derive(Debug)]
pub(crate) enum LightmapReadResult {
    Read {
        lightmap: Vec<u8>,
        shadowmask: Option<Vec<u8>>,
    },
    Failed(PrlLoadError),
    /// The block left the target set before every range was read.
    Cancelled,
}

impl LightmapReadResult {
    /// Bytes the issuer charged to the route for this result, still held.
    pub(crate) fn read_bytes(&self) -> u64 {
        match self {
            Self::Read {
                lightmap,
                shadowmask,
            } => lightmap.len() as u64 + shadowmask.as_ref().map_or(0, |bytes| bytes.len() as u64),
            Self::Failed(_) | Self::Cancelled => 0,
        }
    }
}

#[derive(Debug)]
pub(crate) struct LightmapCompletion {
    pub(crate) request: ReadRequest,
    pub(crate) result: LightmapReadResult,
}

/// Route-side counters shared with the frame thread.
#[derive(Debug, Default)]
pub(crate) struct LightmapRouteLedger {
    /// Payload bytes read and not yet consumed by the controller.
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

    /// The frame thread consumed `bytes` of delivered payload.
    pub(crate) fn release(&self, bytes: u64) {
        self.in_memory_bytes.fetch_sub(bytes, Ordering::AcqRel);
    }
}

/// Reads through the level's source, checks the block target bitset, and
/// hands each request's one outcome straight to the completion queue.
pub(crate) struct LightmapReadRoute {
    source: Arc<dyn LightmapBlockSource>,
    targets: Arc<TargetBitset>,
    ledger: Arc<LightmapRouteLedger>,
    completed: SyncSender<LightmapCompletion>,
}

/// The route plus the frame side's completion receiver. It holds every
/// completion the controller's submissions can produce (see
/// [`LIGHTMAP_QUEUE_CAPACITY`]), so a delivery never waits on the frame.
pub(crate) fn lightmap_route(
    source: Arc<dyn LightmapBlockSource>,
    targets: Arc<TargetBitset>,
    ledger: Arc<LightmapRouteLedger>,
) -> (LightmapReadRoute, Receiver<LightmapCompletion>) {
    let (completed, receiver) = sync_channel(LIGHTMAP_QUEUE_CAPACITY);
    (
        LightmapReadRoute {
            source,
            targets,
            ledger,
            completed,
        },
        receiver,
    )
}

impl ReadRoute for LightmapReadRoute {
    fn is_targeted(&self, key: u32) -> bool {
        self.targets.contains(key)
    }

    fn read_span(&self, span: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        self.source.read_file_span(span)
    }

    fn charge_read_bytes(&self, bytes: u64) {
        self.ledger
            .in_memory_bytes
            .fetch_add(bytes, Ordering::AcqRel);
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

    fn deliver(&self, request: ReadRequest, outcome: ReadOutcome) -> bool {
        let result = match outcome {
            ReadOutcome::Read(parts) => {
                let mut parts = parts.into_iter();
                let lightmap = parts.next().unwrap_or_default();
                // A two-range request is a pair: its second range is id 42.
                let shadowmask =
                    (request.ranges.len() > 1).then(|| parts.next().unwrap_or_default());
                LightmapReadResult::Read {
                    lightmap,
                    shadowmask,
                }
            }
            ReadOutcome::Failed(error) => LightmapReadResult::Failed(error),
            ReadOutcome::Cancelled => LightmapReadResult::Cancelled,
        };
        let read_bytes = result.read_bytes();
        if self
            .completed
            .send(LightmapCompletion { request, result })
            .is_err()
        {
            // The session is gone; its bytes drop here, undelivered.
            self.ledger.release(read_bytes);
            return false;
        }
        true
    }
}

#[cfg(test)]
#[path = "route_tests.rs"]
mod tests;
