//! What level prep hands on: the GPU-bound products the upload stage
//! consumes, and the main-pinned ones the commit consumes.
//! See: context/plans/in-progress/level-load-pipeline/index.md (Staged pipeline)

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use postretro_audio::DecodedLevelSounds;
use postretro_model::gltf_loader::{LoadedModel, ModelLoadError};
use postretro_render_cpu::texture_prep::PreparedModelMaterials;

use postretro_level_loader::{GpuLightingPayloads, LevelWorld};
use postretro_render_cpu::device_limits::DeviceLimits;
use postretro_render_cpu::fluid_surface::prepared::PreparedFluidSurfaces;
use postretro_render_cpu::lightmap_family::LightmapFamilyPlan;
use postretro_render_cpu::sh_residency::ShResidencyPlan;
use postretro_render_cpu::texture_prep::PreparedWorldTextures;
use postretro_render_data::material::Material;

use super::model_cache::ModelCacheUse;
use super::{LevelTiers, PrepKind};
use crate::collision::CollisionWorld;
use crate::scripting_systems::hit_zones::ModelHitZones;
use crate::session::level_streaming::StreamingLevelData;
use crate::startup::lifecycle::ModelClipEntry;

/// One level, every level-data check passed and every CPU product built.
#[derive(Debug)]
pub(crate) struct PreparedLevel {
    /// The assembled level, UVs normalized by the prepared texture dims, its
    /// GPU-only lightmap and shadowmask payloads taken
    /// (`LevelWorld::take_gpu_lighting_payloads`); the commit installs it as
    /// today's `App::level`. Shared: a load that builds a set hands the
    /// geometry part a clone, which the part drops before the set arrives,
    /// so the load-stage handle unwraps it then.
    pub(crate) world: Arc<LevelWorld>,
    /// What the upload stage builds the load's renderer set from, kept here
    /// only by a load that builds no set (no renderer at dispatch). A load
    /// with a set builder handed each product to it as it was made.
    pub(crate) gpu: Option<PreparedGpuLevel>,
    /// The static collision world the commit installs.
    pub(crate) collision: CollisionWorld,
    /// What the commit creates the level's streaming sessions from.
    pub(crate) streaming: StreamingLevelData,
    /// The sounds and models the commit registers and installs.
    pub(crate) presentation: PreparedPresentation,
    /// Where the load's time went, for log line C.
    pub(crate) timings: PrepTimings,
}

/// The upload stage's inputs, all `Send`. A prep-only bundle keeps them;
/// production never dispatches one (the load state machine holds a load
/// queued until the renderer is full-ready), so only tests read them.
#[cfg_attr(not(test), allow(dead_code))]
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
    /// The level's GPU-only lightmap and shadowmask payloads, taken out of
    /// the world at assembly.
    pub(crate) gpu_lighting_payloads: GpuLightingPayloads,
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

/// The main-pinned presentation products: the level's sounds, decoded, and
/// its predicted models, parsed with their CPU products built.
#[derive(Debug, Default)]
pub(crate) struct PreparedPresentation {
    /// The level's sounds in sorted path order, for the commit to register
    /// (`Audio::register_level_sounds`). `None` when the dispatch snapshot
    /// had no audio, as the install then loads none.
    pub(crate) sounds: Option<DecodedLevelSounds>,
    pub(crate) models: PreparedModels,
}

/// Every predicted model, prepared, in predicted order and keyed by handle.
#[derive(Debug, Default)]
pub(crate) struct PreparedModels {
    models: Vec<PreparedModel>,
    by_handle: HashMap<String, usize>,
}

