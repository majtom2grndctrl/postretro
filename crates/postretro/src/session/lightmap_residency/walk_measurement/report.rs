//! The residency walks' per-run report.
//! See: context/lib/rendering_pipeline.md §4

use std::time::Duration;

use super::pool_mirror::PoolStats;
use super::trip_record::{GateSplit, RunRecord, SplitTotals};
use super::{Levers, Walk};

const MIB: f64 = 1024.0 * 1024.0;

/// Visible-miss block-frames, one bucket pair per step kind.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Misses {
    frames: u64,
    outside: u64,
    not_resident: u64,
    frames_outside: u64,
    frames_not_resident: u64,
    max_outside: u32,
    max_not_resident: u32,
    /// Of `outside`: drawn while not resident after the frame's drain, so
    /// sampled SH-only (the harness's own count, over the same drawn set).
    outside_missing: u64,
    frames_outside_missing: u64,
}

impl Misses {
    pub(super) fn add(&mut self, outside: u32, not_resident: u32, outside_missing: u32) {
        self.frames += 1;
        self.outside += u64::from(outside);
        self.not_resident += u64::from(not_resident);
        self.frames_outside += u64::from(outside > 0);
        self.frames_not_resident += u64::from(not_resident > 0);
        self.max_outside = self.max_outside.max(outside);
        self.max_not_resident = self.max_not_resident.max(not_resident);
        self.outside_missing += u64::from(outside_missing);
        self.frames_outside_missing += u64::from(outside_missing > 0);
    }

    fn line(&self) -> String {
        format!(
            "{} frames: outside baked set {} block-frames in {} frames (max {}/frame), of which \
             drawn non-resident {} in {} frames; not resident {} block-frames in {} frames \
             (max {}/frame)",
            self.frames,
            self.outside,
            self.frames_outside,
            self.max_outside,
            self.outside_missing,
            self.frames_outside_missing,
            self.not_resident,
            self.frames_not_resident,
            self.max_not_resident,
        )
    }
}

#[derive(Debug, Default)]
pub(super) struct RunReport {
    pub(super) frames: u64,
    pub(super) wall: Duration,
    pub(super) paced_frames: u64,
    pub(super) locate_mismatch_frames: u64,
    pub(super) path_portal: u64,
    pub(super) path_step_limit: u64,
    pub(super) path_other: u64,
    pub(super) preload_pairs: u32,
    pub(super) preload_bytes: u64,
    pub(super) preload_millis: f64,
    /// Per step end: (resident id 22, resident id 42, mandatory id 22,
    /// mandatory id 42) bytes.
    pub(super) step_bytes: Vec<[u64; 4]>,
    /// Per step: id-22 and id-42 bytes read at the positional reader.
    pub(super) step_reads: Vec<(u64, u64)>,
    pub(super) entering_blocks: u64,
    pub(super) entering_resident: u64,
    pub(super) spawn: Misses,
    pub(super) adjacent: Misses,
    /// Adjacent-step frames split: the frame the camera cell changed, and
    /// the rest.
    pub(super) adjacent_change_frames: Misses,
    pub(super) teleport: Misses,
    pub(super) residency_nanos_change: Vec<u64>,
    pub(super) residency_nanos_steady: Vec<u64>,
    /// Steady frames' wall time per part: `prepare_drains` (demand,
    /// completions, batch, reads), `apply_outcome`, `finish_frame`.
    pub(super) steady_parts: [Vec<u64>; 3],
    /// Steady frames' `prepare_drains` time, split by whether the frame
    /// admitted delivered read bytes (completions), and the most admitted.
    pub(super) prepare_with_completions: Vec<u64>,
    pub(super) prepare_without_completions: Vec<u64>,
    pub(super) max_admitted_bytes: u64,
    pub(super) counters: Option<crate::lightmap_streaming::controller::LightmapResidencyCounters>,
    pub(super) physical_reads: u64,
    pub(super) pool: PoolStats,
    pub(super) layer_bytes: u64,
    pub(super) first_not_resident: Vec<(usize, u32, u32)>,
    pub(super) first_outside: Vec<(usize, u32, u32)>,
    /// Drawn non-resident block-frames by cause, after the last drain.
    pub(super) causes_spawn: Causes,
    pub(super) causes_play: Causes,
    /// Drawn cells outside the camera cell's baked set at L that own blocks.
    pub(super) outside_cells: OutsideCells,
    /// Blocks in each camera cell's baked mandatory set at L: p50, p95, max.
    pub(super) mandatory_set: [u64; 3],
    /// Per-frame trip record of this run.
    pub(super) record: RunRecord,
}

