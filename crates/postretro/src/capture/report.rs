// Capture measurement report schema and CPU-only summary math.
// See: context/lib/rendering_pipeline.md §7.8

use serde::Serialize;

use super::scene::{CameraPose, CaptureScene};
use crate::render::{
    CaptureAdapterIdentity, CaptureGpuTimingState, CaptureGpuTimingWindow, ShResidencyAllocation,
    ShResidencyAllocationShape, ShResidencyAllocationState, ShResidencyReport, ShResidencySource,
};

const MEASUREMENT_SCHEMA: &str = "postretro.capture.measurement.v1";
const CPU_COMPLETION_STRATEGY: &str = "device-poll-wait-after-submit";
const CPU_COMPLETION_CADENCE: &str = "once-per-sample-frame";
const GPU_TIMING_WINDOW_FRAMES: u32 = 120;

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
    gpu_windows: Vec<CaptureGpuTimingWindow>,
) -> MeasurementReport {
    let measurement = scene
        .measurement
        .as_ref()
        .expect("measurement report requires validated measurement scene");
    let (gpu_timing, partial_frames) =
        gpu_timing_report(timing_state, measurement.sample_frames, gpu_windows);
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
    sample_frames: u32,
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
        let partial = sample_frames % GPU_TIMING_WINDOW_FRAMES;
        (partial != 0).then_some(partial)
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
}

impl From<ShResidencyReport> for ShResidencyReportJson {
    fn from(report: ShResidencyReport) -> Self {
        Self {
            rows: report
                .allocations
                .into_iter()
                .map(ShResidencyAllocationJson::from)
                .collect(),
            total_bytes: report.total_bytes,
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
            Vec::new(),
        ));

        assert_eq!(json["schema"], MEASUREMENT_SCHEMA);
        assert!(json.get("renderer_accounted_sh").is_none());
        assert_eq!(json["gpu_timing"]["availability"], "not-requested");
        assert_eq!(json["gpu_timing"]["reason"], "env-disabled");
        assert!(json["gpu_timing"].get("windows").is_none());
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
        };
        let json = as_json(measurement_report(
            &scene_with_measurement(),
            99,
            Some("abc123".into()),
            adapter(),
            Some(accounting),
            vec![1.0],
            CaptureGpuTimingState::PlainBuildUnavailable,
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
}
