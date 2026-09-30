//! `Session` entry points for level-scope streaming: replacement, the frame's
//! drain step, and the lightmap renderer seam.
//! See: context/lib/rendering_pipeline.md §4

use std::sync::Arc;

use anyhow::{Context, Result};
use glam::Vec3;
use postretro_level_loader::{
    LevelWorld, ShDrainBatch, ShStreamManifest, requested_streaming_mode,
};
use postretro_renderer::Renderer;
use postretro_stage_timing::StageFrame;

use super::{StreamingFrame, WantedStreaming};
use crate::cpu_timing::StreamingStage;
use crate::session::lightmap_residency::{LightmapLevelView, LightmapStreamingSession};
use crate::session::sh_residency::{ShStreamingSession, require_loaded_streaming_mode};

impl crate::session::Session {
    /// Releases every streaming session, the level's issuer, and their
    /// manifest clones: at level unload, and on every legacy or world-free
    /// frame. A following streamed level always gets fresh nonzero
    /// generations, even when its content bytes match the prior map.
    pub(crate) fn clear_level_streaming(&mut self) {
        self.level_streaming.clear(&mut self.sh_streaming);
    }

    /// Creates or replaces the level's streaming sessions, updates them from
    /// this frame's visibility, and returns SH's one bounded pre-compose
    /// batch. The lightmap batch waits in its session for
    /// [`Self::drain_lightmap_streaming`].
    ///
    /// SH streams when `sh_manifest` is present; lightmaps when the level's
    /// storage is streaming. `level` supplies id 46 (read only when an SH
    /// session is created), id 49 and id 51, all from the same load as
    /// `sh_manifest`.
    pub(crate) fn prepare_streaming_drains(
        &mut self,
        sh_manifest: Option<&Arc<ShStreamManifest>>,
        level: Option<&LevelWorld>,
        renderer: &Renderer,
        frame: StreamingFrame<'_>,
    ) -> Result<ShDrainBatch> {
        self.level_streaming.poll_retirement();
        if !self.ensure_level_streaming_sessions(sh_manifest, level, renderer)? {
            return Ok(ShDrainBatch::default());
        }
        let residency_set = level
            .and_then(LightmapLevelView::of)
            .map(|view| view.residency_set);
        self.level_streaming
            .prepare_drains(&mut self.sh_streaming, residency_set, frame)
    }

    /// Level install: creates the level's streaming sessions, then makes the
    /// spawn camera cell's mandatory lightmap set resident before the first
    /// frame renders. `spawn_eye` is the eye the first frame presents. See
    /// [`super::LevelStreaming::install_spawn_lightmap`].
    pub(crate) fn install_level_streaming(
        &mut self,
        level: &LevelWorld,
        renderer: &mut Renderer,
        spawn_eye: Vec3,
    ) -> Result<()> {
        self.level_streaming.poll_retirement();
        if !self.ensure_level_streaming_sessions(
            level.sh_stream_manifest(),
            Some(level),
            renderer,
        )? {
            return Ok(());
        }
        self.level_streaming
            .install_spawn_lightmap(level, spawn_eye, |batch| {
                renderer.drain_lightmap_residency(batch)
            })?;
        if !self.lightmap_residency_settled() {
            // A failed read already warned; its block renders SH-only.
            log::warn!(
                "[Lightmap streaming] the spawn cell's mandatory set is not fully resident \
                 after preload"
            );
        }
        Ok(())
    }

    /// Whether the camera cell's mandatory lightmap set is resident, as of
    /// the latest demand update. True when the level does not stream its
    /// lightmap. This is the lightmap answer a settle chokepoint asks.
    pub(crate) fn lightmap_residency_settled(&self) -> bool {
        self.level_streaming
            .lightmap()
            .is_none_or(LightmapStreamingSession::settled)
    }

    /// Makes the streaming sessions match the loaded level (see
    /// [`super::LevelStreaming::ensure_sessions`]); a new SH session reads
    /// its budget from the renderer. Returns whether anything streams.
    fn ensure_level_streaming_sessions(
        &mut self,
        sh_manifest: Option<&Arc<ShStreamManifest>>,
        level: Option<&LevelWorld>,
        renderer: &Renderer,
    ) -> Result<bool> {
        let sh = match sh_manifest {
            Some(manifest) => {
                let mode = requested_streaming_mode()?;
                require_loaded_streaming_mode(mode)?;
                Some((manifest, mode))
            }
            None => None,
        };
        let cell_visibility = level.and_then(|world| world.cell_visibility.as_ref());
        self.level_streaming.ensure_sessions(
            &mut self.sh_streaming,
            WantedStreaming {
                sh,
                lightmap: level.and_then(LightmapLevelView::of),
                cluster_directory: level.and_then(LevelWorld::cluster_directory),
            },
            |manifest, mode, hints| {
                let hints = hints.context("[SH streaming] a streamed level carries no id 49")?;
                ShStreamingSession::from_renderer_with_mode(
                    manifest,
                    cell_visibility,
                    renderer,
                    mode,
                    hints,
                )
            },
        )
    }

    /// Hands this frame's lightmap batch to the renderer's lightmap drain and
    /// applies its result, then closes the lightmap frame: visible misses,
    /// diagnostics, and the periodic `[Lightmap streaming]` log line. Call
    /// once per frame after [`Self::prepare_streaming_drains`] and before the
    /// frame records the forward pass, so the frame samples what this drain
    /// made resident. Without a streamed lightmap it does nothing.
    ///
    /// `cpu` times the renderer drain and the controller's share apart;
    /// `now_seconds` is the monotonic time the log window throttles on.
    pub(crate) fn drain_lightmap_streaming(
        &mut self,
        renderer: &mut Renderer,
        cpu: &StageFrame<StreamingStage>,
        now_seconds: f64,
    ) -> Result<()> {
        let Some(lightmap) = self.level_streaming.lightmap_mut() else {
            return Ok(());
        };
        if let Some(batch) = lightmap.take_drain_batch_for_renderer() {
            let result = {
                let _scope = cpu.scope(StreamingStage::LightmapDrain);
                renderer.drain_lightmap_residency(batch)
            };
            let _scope = cpu.scope(StreamingStage::LightmapResidency);
            self.level_streaming.apply_lightmap_drain(result)?;
        }
        // The drain above may have declined the lightmap for the level.
        let Some(lightmap) = self.level_streaming.lightmap_mut() else {
            return Ok(());
        };
        let _scope = cpu.scope(StreamingStage::LightmapResidency);
        lightmap.finish_frame(renderer.lightmap_stream_counters().as_ref(), now_seconds);
        Ok(())
    }
}
