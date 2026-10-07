// Capture measurement report schema and CPU-only summary math.
// See: context/lib/rendering_pipeline.md §7.8

use postretro_renderer::LightmapStreamingLiveDiagnostics;
#[cfg(test)]
use postretro_renderer::ShStreamingAllocationSummary;
use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use super::lightmap::CaptureLightmapResidency;
use super::prepared::measurement_animation_time_seconds;
use super::scene::{CameraPose, CaptureScene};
use crate::render::{
    CaptureAdapterIdentity, CaptureGpuTimingState, CaptureGpuTimingWindow, LightmapResidencyReport,
    ResidencyAllocation, ResidencyAllocationShape, ResidencyAllocationState, ResidencySource,
    ShResidencyReport, ShStreamingLifecycleSummary,
};

const MEASUREMENT_SCHEMA: &str = "postretro.capture.measurement.v3";
const CPU_COMPLETION_STRATEGY: &str = "device-poll-wait-after-submit";
const CPU_COMPLETION_CADENCE: &str = "once-per-sample-frame";

/// Build the schema-v3 report only after all sample frames have completed.
/// The caller owns staging and publication so a previous successful report is
/// never replaced until the final PNG is visible.
pub(super) fn measurement_report(
    scene: &CaptureScene,
    map_bytes: u64,
    revision: Option<String>,
    adapter: CaptureAdapterIdentity,
    sh_residency: Option<ShResidencyReport>,
    lightmap_residency: Option<LightmapResidencyReport>,
    cpu_samples_ms: Vec<f64>,
    timing_state: CaptureGpuTimingState,
    gpu_partial_frames: u32,
    gpu_windows: Vec<CaptureGpuTimingWindow>,
    cpu_stages: crate::cpu_timing::CpuStagesReport,
) -> MeasurementReport {
    let measurement = scene
        .measurement
        .as_ref()
        .expect("measurement report requires validated measurement scene");
    let (gpu_timing, partial_frames) =
        gpu_timing_report(timing_state, gpu_partial_frames, gpu_windows);
    let median_ms = percentile(&cpu_samples_ms, 0.5)
        .expect("validated measurement always has at least one CPU sample");
    let p95_ms = percentile(&cpu_samples_ms, 0.95)
        .expect("validated measurement always has at least one CPU sample");

    MeasurementReport {
        schema: MEASUREMENT_SCHEMA.into(),
        revision,
        map: MapReport {
            path: scene.map.clone(),
            bytes: map_bytes,
        },
        capture: CaptureReport {
            output: scene.output.clone(),
            resolution: scene.resolution,
            camera: CameraReport::from(&scene.camera),
            force_full_resident_sh_compose: scene.force_full_resident_sh_compose,
            animation_time_seconds: measurement_animation_time_seconds(
                u64::from(measurement.warmup_frames) + u64::from(measurement.sample_frames),
            ),
        },
        workload: WorkloadReport {
            warmup_frames: measurement.warmup_frames,
            sample_frames: measurement.sample_frames,
        },
        adapter: AdapterReport::from(adapter),
        renderer_accounted_sh: sh_residency.map(ShResidencyReportJson::from),
        renderer_accounted_lightmap: lightmap_residency.map(LightmapResidencyReportJson::from),
        lightmap_streaming: None,
        cpu_completion: CpuCompletionReport {
            unit: "milliseconds".into(),
            strategy: CPU_COMPLETION_STRATEGY.into(),
            cadence: CPU_COMPLETION_CADENCE.into(),
            samples_ms: cpu_samples_ms,
            median_ms,
            p95_ms,
        },
        gpu_timing: GpuTimingReport {
            availability: gpu_timing.availability,
            reason: gpu_timing.reason,
            windows: gpu_timing.windows,
            partial_frames,
        },
        cpu_stages,
    }
}

impl MeasurementReport {
    pub(super) fn with_lightmap_streaming(mut self, residency: CaptureLightmapResidency) -> Self {
        self.lightmap_streaming = Some(LightmapStreamingReportJson::from(residency));
        self
    }
}

/// Interpolated percentile after ascending sort. This keeps even-sized median
/// values and p95 values stable and records raw samples for alternate analysis.
pub(super) fn percentile(samples: &[f64], fraction: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    debug_assert!((0.0..=1.0).contains(&fraction));
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = fraction * (sorted.len() - 1) as f64;
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    let weight = rank - lower as f64;
    Some(sorted[lower] + (sorted[upper] - sorted[lower]) * weight)
}

