//! `Session` entry points for level-scope streaming: replacement, the frame's
//! drain step, and the lightmap renderer seam. See: context/lib/rendering_pipeline.md §4

use std::sync::Arc;

use anyhow::{Context, Result};
use glam::Vec3;
use postretro_level_loader::{
    LevelWorld, ShDrainBatch, ShStreamManifest, requested_streaming_mode,
};
use postretro_renderer::Renderer;

use super::StreamingFrame;
use crate::session::lightmap_residency::{LightmapLevelView, LightmapStreamingSession};
use crate::session::sh_residency::{ShStreamingSession, require_loaded_streaming_mode};

impl crate::session::Session {
    /// Releases every streaming session, the level's issuer, and their
    /// manifest clones as soon as a legacy or world-free frame becomes active.
    /// A following streamed level always gets fresh nonzero generations, even
    /// when its content bytes match the prior map.
    pub(crate) fn clear_level_streaming(&mut self) {
        self.level_streaming.poll_retirement();
        self.level_streaming.retire(&mut self.sh_streaming);
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
        let lightmap = level.and_then(LightmapLevelView::of);
        if !self.ensure_level_streaming_sessions(sh_manifest, level, lightmap, renderer)? {
            return Ok(ShDrainBatch::default());
        }
        self.level_streaming.prepare_drains(
            &mut self.sh_streaming,
            lightmap.map(|view| view.residency_set),
            frame,
        )
    }

    /// Level install: creates the level's streaming sessions, then makes the
    /// spawn camera cell's mandatory lightmap set resident before the first
    /// frame renders. `spawn_eye` is the eye the first frame presents; its
    /// cell's baked set within lead L, plus the pinned blocks, is read
    /// synchronously through the level's positional reader and installed
    /// through the renderer's lightmap drain. An empty spawn range installs
    /// nothing (P10), and play never waits on a block afterwards.
    pub(crate) fn install_level_streaming(
        &mut self,
        level: &LevelWorld,
        renderer: &mut Renderer,
        spawn_eye: Vec3,
    ) -> Result<()> {
        self.level_streaming.poll_retirement();
        let lightmap = LightmapLevelView::of(level);
        if !self.ensure_level_streaming_sessions(
            level.sh_stream_manifest(),
            Some(level),
            lightmap,
            renderer,
        )? {
            return Ok(());
        }
        let (Some(view), Some(session)) = (lightmap, self.level_streaming.lightmap_mut()) else {
            return Ok(());
        };
        let camera_cell = level.locate_cell(spawn_eye) as u32;
        session.update_camera_set(view.residency_set, camera_cell);
        let summary = session.preload(&[], |batch| {
            renderer
                .drain_lightmap_residency(batch)
                .context("[Lightmap streaming] spawn preload drain")
        })?;
        log::info!(
            "[Lightmap streaming] spawn preload: camera cell {camera_cell}, {} of {} pair(s) \
             installed, {:.1} MiB read in {:.1} ms",
            summary.installed,
            summary.reads.pairs + summary.reads.failed,
            summary.reads.bytes as f64 / (1024.0 * 1024.0),
            summary.elapsed.as_secs_f64() * 1000.0,
        );
        if !self.lightmap_residency_settled() {
            // A failed read already warned; its block renders SH-only.
            log::warn!(
                "[Lightmap streaming] spawn cell {camera_cell}'s mandatory set is not fully \
                 resident after preload"
            );
        }
        Ok(())
    }

    /// Whether the camera cell's mandatory lightmap set is resident, as of
    /// the latest demand update. True when the level does not stream its
    /// lightmap. This is the lightmap answer a settle chokepoint asks
    /// (`drafts/sh-streaming--reveal-gate-and-warm-horizon`).
    pub(crate) fn lightmap_residency_settled(&self) -> bool {
        self.level_streaming
            .lightmap()
            .is_none_or(LightmapStreamingSession::settled)
    }

    /// Makes the streaming sessions match the loaded level. Either session
    /// changing identity or SH changing mode replaces both, because they
    /// share one issuer. Returns whether anything streams; when nothing
    /// does, every session is released.
    fn ensure_level_streaming_sessions(
        &mut self,
        sh_manifest: Option<&Arc<ShStreamManifest>>,
        level: Option<&LevelWorld>,
        lightmap: Option<LightmapLevelView<'_>>,
        renderer: &Renderer,
    ) -> Result<bool> {
        if sh_manifest.is_none() && lightmap.is_none() {
            self.clear_level_streaming();
            return Ok(false);
        }
        let sh_mode = match sh_manifest {
            Some(_) => {
                let mode = requested_streaming_mode()?;
                require_loaded_streaming_mode(mode)?;
                Some(mode)
            }
            None => None,
        };
        let sh_current = match (sh_manifest.zip(sh_mode), self.sh_streaming.as_ref()) {
            (Some((manifest, mode)), Some(streaming)) => streaming.is_for(manifest, mode),
            (None, None) => true,
            _ => false,
        };
        let lightmap_current = match (&lightmap, self.level_streaming.lightmap()) {
            (Some(view), Some(streaming)) => streaming.is_for(view.manifest),
            (None, None) => true,
            _ => false,
        };
        if !(sh_current && lightmap_current) {
            // Cancel the old generation now. Its threads retire off the frame
            // path; the replacement issuer starts only after they have joined.
            self.clear_level_streaming();
            if let Some((manifest, mode)) = sh_manifest.zip(sh_mode) {
                self.sh_streaming = Some(ShStreamingSession::from_renderer_with_mode(
                    manifest.clone(),
                    level.and_then(|world| world.cell_visibility.as_ref()),
                    renderer,
                    mode,
                )?);
            }
            if let Some(view) = lightmap {
                self.level_streaming
                    .install_lightmap(LightmapStreamingSession::new(view)?);
            }
        }
        Ok(true)
    }

    /// Hands this frame's lightmap batch to the renderer's lightmap drain and
    /// takes back its outcome. Call once per frame after
    /// [`Self::prepare_streaming_drains`] and before the frame records the
    /// forward pass, so the frame samples what this drain made resident.
    /// Without a streamed lightmap it does nothing.
    pub(crate) fn drain_lightmap_streaming(&mut self, renderer: &mut Renderer) -> Result<()> {
        let Some(lightmap) = self.level_streaming.lightmap_mut() else {
            return Ok(());
        };
        let Some(batch) = lightmap.take_drain_batch_for_renderer() else {
            return Ok(());
        };
        let outcome = renderer
            .drain_lightmap_residency(batch)
            .context("[Lightmap streaming] renderer drain")?;
        lightmap.apply_outcome(outcome)
    }
}
