//! Terminal-free formatting for the active stage's progress footer.
//! See: context/lib/build_pipeline.md

use std::time::Duration;

use super::StepState;

pub(super) fn progress_text(step: &StepState) -> (String, String) {
    let Some(progress) = &step.progress else {
        return ("working".to_owned(), "Stage ETA  --".to_owned());
    };
    let Some(total) = progress.total() else {
        return ("working  --%".to_owned(), "Stage ETA  --".to_owned());
    };
    let done = progress.completed().min(total);
    let percent = done.saturating_mul(100).checked_div(total).unwrap_or(100);
    let elapsed = step
        .started
        .map_or(Duration::ZERO, |started| started.elapsed());
    let eta = if total == 0 {
        Some(Duration::ZERO)
    } else if done == 0 {
        None
    } else {
        Some(elapsed.mul_f64((total - done) as f64 / done as f64))
    };
    (
        format!("{done}/{total}  {percent:>3}%"),
        eta.map_or_else(
            || "Stage ETA  --".to_owned(),
            |eta| format!("Stage ETA  {:.1}s", eta.as_secs_f64()),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{StageDescriptor, StageId};
    use crate::reporter::StageProgress;

    #[test]
    fn published_zero_work_is_complete() {
        let mut step = StepState::new(StageDescriptor {
            id: StageId::DeltaShBake,
            label: StageId::DeltaShBake.label(),
            predicted_present: true,
        });
        step.progress = Some(StageProgress::with_total(0));
        step.started = Some(std::time::Instant::now());

        assert_eq!(
            progress_text(&step),
            ("0/0  100%".to_owned(), "Stage ETA  0.0s".to_owned())
        );
    }
}
