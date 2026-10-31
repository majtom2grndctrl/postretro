//! What level prep hands on: the GPU-bound products the upload stage
//! consumes, and the main-pinned ones the commit consumes.
//! See: context/plans/in-progress/level-load-pipeline/index.md (Staged pipeline)

use std::path::PathBuf;
use std::time::Duration;

use postretro_level_loader::LevelWorld;
use postretro_render_cpu::device_limits::DeviceLimits;
use postretro_render_cpu::fluid_surface::prepared::PreparedFluidSurfaces;
use postretro_render_cpu::lightmap_family::LightmapFamilyPlan;
use postretro_render_cpu::sh_residency::ShResidencyPlan;
use postretro_render_cpu::texture_prep::PreparedWorldTextures;
use postretro_render_data::material::Material;

use super::{LevelTiers, PrepKind};
use crate::collision::CollisionWorld;
use crate::session::level_streaming::StreamingLevelData;

/// One level, every level-data check passed and every CPU product built.
#[derive(Debug)]
pub(crate) struct PreparedLevel {
    /// The assembled level, UVs normalized by the prepared texture dims. It
    /// still holds its GPU-only lightmap and shadowmask payloads, which the
    /// upload takes (`LevelWorld::take_gpu_lighting_payloads`); the commit
    /// installs the rest as today's `App::level`.
    pub(crate) world: LevelWorld,
    /// What the upload stage builds the load's renderer set from.
    pub(crate) gpu: PreparedGpuLevel,
    /// The static collision world the commit installs.
    pub(crate) collision: CollisionWorld,
    /// What the commit creates the level's streaming sessions from.
    pub(crate) streaming: StreamingLevelData,
    /// Sound and model products: empty until their prep lands.
    pub(crate) presentation: PreparedPresentation,
    /// Where the load's time went, for log line C.
    pub(crate) timings: PrepTimings,
}

/// The upload stage's inputs, all `Send`.
#[derive(Debug)]
pub(crate) struct PreparedGpuLevel {
    pub(crate) textures: PreparedWorldTextures,
    /// One material per world texture name: shininess and Surface Depth
    /// inputs derived from the name, as the oracle derives them.
    pub(crate) texture_materials: Vec<Material>,
    /// Each bound world material slot's diffuse size (one placeholder slot
    /// for a level with no materials): what UV normalize and the fluid pack
    /// used.
    pub(crate) diffuse_dims: Vec<(u32, u32)>,
    /// The streamed-SH residency plan; `None` for a level without an SH
    /// manifest.
    pub(crate) sh_residency: Option<ShResidencyPlan>,
    pub(crate) lightmap_family: LightmapFamilyPlan,
    /// `None` for a level without a fluid surface.
    pub(crate) fluid: Option<PreparedFluidSurfaces>,
    /// The limits every plan checked against, and the tiers the load was
    /// dispatched at.
    pub(crate) device_limits: DeviceLimits,
    pub(crate) tiers: LevelTiers,
    pub(crate) prm_cache_root: PathBuf,
}

/// The seam for sound decode and model prediction and parse, which land in
/// prep next; empty until then.
#[derive(Debug, Default)]
pub(crate) struct PreparedPresentation {}

/// Prep's own timings: the parse's wall time, then each task's, in the order
/// they finished. Log-only.
#[derive(Debug, Default, Clone)]
pub(crate) struct PrepTimings {
    pub(crate) parse: Duration,
    pub(crate) tasks: Vec<(PrepKind, Duration)>,
}

// Compile-time guard: the prepared level crosses from the load pool to the
// upload stage and the main thread.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<PreparedLevel>();
    assert_send::<PreparedGpuLevel>();
};
