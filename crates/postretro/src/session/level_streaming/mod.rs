//! Level-scope streaming: one read issuer and one drain step for SH and lightmaps.
//! See: context/lib/rendering_pipeline.md §4

mod hooks;
mod io;
mod sessions;
pub(crate) mod settle;
#[cfg(test)]
mod tests;

use std::sync::{Arc, Weak};

use anyhow::Result;
use postretro_level_format::cell_residency_set::CellResidencySetSection;
use postretro_level_loader::{LightmapStreamManifest, ShDrainBatch};
use postretro_stage_timing::StageFrame;
use postretro_visibility::{VisibilityPath, VisibleCells};

pub(crate) use io::{LevelReadIssuer, StreamingRetirement};
pub(crate) use sessions::WantedStreaming;

use super::lightmap_residency::LightmapStreamingSession;
use super::sh_residency::ShStreamingSession;
use crate::cpu_timing::StreamingStage;
use crate::lightmap_streaming::demand::DemandFrame;
use crate::streaming::cluster_hints::ClusterHints;
use crate::streaming::shared_drain::SharedDrain;

/// One frame's visibility, as every streamed resource reads it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamingFrame<'a> {
    pub(crate) visible_cells: &'a VisibleCells,
    /// The locator's camera cell; `None` without a level.
    pub(crate) camera_cell: Option<usize>,
    pub(crate) path: VisibilityPath,
    pub(crate) monotonic_seconds: f64,
    /// A Settling frame: the level is held behind the loading tree until its
    /// settle set is resident.
    pub(crate) settling: bool,
    /// This frame's streaming CPU stages; the binary folds them under
    /// `render_prep`.
    pub(crate) cpu: &'a StageFrame<StreamingStage>,
}

/// Streaming state whose lifetime is one loaded level: the lightmap session,
/// the one issuer both resources read through, the level's id-49 hints
/// decoded once for both, a cancelled predecessor's retirement, and the
/// reused merged drain. SH's session stays on `Session` (it is read by
/// diagnostics and outcome hooks) and is passed in; this owner retires the
/// issuer whenever either session is replaced, so no session outlives the
/// issuer it reads through.
#[derive(Debug, Default)]
pub(crate) struct LevelStreaming {
    lightmap: Option<LightmapStreamingSession>,
    reads: Option<LevelReadIssuer>,
    retirement: Option<StreamingRetirement>,
    drain: SharedDrain,
    /// The sessions' level's id 49, decoded once for every resource.
    hints: Option<Arc<ClusterHints>>,
    /// A lightmap the renderer does not stream (it fell back to the
    /// placeholder). Its level runs without lightmap streaming. Weak, so it
    /// keeps the manifest's allocation, never the manifest.
    declined_lightmap: Option<Weak<LightmapStreamManifest>>,
}

impl LevelStreaming {
    /// The level's lightmap session; `None` once the level declined it, even
    /// while the declined session stays parked for the rest of the level.
    pub(crate) fn lightmap(&self) -> Option<&LightmapStreamingSession> {
        self.lightmap
            .as_ref()
            .filter(|session| !session.is_declined())
    }

    pub(crate) fn lightmap_mut(&mut self) -> Option<&mut LightmapStreamingSession> {
        self.lightmap
            .as_mut()
            .filter(|session| !session.is_declined())
    }

    /// Installs a fresh lightmap session. Call only after [`Self::retire`],
    /// so the previous generation's I/O is already cancelled.
    pub(crate) fn install_lightmap(&mut self, session: LightmapStreamingSession) {
        debug_assert!(
            self.lightmap.is_none() && self.reads.is_none(),
            "a lightmap session is installed only after retirement"
        );
        self.lightmap = Some(session);
    }

    /// Joins a predecessor's threads once they have all finished.
    pub(crate) fn poll_retirement(&mut self) {
        if self
            .retirement
            .as_mut()
            .is_some_and(StreamingRetirement::try_finish)
        {
            self.retirement = None;
        }
    }

    /// Cancels this level's I/O without joining: SH's workers, the lightmap
    /// session, and the issuer, in that order, so the issuer's last handle
    /// drops last. Their threads retire off the frame path; the next
    /// generation's issuer starts only after they have joined. Releases the
    /// sessions' manifest clones once those threads are gone.
    pub(crate) fn retire(&mut self, sh: &mut Option<ShStreamingSession>) {
        let mut retirement = self.retirement.take().unwrap_or_default();
        if let Some(mut streaming) = sh.take()
            && let Some(workers) = streaming.begin_worker_retirement()
        {
            retirement.add_sh(workers);
        }
        if let Some(lightmap) = self.lightmap.take() {
            retirement.retain_lightmap(lightmap.into_retained_source());
        }
        if let Some(reads) = self.reads.take() {
            retirement.add_issuer(reads.begin_retirement());
        }
        if !retirement.is_empty() {
            self.retirement = Some(retirement);
        }
    }

