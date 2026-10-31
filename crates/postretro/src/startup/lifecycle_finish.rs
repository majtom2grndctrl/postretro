//! Finishing a delivered level load: install, the level's streaming sessions,
//! then Settling behind the loading tree. Production loads finish through
//! their bundle's commit (`lifecycle_commit.rs`); `finish_level_payload`, the
//! main-thread oracle's finish, stays for the tests that compare the two.
//! See: context/lib/boot_sequence.md §1 (Settling) · §3

use super::*;

impl App {
    /// The oracle: install a worker payload on the main thread, then Settling.
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
        // M15 Phase 3 (issue 3b): register the listen host's own boot pawn for
        // outbound replication now that the install has spawned + marked it the
        // local player. Reload-safe and a no-op off the host / on a map without a
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
