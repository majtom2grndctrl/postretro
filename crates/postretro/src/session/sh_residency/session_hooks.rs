//! `Session` entry points for SH streaming outcomes and compose submission.
//! See: context/lib/rendering_pipeline.md §4

use anyhow::{Result, bail};
use postretro_level_loader::ShDrainOutcome;
use postretro_renderer::Renderer;

impl super::super::Session {
    /// Applies an outcome before the app propagates a later renderer frame
    /// error. Legacy frames pass a default empty outcome safely.
    pub(crate) fn apply_sh_streaming_outcome(
        &mut self,
        outcome: ShDrainOutcome,
        renderer: &Renderer,
    ) -> Result<()> {
        let Some(streaming) = self.sh_streaming.as_mut() else {
            if outcome.accepted.is_empty()
                && outcome.dropped.is_empty()
                && outcome.deferred.is_empty()
                && outcome.evicted.is_empty()
            {
                return Ok(());
            }
            bail!(
                "[SH streaming] renderer returned a non-empty drain outcome without a session controller"
            );
        };
        streaming.apply_outcome(outcome, renderer)
    }

    /// Marks a windowed frame as having submitted its compose work. `false`
    /// deliberately leaves accepted installs uncomposed after an occluded or
    /// failed scene frame.
    pub(crate) fn mark_sh_streaming_compose_submitted(&mut self, submitted: bool) {
        if submitted && let Some(streaming) = self.sh_streaming.as_mut() {
            streaming.mark_compose_submitted();
        }
    }
}
