//! Session-owned SH worker set: SH's route on the shared read issuer, its
//! decode jobs on the level's shared decode pool, and its completions.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency".

mod decode_job;
#[cfg(test)]
mod issuer_trace_tests;
#[cfg(test)]
mod ledger_tests;
mod route;
mod stats;
#[cfg(test)]
mod tests;

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_level_loader::{PrlLoadError, ShStreamManifest};

use crate::sh_streaming::budget::CpuPhaseLedger;
use crate::sh_streaming::controller::{MAX_STREAM_PERMITS, ShClusterRequest};
use crate::streaming::decode_pool::DecodeQueue;
use crate::streaming::drain_budget::DrainClass;
use crate::streaming::issuer::{ReadIssuer, ReadRoute};
use crate::streaming::target_set::TargetSet;

pub(crate) use stats::ShWorkerStats;

/// Everything the SH route and decode pool need from a level. Production
/// reads the retained manifest; tests inject sources that record offsets and
/// threads.
pub(in crate::session) trait ShWorkerSource: Send + Sync {
    fn cluster_count(&self) -> u32;
    /// Absolute file range of one chunk; empty for a canonical empty cluster.
    fn chunk_file_range(&self, cluster_id: u32) -> Result<Range<u64>, PrlLoadError>;
    /// A chunk's decoded length: its charge from the moment it is queued for
    /// decode until the frame takes its completion.
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
        self.manifest.decoded_chunk_len(cluster_id)
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

/// State shared by the frame thread, SH's issuer route, and its decode jobs.
/// Locks are held only for counter updates and handle clones, never across a
/// read, a decode, or a send, and nothing panics while holding one. A
/// poisoned lock is recovered: every update under it is a single counter
/// change, so its data is whole.
struct WorkerShared {
    /// Stops SH's queued and running decode jobs; the issuer has its own
    /// flag.
    cancel: AtomicBool,
    phases: Mutex<CpuPhaseLedger>,
    stats: Mutex<ShWorkerStats>,
    /// Targets with their live classes: the issuer cancels by membership,
    /// and a pending read or queued decode takes its tier and rank from its
    /// cluster's class.
    targets: TargetSet,
    /// The level's decode pool, cloned by the route per job. Cancel takes
    /// it, so SH stops submitting even while a level-scope issuer shared
    /// with another resource outlives SH.
    decode: Mutex<Option<DecodeQueue>>,
}

impl WorkerShared {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    fn decode_queue(&self) -> Option<DecodeQueue> {
        self.decode
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn phases(&self) -> MutexGuard<'_, CpuPhaseLedger> {
        self.phases.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn stats(&self) -> MutexGuard<'_, ShWorkerStats> {
        self.stats.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Debug)]
pub(super) enum ShWorkerResult {
    Prepared(DecodedClusterShPayload),
    Failed(PrlLoadError),
    /// The cluster left the target set before its read or its decode;
    /// nothing was decoded.
    Cancelled,
}

pub(super) struct ShWorkerCompletion {
    pub(super) request: ShClusterRequest,
    pub(super) result: ShWorkerResult,
    pub(super) ready_bytes: u64,
}

pub(super) struct ShAsyncWorkers {
    issuer: Option<ReadIssuer>,
    /// A test worker set's own decode pool; production decodes on the
    /// level's.
    #[cfg(test)]
    own_decode: Option<crate::streaming::decode_pool::DecodePool>,
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
    /// A worker set with its own SH-only issuer and decode pool. The
    /// level-scope owner uses [`Self::prepare`] instead, so SH shares one
    /// issuer and one decode pool with lightmap blocks.
    #[cfg(test)]
    pub(super) fn new(manifest: Arc<ShStreamManifest>) -> std::io::Result<Self> {
        let available = crate::parallelism::available_parallelism();
        let mut workers = Self::with_source(
            Arc::new(ManifestWorkerSource {
                manifest: manifest.clone(),
            }),
            crate::streaming::decode_pool::decode_pool_size(available),
        )?;
        workers.retained_manifest = Some(manifest);
        Ok(workers)
    }

    /// SH's issuer route over the level's decode pool, before any issuer
    /// exists. The level-scope owner spawns one issuer over this route and
    /// the other resources' routes, then hands a handle back through
    /// [`Self::attach_issuer`].
    pub(in crate::session) fn prepare(
        manifest: Arc<ShStreamManifest>,
        decode: DecodeQueue,
    ) -> (Self, Box<dyn ReadRoute>) {
        let (mut workers, route) = Self::prepare_with_source(
            Arc::new(ManifestWorkerSource {
                manifest: manifest.clone(),
            }),
            decode,
        );
        workers.retained_manifest = Some(manifest);
        (workers, Box::new(route))
    }

