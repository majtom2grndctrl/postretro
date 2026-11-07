//! Everything level prep reads, captured on the main thread at dispatch, so
//! no prep task reads the App, the renderer, the options or the environment
//! live (pin P21).
//! See: context/plans/done/level-load-pipeline/index.md (Settings and scene extent, dispatch to commit)

use std::path::PathBuf;

use postretro_level_loader::ShStreamingMode;
use postretro_render_cpu::device_limits::DeviceLimits;

use super::model_prediction::ModelPredictionInputs;
use crate::App;
use crate::options::{ReflectionQuality, ShadowQuality, SurfaceDepthQuality};

/// The player render tiers the oracle's renderer stage applies before it
/// installs textures, geometry and fluids. Prep's CPU products do not depend
/// on them; the bundle carries the dispatch snapshot to the upload stage and
/// the commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LevelTiers {
    pub(crate) shadow_quality: ShadowQuality,
    pub(crate) surface_depth_quality: SurfaceDepthQuality,
    pub(crate) reflection_quality: ReflectionQuality,
}

/// One load's prep inputs.
#[derive(Debug)]
pub(crate) struct LevelPrepInputs {
    pub(crate) map_path: PathBuf,
    /// The mod's content root; sound decode and the model parse read it.
    pub(crate) content_root: PathBuf,
    /// Where the world materials' `.prm` sidecars live.
    pub(crate) prm_cache_root: PathBuf,
    /// The renderer's device limits, which the SH volume filter, the SH
    /// lattice and the lightmap family plan check against.
    pub(crate) device_limits: DeviceLimits,
    /// The `POSTRETRO_SH_STREAMING` mode, or why it could not be read. Only a
    /// streamed-SH level reads it, as the streaming sessions always did.
    pub(crate) sh_streaming_mode: Result<ShStreamingMode, String>,
    pub(crate) tiers: LevelTiers,
    /// Whether the session has audio, read where the install decides to load
    /// sounds: without it prep decodes none.
    pub(crate) audio_enabled: bool,
    /// The descriptors, built-in classnames and net role the model
    /// prediction reads.
    pub(crate) models: ModelPredictionInputs,
    /// The load's renderer-set builder, begun at dispatch; prep tasks hand
    /// it each GPU-bound product. `None` runs prep only (no renderer).
    pub(crate) upload: Option<super::upload::UploadBuilder>,
    /// Test-only failure injection and parse holds.
    #[cfg(test)]
    pub(crate) hooks: super::test_support::PrepTestHooks,
}

// Compile-time guard: the inputs move into the load's pool tasks.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<LevelPrepInputs>();
};

impl App {
    /// Capture one load's prep inputs at dispatch. `None` before the
    /// renderer or the session exists: prep plans against the device's
    /// limits and the player's tiers.
    pub(crate) fn capture_level_prep_inputs(
        &self,
        map_path: PathBuf,
        content_root: PathBuf,
    ) -> Option<LevelPrepInputs> {
        let renderer = self.renderer.as_ref()?;
        let session = self.session.as_ref()?;
        let options = &session.player_options;
        let prm_cache_root =
            crate::startup::worker::resolve_prm_root(&content_root, self.baked_root.as_deref());
        Some(LevelPrepInputs {
            map_path,
            content_root,
            prm_cache_root,
            device_limits: renderer.install_device_limits(),
            sh_streaming_mode: postretro_level_loader::requested_streaming_mode()
                .map_err(|error| error.to_string()),
            tiers: LevelTiers {
                shadow_quality: options.shadow_quality,
                surface_depth_quality: options.surface_depth_quality,
                reflection_quality: options.reflection_quality,
            },
            audio_enabled: session.audio.is_some(),
            // The descriptors this load's install will spawn from: a staged
            // reload's deferred presentation snapshot when one is pending
            // (the install promotes it before its model sweep), else the
            // committed registry's (pin P9).
            models: {
                let registry = session.scripting.script_ctx.data_registry.borrow();
                let descriptors = session
                    .scripting
                    .script_runtime
                    .deferred_mesh_descriptors()
                    .unwrap_or(&registry.entities);
                ModelPredictionInputs::capture(
                    descriptors,
                    &session.classname_dispatch,
                    self.is_connected_client(),
                )
            },
            upload: None,
            #[cfg(test)]
            hooks: Default::default(),
        })
    }
}
