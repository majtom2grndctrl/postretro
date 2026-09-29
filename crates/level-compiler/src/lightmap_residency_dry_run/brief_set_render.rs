//! Printed tables for the brief set: per-lead block bytes and pool layers
//! with and without dilation, the cost of dilation, the prefetch band, the
//! would-be residency section's size, and the band-aware pool walks.

use std::fmt::Write as _;

use super::band_pool_sim::{BandRun, THRASH_WINDOW_STEPS};
use super::block_pool_sim::SIM_SEED;
use super::brief_set_residency::{BriefLeadResult, BriefVariant};
use super::cell_block_residency::SIM_LEAD_METERS;
use super::render::{LOW_TIER_BUDGET_BYTES, LOW_TIER_BUDGET_MIB, mib_f64, percentile_desc};
use super::report::DryRunReport;
use super::visible_set::VisibleSetResult;
use crate::cell_residency_bake::pvs_sampling::SampleDensity;

/// Max, p95, the count over Low, and the worst camera cell of one per-camera
/// metric.
struct Spread {
    max: f64,
    p95: f64,
    over_low: usize,
    worst: Option<u32>,
}

impl Spread {
    fn of(values: impl Iterator<Item = f64>, camera_cells: &[u32]) -> Self {
        let mut sorted: Vec<(f64, u32)> = values.zip(camera_cells.iter().copied()).collect();
        sorted.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        Self {
            max: sorted.first().map_or(0.0, |v| v.0),
            p95: percentile_desc(&sorted, 95),
            over_low: sorted
                .iter()
                .filter(|(v, _)| *v > LOW_TIER_BUDGET_BYTES as f64)
                .count(),
            worst: sorted.first().map(|v| v.1),
        }
    }

    fn bytes(values: &[u64], camera_cells: &[u32]) -> Self {
        Self::of(values.iter().map(|&v| v as f64), camera_cells)
    }
}

fn change(from: f64, to: f64) -> String {
    if from > 0.0 {
        format!("{:+.1}%", (to - from) / from * 100.0)
    } else {
        "n/a".to_string()
    }
}

impl DryRunReport {
    pub(super) fn render_brief_set(&self, out: &mut String, visible: &VisibleSetResult) {
        let brief = &visible.brief_set;
        let cameras = &self.camera_cells;
        let _ = writeln!(
            out,
            "\n-- brief set: M(c, L) = W(c, L) + Dil(PVS(W)) + pinned, lead map to {}m ({}, {} \
             camera cells) --",
            brief.max_lead_meters,
            SampleDensity::Dense.label(),
            cameras.len()
        );
        let _ = writeln!(
            out,
            "W = the camera cell plus every cell within untruncated hub-metric L; Dil = one \
             portal hop over id 15; pinned = cells of id-49 clusters flagged pinned ({} cells); \
             no camera-cluster term. The cell-granular M(c) above adds cluster(c) and has no \
             dilation. lead(c, x) = the smallest L with x in M(c, L); the map omits pins, as \
             the wire format does",
            brief.pinned_cells
        );
        let consistency: Vec<String> = brief
            .variants
            .iter()
            .map(|v| {
                format!(
                    "{} {} mismatched of {} (camera, lead) checks over {} distinct map leads",
                    v.dilation.label(),
                    v.consistency.mismatched,
                    v.consistency.checked,
                    v.consistency.distinct_leads
                )
            })
            .collect();
        let _ = writeln!(
            out,
            "lead-map consistency (map-read M(c, L) vs direct evaluation, every camera cell at \
             each breakpoint of either side, so every L up to {}m): {}",
            brief.max_lead_meters,
            consistency.join("; ")
        );
        let baked = match &brief.baked {
            Some(check) => format!(
                "{} mismatched of {} (camera, lead) checks over {} distinct baked leads",
                check.mismatched, check.checked, check.distinct_leads
            ),
            None => "no section in this PRL".to_string(),
        };
        let _ = writeln!(
            out,
            "baked id 51 vs direct evaluation (dilated, no pins, every breakpoint): {baked}"
        );
        let _ = writeln!(
            out,
            "{:<10} {:<4} {:>8} {:>9} {:>7} {:>9} {:>10} {:>10} {:>10} {:>10} {:>18}  worst cell",
            "variant",
            "L",
            "mean set",
            "max MiB",
            format!("[>{LOW_TIER_BUDGET_MIB}]"),
            "p95 MiB",
            "exact max",
            "exact p95",
            "shelf max",
            "shelf p95",
            "old cell max / p95"
        );
        for variant in &brief.variants {
            for lead in &variant.leads {
                self.render_brief_lead(out, visible, variant, lead);
            }
        }
        self.render_dilation_cost(out, &brief.variants);
        self.render_band_and_section(out, &brief.variants, brief.max_lead_meters);
        self.render_band_walks(out, visible);
    }

