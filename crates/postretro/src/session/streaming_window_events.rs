//! SH and lightmap streaming windows as telemetry events, built from the
//! same baseline and live snapshot each window's log line formats. A session
//! that ends having counted something since its last line emits one final
//! `partial` window, which has no line.
//! See: context/lib/rendering_pipeline.md §Cluster SH residency, §12 · context/lib/development_guide.md §6.5

use std::sync::atomic::{AtomicU64, Ordering};

use postretro_renderer::{LightmapStreamingLiveDiagnostics, ShStreamingLiveDiagnostics};
use postretro_telemetry::Kind;
use serde::Serialize;

static NEXT_STREAMING_SESSION: AtomicU64 = AtomicU64::new(1);

/// A process-unique id for one streaming session's windows: totals restart
/// with it, so differencing is valid only within one id.
pub(crate) fn next_streaming_session() -> u64 {
    NEXT_STREAMING_SESSION.fetch_add(1, Ordering::Relaxed)
}

/// Declares a counter struct whose fields copy from a live snapshot, and
/// its field-wise window delta.
macro_rules! counters {
    ($name:ident, $live:ty, { $($field:ident: $pick:expr),* $(,)? }) => {
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
        pub(crate) struct $name {
            $(pub(crate) $field: u64,)*
        }

        impl $name {
            pub(crate) fn from_live(live: &$live) -> Self {
                Self { $($field: ($pick)(live),)* }
            }

            pub(crate) fn minus(&self, before: &Self) -> Self {
                Self { $($field: self.$field.saturating_sub(before.$field),)* }
            }
        }
    };
}

counters!(ShCounters, ShStreamingLiveDiagnostics, {
    reads: |d: &ShStreamingLiveDiagnostics| d.reads_issued,
    coalesced_reads: |d: &ShStreamingLiveDiagnostics| d.coalesced_reads,
    read_bytes: |d: &ShStreamingLiveDiagnostics| d.read_bytes,
    gap_bytes: |d: &ShStreamingLiveDiagnostics| d.gap_bytes,
    installs: |d: &ShStreamingLiveDiagnostics| d.installs,
    installed_bytes: |d: &ShStreamingLiveDiagnostics| d.decoded_bytes_installed,
    misses: |d: &ShStreamingLiveDiagnostics| d.misses,
    miss_reach: |d: &ShStreamingLiveDiagnostics| d.miss_buckets.outside_reach,
    miss_pressure: |d: &ShStreamingLiveDiagnostics| d.miss_buckets.trimmed_by_pressure,
    miss_in_flight: |d: &ShStreamingLiveDiagnostics| d.miss_buckets.read_in_flight,
    miss_budget: |d: &ShStreamingLiveDiagnostics| d.miss_buckets.held_by_drain_budget,
    miss_compose: |d: &ShStreamingLiveDiagnostics| d.miss_buckets.awaiting_compose,
    miss_failed: |d: &ShStreamingLiveDiagnostics| d.miss_buckets.failed,
    miss_device_limit: |d: &ShStreamingLiveDiagnostics| d.miss_buckets.device_limited,
    miss_map_cap: |d: &ShStreamingLiveDiagnostics| d.miss_buckets.at_whole_map_cap,
    rereads: |d: &ShStreamingLiveDiagnostics| d.rereads,
    evictions: |d: &ShStreamingLiveDiagnostics| d.evictions,
    retries: |d: &ShStreamingLiveDiagnostics| d.retries,
    cancelled: |d: &ShStreamingLiveDiagnostics| d.cancelled_requests,
    discarded: |d: &ShStreamingLiveDiagnostics| d.discarded_reads,
    discarded_bytes: |d: &ShStreamingLiveDiagnostics| d.discarded_read_bytes,
    budget_limited_drains: |d: &ShStreamingLiveDiagnostics| d.budget_limited_drains,
    pool_growths: |d: &ShStreamingLiveDiagnostics| d.pool_growth_events,
    pool_growth_bytes: |d: &ShStreamingLiveDiagnostics| d.pool_growth_bytes,
    install_cpu_micros: |d: &ShStreamingLiveDiagnostics| d.install_cpu_total_micros,
    pool_growth_cpu_micros: |d: &ShStreamingLiveDiagnostics| d.pool_growth_cpu_micros,
});

/// One compose pass's frame gauges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct ComposeGauges {
    pub(crate) rows: u64,
    pub(crate) dispatches: u64,
    pub(crate) lagged: u64,
    pub(crate) remaining: u64,
}

impl From<&postretro_renderer::ShComposePassDiagnostics> for ComposeGauges {
    fn from(pass: &postretro_renderer::ShComposePassDiagnostics) -> Self {
        Self {
            rows: pass.rows_composed,
            dispatches: pass.dispatches,
            lagged: pass.lagged_rows_composed,
            remaining: pass.resident_rows_still_lagging,
        }
    }
}

