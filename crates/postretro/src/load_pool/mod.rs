// Engine load pool: a few named threads with boot lifetime that run a load's
// CPU work as cancellable, out-of-order tasks grouped per load.
// See: context/lib/boot_sequence.md §9 · context/plans/in-progress/level-load-pipeline/index.md

use std::any::Any;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;

// Items marked `allow(dead_code)` outside tests are the cancel, retirement
// and inspection API the later load stages consume.

/// The most threads the pool ever runs. Prep shares the CPU with the main
/// thread, the upload thread and the SH decode workers, so width buys
/// structure, not speed.
pub(crate) const LOAD_POOL_WIDTH_CAP: usize = 2;

/// The pool width this machine ships with: two cores left to the main and
/// upload threads, at least one worker, never past the cap.
pub(crate) fn shipped_load_pool_width() -> usize {
    let parallelism = std::thread::available_parallelism().map_or(1, usize::from);
    width_for_parallelism(parallelism)
}

fn width_for_parallelism(parallelism: usize) -> usize {
    parallelism.saturating_sub(2).clamp(1, LOAD_POOL_WIDTH_CAP)
}

/// Which queue a task joins. A worker takes every queued `Ui` task before any
/// `Load` task: the player sees loading-screen images first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Priority {
    Ui,
    #[cfg_attr(not(test), allow(dead_code))]
    Load,
}

/// Why a task produced no value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskError {
    /// The task panicked; the message is the panic payload's text.
    Panicked(String),
    /// The task never ran: its group was cancelled, or the pool shut down,
    /// before a worker took it.
    Cancelled,
}

/// What a task reads between steps to stop early: set when its group is
/// cancelled or the pool shuts down.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone)]
pub(crate) struct CancelToken {
    group: Arc<AtomicBool>,
    pool: Arc<AtomicBool>,
}

impl CancelToken {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_cancelled(&self) -> bool {
        self.group.load(Ordering::Acquire) || self.pool.load(Ordering::Acquire)
    }
}

/// One task's result. Polled without blocking; dropping it discards the
/// result when the task finishes.
#[derive(Debug)]
pub(crate) struct TaskHandle<T> {
    result: Option<Receiver<Result<T, TaskError>>>,
}

impl<T> TaskHandle<T> {
    /// The task's result once it has finished, exactly once; `None` before
    /// that and after it was taken. Never waits.
    pub(crate) fn try_take(&mut self) -> Option<Result<T, TaskError>> {
        let taken = match self.result.as_ref()?.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return None,
            // The job dropped unrun, so its sender went with it.
            Err(TryRecvError::Disconnected) => Err(TaskError::Cancelled),
        };
        self.result = None;
        Some(taken)
    }
}

/// The engine's load pool. Built once per boot and kept for the process
/// lifetime; every load submits through a [`TaskGroup`].
///
/// Dropping it shuts it down without waiting: queued tasks drop, running
/// tasks see their token cancelled, finished workers are joined and the rest
/// are detached (see [`LoadPool::shutdown`]).
#[derive(Debug)]
pub(crate) struct LoadPool {
    shared: Arc<PoolShared>,
    workers: Vec<JoinHandle<()>>,
}

impl LoadPool {
    /// Start `width` workers (at least one), named `load-pool-N`. A worker
    /// that fails to start narrows the pool; none starting is an error.
    pub(crate) fn new(width: usize) -> std::io::Result<Self> {
        let shared = Arc::new(PoolShared::default());
        let mut workers = Vec::new();
        let mut first_error = None;
        for index in 0..width.max(1) {
            let worker_shared = shared.clone();
            match std::thread::Builder::new()
                .name(format!("load-pool-{index}"))
                .spawn(move || run_worker(&worker_shared))
            {
                Ok(handle) => {
                    shared.threads_started.fetch_add(1, Ordering::Relaxed);
                    workers.push(handle);
                }
                Err(err) => {
                    first_error.get_or_insert(err);
                }
            }
        }
        match first_error {
            Some(err) if workers.is_empty() => Err(err),
            Some(err) => {
                log::warn!(
                    "[Load] the load pool started {} of {} workers ({err})",
                    workers.len(),
                    width.max(1)
                );
                Ok(Self { shared, workers })
            }
            None => Ok(Self { shared, workers }),
        }
    }

