//! Display-only progress and cooperative admission state for instrumented bakes.
//! See: `context/lib/build_pipeline.md`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::governor::Governor;
use crate::reporter::StageProgress;

/// Shared state carried to an outermost bake work-item boundary: the admission
/// governor plus the stage's `StageProgress`. Holding the whole `StageProgress`
/// (its Arcs shared by `Clone`) keeps `publish_total` delegating to the reporter,
/// so the total/published store-ordering contract lives in exactly one place.
/// `completed` is cached separately so the hot `advance` path is a lone
/// `fetch_add` with no per-call Arc clone.
#[derive(Clone, Debug)]
pub struct BakeControl {
    governor: Arc<Governor>,
    progress: StageProgress,
    completed: Arc<AtomicUsize>,
}

impl BakeControl {
    pub fn new(governor: Arc<Governor>, progress: &StageProgress) -> Self {
        Self {
            governor,
            progress: progress.clone(),
            completed: progress.completed_handle(),
        }
    }

    /// Control used by compatibility entry points and unit tests that do not
    /// observe progress. It never throttles practical bake concurrency.
    pub fn unrestricted() -> Self {
        let progress = StageProgress::indeterminate();
        Self::new(Arc::new(Governor::new(usize::MAX, false)), &progress)
    }

    pub fn governor(&self) -> &Governor {
        &self.governor
    }

    pub fn publish_total(&self, total: usize) {
        self.progress.publish_total(total);
    }

    pub fn advance(&self, units: usize) {
        self.completed.fetch_add(units, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_updates_the_reporter_handles() {
        let progress = StageProgress::indeterminate();
        let control = BakeControl::new(Arc::new(Governor::new(1, false)), &progress);

        control.publish_total(7);
        control.advance(3);

        assert_eq!(progress.total(), Some(7));
        assert_eq!(progress.completed(), 3);
    }

    #[test]
    fn control_can_publish_zero_work() {
        let progress = StageProgress::indeterminate();
        let control = BakeControl::new(Arc::new(Governor::new(1, false)), &progress);

        control.publish_total(0);

        assert_eq!(progress.total(), Some(0));
    }
}
