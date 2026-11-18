//! SH's route on the shared read issuer: targets, byte accounting, decode handoff.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency".

use std::ops::Range;
use std::sync::Arc;
use std::sync::mpsc::SyncSender;

use postretro_level_loader::PrlLoadError;

use super::decode_job::ShDecodeJob;
use super::{ShWorkerCompletion, ShWorkerResult, ShWorkerSource, WorkerShared};
use crate::sh_streaming::controller::ShClusterRequest;
use crate::streaming::drain_budget::DrainClass;
use crate::streaming::issuer::{ReadOutcome, ReadRecord, ReadRoute};
use crate::streaming::request::{ReadIdentity, ReadRanges, ReadRequest, StreamResource};

/// The shared-issuer request for one SH cluster chunk at `range`.
pub(super) fn read_request(request: ShClusterRequest, range: Range<u64>) -> ReadRequest {
    ReadRequest {
        resource: StreamResource::Sh,
        key: request.cluster_id,
        class: request.class,
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
        class: request.class,
    }
}

/// Read bytes go to the level's decode pool, ranked by their cluster's live
/// class; cancellations and failures complete directly, without a decode.
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

    fn live_class(&self, key: u32) -> Option<DrainClass> {
        self.shared.targets.class(key)
    }

    fn read_span(&self, span: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        self.source.read_file_span(span)
    }

    fn charge_read_bytes(&self, bytes: u64) {
        // Checked once the guard has dropped, so a failure never poisons the
        // ledger.
        let charged = self
            .shared
            .phases()
            .encoded
            .add(bytes, "encoded worker bytes");
        charged.expect("validated encoded bound");
    }

    fn release_read_bytes(&self, bytes: u64) {
        let released = self
            .shared
            .phases()
            .encoded
            .remove(bytes, "encoded worker bytes");
        released.expect("balanced encoded read");
    }

    fn record_read(&self, read: &ReadRecord<'_>) {
        let mut stats = self.shared.stats();
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
                // Cancelled workers have taken the queue: SH's receiver is
                // gone, and the read bytes drop here.
                let Some(decode) = self.shared.decode_queue() else {
                    self.release_read_bytes(bytes.len() as u64);
                    return false;
                };
                // From here the chunk is charged at its decoded size, as a
                // lightmap pair is; its stored bytes leave the encoded phase.
                // A retired pool discards the job, releasing that charge.
                let decoded_bytes = self.source.decoded_bytes(request.cluster_id);
                let moved = self
                    .shared
                    .phases()
                    .move_encoded_to_decoding(bytes.len() as u64, decoded_bytes);
                moved.expect("balanced encoded read within the validated decode bound");
                decode.submit(Box::new(ShDecodeJob {
                    request,
                    bytes,
                    decoded_bytes,
                    source: Arc::clone(&self.source),
                    shared: Arc::clone(&self.shared),
                    completed: Some(self.completed.clone()),
                }))
            }
            ReadOutcome::Failed(error) => self.complete(request, ShWorkerResult::Failed(error)),
            ReadOutcome::Cancelled => self.complete(request, ShWorkerResult::Cancelled),
        }
    }
}
