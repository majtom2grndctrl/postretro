//! Boot-state dispatch for each redraw and the runtime level-request queue.
//! See: context/lib/boot_sequence.md §1

#[cfg(feature = "dev-tools")]
use std::path::PathBuf;

use winit::event_loop::ActiveEventLoop;

use crate::App;
use crate::startup::{BootState, LevelRequest, LevelSource};

#[cfg(feature = "dev-tools")]
const DEV_LEVEL_CYCLE_TARGET: &str = "content/dev/maps/combat-demo.prl";

impl App {
    pub(crate) fn initial_boot_state() -> BootState {
        BootState::Booting
    }

    pub(crate) fn enter_splash_state(&mut self) {
        self.boot_state = BootState::Splash;
    }

    pub(crate) fn reset_boot_state_after_suspend(&mut self) {
        // Reset the boot state so `resumed()` re-runs window + renderer
        // creation. Without this, the `Booting` guard in `resumed()` would
        // no-op and the engine would stay permanently renderer-less.
        self.boot_state = BootState::Booting;
        self.splash_frame = 0;
        self.os_wait_from = None;
        self.pending_level_log = false;
        self.level_load = None;
        self.active_level_tags.clear();
        self.active_level_source = None;
        self.level_requests.clear();
        self.boot_load = false;
        self.boot_destination = None;
    }

    pub(crate) fn drive_boot_state_for_redraw(
        &mut self,
        event_loop: &ActiveEventLoop,
        frame_dt: f32,
    ) -> bool {
        if matches!(
            self.boot_state,
            BootState::Loading | BootState::Frontend | BootState::Running
        ) {
            self.drain_level_requests();
        }

        // Splash and Loading frames draw no UI, so UI input that reached them
        // is dropped here rather than delivered to the first frame that does.
        if matches!(
            self.boot_state,
            BootState::Booting | BootState::Splash | BootState::Loading
        ) {
            self.drop_ui_input_on_non_ui_frame();
        }

        match self.boot_state {
            BootState::Booting => {
                // A `RedrawRequested` queued before `resumed()` (or after
                // `suspended()` resets boot_state back to `Booting`) can
                // legally arrive here. Drop it silently — `resumed()` will
                // rebuild and request a fresh redraw.
                false
            }
            BootState::Splash => self.run_splash_frame(event_loop, frame_dt),
            BootState::Loading => self.run_loading_frame(event_loop, frame_dt),
            BootState::Frontend | BootState::FirstLaunchHold => {
                // No level is installed. Let the normal redraw handler render a
                // frontend-safe frame that skips gameplay/world work.
                true
            }
            BootState::Running => {
                // Steady state — fall through to the normal frame loop.
                true
            }
        }
    }

    pub(crate) fn enqueue_level_request(&mut self, request: LevelRequest) {
        if self.boot_state == BootState::Loading && self.level_load_in_flight() && self.boot_load {
            log::warn!(
                "[Loader] ignoring runtime lifecycle request while boot map load is in flight"
            );
            return;
        }

        match &request {
            LevelRequest::Load(_) => {
                self.level_requests
                    .retain(|queued| !matches!(queued, LevelRequest::Load(_)));
            }
            LevelRequest::Unload => {
                if self
                    .level_requests
                    .iter()
                    .any(|queued| matches!(queued, LevelRequest::Unload))
                {
                    return;
                }
            }
        }
        self.level_requests.push_back(request);
    }

    /// Follow a server-selected catalog level through the ordinary runtime
    /// request path. A catalog mismatch is recoverable content divergence, not
    /// a transport failure: leave the connection alive for a later relevel.
    pub(crate) fn follow_relevel_catalog(&mut self, catalog_id: String) {
        let catalog_has_id = self.session.as_ref().is_some_and(|session| {
            session
                .scripting
                .script_ctx
                .data_registry
                .borrow()
                .maps
                .iter()
                .any(|entry| entry.id == catalog_id)
        });
        if !catalog_has_id {
            log::warn!("[Net] relevel names unknown catalog id `{catalog_id}`");
            return;
        }

        if self.relevel_is_already_selected(&catalog_id) {
            return;
        }

        self.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog(catalog_id)));
    }

    fn relevel_is_already_selected(&self, catalog_id: &str) -> bool {
        let source_is_catalog =
            |source: &LevelSource| matches!(source, LevelSource::Catalog(id) if id == catalog_id);
        self.active_level_source.as_ref().is_some_and(source_is_catalog)
            || self
                .level_load
                .as_ref()
                .is_some_and(|load| load.entry.catalog_id.as_deref() == Some(catalog_id))
            || self.level_requests.iter().any(|request| {
                matches!(request, LevelRequest::Load(source) if source_is_catalog(source))
            })
    }

    #[cfg(feature = "dev-tools")]
    pub(crate) fn enqueue_dev_level_cycle(&mut self) {
        self.enqueue_dev_level_cycle_target(PathBuf::from(DEV_LEVEL_CYCLE_TARGET));
    }

    #[cfg(feature = "dev-tools")]
    pub(super) fn enqueue_dev_level_cycle_target(&mut self, target: PathBuf) {
        if self.boot_state == BootState::Loading && self.level_load_in_flight() {
            log::info!("[Loader] dev level lifecycle cycle ignored while level load is in flight");
            return;
        }

        if !target.is_file() {
            log::warn!(
                "[Loader] dev level lifecycle cycle ignored: target does not exist: {}",
                target.display()
            );
            return;
        }

        self.enqueue_level_request(LevelRequest::Unload);
        let target_display = target.display().to_string();
        self.enqueue_level_request(LevelRequest::Load(LevelSource::Path(target)));
        log::info!("[Loader] queued dev level lifecycle cycle: {target_display}");
    }

    pub(in crate::startup) fn drain_level_requests(&mut self) {
        if self.boot_state == BootState::Loading && self.level_load_in_flight() {
            return;
        }

        while let Some(request) = self.level_requests.pop_front() {
            match request {
                LevelRequest::Load(source) => {
                    let Some(load) = self.resolve_level_source(source) else {
                        continue;
                    };
                    if self.boot_state == BootState::Running {
                        self.unload_level();
                    }
                    self.begin_level_load(load);
                    return;
                }
                LevelRequest::Unload => {
                    if self.boot_state == BootState::Running {
                        self.unload_level();
                    }
                }
            }
        }
    }

    pub(super) fn level_load_in_flight(&self) -> bool {
        self.level_rx.is_some() || self.level_worker.is_some()
    }
}
