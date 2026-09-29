//! Printed tables for cell blocks: packing overhead, mandatory block bytes and
//! the static pool bound per lead, and the pool fragmentation walks.

use std::fmt::Write as _;

use super::block_pool_sim::{FIXED_POOL_PERCENT, SIM_SEED};
use super::cell_block_residency::SIM_LEAD_METERS;
use super::cell_blocks::{CANDIDATE_WIDTHS, POOL_LAYER_EDGE};
use super::mandatory::Granularity;
use super::pvs_sampling::SampleDensity;
use super::render::{LOW_TIER_BUDGET_BYTES, mib_f64, percentile_desc};
use super::report::DryRunReport;
use super::visible_set::VisibleSetResult;

/// `(value, cell)` pairs sorted largest first, ties by ascending cell.
fn sorted_desc(values: impl Iterator<Item = (f64, u32)>) -> Vec<(f64, u32)> {
    let mut sorted: Vec<(f64, u32)> = values.collect();
    sorted.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    sorted
}

impl DryRunReport {
    pub(super) fn render_cell_blocks(&self, out: &mut String) {
        let blocks = &self.cell_blocks;
        let overhead = blocks.overhead();
        let _ = writeln!(
            out,
            "\n-- cell blocks: each cell's charts packed into one BC-aligned block (bake MaxRects, \
             padding kept; min area over up to {CANDIDATE_WIDTHS} 4-aligned widths, each at its \
             shortest 4-aligned height) --"
        );
        let _ = writeln!(
            out,
            "single layer per cell today: the bake's leaf-cohesive packer keeps a BVH leaf (one \
             cell) on one layer; recovered cells whose charts span >1 stored layer: {}",
            blocks.multi_layer_cells
        );
        let ratio_at = |p| percentile_desc(&overhead.ratios, p);
        let worst_ratio = overhead.ratios.first().map_or(String::new(), |&(r, cell)| {
            format!("{r:.2}x at {cell}@{}", self.format_center(cell))
        });
        let largest = overhead.largest.map_or("none".to_string(), |(cell, d)| {
            format!(
                "{}x{} ({cell}@{})",
                d.width,
                d.height,
                self.format_center(cell)
            )
        });
        let _ = writeln!(
            out,
            "{} blocks: block texels {} / chart texels {} = {:.3}x; per-cell ratio p50 {:.3}x, \
             p95 {:.3}x, max {worst_ratio}",
            overhead.blocks,
            overhead.block_texels,
            overhead.chart_texels,
            overhead.block_texels as f64 / overhead.chart_texels.max(1) as f64,
            ratio_at(50),
            ratio_at(95)
        );
        let _ = writeln!(
            out,
            "largest block {largest}; pool layer {POOL_LAYER_EDGE}² = {:.1} MiB (id22 + id42); \
             blocks over {POOL_LAYER_EDGE} in either dimension: {}",
            mib_f64(blocks.pool_layer_bytes as f64),
            overhead.over_pool_edge.len()
        );
        for &(cell, dims) in &overhead.over_pool_edge {
            let (widest, tallest) = blocks.largest_chart[cell as usize];
            let _ = writeln!(
                out,
                "    cell {cell}@{}: block {}x{}, {} chart texels, largest chart {widest}x{tallest}{}",
                self.format_center(cell),
                dims.width,
                dims.height,
                blocks.chart_texels[cell as usize],
                if widest > POOL_LAYER_EDGE || tallest > POOL_LAYER_EDGE {
                    " (one chart alone exceeds a pool layer)"
                } else {
                    " (charts fit alone; no candidate width packed the cell under the edge)"
                }
            );
        }
        if let Some(visible) = &self.visible_set {
            self.render_cell_block_residency(out, visible);
        }
    }