    /// One frame's streaming work for both resources:
    ///
    /// 1. start this level's issuer if no predecessor is still retiring;
    /// 2. SH and lightmaps each update demand, admit completions, and offer
    ///    ready items to one merged list;
    /// 3. the list is admitted once against the shared per-drain budget;
    /// 4. each takes its admitted prefix into its batch and submits reads.
    ///
    /// Returns SH's batch; the lightmap batch waits in its session for the
    /// renderer's lightmap drain. `residency_set` is the level's id 51, present
    /// whenever a lightmap session is.
    pub(crate) fn prepare_drains(
        &mut self,
        sh: &mut Option<ShStreamingSession>,
        residency_set: Option<&CellResidencySetSection>,
        frame: StreamingFrame<'_>,
    ) -> Result<ShDrainBatch> {
        self.poll_retirement();
        self.start_reads(sh)?;
        self.drain.begin();
        let sh_pending = match sh.as_mut() {
            Some(streaming) => Some(streaming.begin_drain(
                frame.visible_cells,
                frame.camera_cell,
                frame.monotonic_seconds,
                &mut self.drain,
            )?),
            None => None,
        };
        let lightmap_frame =
            residency_set
                .zip(frame.camera_cell)
                .map(|(residency_set, camera_cell)| DemandFrame {
                    residency_set,
                    camera_cell: camera_cell as u32,
                    path: frame.path,
                    visible_cells: frame.visible_cells,
                });
        if let Some(parked) = self
            .lightmap
            .as_mut()
            .filter(|session| session.is_declined())
        {
            parked.drain_declined_completions();
        }
        let lightmap = self
            .lightmap
            .as_mut()
            .filter(|session| !session.is_declined());
        let lightmap_drains = match (lightmap, lightmap_frame) {
            (Some(lightmap), Some(lightmap_frame)) => {
                let _scope = frame.cpu.scope(StreamingStage::LightmapResidency);
                lightmap.begin_drain(lightmap_frame, frame.settling, &mut self.drain)?;
                true
            }
            _ => false,
        };
        self.drain.admit()?;
        let batch = match (sh.as_mut(), sh_pending) {
            (Some(streaming), Some(pending)) => streaming.finish_drain(pending, &self.drain)?,
            _ => ShDrainBatch::default(),
        };
        if lightmap_drains && let Some(lightmap) = self.lightmap.as_mut() {
            let _scope = frame.cpu.scope(StreamingStage::LightmapResidency);
            lightmap.finish_drain(
                &self.drain,
                self.reads.as_ref().map(LevelReadIssuer::issuer),
            )?;
        }
        Ok(batch)
    }

    /// Spawns the level's one issuer over every route that needs it (SH's in
    /// async mode, the lightmap session's), once no predecessor is retiring.
    fn start_reads(&mut self, sh: &mut Option<ShStreamingSession>) -> Result<()> {
        if self.reads.is_some() || self.retirement.is_some() {
            return Ok(());
        }
        let sh_prepared = match sh.as_ref() {
            Some(streaming) => streaming.prepare_async_workers()?,
            None => None,
        };
        // A declined session reads nothing: the new issuer gets no route for it.
        let lightmap_route = self
            .lightmap
            .as_mut()
            .filter(|session| !session.is_declined())
            .and_then(LightmapStreamingSession::take_route);
        if sh_prepared.is_none() && lightmap_route.is_none() {
            return Ok(());
        }
        let (sh_workers, sh_route) = sh_prepared.unzip();
        let reads = LevelReadIssuer::spawn(sh_route, lightmap_route)?;
        if let (Some(mut workers), Some(streaming)) = (sh_workers, sh.as_mut()) {
            workers.attach_issuer(reads.issuer().clone());
            streaming.attach_workers(workers);
        }
        self.reads = Some(reads);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn is_retiring(&self) -> bool {
        self.retirement.is_some()
    }

    #[cfg(test)]
    pub(crate) fn drain_capacity(&self) -> usize {
        self.drain.capacity()
    }
}
