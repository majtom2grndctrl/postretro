//! SH's decode job on the level's decode pool: verify and decode one encoded
//! chunk the issuer read, then publish its completion to the session.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency".

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::time::{Duration, Instant};

use postretro_level_loader::PrlLoadError;

use super::{ShWorkerCompletion, ShWorkerResult, ShWorkerSource, WorkerShared};
use crate::sh_streaming::controller::{ShClusterRequest, ShResidencyControllerError};
use crate::streaming::decode_pool::{DecodeJob, DecodeRank};
use crate::streaming::request::StreamResource;

/// A read chunk's stored bytes, queued on the level's decode pool. It is
/// charged to the decoding phase at its decoded size from the moment it is
/// queued; the job moves that charge to the ready phase when it delivers, and
/// releases it when it fails or panics, finds its cluster untargeted, is
/// discarded, finds the pool retiring, or drops holding it.
pub(super) struct ShDecodeJob {
    pub(super) request: ShClusterRequest,
    pub(super) bytes: Vec<u8>,
    /// Charged to the decoding phase and not yet released.
    pub(super) decoded_bytes: u64,
    pub(super) source: Arc<dyn ShWorkerSource>,
    pub(super) shared: Arc<WorkerShared>,
    /// Taken by the chunk's one completion.
    pub(super) completed: Option<SyncSender<ShWorkerCompletion>>,
}

impl ShDecodeJob {
    /// Returns the decoding charge. An unbalanced ledger panics, after the
    /// guard has dropped.
    fn release_decoding(&mut self) {
        if let Err(error) = self.try_release_decoding() {
            panic!("balanced decode: {error}");
        }
    }

    fn try_release_decoding(&mut self) -> Result<(), ShResidencyControllerError> {
        let decoded_bytes = std::mem::take(&mut self.decoded_bytes);
        if decoded_bytes == 0 {
            return Ok(());
        }
        self.shared
            .phases()
            .decoding
            .remove(decoded_bytes, "decoded worker bytes")
    }

    fn stopped(&self, retiring: &AtomicBool) -> bool {
        self.shared.cancelled() || retiring.load(Ordering::Acquire)
    }

    /// Sends the chunk's one completion. A gone session drops it; its ledger
    /// goes with it.
    fn complete(&mut self, result: ShWorkerResult, ready_bytes: u64) {
        if let Some(completed) = self.completed.take() {
            let _ = completed.send(ShWorkerCompletion {
                request: self.request,
                result,
                ready_bytes,
            });
        }
    }
}

impl DecodeJob for ShDecodeJob {
    /// Departed once the cluster leaves its targets (its run then completes
    /// cancelled without a decode), otherwise the cluster's live class.
    fn rank(&self) -> DecodeRank {
        DecodeRank::live(
            &self.shared.targets,
            self.request.cluster_id,
            self.request.class,
            StreamResource::Sh,
        )
    }

    fn run(&mut self, retiring: &AtomicBool, queue_wait: Duration) {
        if self.stopped(retiring) {
            return self.release_decoding();
        }
        if !self.shared.targets.contains(self.request.cluster_id) {
            // Left its targets while queued: no decode. The controller frees
            // its permit; a cluster targeted again before this check decodes.
            self.release_decoding();
            return self.complete(ShWorkerResult::Cancelled, 0);
        }
        let started = Instant::now();
        let result = self
            .source
            .decode(self.request.cluster_id, std::mem::take(&mut self.bytes));
        let elapsed = started.elapsed();
        {
            let mut stats = self.shared.stats();
            stats.decode_queue_wait.record(queue_wait);
            stats.decode_latency.record(elapsed);
        }
        self.release_decoding();
        if self.stopped(retiring) {
            return;
        }
        let ready_bytes = result.as_ref().map_or(0, |chunk| chunk.bytes.len() as u64);
        if ready_bytes != 0 {
            let charged = self
                .shared
                .phases()
                .ready
                .add(ready_bytes, "worker completion bytes");
            charged.expect("validated ready bound");
        }
        let result = match result {
            Ok(chunk) => ShWorkerResult::Prepared(chunk),
            Err(error) => ShWorkerResult::Failed(error),
        };
        self.complete(result, ready_bytes);
    }

    fn discard(mut self: Box<Self>) {
        self.release_decoding();
    }

    /// The decode panicked before delivering: the chunk takes the failed
    /// chunk path. Never panics, so the chunk always completes and the
    /// controller frees its permit: an unbalanced decoding charge is logged
    /// instead.
    fn panicked(mut self: Box<Self>, retiring: &AtomicBool) {
        let cluster_id = self.request.cluster_id;
        if let Err(error) = self.try_release_decoding() {
            log::error!(
                "[SH streaming] cluster {cluster_id}: decoding charge not returned after its \
                 decode panicked: {error}"
            );
        }
        if self.stopped(retiring) {
            return;
        }
        self.complete(
            ShWorkerResult::Failed(PrlLoadError::SectionValidation {
                section: StreamResource::Sh.label(),
                message: format!("cluster {cluster_id} decode panicked"),
            }),
            0,
        );
    }
}

/// Returns any decoding charge still held by a job dropped without its
/// cleanup having run. Never panics, since it may run while unwinding.
impl Drop for ShDecodeJob {
    fn drop(&mut self) {
        let _ = self.try_release_decoding();
    }
}
