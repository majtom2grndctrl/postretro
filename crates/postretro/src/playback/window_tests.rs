// Playback's telemetry windows: figures equal an independent reduction of the
// record's rows, one event per 120 rows and none per frame, and a final
// partial before `run_end` when a run completes or aborts.
// See: context/lib/development_guide.md §6.5 · context/lib/testing_guide.md §3 (Telemetry assertions)

use std::sync::Arc;
use std::time::{Duration, Instant};

use postretro_telemetry::{Envelope, Hub, HubConfig, Kind, Source};

use super::*;
use crate::playback::lifecycle::tests::{Run, frame, playing, record};
use crate::playback::marks::{ChangeKind, ChangeSet};
use crate::playback::record::{RowDrain, RowFill};

const CAP: CapReport = CapReport {
    source: "seed",
    decoded_bytes_per_drain: 4 * 1024 * 1024,
};

fn scoped_hub() -> (Arc<Hub>, postretro_telemetry::scoped::ScopedHub) {
    let hub = Arc::new(Hub::start(HubConfig {
        memory: true,
        ..HubConfig::new(Source::Engine, "t")
    }));
    let scope = postretro_telemetry::scoped::install(Arc::clone(&hub));
    (hub, scope)
}

fn events(hub: &Hub) -> Vec<Envelope> {
    hub.lines()
        .iter()
        .map(|line| Envelope::parse_line(line).unwrap())
        .collect()
}

fn windows(hub: &Hub) -> Vec<serde_json::Value> {
    events(hub)
        .into_iter()
        .filter(|event| event.kind == Kind::PlaybackWindow)
        .map(|event| event.payload)
        .collect()
}

fn row(frame: u64, phase: RowKind, interval_us: Option<u64>) -> Row {
    Row {
        frame,
        phase,
        pose: 0,
        route_time_s: None,
        interval_us,
        changes: ChangeSet::default(),
        gap: false,
        fill: RowFill::default(),
        gpu: None,
    }
}

/// 120 rows: hold, route, no-draw, a gap, then a mid-run change to the end;
/// route rows carry varied intervals, CPU drain stages and late counts.
fn mixed_rows() -> Vec<Row> {
    (0..WINDOW_ROWS as u64)
        .map(|frame| {
            let phase = match frame {
                0..10 => RowKind::Hold,
                50 => RowKind::NoDraw,
                _ => RowKind::Route,
            };
            let interval = (frame > 0).then(|| 16_000 + (frame * 7_919) % 30_000);
            let mut row = row(frame, phase, interval);
            row.gap = frame == 60;
            if frame >= 110 {
                row.changes.insert(ChangeKind::DrainCap);
            }
            row.fill = RowFill {
                cpu_ns: Some(RowCpu::with_values(&[
                    ("lightmap_drain", 10_000 + frame * 1_000),
                    ("render_sh_drain", 500_000 - frame * 1_000),
                ])),
                ticks: Some(1),
                drain: RowDrain {
                    lightmap_late_block_frames: Some((frame % 3) as u32),
                    sh_late_cluster_frames: Some((frame % 5) as u32),
                    ..RowDrain::default()
                },
            };
            row
        })
        .collect()
}

/// The figures computed the long way, from the rows the record's per-run
/// figures keep: route rows with no change and no gap.
fn reduced(rows: &[Row]) -> WindowFigures {
    let measured: Vec<&Row> = rows
        .iter()
        .filter(|row| row.phase == RowKind::Route && row.changes.is_empty() && !row.gap)
        .collect();
    let mut intervals: Vec<f64> = measured
        .iter()
        .filter_map(|row| row.interval_us)
        .map(|us| us as f64 / 1_000.0)
        .collect();
    intervals.sort_by(f64::total_cmp);
    let p99 = (!intervals.is_empty())
        .then(|| intervals[(0.99 * intervals.len() as f64).ceil() as usize - 1]);
    let stage_max = |label: &str| {
        measured
            .iter()
            .filter_map(|row| row.fill.cpu_ns.and_then(|cpu| cpu.value(label)))
            .map(|ns| ns as f64 / 1_000_000.0)
            .reduce(f64::max)
    };
    let late = |pick: fn(&RowDrain) -> Option<u32>| {
        measured
            .iter()
            .filter_map(|row| pick(&row.fill.drain))
            .map(u64::from)
            .reduce(|a, b| a + b)
    };
    WindowFigures {
        rows: rows.len(),
        route_rows: rows
            .iter()
            .filter(|row| row.phase == RowKind::Route)
            .count(),
        measured_rows: measured.len(),
        excluded_rows: rows.len() - measured.len(),
        frame_time_p99_ms: p99,
        frames_over_33ms: (!intervals.is_empty())
            .then(|| intervals.iter().filter(|ms| **ms > 33.0).count() as u32),
        lightmap_drain_cpu_max_ms: stage_max("lightmap_drain"),
        sh_drain_cpu_max_ms: stage_max("render_sh_drain"),
        late_lightmap_block_frames: late(|drain| drain.lightmap_late_block_frames),
        late_sh_cluster_frames: late(|drain| drain.sh_late_cluster_frames),
    }
}

