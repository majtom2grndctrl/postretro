// Host-only ghost-link commands applied at each authoritative fixed tick's start.
// See: context/lib/scripting.md §12 (Entity addressing) · context/lib/networking.md §Current contract

use postretro_entities::ScriptCtx;

impl super::GhostLinks {
    /// Apply every queued `arm`, `disarm`, `setMembers` and `setColorFilter`
    /// at the start of one authoritative fixed tick, in call order, at the
    /// tick's script time. A first filter on a field ramps from this peer's
    /// resolved style look. On a connected client the queue is dropped unread.
    pub(crate) fn apply_pending_commands(&self, ctx: &ScriptCtx) {
        postretro_sim::ghost_link_commands::apply_pending_ghost_link_commands(
            ctx,
            ctx.presentation_script_time.get(),
            |index| self.style_look(index),
        );
    }
}

impl crate::App {
    /// Host and single player, at each fixed tick's start: before anything in
    /// the tick runs, as teleporter changes apply, so the end-of-tick state
    /// history records reflects every step queued before this tick.
    pub(crate) fn apply_pending_ghost_link_commands(&self) {
        if let Some(session) = self.session.as_ref() {
            self.ghost_links
                .apply_pending_commands(&session.scripting.script_ctx);
        }
    }
}

#[cfg(test)]
#[path = "commands_tests.rs"]
mod tests;
