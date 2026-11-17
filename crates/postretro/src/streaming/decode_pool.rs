//! The level's one decode pool: SH chunks and lightmap pairs decode here, off the issuer.
//! See: context/lib/rendering_pipeline.md §4 (one issuer, one budget)

// The pool spawns and retires with the level's read issuer. Routes hand it
// jobs from the issuer thread; the issuer itself only reads. A thread takes
// the oldest queued mandatory job before any optional one, of either
// resource, so a mandatory decode waits at most for the decodes already
// running. Each job owns whatever it is charged against and releases it
// whether it runs, is discarded at retirement, or finds the pool retiring.

use std::collections::VecDeque;
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
    /// any.
    fn tier(&self) -> ReadTier;
    /// Decodes and delivers to the resource. `retiring` is set once the pool
    /// retires: a job that finds it set after decoding releases what it holds
    /// instead of delivering.
    fn run(self: Box<Self>, retiring: &AtomicBool);
    /// The pool retired before this job ran: release what it holds.
    fn discard(self: Box<Self>);
}

#[derive(Default)]
struct Queues {
    mandatory: VecDeque<Box<dyn DecodeJob>>,
    optional: VecDeque<Box<dyn DecodeJob>>,
    closed: bool,
}

impl Queues {
    fn pop(&mut self) -> Option<Box<dyn DecodeJob>> {
        self.mandatory
            .pop_front()
            .or_else(|| self.optional.pop_front())
    }
}

struct PoolShared {
    queues: Mutex<Queues>,
    ready: Condvar,
    retiring: AtomicBool,
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
        for job in mandatory.into_iter().chain(optional) {
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
        match job.tier() {
            ReadTier::Mandatory => queues.mandatory.push_back(job),
            ReadTier::Optional => queues.optional.push_back(job),
        }
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
                closed: false,
            }),
            ready: Condvar::new(),
            retiring: AtomicBool::new(false),
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
        job.run(&shared.retiring);
    }
}

#[cfg(test)]
#[path = "decode_pool_tests.rs"]
mod tests;
