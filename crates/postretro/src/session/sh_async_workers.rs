//! Four session-owned positional SH readers. No read or decode runs on the
//! renderer thread; a completion remains covered by its controller permit.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use postretro_level_format::cluster_sh_payloads::DecodedClusterShPayload;
use postretro_level_loader::{PrlLoadError, ShStreamManifest};

use crate::sh_streaming::budget::CpuPhaseLedger;
use crate::sh_streaming::controller::{MAX_STREAM_PERMITS, ShClusterRequest};

trait ShWorkerSource: Send + Sync {
    fn byte_counts(&self, cluster_id: u32) -> (u64, u64);
    fn read_encoded(&self, cluster_id: u32) -> Result<Vec<u8>, PrlLoadError>;
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
    fn byte_counts(&self, cluster_id: u32) -> (u64, u64) {
        let index = &self.manifest.payloads().index[cluster_id as usize];
        (index.payload_len, index.decoded_bytes)
    }

    fn read_encoded(&self, cluster_id: u32) -> Result<Vec<u8>, PrlLoadError> {
        self.manifest.read_encoded_cluster(cluster_id)
    }

    fn decode(
        &self,
        cluster_id: u32,
        bytes: Vec<u8>,
    ) -> Result<DecodedClusterShPayload, PrlLoadError> {
        self.manifest.decode_encoded_cluster(cluster_id, bytes)
    }
}

pub(super) struct ShWorkerCompletion {
    pub(super) request: ShClusterRequest,
    pub(super) result: Result<DecodedClusterShPayload, PrlLoadError>,
    pub(super) ready_bytes: u64,
}

pub(super) struct ShAsyncWorkers {
    requests: Option<SyncSender<ShClusterRequest>>,
    completed: Receiver<ShWorkerCompletion>,
    cancel: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
    phases: Arc<Mutex<CpuPhaseLedger>>,
    retained_manifest: Option<Arc<ShStreamManifest>>,
}

impl std::fmt::Debug for ShAsyncWorkers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShAsyncWorkers")
            .field("worker_count", &self.handles.len())
            .finish_non_exhaustive()
    }
}

impl ShAsyncWorkers {
    pub(super) fn new(manifest: Arc<ShStreamManifest>) -> std::io::Result<Self> {
        let mut workers = Self::with_source(Arc::new(ManifestWorkerSource {
            manifest: manifest.clone(),
        }))?;
        workers.retained_manifest = Some(manifest);
        Ok(workers)
    }

    fn with_source(source: Arc<dyn ShWorkerSource>) -> std::io::Result<Self> {
        let (requests, pending) = sync_channel(MAX_STREAM_PERMITS);
        let (completed_sender, completed) = sync_channel(MAX_STREAM_PERMITS);
        let pending = Arc::new(Mutex::new(pending));
        let cancel = Arc::new(AtomicBool::new(false));
        let phases = Arc::new(Mutex::new(CpuPhaseLedger::default()));
        let mut manager = Self {
            requests: Some(requests),
            completed,
            cancel,
            handles: Vec::with_capacity(MAX_STREAM_PERMITS),
            phases,
            retained_manifest: None,
        };
        for worker_id in 0..MAX_STREAM_PERMITS {
            let source = Arc::clone(&source);
            let pending = Arc::clone(&pending);
            let sender = completed_sender.clone();
            let cancel = Arc::clone(&manager.cancel);
            let phases = Arc::clone(&manager.phases);
            let handle = thread::Builder::new()
                .name(format!("sh-probe-read-{worker_id}"))
                .spawn(move || worker_loop(&pending, &sender, &cancel, &phases, &*source))?;
            manager.handles.push(handle);
        }
        Ok(manager)
    }