/// The line's `now:` and compose-frame gauges at the window's close.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct ShGauges {
    pub(crate) targets: u64,
    pub(crate) lead: u64,
    pub(crate) sampleable: u64,
    pub(crate) queued: u64,
    pub(crate) ready: u64,
    pub(crate) permits: u64,
    pub(crate) pool_occupancy_bytes: u64,
    pub(crate) pool_capacity_bytes: u64,
    /// The session's read and decode latencies; `None` before its first
    /// read, never a zero.
    pub(crate) read_latency_p50_ms: Option<f32>,
    pub(crate) read_latency_p95_ms: Option<f32>,
    pub(crate) read_latency_max_ms: Option<f32>,
    pub(crate) decode_latency_max_ms: Option<f32>,
    pub(crate) largest_drain_bytes: u64,
    pub(crate) slowest_install_ms: f64,
    pub(crate) slowest_steady_install_ms: f64,
    pub(crate) indirect_compose: ComposeGauges,
    pub(crate) static_direct_compose: ComposeGauges,
    pub(crate) animated_direct_compose: ComposeGauges,
    pub(crate) compose_planning_ms: f64,
}

impl ShGauges {
    fn from_live(now: &ShStreamingLiveDiagnostics) -> Self {
        // The worker's histograms read 0 with no sample; a session that has
        // read nothing has no latency.
        let sampled = |value: f32| (now.reads_issued > 0).then_some(value);
        Self {
            targets: now.target_clusters,
            lead: now.lead_clusters,
            sampleable: now.sampleable_clusters,
            queued: now.queued_clusters,
            ready: now.ready_clusters,
            permits: now.permits_in_use,
            pool_occupancy_bytes: now.logical_occupancy_bytes,
            pool_capacity_bytes: now.active_capacity_bytes,
            read_latency_p50_ms: sampled(now.read_latency_p50_ms),
            read_latency_p95_ms: sampled(now.read_latency_p95_ms),
            read_latency_max_ms: sampled(now.read_latency_max_ms),
            decode_latency_max_ms: sampled(now.decode_latency_max_ms),
            largest_drain_bytes: now.max_drain_decoded_bytes,
            slowest_install_ms: now.install_cpu_max_drain_micros as f64 / 1000.0,
            slowest_steady_install_ms: now.install_cpu_max_steady_drain_micros as f64 / 1000.0,
            indirect_compose: (&now.indirect_compose).into(),
            static_direct_compose: (&now.static_direct_compose).into(),
            animated_direct_compose: (&now.animated_direct_compose).into(),
            compose_planning_ms: now.compose_planning_cpu_micros as f64 / 1000.0,
        }
    }
}

/// `sh_streaming_window`: one closed window, or a session's final partial.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ShWindowEvent {
    pub(crate) session: u64,
    pub(crate) partial: bool,
    pub(crate) elapsed_s: f64,
    pub(crate) delta: ShCounters,
    /// Since the session started.
    pub(crate) total: ShCounters,
    pub(crate) now: ShGauges,
}

impl ShWindowEvent {
    pub(crate) fn new(
        session: u64,
        partial: bool,
        elapsed_s: f64,
        before: &ShStreamingLiveDiagnostics,
        now: &ShStreamingLiveDiagnostics,
    ) -> Self {
        let total = ShCounters::from_live(now);
        Self {
            session,
            partial,
            elapsed_s,
            delta: total.minus(&ShCounters::from_live(before)),
            total,
            now: ShGauges::from_live(now),
        }
    }
}

pub(crate) fn emit_sh_window(
    session: u64,
    partial: bool,
    elapsed_s: f64,
    before: &ShStreamingLiveDiagnostics,
    now: &ShStreamingLiveDiagnostics,
) {
    postretro_telemetry::emit_with(Kind::ShStreamingWindow, || {
        ShWindowEvent::new(session, partial, elapsed_s, before, now)
    });
}

counters!(LightmapCounters, LightmapStreamingLiveDiagnostics, {
    pairs_requested: |d: &LightmapStreamingLiveDiagnostics| d.reads_requested,
    reads: |d: &LightmapStreamingLiveDiagnostics| d.physical_reads,
    lightmap_bytes_read: |d: &LightmapStreamingLiveDiagnostics| d.lightmap_bytes_read,
    shadowmask_bytes_read: |d: &LightmapStreamingLiveDiagnostics| d.shadowmask_bytes_read,
    installs: |d: &LightmapStreamingLiveDiagnostics| d.installs,
    evictions: |d: &LightmapStreamingLiveDiagnostics| d.evictions,
    refusals: |d: &LightmapStreamingLiveDiagnostics| d.refusals,
    deferrals: |d: &LightmapStreamingLiveDiagnostics| d.deferrals,
    failed_installs: |d: &LightmapStreamingLiveDiagnostics| d.failed_installs,
    failed_reads: |d: &LightmapStreamingLiveDiagnostics| d.failed_reads,
    cancelled_reads: |d: &LightmapStreamingLiveDiagnostics| d.cancelled_reads,
    repacks: |d: &LightmapStreamingLiveDiagnostics| d.repacks.lightmap,
    mask_repacks: |d: &LightmapStreamingLiveDiagnostics| d.repacks.mask,
    compactions: |d: &LightmapStreamingLiveDiagnostics| d.compactions.lightmap,
    mask_compactions: |d: &LightmapStreamingLiveDiagnostics| d.compactions.mask,
    compaction_rollbacks: |d: &LightmapStreamingLiveDiagnostics| d.compaction_rollbacks.lightmap,
    mask_compaction_rollbacks: |d: &LightmapStreamingLiveDiagnostics| d.compaction_rollbacks.mask,
    growths: |d: &LightmapStreamingLiveDiagnostics| d.growths.lightmap,
    mask_growths: |d: &LightmapStreamingLiveDiagnostics| d.growths.mask,
    missed_outside_baked_set: |d: &LightmapStreamingLiveDiagnostics| d.drawn_outside_baked_set,
    missed_not_resident: |d: &LightmapStreamingLiveDiagnostics| d.drawn_not_resident,
});

