// Options-menu wiring between the modal stack, the `options.*` working copies
// and the session's options bridge.
// See: context/lib/player_options.md §4

use crate::*;

impl App {
    pub(crate) fn options_menu_is_top(&self) -> bool {
        self.session.as_ref().is_some_and(|session| {
            session.modal_stack.active_name() == Some(options::OPTIONS_MENU_TREE_NAME)
        })
    }

    pub(crate) fn seed_options_menu_slots(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let crate::session::Session {
            options_bridge,
            player_options,
            scripting,
            ..
        } = session;
        options_bridge.seed_on_open(
            &mut scripting.script_ctx.slot_table.borrow_mut(),
            player_options,
        );
    }

    /// Apply accepted option-slot writes after the frame's command drains.
    /// Closing flushes only after those writes settle, so a change and Back in
    /// the same frame cannot strand a pending value behind the debounce.
    pub(crate) fn update_player_options(&mut self, frame_dt: f32, options_menu_was_open: bool) {
        let effects = {
            let Some(session) = self.session.as_mut() else {
                return;
            };
            let crate::session::Session {
                options_bridge,
                player_options,
                input_system,
                settings_path,
                scripting,
                ..
            } = session;
            let mut slot_table = scripting.script_ctx.slot_table.borrow_mut();
            options_bridge.update(
                frame_dt,
                &mut slot_table,
                player_options,
                input_system,
                settings_path.as_deref(),
            )
        };

        if let Some(quality) = effects.fog_quality {
            self.apply_player_fog_quality(quality);
        }

        // Live: the renderer rewrites every installed material's uniform
        // buffer, so this takes effect on the next frame with no level reload
        // and is a safe no-op when no level (or no renderer) is present.
        if let Some(quality) = effects.surface_depth_quality {
            self.apply_player_surface_depth_quality(quality);
        }

        if options_menu_was_open && !self.options_menu_is_top() {
            let session = self
                .session
                .as_mut()
                .expect("options close requires an installed session");
            session
                .options_bridge
                .flush_on_options_close(&session.player_options, session.settings_path.as_deref());
        }
    }
}
