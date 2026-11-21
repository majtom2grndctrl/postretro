//! One machine-readable result line per residency walk run, beside the prose report.
//! See: context/lib/rendering_pipeline.md §4

// Every run prints `walk-result {json}` on one line, and appends the same JSON
// line to `POSTRETRO_LIGHTMAP_WALK_RESULTS_OUT` when set, so a script can run
// the matrix as separate interleaved invocations into one file and take
// medians per cell. `POSTRETRO_LIGHTMAP_WALK_TAG` labels an invocation (a
// repeat index, a build name). Matrix keys: prl, walk, reads, sh, cores,
// lead_m, pools, drain_cap_mib and in_hand_cap_mib (each null
// when unset), planar, modes.
//
// `late_blocks` is the after-spawn total of drawn blocks still not resident
// after each frame's last drain, in block-frames (deferred + left behind);
// `late_blocks_spawn` is the spawn step's. `sh_late` counts SH visible-miss
// episodes after the spawn step; `sh_late_read_in_flight` and
// `sh_late_held_by_drain_budget` are whole-run episode buckets.
//
// The drain-budget tallies (`budget_tallies`, `drain_tally`) are after the
// spawn step, each with a `_spawn` twin for the spawn step alone:
// - `lm_late_<cause>`: the lightmap controller's drawn-not-resident
//   block-frames by cause, summing to `lm_drawn_not_resident`.
//   `lm_held_frame_uncounted` is the walk's tally past the controller's,
//   summed per frame, and `lm_counted_beyond_walk` the reverse; both are
//   expected 0 (research P9), and the key names are historical.
// - `sh_frames_<cause>`: drawn SH clusters not Sampleable, in cluster-frames,
//   classified on every frame at the frame's first target update.
//   `sh_episodes_<cause>`: SH's episode buckets, each episode filed by its
//   first frame. Both null with SH off.
// - `drains`, `drains_admitting`, `same_class_cuts` (drains whose cut admits
//   one resource and leaves a same-class item of the other),
//   `drain_admitted_bytes_p95` / `_max` over admitting drains, and
//   `largest_drains`: the five largest by decoded bytes admitted, each with
//   `bytes`, `sh_bytes`, `lightmap_bytes` and `items`. Completed mode counts
//   both of a frame's drains.

use std::io::Write;

use serde_json::{Value, json};

use super::levers::Levers;
use super::modes::{ReadsMode, WalkModes};
use super::report::{RunReport, max_and_p95};
use crate::streaming::latency_histogram::LatencyHistogram;
use postretro_camera_route::{Walk, WalkKind};

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
        "drain_cap_mib": keys.levers.drain_cap.map(|cap| cap.mib),
        "in_hand_cap_mib": keys.levers.in_hand_cap.map(|cap| cap.mib),
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
    let tallies_part = Value::Object(report.tallies.json(sh));
    let mut line = serde_json::Map::new();
    for part in [keys_part, lightmap_part, sh_part, decode_part, tallies_part] {
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
    report.tallies.print(report.sh.as_ref());
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

#[cfg(test)]
mod tests {
    use super::super::cores::Cores;
    use super::super::levers::{DEFAULT_LEVERS, parse_drain_cap, parse_in_hand_cap};
    use super::super::modes::Projection;
    use super::*;

    fn line_with(levers: Levers) -> Value {
        let walk = Walk {
            kind: WalkKind::FarPointTour,
            cells: Vec::new(),
            teleport: Vec::new(),
            dwell: Vec::new(),
            travel_yaw: Vec::new(),
            seed: postretro_camera_route::WALK_SEED,
        };
        let modes = WalkModes {
            projection: Projection::default(),
            pitch: 0.0,
            pitch_degrees: 0.0,
            reads: ReadsMode::CompletedReads,
            record_out: None,
            record_in: None,
            planar: false,
            sh: false,
            cores: Cores::Machine,
        };
        let keys = RunKeys {
            prl: "level.prl",
            bake: "bake",
            walk: &walk,
            levers,
            modes: &modes,
            sh: ShStatus::Off,
        };
        result_line(&keys, &RunReport::default())
    }

    // Tagged results tell a capped run from a production one.
    #[test]
    fn result_line_carries_the_drain_cap_and_null_at_production() {
        assert_eq!(line_with(DEFAULT_LEVERS)["drain_cap_mib"], Value::Null);
        let capped = Levers {
            drain_cap: Some(parse_drain_cap("16").unwrap()),
            ..DEFAULT_LEVERS
        };
        assert_eq!(line_with(capped)["drain_cap_mib"], json!(16.0));
    }

    #[test]
    fn result_line_carries_the_in_hand_cap_and_null_at_production() {
        assert_eq!(line_with(DEFAULT_LEVERS)["in_hand_cap_mib"], Value::Null);
        let capped = Levers {
            in_hand_cap: Some(parse_in_hand_cap("64").unwrap()),
            ..DEFAULT_LEVERS
        };
        let line = line_with(capped);
        assert_eq!(line["in_hand_cap_mib"], json!(64.0));
        assert_eq!(line["drain_cap_mib"], Value::Null);
    }
}
