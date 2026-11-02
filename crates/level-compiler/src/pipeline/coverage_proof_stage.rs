// Coverage proof run after atlas preparation, over the faces the lightmap walk
// traces, and the hole warnings it gives the author.
// See: context/lib/build_pipeline.md §Compiler pipeline step 12 (Exact skips)

use crate::coverage_proof::{CoverageReport, prove_coverage};
use crate::geometry::GeometryResult;
use crate::governor::Governor;
use crate::map_data::MapAssembly;

use super::watertight_open_edge_provenance;

/// Prove the post-cut geometry's coverage and log one info line. Hole
/// warnings and the reach cull's switch read the returned report. The
/// caller holds no governor permit.
pub(super) fn run_coverage_proof(
    geometry: &GeometryResult,
    face_brushes: &[Option<u32>],
    governor: &Governor,
) -> CoverageReport {
    let report = prove_coverage(geometry, face_brushes, governor);
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

/// Warn about every hole the coverage proof found: the only author-facing
/// hole warning. Each sample names its location and brush, up to the sample
/// cap, after the total. Runs every build; never fails it.
pub(super) fn warn_coverage_holes(
    report: &CoverageReport,
    brush_assembly: &[Option<usize>],
    assemblies: &[MapAssembly],
) {
    if report.unproven.is_empty() {
        return;
    }
    let samples = report.samples();
    log::warn!(
        "[Compiler] Coverage proof: {} hole(s) in world geometry ({:.3} m of face edge no \
         face closes within the tracer's tolerance). The build still succeeds. Locations \
         (world-space meters, showing {} of {}):",
        report.unproven.len(),
        report.unproven_length(),
        samples.len(),
        report.unproven.len(),
    );
    for span in samples {
        let at = span.midpoint;
        let brush = match span.open_edge() {
            Some(edge) => {
                match watertight_open_edge_provenance(&edge, brush_assembly, assemblies) {
                    Some(group) => format!("brush {} in group {group}", edge.brush_index),
                    None => format!("brush {}", edge.brush_index),
                }
            }
            None => "no source brush".to_string(),
        };
        log::warn!(
            "[Compiler]   hole near ({:.3}, {:.3}, {:.3}), {:.3} m long — {brush}",
            at.x,
            at.y,
            at.z,
            span.length,
        );
    }
}
