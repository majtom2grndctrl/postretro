// A done/total work counter a loader advances on its own thread while another
// thread reads the fraction, and the phase-weighted counter for a whole level
// load. Units are each phase's choice (the PRL parse counts bytes); only the
// ratios are published.
// See: context/lib/boot_sequence.md §2

use std::sync::{Arc, Mutex};

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
    /// `begin` (or the first `credit_remaining`) fixed the plan.
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
            debug_assert!(!counts.began, "a load plans its total once");
            counts.began = true;
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

    /// Credit work measured as a shortfall: `remaining` units are still
    /// outstanding. The first call plans `remaining` as the total; after
    /// that, a smaller shortfall credits the difference and a larger one
    /// credits nothing, so a shortfall that grows stalls the fraction rather
    /// than lowering it. For phases whose work is "until this set settles".
    pub fn credit_remaining(&self, remaining: u64) {
        self.update(|counts| {
            if !counts.began {
                counts.began = true;
                counts.total = remaining;
                counts.done = 0;
                return;
            }
            let earned = counts.total.saturating_sub(remaining);
            counts.done = counts.done.max(earned).min(counts.total);
        });
    }

    /// Whether [`finish`](Self::finish) has run.
    pub fn is_finished(&self) -> bool {
        self.lock().finished
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

/// One phase of a level load, in the order a load runs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoadPhase {
    /// The PRL parse. Units: bytes the parse reads.
    Parse,
    /// CPU preparation of the parse's hand-outs. Units: the caller's (prep
    /// tasks finished, say).
    Prep,
    /// GPU resource creation and writes. Units: the caller's (the level
    /// set build's parts finished, as the app credits them).
    Upload,
    /// Holding the installed level until its streamed sets settle. Units: the
    /// caller's (the settle-set shortfall via
    /// [`LoadProgress::credit_remaining`], say).
    Settling,
}

impl LoadPhase {
    pub const ALL: [LoadPhase; 4] = [
        LoadPhase::Parse,
        LoadPhase::Prep,
        LoadPhase::Upload,
        LoadPhase::Settling,
    ];

    /// This phase's share of the whole load's bar.
    pub const fn weight(self) -> f64 {
        match self {
            LoadPhase::Parse => LOAD_PHASE_WEIGHT_PARSE,
            LoadPhase::Prep => LOAD_PHASE_WEIGHT_PREP,
            LoadPhase::Upload => LOAD_PHASE_WEIGHT_UPLOAD,
            LoadPhase::Settling => LOAD_PHASE_WEIGHT_SETTLING,
        }
    }

    const fn index(self) -> usize {
        match self {
            LoadPhase::Parse => 0,
            LoadPhase::Prep => 1,
            LoadPhase::Upload => 2,
            LoadPhase::Settling => 3,
        }
    }
}

/// Phase weights: coarse, and a tuning number reported with the Metal run.
pub const LOAD_PHASE_WEIGHT_PARSE: f64 = 0.40;
pub const LOAD_PHASE_WEIGHT_PREP: f64 = 0.15;
pub const LOAD_PHASE_WEIGHT_UPLOAD: f64 = 0.15;
pub const LOAD_PHASE_WEIGHT_SETTLING: f64 = 0.30;

/// The largest fraction an unfinished load reports: strictly below 1.0 in
/// `f32`, so only a load whose every phase finished reads exactly 1.0.
const UNFINISHED_CEILING: f32 = 1.0 - f32::EPSILON;

/// Progress of one whole level load: a [`LoadProgress`] per [`LoadPhase`],
/// combined by the phase weights.
///
/// Each phase credits real units of its own work on its own counter, from
/// whichever thread does that work. The combined [`fraction`](Self::fraction)
/// is the weighted sum of the phases' fractions, so it never decreases while
/// every phase's own fraction never decreases. It stays below 1.0 until every
/// phase has finished and reads exactly 1.0 after. A phase with no work
/// finishes without planning anything (`phase.finish()`), which counts as
/// fully done: a near-empty level or an empty prep set still reaches 1.0.
#[derive(Debug, Default)]
pub struct LevelLoadProgress {
    phases: [Arc<LoadProgress>; 4],
}

impl LevelLoadProgress {
    pub fn new() -> Self {
        Self::default()
    }

    /// The counter `phase` credits. Shared by `Arc` with the thread doing
    /// that phase's work; the parse takes [`LoadPhase::Parse`]'s.
    pub fn phase(&self, phase: LoadPhase) -> &Arc<LoadProgress> {
        &self.phases[phase.index()]
    }

    /// Whether every phase has finished.
    pub fn is_finished(&self) -> bool {
        self.phases.iter().all(|phase| phase.is_finished())
    }

    /// Weighted completed share of the whole load in `[0, 1]`: below 1.0
    /// until every phase finishes, exactly 1.0 after.
    pub fn fraction(&self) -> f32 {
        if self.is_finished() {
            return 1.0;
        }
        let weighted: f64 = LoadPhase::ALL
            .into_iter()
            .map(|phase| phase.weight() * f64::from(self.phase(phase).fraction()))
            .sum();
        (weighted as f32).clamp(0.0, UNFINISHED_CEILING)
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

    #[test]
    fn credit_remaining_plans_on_first_call_and_stalls_when_the_shortfall_grows() {
        let progress = LoadProgress::new();
        progress.credit_remaining(4);
        assert_eq!(progress.counts(), (0, 4));
        progress.credit_remaining(1);
        assert_eq!(progress.counts(), (3, 4));
        // A grown shortfall credits nothing and takes nothing back.
        progress.credit_remaining(3);
        assert_eq!(progress.counts(), (3, 4));
        progress.credit_remaining(0);
        assert_eq!(progress.fraction(), 1.0);
        assert!(!progress.is_finished());
    }

    #[test]
    fn level_load_progress_weights_phases_and_reads_one_only_when_all_finish() {
        let total: f64 = LoadPhase::ALL.into_iter().map(LoadPhase::weight).sum();
        assert!(
            (total - 1.0).abs() < 1e-12,
            "the weights cover the whole bar"
        );

        let load = LevelLoadProgress::new();
        assert_eq!(load.fraction(), 0.0);
        load.phase(LoadPhase::Parse).begin(10);
        load.phase(LoadPhase::Parse).advance(5);
        assert!((load.fraction() - 0.20).abs() < 1e-6);
        load.phase(LoadPhase::Parse).finish();
        load.phase(LoadPhase::Prep).finish();
        load.phase(LoadPhase::Upload).finish();
        // Settling's units all credited but the phase not finished: below 1.
        load.phase(LoadPhase::Settling).credit_remaining(2);
        load.phase(LoadPhase::Settling).credit_remaining(0);
        assert!(load.fraction() < 1.0);
        assert!(!load.is_finished());
        load.phase(LoadPhase::Settling).finish();
        assert_eq!(load.fraction(), 1.0);
    }

    #[test]
    fn empty_phases_count_as_fully_done() {
        let load = LevelLoadProgress::new();
        for phase in LoadPhase::ALL {
            load.phase(phase).finish();
        }
        assert!(load.is_finished());
        assert_eq!(load.fraction(), 1.0);
    }
}
