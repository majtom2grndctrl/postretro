//! Cooperative pause and parallel-work admission control for compiler bakes.
//! See: `context/lib/build_pipeline.md`.

use std::sync::{Condvar, Mutex, MutexGuard};

#[cfg(test)]
use std::fmt;
#[cfg(test)]
use std::sync::Arc;

#[derive(Debug)]
struct GateState {
    paused: bool,
    permits: usize,
    active: usize,
}

/// A live, cooperative gate shared by compiler work items.
///
/// Serial loops call [`Governor::checkpoint`]. Parallel work items call
/// [`Governor::enter`] exactly once at their outermost boundary and hold the
/// returned permit for the duration of that item. A permitted item must never
/// wait for another permitted item: doing so could deadlock when the permit
/// count is one.
#[cfg_attr(not(test), derive(Debug))]
pub struct Governor {
    state: Mutex<GateState>,
    changed: Condvar,
    #[cfg(test)]
    enter_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

#[cfg(test)]
impl fmt::Debug for Governor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Governor")
            .field("state", &*self.lock())
            .finish_non_exhaustive()
    }
}

impl Governor {
    /// Create a governor with a non-zero permit target and initial pause state.
    pub fn new(permits: usize, paused: bool) -> Self {
        assert!(permits > 0, "governor permit count must be at least one");
        Self {
            state: Mutex::new(GateState {
                paused,
                permits,
                active: 0,
            }),
            changed: Condvar::new(),
            #[cfg(test)]
            enter_hook: Mutex::new(None),
        }
    }

    fn lock(&self) -> MutexGuard<'_, GateState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Park the caller while the compiler is paused.
    pub fn checkpoint(&self) {
        let mut state = self.lock();
        while state.paused {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    /// Park while paused and report once the paused condition is observed.
    ///
    /// Tests use this rendezvous to prove a specific cooperative checkpoint
    /// blocks. The observer runs while the gate lock is held, immediately
    /// before the condition-variable wait releases it.
    #[cfg(test)]
    pub(crate) fn checkpoint_with_wait_observer(&self, observer: impl FnOnce()) {
        let mut observer = Some(observer);
        let mut state = self.lock();
        while state.paused {
            if let Some(observer) = observer.take() {
                observer();
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    /// Park while paused or at the concurrency limit, then acquire one permit.
    pub fn enter(&self) -> Permit<'_> {
        let mut state = self.lock();
        while state.paused || state.active >= state.permits {
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        state.active += 1;
        drop(state);
        let permit = Permit { governor: self };
        #[cfg(test)]
        let hook = {
            self.enter_hook
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        };
        #[cfg(test)]
        if let Some(hook) = hook {
            hook();
        }
        permit
    }

    /// Install an admission rendezvous for deterministic concurrency tests.
    #[cfg(test)]
    pub(crate) fn set_enter_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
        *self
            .enter_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(hook);
    }

    /// Change the target concurrency. Existing work is not preempted.
    pub fn set_permits(&self, permits: usize) {
        assert!(permits > 0, "governor permit count must be at least one");
        self.lock().permits = permits;
        self.changed.notify_all();
    }

    /// Pause or resume cooperative work. Resuming wakes all parked callers.
    pub fn set_paused(&self, paused: bool) {
        self.lock().paused = paused;
        self.changed.notify_all();
    }

    /// Return the current target concurrency.
    pub fn permits(&self) -> usize {
        self.lock().permits
    }

    /// Return whether cooperative work is paused.
    pub fn is_paused(&self) -> bool {
        self.lock().paused
    }
}

/// An RAII admission permit returned by [`Governor::enter`].
#[must_use = "dropping the permit releases admission for another work item"]
pub struct Permit<'a> {
    governor: &'a Governor,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut state = self.governor.lock();
        debug_assert!(state.active > 0);
        state.active -= 1;
        drop(state);
        self.governor.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    const BLOCKED_WINDOW: Duration = Duration::from_millis(50);
    const TEST_TIMEOUT: Duration = Duration::from_secs(2);

    #[test]
    fn paused_checkpoint_wakes_on_resume() {
        let governor = Arc::new(Governor::new(1, true));
        let (ready_tx, ready_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let worker_governor = Arc::clone(&governor);
        let worker = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            worker_governor.checkpoint();
            tx.send(()).unwrap();
        });

        ready_rx
            .recv_timeout(TEST_TIMEOUT)
            .expect("checkpoint worker did not become ready before timeout");
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        governor.set_paused(false);
        rx.recv_timeout(TEST_TIMEOUT)
            .expect("checkpoint worker did not resume before timeout");
        worker.join().unwrap();
    }

    #[test]
    fn paused_enter_blocks_admission_until_resume() {
        let governor = Arc::new(Governor::new(1, true));
        let (ready_tx, ready_rx) = mpsc::channel();
        let (admitted_tx, admitted_rx) = mpsc::channel();
        let worker_governor = Arc::clone(&governor);
        let worker = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let _permit = worker_governor.enter();
            admitted_tx.send(()).unwrap();
        });

        ready_rx
            .recv_timeout(TEST_TIMEOUT)
            .expect("entry worker did not become ready before timeout");
        let while_paused = admitted_rx.recv_timeout(BLOCKED_WINDOW);
        governor.set_paused(false);
        let after_resume = admitted_rx.recv_timeout(TEST_TIMEOUT);

        assert!(
            matches!(while_paused, Err(mpsc::RecvTimeoutError::Timeout)),
            "paused governor admitted an entry: {while_paused:?}"
        );
        after_resume.expect("entry worker did not wake after resume before timeout");
        worker.join().unwrap();
    }

