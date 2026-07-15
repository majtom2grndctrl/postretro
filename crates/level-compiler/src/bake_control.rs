//! Display-only progress and cooperative admission state for instrumented bakes.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::governor::Governor;
use crate::reporter::StageProgress;

/// Shared handles carried to an outermost bake work-item boundary.
#[derive(Clone, Debug)]
pub struct BakeControl {
    governor: Arc<Governor>,
    completed: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
}

impl BakeControl {
    pub fn new(governor: Arc<Governor>, progress: &StageProgress) -> Self {
        Self {
            governor,
            completed: progress.completed_handle(),
            total: progress.total_handle(),
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
        self.total.store(total, Ordering::Release);
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
}