impl PreparedModels {
    /// `models` in predicted order; a repeated handle keeps its first entry.
    pub(crate) fn new(models: Vec<PreparedModel>) -> Self {
        let mut prepared = Self::default();
        for model in models {
            if !prepared.by_handle.contains_key(&model.handle) {
                prepared
                    .by_handle
                    .insert(model.handle.clone(), prepared.models.len());
                prepared.models.push(model);
            }
        }
        prepared
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// The predicted handles, in predicted order.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn handles(&self) -> impl Iterator<Item = &str> {
        self.models.iter().map(|model| model.handle.as_str())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &PreparedModel> {
        self.models.iter()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn get(&self, handle: &str) -> Option<&PreparedModel> {
        self.by_handle.get(handle).map(|&index| &self.models[index])
    }

    /// The models, in predicted order, for a consumer that takes them.
    pub(crate) fn into_models(self) -> Vec<PreparedModel> {
        self.models
    }

    /// The prediction misses: handles the real sweep holds that prep did not
    /// prepare, in the sweep's order. Each one is a counted synchronous
    /// fallback at commit.
    pub(crate) fn misses<'a>(&self, real_sweep: &'a [String]) -> Vec<&'a str> {
        real_sweep
            .iter()
            .map(String::as_str)
            .filter(|handle| !self.by_handle.contains_key(*handle))
            .collect()
    }

    /// The over-predictions: handles prep prepared that the real sweep does
    /// not hold, in predicted order. Each one cost a parse and nothing else.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn over_predictions(&self, real_sweep: &[String]) -> Vec<&str> {
        let real: std::collections::HashSet<&str> = real_sweep.iter().map(String::as_str).collect();
        self.handles()
            .filter(|handle| !real.contains(handle))
            .collect()
    }
}

/// One predicted model, prepared on the pool by the oracle sweep's own
/// builders: the parse (`load_model`'s two halves), the clip-table entry
/// (`model_clip_entry`), the hit zones (`ModelHitZones::build`) and the CPU
/// half of the upload (`prepare_model_materials`), or taken from the shared
/// model cache when its content key hits.
pub(crate) struct PreparedModel {
    /// The verbatim handle: the renderer's cache key and `MeshComponent.model`.
    pub(crate) handle: String,
    /// `content_root.join(handle)`, the file that was opened; a failed
    /// parse's warning names it.
    pub(crate) open_path: PathBuf,
    /// The game-side clip-table entry; the empty entry with zero bounds for a
    /// model that failed to parse.
    pub(crate) clip_entry: ModelClipEntry,
    /// The parse and what it feeds, or the parse's error, which degrades
    /// exactly as the oracle sweep's: warned, nothing cached, never a load
    /// failure. A cache hit carries the products of the parse that made the
    /// cached entry, so the commit reads a hit as it reads a parse.
    pub(crate) parsed: Result<ParsedModelProducts, ModelLoadError>,
    /// How the model met the shared cache (log line C and tests; the commit
    /// never branches on it).
    pub(crate) cache: ModelCacheUse,
}

/// A parsed model and the CPU products built from it.
pub(crate) struct ParsedModelProducts {
    /// The hit-zone entry `HitZoneStore::insert_built` installs.
    pub(crate) hit_zones: ModelHitZones,
    /// What the upload's model part reads, kept here only by a load that
    /// builds no set; a load with a set builder moved it into the model's
    /// part from the parse task.
    pub(crate) upload: Option<ModelUpload>,
}

/// One parsed model's upload inputs, kept only by a prep-only load (see
/// [`PreparedGpuLevel`]).
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct ModelUpload {
    /// The parse itself, which the upload reads (mesh, skeleton, clips).
    pub(crate) model: LoadedModel,
    /// The submesh material plan and each distinct key's prepared diffuse,
    /// so the upload's model part only creates GPU objects.
    pub(crate) materials: PreparedModelMaterials,
}

impl std::fmt::Debug for PreparedModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedModel")
            .field("handle", &self.handle)
            .field("clips", &self.clip_entry.clips.len())
            .field("cache", &self.cache)
            .field(
                "parsed",
                &self
                    .parsed
                    .as_ref()
                    .map(|_| ())
                    .map_err(ToString::to_string),
            )
            .finish_non_exhaustive()
    }
}

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
    assert_send::<PreparedPresentation>();
    assert_send::<PreparedModel>();
};
