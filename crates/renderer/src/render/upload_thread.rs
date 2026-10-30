// Renderer-owned upload thread: off-main GPU creation with per-job error scopes.
// See: context/lib/boot_sequence.md §2 · context/lib/rendering_pipeline.md §1

use std::sync::mpsc;
use std::thread::JoinHandle;

/// What a job sees on the upload thread: clones of the renderer's device and
/// raw queue. Nothing a job creates is reachable from the frame path until the
/// main thread installs its result.
pub struct UploadContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// Why an upload job produced no result.
#[derive(Debug)]
pub enum UploadError {
    /// A wgpu validation or out-of-memory error raised inside the job,
    /// captured by the job's error scope instead of wgpu's panicking default.
    Wgpu(String),
    /// The job panicked; the thread caught it and keeps serving.
    Panicked(String),
    /// The upload thread is gone, so the job's result will never arrive.
    ThreadGone,
}

impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wgpu(error) => write!(f, "upload job raised a wgpu error: {error}"),
            Self::Panicked(message) => write!(f, "upload job panicked: {message}"),
            Self::ThreadGone => write!(f, "upload thread is gone"),
        }
    }
}

impl std::error::Error for UploadError {}

type Job = Box<dyn FnOnce(&UploadContext) + Send + 'static>;

/// One thread that runs GPU creation jobs in submission order.
pub struct UploadThread {
    jobs: Option<mpsc::Sender<Job>>,
    // Kept, not joined: see `Drop`.
    _handle: Option<JoinHandle<()>>,
}

impl UploadThread {
    pub fn spawn(device: wgpu::Device, queue: wgpu::Queue) -> std::io::Result<Self> {
        let (jobs, receiver) = mpsc::channel::<Job>();
        let handle = std::thread::Builder::new()
            .name("render-upload".into())
            .spawn(move || {
                let context = UploadContext { device, queue };
                // A closed channel (the owner dropped) ends the thread.
                while let Ok(job) = receiver.recv() {
                    job(&context);
                }
            })?;
        Ok(Self {
            jobs: Some(jobs),
            _handle: Some(handle),
        })
    }

    /// Queue `job` and return a handle the caller polls. The job's GPU writes
    /// are submitted before its result is sent, so a result in hand means its
    /// writes precede any later submit on any thread.
    pub fn submit<T: Send + 'static>(
        &self,
        job: impl FnOnce(&UploadContext) -> Result<T, UploadError> + Send + 'static,
    ) -> PendingUpload<T> {
        let (result_tx, result_rx) = mpsc::channel();
        let wrapped: Job = Box::new(move |context| {
            // The receiver may be gone (a cancelled load); nothing waits then.
            let _ = result_tx.send(run_job(context, job));
        });
        if let Some(jobs) = &self.jobs {
            // A send failure drops `wrapped` and its sender, so the pending
            // handle reads `ThreadGone`.
            let _ = jobs.send(wrapped);
        }
        PendingUpload { result: result_rx }
    }
}

/// One job under its error scopes, then the flush, then the scopes' verdict.
fn run_job<T>(
    context: &UploadContext,
    job: impl FnOnce(&UploadContext) -> Result<T, UploadError>,
) -> Result<T, UploadError> {
    // Error scopes are per thread, so these see only this job's errors.
    let validation = context
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let out_of_memory = context
        .device
        .push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(context)))
        .unwrap_or_else(|payload| Err(UploadError::Panicked(panic_message(payload.as_ref()))));
    // Submit every write the job staged before the result can be handed back,
    // so the hand-back never precedes the flush (pin P13).
    flush_staged_writes(&context.queue);
    // Scopes pop in reverse push order.
    let out_of_memory = pollster::block_on(out_of_memory.pop());
    let validation = pollster::block_on(validation.pop());
    match validation.or(out_of_memory) {
        Some(error) if result.is_ok() => Err(UploadError::Wgpu(error.to_string())),
        _ => result,
    }
}

fn flush_staged_writes(queue: &wgpu::Queue) {
    queue.submit(std::iter::empty());
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| message.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic payload".to_string())
}

impl Drop for UploadThread {
    fn drop(&mut self) {
        // Closing the job channel ends the thread after its current job. No
        // join: this runs on the event-loop thread, which must never wait on a
        // stage thread; retirement polls and joins elsewhere.
        self.jobs.take();
    }
}

/// A job's result, polled without blocking.
pub struct PendingUpload<T> {
    result: mpsc::Receiver<Result<T, UploadError>>,
}

impl<T> PendingUpload<T> {
    /// `None` while the job runs. Never blocks.
    pub fn try_take(&mut self) -> Option<Result<T, UploadError>> {
        match self.result.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(UploadError::ThreadGone)),
        }
    }
}

#[cfg(test)]
#[path = "upload_thread_tests.rs"]
pub(super) mod tests;
