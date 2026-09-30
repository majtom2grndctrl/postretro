//! Printed tables for cell blocks: packing overhead, mandatory block bytes and
//! the no-fragmentation pool reference per lead, and the pool fragmentation
//! walks.

use std::fmt::Write as _;

use super::block_pool_sim::{FIXED_POOL_PERCENT, SIM_SEED};
use super::camera_walks::STALL_TELEPORT_STEPS;
use super::cell_block_residency::SIM_LEAD_METERS;
use super::cell_blocks::{CANDIDATE_WIDTHS, POOL_LAYER_EDGE};
use super::mandatory::Granularity;
use super::render::{LOW_TIER_BUDGET_BYTES, LOW_TIER_BUDGET_MIB, mib_f64, percentile_desc};
use super::report::DryRunReport;
use super::visible_set::VisibleSetResult;
use crate::cell_residency_bake::pvs_sampling::SampleDensity;

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
            "\n-- cell blocks: each cell's charts packed into one block on a {}-texel grid (BC \
             block edge and direction scale; bake MaxRects, padding kept; min area over up to \
             {CANDIDATE_WIDTHS} aligned widths, each at its shortest aligned height), or into \
             several when they do not fit one {POOL_LAYER_EDGE}² pool layer --",
            blocks.alignment
        );
        let _ = writeln!(
            out,
            "recovered cells whose charts span >1 stored block: {}",
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
             cells packed into several blocks: {}",
            mib_f64(blocks.pool_layer_bytes as f64),
            overhead.multi_block_cells.len()
        );
        for &cell in &overhead.multi_block_cells {
            let dims: Vec<String> = blocks
                .cell_dims(cell)
                .iter()
                .map(|d| format!("{}x{}", d.width, d.height))
                .collect();
            let _ = writeln!(
                out,
                "    cell {cell}@{}: {} blocks ({}), {} chart texels, {:.1} MiB",
                self.format_center(cell),
                dims.len(),
                dims.join(", "),
                blocks.chart_texels[cell as usize],
                mib_f64(blocks.set_bytes(&[cell]) as f64)
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
        let _ = writeln!(
            out,
            "block bytes = sum of M(c)'s block bytes (id22 + id42 at each block's own extent); \
             layers = M(c)'s blocks packed from scratch into {POOL_LAYER_EDGE}² layers with \
             greedy MaxRects, area-descending: the no-fragmentation reference (greedy MaxRects) \
             for packing quality; the walks below measure against the shelf allocator instead"
        );
        if self.omitted_mask {
            out.push_str(
                "exact columns include the omitted id 42 restored, as every block narrow enough \
                 to double charges it\n",
            );
        }
        let _ = writeln!(
            out,
            "{:<5} {:>9} {:>7} {:>9} {:>10} {:>10} {:>12} {:>11} {:>10} {:>13} {:>12}  worst cell",
            "L",
            "max MiB",
            format!("[>{LOW_TIER_BUDGET_MIB}]"),
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
                    .map(|(cell, bytes)| (bytes.texel_exact_charging_mask(), *cell)),
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
        let shelf_worst = shelf.first().map_or(0, |v| v.0 as u32);
        let maxrects_worst = residency
            .leads
            .iter()
            .find(|l| l.lead_meters == SIM_LEAD_METERS)
            .and_then(|l| l.static_layers.iter().max().copied())
            .unwrap_or(0);
        let _ = writeln!(
            out,
            "shelf allocator from scratch at L={SIM_LEAD_METERS}m (tallest first; the walks' \
             baseline): layers max {shelf_worst}, p95 {}",
            percentile_desc(&shelf, 95)
        );
        let _ = writeln!(
            out,
            "packing gap at L={SIM_LEAD_METERS}m, worst cell: MaxRects from scratch \
             {maxrects_worst} -> shelf from scratch {shelf_worst} layers ({:+}, allocator choice \
             alone); shelf from scratch -> dynamic shelf is the walks' excess below \
             (fragmentation alone)",
            i64::from(shelf_worst) - i64::from(maxrects_worst)
        );

        let _ = writeln!(
            out,
            "\n-- cell-block pool walks: L={SIM_LEAD_METERS}m cell-granular M(c), shelf allocator \
             with free/merge (etagere-like), seed {SIM_SEED:#x} --"
        );
        let _ = writeln!(
            out,
            "each step moves to a portal-adjacent camera cell; the random walk teleports to a \
             seeded unvisited camera cell after {STALL_TELEPORT_STEPS} steps without a new cell, \
             the tour when its component is exhausted. Blocks leaving M(c) are freed (immediate) \
             or kept until space is needed (LRU); new blocks allocate tallest first, first-fit \
             by layer. A step whose allocation fails with nothing evictable defragments: every \
             mandatory block is repacked from scratch with the same shelf allocator; a hard fail \
             means even the repack did not fit, impossible at or above the shelf worst"
        );
        let walks = &residency.walks;
        for walk in &walks.walks {
            let u = &walk.unbounded;
            let _ = writeln!(
                out,
                "{}: {} steps, {} distinct of {} camera cells ({} portal components), {} \
                 teleports; uncapped immediate-free pool: peak {} layers ({:.1} MiB) vs shelf \
                 from scratch worst of visited cells {} (all cells {shelf_worst}); steps over \
                 their own shelf count {} ({:.1}%), excess mean {:.2} / max {} layers",
                walk.kind.label(),
                walk.steps,
                walk.distinct_cells,
                self.camera_cells.len(),
                walks.camera_components,
                walk.teleports,
                u.peak_layers,
                mib_f64(self.cell_blocks.pool_bytes(u.peak_layers as u32) as f64),
                u.walk_static_peak,
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
            "fixed pools are {} of the shelf from-scratch worst at L={SIM_LEAD_METERS}m \
             ({shelf_worst} layers), rounded up",
            FIXED_POOL_PERCENT
                .iter()
                .map(|p| format!("{p}%"))
                .collect::<Vec<_>>()
                .join(" and ")
        );
    }
}
