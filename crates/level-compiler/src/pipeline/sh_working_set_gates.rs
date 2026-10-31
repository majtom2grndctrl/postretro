// SH working-set gates, checked cheapest prediction first.
// See: context/lib/build_pipeline.md §Compiler pipeline

use super::delta_working_set::{
    DeltaWorkingSetGateError, DeltaWorkingSetProjection, log_delta_working_set_projection,
};
use crate::surface_radiance::ShBaseWorkingSetProjection;

/// Check the SH-delta and SH-base projections against their shared budget
/// in ascending predicted bytes (the delta gate first on a tie), so a
/// budget both fail refuses by the cheaper gate's name. Both predictions
/// are made before either is checked, and before the material stage, the
/// static emitter tests and any SH ray.
pub(super) fn admit_sh_working_sets(
    delta: DeltaWorkingSetProjection,
    base: &ShBaseWorkingSetProjection,
    verbose: bool,
) -> anyhow::Result<()> {
    if base.estimated_peak_bytes < delta.estimated_peak_bytes {
        admit_base(base)?;
        admit_delta(delta, verbose)
    } else {
        admit_delta(delta, verbose)?;
        admit_base(base)
    }
}

fn admit_delta(projection: DeltaWorkingSetProjection, verbose: bool) -> anyhow::Result<()> {
    match projection.admit() {
        Ok(projection) => {
            log_delta_working_set_projection(&projection, verbose);
            Ok(())
        }
        Err(error) => {
            if let DeltaWorkingSetGateError::BudgetExceeded(projection) = &error {
                log::info!(
                    "[Compiler] SH delta working-set gate: estimated peak {} bytes, budget {} bytes, refused",
                    projection.estimated_peak_bytes,
                    projection.budget_bytes,
                );
            }
            Err(anyhow::Error::new(error))
        }
    }
}

fn admit_base(projection: &ShBaseWorkingSetProjection) -> anyhow::Result<()> {
    let verdict = if projection.admit().is_ok() {
        "admitted"
    } else {
        "refused"
    };
    log::info!(
        "[Compiler] SH base working-set gate: estimated peak {} bytes, budget {} bytes, {verdict} ({projection})",
        projection.estimated_peak_bytes,
        projection.budget_bytes,
    );
    projection.admit().map_err(anyhow::Error::new)
}
