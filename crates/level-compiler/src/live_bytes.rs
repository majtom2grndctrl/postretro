// Test-only live-bytes ledger: what a bake holds, by capacity, through its phases.
// See: context/lib/testing_guide.md §3 (Resource bounds)

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Default)]
struct State {
    live: u64,
    peak: u64,
    window_peak: u64,
    by_kind: BTreeMap<&'static str, u64>,
    at_peak: BTreeMap<&'static str, u64>,
    phases: Vec<Phase>,
}

type Ledger = Arc<Mutex<State>>;

fn lock(ledger: &Ledger) -> MutexGuard<'_, State> {
    ledger
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

thread_local! {
    static ACTIVE: RefCell<Option<Ledger>> = const { RefCell::new(None) };
}

fn active() -> Option<Ledger> {
    ACTIVE.with(|active| active.borrow().clone())
}

/// One phase boundary: the live bytes at it, and the most held between the
/// previous boundary and this one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Phase {
    pub(crate) label: &'static str,
    pub(crate) live: u64,
    pub(crate) peak: u64,
}

/// What one measured run held.
#[derive(Clone, Debug)]
pub(crate) struct Report {
    pub(crate) peak: u64,
    /// The live set by kind at the moment of the peak.
    pub(crate) at_peak: BTreeMap<&'static str, u64>,
    pub(crate) phases: Vec<Phase>,
    /// Bytes still held when the run returned: what left it.
    pub(crate) live_at_end: u64,
}

/// One routed allocation, sized by capacity: counted live from creation
/// until drop. A guard made on a thread with no ledger counts nothing. The
/// ledger proves only allocations routed through it.
#[derive(Default)]
pub(crate) struct Held {
    kind: &'static str,
    bytes: u64,
    ledger: Option<Ledger>,
}

impl Held {
    fn on(ledger: Option<Ledger>, kind: &'static str, bytes: u64) -> Self {
        if let Some(ledger) = &ledger {
            let mut state = lock(ledger);
            state.live += bytes;
            *state.by_kind.entry(kind).or_default() += bytes;
            if state.live > state.peak {
                state.peak = state.live;
                state.at_peak = state.by_kind.clone();
            }
            state.window_peak = state.window_peak.max(state.live);
        }
        Self {
            kind,
            bytes,
            ledger,
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if let Some(ledger) = &self.ledger {
            let mut state = lock(ledger);
            state.live -= self.bytes;
            *state.by_kind.entry(self.kind).or_default() -= self.bytes;
        }
    }
}

impl Clone for Held {
    fn clone(&self) -> Self {
        Self::on(self.ledger.clone(), self.kind, self.bytes)
    }
}

/// Guards never make two values unequal.
impl PartialEq for Held {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl fmt::Debug for Held {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Held({} {} bytes)", self.kind, self.bytes)
    }
}

/// Count `bytes` of `kind` live on this thread's ledger until the guard
/// drops.
pub(crate) fn hold(kind: &'static str, bytes: u64) -> Held {
    Held::on(active(), kind, bytes)
}

/// [`hold`] a vector's allocation, by capacity.
pub(crate) fn hold_vec<T>(kind: &'static str, vec: &Vec<T>) -> Held {
    hold(kind, (vec.capacity() * size_of::<T>()) as u64)
}

/// Mark a phase boundary on this thread's ledger.
pub(crate) fn phase(label: &'static str) {
    if let Some(ledger) = active() {
        let mut state = lock(&ledger);
        let phase = Phase {
            label,
            live: state.live,
            peak: state.window_peak,
        };
        state.phases.push(phase);
        state.window_peak = state.live;
    }
}

/// Run `work` on a fresh pool of `threads` workers with one ledger across
/// them, and report what it held. The pool runs no other test's work.
pub(crate) fn measured<R: Send>(threads: usize, work: impl FnOnce() -> R + Send) -> (R, Report) {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .expect("measuring pool");
    let ledger: Ledger = Arc::default();
    pool.broadcast(|_| ACTIVE.with(|active| *active.borrow_mut() = Some(Arc::clone(&ledger))));
    let result = pool.install(work);
    pool.broadcast(|_| ACTIVE.with(|active| active.borrow_mut().take()));
    let state = lock(&ledger);
    let report = Report {
        peak: state.peak,
        at_peak: state.at_peak.clone(),
        phases: state.phases.clone(),
        live_at_end: state.live,
    };
    (result, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ledger_tracks_live_bytes_and_the_peak_per_phase() {
        let ((), report) = measured(2, || {
            let a = hold("a", 100);
            phase("start");
            let b = hold("b", 50);
            drop(a);
            let c = b.clone();
            phase("middle");
            drop((b, c));
        });
        assert_eq!(report.peak, 150);
        assert_eq!(report.at_peak, BTreeMap::from([("a", 100), ("b", 50)]));
        assert_eq!(
            report.phases,
            vec![
                Phase {
                    label: "start",
                    live: 100,
                    peak: 100
                },
                Phase {
                    label: "middle",
                    live: 100,
                    peak: 150
                },
            ]
        );
        assert_eq!(report.live_at_end, 0);
        // Outside a measured pool nothing is counted.
        let outside = hold("outside", 7);
        assert!(outside.ledger.is_none());
    }
}
