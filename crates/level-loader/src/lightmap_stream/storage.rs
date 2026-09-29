// Lightmap block ownership mode for a loaded level, and the rule that selects it.
// See: context/lib/rendering_pipeline.md §4 (Lightmap cell-block residency)

use std::sync::Arc;

use super::boundary::LightmapStreamingMode;
use super::manifest::LightmapStreamManifest;

/// How a loaded level owns its id-22/42 block texels, mirroring
/// `ShStorage`.
///
/// `AllResident`: the texels are in `LevelWorld::gpu_lighting_payloads` until
/// install moves them into the pool (also placeholder mode: no id 22, or zero
/// blocks). `Streaming`: the level keeps only the manifest; the payloads stay
/// empty and blocks are read on demand. The manifest, and the retained file
/// unless an SH manifest shares it, are released when this storage drops
/// with its `LevelWorld` and every clone of the `Arc` is gone.
#[derive(Debug, Default)]
pub enum LightmapStorage {
    #[default]
    AllResident,
    Streaming(Arc<LightmapStreamManifest>),
}

impl LightmapStorage {
    pub fn manifest(&self) -> Option<&Arc<LightmapStreamManifest>> {
        match self {
            Self::AllResident => None,
            Self::Streaming(manifest) => Some(manifest),
        }
    }

    pub fn mode(&self) -> LightmapStreamingMode {
        match self {
            Self::AllResident => LightmapStreamingMode::AllResident,
            Self::Streaming(_) => LightmapStreamingMode::Stream,
        }
    }

    pub fn is_streaming(&self) -> bool {
        matches!(self, Self::Streaming(_))
    }
}

/// Why a level loaded in the mode it did. Logged at info on every load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LightmapResidencyReason {
    /// Streaming requested, with an id-51 set, usable portals and blocks.
    Streamed,
    RequestedAllResident,
    /// No usable portal graph: a baked set would mean nothing.
    NoUsablePortals,
    /// No id-51 residency set.
    NoResidencySet,
    /// The container was read from a whole image with no retained handle
    /// (a test-only load path).
    NoRetainedFile,
    /// No id 22, or zero blocks: placeholder mode.
    NoLightmapBlocks,
}

impl LightmapResidencyReason {
    fn describe(self) -> &'static str {
        match self {
            Self::Streamed => "residency set, usable portals and cell blocks present",
            Self::RequestedAllResident => "POSTRETRO_LIGHTMAP_STREAMING=all-resident",
            Self::NoUsablePortals => "level has no usable portals",
            Self::NoResidencySet => "level has no CellResidencySet (id 51)",
            Self::NoRetainedFile => "level was not read through a retained file",
            Self::NoLightmapBlocks => "level has no lightmap cell blocks",
        }
    }
}

/// What the load knows before it reads id 22.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LightmapResidencyInputs {
    pub(crate) requested: LightmapStreamingMode,
    pub(crate) has_residency_set: bool,
    pub(crate) has_usable_portals: bool,
    pub(crate) has_retained_file: bool,
}

impl LightmapResidencyInputs {
    /// Why this level cannot stream whatever its blocks, or `None` when only
    /// the block count is left to decide. The loader reads id 22 as an index
    /// prefix exactly when this is `None`.
    pub(crate) fn blocker(&self) -> Option<LightmapResidencyReason> {
        if self.requested == LightmapStreamingMode::AllResident {
            Some(LightmapResidencyReason::RequestedAllResident)
        } else if !self.has_usable_portals {
            Some(LightmapResidencyReason::NoUsablePortals)
        } else if !self.has_residency_set {
            Some(LightmapResidencyReason::NoResidencySet)
        } else if !self.has_retained_file {
            Some(LightmapResidencyReason::NoRetainedFile)
        } else {
            None
        }
    }
}

/// The one mode rule. `Stream` needs streaming requested, a usable id 51,
/// usable portals, a retained file, and at least one block; anything else
/// loads all-resident, and zero blocks stays in placeholder mode. Pure, so
/// tests pass the requested mode instead of racing on the environment.
pub(crate) fn select_lightmap_residency(
    inputs: LightmapResidencyInputs,
    block_count: u32,
) -> (LightmapStreamingMode, LightmapResidencyReason) {
    if let Some(reason) = inputs.blocker() {
        return (LightmapStreamingMode::AllResident, reason);
    }
    if block_count == 0 {
        return (
            LightmapStreamingMode::AllResident,
            LightmapResidencyReason::NoLightmapBlocks,
        );
    }
    (
        LightmapStreamingMode::Stream,
        LightmapResidencyReason::Streamed,
    )
}

pub(crate) fn log_lightmap_residency(
    mode: LightmapStreamingMode,
    reason: LightmapResidencyReason,
    block_count: u32,
) {
    let mode = match mode {
        LightmapStreamingMode::AllResident => "all-resident",
        LightmapStreamingMode::Stream => "stream",
    };
    log::info!(
        "[PRL] Lightmap residency: {mode}, {block_count} cell block(s) ({})",
        reason.describe()
    );
}
