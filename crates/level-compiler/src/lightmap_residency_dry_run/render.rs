//! Printed tables and CSV for a finished dry run.

use std::fmt::Write as _;

use postretro_level_format::cell_visibility::{
    CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE, CELL_VISIBILITY_FANOUT_K,
};
use postretro_level_format::shadowmask_atlas::SHADOWMASK_GROUP_COUNT;

use super::layouts::RUNTIME_LAYER_FLOOR;
use super::mandatory::{DistanceBound, MandatoryBytes};
use super::portal_distance::VALIDATION_TOLERANCE_FIXED;
use super::report::DryRunReport;
use super::{AtlasFormats, DryRunInput, ShadowmaskState};
use crate::shadowmask_bake::MAX_SHADOWMASK_TEXTURE_WIDTH;

/// Low tier budget for lightmap + shadowmask data.
pub(crate) const LOW_TIER_BUDGET_BYTES: u64 = 256 * 1024 * 1024;
const TOP_CELLS: usize = 5;

pub(super) fn input_summary(input: &DryRunInput) -> String {
    let f = &input.formats;
    let r = &input.reconstruction;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "id 22: {} layers of {}x{} irr (format {}) + {}x{} dir (format {}); payload irr {} + dir {} bytes",
        f.layer_count,
        f.irr_width,
        f.irr_height,
        f.irr_format,
        f.dir_width,
        f.dir_height,
        f.dir_format,
        f.irr_payload_bytes,
        f.dir_payload_bytes
    );
    match &f.shadowmask {
        ShadowmaskState::Stored(sm) => {
            let _ = writeln!(
                out,
                "id 42: BC5 side-by-side, {} layers of {}x{} lightmap texels; payload {} bytes",
                sm.layer_count, sm.width, sm.height, sm.payload_bytes
            );
        }
        ShadowmaskState::OmittedForWidth | ShadowmaskState::Absent => {}
    }
    let _ = writeln!(out, "{}", shadowmask_policy(f));
    let _ = writeln!(
        out,
        "bytes per chart texel: id22 {:.3} + id42 {:.3}; stored layer {} bytes",
        f.lightmap_bytes_per_texel(),
        f.shadowmask_bytes_per_texel(),
        f.stored_layer_bytes()
    );
    let _ = writeln!(
        out,
        "cells {}, clusters {}, pinned clusters {:?}, id 46 present: {}, coupled pairs {}",
        input.cell_count(),
        input.cluster_count,
        input.pinned_clusters,
        input.cell_visibility_present,
        input.coupled_pairs.len()
    );
    let _ = writeln!(
        out,
        "faces {} -> charts {}; 1x1 placeholders: zero-index {}, short-index {}, uncharted {}, edge-clamped {}; unrecovered: mixed-layer {}, out-of-bounds {}; unmatched BVH faces {}",
        r.faces,
        r.charts,
        r.zero_index_faces,
        r.short_index_faces,
        r.uncharted_faces,
        r.edge_clamped_faces,
        r.mixed_layer_faces,
        r.out_of_bounds_faces,
        r.unmatched_bvh_faces
    );
    out
}

/// How layouts charge id 42, printed beside the layout tables.
pub(super) fn shadowmask_policy(formats: &AtlasFormats) -> String {
    let limit = MAX_SHADOWMASK_TEXTURE_WIDTH / SHADOWMASK_GROUP_COUNT;
    match formats.shadowmask {
        ShadowmaskState::Stored(_) => format!(
            "id 42 stored: charged 2 B/texel in texel-exact and in every layer up to {limit} wide"
        ),
        ShadowmaskState::OmittedForWidth => format!(
            "id 42 omitted for width (selected shadow lights, {}-wide layers cannot double): \
             texel-exact and stored layout carry none, the \"+ omitted id42\" rows restore it; \
             simulated layers up to {limit} wide are charged 2 B/texel",
            formats.irr_width
        ),
        ShadowmaskState::Absent => "id 42 absent: no selected shadow lights, never charged".into(),
    }
}

fn mib(bytes: u64) -> f64 {
    mib_f64(bytes as f64)
}

pub(super) fn mib_f64(bytes: f64) -> f64 {
    bytes / (1024.0 * 1024.0)
}

impl DryRunReport {
    fn metric_names(&self) -> Vec<String> {
        let mut names = vec!["texel-exact".to_string(), "half-res texel".to_string()];
        if self.omitted_mask {
            names.push("texel-exact + omitted id42".to_string());
            names.push("half-res + omitted id42".to_string());
        }
        names.extend(self.layouts.iter().map(|l| format!("layer {}", l.name)));
        names
    }