/// Drawn blocks still non-resident after the frame's last drain, by cause
/// (O29): deferred behind a retiring generation, or left behind by
/// admission (undemanded, unread, over the install budget, or refused).
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Causes {
    frames: u64,
    deferred: u64,
    left_behind: u64,
    frames_deferred: u64,
    frames_left_behind: u64,
    max_deferred: u32,
}

impl Causes {
    pub(super) fn add(&mut self, deferred: u32, left_behind: u32) {
        self.frames += 1;
        self.deferred += u64::from(deferred);
        self.left_behind += u64::from(left_behind);
        self.frames_deferred += u64::from(deferred > 0);
        self.frames_left_behind += u64::from(left_behind > 0);
        self.max_deferred = self.max_deferred.max(deferred);
    }

    fn line(&self) -> String {
        format!(
            "{} frames: deferred behind a retiring generation {} block-frames in {} frames (max \
             {}/frame); left behind by admission {} block-frames in {} frames",
            self.frames,
            self.deferred,
            self.frames_deferred,
            self.max_deferred,
            self.left_behind,
            self.frames_left_behind,
        )
    }
}

/// Drawn cells outside the camera cell's baked set at L, per frame (R3).
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct OutsideCells {
    pub(super) cell_frames: u64,
    pub(super) blocks: u64,
    pub(super) max_blocks: u32,
}

impl OutsideCells {
    pub(super) fn add(&mut self, blocks: u32) {
        self.cell_frames += 1;
        self.blocks += u64::from(blocks);
        self.max_blocks = self.max_blocks.max(blocks);
    }
}

fn totals_line(t: &SplitTotals) -> String {
    format!(
        "{} frames ({} trip frames): deferred {}, left behind {}, non-resident {}, outside set {} \
         block-frames",
        t.frames,
        t.trip_frames,
        t.deferred,
        t.left_behind,
        t.not_resident(),
        t.outside,
    )
}

/// The gate split: this run's frames against `against`, both runs' totals.
pub(super) fn print_split(against: &str, result: &Result<GateSplit, String>) {
    match result {
        Err(why) => println!("   gate split by {against}: INVALID pair, rerun both: {why}"),
        Ok(split) => {
            println!(
                "   gate split by {against}: spawn frames apart {}; outside the gate {} frames \
                 ({} held after a trip until its growth retired)",
                split.spawn_frames, split.apart[0].frames, split.held_after_trip,
            );
            println!(
                "     in gate, this run:     {}",
                totals_line(&split.gate[0])
            );
            println!(
                "     in gate, recorded run: {}",
                totals_line(&split.gate[1])
            );
            println!(
                "     apart,   this run:     {}",
                totals_line(&split.apart[0])
            );
            println!(
                "     apart,   recorded run: {}",
                totals_line(&split.apart[1])
            );
            let (this, recorded) = (split.gate[0].deferred, split.gate[1].deferred);
            println!(
                "     gate: deferred block-frames {this} vs recorded {recorded}: {}",
                if this <= recorded {
                    "within"
                } else {
                    "EXCEEDS"
                }
            );
        }
    }
}

pub(super) fn max_and_p95(mut values: Vec<u64>) -> (u64, u64) {
    if values.is_empty() {
        return (0, 0);
    }
    values.sort_unstable();
    let p95 = values[((values.len() as f64 * 0.95).ceil() as usize).clamp(1, values.len()) - 1];
    (*values.last().expect("non-empty"), p95)
}

