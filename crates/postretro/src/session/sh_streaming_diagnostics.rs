//! Live SH streaming diagnostics: one plain-data view assembled from the
//! controller, the worker threads, and the renderer, plus the periodic
//! `[SH streaming]` log line.
//! See: context/lib/rendering_pipeline.md §"Cluster SH residency".

use postretro_renderer::{ShResidencySnapshot, ShStreamingLiveDiagnostics};

use super::sh_async_workers::ShWorkerStats;
use crate::sh_streaming::controller::{MAX_STREAM_PERMITS, ShResidencyControllerSnapshot};

/// Minimum monotonic render time between two periodic log lines.
pub(super) const DIAGNOSTICS_LOG_INTERVAL_SECONDS: f64 = 5.0;

/// Refreshes `live` in place. A `None` source leaves its fields as they were,
/// so worker and renderer values survive a frame that lacks them.
pub(super) fn assemble_live_diagnostics(
    live: &mut ShStreamingLiveDiagnostics,
    controller: &ShResidencyControllerSnapshot,
    worker: Option<&ShWorkerStats>,
    renderer: Option<&ShResidencySnapshot>,
) {
    let count = |value: usize| u64::try_from(value).unwrap_or(u64::MAX);
    live.target_clusters = count(controller.target_clusters);
    live.warm_clusters = count(controller.warm_clusters);
    live.sampleable_clusters = count(controller.sampleable_clusters);
    live.queued_clusters = count(controller.queued_clusters);
    live.ready_clusters = count(controller.ready_clusters);
    live.permits_in_use = count(controller.permits_in_use);
    let counters = &controller.counters;
    live.misses = counters.misses;
    live.installs = counters.installs;
    live.evictions = counters.evictions;
    live.retries = counters.retries;
    live.cancelled_requests = counters.cancelled_requests;
    live.discarded_reads = counters.discarded_reads;
    live.discarded_read_bytes = counters.discarded_read_bytes;
    live.decoded_bytes_installed = counters.decoded_bytes_installed;
    live.last_drain_decoded_bytes = counters.last_drain_decoded_bytes;
    live.max_drain_decoded_bytes = counters.max_drain_decoded_bytes;
    live.budget_limited_drains = counters.budget_limited_drains;
    if let Some(worker) = worker {
        live.reads_issued = worker.reads_issued;
        live.coalesced_reads = worker.coalesced_reads;
        live.read_bytes = worker.read_bytes;
        live.gap_bytes = worker.gap_bytes;
        live.read_latency_p50_ms = worker.read_latency.quantile_ms(0.50);
        live.read_latency_p95_ms = worker.read_latency.quantile_ms(0.95);
        live.read_latency_max_ms = worker.read_latency.max_ms();
        live.decode_latency_max_ms = worker.decode_latency.max_ms();
    }
    // The renderer owns its sampleable count; it overrides the controller's.
    if let Some(renderer) = renderer {
        live.record_renderer_snapshot(renderer);
    }
}

/// Throttles the periodic log line. A line needs both a full interval since
/// the previous line and a cumulative counter that moved since then, so an
/// idle session stays silent.
#[derive(Debug, Default)]
pub(super) struct ShStreamingLogWindow {
    window_start: Option<f64>,
    baseline: ShStreamingLiveDiagnostics,
}

impl ShStreamingLogWindow {
    pub(super) fn observe(&mut self, now_seconds: f64, live: &ShStreamingLiveDiagnostics) {
        if let Some(line) = self.line_if_due(now_seconds, live) {
            log::info!("{line}");
        }
    }

    fn line_if_due(
        &mut self,
        now_seconds: f64,
        live: &ShStreamingLiveDiagnostics,
    ) -> Option<String> {
        let start = *self.window_start.get_or_insert(now_seconds);
        let elapsed = now_seconds - start;
        if elapsed < DIAGNOSTICS_LOG_INTERVAL_SECONDS
            || cumulative_counters(live) == cumulative_counters(&self.baseline)
        {
            return None;
        }
        let line = format_line(elapsed, &self.baseline, live);
        self.baseline = live.clone();
        self.window_start = Some(now_seconds);
        Some(line)
    }
}

