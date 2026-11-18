//! One machine-readable result line per residency walk run, beside the prose report.
//! See: context/lib/rendering_pipeline.md §4

// Every run prints `walk-result {json}` on one line, and appends the same JSON
// line to `POSTRETRO_LIGHTMAP_WALK_RESULTS_OUT` when set, so a script can run
// the matrix as separate interleaved invocations into one file and take
// medians per cell. `POSTRETRO_LIGHTMAP_WALK_TAG` labels an invocation (a
// repeat index, a build name). Matrix keys: prl, walk, reads, sh, cores,
// lead_m, pools, planar, modes.
//
// `late_blocks` is the after-spawn total of drawn blocks still not resident
// after each frame's last drain, in block-frames (deferred + left behind);
// `late_blocks_spawn` is the spawn step's. `sh_late` counts SH visible-miss
// episodes after the spawn step.

use std::io::Write;

use serde_json::{Value, json};

use super::levers::Levers;
use super::modes::{ReadsMode, WalkModes};
use super::paths::WalkKind;
use super::report::{RunReport, max_and_p95};
use super::walk::Walk;
use crate::streaming::latency_histogram::LatencyHistogram;

const RESULTS_OUT_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_RESULTS_OUT";
const TAG_ENV: &str = "POSTRETRO_LIGHTMAP_WALK_TAG";

/// Whether this run streamed SH, and why not when asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ShStatus {
    Off,
    On,
    /// Asked for, but the level has no streamed id 50.
    NoId50,
}

impl ShStatus {
    fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::On => "on",
            Self::NoId50 => "no-id50",
        }
    }
}

/// What a run was, beside what it measured.
pub(super) struct RunKeys<'a> {
    pub(super) prl: &'a str,
    pub(super) bake: &'a str,
    pub(super) walk: &'a Walk,
    pub(super) levers: Levers,
    pub(super) modes: &'a WalkModes,
    pub(super) sh: ShStatus,
}

/// The run's result line.
pub(super) fn result_line(keys: &RunKeys<'_>, report: &RunReport) -> Value {
    let pool = &report.pool;
    let (drain_max, drain_p95) = max_and_p95(pool.drain_install_bytes.clone());
    let (play, spawn) = (&report.causes_play, &report.causes_spawn);
    let growths = pool.growths();
    let sh = report.sh.as_ref();
    // In parts: one `json!` this long overflows the macro recursion limit.
    let keys_part = json!({
        "tag": std::env::var(TAG_ENV).unwrap_or_default(),
        "prl": keys.prl,
        "bake": keys.bake,
        "walk": match keys.walk.kind {
            WalkKind::Random => "random",
            WalkKind::FarPointTour => "far-point",
        },
        "reads": match keys.modes.reads {
            ReadsMode::RealTime => "realtime",
            ReadsMode::CompletedReads => "completed",
        },
        "sh": keys.sh.key(),
        "cores": keys.modes.cores.key(),
        "cores_label": keys.modes.cores.label(),
        "lead_m": keys.levers.lead_metres,
        "pools": keys.levers.pools_label(),
        "planar": keys.modes.planar,
        "modes": keys.modes.label(),
    });
    let lightmap_part = json!({
        "frames": report.frames,
        "paced_frames": report.paced_frames,
        "wall_s": report.wall.as_secs_f64(),
        "installs": pool.installs,
        "uploads": pool.uploads,
        "install_bytes": pool.install_bytes,
        "payload_hash": pool.payload_hash.finish(),
        "hashed_uploads": pool.payload_hash.uploads(),
        "drain_install_bytes_max": drain_max,
        "drain_install_bytes_p95": drain_p95,
        "late_blocks": play.deferred + play.left_behind,
        "late_deferred": play.deferred,
        "late_left_behind": play.left_behind,
        "late_blocks_spawn": spawn.deferred + spawn.left_behind,
        "pool_growths": growths,
        "pool_deferred": pool.deferred,
        "pool_evictions": pool.evictions,
        "pool_refusals": pool.refusals,
        "pool_failed": pool.failed,
        "physical_reads": report.physical_reads,
        "issuer_reads": report.issuer_busy.reads,
        "issuer_busy_p95_ms": report.issuer_busy.p95_ms,
        "issuer_busy_max_ms": report.issuer_busy.max_ms,
        "issuer_busy_total_ms": report.issuer_busy.total_ms,
    });
    let sh_part = json!({
        "sh_late": sh.map(|sh| sh.late_after_spawn()),
        "sh_late_spawn": sh.map(|sh| sh.spawn_misses),
        "sh_installs": sh.map(|sh| sh.counters.installs),
        "sh_late_read_in_flight": sh.map(|sh| sh.counters.miss_buckets.read_in_flight),
        "sh_late_held_by_drain_budget":
            sh.map(|sh| sh.counters.miss_buckets.held_by_drain_budget),
        "sh_max_drain_decoded_bytes": sh.map(|sh| sh.counters.max_drain_decoded_bytes),
    });
    let sh_workers = sh.and_then(|sh| sh.workers);
    let quantiles = |histogram: Option<&LatencyHistogram>| {
        histogram.map(|h| [h.quantile_ms(0.5), h.quantile_ms(0.95), h.max_ms()])
    };
    let decode_part = json!({
        "lightmap_decode_queue_wait_ms": quantiles(Some(&report.lightmap_decode.queue_wait)),
        "lightmap_decode_ms": quantiles(Some(&report.lightmap_decode.decode_time)),
        "lightmap_decodes": report.lightmap_decode.decode_time.samples(),
        "sh_decode_queue_wait_ms": quantiles(sh_workers.as_ref().map(|w| &w.decode_queue_wait)),
        "sh_decode_ms": quantiles(sh_workers.as_ref().map(|w| &w.decode_latency)),
        "sh_read_ms": quantiles(sh_workers.as_ref().map(|w| &w.read_latency)),
    });
    let mut line = serde_json::Map::new();
    for part in [keys_part, lightmap_part, sh_part, decode_part] {
        if let Value::Object(fields) = part {
            line.extend(fields);
        }
    }
    Value::Object(line)
}

/// The prose lines for this task's instruments, then the result line, printed
/// and appended to the results file when one is named.
pub(super) fn print_instruments(keys: &RunKeys<'_>, report: &RunReport) {
    let pool = &report.pool;
    println!(
        "   payload hash {} over {} uploads (preload included); installs {}, late blocks after \
         spawn {} (deferred {}, left behind {})",
        pool.payload_hash.finish(),
        pool.payload_hash.uploads(),
        pool.installs,
        report.causes_play.deferred + report.causes_play.left_behind,
        report.causes_play.deferred,
        report.causes_play.left_behind,
    );
    println!("   issuer: {}", report.issuer_busy.line());
    println!(
        "   lightmap decode: queue wait {}; decode {}",
        report.lightmap_decode.queue_wait.summary(),
        report.lightmap_decode.decode_time.summary(),
    );
    match (&report.sh, keys.sh) {
        (Some(sh), _) => println!("   SH on: {}", sh.line()),
        (None, ShStatus::NoId50) => {
            println!("   SH requested but the level has no streamed id 50: SH off")
        }
        (None, _) => println!("   SH off"),
    }
    let line = result_line(keys, report);
    println!("walk-result {line}");
    if let Ok(path) = std::env::var(RESULTS_OUT_ENV) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap_or_else(|why| panic!("open {RESULTS_OUT_ENV} {path}: {why}"));
        writeln!(file, "{line}").expect("append the result line");
    }
}
