//! First-launch hold: before any level loads, a profile that has never closed
//! the accessibility panel sees it alone on world-less frames.
//! See: context/lib/boot_sequence.md §1 (First-launch hold)

use postretro_ui::demo::ACCESSIBILITY_PANEL_NAME;

use crate::App;
use crate::startup::{BootDestination, BootState};

impl App {
    /// No stored record that the panel was ever closed. A settings file that
    /// cannot be replaced never takes the record, so there the panel shows on
    /// every launch — the fail-safe.
    pub(crate) fn first_launch_hold_required(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| !session.player_options.accessibility_panel_shown)
    }

    /// Clear the splash to a world-less frame showing only the panel. The hold
    /// drains no level requests (so a host's `Relevel` waits in the queue),
    /// keeps the transport alive and runs the options bridge, all through the
    /// frontend frame path; no level runs and no sound plays until it ends.
    pub(crate) fn enter_first_launch_hold(&mut self, destination: BootDestination) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.clear_splash();
        }
        let Some(session) = self.session.as_mut() else {
            self.start_boot_destination(destination);
            return;
        };
        session.modal_stack.clear_pushed();
        session
            .modal_stack
            .push_named(ACCESSIBILITY_PANEL_NAME, None);
        if session.modal_stack.active_name() != Some(ACCESSIBILITY_PANEL_NAME) {
            log::warn!("[UI] accessibility panel unavailable; skipping the first-launch hold");
            self.start_boot_destination(destination);
            return;
        }
        self.boot_destination = Some(destination);
        self.boot_state = BootState::FirstLaunchHold;
        self.accessibility_panel_was_open = true;
        log::info!("[Engine] first launch: showing the accessibility panel before any level loads");
    }

    /// End the hold once the panel has closed by any close path; boot then
    /// continues as on any launch. The close itself wrote the record.
    pub(crate) fn finish_first_launch_hold_if_closed(&mut self) {
        if self.boot_state != BootState::FirstLaunchHold || self.accessibility_panel_is_open() {
            return;
        }
        let destination = self
            .boot_destination
            .take()
            .unwrap_or(BootDestination::Frontend);
        self.start_boot_destination(destination);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use postretro_foundation::ModMapEntry;
    use postretro_scripting_core::runtime::{Frontend, MenuCamera};
    use postretro_ui::demo::{ACCESSIBILITY_PANEL_NAME, build_accessibility_panel_descriptor};
    use postretro_ui::modal_stack::ScopeTier;

    use crate::App;
    use crate::startup::lifecycle::tests::test_app;
    use crate::startup::{BootDestination, BootState, LevelRequest, LevelSource};

    fn map(id: &str) -> ModMapEntry {
        ModMapEntry {
            id: id.to_string(),
            path: format!("maps/{id}.prl"),
            name: id.to_string(),
            tags: Vec::new(),
        }
    }

    /// A first-launch profile with a frontend that declares a backdrop and a
    /// catalog holding the backdrop and a host's map.
    fn first_launch_app() -> App {
        let mut app = test_app();
        app.boot_state = BootState::Splash;
        let session = app.session.as_mut().unwrap();
        session.player_options.accessibility_panel_shown = false;
        session.modal_stack.registry_mut().register(
            ACCESSIBILITY_PANEL_NAME,
            build_accessibility_panel_descriptor(),
            ScopeTier::Engine,
            false,
        );
        session.modal_stack.registry_mut().register(
            postretro_ui::demo::FRONTEND_MENU_NAME,
            postretro_ui::demo::build_frontend_menu_descriptor(),
            ScopeTier::Engine,
            false,
        );
        session.frontend = Some(Frontend {
            menu_tree: postretro_ui::demo::FRONTEND_MENU_NAME.to_string(),
            background_level: Some("backdrop".to_string()),
            camera: MenuCamera {
                position: [0.0, 0.0, 0.0],
                yaw: 0.0,
                pitch: 0.0,
            },
        });
        session
            .scripting
            .script_ctx
            .data_registry
            .borrow_mut()
            .replace_maps(vec![map("backdrop"), map("host_map")]);
        app
    }

    fn loading_catalog_id(app: &App) -> Option<String> {
        app.level_load
            .as_ref()
            .and_then(|load| load.entry.catalog_id.clone())
    }

    fn stop_worker(app: &mut App) {
        app.level_rx = None;
        if let Some(handle) = app.level_worker.take() {
            let _ = handle.join();
        }
        app.level_load = None;
    }

    fn close_panel(app: &mut App) {
        app.session.as_mut().unwrap().modal_stack.pop();
        app.update_player_options(0.0, false);
        app.finish_first_launch_hold_if_closed();
    }

    #[test]
    fn a_profile_that_closed_the_panel_skips_the_hold() {
        let mut app = first_launch_app();
        assert!(app.first_launch_hold_required());
        app.session
            .as_mut()
            .unwrap()
            .player_options
            .accessibility_panel_shown = true;
        assert!(!app.first_launch_hold_required());
    }

    #[test]
    fn the_hold_shows_only_the_panel_and_starts_no_load_until_it_closes() {
        let mut app = first_launch_app();
        app.enter_first_launch_hold(BootDestination::Frontend);
        assert_eq!(app.boot_state, BootState::FirstLaunchHold);
        let stack = &app.session.as_ref().unwrap().modal_stack;
        assert_eq!(stack.active_name(), Some(ACCESSIBILITY_PANEL_NAME));
        assert_eq!(stack.len(), 1);
        assert!(app.level_requests.is_empty(), "no backdrop is requested");
        assert!(app.level_load.is_none());

        // Closing ends the hold: the record is written and boot continues to
        // the frontend, whose backdrop loads.
        close_panel(&mut app);
        assert!(
            app.session
                .as_ref()
                .unwrap()
                .player_options
                .accessibility_panel_shown
        );
        assert_eq!(loading_catalog_id(&app).as_deref(), Some("backdrop"));
        stop_worker(&mut app);
    }

    #[test]
    fn a_host_map_named_during_the_hold_outranks_the_cli_map_and_the_backdrop() {
        // P2: a `--connect` client launched with a CLI map; the host names a
        // different map while the hold is up.
        for destination in [
            BootDestination::BootMap(PathBuf::from("content/dev/maps/cli.prl")),
            BootDestination::Frontend,
        ] {
            let mut app = first_launch_app();
            app.enter_first_launch_hold(destination.clone());
            app.follow_relevel_catalog("host_map".to_string());
            assert!(
                app.level_load.is_none(),
                "the hold drains no level requests"
            );
            assert_eq!(
                app.level_requests,
                [LevelRequest::Load(LevelSource::Catalog(
                    "host_map".to_string()
                ))]
            );

            close_panel(&mut app);
            assert_eq!(
                loading_catalog_id(&app).as_deref(),
                Some("host_map"),
                "{destination:?}"
            );
            assert!(!app.boot_load, "the host's map is not the CLI boot map");
            stop_worker(&mut app);
        }
    }

    #[test]
    fn a_panel_write_at_the_hold_persists_before_the_panel_closes() {
        // UO7: the field saves after the 250 ms settle with the panel open; the
        // record waits for the close, so a quit here shows the panel again.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mut app = first_launch_app();
        app.session.as_mut().unwrap().settings_path = Some(path.clone());
        app.enter_first_launch_hold(BootDestination::Frontend);
        app.update_player_options(0.0, false);
        app.apply_accessibility_field_action("cycle", "monoAudio");
        for _ in 0..4 {
            app.update_player_options(0.1, false);
        }
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("mono_audio = true"), "{saved}");
        assert!(!saved.contains("accessibility_panel_shown"), "{saved}");
        assert_eq!(app.boot_state, BootState::FirstLaunchHold);
    }

    #[test]
    fn a_host_map_named_during_the_splash_wait_outranks_the_backdrop_on_any_launch() {
        let mut app = first_launch_app();
        app.session
            .as_mut()
            .unwrap()
            .player_options
            .accessibility_panel_shown = true;
        // Splash frames poll the transport while waiting for the OS reader.
        app.follow_relevel_catalog("host_map".to_string());
        app.start_boot_destination(BootDestination::Frontend);
        assert_eq!(loading_catalog_id(&app).as_deref(), Some("host_map"));
        stop_worker(&mut app);
    }
}
