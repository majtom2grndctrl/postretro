// A done/total work counter a loader advances on its own thread while another
// thread reads the fraction. Units are the loader's choice (the PRL loader
// counts bytes); only the ratio is published.
// See: context/lib/boot_sequence.md §2

use std::sync::Mutex;

/// Thread-safe load progress, shared by `Arc` between the thread doing the work
/// and any thread displaying it.
///
/// The reported [`fraction`](Self::fraction) never decreases for one load:
/// `begin` fixes the planned total before any work is credited, after which
/// work is only credited (`advance`) or dropped from the plan (`forgo`) — both
/// raise the ratio. `finish` marks the load complete at `done == total`.
#[derive(Debug, Default)]
pub struct LoadProgress {
    counts: Mutex<Counts>,
    /// Every `(done, total)` the counter passed through, so a test can check
    /// the ratio rose monotonically across a whole load.
    #[cfg(test)]
    history: Mutex<Vec<(u64, u64)>>,
}

#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    done: u64,
    total: u64,
    finished: bool,
    #[cfg(test)]
    began: bool,
}

impl LoadProgress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Plan `total` units of work. Call once, before any work is credited.
    pub fn begin(&self, total: u64) {
        self.update(|counts| {
            #[cfg(test)]
            {
                debug_assert!(!counts.began, "a load plans its total once");
                counts.began = true;
            }
            counts.total = total;
            counts.done = 0;
            counts.finished = false;
        });
    }

    /// Credit `units` of planned work as done. Clamped to the plan.
    pub fn advance(&self, units: u64) {
        self.update(|counts| {
            counts.done = counts.done.saturating_add(units).min(counts.total);
        });
    }

    /// Drop `units` of planned work that will not happen after all. The plan
    /// never shrinks below the work already done.
    pub fn forgo(&self, units: u64) {
        self.update(|counts| {
            counts.total = counts.total.saturating_sub(units).max(counts.done);
        });
    }

    /// Mark the load complete: whatever planned work was neither credited nor
    /// forgone is dropped, so `done == total`.
    pub fn finish(&self) {
        self.update(|counts| {
            counts.total = counts.done;
            counts.finished = true;
        });
    }

    /// `(done, total)` units.
    pub fn counts(&self) -> (u64, u64) {
        let counts = self.lock();
        (counts.done, counts.total)
    }

    /// Completed share of the planned work in `[0, 1]`. A finished load reads
    /// 1 even when it planned nothing; an unstarted one reads 0.
    pub fn fraction(&self) -> f32 {
        let counts = self.lock();
        if counts.finished {
            return 1.0;
        }
        if counts.total == 0 {
            return 0.0;
        }
        (counts.done as f64 / counts.total as f64) as f32
    }

    fn update(&self, change: impl FnOnce(&mut Counts)) {
        let mut counts = self.lock();
        change(&mut counts);
        #[cfg(test)]
        self.history
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((counts.done, counts.total));
    }

    #[cfg(test)]
    pub(crate) fn history(&self) -> Vec<(u64, u64)> {
        self.history
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// A poisoned lock only means a reader or writer panicked mid-update; the
    /// counts are plain integers and stay meaningful.
    fn lock(&self) -> std::sync::MutexGuard<'_, Counts> {
        self.counts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credits_and_forgone_work_only_raise_the_fraction() {
        let progress = LoadProgress::new();
        assert_eq!(progress.fraction(), 0.0);
        progress.begin(100);
        progress.advance(25);
        assert_eq!(progress.fraction(), 0.25);
        progress.forgo(50);
        assert_eq!(progress.counts(), (25, 50));
        assert_eq!(progress.fraction(), 0.5);
        // Neither call may push past the plan or below the work done.
        progress.advance(500);
        assert_eq!(progress.counts(), (50, 50));
        progress.forgo(500);
        assert_eq!(progress.counts(), (50, 50));
    }

    #[test]
    fn finish_settles_done_at_total() {
        let progress = LoadProgress::new();
        progress.begin(10);
        progress.advance(4);
        progress.finish();
        assert_eq!(progress.counts(), (4, 4));
        assert_eq!(progress.fraction(), 1.0);

        let empty = LoadProgress::new();
        empty.begin(0);
        empty.finish();
        assert_eq!(empty.fraction(), 1.0);
    }
}
