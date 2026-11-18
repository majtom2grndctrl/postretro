//! The level's one decode pool: SH chunks and lightmap pairs decode here, off the issuer.
//! See: context/lib/rendering_pipeline.md §4 (one issuer, one budget)

// The pool spawns and retires with the level's read issuer. Routes hand it
// jobs from the issuer thread; the issuer itself only reads. A thread takes
// the queued job of best rank, read live from the job's key at every pickup.
// A job whose key has left its targets goes first: it releases its charge
// and completes cancelled without decoding, so it costs almost nothing and
// its resource gets the item's permit back sooner.
// Decodes follow in class order (Visible, then Pinned, then Lead, then the
// optional classes), SH before lightmap at equal class as the drain admits,
// then oldest first. A mandatory decode is therefore taken before every
// queued optional decode, and waits only for decodes already running and
// for more urgent mandatory work. A job whose key turns mandatory while it
// waits is lifted past every optional job, and one whose key is demoted
// loses its precedence. Each job owns whatever it is charged against and
// releases it whether it runs, panics, is discarded at retirement, or finds
// the pool retiring.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::drain_budget::DrainClass;
use super::request::StreamResource;
use super::target_set::TargetSet;

/// Pool thread names: `stream-decode-0`, `stream-decode-1`, ….
pub(crate) const DECODE_THREAD_NAME_PREFIX: &str = "stream-decode-";

/// Decode threads: half the available cores, at least one, at most three.
pub(crate) fn decode_pool_size(available_parallelism: usize) -> usize {
    (available_parallelism / 2).clamp(1, 3)
}

/// A queued job's place in the pool's pickup order. Equal ranks go oldest
/// first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum DecodeRank {
    /// The job's key has left its targets: its run releases its charge and
    /// completes it cancelled without decoding, so it is taken before every
    /// decode and its resource gets the item's permit back sooner.
    Departed,
    /// A decode, compared by class, then resource (SH first at equal class).
    Decode {
        class: DrainClass,
        resource: StreamResource,
    },
}

impl DecodeRank {
    /// The rank of a job for `key` in `targets`: departed when `key` is not
    /// a member, otherwise a decode at its live class (or `requested` where
    /// a test published membership without classes).
    pub(crate) fn live(
        targets: &TargetSet,
        key: u32,
        requested: DrainClass,
        resource: StreamResource,
    ) -> Self {
        match targets.class_or(key, requested) {
            Some(class) => Self::Decode { class, resource },
            None => Self::Departed,
        }
    }
}

/// One resource item's decode, handed over by its route.
pub(crate) trait DecodeJob: Send {
    /// The job's rank, read again at every pickup while it is queued:
    /// usually [`DecodeRank::live`] over its resource's target set.
    fn rank(&self) -> DecodeRank;
    /// Decodes and delivers to the resource. `retiring` is set once the pool
    /// retires: a job that finds it set releases what it holds instead of
    /// delivering. `queue_wait` is the time from submission to this call.
    fn run(&mut self, retiring: &AtomicBool, queue_wait: Duration);
    /// The pool retired before this job ran: release what it holds.
    fn discard(self: Box<Self>);
    /// `run` panicked: release what the job still holds and, unless the pool
    /// is retiring or the job already delivered, complete its item as failed,
    /// so its resource frees the item and nothing waits on it. Must not
    /// panic. If it panics anyway, the job drops mid-unwind: a job that holds
    /// a charge returns it from `Drop`, but its item never completes and its
    /// resource never frees it.
    fn panicked(self: Box<Self>, retiring: &AtomicBool);
}

/// A queued job, its submission order, and when it was submitted.
struct Queued {
    order: u64,
    submitted_at: Instant,
    job: Box<dyn DecodeJob>,
}

#[derive(Default)]
struct Queues {
    jobs: Vec<Queued>,
    submitted: u64,
    closed: bool,
}

impl Queues {
    fn push(&mut self, job: Box<dyn DecodeJob>) {
        self.jobs.push(Queued {
            order: self.submitted,
            submitted_at: Instant::now(),
            job,
        });
        self.submitted += 1;
    }

    /// The job of best live rank, oldest first at equal rank. Ranks are read
    /// at every pickup; the queue holds at most every permit's job, so the
    /// scan is short.
    fn pop(&mut self) -> Option<Queued> {
        let best = self
            .jobs
            .iter()
            .enumerate()
            .min_by_key(|(_, queued)| (queued.job.rank(), queued.order))
            .map(|(index, _)| index)?;
        // Submission order travels with each job, so the swap loses nothing.
        Some(self.jobs.swap_remove(best))
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
        let mut queued = {
            let mut queues = self.lock();
            queues.closed = true;
            std::mem::take(&mut queues.jobs)
        };
        self.ready.notify_all();
        queued.sort_unstable_by_key(|queued| queued.order);
        for queued in queued {
            queued.job.discard();
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
    /// Queues `job` behind every queued job of its rank. False once the pool
    /// has retired; the job is then discarded here.
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
    /// covers every job the routed resources can have outstanding (one per
    /// permit: a job is a delivered read), so steady streaming never grows
    /// the queue. On a spawn failure the threads already
    /// spawned retire before the error returns.
    pub(crate) fn spawn(threads: usize, queue_capacity: usize) -> std::io::Result<Self> {
        let shared = Arc::new(PoolShared {
            queues: Mutex::new(Queues {
                jobs: Vec::with_capacity(queue_capacity),
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
        let queued = {
            let mut queues = shared.lock();
            loop {
                if queues.closed {
                    return;
                }
                if let Some(queued) = queues.pop() {
                    break queued;
                }
                queues = shared
                    .ready
                    .wait(queues)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        run_job(shared, queued);
    }
}

/// Runs one job. A panicking job completes as failed instead of taking its
/// thread down with its charge, its item and every job queued behind it.
fn run_job(shared: &PoolShared, queued: Queued) {
    let Queued {
        submitted_at,
        mut job,
        ..
    } = queued;
    let queue_wait = submitted_at.elapsed();
    let Err(panic) = catch_unwind(AssertUnwindSafe(|| job.run(&shared.retiring, queue_wait)))
    else {
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
    // `panicked` completes the item without panicking. One that panics
    // anyway leaves its thread running; the job drops during that unwind and
    // its `Drop` returns any charge it still holds, but its item gets no
    // completion.
    let _ = catch_unwind(AssertUnwindSafe(|| job.panicked(&shared.retiring)));
}

#[cfg(test)]
#[path = "decode_pool_tests.rs"]
mod tests;
