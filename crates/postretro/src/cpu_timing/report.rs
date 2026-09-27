// Serialized CPU timing windows shared by the capture report and observe-live.
// See: context/lib/rendering_pipeline.md §12

use postretro_stage_timing::{StageKind, TimingGate, WindowSnapshot};
use serde::{Deserialize, Serialize};

/// Whether windows are present, and why not. Mirrors the capture report's
/// `gpu_timing` availability so absent data never reads as a zero-cost stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CpuTimingAvailability {
    Available,
    NotRequested,
    NotYetWindowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CpuTimingReason {
    /// `POSTRETRO_CPU_TIMING` is not `1`.
    EnvDisabled,
    /// Timing is on, but no 120-frame window has closed yet.
    WindowNotComplete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum CpuStageKind {
    Time,
    Count,
    Marker,
}

/// One stage over a closed window. Stages that did not run are omitted, never
/// reported as zero. `average` and `max` are milliseconds for a time stage and
/// units for a count; a marker carries only `frames`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CpuStageReport {
    pub(crate) label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent: Option<String>,
    pub(crate) kind: CpuStageKind,
    /// Frames of the window this stage ran in; its average covers only these.
    pub(crate) frames: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) average: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) max: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CpuWindowReport {
    pub(crate) frames: u32,
    pub(crate) stages: Vec<CpuStageReport>,
}

impl From<&WindowSnapshot> for CpuWindowReport {
    fn from(window: &WindowSnapshot) -> Self {
        Self {
            frames: window.frames,
            stages: window
                .rows
                .iter()
                .map(|row| {
                    let (kind, average, max) = match row.kind {
                        StageKind::Time => (
                            CpuStageKind::Time,
                            Some(row.average_ms()),
                            Some(row.max_ms()),
                        ),
                        StageKind::Count => {
                            (CpuStageKind::Count, Some(row.average), Some(row.max as f64))
                        }
                        StageKind::Marker => (CpuStageKind::Marker, None, None),
                    };
                    CpuStageReport {
                        label: row.label.to_string(),
                        parent: row.parent.map(str::to_string),
                        kind,
                        frames: row.frames,
                        average,
                        max,
                    }
                })
                .collect(),
        }
    }
}

/// The capture report's `cpu_stages` section: every complete post-warmup
/// window once, and a trailing partial window only as a frame count.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CpuStagesReport {
    pub(crate) availability: CpuTimingAvailability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<CpuTimingReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) windows: Option<Vec<CpuWindowReport>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) partial_frames: Option<u32>,
}

#[cfg_attr(not(feature = "capture"), allow(dead_code))]
pub(crate) fn capture_stages_report(
    gate: TimingGate,
    windows: &[WindowSnapshot],
    partial_frames: u32,
) -> CpuStagesReport {
    if !gate.is_enabled() {
        return CpuStagesReport {
            availability: CpuTimingAvailability::NotRequested,
            reason: Some(CpuTimingReason::EnvDisabled),
            windows: None,
            partial_frames: None,
        };
    }
    let partial_frames = (partial_frames > 0).then_some(partial_frames);
    if windows.is_empty() {
        return CpuStagesReport {
            availability: CpuTimingAvailability::NotYetWindowed,
            reason: Some(CpuTimingReason::WindowNotComplete),
            windows: None,
            partial_frames,
        };
    }
    CpuStagesReport {
        availability: CpuTimingAvailability::Available,
        reason: None,
        windows: Some(windows.iter().map(CpuWindowReport::from).collect()),
        partial_frames,
    }
}

/// Observe-live's live-only `cpu_timing` section: the latest closed window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct CpuTimingLiveReport {
    pub(crate) availability: CpuTimingAvailability,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<CpuTimingReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) window: Option<CpuWindowReport>,
}

#[cfg_attr(not(feature = "observe-live"), allow(dead_code))]
pub(crate) fn live_report(gate: TimingGate, last: Option<&WindowSnapshot>) -> CpuTimingLiveReport {
    let (availability, reason) = match (gate.is_enabled(), last) {
        (false, _) => (
            CpuTimingAvailability::NotRequested,
            Some(CpuTimingReason::EnvDisabled),
        ),
        (true, None) => (
            CpuTimingAvailability::NotYetWindowed,
            Some(CpuTimingReason::WindowNotComplete),
        ),
        (true, Some(_)) => (CpuTimingAvailability::Available, None),
    };
    CpuTimingLiveReport {
        availability,
        reason,
        window: last
            .filter(|_| gate.is_enabled())
            .map(CpuWindowReport::from),
    }
}
