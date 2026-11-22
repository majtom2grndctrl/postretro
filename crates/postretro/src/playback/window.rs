// Playback's telemetry windows: one `playback_window` event per 120 record
// rows, and a final partial when the run ends, each built from the rows the
// record holds and emitted beside the log line that formats it. Per-frame rows
// never reach the sink.
// See: context/lib/development_guide.md §6.5 · context/lib/rendering_pipeline.md §12

use serde::Serialize;

use super::record::{CapReport, Row, RowCpu};
use super::run::RowKind;

/// Record rows per window, matching the CPU timing window.
pub(crate) const WINDOW_ROWS: usize = 120;

/// A frame interval over this many microseconds counts as over budget.
const OVER_BUDGET_US: u64 = 33_000;

const LIGHTMAP_DRAIN_STAGE: &str = "lightmap_drain";
const SH_DRAIN_STAGE: &str = "render_sh_drain";

/// Closes a window every [`WINDOW_ROWS`] record rows, and the final partial
/// one when the run ends.
#[derive(Debug, Default)]
pub(crate) struct PlaybackWindows {
    /// The open window's first row.
    start: usize,
    /// Windows emitted so far: the next one's index.
    emitted: u64,
}

impl PlaybackWindows {
    /// After a row was written: closes the open window once it holds
    /// [`WINDOW_ROWS`] rows.
    pub(crate) fn after_row(&mut self, rows: &[Row], pacing: &'static str, cap: CapReport) {
        if rows.len() >= self.start + WINDOW_ROWS {
            self.close(rows, pacing, cap, false);
        }
    }

    /// The run ended, complete or aborted: one partial window covers the rows
    /// since the last window, if any.
    pub(crate) fn finish(&mut self, rows: &[Row], pacing: &'static str, cap: CapReport) {
        if rows.len() > self.start {
            self.close(rows, pacing, cap, true);
        }
    }

    /// The log line and the sink's flushed line are written synchronously at
    /// the frame's end, so their cost lands in the following row's interval,
    /// which carries no mark: one row in [`WINDOW_ROWS`] reads that much
    /// slower. The cost is a line or two of I/O, and it is accepted.
    fn close(&mut self, rows: &[Row], pacing: &'static str, cap: CapReport, partial: bool) {
        let window = &rows[self.start..];
        let figures = WindowFigures::of(window);
        let event = PlaybackWindowEvent {
            window_index: self.emitted,
            first_frame: window.first().map_or(0, |row| row.frame),
            last_frame: window.last().map_or(0, |row| row.frame),
            figures,
            pacing,
            cap,
            partial,
        };
        log_window(&event);
        postretro_telemetry::emit(postretro_telemetry::Kind::PlaybackWindow, &event);
        self.emitted += 1;
        self.start = rows.len();
    }
}

/// `playback_window`: one window of record rows. Figures cover its measured
/// rows only: route rows carrying no mid-run change and no gap, the rows the
/// record's per-run figures keep. Absent values read `null`, never zero.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct PlaybackWindowEvent {
    pub(crate) window_index: u64,
    /// The record frame index of the window's first and last rows.
    pub(crate) first_frame: u64,
    pub(crate) last_frame: u64,
    #[serde(flatten)]
    pub(crate) figures: WindowFigures,
    /// `frame-locked` or `wall-clock`.
    pub(crate) pacing: &'static str,
    /// The play drain cap in force as the window closed.
    pub(crate) cap: CapReport,
    /// The run ended before the window filled.
    pub(crate) partial: bool,
}

/// What a window's rows give. Computed once at the window's close, from a
/// fixed stack array: nothing allocates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct WindowFigures {
    /// Every record row in the window: hold, route and no-draw.
    pub(crate) rows: usize,
    pub(crate) route_rows: usize,
    /// Route rows with no change or gap mark: the figures' rows.
    pub(crate) measured_rows: usize,
    /// `rows - measured_rows`: hold, no-draw and marked rows.
    pub(crate) excluded_rows: usize,
    /// Nearest-rank p99 of the measured rows' frame intervals; `null` when
    /// none carries an interval.
    pub(crate) frame_time_p99_ms: Option<f64>,
    /// Measured rows whose interval exceeds 33 ms; `null` when none carries
    /// an interval.
    pub(crate) frames_over_33ms: Option<u32>,
    /// The `lightmap_drain` CPU stage's max over the measured rows; `null`
    /// with CPU timing off or when no measured row ran it.
    pub(crate) lightmap_drain_cpu_max_ms: Option<f64>,
    /// The `render_sh_drain` CPU stage's max, as above.
    pub(crate) sh_drain_cpu_max_ms: Option<f64>,
    /// Lightmap blocks drawn while not installed, summed over the measured
    /// rows; named as the record row's column. `null` when the level's
    /// lightmap does not stream, or when the window has no measured rows.
    pub(crate) lightmap_late_block_frames: Option<u64>,
    /// SH clusters drawn while not Sampleable, summed likewise; `null` when
    /// the level's SH does not stream, or when the window has no measured
    /// rows.
    pub(crate) sh_late_cluster_frames: Option<u64>,
}

