// Capture measurement report schema and CPU-only summary math.
// See: context/lib/rendering_pipeline.md §7.8

#[cfg(test)]
use postretro_renderer::ShStreamingAllocationSummary;
use serde::Serialize;

use super::prepared::measurement_animation_time_seconds;
use super::scene::{CameraPose, CaptureScene};
use crate::render::{
    CaptureAdapterIdentity, CaptureGpuTimingState, CaptureGpuTimingWindow, ShResidencyAllocation,
    ShResidencyAllocationShape, ShResidencyAllocationState, ShResidencyReport, ShResidencySource,
    ShStreamingLifecycleSummary,
};

const MEASUREMENT_SCHEMA: &str = "postretro.capture.measurement.v1";
const CPU_COMPLETION_STRATEGY: &str = "device-poll-wait-after-submit";
const CPU_COMPLETION_CADENCE: &str = "once-per-sample-frame";

/// Build the schema-v1 report only after all sample frames have completed.
/// The caller owns staging and publication so a previous successful report is
/// never replaced until the final PNG is visible.
pub(super) fn measurement_report(
    scene: &CaptureScene,
    map_bytes: u64,
    revision: Option<String>,
    adapter: CaptureAdapterIdentity,
    sh_residency: Option<ShResidencyReport>,
    cpu_samples_ms: Vec<f64>,
    timing_state: CaptureGpuTimingState,
    gpu_partial_frames: u32,
    gpu_windows: Vec<CaptureGpuTimingWindow>,
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
        schema: MEASUREMENT_SCHEMA,
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
        cpu_completion: CpuCompletionReport {
            unit: "milliseconds",
            strategy: CPU_COMPLETION_STRATEGY,
            cadence: CPU_COMPLETION_CADENCE,
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

#[derive(Debug, Serialize)]
pub(super) struct MeasurementReport {
    schema: &'static str,
    revision: Option<String>,
    map: MapReport,
    capture: CaptureReport,
    workload: WorkloadReport,
    adapter: AdapterReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    renderer_accounted_sh: Option<ShResidencyReportJson>,
    cpu_completion: CpuCompletionReport,
    gpu_timing: GpuTimingReport,
}

#[derive(Debug, Serialize)]
struct MapReport {
    path: String,
    bytes: u64,
}

#[derive(Debug, Serialize)]
struct CaptureReport {
    output: String,
    resolution: [u32; 2],
    camera: CameraReport,
    force_full_resident_sh_compose: bool,
    animation_time_seconds: f32,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
struct WorkloadReport {
    warmup_frames: u32,
    sample_frames: u32,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
struct CpuCompletionReport {
    unit: &'static str,
    strategy: &'static str,
    cadence: &'static str,
    samples_ms: Vec<f64>,
    median_ms: f64,
    p95_ms: f64,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum GpuTimingAvailability {
    Available,
    NotRequested,
    Unsupported,
    PlainBuildUnavailable,
    NotYetWindowed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum GpuTimingReason {
    EnvDisabled,
    AdapterMissingTimestampFeatures,
    DevToolsAccessorUnavailable,
    WindowNotComplete,
}

#[derive(Debug, Serialize)]
struct GpuTimingWindowReport {
    passes: Vec<GpuTimingPassReport>,
}

impl From<CaptureGpuTimingWindow> for GpuTimingWindowReport {
    fn from(window: CaptureGpuTimingWindow) -> Self {
        Self {
            passes: window
                .passes
                .into_iter()
                .map(|pass| GpuTimingPassReport {
                    label: pass.label,
                    average_ms: pass.average_ms,
                    skipped_frames: pass.skipped_frames,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize)]
struct GpuTimingPassReport {
    label: &'static str,
    average_ms: f32,
    skipped_frames: u32,
}

#[derive(Debug, Serialize)]
struct ShResidencyReportJson {
    rows: Vec<ShResidencyAllocationJson>,
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
                .map(ShResidencyAllocationJson::from)
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

/// Streaming values remain outside descriptor rows because logical occupancy
/// and replacement peak are accounting views, not new wgpu allocations.
#[derive(Debug, Serialize)]
struct ShStreamingAllocationSummaryJson {
    fixed_metadata_bytes: u64,
    whole_resident_scatter_bytes: u64,
    active_capacity_bytes: u64,
    logical_occupancy_bytes: u64,
    retiring_capacity_bytes: u64,
    replacement_peak_bytes: u64,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
struct ShComposePassDiagnosticsJson {
    rows_composed: u64,
    dispatches: u64,
    lagged_rows_composed: u64,
    resident_rows_still_lagging: u64,
}

impl From<postretro_renderer::ShComposePassDiagnostics> for ShComposePassDiagnosticsJson {
    fn from(diagnostics: postretro_renderer::ShComposePassDiagnostics) -> Self {
        Self {
            rows_composed: diagnostics.rows_composed,
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

#[derive(Debug, Serialize)]
struct ShResidencyAllocationJson {
    name: &'static str,
    sources: Vec<ShResidencySourceJson>,
    bytes: u64,
    state: ShResidencyStateJson,
    shape: ShResidencyShapeJson,
}

impl From<ShResidencyAllocation> for ShResidencyAllocationJson {
    fn from(row: ShResidencyAllocation) -> Self {
        Self {
            name: row.name,
            sources: row
                .sources
                .into_iter()
                .map(ShResidencySourceJson::from)
                .collect(),
            bytes: row.bytes,
            state: ShResidencyStateJson::from(row.state),
            shape: ShResidencyShapeJson::from(row.shape),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ShResidencySourceJson {
    Section { id: u16 },
    Derived,
}

impl From<ShResidencySource> for ShResidencySourceJson {
    fn from(source: ShResidencySource) -> Self {
        match source {
            ShResidencySource::Section(id) => Self::Section { id },
            ShResidencySource::Derived => Self::Derived,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum ShResidencyStateJson {
    Data,
    Dummy,
    Fallback,
}

impl From<ShResidencyAllocationState> for ShResidencyStateJson {
    fn from(state: ShResidencyAllocationState) -> Self {
        match state {
            ShResidencyAllocationState::Data => Self::Data,
            ShResidencyAllocationState::Dummy => Self::Dummy,
            ShResidencyAllocationState::Fallback => Self::Fallback,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ShResidencyShapeJson {
    Texture {
        format: &'static str,
        dimension: &'static str,
        extent: [u32; 3],
    },
    Buffer {
        binding_bytes: u64,
    },
}

impl From<ShResidencyAllocationShape> for ShResidencyShapeJson {
    fn from(shape: ShResidencyAllocationShape) -> Self {
        match shape {
            ShResidencyAllocationShape::Texture {
                format,
                dimension,
                extent,
            } => Self::Texture {
                format,
                dimension,
                extent,
            },
            ShResidencyAllocationShape::Buffer { binding_bytes } => Self::Buffer { binding_bytes },
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
            vec![1.0],
            CaptureGpuTimingState::NotRequested,
            0,
            Vec::new(),
        ));

        assert_eq!(json["schema"], MEASUREMENT_SCHEMA);
        assert!(json.get("renderer_accounted_sh").is_none());
        assert_eq!(json["gpu_timing"]["availability"], "not-requested");
        assert_eq!(json["gpu_timing"]["reason"], "env-disabled");
        assert!(json["gpu_timing"].get("windows").is_none());
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
            vec![1.0],
            CaptureGpuTimingState::NotRequested,
            0,
            Vec::new(),
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
            allocations: vec![ShResidencyAllocation {
                name: "real_zero_byte_buffer",
                sources: vec![ShResidencySource::Section(34)],
                bytes: 0,
                state: ShResidencyAllocationState::Data,
                shape: ShResidencyAllocationShape::Buffer { binding_bytes: 0 },
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
            vec![1.0],
            CaptureGpuTimingState::PlainBuildUnavailable,
            0,
            Vec::new(),
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
            vec![1.0],
            CaptureGpuTimingState::PlainBuildUnavailable,
            0,
            Vec::new(),
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
                dispatches: 1,
                lagged_rows_composed: 4,
                resident_rows_still_lagging: 5,
            },
            static_direct_compose: postretro_renderer::ShComposePassDiagnostics {
                rows_composed: 18,
                dispatches: 2,
                lagged_rows_composed: 6,
                resident_rows_still_lagging: 7,
            },
            animated_direct_compose: postretro_renderer::ShComposePassDiagnostics {
                rows_composed: 19,
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
            vec![1.0],
            CaptureGpuTimingState::PlainBuildUnavailable,
            0,
            Vec::new(),
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
                vec![1.0],
                state,
                0,
                Vec::new(),
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
            vec![1.0],
            CaptureGpuTimingState::Active,
            1,
            vec![CaptureGpuTimingWindow {
                passes: vec![crate::render::CaptureGpuTimingPass {
                    label: "forward",
                    average_ms: 2.5,
                    skipped_frames: 3,
                }],
            }],
        ));

        assert_eq!(json["gpu_timing"]["availability"], "available");
        assert!(json["gpu_timing"].get("reason").is_none());
        assert_eq!(json["gpu_timing"]["windows"].as_array().unwrap().len(), 1);
        assert_eq!(json["gpu_timing"]["partial_frames"], 1);
    }

    #[test]
    fn partial_timing_frames_follow_readbacks_not_requested_sample_count() {
        // The scene requests 121 frames. A dropped readback can leave a
        // completed 120-sample window and no trailing partial sample.
        let json = as_json(measurement_report(
            &scene_with_measurement(),
            99,
            None,
            adapter(),
            None,
            vec![1.0],
            CaptureGpuTimingState::Active,
            0,
            vec![CaptureGpuTimingWindow { passes: Vec::new() }],
        ));
        assert_eq!(json["workload"]["sample_frames"], 121);
        assert!(json["gpu_timing"].get("partial_frames").is_none());
    }
}
