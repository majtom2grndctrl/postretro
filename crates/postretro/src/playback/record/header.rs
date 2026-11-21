// The playback record's header: what §12 asks a performance run to name
// beside its rows, from the build, the pose file, the level and the machine.
// See: context/lib/rendering_pipeline.md §12

use std::collections::BTreeMap;

use postretro_camera_route::{PoseFile, Projection, RouteOrigin};
use postretro_renderer::GpuTimingSupport;
use serde::Serialize;

use super::super::args::PlaybackArgs;
use super::SCHEMA;
use crate::session::level_streaming::PlayDrainCap;
use crate::streaming::drain_budget::MAX_INSTALL_DECODED_BYTES_PER_DRAIN;

#[derive(Debug, Clone, Serialize)]
pub(super) struct RecordHeader {
    pub(super) schema: &'static str,
    /// The checkout's `HEAD`, read at launch; `null` where no git checkout
    /// answers (an SDK bundle).
    pub(super) revision: Option<String>,
    pub(super) build: BuildReport,
    pub(super) pose_file: PoseFileReport,
    /// The loaded level's bake, once the first gameplay level installs.
    pub(super) level: Option<LevelReport>,
    pub(super) pacing: &'static str,
    pub(super) hold_frames: u32,
    /// The present mode requested at launch, `vsync` or `no-vsync`. The
    /// surface may fall back to another mode the adapter supports; `machine`
    /// names the one the first row observed.
    pub(super) present_mode: &'static str,
    /// The play drain cap in force at the latest written row, hold rows
    /// included; production before the first row.
    pub(super) cap: CapReport,
    /// How Settling ended: `settled` or `timed_out`; `null` when the run
    /// ended before Settling did.
    pub(super) settle: Option<&'static str>,
    /// `complete` or `aborted`; `null` only in a record never written.
    pub(super) outcome: Option<&'static str>,
    /// Why an aborted run ended.
    pub(super) abort_reason: Option<String>,
    /// What route frame 0 rendered: the file's horizontal FOV at the
    /// window's aspect.
    pub(super) rendered_projection: Option<ProjectionReport>,
    /// The machine state the first row recorded under; `null` before it.
    pub(super) machine: Option<RunContext>,
    pub(super) cpu_timing: TimingState,
    pub(super) gpu_timing: GpuTimingReport,
    /// The engine records no GPU clock in-process; a GPU figure from this
    /// record never reads as pinned.
    pub(super) gpu_clock: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct BuildReport {
    /// The cargo profile: `dev`, `release`, `profiling` or another custom one.
    profile: &'static str,
    debug_assertions: bool,
    /// The engine's cargo features this binary was built with.
    features: Vec<&'static str>,
}

impl BuildReport {
    fn of_this_build() -> Self {
        let profile = match env!("POSTRETRO_BUILD_PROFILE_DIR") {
            // Cargo builds the `dev` (and `test`) profile into `debug/`.
            "debug" => "dev",
            other => other,
        };
        let features = [
            ("dev-tools", cfg!(feature = "dev-tools")),
            ("observability", cfg!(feature = "observability")),
            ("observe-live", cfg!(feature = "observe-live")),
            ("capture", cfg!(feature = "capture")),
            ("tracy", cfg!(feature = "tracy")),
            ("test-support", cfg!(feature = "test-support")),
        ];
        Self {
            profile,
            debug_assertions: cfg!(debug_assertions),
            features: features
                .into_iter()
                .filter_map(|(name, on)| on.then_some(name))
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct PoseFileReport {
    path: String,
    origin: String,
    params: BTreeMap<String, String>,
    projection: ProjectionReport,
    bake: String,
    cadence_hz: u32,
    poses: usize,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub(super) struct ProjectionReport {
    pub(super) hfov_degrees: f32,
    pub(super) aspect: f32,
}

impl From<Projection> for ProjectionReport {
    fn from(projection: Projection) -> Self {
        Self {
            hfov_degrees: projection.hfov_degrees,
            aspect: projection.aspect,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct LevelReport {
    pub(super) bake: String,
    pub(super) identity: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct CapReport {
    /// `production`, `seed` (`POSTRETRO_PLAY_DRAIN_CAP_MIB`) or `slider`.
    pub(crate) source: &'static str,
    /// In the drain budget's own unit.
    pub(crate) decoded_bytes_per_drain: u64,
}

impl CapReport {
    pub(crate) const PRODUCTION: Self = Self {
        source: "production",
        decoded_bytes_per_drain: MAX_INSTALL_DECODED_BYTES_PER_DRAIN,
    };
}

impl From<PlayDrainCap> for CapReport {
    fn from(cap: PlayDrainCap) -> Self {
        Self {
            source: cap.source.key(),
            decoded_bytes_per_drain: cap.decoded_bytes_per_drain,
        }
    }
}

/// The run's machine state: §12's adapter, extents, divisor, effective
/// window mode and present mode.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct RunContext {
    pub(crate) adapter: AdapterReport,
    pub(crate) surface_extent: [u32; 2],
    pub(crate) scene_extent: [u32; 2],
    pub(crate) render_divisor: u32,
    pub(crate) window_mode: WindowModeReport,
    /// `vsync` or `no-vsync`.
    pub(crate) present_mode: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AdapterReport {
    pub(crate) name: String,
    pub(crate) backend: String,
    pub(crate) device_type: String,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct WindowModeReport {
    /// `windowed`, `borderless` or `exclusive`.
    pub(crate) mode: &'static str,
    /// An exclusive mode's display mode.
    pub(crate) display: Option<DisplayModeReport>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DisplayModeReport {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) refresh_millihertz: u32,
    pub(crate) monitor: String,
}

/// A timing producer's state, as telemetry's `run_start` names one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct TimingState {
    /// `enabled`, `disabled`, `unsupported`, or `pending` before the run arms.
    state: &'static str,
    /// Why it is not enabled: the variable that enables it, or what the
    /// adapter lacks.
    reason: Option<&'static str>,
}

impl TimingState {
    const PENDING: Self = Self {
        state: "pending",
        reason: None,
    };

    pub(crate) fn cpu(enabled: bool) -> Self {
        if enabled {
            Self {
                state: "enabled",
                reason: None,
            }
        } else {
            Self {
                state: "disabled",
                reason: Some("POSTRETRO_CPU_TIMING=1 enables it"),
            }
        }
    }

    pub(crate) fn gpu(support: GpuTimingSupport) -> Self {
        match support {
            GpuTimingSupport::Enabled => Self {
                state: "enabled",
                reason: None,
            },
            GpuTimingSupport::NotRequested => Self {
                state: "disabled",
                reason: Some("POSTRETRO_GPU_TIMING=1 enables it"),
            },
            GpuTimingSupport::Unsupported => Self {
                state: "unsupported",
                reason: Some(crate::telemetry::GPU_TIMING_UNSUPPORTED_REASON),
            },
        }
    }

    pub(crate) fn enabled(self) -> bool {
        self.state == "enabled"
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct GpuTimingReport {
    #[serde(flatten)]
    pub(super) state: TimingState,
    /// Hold and route rows whose readback never returned; `null` unless
    /// GPU timing ran.
    pub(super) rows_missing: Option<u64>,
    /// Spans that decoded malformed (an endpoint zero, reversed, or past one
    /// second), left absent on their rows.
    pub(super) malformed_spans: u64,
    /// Readbacks still in flight when the run's bounded end wait gave up.
    pub(super) readbacks_outstanding: u64,
    /// Marked frames that found every slot group busy and went untimed.
    pub(super) untimed_frames: u64,
    /// Samples whose frame wrote no row, or wrote a no-draw row, which has
    /// no frame encoder to time.
    pub(super) unmatched_samples: u64,
}

/// One life-state change the local pawn went through.
#[derive(Debug, Clone, Serialize)]
pub(super) struct LifeStateEntry {
    /// The first row written from the frame that saw it on: that frame's
    /// own, or the next one when that frame wrote none; `null` when the run
    /// ended before another row.
    pub(super) frame: Option<u64>,
    /// The pawn now followed; `null` when none is.
    pub(super) pawn: Option<u32>,
    pub(super) life_state: Option<u8>,
    /// The life state's declared name, when the table names it.
    pub(super) life_state_name: Option<String>,
    pub(super) life_state_entry: Option<u32>,
    pub(super) teleport_generation: Option<u8>,
    pub(super) table_revision: Option<u8>,
    /// The life state the previous frame saw.
    pub(super) previous_life_state: Option<u8>,
}

impl RecordHeader {
    pub(super) fn new(args: &PlaybackArgs, file: &PoseFile) -> Self {
        let header = &file.header;
        let origin = match header.origin {
            RouteOrigin::Generated(kind) => format!("generated {}", kind.key()),
            RouteOrigin::Recorded => "recorded".to_string(),
        };
        Self {
            schema: SCHEMA,
            revision: git_revision(),
            build: BuildReport::of_this_build(),
            pose_file: PoseFileReport {
                path: args.poses.display().to_string(),
                origin,
                params: header.params.iter().cloned().collect(),
                projection: header.projection.into(),
                bake: header.bake.clone(),
                cadence_hz: header.cadence_hz,
                poses: file.poses.len(),
            },
            level: None,
            pacing: args.pacing.key(),
            hold_frames: args.hold,
            present_mode: args.present.key(),
            cap: CapReport::PRODUCTION,
            settle: None,
            outcome: None,
            abort_reason: None,
            rendered_projection: None,
            machine: None,
            cpu_timing: TimingState::PENDING,
            gpu_timing: GpuTimingReport {
                state: TimingState::PENDING,
                rows_missing: None,
                malformed_spans: 0,
                readbacks_outstanding: 0,
                untimed_frames: 0,
                unmatched_samples: 0,
            },
            gpu_clock: "unrecorded",
        }
    }
}

/// The working directory's checkout revision, as capture reads it; `None`
/// without git or outside a checkout.
fn git_revision() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--verify", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let revision = String::from_utf8(output.stdout).ok()?;
    let revision = revision.trim();
    (!revision.is_empty()).then(|| revision.to_owned())
}