    fn render_brief_lead(
        &self,
        out: &mut String,
        visible: &VisibleSetResult,
        variant: &BriefVariant,
        lead: &BriefLeadResult,
    ) {
        let cameras = &self.camera_cells;
        let blocks = Spread::bytes(&lead.block_bytes, cameras);
        let exact = Spread::of(lead.texel_exact.iter().copied(), cameras);
        let shelf = Spread::of(lead.shelf_layers.iter().map(|&l| f64::from(l)), cameras);
        let old = visible
            .cell_blocks
            .leads
            .iter()
            .find(|old| old.lead_meters == lead.lead_meters)
            .map_or("n/a".to_string(), |old| {
                let spread = Spread::bytes(&old.bytes, cameras);
                format!("{:.1} / {:.1}", mib_f64(spread.max), mib_f64(spread.p95))
            });
        let mean_set =
            lead.set_cells.iter().sum::<usize>() as f64 / lead.set_cells.len().max(1) as f64;
        let worst = blocks.worst.map_or(String::new(), |cell| {
            format!("{cell}@{}", self.format_center(cell))
        });
        let _ = writeln!(
            out,
            "{:<10} {:<4} {:>8.1} {:>9.1} {:>7} {:>9.1} {:>10.1} {:>10.1} {:>10} {:>10} {:>18}  {worst}",
            variant.dilation.label(),
            format!("{}m", lead.lead_meters),
            mean_set,
            mib_f64(blocks.max),
            format!("[{}]", blocks.over_low),
            mib_f64(blocks.p95),
            mib_f64(exact.max),
            mib_f64(exact.p95),
            shelf.max,
            shelf.p95,
            old
        );
    }

    fn render_dilation_cost(&self, out: &mut String, variants: &[BriefVariant]) {
        let [dilated, undilated] = variants else {
            return;
        };
        let cameras = &self.camera_cells;
        let rows: Vec<String> = dilated
            .leads
            .iter()
            .zip(&undilated.leads)
            .map(|(d, u)| {
                let lead = d.lead_meters;
                let (d, u) = (
                    Spread::bytes(&d.block_bytes, cameras),
                    Spread::bytes(&u.block_bytes, cameras),
                );
                format!(
                    "L={lead}m max {} p95 {}",
                    change(u.max, d.max),
                    change(u.p95, d.p95)
                )
            })
            .collect();
        let _ = writeln!(
            out,
            "dilation cost (block bytes, {} vs {}): {}",
            dilated.dilation.label(),
            undilated.dilation.label(),
            rows.join("; ")
        );
    }

    fn render_band_and_section(
        &self,
        out: &mut String,
        variants: &[BriefVariant],
        max_lead_meters: u32,
    ) {
        let cameras = &self.camera_cells;
        for variant in variants {
            let band = &variant.band;
            let cells = Spread::of(band.cells.iter().map(|&c| c as f64), cameras);
            let mean = band.cells.iter().sum::<usize>() as f64 / band.cells.len().max(1) as f64;
            let bytes = Spread::bytes(&band.block_bytes, cameras);
            let total = Spread::bytes(&band.with_mandatory_bytes, cameras);
            let _ = writeln!(
                out,
                "prefetch band, {}, L={}m < lead <= {max_lead_meters}m: cells mean {mean:.1}, \
                 p95 {:.0}, max {:.0}; band blocks max {:.1} MiB, p95 {:.1} MiB; mandatory + \
                 band max {:.1} MiB, p95 {:.1} MiB [>{LOW_TIER_BUDGET_MIB}: {}]",
                variant.dilation.label(),
                band.lead_meters,
                cells.p95,
                cells.max,
                mib_f64(bytes.max),
                mib_f64(bytes.p95),
                mib_f64(total.max),
                mib_f64(total.p95),
                total.over_low
            );
        }
        for variant in variants {
            let section = &variant.section;
            let _ = writeln!(
                out,
                "would-be cell residency section, {}, max lead {max_lead_meters}m: {} entries \
                 (mean {:.1}, max {} per camera cell; {} name exterior cells), {} bytes = {:.1} \
                 KiB (16 B header, ({} cells + 1) x 4 B CSR over every cell id, 8 B per entry)",
                variant.dilation.label(),
                section.entries,
                section.entries as f64 / cameras.len().max(1) as f64,
                section.max_entries_per_camera,
                section.exterior_entries,
                section.bytes(),
                section.bytes() as f64 / 1024.0,
                section.cell_count
            );
        }
    }

