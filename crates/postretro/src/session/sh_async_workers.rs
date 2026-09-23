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

type ChunkReader = dyn Fn(&ShStreamManifest, u32) -> Result<Vec<u8>, PrlLoadError> + Send + Sync;

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
        Self::with_reader(
            manifest,
            Arc::new(|manifest, id| manifest.read_encoded_cluster(id)),
        )
    }

    fn with_reader(
        manifest: Arc<ShStreamManifest>,
        reader: Arc<ChunkReader>,
    ) -> std::io::Result<Self> {
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
        };
        for worker_id in 0..MAX_STREAM_PERMITS {
            let manifest = Arc::clone(&manifest);
            let pending = Arc::clone(&pending);
            let sender = completed_sender.clone();
            let cancel = Arc::clone(&manager.cancel);
            let phases = Arc::clone(&manager.phases);
            let reader = Arc::clone(&reader);
            let handle = thread::Builder::new()
                .name(format!("sh-probe-read-{worker_id}"))
                .spawn(move || {
                    worker_loop(&manifest, &pending, &sender, &cancel, &phases, &*reader)
                })?;
            manager.handles.push(handle);
        }
        Ok(manager)
    }

    /// A deterministic test executor exercises the same bounded channels and
    /// teardown path without requiring a baked PRL fixture in the app crate.
    #[cfg(test)]
    fn with_test_executor(
        executor: Arc<dyn Fn(ShClusterRequest) -> ShWorkerCompletion + Send + Sync>,
    ) -> std::io::Result<Self> {
        let (requests, pending) = sync_channel(MAX_STREAM_PERMITS);
        let (sender, completed) = sync_channel(MAX_STREAM_PERMITS);
        let pending = Arc::new(Mutex::new(pending));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut manager = Self {
            requests: Some(requests),
            completed,
            cancel,
            handles: Vec::with_capacity(MAX_STREAM_PERMITS),
            phases: Arc::new(Mutex::new(CpuPhaseLedger::default())),
        };
        for worker_id in 0..MAX_STREAM_PERMITS {
            let pending = Arc::clone(&pending);
            let sender = sender.clone();
            let cancel = Arc::clone(&manager.cancel);
            let executor = Arc::clone(&executor);
            manager.handles.push(
                thread::Builder::new()
                    .name(format!("sh-probe-test-{worker_id}"))
                    .spawn(move || {
                        loop {
                            let request =
                                match pending.lock().expect("test request mutex poisoned").recv() {
                                    Ok(request) if !cancel.load(Ordering::Acquire) => request,
                                    _ => return,
                                };
                            let completion = executor(request);
                            if cancel.load(Ordering::Acquire) || sender.send(completion).is_err() {
                                return;
                            }
                        }
                    })?,
            );
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
}

impl Drop for ShAsyncWorkers {
    fn drop(&mut self) {
        self.stop();
    }
}

fn worker_loop(
    manifest: &ShStreamManifest,
    pending: &Mutex<Receiver<ShClusterRequest>>,
    completed: &SyncSender<ShWorkerCompletion>,
    cancel: &AtomicBool,
    phases: &Mutex<CpuPhaseLedger>,
    reader: &ChunkReader,
) {
    loop {
        let request = match pending.lock().expect("SH request mutex poisoned").recv() {
            Ok(request) if !cancel.load(Ordering::Acquire) => request,
            _ => return,
        };
        let index = &manifest.payloads().index[request.cluster_id as usize];
        let encoded_bytes = index.payload_len;
        let decoded_bytes = index.decoded_bytes;
        {
            let mut phases = phases.lock().expect("SH phase mutex poisoned");
            phases
                .encoded
                .add(encoded_bytes, "encoded worker bytes")
                .expect("validated encoded bound");
        }
        let encoded = reader(manifest, request.cluster_id);
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
            let result = manifest.decode_encoded_cluster(request.cluster_id, bytes);
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
    use std::time::{Duration, Instant};

    #[test]
    fn delayed_positional_reader_never_blocks_completion_drain_or_submission() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("encoded.bin");
        std::fs::write(&path, b"headpayloadtail").unwrap();
        let file = Arc::new(std::fs::File::open(path).unwrap());
        let executor = Arc::new(move |request: ShClusterRequest| {
            std::thread::sleep(Duration::from_millis(250));
            let mut bytes = vec![0; 7];
            file.read_at(&mut bytes, 4).unwrap();
            ShWorkerCompletion {
                request,
                result: Ok(DecodedClusterShPayload {
                    cluster_id: request.cluster_id,
                    bytes,
                    blocks: Vec::new(),
                }),
                ready_bytes: 0,
            }
        });
        let mut workers = ShAsyncWorkers::with_test_executor(executor).unwrap();
        let request = ShClusterRequest {
            generation: 17,
            content_tag: [4; 32],
            cluster_id: 0,
            chunk_hash: [7; 32],
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
        workers.stop();
    }

    #[test]
    fn teardown_cancels_queued_work_and_joins_four_workers() {
        let executions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = Arc::clone(&executions);
        let executor = Arc::new(move |request: ShClusterRequest| {
            observed.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(50));
            ShWorkerCompletion {
                request,
                result: Ok(DecodedClusterShPayload {
                    cluster_id: request.cluster_id,
                    bytes: Vec::new(),
                    blocks: Vec::new(),
                }),
                ready_bytes: 0,
            }
        });
        let mut workers = ShAsyncWorkers::with_test_executor(executor).unwrap();
        assert_eq!(workers.handles.len(), MAX_STREAM_PERMITS);
        for cluster_id in 0..MAX_STREAM_PERMITS as u32 {
            workers
                .submit(ShClusterRequest {
                    generation: 1,
                    content_tag: [0; 32],
                    cluster_id,
                    chunk_hash: [0; 32],
                })
                .unwrap();
        }
        workers.stop();
        assert!(workers.handles.is_empty());
        assert!(executions.load(Ordering::SeqCst) <= MAX_STREAM_PERMITS);
    }
}
