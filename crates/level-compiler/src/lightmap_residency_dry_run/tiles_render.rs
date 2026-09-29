//! Printed table for tiled residency: the visible-set mandatory sets costed
//! under fixed-size tiles owned by one cell or one cluster each.

use std::fmt::Write as _;

use super::mandatory::Granularity;
use super::pvs_sampling::SampleDensity;
use super::render::{LOW_TIER_BUDGET_BYTES, mib_f64, percentile_desc};
use super::report::DryRunReport;
use super::tiles::{TILE_SIZES, TileLayout, TileUnit};
use super::visible_set::{GranularityResult, VisibleSetResult};

/// The mandatory set a unit's residency follows. A cluster tile is resident
/// when any member is mandatory, so cluster units read the same bytes from
/// either set; cluster closure is the one that states it.
fn granularity_for(unit: TileUnit) -> Granularity {
    match unit {
        TileUnit::Cell => Granularity::Cell,
        TileUnit::Cluster => Granularity::ClusterClosure,
    }
}

fn dense_for(visible: &VisibleSetResult, granularity: Granularity) -> &GranularityResult {
    visible
        .dense
        .iter()
        .find(|result| result.granularity == granularity)
        .expect("the visible-set pass evaluates every granularity")
}

/// Cluster-unit bytes are the same whether computed from the cell-granular or
/// the cluster-closure set: both reach the same clusters.
pub(crate) fn cluster_units_agree_across_granularity(
    visible: &VisibleSetResult,
    tile_layouts: &[TileLayout],
) -> bool {
    let cell = dense_for(visible, Granularity::Cell);
    let closure = dense_for(visible, Granularity::ClusterClosure);
    cell.leads.iter().zip(&closure.leads).all(|(a, b)| {
        a.tile_bytes.iter().zip(&b.tile_bytes).all(|(a, b)| {
            tile_layouts
                .iter()
                .enumerate()
                .filter(|(_, layout)| layout.unit == TileUnit::Cluster)
                .all(|(index, _)| a[index] == b[index])
        })
    })
}

impl DryRunReport {
    pub(super) fn render_tiles(&self, out: &mut String, visible: &VisibleSetResult) {
        let _ = writeln!(
            out,
            "\n-- tiled residency: P x P tiles, one owner unit each ({}, {} camera cells) --",
            SampleDensity::Dense.label(),
            self.camera_cells.len()
        );
        out.push_str(
            "note: M(c) is the sampled visible set above, so every figure is a lower bound; bytes \
             cover id 22 + id 42 only\n",
        );
        out.push_str(
            "each unit's charts pack first-fit decreasing into its own tiles (bake MaxRects, \
             padding kept); a unit's tiles are resident iff the unit is mandatory; cell units \
             follow cell-granular M(c), cluster units cluster-closure M(c)\n",
        );
        let _ = writeln!(
            out,
            "cluster-unit bytes identical under both M(c): {}",
            if cluster_units_agree_across_granularity(visible, &self.tile_layouts) {
                "yes"
            } else {
                "NO"
            }
        );
        let per_size: Vec<&TileLayout> = TILE_SIZES
            .iter()
            .filter_map(|&p| {
                self.tile_layouts
                    .iter()
                    .find(|l| l.tile_size == p && l.unit == TileUnit::Cell)
            })
            .collect();
        for layout in &per_size {
            let p = layout.tile_size;
            let oversize = layout.oversize;
            let _ = writeln!(
                out,
                "P={p}: tile {:.1} KiB (id22 + id42 at stored encodings); BC alignment: P and tile \
                 origins multiples of 4 (asserted); oversize charts (padded extent > P, costed \
                 ceil(w/P) x ceil(h/P) own tiles): {} charts ({} fit P unpadded), {} texels \
                 ({:.1}% of chart texels) -> {} tiles, {:.2}x their texels",
                layout.tile_bytes as f64 / 1024.0,
                oversize.charts,
                oversize.interior_fits,
                oversize.chart_texels,
                oversize.chart_texels as f64 / layout.chart_texels.max(1) as f64 * 100.0,
                oversize.tiles,
                layout.oversize_overhead()
            );
        }
        out.push_str(
            "tile/exact = tile texels over chart texels, whole map; shared t/e = the same for \
             charts that fit a tile, excluding oversize charts and their dedicated tiles\n",
        );
        let _ = writeln!(
            out,
            "{:<8} {:>4} {:<5} {:>9} {:>7} {:>9} {:>10} {:>11} {:>11} {:>13}  worst cell",
            "unit",
            "P",
            "L",
            "max MiB",
            "[>256]",
            "p95 MiB",
            "exact max",
            "tile/exact",
            "shared t/e",
            "virtual tiles"
        );
        for (index, layout) in self.tile_layouts.iter().enumerate() {
            let result = dense_for(visible, granularity_for(layout.unit));
            for lead in &result.leads {
                let mut sorted: Vec<(f64, u32)> = lead
                    .cells
                    .iter()
                    .zip(&lead.tile_bytes)
                    .map(|((cell, _), bytes)| (bytes[index] as f64, *cell))
                    .collect();
                sorted.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
                let over = sorted
                    .iter()
                    .filter(|(v, _)| *v > LOW_TIER_BUDGET_BYTES as f64)
                    .count();
                let exact_max = lead
                    .cells
                    .iter()
                    .map(|(_, bytes)| bytes.texel_exact)
                    .fold(0.0, f64::max);
                let worst = sorted.first().map_or(String::new(), |&(_, cell)| {
                    format!("{cell}@{}", self.format_center(cell))
                });
                let _ = writeln!(
                    out,
                    "{:<8} {:>4} {:<5} {:>9.1} {:>7} {:>9.1} {:>10.1} {:>10.2}x {:>10.2}x {:>13}  {worst}",
                    layout.unit.label(),
                    layout.tile_size,
                    format!("{}m", lead.lead_meters),
                    mib_f64(sorted.first().map_or(0.0, |v| v.0)),
                    format!("[{over}]"),
                    mib_f64(percentile_desc(&sorted, 95)),
                    mib_f64(exact_max),
                    layout.overhead(),
                    layout.shared_overhead(),
                    layout.virtual_tiles()
                );
            }
        }
    }
}
