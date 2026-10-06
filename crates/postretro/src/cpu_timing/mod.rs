// Binary-owned CPU stage timing: gate, top-level frame stages, and windows.
// See: context/lib/rendering_pipeline.md §12

mod frame_timer;
mod report;

#[cfg(test)]
mod tests;

pub(crate) use frame_timer::{CpuFrameTimer, WaitSource};
pub(crate) use postretro_stage_timing::TimingGate;
use postretro_stage_timing::{StageKind, StageSet};
#[cfg_attr(not(feature = "capture"), allow(unused_imports))]
pub(crate) use report::{CpuStagesReport, capture_stages_report};
#[cfg_attr(not(feature = "observe-live"), allow(unused_imports))]
pub(crate) use report::{CpuTimingLiveReport, live_report};

/// Environment variable that turns CPU stage timing on (`1`).
pub(crate) const CPU_TIMING_ENV: &str = "POSTRETRO_CPU_TIMING";

/// Reads the gate once at startup. Every timed crate receives it as a value.
pub(crate) fn gate_from_env() -> TimingGate {
    TimingGate::new(std::env::var(CPU_TIMING_ENV).ok().as_deref() == Some("1"))
}

/// Top-level in-level frame stages, in frame order.
///
/// A stage's time excludes any vsync block inside it: the binary moves that
/// into the frame's wait bucket (see [`CpuFrameTimer::add_wait_within`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameStage {
    /// Observe-live service, reload drain, boot-state drive, scheduler frame.
    Housekeeping,
    /// Gamepad, input mode, UI intents and focus, look rotation.
    Input,
    /// Client snapshot apply.
    SnapshotApply,
    /// The fixed-step catch-up loop. Sim substages sit inside it.
    FixedStep,
    /// Post-loop presentation: remote interpolation, client pose inputs,
    /// status overlays.
    Presentation,
    /// Descriptor sound requests and the listener update.
    Audio,
    /// Post-tick script drains, system commands, HUD, crossings, options.
    ScriptDrain,
    /// End-of-frame impact-effect removal.
    FrameEndRemoval,
    /// Host snapshot serialize and send.
    HostSend,
    /// UI focus reconcile and frame-eye assembly.
    Eye,
    /// Portal blocking, visible-cell determination, fog and light reach.
    Visibility,
    /// Bridges, collectors, uploads and UI snapshot ahead of recording.
    RenderPrep,
    /// Render recording, submit and present, less the vsync block.
    Render,
    /// Staged-manifest poll, scratch reclaim, diagnostics and title.
    FrameTail,
    /// Fixed ticks run this frame. Recorded every counted frame, zero included.
    Ticks,
}

impl StageSet for FrameStage {
    const ALL: &'static [Self] = &[
        Self::Housekeeping,
        Self::Input,
        Self::SnapshotApply,
        Self::FixedStep,
        Self::Presentation,
        Self::Audio,
        Self::ScriptDrain,
        Self::FrameEndRemoval,
        Self::HostSend,
        Self::Eye,
        Self::Visibility,
        Self::RenderPrep,
        Self::Render,
        Self::FrameTail,
        Self::Ticks,
    ];

    fn index(self) -> usize {
        self as usize
    }

    fn label(self) -> &'static str {
        match self {
            Self::Housekeeping => "housekeeping",
            Self::Input => "input",
            Self::SnapshotApply => "snapshot_apply",
            Self::FixedStep => "fixed_step",
            Self::Presentation => "presentation",
            Self::Audio => "audio",
            Self::ScriptDrain => "script_drain",
            Self::FrameEndRemoval => "frame_end_removal",
            Self::HostSend => "host_send",
            Self::Eye => "eye",
            Self::Visibility => "visibility",
            Self::RenderPrep => "render_prep",
            Self::Render => "render",
            Self::FrameTail => "frame_tail",
            Self::Ticks => "ticks",
        }
    }

    fn kind(self) -> StageKind {
        match self {
            Self::Ticks => StageKind::Count,
            _ => StageKind::Time,
        }
    }
}

/// Connected-client prediction inside the fixed-step loop. Its own labels,
/// never the host's sim labels: a client runs none of the host tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PredictionStage {
    Wieldable,
    Movers,
    Movement,
}

impl StageSet for PredictionStage {
    const ALL: &'static [Self] = &[Self::Wieldable, Self::Movers, Self::Movement];

    fn index(self) -> usize {
        self as usize
    }

    fn label(self) -> &'static str {
        match self {
            Self::Wieldable => "predict_wieldable",
            Self::Movers => "predict_movers",
            Self::Movement => "predict_movement",
        }
    }
}

/// Streamed lightmap residency inside the render-prep stage. Both are roots,
/// placed under `render_prep` by label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamingStage {
    /// The lightmap controller's per-frame CPU: demand update, completion
    /// admission and ready offers, batch assembly and read requests, then
    /// outcome apply, miss counting and diagnostics after the drain.
    LightmapResidency,
    /// The renderer's lightmap drain: placement planning, staging, recording
    /// and submission of the frame's batch.
    LightmapDrain,
}

impl StageSet for StreamingStage {
    const ALL: &'static [Self] = &[Self::LightmapResidency, Self::LightmapDrain];

    fn index(self) -> usize {
        self as usize
    }

    fn label(self) -> &'static str {
        match self {
            Self::LightmapResidency => "lightmap_residency",
            Self::LightmapDrain => "lightmap_drain",
        }
    }
}

/// The CPU particle path inside the render-prep stage: the emitter bridge's
/// spawning and the particle simulation's integrate-and-expire walk. Both are
/// roots, placed under `render_prep` by label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParticleStage {
    /// Emitter bridge: bursts, rate emission and particle spawns.
    Emit,
    /// `particle_sim::tick` over every live particle. Runs once per rendered
    /// frame on frame time, not per fixed tick.
    Sim,
}

impl StageSet for ParticleStage {
    const ALL: &'static [Self] = &[Self::Emit, Self::Sim];

    fn index(self) -> usize {
        self as usize
    }

    fn label(self) -> &'static str {
        match self {
            Self::Emit => "particle_emit",
            Self::Sim => "particle_sim",
        }
    }
}

/// Labels of the frame split the binary derives at commit. `total` and `work`
/// are aggregates; `wait` and `unattributed` sit beside the top-level stages
/// and together with them sum to `total`.
pub(crate) mod derived {
    pub(crate) const TOTAL: &str = "total";
    pub(crate) const WORK: &str = "work";
    pub(crate) const WAIT: &str = "wait";
    pub(crate) const UNATTRIBUTED: &str = "unattributed";
    /// Wait's two sources, as substages of `wait`.
    pub(crate) const WAIT_ACQUIRE: &str = "wait_acquire";
    pub(crate) const WAIT_PRESENT: &str = "wait_present";
}