#[test]
fn playback_window_figures_equal_an_independent_reduction_of_the_record_rows() {
    // AC 31: hold, no-draw, gap and change-marked rows leave the figures.
    let rows = mixed_rows();
    let figures = WindowFigures::of(&rows);
    assert_eq!(figures, reduced(&rows));
    assert_eq!(figures.route_rows, 109);
    assert_eq!(
        figures.measured_rows, 98,
        "less one gap and ten changed rows"
    );
    assert_eq!(figures.excluded_rows, 22);
    assert!(figures.frames_over_33ms.unwrap() > 0);
    assert!(figures.frame_time_p99_ms.unwrap() > 33.0);
}

#[test]
fn playback_window_reads_null_for_absent_figures_never_zero() {
    // CPU timing off: no split on any row, so the drain maxima are null.
    let mut rows = mixed_rows();
    for row in &mut rows {
        row.fill.cpu_ns = None;
        row.fill.drain.sh_late_cluster_frames = None;
    }
    let figures = WindowFigures::of(&rows);
    assert_eq!(figures, reduced(&rows));
    assert_eq!(figures.lightmap_drain_cpu_max_ms, None);
    assert_eq!(figures.sh_drain_cpu_max_ms, None);
    assert_eq!(figures.late_sh_cluster_frames, None, "SH does not stream");
    let json = serde_json::to_value(figures).unwrap();
    assert!(json["lightmap_drain_cpu_max_ms"].is_null());
    assert!(json["sh_drain_cpu_max_ms"].is_null());

    // A hold-only window measures nothing.
    let hold: Vec<Row> = (0..12)
        .map(|frame| row(frame, RowKind::Hold, Some(16_000)))
        .collect();
    let figures = WindowFigures::of(&hold);
    assert_eq!((figures.rows, figures.measured_rows), (12, 0));
    assert_eq!(figures.frame_time_p99_ms, None);
    assert_eq!(figures.frames_over_33ms, None);
    assert_eq!(figures.late_lightmap_block_frames, None);
}

#[test]
fn playback_windows_emit_once_per_120_rows_and_a_final_partial() {
    let (hub, _scope) = scoped_hub();
    let mut windows_state = PlaybackWindows::default();
    let mut rows = Vec::new();
    for frame in 0..250_u64 {
        rows.push(row(frame, RowKind::Route, Some(16_000)));
        windows_state.after_row(&rows, "frame-locked", CAP);
        let expected = rows.len() / WINDOW_ROWS;
        assert_eq!(windows(&hub).len(), expected, "after {} rows", rows.len());
    }
    windows_state.finish(&rows, "frame-locked", CAP);
    windows_state.finish(&rows, "frame-locked", CAP);
    let emitted = windows(&hub);
    assert_eq!(emitted.len(), 3, "two full windows and one partial");
    for (index, (first, last, rows, partial)) in [
        (0, 119, 120, false),
        (120, 239, 120, false),
        (240, 249, 10, true),
    ]
    .into_iter()
    .enumerate()
    {
        let event = &emitted[index];
        assert_eq!(event["window_index"], index as u64);
        assert_eq!(event["first_frame"], first);
        assert_eq!(event["last_frame"], last);
        assert_eq!(event["rows"], rows);
        assert_eq!(event["partial"], partial);
        assert_eq!(event["pacing"], "frame-locked");
        assert_eq!(event["cap"]["source"], "seed");
        assert_eq!(event["cap"]["decoded_bytes_per_drain"], 4 * 1024 * 1024);
    }

    // A run ending on a window's last row writes no empty partial.
    let (hub, _scope) = scoped_hub();
    let mut windows_state = PlaybackWindows::default();
    let rows: Vec<Row> = (0..120)
        .map(|frame| row(frame, RowKind::Route, Some(16_000)))
        .collect();
    windows_state.after_row(&rows, "wall-clock", CAP);
    windows_state.finish(&rows, "wall-clock", CAP);
    assert_eq!(windows(&hub).len(), 1);
}

/// Runs `frames` frames of `run`, each `interval(k)` after the previous.
fn run_frames(run: &mut Run, start: Instant, frames: u64, interval: fn(u64) -> u64) -> Instant {
    let mut now = start;
    for k in 0..frames {
        now += Duration::from_micros(interval(k));
        frame(&mut run.app, now, true, true);
    }
    now
}