    /// A worker set with its own SH-only issuer and a `decode_threads` pool,
    /// the shape SH had before the issuer moved to level scope. Tests drive
    /// SH's route through it.
    #[cfg(test)]
    fn with_source(
        source: Arc<dyn ShWorkerSource>,
        decode_threads: usize,
    ) -> std::io::Result<Self> {
        let pool =
            crate::streaming::decode_pool::DecodePool::spawn(decode_threads, MAX_STREAM_PERMITS)?;
        let (mut manager, route) = Self::prepare_with_source(source, pool.queue());
        manager.own_decode = Some(pool);
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

    /// SH's worker state and route, submitting decodes through `decode`.
    pub(in crate::session) fn prepare_with_source(
        source: Arc<dyn ShWorkerSource>,
        decode: DecodeQueue,
    ) -> (Self, route::ShReadRoute) {
        // Controller permits bound every outstanding request, so the
        // completion channel cannot fill and block a sender.
        let (completed_sender, completed) = sync_channel(MAX_STREAM_PERMITS);
        let manager = Self {
            issuer: None,
            #[cfg(test)]
            own_decode: None,
            source: Arc::clone(&source),
            unresolved: Some(completed_sender.clone()),
            completed,
            shared: Arc::new(WorkerShared {
                cancel: AtomicBool::new(false),
                phases: Mutex::new(CpuPhaseLedger::default()),
                stats: Mutex::new(ShWorkerStats::default()),
                targets: TargetSet::new(source.cluster_count()),
                decode: Mutex::new(Some(decode)),
            }),
            handles: Vec::new(),
            retained_manifest: None,
        };
        let route = route::ShReadRoute {
            source,
            shared: Arc::clone(&manager.shared),
            completed: completed_sender,
        };
        (manager, route)
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

    /// Publishes the controller's current targets and their classes, for
    /// pre-read cancellation and for the tier and rank of each pending read
    /// and queued decode. Call after each target update, before new
    /// submissions.
    pub(super) fn publish_targets(&self, targets: impl IntoIterator<Item = (u32, DrainClass)>) {
        self.shared.targets.publish(targets);
    }

    /// Publishes membership only: each pending read and queued decode keeps
    /// the class it was requested at.
    #[cfg(test)]
    pub(super) fn publish_target_keys(&self, targets: &std::collections::BTreeSet<u32>) {
        self.shared.targets.publish_keys(targets);
    }

    pub(super) fn try_completion(&self) -> Result<Option<ShWorkerCompletion>, &'static str> {
        match self.completed.try_recv() {
            Ok(completion) => {
                if completion.ready_bytes != 0 {
                    self.shared
                        .phases()
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

    /// Makes `forward` the queue completions are taken from, and returns the
    /// queue the workers deliver into: a measurement gate sits between them.
    #[cfg(test)]
    pub(super) fn interpose_completions(
        &mut self,
        forward: Receiver<ShWorkerCompletion>,
    ) -> Receiver<ShWorkerCompletion> {
        std::mem::replace(&mut self.completed, forward)
    }

    #[cfg(feature = "capture")]
    pub(super) fn phase_snapshot(&self) -> CpuPhaseLedger {
        *self.shared.phases()
    }

    pub(super) fn stats(&self) -> ShWorkerStats {
        *self.shared.stats()
    }

    pub(super) fn stop(&mut self) {
        self.cancel_threads();
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
        #[cfg(test)]
        drop(self.own_decode.take());
    }

    /// Cancels this generation without waiting on an in-flight positional
    /// read or decode. The frame path polls the returned handles and joins
    /// only after they have finished; process teardown still joins through
    /// `Drop`.
    pub(super) fn begin_retirement(&mut self) -> ShWorkerRetirement {
        self.cancel_threads();
        #[allow(unused_mut, reason = "only a test worker set owns its pool")]
        let mut handles = std::mem::take(&mut self.handles);
        #[cfg(test)]
        if let Some(pool) = self.own_decode.take() {
            handles.extend(pool.begin_retirement());
        }
        ShWorkerRetirement {
            handles,
            retained_manifest: self.retained_manifest.take(),
        }
    }
}

impl ShAsyncWorkers {
    /// Cancels the issuer handle and SH's decode jobs and drops every
    /// frame-side sender, so SH submits nothing more and the completion
    /// queue disconnects once the shared threads let go of it.
    fn cancel_threads(&mut self) {
        self.shared.cancel.store(true, Ordering::Release);
        if let Some(issuer) = self.issuer.take() {
            issuer.cancel();
        }
        self.unresolved.take();
        self.shared
            .decode
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }
}

#[derive(Debug)]
pub(crate) struct ShWorkerRetirement {
    handles: Vec<JoinHandle<()>>,
    /// Keeps the opened PRL alive until every handle has joined. Thread
    /// safety does not rest on it: each route and decode job holds its own
    /// `Arc` of the source it reads and decodes through.
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
