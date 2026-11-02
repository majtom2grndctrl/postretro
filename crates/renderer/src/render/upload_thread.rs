// Renderer-owned upload thread: off-main GPU creation with per-job error scopes.
// See: context/lib/boot_sequence.md §2 · context/lib/rendering_pipeline.md §1

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::JoinHandle;

use super::queue_writes::QueueWrites;

/// The most queue-write bytes one upload-thread submit carries. Every byte a
/// job stages on the queue is metered as it is staged: its
/// [`QueueWrites`] buffer and texture writes, the initial data of a texture
/// created through `GpuCreate::new_texture_with_data`, and the contents of a
/// buffer created through `GpuCreate::new_buffer_init` at the size wgpu
/// pads them to (it stages them on the queue's pending writes when it unmaps
/// the buffer). One that would
/// push the pending batch past this submits the batch first, so every batch
/// stays within it except a single write larger than the chunk, which is its
/// own batch. Bounds the flush any one submit (and the frame after it) pays.
pub const UPLOAD_SUBMIT_CHUNK_BYTES: u64 = 32 * 1024 * 1024;

/// What a job sees on the upload thread: a clone of the renderer's device,
/// and the metered writer every write goes through (the raw queue stays
/// inside it). Nothing a job creates is reachable from the frame path until
/// the main thread installs its result.
pub struct UploadContext {
    pub device: wgpu::Device,
    /// The upload thread's queue writes, submitted in bounded batches.
    pub(crate) writes: ChunkedWrites,
}

/// The upload thread's byte-counting writer: stages each write on the queue
/// and submits the pending batch before a write would carry it past
/// [`UPLOAD_SUBMIT_CHUNK_BYTES`]. Every job ends with [`Self::flush`], so a
/// job's writes are submitted before its result is delivered (pin P13).
/// Clones share one pending batch: the thread's creation meter
/// ([`meter_initial_buffer_data`]) holds one, so buffer contents count into
/// the same batch as the job's writes.
#[derive(Clone)]
pub(crate) struct ChunkedWrites {
    queue: wgpu::Queue,
    chunk_bytes: u64,
    pending: Rc<Cell<u64>>,
    /// Every metered and submitted byte, shared with the owning renderer so
    /// the app's tests can read the writer's state against a frame (P13).
    #[cfg(feature = "test-seams")]
    ledger: std::sync::Arc<UploadWriteLedger>,
    /// Each submitted batch's bytes, oldest first; tests read and clear it.
    #[cfg(test)]
    batches: Rc<RefCell<Vec<u64>>>,
}

// The upload thread's writer, installed when the thread starts: what the
// creation chokepoint meters a buffer's initial contents through. Unset on
// every other thread.
thread_local! {
    static THREAD_WRITES: RefCell<Option<ChunkedWrites>> = const { RefCell::new(None) };
}

/// Drops the upload thread's writer while thread-locals are still alive, on a
/// normal exit or an unwind. Left to the thread-local destructor, dropping
/// its queue would reach wgpu's own thread-locals, which may already be
/// destroyed by then: a panic in a thread-local destructor aborts the process.
/// Observed: without this guard the test binaries aborted with `0xc0000409`
/// (`STATUS_STACK_BUFFER_OVERRUN`) as the upload thread exited, from a
/// wgpu-core thread-local `AccessError` panic while the destructor dropped
/// the queue.
struct ClearThreadWrites;

impl Drop for ClearThreadWrites {
    fn drop(&mut self) {
        let _ = THREAD_WRITES.try_with(|writes| writes.borrow_mut().take());
    }
}

/// Meter `bytes` of a buffer's initial contents through the calling thread's
/// upload writer, before the buffer is created: on the upload thread they
/// count toward its submit chunk like any queued write (the pending batch
/// submits first when they would carry it past the chunk); on any other
/// thread this does nothing, and the contents ride that thread's next
/// submit. Only the creation chokepoint (`GpuCreate::new_buffer_init`)
/// calls it, with the contents' length.
pub(super) fn meter_initial_buffer_data(bytes: u64) {
    // wgpu sizes the buffer, and so the staged copy, up to the copy alignment
    // (empty contents stay zero).
    let staged = bytes.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT);
    THREAD_WRITES.with(|writes| {
        if let Some(writes) = writes.borrow().as_ref() {
            writes.meter(staged);
        }
    });
}