    fn render_cell_block_residency(&self, out: &mut String, visible: &VisibleSetResult) {
        let residency = &visible.cell_blocks;
        let exact = visible
            .dense
            .iter()
            .find(|result| result.granularity == Granularity::Cell)
            .expect("the visible-set pass evaluates every granularity");
        let _ = writeln!(
            out,
            "\n-- cell-block mandatory bytes and static pool ({}, cell-granular M(c), {} camera cells) --",
            SampleDensity::Dense.label(),
            self.camera_cells.len()
        );
        out.push_str(
            "block bytes = sum of M(c)'s block bytes (id22 + id42 at each block's own extent); \
             static layers = M(c)'s blocks packed from scratch into 2048² layers with MaxRects, \
             area-descending: the no-fragmentation lower bound on the pool\n",
        );
        let _ = writeln!(
            out,
            "{:<5} {:>9} {:>7} {:>9} {:>10} {:>10} {:>12} {:>11} {:>10} {:>13} {:>12}  worst cell",
            "L",
            "max MiB",
            "[>256]",
            "p95 MiB",
            "exact max",
            "exact p95",
            "block/exact",
            "layers max",
            "layers p95",
            "pool MiB max",
            "unplaceable"
        );
        for (lead, exact_lead) in residency.leads.iter().zip(&exact.leads) {
            let bytes = sorted_desc(
                lead.bytes
                    .iter()
                    .zip(&self.camera_cells)
                    .map(|(&b, &cell)| (b as f64, cell)),
            );
            let exact_sorted = sorted_desc(
                exact_lead
                    .cells
                    .iter()
                    .map(|(cell, bytes)| (bytes.texel_exact, *cell)),
            );
            let layers = sorted_desc(
                lead.static_layers
                    .iter()
                    .zip(&self.camera_cells)
                    .map(|(&l, &cell)| (f64::from(l), cell)),
            );
            let over = bytes
                .iter()
                .filter(|(v, _)| *v > LOW_TIER_BUDGET_BYTES as f64)
                .count();
            let max_bytes = bytes.first().map_or(0.0, |v| v.0);
            let exact_max = exact_sorted.first().map_or(0.0, |v| v.0);
            let max_layers = layers.first().map_or(0.0, |v| v.0);
            let worst = bytes.first().map_or(String::new(), |&(_, cell)| {
                format!("{cell}@{}", self.format_center(cell))
            });
            let _ = writeln!(
                out,
                "{:<5} {:>9.1} {:>7} {:>9.1} {:>10.1} {:>10.1} {:>11.3}x {:>11} {:>10} {:>13.1} {:>12}  {worst}",
                format!("{}m", lead.lead_meters),
                mib_f64(max_bytes),
                format!("[{over}]"),
                mib_f64(percentile_desc(&bytes, 95)),
                mib_f64(exact_max),
                mib_f64(percentile_desc(&exact_sorted, 95)),
                max_bytes / exact_max.max(1.0),
                max_layers,
                percentile_desc(&layers, 95),
                mib_f64(self.cell_blocks.pool_bytes(max_layers as u32) as f64),
                lead.unplaceable_blocks
            );
        }
        let shelf = sorted_desc(
            residency
                .shelf_static_layers
                .iter()
                .zip(&self.camera_cells)
                .map(|(&l, &cell)| (f64::from(l), cell)),
        );
        let _ = writeln!(
            out,
            "shelf allocator from scratch at L={SIM_LEAD_METERS}m (tallest first): layers max {}, p95 {}",
            shelf.first().map_or(0.0, |v| v.0),
            percentile_desc(&shelf, 95)
        );

        let _ = writeln!(
            out,
            "\n-- cell-block pool walks: L={SIM_LEAD_METERS}m cell-granular M(c), shelf allocator \
             with free/merge (etagere-like), seed {SIM_SEED:#x} --"
        );
        out.push_str(
            "each step moves to a portal-adjacent camera cell; blocks leaving M(c) are freed \
             (immediate) or kept until space is needed (LRU); new blocks allocate tallest first, \
             first-fit by layer. A step whose allocation fails with nothing evictable \
             defragments: every mandatory block is repacked from scratch; a hard fail means \
             even the repack did not fit\n",
        );
        for walk in &residency.walks {
            let u = &walk.unbounded;
            let _ = writeln!(
                out,
                "{}: {} steps, {} distinct cells, {} teleports; uncapped immediate-free pool: \
                 peak {} layers ({:.1} MiB) vs static worst of visited cells {} (all cells {}); \
                 steps over their own static count {} ({:.1}%), excess mean {:.2} / max {} layers",
                walk.kind.label(),
                walk.steps,
                walk.distinct_cells,
                walk.teleports,
                u.peak_layers,
                mib_f64(self.cell_blocks.pool_bytes(u.peak_layers as u32) as f64),
                u.walk_static_peak,
                residency
                    .leads
                    .iter()
                    .find(|l| l.lead_meters == SIM_LEAD_METERS)
                    .and_then(|l| l.static_layers.iter().max().copied())
                    .unwrap_or(0),
                u.steps_over_static,
                u.steps_over_static as f64 / walk.steps.max(1) as f64 * 100.0,
                u.mean_excess,
                u.max_excess
            );
            for run in &walk.fixed {
                let _ = writeln!(
                    out,
                    "    pool {:>3} layers ({:>6.1} MiB) {:<9}: defrag steps {:>6} ({:>5.2}%), \
                     hard-fail steps {:>6} ({} blocks), LRU evictions {}",
                    run.pool_layers,
                    mib_f64(self.cell_blocks.pool_bytes(run.pool_layers) as f64),
                    run.eviction.label(),
                    run.defrag_steps,
                    run.defrag_steps as f64 / walk.steps.max(1) as f64 * 100.0,
                    run.hard_fail_steps,
                    run.hard_fail_blocks,
                    run.evictions
                );
            }
        }
        let _ = writeln!(
            out,
            "fixed pools are {} of the static worst at L={SIM_LEAD_METERS}m, rounded up",
            FIXED_POOL_PERCENT
                .iter()
                .map(|p| format!("{p}%"))
                .collect::<Vec<_>>()
                .join(" and ")
        );
    }
}
