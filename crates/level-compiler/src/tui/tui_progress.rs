//! Terminal-free formatting and timing for the active stage's progress footer.
//! See: context/lib/build_pipeline.md

use std::time::{Duration, Instant};

use super::StepState;

#[derive(Clone, Debug, Default)]
pub(super) struct RemainingEstimate {
    last_sample: Option<(usize, Option<usize>)>,
    projected_remaining: Option<Duration>,
}

impl RemainingEstimate {
    /// Recalibrate only when the progress sample changes. A stalled stage keeps
    /// its most recent projection instead of counting down without progress.
    pub(super) fn observe(
        &mut self,
        now: Instant,
        started: Instant,
        completed: usize,
        total: Option<usize>,
    ) {
        let sample = (completed, total);
        if self.last_sample == Some(sample) {
            return;
        }
        self.last_sample = Some(sample);
        self.projected_remaining = total.and_then(|total| {
            let completed = completed.min(total);
            if total == 0 || completed == total {
                Some(Duration::ZERO)
            } else if completed == 0 {
                None
            } else {
                Some(
                    now.saturating_duration_since(started)
                        .mul_f64((total - completed) as f64 / completed as f64),
                )
            }
        });
    }

    fn remaining(&self) -> Option<Duration> {
        self.projected_remaining
    }
}

pub(super) fn progress_text(step: &StepState) -> (String, String) {
    const LABEL: &str = "Est. remaining";

    let Some(progress) = &step.progress else {
        return ("working".to_owned(), format!("{LABEL}  --"));
    };
    let Some(total) = progress.total() else {
        return ("working  --%".to_owned(), format!("{LABEL}  --"));
    };
    let done = progress.completed().min(total);
    let percent = done.saturating_mul(100).checked_div(total).unwrap_or(100);
    let remaining = if total == 0 {
        Some(Duration::ZERO)
    } else {
        step.remaining_estimate.remaining()
    };
    (
        format!("{done}/{total}  {percent:>3}%"),
        remaining.map_or_else(
            || format!("{LABEL}  --"),
            |remaining| format!("{LABEL}  {:.1}s", remaining.as_secs_f64()),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{StageDescriptor, StageId};
    use crate::reporter::StageProgress;
    use std::sync::atomic::Ordering;

    fn step_with_progress(progress: StageProgress, started: Instant) -> StepState {
        let mut step = StepState::new(StageDescriptor {
            id: StageId::DeltaShBake,
            label: StageId::DeltaShBake.label(),
            predicted_present: true,
        });
        step.progress = Some(progress);
        step.started = Some(started);
        step
    }

    #[test]
    fn stalled_counter_keeps_estimated_remaining_frozen() {
        let started = Instant::now();
        let sampled = started + Duration::from_secs(10);
        let progress = StageProgress::with_total(10);
        progress.completed_handle().store(5, Ordering::Relaxed);
        let mut step = step_with_progress(progress, started);
        step.remaining_estimate
            .observe(sampled, started, 5, Some(10));

        assert_eq!(progress_text(&step).1, "Est. remaining  10.0s");

        step.remaining_estimate
            .observe(sampled + Duration::from_secs(30), started, 5, Some(10));
        assert_eq!(progress_text(&step).1, "Est. remaining  10.0s");
    }

    #[test]
    fn new_work_samples_can_recalibrate_slower_or_faster() {
        let started = Instant::now();
        let mut estimate = RemainingEstimate::default();
        estimate.observe(started + Duration::from_secs(10), started, 5, Some(10));
        assert_eq!(estimate.remaining(), Some(Duration::from_secs(10)));

        estimate.observe(started + Duration::from_secs(20), started, 6, Some(10));
        let slower = estimate.remaining().unwrap();
        assert!(slower > Duration::from_secs(13));

        estimate.observe(started + Duration::from_secs(21), started, 9, Some(10));
        let faster = estimate.remaining().unwrap();
        assert!(faster < Duration::from_secs(3));
    }

    #[test]
    fn late_total_and_progress_regression_recalibrate_safely() {
        let started = Instant::now();
        let mut estimate = RemainingEstimate::default();
        estimate.observe(started + Duration::from_secs(5), started, 3, None);
        assert_eq!(estimate.remaining(), None);

        estimate.observe(started + Duration::from_secs(6), started, 3, Some(6));
        assert_eq!(estimate.remaining(), Some(Duration::from_secs(6)));

        estimate.observe(started + Duration::from_secs(8), started, 2, Some(6));
        assert_eq!(estimate.remaining(), Some(Duration::from_secs(16)));

        estimate.observe(started + Duration::from_secs(9), started, 7, Some(6));
        assert_eq!(estimate.remaining(), Some(Duration::ZERO));
    }

    #[test]
    fn indeterminate_progress_has_no_estimated_remaining() {
        let started = Instant::now();
        let progress = StageProgress::indeterminate();
        let mut step = step_with_progress(progress, started);
        step.remaining_estimate.observe(started, started, 0, None);

        assert_eq!(
            progress_text(&step),
            ("working  --%".to_owned(), "Est. remaining  --".to_owned())
        );
    }

    #[test]
    fn published_zero_work_is_complete() {
        let started = Instant::now();
        let progress = StageProgress::with_total(0);
        let mut step = step_with_progress(progress, started);
        step.remaining_estimate
            .observe(started, started, 0, Some(0));

        assert_eq!(
            progress_text(&step),
            ("0/0  100%".to_owned(), "Est. remaining  0.0s".to_owned())
        );
    }
}