pub(super) fn mib(bytes: u64) -> f64 {
    bytes as f64 / MIB
}

fn micros_line(label: &str, nanos: &[u64]) -> String {
    if nanos.is_empty() {
        return format!("{label}: no frames");
    }
    let mut sorted = nanos.to_vec();
    sorted.sort_unstable();
    let at = |q: f64| {
        sorted[((sorted.len() as f64 * q).ceil() as usize).clamp(1, sorted.len()) - 1] as f64
            / 1000.0
    };
    let mean = sorted.iter().sum::<u64>() as f64 / sorted.len() as f64 / 1000.0;
    format!(
        "{label}: {} frames, mean {mean:.1} us, p50 {:.1}, p95 {:.1}, p99 {:.1}, max {:.1} us",
        sorted.len(),
        at(0.5),
        at(0.95),
        at(0.99),
        at(1.0),
    )
}

pub(super) fn print_run(walk: &Walk, levers: Levers, report: &RunReport) {
    let steps = report.step_bytes.len().max(1) as u64;
    let column = |i: usize| report.step_bytes.iter().map(|b| b[i]).collect::<Vec<_>>();
    let total = |a: usize, b: usize| {
        report
            .step_bytes
            .iter()
            .map(|v| v[a] + v[b])
            .collect::<Vec<_>>()
    };
    let (res_max, res_p95) = max_and_p95(total(0, 1));
    let (res22_max, res22_p95) = max_and_p95(column(0));
    let (res42_max, res42_p95) = max_and_p95(column(1));
    let (man_max, man_p95) = max_and_p95(total(2, 3));
    let (man22_max, man22_p95) = max_and_p95(column(2));
    let (man42_max, man42_p95) = max_and_p95(column(3));
    let reads22: u64 = report.step_reads.iter().map(|r| r.0).sum();
    let reads42: u64 = report.step_reads.iter().map(|r| r.1).sum();
    let (step_read_max, step_read_p95) =
        max_and_p95(report.step_reads.iter().map(|r| r.0 + r.1).collect());
    let pool = &report.pool;
    let c = report.counters.unwrap_or_default();
    let (drain_bytes_max, drain_bytes_p95) = max_and_p95(pool.drain_install_bytes.clone());
    println!(
        "== {} | L = {} m | cap {} | {} steps, {} teleports, {} frames ({} paced), wall {:.1} s",
        walk.kind.label(),
        levers.lead_metres,
        levers.cap_layers,
        report.step_bytes.len(),
        walk.teleports(),
        report.frames,
        report.paced_frames,
        report.wall.as_secs_f64(),
    );
    println!(
        "   settled-entry preload: {} pairs, {:.1} MiB read, {:.1} MiB uploaded in one drain, {:.1} ms \
         (positional reads + model plan)",
        report.preload_pairs,
        mib(report.preload_bytes),
        mib(pool.preload_install_bytes),
        report.preload_millis,
    );
    println!(
        "   resident MiB max / p95: total {:.1} / {:.1} (id 22 {:.1} / {:.1}, id 42 {:.1} / {:.1})",
        mib(res_max),
        mib(res_p95),
        mib(res22_max),
        mib(res22_p95),
        mib(res42_max),
        mib(res42_p95),
    );
    println!(
        "   mandatory MiB max / p95: total {:.1} / {:.1} (id 22 {:.1} / {:.1}, id 42 {:.1} / {:.1})",
        mib(man_max),
        mib(man_p95),
        mib(man22_max),
        mib(man22_p95),
        mib(man42_max),
        mib(man42_p95),
    );
    println!(
        "   pool: first {} layers, peak {} layers ({} occupied), layer {:.1} MiB; peak held \
         {:.1} MiB (spare and retiring included); growths {}, growth transient peak {:.1} MiB; \
         repacks {} ({:.2}% of steps, {} copies); deferred {}; evictions {}; refusals {}; \
         failed {}",
        pool.first_layers,
        pool.peak_layers,
        pool.peak_occupied_layers,
        mib(report.layer_bytes),
        mib(pool.peak_pool_bytes),
        pool.growths,
        mib(pool.growth_transient_peak_bytes),
        pool.repacks,
        pool.repacks as f64 * 100.0 / steps as f64,
        pool.repack_copies,
        pool.deferred,
        pool.evictions,
        pool.refusals,
        pool.failed,
    );
    println!(
        "   reads (after spawn): id 22 {:.1} MiB, id 42 {:.1} MiB; per step mean {:.2} MiB, \
         p95 {:.2}, max {:.2}; {} pairs requested, {} physical reads; installs {} ({:.1} MiB \
         uploaded over {} submitted drains, per drain p95 {:.2} MiB, max {:.2} MiB); \
         cancelled {}, departed {}, failed reads {}",
        mib(reads22),
        mib(reads42),
        mib(reads22 + reads42) / steps as f64,
        mib(step_read_p95),
        mib(step_read_max),
        c.reads_requested,
        report.physical_reads,
        pool.uploads,
        mib(pool.install_bytes),
        pool.submissions,
        mib(drain_bytes_p95),
        mib(drain_bytes_max),
        c.cancelled_reads,
        c.departed_reads,
        c.failed_reads,
    );
    println!(
        "   hit rate: {} of {} blocks joining M(c, L) already resident ({:.1}%)",
        report.entering_resident,
        report.entering_blocks,
        report.entering_resident as f64 * 100.0 / report.entering_blocks.max(1) as f64,
    );
    println!("   misses, spawn step: {}", report.spawn.line());
    println!("   misses, adjacent steps: {}", report.adjacent.line());
    println!(
        "   misses, adjacent steps, cell-change frames only: {}",
        report.adjacent_change_frames.line()
    );
    println!("   misses, teleport steps: {}", report.teleport.line());
    if !report.first_outside.is_empty() {
        println!(
            "   first outside-baked-set frames (step, camera cell, blocks): {:?}",
            report.first_outside
        );
    }
    if !report.first_not_resident.is_empty() {
        println!(
            "   first not-resident frames (step, camera cell, blocks): {:?}",
            report.first_not_resident
        );
    }
    println!(
        "   non-resident by cause, spawn step: {}",
        report.causes_spawn.line()
    );
    println!(
        "   non-resident by cause, after spawn: {}",
        report.causes_play.line()
    );
    let cells = &report.outside_cells;
    println!(
        "   drawn cells outside the baked set (with blocks): {} cell-frames, {} blocks, {:.2} \
         blocks per cell (max {})",
        cells.cell_frames,
        cells.blocks,
        cells.blocks as f64 / cells.cell_frames.max(1) as f64,
        cells.max_blocks,
    );
    let [p50, p95, max] = report.mandatory_set;
    println!("   baked mandatory set per camera cell: p50 {p50}, p95 {p95}, max {max} blocks");
    println!(
        "   visibility paths: portal {}, step-limit {}, other {}; eye-locate mismatches {}",
        report.path_portal,
        report.path_step_limit,
        report.path_other,
        report.locate_mismatch_frames,
    );
    println!(
        "   cpu {}",
        micros_line(
            "lightmap_residency, camera-cell change",
            &report.residency_nanos_change
        )
    );
    println!(
        "   cpu {}",
        micros_line("lightmap_residency, steady", &report.residency_nanos_steady)
    );
    for (label, parts) in [
        "steady part prepare_drains",
        "steady part apply_outcome",
        "steady part finish_frame",
    ]
    .into_iter()
    .zip(&report.steady_parts)
    {
        println!("   wall {}", micros_line(label, parts));
    }
    println!(
        "   wall {}",
        micros_line(
            "steady prepare_drains, frames admitting completions",
            &report.prepare_with_completions
        )
    );
    println!(
        "   wall {} (most admitted in one frame {:.2} MiB)",
        micros_line(
            "steady prepare_drains, frames admitting none",
            &report.prepare_without_completions
        ),
        mib(report.max_admitted_bytes),
    );
    println!(
        "   cpu {}",
        micros_line("pool model plan per submitted drain", &pool.plan_nanos)
    );
}
