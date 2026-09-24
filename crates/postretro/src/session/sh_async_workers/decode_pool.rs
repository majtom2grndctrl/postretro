//! Decode pool threads: verify and decode encoded chunks the issuer read,
//! then publish completions to the session.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency".

use std::sync::Mutex;
use std::sync::mpsc::{Receiver, SyncSender};
use std::time::Instant;

use super::{DecodeJob, ShWorkerCompletion, ShWorkerResult, ShWorkerSource, WorkerShared};

/// Runs until cancellation or until the issuer drops the job sender.
pub(super) fn decode_loop(
    jobs: &Mutex<Receiver<DecodeJob>>,
    completed: &SyncSender<ShWorkerCompletion>,
    shared: &WorkerShared,
    source: &dyn ShWorkerSource,
) {
    loop {
        let job = match jobs.lock().expect("SH decode queue mutex poisoned").recv() {
            Ok(job) if !shared.cancelled() => job,
            _ => return,
        };
        let cluster_id = job.request.cluster_id;
        let encoded_bytes = job.bytes.len() as u64;
        let decoded_bytes = source.decoded_bytes(cluster_id);
        {
            let mut phases = shared.phases.lock().expect("SH phase mutex poisoned");
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
        let result = source.decode(cluster_id, job.bytes);
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
        if shared.cancelled() {
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
        if completed
            .send(ShWorkerCompletion {
                request: job.request,
                result,
                ready_bytes,
            })
            .is_err()
        {
            return;
        }
    }
}
