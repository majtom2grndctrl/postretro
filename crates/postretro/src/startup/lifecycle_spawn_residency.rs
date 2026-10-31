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

    /// Creates the level's streaming sessions at install. Nothing is read
    /// here; Settling's drains make the presented pose's set resident. A
    /// failure is fatal, as it is for the frame's streaming step.
    ///
    /// The level-data checks run first, through the same
    /// `decode_streaming_level_data` load prep runs off the main thread; the
    /// sessions are then built from what it decoded.
    pub(super) fn install_level_streaming_sessions(&mut self) -> Result<()> {
        let (Some(world), Some(renderer), Some(session)) = (
            self.level.as_ref(),
            self.renderer.as_ref(),
            self.session.as_mut(),
        ) else {
            return Ok(());
        };
        let data = crate::session::level_streaming::decode_streaming_level_data(
            crate::session::level_streaming::StreamingLevelParts::of(world),
            || Ok(postretro_level_loader::requested_streaming_mode()?),
        )?;
        session.install_level_streaming(world, renderer, data)?;
        self.level_timings.record("streaming_sessions");
        Ok(())
    }

    /// A level load's commit's streaming step: install the sessions its
    /// checks built against the level's set before the commit mutated
    /// anything (`prepare_level_streaming`). Fails on nothing. `None` for a
    /// renderer-less commit, which, like the main-thread install without a
    /// renderer, creates no session.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn install_prepared_level_streaming_sessions(
        &mut self,
        prepared: Option<crate::session::level_streaming::PreparedLevelStreaming>,
    ) {
        let (Some(prepared), Some(world), Some(session)) =
            (prepared, self.level.as_ref(), self.session.as_mut())
        else {
            return;
        };
        session.install_prepared_level_streaming(world, prepared);
        self.level_timings.record("streaming_sessions");
    }
}
