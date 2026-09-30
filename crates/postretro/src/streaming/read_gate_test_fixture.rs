//! Test-only hold-by-offset gate for fault-injected issuer routes and sources.
//! See: context/lib/testing_guide.md

use std::collections::BTreeSet;
use std::sync::{Condvar, Mutex};

/// Physical reads starting at a held offset block inside the reader until
/// the test releases that offset. Lets a test pin the issuer inside one read
/// while it queues work, or hold back one half of a pair.
#[derive(Debug, Default)]
pub(crate) struct ReadGate {
    held: Mutex<BTreeSet<u64>>,
    gate: Condvar,
}

impl ReadGate {
    pub(crate) fn hold(&self, offset: u64) {
        self.held.lock().unwrap().insert(offset);
    }

    pub(crate) fn release(&self, offset: u64) {
        self.held.lock().unwrap().remove(&offset);
        self.gate.notify_all();
    }

    /// Opens every hold; safe to call while unwinding from a failed test.
    pub(crate) fn release_all(&self) {
        self.held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.gate.notify_all();
    }

    /// Called by a reader: blocks while a read at `offset` is held.
    pub(crate) fn wait_while_held(&self, offset: u64) {
        let mut held = self.held.lock().unwrap();
        while held.contains(&offset) {
            held = self.gate.wait(held).unwrap();
        }
    }
}