fn cumulative_counters(d: &ShStreamingLiveDiagnostics) -> [u64; 16] {
    [
        d.misses,
        d.installs,
        d.evictions,
        d.retries,
        d.cancelled_requests,
        d.discarded_reads,
        d.discarded_read_bytes,
        d.decoded_bytes_installed,
        d.budget_limited_drains,
        d.reads_issued,
        d.coalesced_reads,
        d.read_bytes,
        d.gap_bytes,
        d.install_cpu_total_micros,
        d.pool_growth_events,
        d.pool_growth_bytes,
    ]
}

/// Window deltas first, then current gauges and running maxima, on one line.
fn format_line(
    elapsed_seconds: f64,
    before: &ShStreamingLiveDiagnostics,
    now: &ShStreamingLiveDiagnostics,
) -> String {
    let delta =
        |pick: fn(&ShStreamingLiveDiagnostics) -> u64| pick(now).saturating_sub(pick(before));
    format!(
        "[SH streaming] last {elapsed_seconds:.1} s: {reads} reads ({coalesced} coalesced) \
         {read_bytes} incl. {gap_bytes} gap, {installs} installs {installed_bytes}, \
         {misses} misses, {evictions} evictions, {retries} retries, {cancelled} cancelled, \
         {discarded} discarded ({discarded_bytes}), {budget_limited} budget-limited drains, \
         {growths} pool growths (+{growth_bytes}), install CPU {install_ms:.2} ms \
         | now: {targets} targets ({warm} warm), {sampleable} sampleable, {queued} queued, \
         {ready} ready, {permits}/{MAX_STREAM_PERMITS} permits, pool {occupancy} of {capacity}, \
         read latency p50 {p50:.1} / p95 {p95:.1} / max {read_max:.1} ms, \
         decode max {decode_max:.1} ms, largest drain {largest_drain}, \
         slowest install {slowest_install_ms:.2} ms",
        reads = delta(|d| d.reads_issued),
        coalesced = delta(|d| d.coalesced_reads),
        read_bytes = format_bytes(delta(|d| d.read_bytes)),
        gap_bytes = format_bytes(delta(|d| d.gap_bytes)),
        installs = delta(|d| d.installs),
        installed_bytes = format_bytes(delta(|d| d.decoded_bytes_installed)),
        misses = delta(|d| d.misses),
        evictions = delta(|d| d.evictions),
        retries = delta(|d| d.retries),
        cancelled = delta(|d| d.cancelled_requests),
        discarded = delta(|d| d.discarded_reads),
        discarded_bytes = format_bytes(delta(|d| d.discarded_read_bytes)),
        budget_limited = delta(|d| d.budget_limited_drains),
        growths = delta(|d| d.pool_growth_events),
        growth_bytes = format_bytes(delta(|d| d.pool_growth_bytes)),
        install_ms = delta(|d| d.install_cpu_total_micros) as f64 / 1000.0,
        targets = now.target_clusters,
        warm = now.warm_clusters,
        sampleable = now.sampleable_clusters,
        queued = now.queued_clusters,
        ready = now.ready_clusters,
        permits = now.permits_in_use,
        occupancy = format_bytes(now.logical_occupancy_bytes),
        capacity = format_bytes(now.active_capacity_bytes),
        p50 = now.read_latency_p50_ms,
        p95 = now.read_latency_p95_ms,
        read_max = now.read_latency_max_ms,
        decode_max = now.decode_latency_max_ms,
        largest_drain = format_bytes(now.max_drain_decoded_bytes),
        slowest_install_ms = now.install_cpu_max_drain_micros as f64 / 1000.0,
    )
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let value = bytes as f64;
    if value >= MIB {
        format!("{:.1} MiB", value / MIB)
    } else if value >= KIB {
        format!("{:.1} KiB", value / KIB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sh_streaming::controller::ShResidencyCounters;
    use postretro_test_log_capture::LogCapture;

    #[test]
    fn assembly_fills_controller_worker_and_renderer_fields() {
        let mut live = ShStreamingLiveDiagnostics::default();
        let controller = ShResidencyControllerSnapshot {
            target_clusters: 12,
            warm_clusters: 8,
            sampleable_clusters: 2,
            permits_in_use: 3,
            counters: ShResidencyCounters {
                misses: 4,
                cancelled_requests: 5,
                budget_limited_drains: 6,
                ..ShResidencyCounters::default()
            },
            ..ShResidencyControllerSnapshot::default()
        };
        let worker = ShWorkerStats {
            reads_issued: 7,
            coalesced_reads: 2,
            gap_bytes: 9,
            ..ShWorkerStats::default()
        };
        let renderer = ShResidencySnapshot {
            sampleable_clusters: 10,
            install_cpu_total_micros: 11,
            ..ShResidencySnapshot::default()
        };
        assemble_live_diagnostics(&mut live, &controller, Some(&worker), Some(&renderer));
        assert_eq!(live.target_clusters, 12);
        assert_eq!(live.warm_clusters, 8);
        assert_eq!(live.permits_in_use, 3);
        assert_eq!(live.misses, 4);
        assert_eq!(live.cancelled_requests, 5);
        assert_eq!(live.budget_limited_drains, 6);
        assert_eq!(live.reads_issued, 7);
        assert_eq!(live.coalesced_reads, 2);
        assert_eq!(live.gap_bytes, 9);
        assert_eq!(live.sampleable_clusters, 10, "renderer owns sampleable");
        assert_eq!(live.install_cpu_total_micros, 11);

        // Frames without worker or renderer input keep the last values.
        assemble_live_diagnostics(&mut live, &controller, None, None);
        assert_eq!(live.reads_issued, 7);
        assert_eq!(live.install_cpu_total_micros, 11);
    }

    #[test]
    fn log_line_fires_once_per_interval_while_counters_change_and_never_when_idle() {
        let capture = LogCapture::start();
        let mut window = ShStreamingLogWindow::default();
        let mut live = ShStreamingLiveDiagnostics::default();
        // 60 Hz for 20 s with a read every frame: one line per 5 s window.
        for frame in 0..=1200u64 {
            live.reads_issued = frame;
            window.observe(frame as f64 / 60.0, &live);
        }
        let lines = |capture: &LogCapture| {
            capture
                .records()
                .iter()
                .filter(|record| record.message.starts_with("[SH streaming] last"))
                .count()
        };
        assert_eq!(lines(&capture), 4);

        // Another 20 s with frozen counters: silent.
        for frame in 1201..=2400u64 {
            window.observe(frame as f64 / 60.0, &live);
        }
        assert_eq!(lines(&capture), 4);

        // Activity after the idle stretch logs at once: the interval since the
        // previous line has long passed.
        live.installs += 1;
        window.observe(2401.0 / 60.0, &live);
        assert_eq!(lines(&capture), 5);
        capture.assert_logged(log::Level::Info, "1 installs");
    }

    #[test]
    fn log_line_reports_window_deltas_and_current_gauges() {
        let before = ShStreamingLiveDiagnostics {
            reads_issued: 10,
            read_bytes: 1024 * 1024,
            ..ShStreamingLiveDiagnostics::default()
        };
        let now = ShStreamingLiveDiagnostics {
            reads_issued: 13,
            coalesced_reads: 1,
            read_bytes: 3 * 1024 * 1024,
            gap_bytes: 2048,
            target_clusters: 24,
            warm_clusters: 8,
            permits_in_use: 4,
            read_latency_p95_ms: 9.3,
            ..ShStreamingLiveDiagnostics::default()
        };
        let line = format_line(5.0, &before, &now);
        assert!(line.starts_with("[SH streaming] last 5.0 s: 3 reads (1 coalesced)"));
        assert!(line.contains("2.0 MiB incl. 2.0 KiB gap"), "{line}");
        assert!(line.contains("24 targets (8 warm)"), "{line}");
        assert!(line.contains("4/8 permits"), "{line}");
        assert!(line.contains("p95 9.3"), "{line}");
        assert!(!line.contains('\n'));
    }
}