impl ChunkedWrites {
    fn new(
        queue: wgpu::Queue,
        #[cfg(feature = "test-seams")] ledger: std::sync::Arc<UploadWriteLedger>,
    ) -> Self {
        Self {
            queue,
            chunk_bytes: UPLOAD_SUBMIT_CHUNK_BYTES,
            pending: Rc::new(Cell::new(0)),
            #[cfg(feature = "test-seams")]
            ledger,
            #[cfg(test)]
            batches: Default::default(),
        }
    }

    /// Submit the pending batch first when `bytes` more would carry it past
    /// the chunk; then count them in.
    fn meter(&self, bytes: u64) {
        let pending = self.pending.get();
        if pending > 0 && pending.saturating_add(bytes) > self.chunk_bytes {
            self.flush();
        }
        self.pending.set(self.pending.get().saturating_add(bytes));
        #[cfg(feature = "test-seams")]
        self.ledger.staged(bytes);
        #[cfg(test)]
        UNSUBMITTED.with(|unsubmitted| unsubmitted.set(self.pending.get()));
    }

    /// Submit every staged write. A batch with no bytes still submits, so a
    /// job's writes staged outside this writer flush too.
    pub(crate) fn flush(&self) {
        flush_staged_writes(&self.queue);
        let bytes = self.pending.replace(0);
        #[cfg(feature = "test-seams")]
        self.ledger.submitted(bytes);
        #[cfg(test)]
        UNSUBMITTED.with(|unsubmitted| unsubmitted.set(0));
        #[cfg(test)]
        if bytes > 0 {
            self.batches.borrow_mut().push(bytes);
        }
        #[cfg(not(any(test, feature = "test-seams")))]
        let _ = bytes;
    }

    /// The batches submitted so far, emptied.
    #[cfg(test)]
    pub(crate) fn take_batches(&self) -> Vec<u64> {
        std::mem::take(&mut *self.batches.borrow_mut())
    }
}

/// One upload thread's writes, counted where they are metered and where
/// they are submitted. Test seam: the app's acquire-skip test reads it
/// against the first frame that presents (pin P13).
#[cfg(feature = "test-seams")]
#[derive(Debug, Default)]
pub struct UploadWriteLedger {
    staged_bytes: std::sync::atomic::AtomicU64,
    submitted_bytes: std::sync::atomic::AtomicU64,
    submits: std::sync::atomic::AtomicU64,
}

/// A reading of an [`UploadWriteLedger`].
#[cfg(feature = "test-seams")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UploadWrites {
    /// Bytes the writer staged on the queue.
    pub staged_bytes: u64,
    /// Bytes an upload-thread submit carried.
    pub submitted_bytes: u64,
    /// Upload-thread submits (every job ends with one).
    pub submits: u64,
}

#[cfg(feature = "test-seams")]
impl UploadWrites {
    /// Bytes staged on the queue and not yet submitted by the upload thread.
    pub fn unsubmitted_bytes(&self) -> u64 {
        self.staged_bytes - self.submitted_bytes
    }
}

#[cfg(feature = "test-seams")]
impl UploadWriteLedger {
    fn staged(&self, bytes: u64) {
        self.staged_bytes
            .fetch_add(bytes, std::sync::atomic::Ordering::AcqRel);
    }

    fn submitted(&self, bytes: u64) {
        // The submit itself precedes this count, so a reader that sees the
        // bytes counted sees them submitted.
        self.submitted_bytes
            .fetch_add(bytes, std::sync::atomic::Ordering::AcqRel);
        self.submits
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }

