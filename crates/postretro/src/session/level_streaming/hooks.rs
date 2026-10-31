//! `Session` entry points for level-scope streaming: replacement, the frame's
//! drain step, and the lightmap renderer seam.
//! See: context/lib/rendering_pipeline.md §4

use std::sync::Arc;

use anyhow::{Context, Result};
use postretro_level_loader::{
    LevelWorld, ShDrainBatch, ShStreamManifest, requested_streaming_mode,
};
use postretro_renderer::{Renderer, ShResidencySnapshot};
use postretro_stage_timing::StageFrame;

use super::{StreamingFrame, StreamingLevelData, WantedStreaming};
use crate::cpu_timing::StreamingStage;
use crate::session::lightmap_residency::{LightmapLevelView, LightmapStreamingSession};
use crate::session::sh_residency::{ShStreamingSession, require_loaded_streaming_mode};
use crate::streaming::cluster_hints::ClusterHints;

impl crate::session::Session {
    /// Releases every streaming session, the level's issuer, and their
    /// manifest clones: at level unload, and on every legacy frame or
    /// world-less frame that holds no level (never Settling's held frames).
    /// A following streamed level always gets fresh nonzero generations,
    /// even when its content bytes match the prior map.
    pub(crate) fn clear_level_streaming(&mut self) {
        self.level_streaming.clear(&mut self.sh_streaming);
    }

    /// Creates or replaces the level's streaming sessions, updates them from
    /// this frame's visibility, and returns SH's one bounded pre-compose
    /// batch. The lightmap batch waits in its session for
    /// [`Self::drain_lightmap_streaming`].
    ///
    /// SH streams when `sh_manifest` is present; lightmaps when the level's
    /// storage is streaming. `level` supplies id 49 and id 51, both from the
    /// same load as `sh_manifest`.
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
        // Id 51 straight from the level: SH reads the reach whether or not
        // the lightmap streams.
        let residency_set = level.and_then(|world| world.cell_residency_set.as_ref());
        self.level_streaming
            .prepare_drains(&mut self.sh_streaming, residency_set, frame)
    }

    /// The frame's whole streaming step, shared by every frame that holds a
    /// level: [`Self::prepare_streaming_drains`], then
    /// [`Self::drain_lightmap_streaming`]. Returns SH's batch for the
    /// renderer's pre-compose drain. A Running frame draws what it made
    /// resident; a held frame (Settling) composes it without drawing.
    pub(crate) fn run_level_streaming_step(
        &mut self,
        sh_manifest: Option<&Arc<ShStreamManifest>>,
        level: Option<&LevelWorld>,
        renderer: &mut Renderer,
        frame: StreamingFrame<'_>,
    ) -> Result<ShDrainBatch> {
        let cpu = frame.cpu;
        let now_seconds = frame.monotonic_seconds;
        let batch = self.prepare_streaming_drains(sh_manifest, level, renderer, frame)?;
        // The lightmap drain runs before the frame records its passes, so
        // the frame samples what it made resident.
        self.drain_lightmap_streaming(renderer, cpu, now_seconds)?;
        Ok(batch)
    }

    /// Level install: creates the level's streaming sessions, so the settle
    /// chokepoint reads a missing session as "not streamed". Reads nothing:
    /// Settling's drains make the presented pose's set resident.
    ///
    /// `data` is the level's decoded streaming data
    /// ([`super::decode_streaming_level_data`] on this `level`), so every
    /// level-data check has already passed; what can fail here is the
    /// renderer's residency snapshot of the published set, the budget it
    /// yields, and the generation clocks, all in
    /// [`prepare_level_streaming`], before any session changes.
    // Reached only from the main-thread oracle's streaming install.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn install_level_streaming(
        &mut self,
        level: &LevelWorld,
        renderer: &Renderer,
        data: StreamingLevelData,
    ) -> Result<()> {
        let prepared = prepare_level_streaming(level, renderer.sh_residency_snapshot(), data)?;
        self.install_prepared_level_streaming(level, prepared);
        Ok(())
    }

    /// Level install's second half: replace every streaming session with the
    /// ones [`prepare_level_streaming`] built for `level`. Fails on nothing:
    /// whatever session creation can fail on already ran there.
    pub(crate) fn install_prepared_level_streaming(
        &mut self,
        level: &LevelWorld,
        prepared: PreparedLevelStreaming,
    ) {
        self.level_streaming.poll_retirement();
        self.level_streaming.install_prepared(
            &mut self.sh_streaming,
            level.cell_residency_set.as_ref(),
            prepared,
        );
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
        self.level_streaming.ensure_sessions(
            &mut self.sh_streaming,
            WantedStreaming {
                sh,
                lightmap: level.and_then(LightmapLevelView::of),
                cluster_directory: level.and_then(LevelWorld::cluster_directory),
                residency_set: level.and_then(|world| world.cell_residency_set.as_ref()),
            },
            |manifest, mode, hints| {
                let hints = hints.context("[SH streaming] a streamed level carries no id 49")?;
                ShStreamingSession::from_renderer_with_mode(manifest, renderer, mode, hints)
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

/// A level's streaming sessions, built for the level an install is about to
/// make current and not yet installed: SH's session over the residency
/// snapshot of the renderer set that holds the level, the lightmap session
/// over its block map, and the id-49 hints both share. Building them is
/// everything session creation can fail on; installing them
/// ([`crate::session::Session::install_prepared_level_streaming`]) fails on
/// nothing.
#[derive(Debug, Default)]
pub(crate) struct PreparedLevelStreaming {
    pub(super) hints: Option<Arc<ClusterHints>>,
    pub(super) sh: Option<ShStreamingSession>,
    pub(super) lightmap: Option<LightmapStreamingSession>,
}

/// Build `level`'s streaming sessions from its decoded streaming data (`data`,
/// from this `level`, every level-data check passed) and `snapshot`, the SH
/// residency snapshot of the renderer set built for it, published or not.
///
/// Fails only on what is not level data, each before anything else is
/// built: no snapshot for a streamed-SH level, the SH controller refusing
/// the budget the snapshot yields or its topology, a generation clock, and
/// decoded data that is not this level's. Touches no session state, so a
/// failure leaves every running session as it was; it may only have spent
/// a controller generation, which no session ever reuses. A level that
/// streams nothing builds nothing.
pub(crate) fn prepare_level_streaming(
    level: &LevelWorld,
    snapshot: Option<ShResidencySnapshot>,
    data: StreamingLevelData,
) -> Result<PreparedLevelStreaming> {
    let StreamingLevelData {
        hints,
        sh,
        lightmap,
    } = data;
    let sh = match sh {
        Some(sh) => {
            anyhow::ensure!(
                level
                    .sh_stream_manifest()
                    .is_some_and(|manifest| Arc::ptr_eq(manifest, &sh.manifest)),
                "[SH streaming] the decoded streaming data is not this level's"
            );
            Some(ShStreamingSession::from_snapshot_with_topology(
                sh.manifest,
                snapshot,
                sh.mode,
                sh.topology,
            )?)
        }
        None => None,
    };
    let lightmap = match lightmap {
        Some(data) => {
            let view = LightmapLevelView::of(level)
                .filter(|view| Arc::ptr_eq(view.manifest, &data.manifest))
                .context("[Lightmap streaming] the decoded streaming data is not this level's")?;
            Some(LightmapStreamingSession::with_block_map(view, data.blocks)?)
        }
        None => None,
    };
    Ok(PreparedLevelStreaming {
        hints,
        sh,
        lightmap,
    })
}