    /// How many tasks run at once, at most.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn width(&self) -> usize {
        self.workers.len()
    }

    /// Threads this pool has started, counted by the pool itself. Never
    /// exceeds [`LoadPool::width`].
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn threads_started(&self) -> usize {
        self.shared.threads_started.load(Ordering::Relaxed)
    }

    /// Tasks a worker has taken off the queue.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn tasks_started(&self) -> usize {
        self.shared.tasks_started.load(Ordering::Relaxed)
    }

    /// A fresh group for one load's tasks.
    pub(crate) fn group(&self) -> TaskGroup {
        TaskGroup {
            spawner: GroupSpawner {
                pool: self.shared.clone(),
                group: Arc::new(GroupState::default()),
            },
        }
    }

    /// Signal shutdown and never wait: queued tasks drop unrun, running
    /// tasks see their token cancelled, and each worker exits after its
    /// current task. Workers already finished are joined; the rest are
    /// detached and end with their task or the process. Safe because a
    /// worker owns everything it touches (`'static` tasks, `Arc` pool
    /// state) and load tasks only read files and build memory, so no state
    /// outlives a task cut short at exit. Idempotent.
    pub(crate) fn shutdown(&mut self) {
        self.shared.shutdown_flag.store(true, Ordering::Release);
        let dropped = {
            let mut queue = self.shared.lock_queue();
            queue.shutdown = true;
            let mut dropped: Vec<Job> = queue.ui.drain(..).collect();
            dropped.extend(queue.load.drain(..));
            dropped
        };
        self.shared.ready.notify_all();
        drop(dropped);
        for handle in std::mem::take(&mut self.workers) {
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

impl Drop for LoadPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// One load's tasks. Cancelling it, or dropping it, drops its queued tasks
/// unrun and sets its [`CancelToken`]; running tasks finish or notice.
#[derive(Debug)]
pub(crate) struct TaskGroup {
    spawner: GroupSpawner,
}

impl TaskGroup {
    /// Queue `task` at `priority`. On a cancelled group or a shut-down pool
    /// it never runs and its handle yields [`TaskError::Cancelled`].
    pub(crate) fn spawn<T, F>(&self, priority: Priority, task: F) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(&CancelToken) -> T + Send + 'static,
    {
        self.spawner.spawn(priority, task)
    }

    /// A cloneable submitter into this group, for a task that queues more
    /// work (the parse handing prep its sections). It never cancels.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn spawner(&self) -> GroupSpawner {
        self.spawner.clone()
    }

    /// Keep `value` alive until every task this group started has returned,
    /// even past a cancel: the retirement releases it when it finishes.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn retain<A: Any + Send + Sync>(&self, value: Arc<A>) {
        lock(&self.spawner.group.retained).push(value);
    }

    /// Cancel the group and hand back what tracks its started tasks. Never
    /// waits.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn cancel(self) -> GroupRetirement {
        self.cancel_now();
        GroupRetirement {
            group: self.spawner.group.clone(),
        }
    }

    /// Cancel the group but keep it: queued tasks drop and tokens flip, as
    /// in [`Self::cancel`], which still hands back the retirement later.
    /// For an owner that must stop the group's work at once (a load's first
    /// failure) but retires it on its own schedule.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn cancel_in_place(&self) {
        self.cancel_now();
    }

    fn cancel_now(&self) {
        let GroupSpawner { pool, group } = &self.spawner;
        let dropped = {
            let mut guard = pool.lock_queue();
            let queue = &mut *guard;
            // Under the queue lock, so no worker takes one of this group's
            // tasks after this point and every taken one is counted running.
            group.cancelled.store(true, Ordering::Release);
            let mut dropped = Vec::new();
            for tasks in [&mut queue.ui, &mut queue.load] {
                for job in std::mem::take(tasks) {
                    if Arc::ptr_eq(&job.group, group) {
                        dropped.push(job);
                    } else {
                        tasks.push_back(job);
                    }
                }
            }
            dropped
        };
        // Dropping a job drops its inputs; never under the queue lock.
        drop(dropped);
    }
}

impl Drop for TaskGroup {
    fn drop(&mut self) {
        self.cancel_now();
    }
}

/// Submits into one group from any thread. It cannot cancel the group, and
/// once the group is cancelled its spawns never run.
#[derive(Debug, Clone)]
pub(crate) struct GroupSpawner {
    pool: Arc<PoolShared>,
    group: Arc<GroupState>,
}

impl GroupSpawner {
    /// As [`TaskGroup::spawn`].
    pub(crate) fn spawn<T, F>(&self, priority: Priority, task: F) -> TaskHandle<T>
    where
        T: Send + 'static,
        F: FnOnce(&CancelToken) -> T + Send + 'static,
    {
        // A result the handle no longer wants is dropped on the send.
        let (sender, receiver) = mpsc::sync_channel(1);
        let run: Box<dyn FnOnce(&CancelToken) + Send> = Box::new(move |token| {
            let result = catch_unwind(AssertUnwindSafe(|| task(token)))
                .map_err(|payload| TaskError::Panicked(panic_message(payload.as_ref())));
            let _ = sender.send(result);
        });
        let job = Job {
            group: self.group.clone(),
            token: self.token(),
            run,
        };
        let refused = {
            let mut queue = self.pool.lock_queue();
            if queue.shutdown || self.group.cancelled.load(Ordering::Acquire) {
                Some(job)
            } else {
                match priority {
                    Priority::Ui => queue.ui.push_back(job),
                    Priority::Load => queue.load.push_back(job),
                }
                None
            }
        };
        match refused {
            Some(job) => drop(job),
            None => self.pool.ready.notify_one(),
        }
        TaskHandle {
            result: Some(receiver),
        }
    }

