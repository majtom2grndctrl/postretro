//! Level install's streaming step: the level's streaming sessions, before
//! Settling drains the presented pose's set.
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

    /// Runs once the rest of level install has placed the camera: creates
    /// the level's streaming sessions. Nothing is read here; Settling's drains
    /// make the presented pose's set resident. A failure is fatal, as it is
    /// for the frame's streaming step.
    pub(super) fn install_level_streaming_sessions(&mut self) -> Result<()> {
        let (Some(world), Some(renderer), Some(session)) = (
            self.level.as_ref(),
            self.renderer.as_ref(),
            self.session.as_mut(),
        ) else {
            return Ok(());
        };
        session.install_level_streaming(world, renderer)?;
        self.level_timings.record("streaming_sessions");
        Ok(())
    }
}
