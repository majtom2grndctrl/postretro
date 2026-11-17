//! SH's decode job on the level's decode pool: verify and decode one encoded
//! chunk the issuer read, then publish its completion to the session.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency".

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::time::Instant;

use super::{ShWorkerCompletion, ShWorkerResult, ShWorkerSource, WorkerShared};
use crate::sh_streaming::controller::ShClusterRequest;
use crate::streaming::decode_pool::DecodeJob;
use crate::streaming::request::ReadTier;

/// A read chunk's stored bytes, queued on the level's decode pool. It is
/// charged to the decoding phase at its decoded size from the moment it is
/// queued; the job moves that charge to the ready phase when it delivers, and
/// releases it when it fails, is discarded, or finds the pool retiring.
pub(super) struct ShDecodeJob {
    pub(super) request: ShClusterRequest,
    pub(super) bytes: Vec<u8>,
    pub(super) decoded_bytes: u64,
    pub(super) source: Arc<dyn ShWorkerSource>,
    pub(super) shared: Arc<WorkerShared>,
    pub(super) completed: SyncSender<ShWorkerCompletion>,
}

impl ShDecodeJob {
    fn release_decoding(shared: &WorkerShared, decoded_bytes: u64) {
        shared
            .phases
            .lock()
            .expect("SH phase mutex poisoned")
            .decoding
            .remove(decoded_bytes, "decoded worker bytes")
            .expect("balanced decode");
    }
}

impl DecodeJob for ShDecodeJob {
    fn tier(&self) -> ReadTier {
        if self.request.mandatory {
            ReadTier::Mandatory
        } else {
            ReadTier::Optional
        }
    }

    fn run(self: Box<Self>, retiring: &AtomicBool) {
        let stopped = || self.shared.cancelled() || retiring.load(Ordering::Acquire);
        if stopped() {
            return self.discard();
        }
        let started = Instant::now();
        let ShDecodeJob {
            request,
            bytes,
            decoded_bytes,
            source,
            shared,
            completed,
        } = *self;
        let result = source.decode(request.cluster_id, bytes);
        let elapsed = started.elapsed();
        shared
            .stats
            .lock()
            .expect("SH stats mutex poisoned")
            .decode_latency
            .record(elapsed);
        Self::release_decoding(&shared, decoded_bytes);
        if shared.cancelled() || retiring.load(Ordering::Acquire) {
            return;
        }
        let ready_bytes = result.as_ref().map_or(0, |chunk| chunk.bytes.len() as u64);
        if ready_bytes != 0 {
            shared
                .phases
                .lock()
                .expect("SH phase mutex poisoned")
                .ready
                .add(ready_bytes, "worker completion bytes")
                .expect("validated ready bound");
        }
        let result = match result {
            Ok(chunk) => ShWorkerResult::Prepared(chunk),
            Err(error) => ShWorkerResult::Failed(error),
        };
        // A gone session drops the completion; its ledger goes with it.
        let _ = completed.send(ShWorkerCompletion {
            request,
            result,
            ready_bytes,
        });
    }

    fn discard(self: Box<Self>) {
        Self::release_decoding(&self.shared, self.decoded_bytes);
    }
}
