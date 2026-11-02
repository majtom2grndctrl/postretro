// Coverage proof run after atlas preparation, over the faces the lightmap walk traces.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use crate::coverage_proof::{CoverageReport, prove_coverage};
use crate::geometry::GeometryResult;

/// Prove the post-cut geometry's coverage and log one info line. Hole
/// warnings and the reach cull's switch read the returned report.
pub(super) fn run_coverage_proof(
    geometry: &GeometryResult,
    face_brushes: &[Option<u32>],
) -> CoverageReport {
    let report = prove_coverage(geometry, face_brushes);
    log::info!(
        "[Compiler] Coverage proof: {} of {} edge(s) proven, {} unproven span(s) ({:.3} m) in {:.3} s",
        report.edges_proven,
        report.edges,
        report.unproven.len(),
        report.unproven_length(),
        report.elapsed.as_secs_f64(),
    );
    report
}