    #[test]
    fn increasing_permits_wakes_waiting_entry() {
        let governor = Arc::new(Governor::new(1, false));
        let held = governor.enter();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let worker_governor = Arc::clone(&governor);
        let worker = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let _permit = worker_governor.enter();
            tx.send(()).unwrap();
        });

        ready_rx
            .recv_timeout(TEST_TIMEOUT)
            .expect("entry worker did not become ready before timeout");
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        governor.set_permits(2);
        rx.recv_timeout(TEST_TIMEOUT)
            .expect("entry worker did not wake after permit increase before timeout");
        drop(held);
        worker.join().unwrap();
    }

    #[test]
    fn lowering_permits_blocks_new_admission_until_active_work_drains_below_target() {
        let governor = Arc::new(Governor::new(3, false));
        let first = governor.enter();
        let second = governor.enter();
        let third = governor.enter();
        governor.set_permits(1);

        let (ready_tx, ready_rx) = mpsc::channel();
        let (admitted_tx, admitted_rx) = mpsc::channel();
        let worker_governor = Arc::clone(&governor);
        let worker = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let _permit = worker_governor.enter();
            admitted_tx.send(()).unwrap();
        });

        ready_rx
            .recv_timeout(TEST_TIMEOUT)
            .expect("entry worker did not become ready before timeout");
        let with_three_active = admitted_rx.recv_timeout(BLOCKED_WINDOW);
        drop(first);
        let with_two_active = admitted_rx.recv_timeout(BLOCKED_WINDOW);
        drop(second);
        let at_target = admitted_rx.recv_timeout(BLOCKED_WINDOW);
        drop(third);
        let below_target = admitted_rx.recv_timeout(TEST_TIMEOUT);

        assert!(
            matches!(with_three_active, Err(mpsc::RecvTimeoutError::Timeout)),
            "permit reduction preempted active work or admitted new work: {with_three_active:?}"
        );
        assert!(
            matches!(with_two_active, Err(mpsc::RecvTimeoutError::Timeout)),
            "new work was admitted while active permits exceeded the target: {with_two_active:?}"
        );
        assert!(
            matches!(at_target, Err(mpsc::RecvTimeoutError::Timeout)),
            "new work was admitted before active permits fell below the target: {at_target:?}"
        );
        below_target.expect("entry worker was not admitted after active work drained below target");
        worker.join().unwrap();
    }

    #[test]
    fn permit_drop_wakes_next_entry() {
        let governor = Arc::new(Governor::new(1, false));
        let held = governor.enter();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let worker_governor = Arc::clone(&governor);
        let worker = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let _permit = worker_governor.enter();
            tx.send(()).unwrap();
        });

        ready_rx
            .recv_timeout(TEST_TIMEOUT)
            .expect("entry worker did not become ready before timeout");
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        drop(held);
        rx.recv_timeout(TEST_TIMEOUT)
            .expect("entry worker did not wake after permit drop before timeout");
        worker.join().unwrap();
    }

    #[test]
    fn permit_is_released_when_a_gated_closure_panics() {
        let governor = Arc::new(Governor::new(1, false));
        let worker_governor = Arc::clone(&governor);
        let worker = thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _permit = worker_governor.enter();
                panic!("intentional panic while holding a gated permit");
            }));
            assert!(result.is_err());
        });
        worker
            .join()
            .expect("worker thread should not propagate the panic (caught by catch_unwind)");

        // If the permit leaked during unwind, this enter() would hang.
        let _permit = governor.enter();
    }

    #[test]
    fn clearing_pause_wakes_checkpoint_and_enter_waiters_together() {
        let governor = Arc::new(Governor::new(1, true));

        let (checkpoint_ready_tx, checkpoint_ready_rx) = mpsc::channel();
        let (checkpoint_tx, checkpoint_rx) = mpsc::channel();
        let checkpoint_governor = Arc::clone(&governor);
        let checkpoint_worker = thread::spawn(move || {
            checkpoint_ready_tx.send(()).unwrap();
            checkpoint_governor.checkpoint();
            checkpoint_tx.send(()).unwrap();
        });

        let (enter_ready_tx, enter_ready_rx) = mpsc::channel();
        let (enter_tx, enter_rx) = mpsc::channel();
        let enter_governor = Arc::clone(&governor);
        let enter_worker = thread::spawn(move || {
            enter_ready_tx.send(()).unwrap();
            let _permit = enter_governor.enter();
            enter_tx.send(()).unwrap();
        });

        checkpoint_ready_rx
            .recv_timeout(TEST_TIMEOUT)
            .expect("checkpoint worker did not become ready before timeout");
        enter_ready_rx
            .recv_timeout(TEST_TIMEOUT)
            .expect("entry worker did not become ready before timeout");

        let checkpoint_while_paused = checkpoint_rx.recv_timeout(BLOCKED_WINDOW);
        let enter_while_paused = enter_rx.recv_timeout(BLOCKED_WINDOW);
        assert!(
            matches!(
                checkpoint_while_paused,
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "checkpoint worker made progress while paused: {checkpoint_while_paused:?}"
        );
        assert!(
            matches!(enter_while_paused, Err(mpsc::RecvTimeoutError::Timeout)),
            "entry worker made progress while paused: {enter_while_paused:?}"
        );

        governor.set_paused(false);

        checkpoint_rx
            .recv_timeout(TEST_TIMEOUT)
            .expect("checkpoint worker did not wake after a single resume before timeout");
        enter_rx
            .recv_timeout(TEST_TIMEOUT)
            .expect("entry worker did not wake after a single resume before timeout");

        checkpoint_worker.join().unwrap();
        enter_worker.join().unwrap();
    }

    #[test]
    fn raising_permits_while_paused_does_not_admit_until_resumed() {
        let governor = Arc::new(Governor::new(1, true));
        let (ready_tx, ready_rx) = mpsc::channel();
        let (admitted_tx, admitted_rx) = mpsc::channel();
        let worker_governor = Arc::clone(&governor);
        let worker = thread::spawn(move || {
            ready_tx.send(()).unwrap();
            let _permit = worker_governor.enter();
            admitted_tx.send(()).unwrap();
        });

        ready_rx
            .recv_timeout(TEST_TIMEOUT)
            .expect("entry worker did not become ready before timeout");

        governor.set_permits(4);
        let while_paused = admitted_rx.recv_timeout(BLOCKED_WINDOW);
        assert!(
            matches!(while_paused, Err(mpsc::RecvTimeoutError::Timeout)),
            "raising permits admitted a waiter while still paused: {while_paused:?}"
        );

        governor.set_paused(false);
        admitted_rx
            .recv_timeout(TEST_TIMEOUT)
            .expect("entry worker was not admitted after resume before timeout");
        worker.join().unwrap();
    }
}
