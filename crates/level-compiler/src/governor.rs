//! Cooperative pause and parallel-work admission control for compiler bakes.

use std::sync::{Condvar, Mutex, MutexGuard};

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
#[derive(Debug)]
pub struct Governor {
    state: Mutex<GateState>,
    changed: Condvar,
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
        Permit { governor: self }
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

        ready_rx.recv().unwrap();
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        governor.set_paused(false);
        rx.recv().unwrap();
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

        ready_rx.recv().unwrap();
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        governor.set_permits(2);
        rx.recv().unwrap();
        drop(held);
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

        ready_rx.recv().unwrap();
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        drop(held);
        rx.recv().unwrap();
        worker.join().unwrap();
    }
}
