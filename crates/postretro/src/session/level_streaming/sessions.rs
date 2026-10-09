//! Which streaming sessions a level needs, their replacement, and the
//! renderer's lightmap drain result.
//! See: context/lib/rendering_pipeline.md §4

use crate::streaming::cell_demand::CellDemand;
use postretro_level_format::cell_residency_set::CellResidencySetSection;
use std::sync::Arc;

use anyhow::{Context, Result};
use postretro_level_format::cluster_directory::ClusterDirectorySection;
use postretro_level_loader::{
    LightmapDrainOutcome, LightmapStreamManifest, ShStreamManifest, ShStreamingMode,
};
use postretro_renderer::LightmapResidencyDrainError;

use super::LevelStreaming;
use crate::session::lightmap_residency::{
    LightmapLevelView, LightmapStreamingSession, RendererDrainFailure,
};
use crate::session::sh_residency::ShStreamingSession;
use crate::streaming::cluster_hints::{ClusterHints, decode_level_hints};

/// What the loaded level streams, compared against the live sessions.
#[derive(Debug, Clone, Copy)]
pub(crate) struct WantedStreaming<'a> {
    /// SH's manifest and the mode it streams in.
    pub(crate) sh: Option<(&'a Arc<ShStreamManifest>, ShStreamingMode)>,
    pub(crate) lightmap: Option<LightmapLevelView<'a>>,
    /// The level's id 49: both sessions share the hints decoded from it.
    pub(crate) cluster_directory: Option<&'a ClusterDirectorySection>,
    /// The level's id 51, read directly from the level so a level that
    /// streams SH alone, or declines its lightmap, still has its reach.
    pub(crate) residency_set: Option<&'a CellResidencySetSection>,
}

impl LevelStreaming {
    /// Makes the sessions match the loaded level. Returns whether anything
    /// streams; when nothing does, every session is released. `make_sh`
    /// builds a fresh SH session from its manifest, its mode, and the
    /// level's shared hints.
    ///
    /// A session whose identity changed is replaced, and the shared issuer
    /// with it. When only SH changed (its manifest or its mode), the lightmap
    /// session is kept, controller and resident blocks included: it
    /// re-attaches to the next issuer, and its reads in flight are read
    /// again. A fresh controller would know none of the blocks the
    /// renderer's pool still holds.
    pub(crate) fn ensure_sessions(
        &mut self,
        sh: &mut Option<ShStreamingSession>,
        wanted: WantedStreaming<'_>,
        make_sh: impl FnOnce(
            Arc<ShStreamManifest>,
            ShStreamingMode,
            Option<Arc<ClusterHints>>,
        ) -> Result<ShStreamingSession>,
    ) -> Result<bool> {
        let lightmap = wanted.lightmap.filter(|view| !self.declined(view.manifest));
        if wanted.sh.is_none() && lightmap.is_none() {
            self.clear(sh);
            return Ok(false);
        }
        let sh_current = match (wanted.sh, sh.as_ref()) {
            (Some((manifest, mode)), Some(streaming)) => streaming.is_for(manifest, mode),
            (None, None) => true,
            _ => false,
        };
        let lightmap_current = match (&lightmap, &self.lightmap) {
            (Some(view), Some(streaming)) => streaming.is_for(view.manifest),
            (None, None) => true,
            // A session parked by a mid-level decline stays for its level, so
            // the issuer SH reads through keeps running and SH keeps its
            // residency.
            (None, Some(streaming)) => {
                streaming.is_declined()
                    && wanted
                        .lightmap
                        .is_some_and(|view| streaming.is_for(view.manifest))
            }
            _ => false,
        };
        if sh_current && lightmap_current {
            return Ok(true);
        }
        // A declined lightmap is still the same level, so this compares the
        // manifest wanted before the decline filter.
        let level_changed = self.lightmap.as_ref().is_some_and(|streaming| {
            !wanted
                .lightmap
                .is_some_and(|view| streaming.is_for(view.manifest))
        }) || sh.as_ref().is_some_and(|streaming| {
            !wanted
                .sh
                .is_some_and(|(manifest, _)| streaming.is_for_manifest(manifest))
        });
        // Cancel the old I/O now. Its threads retire off the frame path; the
        // replacement issuer starts only after they have joined.
        if lightmap_current && self.lightmap.is_some() {
            self.retire_sh_keep_lightmap(sh);
        } else {
            self.retire(sh);
        }
        // Hints and the cell-demand stage belong to the level: a session
        // replaced within it keeps them, and a new manifest invalidates them.
        if level_changed {
            self.hints = None;
            self.cell_demand = None;
        }
        if self.cell_demand.is_none() {
            self.cell_demand = wanted
                .residency_set
                .map(|set| CellDemand::new(set.max_lead));
        }
        if self.hints.is_none() {
            // Tagged for the shared layer: the hints serve both resources.
            self.hints = decode_level_hints(wanted.cluster_directory)
                .context("[Streaming] id-49 cluster hints")?;
        }
        if let Some((manifest, mode)) = wanted.sh {
            *sh = Some(make_sh(Arc::clone(manifest), mode, self.hints.clone())?);
        }
        if let Some(view) = lightmap
            && self.lightmap.is_none()
        {
            self.install_lightmap(LightmapStreamingSession::new(view, self.hints.as_deref())?);
        }
        Ok(true)
    }