    pub(super) fn metric_values(bytes: &MandatoryBytes) -> Vec<f64> {
        let mut values = vec![bytes.texel_exact, bytes.half_res];
        if let Some((texel_exact, half_res)) = bytes.with_omitted_mask {
            values.extend([texel_exact, half_res]);
        }
        values.extend(bytes.layer.iter().map(|&b| b as f64));
        values
    }

    pub(crate) fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("== lightmap residency dry run ==\n");
        out.push_str(&self.input_summary);

        out.push_str("\n-- attribution (stored layout; majority owner per stored unit) --\n");
        let mut sections = vec![
            ("id22 irradiance", &self.attribution.irradiance),
            ("id22 direction", &self.attribution.direction),
        ];
        if let Some(sm) = &self.attribution.shadowmask {
            sections.push(("id42 shadowmask", sm));
        }
        for (name, section) in sections {
            let attributed = section.attributed();
            let _ = writeln!(
                out,
                "{name:<16} attributed {attributed} + unattributed {} = {} vs payload {} ({})",
                section.unattributed,
                attributed + section.unattributed,
                section.payload,
                if attributed + section.unattributed == section.payload {
                    "exact"
                } else {
                    "MISMATCH"
                }
            );
        }
        let _ = writeln!(
            out,
            "chart overlap texels: {} (nonzero = reconstruction disagrees with packing)",
            self.attribution.overlap_texels
        );
        let _ = writeln!(
            out,
            "stored repack with the bake packer (placeholders included): {}/{} charts at stored placement, dims match: {}{}",
            self.repack.matched,
            self.repack.total,
            self.repack.dims_match,
            self.repack
                .error
                .as_ref()
                .map_or(String::new(), |error| format!(", PACKER ERROR: {error}"))
        );
        let _ = writeln!(
            out,
            "cluster  cells charts   irr MiB    dir MiB     sm MiB  total MiB  layers {}",
            self.layouts
                .iter()
                .map(|l| l.name.as_str())
                .collect::<Vec<_>>()
                .join("/")
        );
        for row in &self.cluster_rows {
            let _ = writeln!(
                out,
                "{:>7} {:>5} {:>6} {:>10.2} {:>10.2} {:>10.2} {:>10.2}  {}",
                row.cluster,
                row.cells,
                row.charts,
                mib(row.irradiance),
                mib(row.direction),
                mib(row.shadowmask),
                mib(row.irradiance + row.direction + row.shadowmask),
                row.layers
                    .iter()
                    .map(usize::to_string)
                    .collect::<Vec<_>>()
                    .join("/")
            );
        }

        out.push_str("\n-- layouts --\n");
        let _ = writeln!(out, "{}", self.shadowmask_policy);
        for layout in &self.layouts {
            let _ = writeln!(
                out,
                "{:<22} regular layers {} ({}), oversize cells {}, total {:.1} MiB, texels {:.3}x stored{}",
                layout.name,
                layout.regular_layer_count,
                layout
                    .layer_dims
                    .first()
                    .map_or("none".to_string(), |d| format!("{d}²")),
                layout.oversize_cells.len(),
                mib(layout.total_bytes()),
                layout.total_layer_texels() as f64 / self.stored_total_texels.max(1) as f64,
                if layout.regular_layer_count > RUNTIME_LAYER_FLOOR {
                    format!(" [over {RUNTIME_LAYER_FLOOR}-layer runtime floor]")
                } else {
                    String::new()
                }
            );
            for oversize in &layout.oversize_cells {
                let _ = writeln!(
                    out,
                    "    oversize cell {} (cluster {}) at {} -> own {}² layer",
                    oversize.cell,
                    self.cell_clusters[oversize.cell as usize],
                    self.format_center(oversize.cell),
                    oversize.layer_dim
                );
            }
        }

        if let Some(v) = &self.validation {
            let _ = writeln!(
                out,
                "\nhub-metric recompute vs id-46 records within {}m: checked {}, matched {} (±{} fixed), mismatched {}, missing {}, max diff {}",
                DistanceBound::max_fixed() / CELL_VISIBILITY_DISTANCE_FIXED_POINT_SCALE,
                v.checked,
                v.matched,
                VALIDATION_TOLERANCE_FIXED,
                v.mismatched,
                v.missing,
                v.max_abs_diff
            );
        }