    fn token(&self) -> CancelToken {
        CancelToken {
            group: self.group.cancelled.clone(),
            pool: self.pool.shutdown_flag.clone(),
        }
    }
}

/// A cancelled group still finishing the tasks it started. Poll
/// [`GroupRetirement::try_finish`] on later frames; nothing here waits, and
/// dropping it early gives up nothing a running task needs.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug)]
pub(crate) struct GroupRetirement {
    group: Arc<GroupState>,
}

#[cfg_attr(not(test), allow(dead_code))]
impl GroupRetirement {
    /// True once every task the group started has returned; its queued tasks
    /// were dropped at cancel. Then releases what the group retained.
    pub(crate) fn try_finish(&mut self) -> bool {
        if self.group.running.load(Ordering::Acquire) != 0 {
            return false;
        }
        drop(std::mem::take(&mut *lock(&self.group.retained)));
        true
    }
}

#[derive(Debug, Default)]
struct PoolShared {
    queue: Mutex<Queue>,
    ready: Condvar,
    /// Mirrors `Queue::shutdown` for tokens, which read it without the lock.
    shutdown_flag: Arc<AtomicBool>,
    threads_started: AtomicUsize,
    tasks_started: AtomicUsize,
}

impl PoolShared {
    fn lock_queue(&self) -> MutexGuard<'_, Queue> {
        lock(&self.queue)
    }
}

#[derive(Default)]
struct Queue {
    ui: VecDeque<Job>,
    load: VecDeque<Job>,
    shutdown: bool,
}

impl std::fmt::Debug for Queue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Queue")
            .field("ui", &self.ui.len())
            .field("load", &self.load.len())
            .field("shutdown", &self.shutdown)
            .finish()
    }
}

#[derive(Debug, Default)]
struct GroupState {
    cancelled: Arc<AtomicBool>,
    /// Tasks a worker has taken and not finished. Raised under the queue
    /// lock, lowered after the task and its inputs are dropped.
    running: AtomicUsize,
    /// Kept until the retirement finishes; failing that, until the last
    /// running task drops its hold on the group.
    #[cfg_attr(not(test), allow(dead_code))]
    retained: Mutex<Vec<Arc<dyn Any + Send + Sync>>>,
}

struct Job {
    group: Arc<GroupState>,
    token: CancelToken,
    run: Box<dyn FnOnce(&CancelToken) + Send>,
}

fn run_worker(shared: &PoolShared) {
    loop {
        let job = {
            let mut queue = shared.lock_queue();
            loop {
                if queue.shutdown {
                    return;
                }
                let next = match queue.ui.pop_front() {
                    Some(job) => Some(job),
                    None => queue.load.pop_front(),
                };
                if let Some(job) = next {
                    job.group.running.fetch_add(1, Ordering::AcqRel);
                    break job;
                }
                queue = shared
                    .ready
                    .wait(queue)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        shared.tasks_started.fetch_add(1, Ordering::Relaxed);
        let Job { group, token, run } = job;
        // `run` catches the task's panic, so the worker survives it.
        run(&token);
        group.running.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Every lock here guards plain data no panic can leave half-written, and
/// tasks run outside every lock, so a poisoned guard is still sound.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "a non-string panic payload".to_string()
    }
}

/// The app's pool slot: the width it builds at, and the pool once built.
/// Built on first use, then kept across suspend and resume until exit.
#[derive(Debug)]
pub(crate) struct EngineLoadPool {
    width: usize,
    pool: Option<LoadPool>,
}

impl EngineLoadPool {
    /// A slot that builds at [`shipped_load_pool_width`].
    pub(crate) fn shipped() -> Self {
        Self::with_width(shipped_load_pool_width())
    }

    /// A slot that builds at `width`, whatever the machine (tests use 2).
    pub(crate) fn with_width(width: usize) -> Self {
        Self { width, pool: None }
    }

    /// The pool, built now if this is its first use. A failed build is
    /// retried on the next use.
    pub(crate) fn get_or_build(&mut self) -> Result<&LoadPool, String> {
        if self.pool.is_none() {
            self.pool = Some(LoadPool::new(self.width).map_err(|err| err.to_string())?);
        }
        Ok(self.pool.as_ref().expect("built just above"))
    }

    /// The pool, if it has been built.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn built(&self) -> Option<&LoadPool> {
        self.pool.as_ref()
    }

    /// Process exit: shut the pool down without waiting.
    pub(crate) fn shutdown(&mut self) {
        if let Some(pool) = self.pool.as_mut() {
            pool.shutdown();
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod spawn_gate_tests;
