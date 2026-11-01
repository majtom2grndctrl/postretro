//! The tail a level load ends with, shared by the commit and the main-thread
//! test oracle (`finish_level_payload`): host pawn registration, then Settling.
//! See: context/lib/boot_sequence.md §1 (Settling) · §3

use super::*;

impl App {
    /// The oracle: install a parsed level payload on the main thread, then
    /// Settling. Production loads finish through the commit instead.
    #[cfg(test)]
    pub(super) fn finish_level_payload(
        &mut self,
        payload: crate::startup::worker::LevelPayload,
        event_loop: &impl LoopExit,
    ) {
        match payload.level {
            Some(world) => {
                if let Err(err) = self.install_level_payload(world, payload.prm_cache_root) {
                    self.finish_level_failure(err.to_string(), event_loop);
                    return;
                }
                // A level tree drawing a loading-only image keeps it loaded.
                self.promote_level_tree_images();
                // The level's streaming sessions exist before Settling asks
                // them anything, so a missing session means "not streamed".
                if let Err(err) = self.install_level_streaming_sessions() {
                    log::error!("[Loader] level streaming install failed: {err:#}");
                    self.exit_result = Err(err);
                    event_loop.exit();
                    return;
                }
                self.enter_installed_level_settling();
            }
            None => {
                self.finish_level_failure(
                    "worker delivered no level payload".to_string(),
                    event_loop,
                );
            }
        }
    }

    /// The tail every installed level ends with, on the main-thread install
    /// and a level load's commit alike, once its streaming sessions exist:
    /// the CPU timer, the host's own pawn, the load's end, and Settling.
    pub(super) fn enter_installed_level_settling(&mut self) {
        // The install frame never counts, and no CPU timing surface may
        // show a window from the previous level.
        self.cpu_timer.level_changed();
        // Register the listen host's own boot pawn for outbound replication
        // once the install has spawned and marked it the local player.
        // Reload-safe and a no-op off the host / on a map without a
        // player_spawn. The host pawn stays driven locally by `simulate_tick`.
        self.host_register_own_pawn_after_install();
        self.level_load = None;
        self.boot_load = false;
        // The level is held behind the loading tree, which stays
        // active, until its first frame's streamed set is resident.
        self.enter_settling(std::time::Instant::now());
        self.request_redraw();
    }
}
