//! Level install's streaming step: the level's streaming sessions and the
//! spawn cell's lightmap preload, before the first level frame.
//! See: context/lib/boot_sequence.md §3 · context/lib/rendering_pipeline.md §4

use anyhow::Result;
use glam::Vec3;

use crate::App;

impl App {
    /// The followed local pawn's eye at install, `None` when no pawn is
    /// followed (a pawnless fly camera, a client awaiting its pawn). Install
    /// carries no reconcile history, so no presentation offset.
    pub(super) fn followed_pawn_eye(&self) -> Option<Vec3> {
        self.session.as_ref().and_then(|session| {
            crate::local_pawn_eye_position(
                &session.scripting.script_ctx.registry.borrow(),
                Vec3::ZERO,
            )
        })
    }

    /// Runs once the rest of level install (spawn and start pose included)
    /// has placed the camera, before the first level frame renders. A failure
    /// is fatal, as it is for the frame's streaming step.
    pub(super) fn install_spawn_streaming(&mut self) -> Result<()> {
        let spawn_eye = self.presented_pose().position;
        let (Some(world), Some(renderer), Some(session)) = (
            self.level.as_ref(),
            self.renderer.as_mut(),
            self.session.as_mut(),
        ) else {
            return Ok(());
        };
        session.install_level_streaming(world, renderer, spawn_eye)?;
        self.level_timings.record("streaming_preload");
        Ok(())
    }
}
