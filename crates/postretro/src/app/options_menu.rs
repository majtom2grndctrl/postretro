// Options-menu wiring between the modal stack, the `options.*` working copies
// and the session's options bridge.
// See: context/lib/player_options.md §4

use crate::*;

impl App {
    /// Frame top: take the OS reader's latest readings. The bridge applies them
    /// to unset fields at its next update, the same frame (UO1).
    pub(crate) fn poll_os_preferences(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if let Some(readings) = session.os_preferences.poll() {
            session
                .options_bridge
                .set_os_preferences(options::OsPreferences {
                    reduce_motion: readings.reduce_motion,
                });
        }
    }

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
        self.refresh_window_modes();
    }

    /// Apply accepted option-slot writes after the frame's command drains.
    /// Closing flushes only after those writes settle, so a change and Back in
    /// the same frame cannot strand a pending value behind the debounce.
    pub(crate) fn update_player_options(&mut self, frame_dt: f32, options_menu_was_open: bool) {
        self.update_player_options_with_window_modes(
            frame_dt,
            options_menu_was_open,
            |app, mode| {
                if let Some(mode) = mode {
                    app.request_window_mode(mode);
                }
                app.service_window_modes();
            },
        );
    }

    /// Apply window effects through their adapter after the working-copy bridge.
    pub(crate) fn update_player_options_with_window_modes(
        &mut self,
        frame_dt: f32,
        options_menu_was_open: bool,
        apply_window_modes: impl FnOnce(&mut Self, Option<options::WindowMode>),
    ) {
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

        // UI actions on this tick have run, so keep wins over same-tick expiry.
        apply_window_modes(self, effects.window_mode);
        if let Some(quality) = effects.fog_quality {
            self.apply_player_fog_quality(quality);
        }

        if let Some(resolved) = effects.accessibility
            && let Some(session) = self.session.as_mut()
        {
            session
                .input_system
                .set_hold_timing_scale(resolved.hold_timing_scale);
            if let Some(audio) = session.audio.as_mut() {
                options::apply_to_audio(&resolved, audio);
            }
        }
        if let Some(swap) = effects.swap_confirm_cancel
            && let Some(session) = self.session.as_mut()
        {
            session.bindings.set_swap_confirm_cancel(swap);
        }

        // Live: the renderer rewrites every installed material's uniform
        // buffer, so this takes effect on the next frame with no level reload
        // and is a safe no-op when no level (or no renderer) is present.
        if let Some(quality) = effects.surface_depth_quality {
            self.apply_player_surface_depth_quality(quality);
        }

        // Live and record-only: the frame-start extent commit rebuilds once
        // from the final values, so a resize in the same frame costs no extra
        // rebuild.
        if let Some(resolution) = effects.render_resolution {
            self.apply_player_render_resolution(resolution);
        }

        // Any close path — close button or cancel — writes the
        // first-launch record and flushes a pending panel write.
        let panel_open = self.accessibility_panel_is_open();
        if self.accessibility_panel_was_open && !panel_open {
            self.note_accessibility_panel_closed();
        }
        self.accessibility_panel_was_open = panel_open;

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
