//! SH's route on the shared read issuer: targets, byte accounting, decode handoff.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency".

use std::ops::Range;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;

use postretro_level_loader::PrlLoadError;

use super::{DecodeJob, ShWorkerCompletion, ShWorkerResult, ShWorkerSource, WorkerShared};
use crate::sh_streaming::controller::ShClusterRequest;
use crate::streaming::issuer::{ReadOutcome, ReadRecord, ReadRoute};
use crate::streaming::request::{ReadIdentity, ReadRanges, ReadRequest, ReadTier, StreamResource};

/// The shared-issuer request for one SH cluster chunk at `range`.
pub(super) fn read_request(request: ShClusterRequest, range: Range<u64>) -> ReadRequest {
    ReadRequest {
        resource: StreamResource::Sh,
        key: request.cluster_id,
        tier: if request.mandatory {
            ReadTier::Mandatory
        } else {
            ReadTier::Optional
        },
        identity: ReadIdentity {
            generation: request.generation,
            content_tag: request.content_tag,
            item_hash: request.chunk_hash,
        },
        ranges: ReadRanges::one(range),
    }
}

fn cluster_request(request: ReadRequest) -> ShClusterRequest {
    ShClusterRequest {
        generation: request.identity.generation,
        content_tag: request.identity.content_tag,
        cluster_id: request.key,
        chunk_hash: request.identity.item_hash,
        mandatory: request.tier == ReadTier::Mandatory,
    }
}

/// Read bytes go to the decode pool; cancellations and failures complete
/// directly, without a decode.
pub(in crate::session) struct ShReadRoute {
    pub(super) source: Arc<dyn ShWorkerSource>,
    pub(super) shared: Arc<WorkerShared>,
    pub(super) completed: SyncSender<ShWorkerCompletion>,
}

impl ShReadRoute {
    fn complete(&self, request: ShClusterRequest, result: ShWorkerResult) -> bool {
        self.completed
            .send(ShWorkerCompletion {
                request,
                result,
                ready_bytes: 0,
            })
            .is_ok()
    }
}

impl ReadRoute for ShReadRoute {
    fn is_targeted(&self, key: u32) -> bool {
        self.shared.targets.contains(key)
    }

    fn read_span(&self, span: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        self.source.read_file_span(span)
    }

    fn charge_read_bytes(&self, bytes: u64) {
        self.shared
            .phases
            .lock()
            .expect("SH phase mutex poisoned")
            .encoded
            .add(bytes, "encoded worker bytes")
            .expect("validated encoded bound");
    }

    fn release_read_bytes(&self, bytes: u64) {
        self.shared
            .phases
            .lock()
            .expect("SH phase mutex poisoned")
            .encoded
            .remove(bytes, "encoded worker bytes")
            .expect("balanced encoded read");
    }

    fn record_read(&self, read: &ReadRecord<'_>) {
        let mut stats = self.shared.stats.lock().expect("SH stats mutex poisoned");
        stats.record_read(read.span_bytes, read.ranges_served, read.gap_bytes);
        for &latency in read.latencies {
            stats.read_latency.record(latency);
        }
    }

    fn deliver(&self, request: ReadRequest, outcome: ReadOutcome) -> bool {
        let request = cluster_request(request);
        match outcome {
            ReadOutcome::Read(parts) => {
                // An SH request carries exactly one range.
                let bytes = parts.into_iter().next().unwrap_or_default();
                // Cancelled workers have taken the sender: SH's receiver is gone.
                self.shared
                    .decode_sender()
                    .is_some_and(|decode| decode.send(DecodeJob { request, bytes }).is_ok())
            }
            ReadOutcome::Failed(error) => self.complete(request, ShWorkerResult::Failed(error)),
            ReadOutcome::Cancelled => self.complete(request, ShWorkerResult::Cancelled),
        }
    }
}