    /// Releases every session, the issuer, the level's hints, and its
    /// cell-demand stage: level unload, or a frame with nothing to stream.
    /// Threads that have already finished join at once; the rest retire off
    /// the frame path.
    pub(crate) fn clear(&mut self, sh: &mut Option<ShStreamingSession>) {
        self.retire(sh);
        self.hints = None;
        self.cell_demand = None;
        self.poll_retirement();
    }

    /// Retires SH's session and the issuer the two sessions share, keeping
    /// the lightmap session, which re-attaches to the next issuer.
    fn retire_sh_keep_lightmap(&mut self, sh: &mut Option<ShStreamingSession>) {
        let mut retirement = self.retirement.take().unwrap_or_default();
        if let Some(mut streaming) = sh.take()
            && let Some(workers) = streaming.begin_worker_retirement()
        {
            retirement.add_sh(workers);
        }
        if let Some(reads) = self.reads.take() {
            retirement.add_issuer(reads.begin_retirement());
            if let Some(lightmap) = self.lightmap.as_mut() {
                let completions = lightmap.detach_from_issuer();
                retirement.drain_lightmap_completions(completions, Arc::clone(lightmap.ledger()));
            }
        }
        if !retirement.is_empty() {
            self.retirement = Some(retirement);
        }
    }

    /// The level stops streaming its lightmap. The manifest is declined, so
    /// no later frame recreates its session. The caller logs why.
    ///
    /// SH is untouched: this may run between SH's drain batch and its
    /// outcome, and SH keeps its session and residency afterwards. Without a
    /// running issuer the session drops whole, its route with it. With one,
    /// the issuer's lightmap route delivers into the session's queue, and a
    /// closed queue would stop the issuer SH reads through too. The session
    /// is then parked for the rest of the level: untargeted, inert, draining
    /// its queue each frame, and given no route by a later issuer.
    pub(crate) fn decline_lightmap(&mut self) {
        let Some(lightmap) = self.lightmap.as_mut() else {
            return;
        };
        self.declined_lightmap = lightmap.manifest_identity();
        if self.reads.is_none() {
            self.lightmap = None;
        } else {
            lightmap.park_declined();
        }
    }

    fn declined(&self, manifest: &Arc<LightmapStreamManifest>) -> bool {
        self.declined_lightmap
            .as_ref()
            .is_some_and(|declined| std::ptr::eq(declined.as_ptr(), Arc::as_ptr(manifest)))
    }

    /// Applies the renderer's result for the parked lightmap batch. An
    /// outcome goes to the controller. An error returns the batch's pairs
    /// to the controller; then a renderer that does not stream the lightmap
    /// declines it for the level, a rolled-back drain carries on, and a
    /// drain-contract violation is fatal. A rollback that recurs on
    /// [`MAX_CONSECUTIVE_ROLLED_BACK_DRAINS`] drains in a row declines the
    /// lightmap too, with one error, rather than re-reading every pair
    /// forever. Declining leaves SH's session, and its pending outcome,
    /// alone.
    pub(crate) fn apply_lightmap_drain(
        &mut self,
        result: Result<LightmapDrainOutcome, LightmapResidencyDrainError>,
    ) -> Result<()> {
        let Some(lightmap) = self.lightmap_mut() else {
            return Ok(());
        };
        let error = match result {
            Ok(outcome) => return lightmap.apply_outcome(outcome),
            Err(error) => error,
        };
        match lightmap.renderer_drain_failed(&error) {
            RendererDrainFailure::NotStreaming => {
                warn_not_streaming();
                self.decline_lightmap();
                Ok(())
            }
            RendererDrainFailure::RolledBack => {
                if lightmap.consecutive_rolled_back_drains() >= MAX_CONSECUTIVE_ROLLED_BACK_DRAINS {
                    log::error!(
                        "[Lightmap streaming] {MAX_CONSECUTIVE_ROLLED_BACK_DRAINS} renderer drains \
                         in a row failed and were rolled back (last: {error}); lightmap \
                         streaming stops for this level, and blocks not yet resident render \
                         without static light"
                    );
                    self.decline_lightmap();
                }
                Ok(())
            }
            RendererDrainFailure::Contract => {
                Err(anyhow::Error::new(error).context("[Lightmap streaming] renderer drain"))
            }
        }
    }
}

/// Renderer drains that fail and roll back, in a row, before the level stops
/// streaming its lightmap. A transient failure clears within a drain or two;
/// eight in a row (about 0.13 s at 60 fps) means the failure recurs every
/// drain, and each one re-reads every pair it carried.
pub(crate) const MAX_CONSECUTIVE_ROLLED_BACK_DRAINS: u32 = 8;

fn warn_not_streaming() {
    log::warn!(
        "[Lightmap streaming] the renderer does not stream this level's lightmap; the level \
         renders with the placeholder lightmap"
    );
}
