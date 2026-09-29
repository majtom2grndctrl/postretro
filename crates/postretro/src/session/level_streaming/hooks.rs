//! `Session` entry points for level-scope streaming: replacement, the frame's
//! drain step, and the lightmap renderer seam. See: context/lib/rendering_pipeline.md §4

use std::sync::Arc;

use anyhow::{Context, Result};
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
    /// storage is streaming. Either session changing identity or SH changing
    /// mode replaces both, because they share one issuer. `level` supplies id
    /// 46 (read only when an SH session is created), id 49 and id 51, all from
    /// the same load as `sh_manifest`.
    pub(crate) fn prepare_streaming_drains(
        &mut self,
        sh_manifest: Option<&Arc<ShStreamManifest>>,
        level: Option<&LevelWorld>,
        renderer: &Renderer,
        frame: StreamingFrame<'_>,
    ) -> Result<ShDrainBatch> {
        self.level_streaming.poll_retirement();
        let lightmap = level.and_then(LightmapLevelView::of);
        if sh_manifest.is_none() && lightmap.is_none() {
            self.clear_level_streaming();
            return Ok(ShDrainBatch::default());
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
        self.level_streaming.prepare_drains(
            &mut self.sh_streaming,
            lightmap.map(|view| view.residency_set),
            frame,
        )
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