    /// The writer's counts now. Submitted bytes read first, so a write
    /// racing the read never shows as submitted before it is staged.
    pub fn read(&self) -> UploadWrites {
        let submits = self.submits.load(std::sync::atomic::Ordering::Acquire);
        let submitted_bytes = self
            .submitted_bytes
            .load(std::sync::atomic::Ordering::Acquire);
        let staged_bytes = self.staged_bytes.load(std::sync::atomic::Ordering::Acquire);
        UploadWrites {
            staged_bytes,
            submitted_bytes,
            submits,
        }
    }
}

// The upload thread's writer's unsubmitted bytes, mirrored where a job's
// delivery (on the same thread) can read them.
#[cfg(test)]
thread_local! {
    static UNSUBMITTED: Cell<u64> = const { Cell::new(0) };
}

/// The calling thread's upload writer's unsubmitted bytes (zero off the
/// upload thread).
#[cfg(test)]
pub(crate) fn unsubmitted_bytes_on_this_thread() -> u64 {
    UNSUBMITTED.with(Cell::get)
}

impl QueueWrites for ChunkedWrites {
    fn write_buffer(&self, buffer: &wgpu::Buffer, offset: wgpu::BufferAddress, data: &[u8]) {
        self.meter(data.len() as u64);
        self.queue.write_buffer(buffer, offset, data);
    }

    fn write_texture(
        &self,
        texture: wgpu::TexelCopyTextureInfo<'_>,
        data: &[u8],
        data_layout: wgpu::TexelCopyBufferLayout,
        size: wgpu::Extent3d,
    ) {
        self.meter(data.len() as u64);
        self.queue.write_texture(texture, data, data_layout, size);
    }

    fn queue_for_initial_data(&self, bytes: u64) -> &wgpu::Queue {
        self.meter(bytes);
        &self.queue
    }
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
    // Kept, not joined: see `Drop`. Handed off by `take_retirement`.
    handle: Option<JoinHandle<()>>,
    #[cfg(feature = "test-seams")]
    ledger: std::sync::Arc<UploadWriteLedger>,
}

impl UploadThread {
    pub fn spawn(device: wgpu::Device, queue: wgpu::Queue) -> std::io::Result<Self> {
        let (jobs, receiver) = mpsc::channel::<Job>();
        #[cfg(feature = "test-seams")]
        let ledger = std::sync::Arc::<UploadWriteLedger>::default();
        #[cfg(feature = "test-seams")]
        let thread_ledger = std::sync::Arc::clone(&ledger);
        let handle = std::thread::Builder::new()
            .name("render-upload".into())
            .spawn(move || {
                let context = UploadContext {
                    device,
                    writes: ChunkedWrites::new(
                        queue,
                        #[cfg(feature = "test-seams")]
                        thread_ledger,
                    ),
                };
                // Buffers created with contents on this thread meter through
                // the jobs' own writer.
                THREAD_WRITES.with(|writes| *writes.borrow_mut() = Some(context.writes.clone()));
                let _clear_writes = ClearThreadWrites;
                // A closed channel (the owner dropped) ends the thread.
                while let Ok(job) = receiver.recv() {
                    job(&context);
                }
            })?;
        Ok(Self {
            jobs: Some(jobs),
            handle: Some(handle),
            #[cfg(feature = "test-seams")]
            ledger,
        })
    }

    /// This thread's write counts (test seam, pin P13).
    #[cfg(feature = "test-seams")]
    pub fn writes(&self) -> UploadWrites {
        self.ledger.read()
    }

