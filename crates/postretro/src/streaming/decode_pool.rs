//! The level's one decode pool: SH chunks and lightmap pairs decode here, off the issuer.
//! See: context/lib/rendering_pipeline.md §4 (one issuer, one budget)

// The pool spawns and retires with the level's read issuer. Routes hand it
// jobs from the issuer thread; the issuer itself only reads. A thread takes
// the oldest queued mandatory job before any optional one, of either
// resource, so a mandatory decode waits at most for the decodes already
// running. Each job owns whatever it is charged against and releases it
// whether it runs, panics, is discarded at retirement, or finds the pool
// retiring.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};

use super::request::ReadTier;

/// Pool thread names: `stream-decode-0`, `stream-decode-1`, ….
pub(crate) const DECODE_THREAD_NAME_PREFIX: &str = "stream-decode-";

/// Decode threads: half the available cores, at least one, at most three.
pub(crate) fn decode_pool_size(available_parallelism: usize) -> usize {
    (available_parallelism / 2).clamp(1, 3)
}

/// One resource item's decode, handed over by its route.
pub(crate) trait DecodeJob: Send {
    /// Mandatory jobs of every resource are taken before optional jobs of
    /// any. A queued optional job may become mandatory (a raise); the pool
    /// rechecks queued optional jobs before taking one.
    fn tier(&self) -> ReadTier;
    /// Decodes and delivers to the resource. `retiring` is set once the pool
    /// retires: a job that finds it set releases what it holds instead of
    /// delivering.
    fn run(&mut self, retiring: &AtomicBool);
    /// The pool retired before this job ran: release what it holds.
    fn discard(self: Box<Self>);
    /// `run` panicked: release what the job still holds and, unless the pool
    /// is retiring or the job already delivered, complete its item as failed,
    /// so its resource frees the item and nothing waits on it.
    fn panicked(self: Box<Self>, retiring: &AtomicBool);
}

/// A queued job and its submission order.
type Queued = (u64, Box<dyn DecodeJob>);

#[derive(Default)]
struct Queues {
    mandatory: VecDeque<Queued>,
    optional: VecDeque<Queued>,
    submitted: u64,
    closed: bool,
}

impl Queues {
    fn push(&mut self, job: Box<dyn DecodeJob>) {
        let queued = (self.submitted, job);
        self.submitted += 1;
        match queued.1.tier() {
            ReadTier::Mandatory => self.mandatory.push_back(queued),
            ReadTier::Optional => self.optional.push_back(queued),
        }
    }

    /// The oldest mandatory job, counting an optional job raised to mandatory
    /// while queued from its own submission; then the oldest optional job.
    fn pop(&mut self) -> Option<Box<dyn DecodeJob>> {
        let raised = self
            .optional
            .iter()
            .position(|(_, job)| job.tier() == ReadTier::Mandatory);
        let take_raised = match (raised, self.mandatory.front()) {
            (Some(index), Some(&(oldest, _))) => self.optional[index].0 < oldest,
            (Some(_), None) => true,
            (None, _) => false,
        };
        let queued = if take_raised {
            raised.and_then(|index| self.optional.remove(index))
        } else {
            self.mandatory
                .pop_front()
                .or_else(|| self.optional.pop_front())
        };
        queued.map(|(_, job)| job)
    }
}

struct PoolShared {
    queues: Mutex<Queues>,
    ready: Condvar,
    retiring: AtomicBool,
    /// A job has panicked; only the first is logged.
    panic_logged: AtomicBool,
}

impl PoolShared {
    fn lock(&self) -> MutexGuard<'_, Queues> {
        self.queues
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Stops new submissions, wakes every thread, and discards every queued
    /// job outside the lock.
    fn close(&self) {
        self.retiring.store(true, Ordering::Release);
        let (mandatory, optional) = {
            let mut queues = self.lock();
            queues.closed = true;
            (
                std::mem::take(&mut queues.mandatory),
                std::mem::take(&mut queues.optional),
            )
        };
        self.ready.notify_all();
        for (_, job) in mandatory.into_iter().chain(optional) {
            job.discard();
        }
    }
}