    fn render_band_walks(&self, out: &mut String, visible: &VisibleSetResult) {
        let walks = &visible.brief_set.walks;
        let _ = writeln!(
            out,
            "\n-- brief-set pool walks: L={SIM_LEAD_METERS}m dilated M(c, L), same walks and \
             shelf allocator as above, seed {SIM_SEED:#x} --"
        );
        let _ = writeln!(
            out,
            "mandatory blocks are never refused: a miss evicts band blocks (farthest lead \
             first), then repacks in place if M(c, L) alone fits the cap, else grows past it. \
             A repack moves M(c, L), then the band blocks resident at step start (nearest lead \
             first), under the cap without reads. Band retain keeps resident band blocks under \
             the cap, freeing any past it, and prefetches the rest (nearest lead first) into free \
             space under the cap; a block freed this step is not prefetched back this step. \
             Immediate free keeps only M(c, L). No drain budget: every request completes in its \
             step. growth = steps where M(c, L) opened a layer at or past the cap; over cap = \
             steps ending with mandatory blocks past it; hit rate = blocks joining M(c, L) that \
             were already resident; blocks larger than a pool layer are excluded; demand reads \
             make a mandatory block resident, prefetch reads a band block (repack moves are not \
             reads); read MiB/step counts both; thrash = demand and prefetch reads of a block \
             freed within the last {THRASH_WINDOW_STEPS} steps (an arbitrary window)",
        );
        let caps: Vec<String> = walks
            .caps
            .iter()
            .map(|cap| {
                format!(
                    "{} layers ({:.1} MiB, {})",
                    cap.layers,
                    mib_f64(self.cell_blocks.pool_bytes(cap.layers) as f64),
                    cap.basis
                )
            })
            .collect();
        let _ = writeln!(out, "caps: {}", caps.join(", "));
        for walk in &walks.walks {
            let _ = writeln!(out, "{} ({} steps):", walk.kind.label(), walk.steps);
            let _ = writeln!(
                out,
                "    {:<8} {:<15} {:>5} {:>7} {:>9} {:>14} {:>10} {:>9} {:>12} {:>14} {:>14} {:>12} {:>16}",
                "cap",
                "policy",
                "peak",
                "growth",
                "over cap",
                "repacks",
                "band evict",
                "hit rate",
                "demand/step",
                "prefetch/step",
                "read MiB/step",
                "thrash/step",
                "thrash MiB/step"
            );
            for run in &walk.runs {
                self.render_band_run(out, run);
            }
        }
    }

    fn render_band_run(&self, out: &mut String, run: &BandRun) {
        let steps = run.steps.max(1) as f64;
        let _ = writeln!(
            out,
            "    {:<8} {:<15} {:>5} {:>7} {:>9} {:>14} {:>10} {:>8.1}% {:>12.2} {:>14.2} {:>14.2} {:>12.2} {:>16.2}",
            run.cap
                .map_or("uncapped".to_string(), |cap| cap.to_string()),
            run.policy.label(),
            run.peak_layers,
            run.growth_steps,
            run.over_cap_steps,
            format!(
                "{} ({:.2}%)",
                run.repack_steps,
                run.repack_steps as f64 / steps * 100.0
            ),
            run.band_evictions,
            run.hit_rate() * 100.0,
            run.demand_reads as f64 / steps,
            run.prefetch_reads as f64 / steps,
            mib_f64(run.read_bytes as f64) / steps,
            run.thrash_reads as f64 / steps,
            mib_f64(run.thrash_bytes as f64) / steps
        );
    }
}