    /// Queue `job` and return a handle the caller polls. The job's GPU writes
    /// are submitted before its result is sent, so a result in hand means its
    /// writes precede any later submit on any thread. Tests' polling form of
    /// [`Self::submit_then`]; production jobs deliver through that.
    #[cfg(test)]
    pub(crate) fn submit<T: Send + 'static>(
        &self,
        job: impl FnOnce(&UploadContext) -> Result<T, UploadError> + Send + 'static,
    ) -> PendingUpload<T> {
        let (result_tx, result_rx) = mpsc::channel();
        // The receiver may be gone (a cancelled load); nothing waits then. A
        // send failure in `submit_then` drops this sender, so the pending
        // handle reads `ThreadGone`.
        self.submit_then(job, move |result| {
            let _ = result_tx.send(result);
        });
        PendingUpload { result: result_rx }
    }

    /// Queue `job`, then hand its result to `deliver` on the upload thread,
    /// after the job's flush and error scopes, before the next job starts. A
    /// later job therefore observes everything `deliver` published. If the
    /// thread is gone, both closures drop unrun. A panic in `deliver` is
    /// contained and logged, so the thread keeps serving later jobs; turning
    /// it into the job's own failure is the deliverer's (a level build's
    /// does).
    pub(crate) fn submit_then<T: 'static>(
        &self,
        job: impl FnOnce(&UploadContext) -> Result<T, UploadError> + Send + 'static,
        deliver: impl FnOnce(Result<T, UploadError>) + Send + 'static,
    ) {
        if let Some(submitter) = self.submitter() {
            submitter.submit_then(job, deliver);
        }
    }

    /// A handle that queues jobs on this thread from any thread; `None` once
    /// the thread is retired.
    pub(crate) fn submitter(&self) -> Option<UploadSubmitter> {
        self.jobs.clone().map(UploadSubmitter)
    }

    /// Close the job queue and hand off the thread for a poll-and-join
    /// retirement. Jobs already queued still run (on this device; their
    /// results reach dropped receivers), then the thread ends. Later
    /// submissions read `ThreadGone`. `None` once taken.
    pub(crate) fn take_retirement(&mut self) -> Option<UploadRetirement> {
        self.jobs.take();
        self.handle.take().map(UploadRetirement::from_thread)
    }
}

/// Queues jobs on one upload thread from any thread. Cloned into a level
/// build's builder, so pool tasks queue their parts directly.
#[derive(Clone)]
pub(crate) struct UploadSubmitter(mpsc::Sender<Job>);

impl UploadSubmitter {
    /// [`UploadThread::submit_then`] from any thread. Returns `false`, with
    /// both closures dropped unrun, when the thread is gone.
    pub(crate) fn submit_then<T: 'static>(
        &self,
        job: impl FnOnce(&UploadContext) -> Result<T, UploadError> + Send + 'static,
        deliver: impl FnOnce(Result<T, UploadError>) + Send + 'static,
    ) -> bool {
        let wrapped: Job = Box::new(move |context| {
            let result = run_job(context, job);
            // Delivery runs outside the job's `catch_unwind`; a panic here
            // must not end the thread every later job (other loads') needs.
            if let Err(payload) =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| deliver(result)))
            {
                log::error!(
                    "[Renderer] upload job delivery panicked: {}",
                    panic_message(payload.as_ref())
                );
            }
        });
        self.0.send(wrapped).is_ok()
    }
}

/// A torn-down renderer's upload thread, joined only once it has finished.
/// Dropping it detaches the thread: nothing here ever blocks.
#[derive(Debug)]
pub struct UploadRetirement {
    handle: Option<JoinHandle<()>>,
}

impl UploadRetirement {
    /// Retire any finished-or-finishing thread; the app's tests build one
    /// from a plain thread, since a real upload thread needs a device.
    pub fn from_thread(handle: JoinHandle<()>) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    /// `true` once the thread has finished and been joined. Joins only when
    /// `is_finished`, so the join returns at once; never waits.
    pub fn try_finish(&mut self) -> bool {
        match &self.handle {
            Some(handle) if !handle.is_finished() => false,
            Some(_) => {
                if let Some(handle) = self.handle.take() {
                    // A job panic was caught inside the thread; a join error
                    // here would mean the loop itself panicked, with nothing
                    // left to recover.
                    let _ = handle.join();
                }
                true
            }
            None => true,
        }
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
    context.writes.flush();
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

/// A caught panic's message, for an [`UploadError::Panicked`].
pub(super) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
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

/// A job's result, polled without blocking (tests).
#[cfg(test)]
pub(crate) struct PendingUpload<T> {
    result: mpsc::Receiver<Result<T, UploadError>>,
}

#[cfg(test)]
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
