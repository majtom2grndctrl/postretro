//! Terminal-free formatting for the active stage's progress footer.

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
    let percent = done.saturating_mul(100) / total;
    let elapsed = step
        .started
        .map_or(Duration::ZERO, |started| started.elapsed());
    let eta = if done == 0 {
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
