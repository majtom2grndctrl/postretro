//! Session-owned SH worker set: the shared read issuer with SH's route, plus
//! a small decode pool. See: context/lib/rendering_pipeline.md §"Cluster SH residency".

mod decode_pool;
#[cfg(test)]
mod issuer_trace_tests;
mod route;
mod stats;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_level_loader::{PrlLoadError, ShStreamManifest};

use crate::sh_streaming::budget::CpuPhaseLedger;
use crate::sh_streaming::controller::{MAX_STREAM_PERMITS, ShClusterRequest};
use crate::streaming::issuer::{ReadIssuer, ReadRoute};
use crate::streaming::target_bitset::TargetBitset;

pub(crate) use stats::ShWorkerStats;

/// Everything the SH route and decode pool need from a level. Production
/// reads the retained manifest; tests inject sources that record offsets and
/// threads.
pub(in crate::session) trait ShWorkerSource: Send + Sync {
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

/// State shared by the frame thread, SH's issuer route, and the decode pool.
/// Locks are held only for counter updates and sender clones, never across a
/// read, a decode, or a send.
struct WorkerShared {
    /// Stops the decode pool; the issuer has its own flag.
    cancel: AtomicBool,
    phases: Mutex<CpuPhaseLedger>,
    stats: Mutex<ShWorkerStats>,
    targets: TargetBitset,
    /// The decode queue's one sender, cloned by the route per job. Cancel
    /// takes it, so the pool's queue disconnects even while a level-scope
    /// issuer shared with another resource outlives SH.
    decode: Mutex<Option<SyncSender<DecodeJob>>>,
}

impl WorkerShared {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    fn decode_sender(&self) -> Option<SyncSender<DecodeJob>> {
        self.decode
            .lock()
            .expect("SH decode sender mutex poisoned")
            .clone()
    }
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
    issuer: Option<ReadIssuer>,
    /// Resolves a request's chunk range at submission, on the frame thread.
    source: Arc<dyn ShWorkerSource>,
    /// Completes a request whose range cannot be resolved, without a read.
    unresolved: Option<SyncSender<ShWorkerCompletion>>,
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
    /// A worker set with its own SH-only issuer. The level-scope owner uses
    /// [`Self::prepare`] instead, so SH shares one issuer with lightmap blocks.
    #[cfg(test)]
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

    /// The decode pool plus SH's issuer route, before any issuer exists. The
    /// level-scope owner spawns one issuer over this route and the other
    /// resources' routes, then hands a handle back through
    /// [`Self::attach_issuer`].
    pub(in crate::session) fn prepare(
        manifest: Arc<ShStreamManifest>,
    ) -> std::io::Result<(Self, Box<dyn ReadRoute>)> {
        let available = thread::available_parallelism().map_or(1, std::num::NonZero::get);
        let (mut workers, route) = Self::prepare_with_source(
            Arc::new(ManifestWorkerSource {
                manifest: manifest.clone(),
            }),
            decode_pool_size(available),
        )?;
        workers.retained_manifest = Some(manifest);
        Ok((workers, Box::new(route)))
    }

    /// A worker set with its own SH-only issuer, the shape SH had before the
    /// issuer moved to level scope. Tests drive SH's route through it.
    #[cfg(test)]
    fn with_source(
        source: Arc<dyn ShWorkerSource>,
        decode_threads: usize,
    ) -> std::io::Result<Self> {
        let (mut manager, route) = Self::prepare_with_source(source, decode_threads)?;
        let (issuer, handle) = ReadIssuer::spawn(
            crate::streaming::issuer::ReadRoutes::default().with(
                crate::streaming::request::StreamResource::Sh,
                Box::new(route),
            ),
            MAX_STREAM_PERMITS,
        )?;
        manager.issuer = Some(issuer);
        manager.handles.push(handle);
        Ok(manager)
    }

    /// On a spawn failure the caller drops the manager, whose cancel takes the
    /// decode sender, so already spawned pool threads can exit and join.
    pub(in crate::session) fn prepare_with_source(
        source: Arc<dyn ShWorkerSource>,
        decode_threads: usize,
    ) -> std::io::Result<(Self, route::ShReadRoute)> {
        // Controller permits bound every outstanding request, so none of these
        // channels can fill and block a sender.
        let (completed_sender, completed) = sync_channel(MAX_STREAM_PERMITS);
        let (decode_sender, decode_jobs) = sync_channel(MAX_STREAM_PERMITS);
        let mut manager = Self {
            issuer: None,
            source: Arc::clone(&source),
            unresolved: Some(completed_sender.clone()),
            completed,
            shared: Arc::new(WorkerShared {
                cancel: AtomicBool::new(false),
                phases: Mutex::new(CpuPhaseLedger::default()),
                stats: Mutex::new(ShWorkerStats::default()),
                targets: TargetBitset::new(source.cluster_count()),
                decode: Mutex::new(Some(decode_sender)),
            }),
            handles: Vec::with_capacity(decode_threads + 1),
            retained_manifest: None,
        };
        let decode_jobs = Arc::new(Mutex::new(decode_jobs));
        for index in 0..decode_threads.max(1) {
            let source = Arc::clone(&source);
            let jobs = Arc::clone(&decode_jobs);
            let completed = completed_sender.clone();
            let shared = Arc::clone(&manager.shared);
            let handle = thread::Builder::new()
                .name(format!("sh-probe-decode-{index}"))
                .spawn(move || decode_pool::decode_loop(&jobs, &completed, &shared, &*source))?;
            manager.handles.push(handle);
        }
        let route = route::ShReadRoute {
            source,
            shared: Arc::clone(&manager.shared),
            completed: completed_sender,
        };
        Ok((manager, route))
    }

    /// Hands the workers their submit handle on a level-scope issuer that
    /// already serves [`Self::prepare`]'s route. Its thread is the owner's to
    /// join, not these workers'.
    pub(in crate::session) fn attach_issuer(&mut self, issuer: ReadIssuer) {
        self.issuer = Some(issuer);
    }

    pub(super) fn submit(&self, request: ShClusterRequest) -> Result<(), &'static str> {
        let issuer = self.issuer.as_ref().ok_or("worker manager stopped")?;
        match self.source.chunk_file_range(request.cluster_id) {
            Ok(range) => issuer
                .submit(route::read_request(request, range))
                .map_err(|_| "SH worker request queue full or disconnected"),
            Err(error) => self
                .unresolved
                .as_ref()
                .ok_or("worker manager stopped")?
                .try_send(ShWorkerCompletion {
                    request,
                    result: ShWorkerResult::Failed(error),
                    ready_bytes: 0,
                })
                .map_err(|_| "SH worker completion queue full or disconnected"),
        }
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
        self.cancel_threads();
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }

    /// Cancels this generation without waiting on an in-flight positional
    /// read. The frame path polls the returned handles and joins only after
    /// they have finished; process teardown still joins through `Drop`.
    pub(super) fn begin_retirement(&mut self) -> ShWorkerRetirement {
        self.cancel_threads();
        ShWorkerRetirement {
            handles: std::mem::take(&mut self.handles),
            retained_manifest: self.retained_manifest.take(),
        }
    }
}

impl ShAsyncWorkers {
    /// Cancels the issuer and decode pool and drops every frame-side sender,
    /// so the threads exit and the completion queue then disconnects.
    fn cancel_threads(&mut self) {
        self.shared.cancel.store(true, Ordering::Release);
        if let Some(issuer) = self.issuer.take() {
            issuer.cancel();
        }
        self.unresolved.take();
        self.shared
            .decode
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
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