fn gpu_timing_report(
    state: CaptureGpuTimingState,
    gpu_partial_frames: u32,
    windows: Vec<CaptureGpuTimingWindow>,
) -> (GpuTimingReportInner, Option<u32>) {
    let timing = match state {
        CaptureGpuTimingState::NotRequested => GpuTimingReportInner {
            availability: GpuTimingAvailability::NotRequested,
            reason: Some(GpuTimingReason::EnvDisabled),
            windows: None,
        },
        CaptureGpuTimingState::Unsupported => GpuTimingReportInner {
            availability: GpuTimingAvailability::Unsupported,
            reason: Some(GpuTimingReason::AdapterMissingTimestampFeatures),
            windows: None,
        },
        CaptureGpuTimingState::PlainBuildUnavailable => GpuTimingReportInner {
            availability: GpuTimingAvailability::PlainBuildUnavailable,
            reason: Some(GpuTimingReason::DevToolsAccessorUnavailable),
            windows: None,
        },
        CaptureGpuTimingState::Active if windows.is_empty() => GpuTimingReportInner {
            availability: GpuTimingAvailability::NotYetWindowed,
            reason: Some(GpuTimingReason::WindowNotComplete),
            windows: None,
        },
        CaptureGpuTimingState::Active => GpuTimingReportInner {
            availability: GpuTimingAvailability::Available,
            reason: None,
            windows: Some(
                windows
                    .into_iter()
                    .map(GpuTimingWindowReport::from)
                    .collect(),
            ),
        },
    };
    let partial_frames = if state == CaptureGpuTimingState::Active {
        (gpu_partial_frames != 0).then_some(gpu_partial_frames)
    } else {
        None
    };
    (timing, partial_frames)
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct MeasurementReport {
    schema: Cow<'static, str>,
    revision: Option<String>,
    map: MapReport,
    capture: CaptureReport,
    workload: WorkloadReport,
    adapter: AdapterReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    renderer_accounted_sh: Option<ShResidencyReportJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    renderer_accounted_lightmap: Option<LightmapResidencyReportJson>,
    /// How the capture owned its lightmap blocks and how many were resident.
    #[serde(skip_serializing_if = "Option::is_none")]
    lightmap_streaming: Option<LightmapStreamingReportJson>,
    cpu_completion: CpuCompletionReport,
    gpu_timing: GpuTimingReport,
    /// Renderer recording stages over complete post-warmup windows. Capture
    /// runs no tick or walk per sample, so only recording stages appear.
    cpu_stages: crate::cpu_timing::CpuStagesReport,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct MapReport {
    path: String,
    bytes: u64,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct CaptureReport {
    output: String,
    resolution: [u32; 2],
    camera: CameraReport,
    force_full_resident_sh_compose: bool,
    animation_time_seconds: f32,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct CameraReport {
    position: [f32; 3],
    yaw_deg: f32,
    pitch_deg: f32,
    fov_deg: f32,
}

impl From<&CameraPose> for CameraReport {
    fn from(camera: &CameraPose) -> Self {
        Self {
            position: camera.position,
            yaw_deg: camera.yaw_deg,
            pitch_deg: camera.pitch_deg,
            fov_deg: camera.fov_deg,
        }
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct WorkloadReport {
    warmup_frames: u32,
    sample_frames: u32,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct AdapterReport {
    name: String,
    backend: String,
    device_type: String,
}

impl From<CaptureAdapterIdentity> for AdapterReport {
    fn from(adapter: CaptureAdapterIdentity) -> Self {
        Self {
            name: adapter.name,
            backend: adapter.backend,
            device_type: adapter.device_type,
        }
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct CpuCompletionReport {
    unit: Cow<'static, str>,
    strategy: Cow<'static, str>,
    cadence: Cow<'static, str>,
    samples_ms: Vec<f64>,
    median_ms: f64,
    p95_ms: f64,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct GpuTimingReport {
    availability: GpuTimingAvailability,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<GpuTimingReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    windows: Option<Vec<GpuTimingWindowReport>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    partial_frames: Option<u32>,
}

#[derive(Debug)]
struct GpuTimingReportInner {
    availability: GpuTimingAvailability,
    reason: Option<GpuTimingReason>,
    windows: Option<Vec<GpuTimingWindowReport>>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum GpuTimingAvailability {
    Available,
    NotRequested,
    Unsupported,
    PlainBuildUnavailable,
    NotYetWindowed,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum GpuTimingReason {
    EnvDisabled,
    AdapterMissingTimestampFeatures,
    DevToolsAccessorUnavailable,
    WindowNotComplete,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct GpuTimingWindowReport {
    /// Readbacks in the window; bounds every pass's `sampled_readbacks`.
    readbacks: u32,
    passes: Vec<GpuTimingPassReport>,
}

impl From<CaptureGpuTimingWindow> for GpuTimingWindowReport {
    fn from(window: CaptureGpuTimingWindow) -> Self {
        Self {
            readbacks: window.readbacks,
            passes: window
                .passes
                .into_iter()
                .map(|pass| GpuTimingPassReport {
                    label: pass.label.into(),
                    average_ms: pass.average_ms,
                    sampled_readbacks: pass.sampled_readbacks,
                    malformed_readbacks: pass.malformed_readbacks,
                })
                .collect(),
        }
    }
}

/// `average_ms` is the mean over `sampled_readbacks`, not over the window, so
/// summing passes over-counts conditional ones. It serializes as `null` when
/// the pass was not sampled, never as a zero-cost pass.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct GpuTimingPassReport {
    label: Cow<'static, str>,
    average_ms: Option<f32>,
    sampled_readbacks: u32,
    malformed_readbacks: u32,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct ShResidencyReportJson {
    rows: Vec<ResidencyAllocationJson>,
    total_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    streaming: Option<ShStreamingAllocationSummaryJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    streaming_lifecycle: Option<ShStreamingLifecycleSummaryJson>,
}

impl From<ShResidencyReport> for ShResidencyReportJson {
    fn from(report: ShResidencyReport) -> Self {
        let ShResidencyReport {
            allocations,
            total_bytes,
            streaming,
            streaming_lifecycle,
        } = report;
        Self {
            rows: allocations
                .into_iter()
                .map(ResidencyAllocationJson::from)
                .collect(),
            total_bytes,
            streaming: streaming.map(|summary| ShStreamingAllocationSummaryJson {
                fixed_metadata_bytes: summary.fixed_metadata_bytes,
                whole_resident_scatter_bytes: summary.whole_resident_scatter_bytes,
                active_capacity_bytes: summary.active_capacity_bytes,
                logical_occupancy_bytes: summary.logical_occupancy_bytes,
                retiring_capacity_bytes: summary.retiring_capacity_bytes,
                replacement_peak_bytes: summary.replacement_peak_bytes,
            }),
            streaming_lifecycle: streaming_lifecycle.map(ShStreamingLifecycleSummaryJson::from),
        }
    }
}

/// Lightmap-family rows, the same bytes the load log and dev panel print.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct LightmapResidencyReportJson {
    rows: Vec<ResidencyAllocationJson>,
    total_bytes: u64,
}

impl From<LightmapResidencyReport> for LightmapResidencyReportJson {
    fn from(report: LightmapResidencyReport) -> Self {
        Self {
            rows: report
                .allocations
                .into_iter()
                .map(ResidencyAllocationJson::from)
                .collect(),
            total_bytes: report.total_bytes,
        }
    }
}

/// Lightmap block residency at the captured instant: the mode, blocks
/// resident of the level's total, the streamed pool's layers and cap, and
/// (streaming only) the residency counters.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct LightmapStreamingReportJson {
    mode: Cow<'static, str>,
    block_count: u32,
    resident_blocks: u32,
    forced_missing_blocks: u32,
    pool_layers: Option<u32>,
    pool_cap_layers: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    counters: Option<LightmapStreamingCountersJson>,
}

impl From<CaptureLightmapResidency> for LightmapStreamingReportJson {
    fn from(residency: CaptureLightmapResidency) -> Self {
        Self {
            mode: residency.mode.label().into(),
            block_count: residency.block_count,
            resident_blocks: residency.resident_blocks,
            forced_missing_blocks: residency.forced_missing_blocks,
            pool_layers: residency.pool_layers,
            pool_cap_layers: residency.pool_cap_layers,
            counters: residency
                .counters
                .as_ref()
                .map(LightmapStreamingCountersJson::from),
        }
    }
}

/// Streamed lightmap counters after the capture's preload drain. Bytes are
/// per section: `lightmap` is id 22, `shadowmask` is id 42. Mandatory is the
/// view's camera cell's baked set within `lead_metres`, plus the pins.
/// Visible misses are the view's drawn blocks in two buckets, which may
/// overlap: outside the baked set, and not resident.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct LightmapStreamingCountersJson {
    lead_metres: f32,
    resident_lightmap_bytes: u64,
    resident_shadowmask_bytes: u64,
    mandatory_blocks: u64,
    mandatory_lightmap_bytes: u64,
    mandatory_shadowmask_bytes: u64,
    pool_bytes: u64,
    peak_pool_layers: u32,
    repacks: u64,
    growths: u64,
    growth_transient_peak_bytes: u64,
    last_drain_install_micros: u64,
    max_drain_install_micros: u64,
    lightmap_bytes_read: u64,
    shadowmask_bytes_read: u64,
    visible_misses_outside_baked_set: u64,
    visible_misses_not_resident: u64,
    failed_installs: u64,
    failed_reads: u64,
    refusals: u64,
    deferrals: u64,
}

impl From<&LightmapStreamingLiveDiagnostics> for LightmapStreamingCountersJson {
    fn from(live: &LightmapStreamingLiveDiagnostics) -> Self {
        Self {
            lead_metres: live.lead_metres,
            resident_lightmap_bytes: live.resident_lightmap_bytes,
            resident_shadowmask_bytes: live.resident_shadowmask_bytes,
            mandatory_blocks: live.mandatory_blocks,
            mandatory_lightmap_bytes: live.mandatory_lightmap_bytes,
            mandatory_shadowmask_bytes: live.mandatory_shadowmask_bytes,
            pool_bytes: live.pool_bytes,
            peak_pool_layers: live.peak_pool_layers,
            repacks: live.repacks,
            growths: live.growths,
            growth_transient_peak_bytes: live.growth_transient_peak_bytes,
            last_drain_install_micros: live.last_drain_install_micros,
            max_drain_install_micros: live.max_drain_install_micros,
            lightmap_bytes_read: live.lightmap_bytes_read,
            shadowmask_bytes_read: live.shadowmask_bytes_read,
            visible_misses_outside_baked_set: live.drawn_outside_baked_set,
            visible_misses_not_resident: live.drawn_not_resident,
            failed_installs: live.failed_installs,
            failed_reads: live.failed_reads,
            refusals: live.refusals,
            deferrals: live.deferrals,
        }
    }
}

/// Streaming values remain outside descriptor rows because logical occupancy
/// and replacement peak are accounting views, not new wgpu allocations.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct ShStreamingAllocationSummaryJson {
    fixed_metadata_bytes: u64,
    whole_resident_scatter_bytes: u64,
    active_capacity_bytes: u64,
    logical_occupancy_bytes: u64,
    retiring_capacity_bytes: u64,
    replacement_peak_bytes: u64,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct ShStreamingLifecycleSummaryJson {
    non_evictable_overshoot_bytes: u64,
    encoded_current_bytes: u64,
    encoded_high_water_upper_bound_bytes: u64,
    decoding_current_bytes: u64,
    decoding_high_water_upper_bound_bytes: u64,
    ready_current_bytes: u64,
    ready_high_water_upper_bound_bytes: u64,
    permits_in_use: u64,
    target_clusters: u64,
    absent_clusters: u64,
    queued_clusters: u64,
    ready_clusters: u64,
    installed_uncomposed_clusters: u64,
    sampleable_clusters: u64,
    failed_clusters: u64,
    misses: u64,
    installs: u64,
    evictions: u64,
    retries: u64,
    warm_clusters: u64,
    cancelled_requests: u64,
    discarded_reads: u64,
    discarded_read_bytes: u64,
    decoded_bytes_installed: u64,
    last_drain_decoded_bytes: u64,
    max_drain_decoded_bytes: u64,
    budget_limited_drains: u64,
    reads_issued: u64,
    coalesced_reads: u64,
    read_bytes: u64,
    gap_bytes: u64,
    read_latency_p50_ms: f32,
    read_latency_p95_ms: f32,
    read_latency_max_ms: f32,
    decode_latency_max_ms: f32,
    install_cpu_total_micros: u64,
    install_cpu_max_drain_micros: u64,
    install_cpu_last_drain_micros: u64,
    install_cpu_max_steady_drain_micros: u64,
    pool_growth_events: u64,
    pool_growth_bytes: u64,
    pool_growth_cpu_micros: u64,
    indirect_compose: ShComposePassDiagnosticsJson,
    static_direct_compose: ShComposePassDiagnosticsJson,
    animated_direct_compose: ShComposePassDiagnosticsJson,
    compose_planning_cpu_micros: u64,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct ShComposePassDiagnosticsJson {
    rows_composed: u64,
    entry_rows_composed: u64,
    dispatches: u64,
    lagged_rows_composed: u64,
    resident_rows_still_lagging: u64,
}

impl From<postretro_renderer::ShComposePassDiagnostics> for ShComposePassDiagnosticsJson {
    fn from(diagnostics: postretro_renderer::ShComposePassDiagnostics) -> Self {
        Self {
            rows_composed: diagnostics.rows_composed,
            entry_rows_composed: diagnostics.entry_rows_composed,
            dispatches: diagnostics.dispatches,
            lagged_rows_composed: diagnostics.lagged_rows_composed,
            resident_rows_still_lagging: diagnostics.resident_rows_still_lagging,
        }
    }
}

impl From<ShStreamingLifecycleSummary> for ShStreamingLifecycleSummaryJson {
    fn from(summary: ShStreamingLifecycleSummary) -> Self {
        Self {
            non_evictable_overshoot_bytes: summary.non_evictable_overshoot_bytes,
            encoded_current_bytes: summary.encoded_current_bytes,
            encoded_high_water_upper_bound_bytes: summary.encoded_high_water_upper_bound_bytes,
            decoding_current_bytes: summary.decoding_current_bytes,
            decoding_high_water_upper_bound_bytes: summary.decoding_high_water_upper_bound_bytes,
            ready_current_bytes: summary.ready_current_bytes,
            ready_high_water_upper_bound_bytes: summary.ready_high_water_upper_bound_bytes,
            permits_in_use: summary.permits_in_use,
            target_clusters: summary.target_clusters,
            absent_clusters: summary.absent_clusters,
            queued_clusters: summary.queued_clusters,
            ready_clusters: summary.ready_clusters,
            installed_uncomposed_clusters: summary.installed_uncomposed_clusters,
            sampleable_clusters: summary.sampleable_clusters,
            failed_clusters: summary.failed_clusters,
            misses: summary.misses,
            installs: summary.installs,
            evictions: summary.evictions,
            retries: summary.retries,
            warm_clusters: summary.warm_clusters,
            cancelled_requests: summary.cancelled_requests,
            discarded_reads: summary.discarded_reads,
            discarded_read_bytes: summary.discarded_read_bytes,
            decoded_bytes_installed: summary.decoded_bytes_installed,
            last_drain_decoded_bytes: summary.last_drain_decoded_bytes,
            max_drain_decoded_bytes: summary.max_drain_decoded_bytes,
            budget_limited_drains: summary.budget_limited_drains,
            reads_issued: summary.reads_issued,
            coalesced_reads: summary.coalesced_reads,
            read_bytes: summary.read_bytes,
            gap_bytes: summary.gap_bytes,
            read_latency_p50_ms: summary.read_latency_p50_ms,
            read_latency_p95_ms: summary.read_latency_p95_ms,
            read_latency_max_ms: summary.read_latency_max_ms,
            decode_latency_max_ms: summary.decode_latency_max_ms,
            install_cpu_total_micros: summary.install_cpu_total_micros,
            install_cpu_max_drain_micros: summary.install_cpu_max_drain_micros,
            install_cpu_last_drain_micros: summary.install_cpu_last_drain_micros,
            install_cpu_max_steady_drain_micros: summary.install_cpu_max_steady_drain_micros,
            pool_growth_events: summary.pool_growth_events,
            pool_growth_bytes: summary.pool_growth_bytes,
            pool_growth_cpu_micros: summary.pool_growth_cpu_micros,
            indirect_compose: summary.indirect_compose.into(),
            static_direct_compose: summary.static_direct_compose.into(),
            animated_direct_compose: summary.animated_direct_compose.into(),
            compose_planning_cpu_micros: summary.compose_planning_cpu_micros,
        }
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct ResidencyAllocationJson {
    name: Cow<'static, str>,
    sources: Vec<ResidencySourceJson>,
    bytes: u64,
    state: ResidencyStateJson,
    shape: ResidencyShapeJson,
}

impl From<ResidencyAllocation> for ResidencyAllocationJson {
    fn from(row: ResidencyAllocation) -> Self {
        Self {
            name: row.name.into(),
            sources: row
                .sources
                .into_iter()
                .map(ResidencySourceJson::from)
                .collect(),
            bytes: row.bytes,
            state: ResidencyStateJson::from(row.state),
            shape: ResidencyShapeJson::from(row.shape),
        }
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ResidencySourceJson {
    Section { id: u16 },
    Derived,
}

impl From<ResidencySource> for ResidencySourceJson {
    fn from(source: ResidencySource) -> Self {
        match source {
            ResidencySource::Section(id) => Self::Section { id },
            ResidencySource::Derived => Self::Derived,
        }
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ResidencyStateJson {
    Data,
    Dummy,
    Fallback,
}

impl From<ResidencyAllocationState> for ResidencyStateJson {
    fn from(state: ResidencyAllocationState) -> Self {
        match state {
            ResidencyAllocationState::Data => Self::Data,
            ResidencyAllocationState::Dummy => Self::Dummy,
            ResidencyAllocationState::Fallback => Self::Fallback,
        }
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ResidencyShapeJson {
    Texture {
        format: Cow<'static, str>,
        dimension: Cow<'static, str>,
        extent: [u32; 3],
    },
    Buffer {
        binding_bytes: u64,
    },
}

impl From<ResidencyAllocationShape> for ResidencyShapeJson {
    fn from(shape: ResidencyAllocationShape) -> Self {
        match shape {
            ResidencyAllocationShape::Texture {
                format,
                dimension,
                extent,
            } => Self::Texture {
                format: format.into(),
                dimension: dimension.into(),
                extent,
            },
            ResidencyAllocationShape::Buffer { binding_bytes } => Self::Buffer { binding_bytes },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::scene::{CameraPose, CaptureMeasurement, DEFAULT_FOV_DEG};
    use super::*;
    use serde_json::Value;

    fn scene_with_measurement() -> CaptureScene {
        CaptureScene {
            map: "content/dev/maps/test.prl".into(),
            camera: CameraPose {
                position: [1.0, 2.0, 3.0],
                yaw_deg: 45.0,
                pitch_deg: -10.0,
                fov_deg: DEFAULT_FOV_DEG,
            },
            resolution: [1280, 720],
            output: "capture.png".into(),
            force_active: None,
            force_promotion: None,
            force_full_resident_sh_compose: false,
            light_term_mask: None,
            force_missing_lightmap_blocks: Vec::new(),
            lightmap_pool_cap_layers: None,
            measurement: Some(CaptureMeasurement {
                report: "capture.json".into(),
                warmup_frames: 2,
                sample_frames: 121,
            }),
        }
    }

    fn adapter() -> CaptureAdapterIdentity {
        CaptureAdapterIdentity {
            name: "Test Adapter".into(),
            backend: "Metal".into(),
            device_type: "IntegratedGpu".into(),
        }
    }

    fn as_json(report: MeasurementReport) -> Value {
        serde_json::to_value(report).expect("report must serialize")
    }

    #[test]
    fn percentile_interpolates_median_and_p95() {
        let samples = [4.0, 1.0, 3.0, 2.0];
        assert_eq!(percentile(&samples, 0.5), Some(2.5));
        assert!(
            (percentile(&samples, 0.95).expect("p95") - 3.85).abs() < 1.0e-12,
            "p95 uses an interpolated rank"
        );
        assert_eq!(percentile(&[], 0.5), None);
    }

    #[test]
    fn report_omits_optional_sh_accounting_when_no_level_report_exists() {
        let json = as_json(measurement_report(
            &scene_with_measurement(),
            99,
            None,
            adapter(),
            None,
            None,
            vec![1.0],
            CaptureGpuTimingState::NotRequested,
            0,
            Vec::new(),
            crate::cpu_timing::capture_stages_report(
                postretro_stage_timing::TimingGate::OFF,
                &[],
                0,
            ),
        ));

        assert_eq!(json["schema"], MEASUREMENT_SCHEMA);
        assert!(json.get("renderer_accounted_sh").is_none());
        assert!(json.get("renderer_accounted_lightmap").is_none());
        assert_eq!(json["gpu_timing"]["availability"], "not-requested");
        assert_eq!(json["gpu_timing"]["reason"], "env-disabled");
        assert!(json["gpu_timing"].get("windows").is_none());
    }

    #[test]
    fn report_serializes_lightmap_family_rows_with_the_meter_bytes() {
        let row = |name: &'static str, bytes: u64| ResidencyAllocation {
            name,
            sources: vec![ResidencySource::Section(25)],
            bytes,
            state: ResidencyAllocationState::Data,
            shape: ResidencyAllocationShape::Texture {
                format: "Rgba16Float",
                dimension: "D2",
                extent: [1024, 1024, 3],
            },
        };
        let meter = LightmapResidencyReport {
            allocations: vec![
                row("animated_irradiance", 25_165_824),
                row("animated_direction", 12_582_912),
            ],
            total_bytes: 37_748_736,
            retiring_bytes: 0,
        };
        let json = as_json(measurement_report(
            &scene_with_measurement(),
            99,
            None,
            adapter(),
            None,
            Some(meter),
            vec![1.0],
            CaptureGpuTimingState::NotRequested,
            0,
            Vec::new(),
            crate::cpu_timing::capture_stages_report(
                postretro_stage_timing::TimingGate::OFF,
                &[],
                0,
            ),
        ));

        let lightmap = &json["renderer_accounted_lightmap"];
        assert_eq!(lightmap["total_bytes"], 37_748_736);
        assert_eq!(lightmap["rows"][0]["name"], "animated_irradiance");
        assert_eq!(lightmap["rows"][0]["bytes"], 25_165_824);
        assert_eq!(lightmap["rows"][1]["name"], "animated_direction");
        assert_eq!(lightmap["rows"][1]["bytes"], 12_582_912);
        assert_eq!(lightmap["rows"][0]["shape"]["extent"][2], 3);
    }

    #[test]
    fn report_carries_lightmap_streaming_residency_when_attached() {
        let report = || {
            measurement_report(
                &scene_with_measurement(),
                99,
                None,
                adapter(),
                None,
                None,
                vec![1.0],
                CaptureGpuTimingState::NotRequested,
                0,
                Vec::new(),
                crate::cpu_timing::capture_stages_report(
                    postretro_stage_timing::TimingGate::OFF,
                    &[],
                    0,
                ),
            )
        };
        assert!(as_json(report()).get("lightmap_streaming").is_none());

        let json = as_json(report().with_lightmap_streaming(CaptureLightmapResidency {
            mode: super::super::lightmap::CaptureLightmapMode::Stream,
            block_count: 198,
            resident_blocks: 41,
            forced_missing_blocks: 1,
            pool_layers: Some(3),
            pool_cap_layers: Some(15),
            counters: None,
        }));
        let lightmap = &json["lightmap_streaming"];
        assert_eq!(lightmap["mode"], "stream");
        assert_eq!(lightmap["block_count"], 198);
        assert_eq!(lightmap["resident_blocks"], 41);
        assert_eq!(lightmap["forced_missing_blocks"], 1);
        assert_eq!(lightmap["pool_layers"], 3);
        assert_eq!(lightmap["pool_cap_layers"], 15);
        assert!(lightmap.get("counters").is_none(), "no counters, no object");
    }

    // A streamed capture records the residency counters under
    // `lightmap_streaming.counters`, field for field.
    #[test]
    fn lightmap_streaming_counters_serialize_every_measured_field() {
        let live = postretro_renderer::LightmapStreamingLiveDiagnostics {
            lead_metres: 16.0,
            resident_lightmap_bytes: 1,
            resident_shadowmask_bytes: 2,
            mandatory_blocks: 3,
            mandatory_lightmap_bytes: 4,
            mandatory_shadowmask_bytes: 5,
            pool_bytes: 6,
            peak_pool_layers: 7,
            repacks: 8,
            growths: 9,
            growth_transient_peak_bytes: 10,
            last_drain_install_micros: 11,
            max_drain_install_micros: 12,
            lightmap_bytes_read: 13,
            shadowmask_bytes_read: 14,
            drawn_outside_baked_set: 15,
            drawn_not_resident: 16,
            failed_installs: 17,
            failed_reads: 18,
            refusals: 19,
            deferrals: 20,
            ..Default::default()
        };
        let json = serde_json::to_value(LightmapStreamingReportJson::from(
            CaptureLightmapResidency {
                mode: super::super::lightmap::CaptureLightmapMode::Stream,
                block_count: 198,
                resident_blocks: 41,
                forced_missing_blocks: 0,
                pool_layers: Some(7),
                pool_cap_layers: Some(15),
                counters: Some(live),
            },
        ))
        .unwrap();
        let expected = serde_json::json!({
            "lead_metres": 16.0,
            "resident_lightmap_bytes": 1,
            "resident_shadowmask_bytes": 2,
            "mandatory_blocks": 3,
            "mandatory_lightmap_bytes": 4,
            "mandatory_shadowmask_bytes": 5,
            "pool_bytes": 6,
            "peak_pool_layers": 7,
            "repacks": 8,
            "growths": 9,
            "growth_transient_peak_bytes": 10,
            "last_drain_install_micros": 11,
            "max_drain_install_micros": 12,
            "lightmap_bytes_read": 13,
            "shadowmask_bytes_read": 14,
            "visible_misses_outside_baked_set": 15,
            "visible_misses_not_resident": 16,
            "failed_installs": 17,
            "failed_reads": 18,
            "refusals": 19,
            "deferrals": 20,
        });
        assert_eq!(json["counters"], expected);
        assert_eq!(json["mode"], "stream");
    }

    #[test]
    fn report_identifies_compose_mode_and_derived_animation_time() {
        let mut scene = scene_with_measurement();
        scene.force_full_resident_sh_compose = true;
        let json = as_json(measurement_report(
            &scene,
            99,
            None,
            adapter(),
            None,
            None,
            vec![1.0],
            CaptureGpuTimingState::NotRequested,
            0,
            Vec::new(),
            crate::cpu_timing::capture_stages_report(
                postretro_stage_timing::TimingGate::OFF,
                &[],
                0,
            ),
        ));

        assert_eq!(json["capture"]["force_full_resident_sh_compose"], true);
        let actual = json["capture"]["animation_time_seconds"]
            .as_f64()
            .expect("animation time serializes as a number");
        let expected = f64::from(measurement_animation_time_seconds(123));
        assert!((actual - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn report_preserves_real_zero_byte_sh_rows_without_inventing_absent_rows() {
        let accounting = ShResidencyReport {
            allocations: vec![ResidencyAllocation {
                name: "real_zero_byte_buffer",
                sources: vec![ResidencySource::Section(34)],
                bytes: 0,
                state: ResidencyAllocationState::Data,
                shape: ResidencyAllocationShape::Buffer { binding_bytes: 0 },
            }],
            total_bytes: 0,
            streaming: None,
            streaming_lifecycle: None,
        };
        let json = as_json(measurement_report(
            &scene_with_measurement(),
            99,
            Some("abc123".into()),
            adapter(),
            Some(accounting),
            None,
            vec![1.0],
            CaptureGpuTimingState::PlainBuildUnavailable,
            0,
            Vec::new(),
            crate::cpu_timing::capture_stages_report(
                postretro_stage_timing::TimingGate::OFF,
                &[],
                0,
            ),
        ));

        assert_eq!(
            json["renderer_accounted_sh"]["rows"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(json["renderer_accounted_sh"]["rows"][0]["bytes"], 0);
        assert_eq!(
            json["gpu_timing"]["availability"],
            "plain-build-unavailable"
        );
        assert_eq!(
            json["gpu_timing"]["reason"],
            "dev-tools-accessor-unavailable"
        );
    }

    #[test]
    fn report_serializes_distinct_streaming_capacity_lifetimes() {
        let accounting = ShResidencyReport {
            allocations: Vec::new(),
            total_bytes: 0,
            streaming: None,
            streaming_lifecycle: None,
        }
        .with_streaming_summary(ShStreamingAllocationSummary {
            fixed_metadata_bytes: 11,
            whole_resident_scatter_bytes: 13,
            active_capacity_bytes: 17,
            logical_occupancy_bytes: 5,
            retiring_capacity_bytes: 19,
            replacement_peak_bytes: 36,
        });
        let json = as_json(measurement_report(
            &scene_with_measurement(),
            99,
            Some("abc123".into()),
            adapter(),
            Some(accounting),
            None,
            vec![1.0],
            CaptureGpuTimingState::PlainBuildUnavailable,
            0,
            Vec::new(),
            crate::cpu_timing::capture_stages_report(
                postretro_stage_timing::TimingGate::OFF,
                &[],
                0,
            ),
        ));
        let streaming = &json["renderer_accounted_sh"]["streaming"];
        assert_eq!(streaming["fixed_metadata_bytes"], 11);
        assert_eq!(streaming["whole_resident_scatter_bytes"], 13);
        assert_eq!(streaming["active_capacity_bytes"], 17);
        assert_eq!(streaming["logical_occupancy_bytes"], 5);
        assert_eq!(streaming["retiring_capacity_bytes"], 19);
        assert_eq!(streaming["replacement_peak_bytes"], 36);
        assert_eq!(json["renderer_accounted_sh"]["total_bytes"], 47);
    }

    #[test]
    fn report_serializes_live_streaming_policy_and_cpu_phase_ledgers() {
        let accounting = ShResidencyReport {
            allocations: Vec::new(),
            total_bytes: 0,
            streaming: None,
            streaming_lifecycle: None,
        }
        .with_streaming_lifecycle_summary(ShStreamingLifecycleSummary {
            non_evictable_overshoot_bytes: 5,
            encoded_current_bytes: 1,
            encoded_high_water_upper_bound_bytes: 2,
            decoding_current_bytes: 3,
            decoding_high_water_upper_bound_bytes: 4,
            ready_current_bytes: 5,
            ready_high_water_upper_bound_bytes: 6,
            permits_in_use: 2,
            target_clusters: 7,
            absent_clusters: 1,
            queued_clusters: 2,
            ready_clusters: 3,
            installed_uncomposed_clusters: 4,
            sampleable_clusters: 5,
            failed_clusters: 6,
            misses: 7,
            installs: 8,
            evictions: 9,
            retries: 10,
            warm_clusters: 8,
            reads_issued: 11,
            coalesced_reads: 3,
            gap_bytes: 12,
            read_latency_p95_ms: 4.5,
            budget_limited_drains: 13,
            install_cpu_max_drain_micros: 14,
            install_cpu_max_steady_drain_micros: 15,
            pool_growth_cpu_micros: 16,
            indirect_compose: postretro_renderer::ShComposePassDiagnostics {
                rows_composed: 17,
                entry_rows_composed: 16,
                dispatches: 1,
                lagged_rows_composed: 4,
                resident_rows_still_lagging: 5,
            },
            static_direct_compose: postretro_renderer::ShComposePassDiagnostics {
                rows_composed: 18,
                entry_rows_composed: 17,
                dispatches: 2,
                lagged_rows_composed: 6,
                resident_rows_still_lagging: 7,
            },
            animated_direct_compose: postretro_renderer::ShComposePassDiagnostics {
                rows_composed: 19,
                entry_rows_composed: 18,
                dispatches: 3,
                lagged_rows_composed: 8,
                resident_rows_still_lagging: 9,
            },
            compose_planning_cpu_micros: 20,
            ..ShStreamingLifecycleSummary::default()
        });
        let json = as_json(measurement_report(
            &scene_with_measurement(),
            99,
            Some("abc123".into()),
            adapter(),
            Some(accounting),
            None,
            vec![1.0],
            CaptureGpuTimingState::PlainBuildUnavailable,
            0,
            Vec::new(),
            crate::cpu_timing::capture_stages_report(
                postretro_stage_timing::TimingGate::OFF,
                &[],
                0,
            ),
        ));
        let lifecycle = &json["renderer_accounted_sh"]["streaming_lifecycle"];
        assert_eq!(lifecycle["non_evictable_overshoot_bytes"], 5);
        assert_eq!(lifecycle["encoded_current_bytes"], 1);
        assert_eq!(lifecycle["ready_high_water_upper_bound_bytes"], 6);
        assert_eq!(lifecycle["permits_in_use"], 2);
        assert_eq!(lifecycle["sampleable_clusters"], 5);
        assert_eq!(lifecycle["retries"], 10);
        assert_eq!(lifecycle["warm_clusters"], 8);
        assert_eq!(lifecycle["reads_issued"], 11);
        assert_eq!(lifecycle["coalesced_reads"], 3);
        assert_eq!(lifecycle["gap_bytes"], 12);
        assert_eq!(lifecycle["read_latency_p95_ms"], 4.5);
        assert_eq!(lifecycle["budget_limited_drains"], 13);
        assert_eq!(lifecycle["install_cpu_max_drain_micros"], 14);
        assert_eq!(lifecycle["install_cpu_max_steady_drain_micros"], 15);
        assert_eq!(lifecycle["pool_growth_cpu_micros"], 16);
        assert_eq!(lifecycle["pool_growth_bytes"], 0);
        assert_eq!(lifecycle["indirect_compose"]["rows_composed"], 17);
        assert_eq!(lifecycle["indirect_compose"]["entry_rows_composed"], 16);
        assert_eq!(lifecycle["indirect_compose"]["dispatches"], 1);
        assert_eq!(
            lifecycle["static_direct_compose"]["lagged_rows_composed"],
            6
        );
        assert_eq!(
            lifecycle["animated_direct_compose"]["resident_rows_still_lagging"],
            9
        );
        assert_eq!(lifecycle["compose_planning_cpu_micros"], 20);
    }

    #[test]
    fn timing_availability_uses_setup_precedence_before_window_progress() {
        for (state, availability, reason) in [
            (
                CaptureGpuTimingState::NotRequested,
                "not-requested",
                "env-disabled",
            ),
            (
                CaptureGpuTimingState::Unsupported,
                "unsupported",
                "adapter-missing-timestamp-features",
            ),
            (
                CaptureGpuTimingState::PlainBuildUnavailable,
                "plain-build-unavailable",
                "dev-tools-accessor-unavailable",
            ),
            (
                CaptureGpuTimingState::Active,
                "not-yet-windowed",
                "window-not-complete",
            ),
        ] {
            let json = as_json(measurement_report(
                &scene_with_measurement(),
                99,
                None,
                adapter(),
                None,
                None,
                vec![1.0],
                state,
                0,
                Vec::new(),
                crate::cpu_timing::capture_stages_report(
                    postretro_stage_timing::TimingGate::OFF,
                    &[],
                    0,
                ),
            ));
            assert_eq!(json["gpu_timing"]["availability"], availability);
            assert_eq!(json["gpu_timing"]["reason"], reason);
        }
    }

    #[test]
    fn active_timing_reports_completed_windows_once_and_only_trailing_partial_count() {
        let json = as_json(measurement_report(
            &scene_with_measurement(),
            99,
            None,
            adapter(),
            None,
            None,
            vec![1.0],
            CaptureGpuTimingState::Active,
            1,
            vec![CaptureGpuTimingWindow {
                readbacks: 120,
                passes: vec![crate::render::CaptureGpuTimingPass {
                    label: "forward",
                    average_ms: Some(2.5),
                    sampled_readbacks: 117,
                    malformed_readbacks: 3,
                }],
            }],
            crate::cpu_timing::capture_stages_report(
                postretro_stage_timing::TimingGate::OFF,
                &[],
                0,
            ),
        ));

        assert_eq!(json["gpu_timing"]["availability"], "available");
        assert!(json["gpu_timing"].get("reason").is_none());
        assert_eq!(json["gpu_timing"]["windows"].as_array().unwrap().len(), 1);
        assert_eq!(json["gpu_timing"]["partial_frames"], 1);
    }

    #[test]
    fn timing_pass_report_carries_sample_counts_and_nulls_an_unsampled_average() {
        let json = as_json(measurement_report(
            &scene_with_measurement(),
            99,
            None,
            adapter(),
            None,
            None,
            vec![1.0],
            CaptureGpuTimingState::Active,
            0,
            vec![CaptureGpuTimingWindow {
                readbacks: 120,
                passes: vec![
                    crate::render::CaptureGpuTimingPass {
                        label: "forward",
                        average_ms: Some(2.5),
                        sampled_readbacks: 117,
                        malformed_readbacks: 3,
                    },
                    crate::render::CaptureGpuTimingPass {
                        label: "animated_lm_compose",
                        average_ms: None,
                        sampled_readbacks: 0,
                        malformed_readbacks: 0,
                    },
                ],
            }],
            crate::cpu_timing::capture_stages_report(
                postretro_stage_timing::TimingGate::OFF,
                &[],
                0,
            ),
        ));

        let window = &json["gpu_timing"]["windows"][0];
        assert_eq!(window["readbacks"], 120);
        let forward = &window["passes"][0];
        let forward_ms = forward["average_ms"].as_f64().unwrap();
        assert!((forward_ms - 2.5).abs() < 1e-6, "{forward_ms}");
        assert_eq!(forward["sampled_readbacks"], 117);
        assert_eq!(forward["malformed_readbacks"], 3);
        let unsampled = &window["passes"][1];
        assert!(
            unsampled["average_ms"].is_null(),
            "unsampled pass must not report zero cost: {unsampled}"
        );
        assert_eq!(unsampled["sampled_readbacks"], 0);
    }

    #[test]
    fn partial_timing_frames_follow_readbacks_not_requested_sample_count() {
        // The scene requests 121 frames. A dropped readback can leave a
        // completed 120-readback window and no trailing partial sample.
        let json = as_json(measurement_report(
            &scene_with_measurement(),
            99,
            None,
            adapter(),
            None,
            None,
            vec![1.0],
            CaptureGpuTimingState::Active,
            0,
            vec![CaptureGpuTimingWindow {
                readbacks: 120,
                passes: Vec::new(),
            }],
            crate::cpu_timing::capture_stages_report(
                postretro_stage_timing::TimingGate::OFF,
                &[],
                0,
            ),
        ));
        assert_eq!(json["workload"]["sample_frames"], 121);
        assert!(json["gpu_timing"].get("partial_frames").is_none());
    }

    #[test]
    fn measurement_report_round_trips_with_cpu_stages() {
        let report = measurement_report(
            &scene_with_measurement(),
            99,
            Some("abc123".to_string()),
            adapter(),
            None,
            None,
            vec![1.0, 2.0],
            CaptureGpuTimingState::NotRequested,
            0,
            Vec::new(),
            crate::cpu_timing::capture_stages_report(
                postretro_stage_timing::TimingGate::ON,
                &[],
                12,
            ),
        );
        let text = serde_json::to_string_pretty(&report).expect("serialize report");
        let back: MeasurementReport = serde_json::from_str(&text).expect("parse report");
        assert_eq!(back, report);
        let json = as_json(report);
        assert_eq!(json["schema"], "postretro.capture.measurement.v3");
        assert_eq!(json["cpu_stages"]["availability"], "not-yet-windowed");
        assert_eq!(json["cpu_stages"]["partial_frames"], 12);
    }
}