/// The window's figures as the written record's rows over the same frames
/// give them: route rows with no change and no gap.
fn figures_from_record(record: &serde_json::Value, first: u64, last: u64) -> serde_json::Value {
    let rows: Vec<&serde_json::Value> = record["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| (first..=last).contains(&row["frame"].as_u64().unwrap()))
        .collect();
    let measured: Vec<&&serde_json::Value> = rows
        .iter()
        .filter(|row| {
            row["phase"] == "route"
                && row["changes"].as_array().unwrap().is_empty()
                && row["gap"] == false
        })
        .collect();
    let mut intervals: Vec<f64> = measured
        .iter()
        .filter_map(|row| row["interval_us"].as_u64())
        .map(|us| us as f64 / 1_000.0)
        .collect();
    intervals.sort_by(f64::total_cmp);
    let p99 = intervals[(0.99 * intervals.len() as f64).ceil() as usize - 1];
    serde_json::json!({
        "rows": rows.len(),
        "measured_rows": measured.len(),
        "frame_time_p99_ms": p99,
        "frames_over_33ms": intervals.iter().filter(|ms| **ms > 33.0).count(),
        "lightmap_drain_cpu_max_ms": null,
        "sh_drain_cpu_max_ms": null,
    })
}

fn assert_window_matches_record(window: &serde_json::Value, record: &serde_json::Value) {
    let first = window["first_frame"].as_u64().unwrap();
    let last = window["last_frame"].as_u64().unwrap();
    let expected = figures_from_record(record, first, last);
    for key in [
        "rows",
        "measured_rows",
        "frame_time_p99_ms",
        "frames_over_33ms",
        "lightmap_drain_cpu_max_ms",
        "sh_drain_cpu_max_ms",
    ] {
        assert_eq!(window[key], expected[key], "{key} over {first}..={last}");
    }
}

#[test]
fn a_completed_run_writes_its_final_partial_window_before_run_end() {
    // AC 31, 32 (P18): with CPU timing off the drain maxima read null; the
    // window names the seeded cap and the pacing.
    let (hub, _scope) = scoped_hub();
    let mut run = playing(2, 130, &[]);
    run.app
        .session
        .as_mut()
        .unwrap()
        .level_streaming
        .apply_play_drain_cap_seed(Ok("4".to_string()));
    let now = run_frames(&mut run, Instant::now(), 119, |k| 16_000 + (k % 9) * 3_000);
    assert!(windows(&hub).is_empty(), "no event per frame");
    run_frames(&mut run, now, 13, |_| 40_000);
    assert!(
        run.app.playback.active.is_none() && run.app.exit_result.is_ok(),
        "the route completed"
    );
    crate::telemetry::end_engine_run(&Ok(()));

    let all = events(&hub);
    assert_eq!(all.last().unwrap().kind, Kind::RunEnd);
    let emitted = windows(&hub);
    assert_eq!(emitted.len(), 2, "one full window, one partial");
    assert_eq!(emitted[0]["partial"], false);
    assert_eq!(
        (
            emitted[1]["first_frame"].as_u64(),
            emitted[1]["last_frame"].as_u64()
        ),
        (Some(120), Some(131))
    );
    assert_eq!(emitted[1]["partial"], true);
    let record = record(&run);
    assert_eq!(record["header"]["cap"]["source"], "seed");
    for window in &emitted {
        assert_window_matches_record(window, &record);
        assert_eq!(window["pacing"], "frame-locked");
        assert_eq!(window["cap"]["source"], "seed");
        assert_eq!(window["cap"]["decoded_bytes_per_drain"], 4 * 1024 * 1024);
        assert!(window["lightmap_drain_cpu_max_ms"].is_null());
    }
}

#[test]
fn an_aborted_run_writes_its_final_partial_window_before_run_end() {
    let (hub, _scope) = scoped_hub();
    let mut run = playing(0, 50, &[]);
    run_frames(&mut run, Instant::now(), 10, |_| 16_000);
    assert!(
        run.app
            .playback_abort(Some("level change requested".to_string()))
    );
    crate::telemetry::end_engine_run(&Ok(()));

    let kinds: Vec<Kind> = events(&hub).iter().map(|event| event.kind).collect();
    let window_at = kinds
        .iter()
        .position(|kind| *kind == Kind::PlaybackWindow)
        .unwrap();
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == Kind::PlaybackWindow)
            .count(),
        1
    );
    assert!(window_at < kinds.len() - 1 && kinds.last() == Some(&Kind::RunEnd));
    let window = &windows(&hub)[0];
    assert_eq!(window["partial"], true);
    assert_eq!(window["rows"], 10);
    assert_eq!(window["cap"]["source"], "production");
    assert_window_matches_record(window, &record(&run));
}