    pub(super) fn submit(&self, request: ShClusterRequest) -> Result<(), &'static str> {
        self.requests
            .as_ref()
            .ok_or("worker manager stopped")?
            .try_send(request)
            .map_err(|_| "SH worker request queue full or disconnected")
    }

    pub(super) fn try_completion(&self) -> Result<Option<ShWorkerCompletion>, &'static str> {
        match self.completed.try_recv() {
            Ok(completion) => {
                if completion.ready_bytes != 0 {
                    self.phases
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
        self.phases
            .lock()
            .map(|phases| *phases)
            .map_err(|_| "SH worker phase ledger poisoned")
    }

    pub(super) fn stop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        self.requests.take();
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }

    /// Cancels this generation without waiting on an in-flight positional
    /// read. The frame path polls the returned handles and joins only after
    /// they have finished; process teardown still joins through `Drop`.
    pub(super) fn begin_retirement(&mut self) -> ShWorkerRetirement {
        self.cancel.store(true, Ordering::Release);
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

fn worker_loop(
    pending: &Mutex<Receiver<ShClusterRequest>>,
    completed: &SyncSender<ShWorkerCompletion>,
    cancel: &AtomicBool,
    phases: &Mutex<CpuPhaseLedger>,
    source: &dyn ShWorkerSource,
) {
    loop {
        let request = match pending.lock().expect("SH request mutex poisoned").recv() {
            Ok(request) if !cancel.load(Ordering::Acquire) => request,
            _ => return,
        };
        let (encoded_bytes, decoded_bytes) = source.byte_counts(request.cluster_id);
        {
            let mut phases = phases.lock().expect("SH phase mutex poisoned");
            phases
                .encoded
                .add(encoded_bytes, "encoded worker bytes")
                .expect("validated encoded bound");
        }
        let encoded = source.read_encoded(request.cluster_id);
        {
            let mut phases = phases.lock().expect("SH phase mutex poisoned");
            phases
                .encoded
                .remove(encoded_bytes, "encoded worker bytes")
                .expect("balanced encoded read");
        }
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let result = encoded.and_then(|bytes| {
            {
                let mut phases = phases.lock().expect("SH phase mutex poisoned");
                phases
                    .decoding
                    .add(decoded_bytes, "decoded worker bytes")
                    .expect("validated decode bound");
            }
            let result = source.decode(request.cluster_id, bytes);
            {
                let mut phases = phases.lock().expect("SH phase mutex poisoned");
                phases
                    .decoding
                    .remove(decoded_bytes, "decoded worker bytes")
                    .expect("balanced decode");
            }
            result
        });
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let ready_bytes = result.as_ref().map_or(0, |chunk| chunk.bytes.len() as u64);
        if ready_bytes != 0 {
            phases
                .lock()
                .expect("SH phase mutex poisoned")
                .ready
                .add(ready_bytes, "worker completion bytes")
                .expect("validated ready bound");
        }
        if completed
            .send(ShWorkerCompletion {
                request,
                result,
                ready_bytes,
            })
            .is_err()
        {
            return;
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::FileExt;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    struct DelayedPositionalSource {
        file: Arc<std::fs::File>,
        delay: Duration,
        executions: Arc<AtomicUsize>,
    }

    impl ShWorkerSource for DelayedPositionalSource {
        fn byte_counts(&self, _cluster_id: u32) -> (u64, u64) {
            (7, 7)
        }

        fn read_encoded(&self, _cluster_id: u32) -> Result<Vec<u8>, PrlLoadError> {
            self.executions.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
            let mut bytes = vec![0; 7];
            self.file.read_at(&mut bytes, 4).unwrap();
            Ok(bytes)
        }

        fn decode(
            &self,
            cluster_id: u32,
            bytes: Vec<u8>,
        ) -> Result<DecodedClusterShPayload, PrlLoadError> {
            Ok(DecodedClusterShPayload {
                cluster_id,
                bytes,
                blocks: Vec::new(),
            })
        }
    }

    fn delayed_source(
        delay: Duration,
        executions: Arc<AtomicUsize>,
    ) -> (tempfile::TempDir, Arc<dyn ShWorkerSource>) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("encoded.bin");
        std::fs::write(&path, b"headpayloadtail").unwrap();
        let source = DelayedPositionalSource {
            file: Arc::new(std::fs::File::open(path).unwrap()),
            delay,
            executions,
        };
        (temp, Arc::new(source))
    }

    #[test]
    fn delayed_positional_reader_never_blocks_completion_drain_or_submission() {
        let executions = Arc::new(AtomicUsize::new(0));
        let (_temp, source) = delayed_source(Duration::from_millis(250), Arc::clone(&executions));
        let mut workers = ShAsyncWorkers::with_source(source).unwrap();
        let request = ShClusterRequest {
            generation: 17,
            content_tag: [4; 32],
            cluster_id: 0,
            chunk_hash: [7; 32],
            mandatory: true,
        };
        let start = Instant::now();
        workers.submit(request).unwrap();
        assert!(workers.try_completion().unwrap().is_none());
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "frame-side submission/drain waited for I/O"
        );
        std::thread::sleep(Duration::from_millis(300));
        let completion = workers
            .try_completion()
            .unwrap()
            .expect("delayed positional completion");
        assert_eq!(completion.request, request);
        assert_eq!(completion.result.unwrap().bytes, b"payload");
        assert_eq!(executions.load(Ordering::SeqCst), 1);
        workers.stop();
    }

    #[test]
    fn delayed_reload_retires_without_blocking_and_cannot_deliver_stale_generation() {
        let executions = Arc::new(AtomicUsize::new(0));
        let (_old_temp, old_source) =
            delayed_source(Duration::from_millis(250), Arc::clone(&executions));
        let mut workers = ShAsyncWorkers::with_source(old_source).unwrap();
        assert_eq!(workers.handles.len(), MAX_STREAM_PERMITS);
        let old_request = |cluster_id| ShClusterRequest {
            generation: 1,
            content_tag: [1; 32],
            cluster_id,
            chunk_hash: [1; 32],
            mandatory: true,
        };
        for cluster_id in 0..MAX_STREAM_PERMITS as u32 {
            workers.submit(old_request(cluster_id)).unwrap();
        }
        while executions.load(Ordering::SeqCst) < MAX_STREAM_PERMITS {
            std::thread::yield_now();
        }

        let start = Instant::now();
        let mut retirement = workers.begin_retirement();
        assert!(start.elapsed() < Duration::from_millis(100));
        assert!(workers.handles.is_empty());
        assert!(
            !retirement.try_finish(),
            "delayed positional read is still live"
        );

        std::thread::sleep(Duration::from_millis(300));
        assert!(retirement.try_finish());
        assert!(retirement.handles.is_empty());
        assert!(
            workers.try_completion().is_err(),
            "cancelled generation must not publish a ready completion"
        );

        // Only after all four old worker handles have joined may the next
        // generation start, so lifecycle permits never overlap generations.
        let (_new_temp, new_source) = delayed_source(Duration::ZERO, Arc::new(AtomicUsize::new(0)));
        let mut replacement = ShAsyncWorkers::with_source(new_source).unwrap();
        let new_request = ShClusterRequest {
            generation: 2,
            content_tag: [2; 32],
            cluster_id: 0,
            chunk_hash: [2; 32],
            mandatory: true,
        };
        replacement.submit(new_request).unwrap();
        let completion = loop {
            if let Some(completion) = replacement.try_completion().unwrap() {
                break completion;
            }
            std::thread::yield_now();
        };
        assert_eq!(completion.request, new_request);
        assert_ne!(completion.request.generation, old_request(0).generation);
        replacement.stop();
    }
}
