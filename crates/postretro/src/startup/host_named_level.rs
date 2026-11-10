//! A connected client's host-named level: one it loads because its host's
//! relevel requested it, or a load of it in flight that a relevel adopted.
//! Its install clears every frontend menu layer; a failed one, or a relevel
//! naming an id the client lacks, puts the menu back with no backdrop load.
//! See: context/lib/boot_sequence.md §1 (Presented pose) · context/lib/networking.md §Slot lifecycle

use crate::App;
use crate::startup::{BootState, LevelRequest, LevelSource};

impl App {
    /// Mark a load of `catalog_id` already in flight or queued as host-named,
    /// returning whether one was. Either installs a fresh life, so a relevel
    /// never restarts it; adopting it makes its install the host's level.
    pub(super) fn adopt_selected_relevel_load(&mut self, catalog_id: &str) -> bool {
        if let Some(load) = self
            .level_load
            .as_mut()
            .filter(|load| load.entry.catalog_id.as_deref() == Some(catalog_id))
        {
            load.host_named = true;
            return true;
        }
        let mut adopted = false;
        for request in &mut self.level_requests {
            match request {
                LevelRequest::HostLevel(id) if id == catalog_id => adopted = true,
                LevelRequest::Load(LevelSource::Catalog(id)) if id == catalog_id => {
                    *request = LevelRequest::HostLevel(catalog_id.to_string());
                    adopted = true;
                }
                _ => {}
            }
        }
        adopted
    }

    /// A host-named install is the host's level, never a backdrop: clear the
    /// frontend menu root and every layer over it before the presented pose
    /// resolves, so the stand-in (the matching hint, else the first spawn) is
    /// what Settling streams from and the reveal presents.
    pub(super) fn clear_frontend_menu_for_host_level(&mut self) {
        if !self.frontend_menu_is_present() {
            return;
        }
        if let Some(session) = self.session.as_mut() {
            session.modal_stack.clear_pushed();
        }
        self.reconcile_ui_focus();
        log::info!("[Net] installing the host's level: frontend menu cleared");
    }

    /// The host named a catalog id this client lacks, so the host-named level
    /// it was following (installed, loading or queued) is stale: it unloads,
    /// and the frontend menu is presented with no backdrop load. A level that
    /// is not host-named (the client's own backdrop, a level it booted) stays.
    /// Before boot clears, no level exists yet: a queued host level is
    /// dropped, and the boot-destination step presents the menu instead of
    /// populating the frontend.
    pub(super) fn leave_stale_host_level(&mut self) {
        if matches!(
            self.boot_state,
            BootState::Splash | BootState::FirstLaunchHold
        ) {
            self.level_requests
                .retain(|request| !matches!(request, LevelRequest::HostLevel(_)));
            self.boot_host_level_missing = true;
            return;
        }
        let queued = self
            .level_requests
            .iter()
            .any(|request| matches!(request, LevelRequest::HostLevel(_)));
        let loading = self.level_load.as_ref().is_some_and(|load| load.host_named);
        let installed = self.active_level_host_named && self.level_is_installed_state();
        if !(queued || loading || installed) {
            return;
        }
        self.level_requests
            .retain(|request| !matches!(request, LevelRequest::HostLevel(_)));
        self.enqueue_level_request(LevelRequest::Unload);
        self.present_frontend_menu();
    }
}