/// A route's handle for submitting jobs. Cloned per route; never blocks.
#[derive(Clone)]
pub(crate) struct DecodeQueue {
    shared: Arc<PoolShared>,
}

impl std::fmt::Debug for DecodeQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodeQueue").finish_non_exhaustive()
    }
}

impl DecodeQueue {
    /// Queues `job` behind every job of its tier. False once the pool has
    /// retired; the job is then discarded here.
    pub(crate) fn submit(&self, job: Box<dyn DecodeJob>) -> bool {
        let mut queues = self.shared.lock();
        if queues.closed {
            drop(queues);
            job.discard();
            return false;
        }
        queues.push(job);
        drop(queues);
        self.shared.ready.notify_one();
        true
    }
}

/// The pool's threads. Dropping it retires and joins them.
pub(crate) struct DecodePool {
    shared: Arc<PoolShared>,
    handles: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for DecodePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecodePool")
            .field("threads", &self.handles.len())
            .finish_non_exhaustive()
    }
}

impl DecodePool {
    /// Spawns `threads` (at least one) decode threads. `queue_capacity`
    /// covers every job the routed resources can have outstanding, so steady
    /// streaming never grows a queue. On a spawn failure the threads already
    /// spawned retire before the error returns.
    pub(crate) fn spawn(threads: usize, queue_capacity: usize) -> std::io::Result<Self> {
        let shared = Arc::new(PoolShared {
            queues: Mutex::new(Queues {
                mandatory: VecDeque::with_capacity(queue_capacity),
                optional: VecDeque::with_capacity(queue_capacity),
                submitted: 0,
                closed: false,
            }),
            ready: Condvar::new(),
            retiring: AtomicBool::new(false),
            panic_logged: AtomicBool::new(false),
        });
        let mut pool = Self {
            shared,
            handles: Vec::with_capacity(threads.max(1)),
        };
        for index in 0..threads.max(1) {
            let shared = Arc::clone(&pool.shared);
            let handle = thread::Builder::new()
                .name(format!("{DECODE_THREAD_NAME_PREFIX}{index}"))
                .spawn(move || decode_loop(&shared))?;
            pool.handles.push(handle);
        }
        Ok(pool)
    }

    pub(crate) fn queue(&self) -> DecodeQueue {
        DecodeQueue {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Retires the pool without waiting: no new job is accepted, every queued
    /// job is discarded, and a running job releases rather than delivers.
    /// Returns the threads for the caller to join once they have finished.
    pub(crate) fn begin_retirement(mut self) -> Vec<JoinHandle<()>> {
        self.shared.close();
        std::mem::take(&mut self.handles)
    }
}

impl Drop for DecodePool {
    fn drop(&mut self) {
        self.shared.close();
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

fn decode_loop(shared: &PoolShared) {
    loop {
        let job = {
            let mut queues = shared.lock();
            loop {
                if queues.closed {
                    return;
                }
                if let Some(job) = queues.pop() {
                    break job;
                }
                queues = shared
                    .ready
                    .wait(queues)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        run_job(shared, job);
    }
}

/// Runs one job. A panicking job completes as failed instead of taking its
/// thread down with its charge, its item and every job queued behind it.
fn run_job(shared: &PoolShared, mut job: Box<dyn DecodeJob>) {
    let Err(panic) = catch_unwind(AssertUnwindSafe(|| job.run(&shared.retiring))) else {
        return;
    };
    if !shared.panic_logged.swap(true, Ordering::AcqRel) {
        let message = panic
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("non-string panic payload");
        log::error!(
            "[Streaming] a decode job panicked: {message}; its item fails and the pool \
             keeps running (later panics are not logged)"
        );
    }
    // A job whose own cleanup panics too leaves its thread running.
    let _ = catch_unwind(AssertUnwindSafe(|| job.panicked(&shared.retiring)));
}

#[cfg(test)]
#[path = "decode_pool_tests.rs"]
mod tests;
