//! Printed tables for the sampled visible-set pass: sampling diagnostics,
//! sightlines, the density convergence check, and mandatory bytes per lead.

use std::fmt::Write as _;

use super::pvs_sampling::{
    CUBE_FACE_FOV_DEGREES, LATTICE_FRACTIONS, LATTICE_POINTS, RUNTIME_MAX_FOV_DEGREES,
    SampleDensity,
};
use super::render::{LOW_TIER_BUDGET_MIB, MetricSummary, mib_f64, percentile_desc};
use super::report::DryRunReport;
use super::visible_set::VisibleSetResult;

impl DryRunReport {
    pub(super) fn render_visible_set(
        &self,
        out: &mut String,
        visible: &VisibleSetResult,
        names: &[String],
    ) {
        let stats = &visible.stats;
        let _ = writeln!(
            out,
            "\n-- sampled visibility: {} eye points x 6 cube faces ({CUBE_FACE_FOV_DEGREES}° each) per camera cell --",
            LATTICE_POINTS
        );
        let _ = writeln!(
            out,
            "note: runtime portal walks from sampled eye points; a sample only misses cells, so every \
             PVS, set, byte figure and sightline below is a lower bound on true visibility, per \
             cell volume (a free-fly camera; eye points span {:.0}-{:.0}% of each cell's AABB on \
             every axis, ceiling bands included), not per standing-player eye; the cube faces \
             tile every view direction, so it holds for any FOV up to the \
             {RUNTIME_MAX_FOV_DEGREES}° maximum; a cell with no eye point keeps the set {{cell}} \
             alone, a much weaker bound",
            LATTICE_FRACTIONS[0] * 100.0,
            LATTICE_FRACTIONS[LATTICE_FRACTIONS.len() - 1] * 100.0
        );
        let _ = writeln!(
            out,
            "eye points: {} candidates, {} in cell, {} after inset, {} rejected; lattice points that \
             missed their cell first landed in solid {}, exterior {}, another cell {}; cells with no eye point {}",
            stats.candidates,
            stats.in_cell,
            stats.inset,
            stats.rejected,
            stats.missed_into_solid,
            stats.missed_into_exterior,
            stats.missed_into_other_cell,
            stats.cells_without_eye
        );
        let _ = writeln!(
            out,
            "walks: {}, step-limit overflow {} (truncated walk kept, not the runtime's frustum superset), \
             other frustum-all fallbacks {} (dropped)",
            stats.walks, stats.step_limit_walks, stats.frustum_all_walks
        );
        let _ = writeln!(
            out,
            "portals the runtime loader would reject (< 3 vertices, a non-finite vertex, zero \
             area, or a bad vertex range): {}; any nonzero count makes the shipped loader drop \
             every portal for its no-portals fallback, so the runtime would not use these walks",
            visible.loader_rejected_portals
        );
        let (sparse, dense) = visible.mean_pvs;
        let _ = writeln!(
            out,
            "mean PVS: {sparse:.1} cells ({}), {dense:.1} cells ({}); dense grew {} of {} camera cells",
            SampleDensity::Sparse.label(),
            SampleDensity::Dense.label(),
            visible.cells_grown_by_density,
            self.camera_cells.len()
        );
        let mut meters: Vec<(f64, u32)> = visible
            .sightlines
            .per_cell
            .iter()
            .map(|&(cell, m)| (m, cell))
            .collect();
        meters.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let farthest = meters.first().map_or(String::new(), |(_, cell)| {
            format!(" at {cell}@{}", self.format_center(*cell))
        });
        let _ = writeln!(
            out,
            "sightline (farthest hub-metric distance to a sampled-visible cell, per camera cell): \
             p50 {:.1} m, p95 {:.1} m, max {:.1} m{farthest}; visible cells with no hub path {}",
            percentile_desc(&meters, 50),
            percentile_desc(&meters, 95),
            meters.first().map_or(0.0, |v| v.0),
            visible.sightlines.unreachable_visible
        );

        for (sparse, dense) in visible.sparse_lead_zero.iter().zip(&visible.dense) {
            let _ = writeln!(
                out,
                "convergence at L=0, {}: {} -> {} (max, p95 MiB)",
                dense.granularity.label(),
                SampleDensity::Sparse.label(),
                SampleDensity::Dense.label()
            );
            let (sparse, dense) = (&sparse.leads[0].cells, &dense.leads[0].cells);
            for (metric, name) in names.iter().enumerate() {
                let (s, d) = (
                    MetricSummary::of(sparse, metric),
                    MetricSummary::of(dense, metric),
                );
                let change = |from: f64, to: f64| {
                    if from > 0.0 {
                        format!("{:+.1}%", (to - from) / from * 100.0)
                    } else {
                        "n/a".to_string()
                    }
                };
                let _ = writeln!(
                    out,
                    "       {:<40} max {:>7.1} -> {:>7.1} ({}), p95 {:>7.1} -> {:>7.1} ({})",
                    name,
                    mib_f64(s.max),
                    mib_f64(d.max),
                    change(s.max, d.max),
                    mib_f64(s.p95),
                    mib_f64(d.p95),
                    change(s.p95, d.p95)
                );
            }
        }

        for result in &visible.dense {
            let _ = writeln!(
                out,
                "\n-- visible-set mandatory bytes per camera cell: {}, {} ({} camera cells) --",
                SampleDensity::Dense.label(),
                result.granularity.label(),
                self.camera_cells.len()
            );
            out.push_str(
                "M(c) = cluster(c) + pinned + every cell within lead L (untruncated hub metric) \
                 and its sampled PVS; L=16m/32m ~ 1 s/2 s at 11-15 m/s\n",
            );
            let _ = writeln!(
                out,
                "{:<6} {:<40} {:>9} {:>9} {:>7}  worst cells (id@center=MiB)",
                "L",
                "metric",
                "max MiB",
                "p95 MiB",
                format!(">{LOW_TIER_BUDGET_MIB}MiB")
            );
            for lead in &result.leads {
                let mean_set = lead
                    .cells
                    .iter()
                    .map(|(_, bytes)| bytes.cell_count)
                    .sum::<usize>() as f64
                    / lead.cells.len().max(1) as f64;
                let _ = writeln!(
                    out,
                    "{:<6} mean set {:.1} cells, reached cells without a sampled PVS {}",
                    format!("{}m", lead.lead_meters),
                    mean_set,
                    lead.reached_without_pvs
                );
                self.render_metric_rows(out, &lead.cells, names);
            }
        }
    }
}
