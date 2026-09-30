//! Capture-only SH residency summary: controller, worker, and renderer figures.
//! See: context/lib/rendering_pipeline.md §4

use anyhow::Result;
use postretro_renderer::ShStreamingLifecycleSummary;

use super::super::sh_async_workers::ShAsyncWorkers;
use super::super::sh_streaming_diagnostics::assemble_live_diagnostics;
use super::ShStreamingSession;
use crate::sh_streaming::budget::BytePhase;

impl ShStreamingSession {
    /// Merge the controller's frame-local policy/permit view with worker
    /// phase bytes for capture. The renderer contributes pool capacity through
    /// its own snapshot; it deliberately never reaches into this session.
    #[cfg(feature = "capture")]
    pub(crate) fn residency_lifecycle_summary(&self) -> Result<ShStreamingLifecycleSummary> {
        let controller = self.controller.report_snapshot();
        let worker = self
            .workers
            .as_ref()
            .map(ShAsyncWorkers::phase_snapshot)
            .transpose()
            .map_err(anyhow::Error::msg)?
            .unwrap_or_default();
        let phase = |controller_phase: BytePhase, worker_phase: BytePhase, label| {
            let current = controller_phase
                .current_bytes
                .checked_add(worker_phase.current_bytes)
                .ok_or_else(|| anyhow::anyhow!("[SH streaming] {label} current bytes overflow"))?;
            // The independently checked ledgers cannot reconstruct one exact
            // combined historic instant. Their sum is a checked conservative
            // upper bound across worker/controller ownership transfer; expose
            // it under that explicit name rather than mislabeling it a peak.
            let high_water_upper_bound = controller_phase
                .high_water_bytes
                .checked_add(worker_phase.high_water_bytes)
                .ok_or_else(|| {
                    anyhow::anyhow!("[SH streaming] {label} high-water upper bound overflow")
                })?;
            Ok::<_, anyhow::Error>((current, high_water_upper_bound))
        };
        let (encoded_current_bytes, encoded_high_water_upper_bound_bytes) =
            phase(controller.cpu.encoded, worker.encoded, "encoded")?;
        let (decoding_current_bytes, decoding_high_water_upper_bound_bytes) =
            phase(controller.cpu.decoding, worker.decoding, "decoding")?;
        let (ready_current_bytes, ready_high_water_upper_bound_bytes) =
            phase(controller.cpu.ready, worker.ready, "ready")?;
        // Renderer fields come from the last applied outcome's snapshot.
        let mut live = self.live.clone();
        let worker_stats = self.read_stats()?;
        assemble_live_diagnostics(&mut live, &controller, worker_stats.as_ref(), None);
        let count = |value: usize, label: &'static str| {
            u64::try_from(value).map_err(|_| anyhow::anyhow!("[SH streaming] {label} exceeds u64"))
        };
        Ok(ShStreamingLifecycleSummary {
            non_evictable_overshoot_bytes: controller.non_evictable_overshoot_bytes,
            encoded_current_bytes,
            encoded_high_water_upper_bound_bytes,
            decoding_current_bytes,
            decoding_high_water_upper_bound_bytes,
            ready_current_bytes,
            ready_high_water_upper_bound_bytes,
            permits_in_use: count(controller.permits_in_use, "permit count")?,
            target_clusters: count(controller.target_clusters, "target count")?,
            absent_clusters: count(controller.absent_clusters, "absent state count")?,
            queued_clusters: count(controller.queued_clusters, "queued state count")?,
            ready_clusters: count(controller.ready_clusters, "ready state count")?,
            installed_uncomposed_clusters: count(
                controller.installed_uncomposed_clusters,
                "installed state count",
            )?,
            sampleable_clusters: count(controller.sampleable_clusters, "sampleable state count")?,
            failed_clusters: count(controller.failed_clusters, "failed state count")?,
            misses: controller.counters.misses,
            installs: controller.counters.installs,
            evictions: controller.counters.evictions,
            retries: controller.counters.retries,
            warm_clusters: live.warm_clusters,
            cancelled_requests: live.cancelled_requests,
            discarded_reads: live.discarded_reads,
            discarded_read_bytes: live.discarded_read_bytes,
            decoded_bytes_installed: live.decoded_bytes_installed,
            last_drain_decoded_bytes: live.last_drain_decoded_bytes,
            max_drain_decoded_bytes: live.max_drain_decoded_bytes,
            budget_limited_drains: live.budget_limited_drains,
            reads_issued: live.reads_issued,
            coalesced_reads: live.coalesced_reads,
            read_bytes: live.read_bytes,
            gap_bytes: live.gap_bytes,
            read_latency_p50_ms: live.read_latency_p50_ms,
            read_latency_p95_ms: live.read_latency_p95_ms,
            read_latency_max_ms: live.read_latency_max_ms,
            decode_latency_max_ms: live.decode_latency_max_ms,
            install_cpu_total_micros: live.install_cpu_total_micros,
            install_cpu_max_drain_micros: live.install_cpu_max_drain_micros,
            install_cpu_last_drain_micros: live.install_cpu_last_drain_micros,
            install_cpu_max_steady_drain_micros: live.install_cpu_max_steady_drain_micros,
            pool_growth_events: live.pool_growth_events,
            pool_growth_bytes: live.pool_growth_bytes,
            pool_growth_cpu_micros: live.pool_growth_cpu_micros,
            indirect_compose: live.indirect_compose,
            static_direct_compose: live.static_direct_compose,
            animated_direct_compose: live.animated_direct_compose,
            compose_planning_cpu_micros: live.compose_planning_cpu_micros,
        })
    }
}