impl WindowFigures {
    /// `rows` holds at most [`WINDOW_ROWS`] rows.
    pub(crate) fn of(rows: &[Row]) -> Self {
        let mut intervals = [0_u64; WINDOW_ROWS];
        let mut timed = 0;
        let mut over_budget = 0_u32;
        let mut figures = Self {
            rows: rows.len(),
            route_rows: 0,
            measured_rows: 0,
            excluded_rows: 0,
            frame_time_p99_ms: None,
            frames_over_33ms: None,
            lightmap_drain_cpu_max_ms: None,
            sh_drain_cpu_max_ms: None,
            lightmap_late_block_frames: None,
            sh_late_cluster_frames: None,
        };
        for row in rows {
            if row.phase != RowKind::Route {
                continue;
            }
            figures.route_rows += 1;
            if !row.changes.is_empty() || row.gap {
                continue;
            }
            figures.measured_rows += 1;
            if let Some(interval) = row.interval_us
                && timed < WINDOW_ROWS
            {
                intervals[timed] = interval;
                timed += 1;
                over_budget += u32::from(interval > OVER_BUDGET_US);
            }
            let cpu = row.fill.cpu_ns.as_ref();
            max_stage(
                &mut figures.lightmap_drain_cpu_max_ms,
                cpu,
                LIGHTMAP_DRAIN_STAGE,
            );
            max_stage(&mut figures.sh_drain_cpu_max_ms, cpu, SH_DRAIN_STAGE);
            let drain = &row.fill.drain;
            sum_late(
                &mut figures.lightmap_late_block_frames,
                drain.lightmap_late_block_frames,
            );
            sum_late(
                &mut figures.sh_late_cluster_frames,
                drain.sh_late_cluster_frames,
            );
        }
        figures.excluded_rows = figures.rows - figures.measured_rows;
        figures.frames_over_33ms = (timed > 0).then_some(over_budget);
        figures.frame_time_p99_ms = p99_us(&mut intervals[..timed]).map(|us| us as f64 / 1_000.0);
        figures
    }
}

/// Nearest-rank p99: the smallest value at least 99% of `values` do not
/// exceed. Sorts in place.
fn p99_us(values: &mut [u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let rank = (99 * values.len()).div_ceil(100);
    Some(values[rank - 1])
}

fn max_stage(max: &mut Option<f64>, cpu: Option<&RowCpu>, stage: &str) {
    if let Some(ns) = cpu.and_then(|cpu| cpu.value(stage)) {
        let ms = ns as f64 / 1_000_000.0;
        *max = Some(max.map_or(ms, |max| max.max(ms)));
    }
}

fn sum_late(sum: &mut Option<u64>, late: Option<u32>) {
    if let Some(late) = late {
        *sum.get_or_insert(0) += u64::from(late);
    }
}

fn log_window(event: &PlaybackWindowEvent) {
    let figures = &event.figures;
    let ms = |value: Option<f64>| value.map_or_else(|| "null".to_string(), |ms| format!("{ms:.3}"));
    let count = |value: Option<u64>| value.map_or_else(|| "null".to_string(), |n| n.to_string());
    log::info!(
        "[Playback] window {}{} frames {}..={}: rows={} route={} measured={} p99={} ms over_33ms={} lightmap_drain_cpu_max={} ms sh_drain_cpu_max={} ms lightmap_late_block_frames={} sh_late_cluster_frames={} pacing={} cap={} {} B",
        event.window_index,
        if event.partial { " (partial)" } else { "" },
        event.first_frame,
        event.last_frame,
        figures.rows,
        figures.route_rows,
        figures.measured_rows,
        ms(figures.frame_time_p99_ms),
        count(figures.frames_over_33ms.map(u64::from)),
        ms(figures.lightmap_drain_cpu_max_ms),
        ms(figures.sh_drain_cpu_max_ms),
        count(figures.lightmap_late_block_frames),
        count(figures.sh_late_cluster_frames),
        event.pacing,
        event.cap.source,
        event.cap.decoded_bytes_per_drain,
    );
}

#[cfg(test)]
#[path = "window_tests.rs"]
mod tests;
