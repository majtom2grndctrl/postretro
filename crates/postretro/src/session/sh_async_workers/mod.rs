//! Session-owned SH worker set: one ordered read issuer thread plus a small
//! decode pool. No read or decode runs on the frame thread; a completion
//! remains covered by its controller permit.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency".

mod decode_pool;
mod issuer;
mod schedule;
mod stats;
mod target_bitset;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_level_loader::{PrlLoadError, ShStreamManifest};

use crate::sh_streaming::budget::CpuPhaseLedger;
use crate::sh_streaming::controller::{MAX_STREAM_PERMITS, ShClusterRequest};

pub(crate) use stats::ShWorkerStats;
use target_bitset::ShTargetBitset;

/// Everything the worker threads need from a level. Production reads the
/// retained manifest; tests inject sources that record offsets and threads.
trait ShWorkerSource: Send + Sync {
    fn cluster_count(&self) -> u32;
    /// Absolute file range of one chunk; empty for a canonical empty cluster.
    fn chunk_file_range(&self, cluster_id: u32) -> Result<Range<u64>, PrlLoadError>;
    fn decoded_bytes(&self, cluster_id: u32) -> u64;
    /// One physical positional read. Must return exactly the span's length.
    fn read_file_span(&self, range: Range<u64>) -> Result<Vec<u8>, PrlLoadError>;
    fn decode(
        &self,
        cluster_id: u32,
        bytes: Vec<u8>,
    ) -> Result<DecodedClusterShPayload, PrlLoadError>;
}

struct ManifestWorkerSource {
    manifest: Arc<ShStreamManifest>,
}

impl ShWorkerSource for ManifestWorkerSource {
    fn cluster_count(&self) -> u32 {
        self.manifest.cluster_count()
    }

    fn chunk_file_range(&self, cluster_id: u32) -> Result<Range<u64>, PrlLoadError> {
        self.manifest.chunk_file_range(cluster_id)
    }

    fn decoded_bytes(&self, cluster_id: u32) -> u64 {
        self.manifest
            .payloads()
            .index
            .get(cluster_id as usize)
            .map_or(0, |index| index.decoded_bytes)
    }

    fn read_file_span(&self, range: Range<u64>) -> Result<Vec<u8>, PrlLoadError> {
        self.manifest.read_file_span(range)
    }

    fn decode(
        &self,
        cluster_id: u32,
        bytes: Vec<u8>,
    ) -> Result<DecodedClusterShPayload, PrlLoadError> {
        self.manifest.decode_encoded_cluster(cluster_id, bytes)
    }
}

/// Decode threads: half the available cores, at least one, at most three.
fn decode_pool_size(available_parallelism: usize) -> usize {
    (available_parallelism / 2).clamp(1, 3)
}

/// State shared by the frame thread, the issuer, and the decode pool. Locks
/// are held only for counter updates, never across a read or a decode.
struct WorkerShared {
    cancel: AtomicBool,
    phases: Mutex<CpuPhaseLedger>,
    stats: Mutex<ShWorkerStats>,
    targets: ShTargetBitset,
}

impl WorkerShared {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
}

/// A controller request stamped when the frame thread handed it over, so read
/// latency covers queueing behind earlier reads.
struct SubmittedRequest {
    request: ShClusterRequest,
    submitted_at: Instant,
}

/// Encoded bytes in hand, queued for a pool thread. Its bytes stay charged to
/// the encoded phase until decode starts.
struct DecodeJob {
    request: ShClusterRequest,
    bytes: Vec<u8>,
}

#[derive(Debug)]
pub(super) enum ShWorkerResult {
    Prepared(DecodedClusterShPayload),
    Failed(PrlLoadError),
    /// The cluster left the target set before its read; nothing was read.
    Cancelled,
}

pub(super) struct ShWorkerCompletion {
    pub(super) request: ShClusterRequest,
    pub(super) result: ShWorkerResult,
    pub(super) ready_bytes: u64,
}

pub(super) struct ShAsyncWorkers {
    requests: Option<SyncSender<SubmittedRequest>>,
    completed: Receiver<ShWorkerCompletion>,
    shared: Arc<WorkerShared>,
    handles: Vec<JoinHandle<()>>,
    retained_manifest: Option<Arc<ShStreamManifest>>,
}

impl std::fmt::Debug for ShAsyncWorkers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShAsyncWorkers")
            .field("thread_count", &self.handles.len())
            .finish_non_exhaustive()
    }
}

impl ShAsyncWorkers {
    pub(super) fn new(manifest: Arc<ShStreamManifest>) -> std::io::Result<Self> {
        let available = thread::available_parallelism().map_or(1, std::num::NonZero::get);
        let mut workers = Self::with_source(
            Arc::new(ManifestWorkerSource {
                manifest: manifest.clone(),
            }),
            decode_pool_size(available),
        )?;
        workers.retained_manifest = Some(manifest);
        Ok(workers)
    }