        let names = self.metric_names();
        for source in &self.sources {
            let _ = writeln!(
                out,
                "\n-- mandatory bytes per camera cell: {}, {} ({} camera cells) --",
                source.name,
                source.granularity.label(),
                self.camera_cells.len()
            );
            if let Some(note) = source.note {
                let _ = writeln!(out, "note: {note}");
            }
            let _ = writeln!(
                out,
                "{:<6} {:<40} {:>9} {:>9} {:>7}  worst cells (id@center=MiB)",
                "D", "metric", "max MiB", "p95 MiB", ">256MiB"
            );
            for bound in &source.bounds {
                let mean_set = bound
                    .cells
                    .iter()
                    .map(|(_, bytes)| bytes.cell_count)
                    .sum::<usize>() as f64
                    / bound.cells.len().max(1) as f64;
                let _ = writeln!(
                    out,
                    "{:<6} mean set {:.1} cells{}",
                    bound.bound.label(),
                    mean_set,
                    if source.counts_truncation && bound.bound.fixed().is_some() {
                        format!(
                            ", {} cells hold >= K={} stored partners (possibly truncated)",
                            bound.possibly_truncated, CELL_VISIBILITY_FANOUT_K
                        )
                    } else {
                        String::new()
                    }
                );
                self.render_metric_rows(&mut out, &bound.cells, &names);
            }
        }
        if let Some(visible) = &self.visible_set {
            self.render_visible_set(&mut out, visible, &names);
            self.render_tiles(&mut out, visible);
        }
        out
    }

    /// One row per metric: worst and p95 camera cell, the count over Low, and
    /// the worst cells.
    pub(super) fn render_metric_rows(
        &self,
        out: &mut String,
        cells: &[(u32, MandatoryBytes)],
        names: &[String],
    ) {
        for (metric, name) in names.iter().enumerate() {
            let summary = MetricSummary::of(cells, metric);
            let worst: Vec<String> = summary
                .sorted
                .iter()
                .take(TOP_CELLS)
                .map(|(v, cell)| format!("{cell}@{}={:.1}", self.format_center(*cell), mib_f64(*v)))
                .collect();
            let _ = writeln!(
                out,
                "{:<6} {:<40} {:>9.1} {:>9.1} {:>7}  {}",
                "",
                name,
                mib_f64(summary.max),
                mib_f64(summary.p95),
                summary.over,
                worst.join(" ")
            );
        }
    }

    pub(super) fn format_center(&self, cell: u32) -> String {
        let [x, y, z] = self.cell_centers[cell as usize];
        format!("({x:.0},{y:.0},{z:.0})")
    }

    pub(crate) fn csv(&self) -> String {
        let mut out = String::from(
            "source,granularity,bound,cell,cluster,center_x,center_y,center_z,set_cells,texel_exact_bytes,half_res_bytes",
        );
        if self.omitted_mask {
            out.push_str(",texel_exact_with_omitted_id42_bytes,half_res_with_omitted_id42_bytes");
        }
        for layout in &self.layouts {
            let _ = write!(out, ",layer_{}_bytes", layout.name.replace([' ', '²'], ""));
        }
        out.push('\n');
        for source in &self.sources {
            for bound in &source.bounds {
                for (cell, bytes) in &bound.cells {
                    let [x, y, z] = self.cell_centers[*cell as usize];
                    let _ = write!(
                        out,
                        "{},{},{},{},{},{x},{y},{z},{},{:.0},{:.0}",
                        source.name,
                        source.granularity.label(),
                        bound.bound.label(),
                        cell,
                        self.cell_clusters[*cell as usize],
                        bytes.cell_count,
                        bytes.texel_exact,
                        bytes.half_res
                    );
                    if let Some((texel_exact, half_res)) = bytes.with_omitted_mask {
                        let _ = write!(out, ",{texel_exact:.0},{half_res:.0}");
                    }
                    for layer in &bytes.layer {
                        let _ = write!(out, ",{layer}");
                    }
                    out.push('\n');
                }
            }
        }
        out
    }
}

/// One metric over every camera cell.
pub(super) struct MetricSummary {
    /// `(bytes, cell)`, largest first, ties by ascending cell.
    pub sorted: Vec<(f64, u32)>,
    pub max: f64,
    pub p95: f64,
    /// Cells over the Low tier budget.
    pub over: usize,
}

impl MetricSummary {
    pub(super) fn of(cells: &[(u32, MandatoryBytes)], metric: usize) -> Self {
        let mut sorted: Vec<(f64, u32)> = cells
            .iter()
            .map(|(cell, bytes)| (DryRunReport::metric_values(bytes)[metric], *cell))
            .collect();
        sorted.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        Self {
            max: sorted.first().map_or(0.0, |v| v.0),
            p95: percentile_desc(&sorted, 95),
            over: sorted
                .iter()
                .filter(|(v, _)| *v > LOW_TIER_BUDGET_BYTES as f64)
                .count(),
            sorted,
        }
    }
}

/// Nearest-rank percentile of values sorted descending.
pub(super) fn percentile_desc(values: &[(f64, u32)], percentile: usize) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let rank = (values.len() * percentile).div_ceil(100).max(1);
    values[values.len() - rank].0
}
