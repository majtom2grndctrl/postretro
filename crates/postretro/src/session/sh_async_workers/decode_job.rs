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

/// Encoded bytes in hand, queued on the level's decode pool. Its bytes stay
/// charged to the encoded phase until decode starts.
pub(super) struct ShDecodeJob {
    pub(super) request: ShClusterRequest,
    pub(super) bytes: Vec<u8>,
    pub(super) source: Arc<dyn ShWorkerSource>,
    pub(super) shared: Arc<WorkerShared>,
    pub(super) completed: SyncSender<ShWorkerCompletion>,
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
        let cluster_id = self.request.cluster_id;
        let encoded_bytes = self.bytes.len() as u64;
        let decoded_bytes = self.source.decoded_bytes(cluster_id);
        {
            let mut phases = self.shared.phases.lock().expect("SH phase mutex poisoned");
            phases
                .encoded
                .remove(encoded_bytes, "encoded worker bytes")
                .expect("balanced encoded read");
            phases
                .decoding
                .add(decoded_bytes, "decoded worker bytes")
                .expect("validated decode bound");
        }
        let started = Instant::now();
        let ShDecodeJob {
            request,
            bytes,
            source,
            shared,
            completed,
        } = *self;
        let result = source.decode(cluster_id, bytes);
        let elapsed = started.elapsed();
        shared
            .stats
            .lock()
            .expect("SH stats mutex poisoned")
            .decode_latency
            .record(elapsed);
        shared
            .phases
            .lock()
            .expect("SH phase mutex poisoned")
            .decoding
            .remove(decoded_bytes, "decoded worker bytes")
            .expect("balanced decode");
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
        self.shared
            .phases
            .lock()
            .expect("SH phase mutex poisoned")
            .encoded
            .remove(self.bytes.len() as u64, "encoded worker bytes")
            .expect("balanced encoded read");
    }
}
