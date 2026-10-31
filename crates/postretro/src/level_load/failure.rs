//! What a level load reports when it fails: the stage that failed first and
//! its message.
//! See: context/plans/in-progress/level-load-pipeline/index.md (The commit cannot fail on level data)

use postretro_level_loader::PrlLoadError;

use crate::render::LevelBuildError;

/// One kind of prep task. Each runs once per load, as soon as the hand-outs
/// (or other prep products) it reads exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum PrepKind {
    /// World `.prm` read, parse and slot plan, then the diffuse dims and the
    /// material table. Starts on hand-out 2.
    Textures,
    /// The streamed-SH residency plan. Starts on hand-outs 1 and 3; a level
    /// without an SH manifest credits it at once.
    ShResidency,
    /// The indirect-draw index-range check. Starts on hand-outs 2 and 3.
    RangeCheck,
    /// Assembling the level from its hand-outs, then UV normalize by the
    /// texture dims. Starts once the parse accepted the level and the
    /// pre-verdict tasks finished.
    Assemble,
    /// The static collision world, on the assembled level.
    Collision,
    /// The streaming sessions' level-data checks (mode gate, id-49 hints,
    /// planner topology, block map), on the assembled level: after the parse
    /// installs the manifest's cluster topology (pin P19).
    StreamingData,
    /// The lightmap family's CPU plan, on the assembled level.
    LightmapFamily,
    /// The fluid surfaces' CPU pack, on the assembled level and the texture
    /// dims.
    Fluid,
    /// The mod's `sounds/` scan and decode, which reads no level data. Starts
    /// on hand-out 4, behind the tasks the parse's earlier hand-outs feed; a
    /// dispatch with no audio credits it at once.
    Sounds,
    /// The model handle prediction, on hand-out 4's map entities and the
    /// dispatch snapshot's descriptors.
    ModelPrediction,
    /// One glTF parse per predicted model, with its clip entry, hit zones and
    /// material CPU products; credited once every model finished. An empty
    /// prediction credits it at once.
    ModelParse,
}

impl PrepKind {
    /// Every prep kind: the fixed prep set of every level.
    pub(crate) const ALL: [PrepKind; 11] = [
        PrepKind::Textures,
        PrepKind::ShResidency,
        PrepKind::RangeCheck,
        PrepKind::Assemble,
        PrepKind::Collision,
        PrepKind::StreamingData,
        PrepKind::LightmapFamily,
        PrepKind::Fluid,
        PrepKind::Sounds,
        PrepKind::ModelPrediction,
        PrepKind::ModelParse,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            PrepKind::Textures => "texture prep",
            PrepKind::ShResidency => "streamed-SH residency plan",
            PrepKind::RangeCheck => "geometry range check",
            PrepKind::Assemble => "level assembly and UV normalize",
            PrepKind::Collision => "static collision",
            PrepKind::StreamingData => "streaming level data",
            PrepKind::LightmapFamily => "lightmap family plan",
            PrepKind::Fluid => "fluid surfaces",
            PrepKind::Sounds => "sound decode",
            PrepKind::ModelPrediction => "model prediction",
            PrepKind::ModelParse => "model parse",
        }
    }

    /// Whether the task runs on hand-outs the parse may still reject. Its
    /// failure is held until the parse's verdict: a rejection wins over it,
    /// since its input was never promised valid. Sound decode reads no level
    /// data, so its failure stands on its own.
    pub(crate) fn before_verdict(self) -> bool {
        matches!(
            self,
            PrepKind::Textures
                | PrepKind::ShResidency
                | PrepKind::RangeCheck
                | PrepKind::ModelPrediction
                | PrepKind::ModelParse
        )
    }
}

/// Where a load failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadStage {
    Parse,
    Prep(PrepKind),
    /// The renderer's off-main set build on the upload thread.
    Upload,
    /// The commit's checks on what is not level data (the bundle's set
    /// still resident, its streaming sessions against that set), which run
    /// before the commit mutates anything.
    Commit,
}

impl LoadStage {
    pub(crate) fn label(self) -> &'static str {
        match self {
            LoadStage::Parse => "level parse",
            LoadStage::Prep(kind) => kind.label(),
            LoadStage::Upload => "level upload",
            LoadStage::Commit => "level commit checks",
        }
    }
}

/// The one failure a load reports: the first stage to fail, and why.
#[derive(Debug, thiserror::Error)]
#[error("{} failed: {message}", stage.label())]
pub(crate) struct LevelLoadFailure {
    pub(crate) stage: LoadStage,
    pub(crate) message: String,
    /// The parse's own error, when the parse rejected the level.
    pub(crate) parse_error: Option<PrlLoadError>,
}

impl LevelLoadFailure {
    pub(crate) fn prep(kind: PrepKind, message: impl Into<String>) -> Self {
        Self {
            stage: LoadStage::Prep(kind),
            message: message.into(),
            parse_error: None,
        }
    }

    pub(crate) fn parse(error: PrlLoadError) -> Self {
        Self {
            stage: LoadStage::Parse,
            message: error.to_string(),
            parse_error: Some(error),
        }
    }

    /// The set build's one error: a captured wgpu error, a job panic, the
    /// upload thread gone, or a GPU half rejecting the level's data.
    pub(crate) fn upload(error: &LevelBuildError) -> Self {
        Self {
            stage: LoadStage::Upload,
            message: error.to_string(),
            parse_error: None,
        }
    }

    /// A commit check that refused the bundle before anything mutated.
    pub(crate) fn commit(message: impl Into<String>) -> Self {
        Self {
            stage: LoadStage::Commit,
            message: message.into(),
            parse_error: None,
        }
    }

    /// A task that panicked instead of returning.
    pub(crate) fn panicked(stage: LoadStage, payload: &str) -> Self {
        Self {
            stage,
            message: format!("panicked: {payload}"),
            parse_error: None,
        }
    }
}

// Compile-time guard: failures cross from pool tasks to the main thread.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<LevelLoadFailure>();
};