/// The lightmap line's `now:` gauges at the window's close.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct LightmapGauges {
    pub(crate) lead_metres: f32,
    pub(crate) max_lead_metres: f32,
    pub(crate) resident_blocks: u64,
    pub(crate) blocks: u64,
    pub(crate) resident_lightmap_bytes: u64,
    pub(crate) resident_shadowmask_bytes: u64,
    pub(crate) mandatory_blocks: u64,
    pub(crate) mandatory_lightmap_bytes: u64,
    pub(crate) mandatory_shadowmask_bytes: u64,
    pub(crate) pool_layers: u32,
    pub(crate) peak_pool_layers: u32,
    pub(crate) pool_bytes: u64,
    pub(crate) retiring_pool_bytes: u64,
    pub(crate) headroom_texels: u64,
    pub(crate) mask_pool_layers: u32,
    pub(crate) mask_peak_pool_layers: u32,
    pub(crate) mask_pool_bytes: u64,
    pub(crate) mask_retiring_pool_bytes: u64,
    pub(crate) mask_headroom_texels: u64,
    pub(crate) growth_peak_bytes: u64,
    pub(crate) install_last_ms: f64,
    pub(crate) install_max_ms: f64,
}

impl LightmapGauges {
    fn from_live(now: &LightmapStreamingLiveDiagnostics) -> Self {
        Self {
            lead_metres: now.lead_metres,
            max_lead_metres: now.max_lead_metres,
            resident_blocks: now.resident_blocks,
            blocks: now.block_count,
            resident_lightmap_bytes: now.resident_lightmap_bytes,
            resident_shadowmask_bytes: now.resident_shadowmask_bytes,
            mandatory_blocks: now.mandatory_blocks,
            mandatory_lightmap_bytes: now.mandatory_lightmap_bytes,
            mandatory_shadowmask_bytes: now.mandatory_shadowmask_bytes,
            pool_layers: now.pool_layers.lightmap,
            peak_pool_layers: now.peak_pool_layers.lightmap,
            pool_bytes: now.pool_bytes.lightmap,
            retiring_pool_bytes: now.retiring_pool_bytes.lightmap,
            headroom_texels: now.band_headroom_texels.lightmap,
            mask_pool_layers: now.pool_layers.mask,
            mask_peak_pool_layers: now.peak_pool_layers.mask,
            mask_pool_bytes: now.pool_bytes.mask,
            mask_retiring_pool_bytes: now.retiring_pool_bytes.mask,
            mask_headroom_texels: now.band_headroom_texels.mask,
            growth_peak_bytes: now.growth_transient_peak_bytes,
            install_last_ms: now.last_drain_install_micros as f64 / 1000.0,
            install_max_ms: now.max_drain_install_micros as f64 / 1000.0,
        }
    }
}

/// `lightmap_streaming_window`: one closed window, or a session's partial.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct LightmapWindowEvent {
    pub(crate) session: u64,
    pub(crate) partial: bool,
    pub(crate) elapsed_s: f64,
    pub(crate) delta: LightmapCounters,
    pub(crate) total: LightmapCounters,
    pub(crate) now: LightmapGauges,
}

impl LightmapWindowEvent {
    pub(crate) fn new(
        session: u64,
        partial: bool,
        elapsed_s: f64,
        before: &LightmapStreamingLiveDiagnostics,
        now: &LightmapStreamingLiveDiagnostics,
    ) -> Self {
        let total = LightmapCounters::from_live(now);
        Self {
            session,
            partial,
            elapsed_s,
            delta: total.minus(&LightmapCounters::from_live(before)),
            total,
            now: LightmapGauges::from_live(now),
        }
    }
}

pub(crate) fn emit_lightmap_window(
    session: u64,
    partial: bool,
    elapsed_s: f64,
    before: &LightmapStreamingLiveDiagnostics,
    now: &LightmapStreamingLiveDiagnostics,
) {
    postretro_telemetry::emit_with(Kind::LightmapStreamingWindow, || {
        LightmapWindowEvent::new(session, partial, elapsed_s, before, now)
    });
}