    fn with_source(
        source: Arc<dyn ShWorkerSource>,
        decode_threads: usize,
    ) -> std::io::Result<Self> {
        // Controller permits bound every outstanding request, so none of these
        // channels can fill and block a sender.
        let (requests, request_receiver) = sync_channel(MAX_STREAM_PERMITS);
        let (completed_sender, completed) = sync_channel(MAX_STREAM_PERMITS);
        let mut manager = Self {
            requests: Some(requests),
            completed,
            shared: Arc::new(WorkerShared {
                cancel: AtomicBool::new(false),
                phases: Mutex::new(CpuPhaseLedger::default()),
                stats: Mutex::new(ShWorkerStats::default()),
                targets: ShTargetBitset::new(source.cluster_count()),
            }),
            handles: Vec::with_capacity(decode_threads + 1),
            retained_manifest: None,
        };
        manager.spawn_threads(source, decode_threads, request_receiver, completed_sender)?;
        Ok(manager)
    }

    /// On a spawn failure the decode sender drops here, before the caller
    /// drops the manager, so already spawned pool threads can exit and join.
    fn spawn_threads(
        &mut self,
        source: Arc<dyn ShWorkerSource>,
        decode_threads: usize,
        request_receiver: Receiver<SubmittedRequest>,
        completed_sender: SyncSender<ShWorkerCompletion>,
    ) -> std::io::Result<()> {
        let (decode_sender, decode_jobs) = sync_channel(MAX_STREAM_PERMITS);
        let decode_jobs = Arc::new(Mutex::new(decode_jobs));
        for index in 0..decode_threads.max(1) {
            let source = Arc::clone(&source);
            let jobs = Arc::clone(&decode_jobs);
            let completed = completed_sender.clone();
            let shared = Arc::clone(&self.shared);
            let handle = thread::Builder::new()
                .name(format!("sh-probe-decode-{index}"))
                .spawn(move || decode_pool::decode_loop(&jobs, &completed, &shared, &*source))?;
            self.handles.push(handle);
        }
        let shared = Arc::clone(&self.shared);
        let channels = issuer::IssuerChannels {
            requests: request_receiver,
            decode: decode_sender,
            completed: completed_sender,
        };
        let handle = thread::Builder::new()
            .name("sh-probe-read".into())
            .spawn(move || issuer::issuer_loop(channels, &shared, &*source))?;
        self.handles.push(handle);
        Ok(())
    }

    pub(super) fn submit(&self, request: ShClusterRequest) -> Result<(), &'static str> {
        self.requests
            .as_ref()
            .ok_or("worker manager stopped")?
            .try_send(SubmittedRequest {
                request,
                submitted_at: Instant::now(),
            })
            .map_err(|_| "SH worker request queue full or disconnected")
    }

    /// Publishes the controller's current target set for pre-read
    /// cancellation. Call after each target update, before new submissions.
    pub(super) fn publish_targets(&self, targets: &BTreeSet<u32>) {
        self.shared.targets.publish(targets);
    }

    pub(super) fn try_completion(&self) -> Result<Option<ShWorkerCompletion>, &'static str> {
        match self.completed.try_recv() {
            Ok(completion) => {
                if completion.ready_bytes != 0 {
                    self.shared
                        .phases
                        .lock()
                        .map_err(|_| "SH worker phase ledger poisoned")?
                        .ready
                        .remove(completion.ready_bytes, "worker completion bytes")
                        .map_err(|_| "SH worker completion byte accounting underflow")?;
                }
                Ok(Some(completion))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err("SH worker completion queue disconnected"),
        }
    }

    #[cfg(feature = "capture")]
    pub(super) fn phase_snapshot(&self) -> Result<CpuPhaseLedger, &'static str> {
        self.shared
            .phases
            .lock()
            .map(|phases| *phases)
            .map_err(|_| "SH worker phase ledger poisoned")
    }

    pub(super) fn stats(&self) -> Result<ShWorkerStats, &'static str> {
        self.shared
            .stats
            .lock()
            .map(|stats| *stats)
            .map_err(|_| "SH worker stats poisoned")
    }

    pub(super) fn stop(&mut self) {
        self.shared.cancel.store(true, Ordering::Release);
        self.requests.take();
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }

    /// Cancels this generation without waiting on an in-flight positional
    /// read. The frame path polls the returned handles and joins only after
    /// they have finished; process teardown still joins through `Drop`.
    pub(super) fn begin_retirement(&mut self) -> ShWorkerRetirement {
        self.shared.cancel.store(true, Ordering::Release);
        self.requests.take();
        ShWorkerRetirement {
            handles: std::mem::take(&mut self.handles),
            retained_manifest: self.retained_manifest.take(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ShWorkerRetirement {
    handles: Vec<JoinHandle<()>>,
    /// Keeps the exact opened PRL alive until every handle has joined.
    retained_manifest: Option<Arc<ShStreamManifest>>,
}

impl ShWorkerRetirement {
    pub(super) fn try_finish(&mut self) -> bool {
        if !self.handles.iter().all(JoinHandle::is_finished) {
            return false;
        }
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
        drop(self.retained_manifest.take());
        true
    }
}

impl Drop for ShWorkerRetirement {
    fn drop(&mut self) {
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
        drop(self.retained_manifest.take());
    }
}

impl Drop for ShAsyncWorkers {
    fn drop(&mut self) {
        self.stop();
    }
}
