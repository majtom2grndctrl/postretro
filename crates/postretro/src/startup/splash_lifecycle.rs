//! Splash-state boot frame driving: black/logo schedule, decode + upload
//! handoff, deferred mod init, and the boot-map / frontend transition.
//! See: context/lib/boot_sequence.md §1 (Splash state machine)

use std::path::Path;

use winit::event_loop::ActiveEventLoop;

use crate::App;
use crate::render;
use crate::render::splash_pass::PresentOutcome;
use crate::scripting::reaction_dispatch::validate_scoped_sequence_primitives;
use crate::scripting::state_persistence::{
    STATE_FILE_PATH, load_persisted_state, overlay_persisted_state,
};
use crate::startup::{BootState, LevelRequest, LevelSource, SplashSource, StartupTimings};

impl App {
    /// Drive one Splash-state frame. Returns `false` when the splash frame was
    /// painted and the redraw should otherwise short-circuit. A boot map exits
    /// Splash by enqueueing a load request and entering `Loading`.
    ///
    /// Frame schedule:
    /// - frame 0: paint a black frame (no splash bound). After present:
    ///   record `first_black_frame`; decode the base PNG synchronously;
    ///   upload + bind it; record `splash_decoded` / `splash_uploaded`.
    ///   (Source is always `Base` until the mod system ships.)
    /// - frame 1: paint splash (now visible). After paint: record
    ///   `first_splash_frame`; emit log line A; run `mod_init`; optionally
    ///   swap splash on override; emit log line B; enqueue boot load or enter
    ///   Frontend when no map was supplied.
    pub(super) fn run_splash_frame(&mut self, event_loop: &ActiveEventLoop) -> bool {
        match self.splash_frame {
            0 => self.run_splash_frame_zero(event_loop),
            1 => self.run_splash_frame_one(event_loop),
            _ => {
                self.boot_state = BootState::Loading;
                self.run_loading_frame(event_loop)
            }
        }
    }

    /// First Splash frame: paint a black screen, then decode + upload the base
    /// splash so a subsequent frame can show it. The splash texture is not yet
    /// decoded, so the splash pass clears to black and draws nothing.
    ///
    /// `first_black_frame` is recorded — and the decode/upload runs — only after
    /// the black frame actually presents. A transient surface failure requests
    /// another redraw WITHOUT advancing the schedule, so the timing marks a real
    /// presented frame.
    fn run_splash_frame_zero(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if self.paint_splash(event_loop) == PresentOutcome::NeedsRedraw {
            self.request_redraw();
            return false;
        }
        self.boot_timings.record("first_black_frame");

        // Now that the OS window is showing a black frame, decode and upload the
        // splash synchronously. PNG decode is bounded CPU work (~ms); doing it
        // here keeps the boot path single-threaded and ordering causal.
        let source = SplashSource::Base;
        match render::splash::load_splash(&source) {
            Ok(loaded) => {
                self.boot_timings.record("splash_decoded");
                if let Some(renderer) = self.renderer.as_mut() {
                    let dims = renderer.install_splash_pixels(&loaded);
                    log::info!("[Engine] Splash loaded: {}×{}", dims[0], dims[1]);
                }
                self.boot_timings.record("splash_uploaded");
            }
            Err(err) => {
                // Missing base splash is a packaging bug; record both stages so
                // log line A always lists the same set of stage names regardless
                // of success/failure. Subsequent splash frames stay black.
                self.boot_timings.record("splash_decoded");
                self.boot_timings.record("splash_uploaded");
                log::warn!("[Engine] failed to decode base splash: {err:#}");
            }
        }

        self.splash_frame += 1;
        self.request_redraw();
        false
    }

    /// Second Splash frame: paint the splash so the user sees it before mod
    /// scripts touch the engine, then run the deferred mod init and exit Splash
    /// — to Loading with a boot map, or Frontend without one.
    fn run_splash_frame_one(&mut self, event_loop: &ActiveEventLoop) -> bool {
        // Run deferred mod init + the boot transition only after the splash
        // (logo) frame actually presents — a transient surface failure just
        // re-requests the redraw, holding the schedule on frame 1.
        if self.paint_splash_after_black(event_loop) == PresentOutcome::NeedsRedraw {
            self.request_redraw();
            return false;
        }
        // First pixels are now on screen (black frame 0, logo frame 1). Finish
        // deferred session startup before any mod-supplied or net-dependent work
        // runs. Net-endpoint setup is `Option::take`-guarded single-commit;
        // audio + dev debug-UI rebuild whenever absent (suspend drops them), so a
        // suspend/resume re-entering this frame restores them without re-running
        // net init. See: context/lib/boot_sequence.md §1, §9.
        self.install_post_splash_services();
        self.install_pending_session();
        self.run_deferred_mod_init();
        self.swap_mod_splash_override_if_pending();
        log::info!("{}", self.mod_timings.summary());

        // Full renderer initialization runs after the first visible logo frame
        // and completes BEFORE the splash clears and before any Frontend /
        // Loading-completion / Running / UI / scene path executes (boot_sequence
        // §1, rendering_pipeline §7.8). Idempotent: a suspend→resume that recreated
        // the surface re-runs this without re-running deferred session init. A
        // hard failure here is a renderer init failure — exit non-zero.
        if !self.finish_renderer_full_init(event_loop) {
            return false;
        }

        let Some(map_path) = self.map_path.clone() else {
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.clear_splash();
            }
            self.boot_state = BootState::Frontend;
            self.populate_frontend();
            self.drain_level_requests();
            self.splash_frame += 1;
            // Final boot summary: the post-logo marks (session/audio/net/full-init)
            // append after the `first_splash_frame` line, so this logs the full
            // auditable boot order in one place. See: boot_sequence §1.
            log::info!("{}", self.boot_timings.summary());
            log::info!("[Engine] no boot map supplied; entering frontend");
            self.request_redraw();
            return false;
        };

        // Route boot-map loading through the same request queue runtime
        // transitions use. PRL parse still runs off the main thread, and
        // `Loading` keeps painting while it waits. The boot worker dispatch is
        // recorded into `boot_timings` so the boot order line proves first
        // pixels precede the level-worker spawn. See: boot_sequence §1.
        self.boot_load = true;
        self.boot_timings.record("boot_worker_dispatch");
        self.enqueue_level_request(LevelRequest::Load(LevelSource::Path(map_path)));
        self.boot_state = BootState::Loading;
        self.drain_level_requests();

        self.splash_frame += 1;
        // Final boot summary with the full mark set (see the no-map branch above).
        log::info!("{}", self.boot_timings.summary());
        self.request_redraw();
        false
    }

    /// Complete full renderer initialization (idempotent / restartable across
    /// surface recreation). Returns `true` on success or when no renderer is
    /// present (nothing to finish); on a hard renderer-init failure it stores the
    /// error, exits the event loop, and returns `false`. Records
    /// `renderer_full_init_complete` into `boot_timings`.
    ///
    /// Called once per boot after the logo frame presents, and again on resume if
    /// the surface was recreated — the renderer's `ensure_full_ready` no-ops when
    /// already full-ready, so the steady boot path pays nothing on re-entry.
    fn finish_renderer_full_init(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let Some(renderer) = self.renderer.as_mut() else {
            return true;
        };
        if let Err(err) = renderer.ensure_full_ready() {
            self.exit_result = Err(err);
            event_loop.exit();
            return false;
        }
        self.boot_timings.record("renderer_full_init_complete");
        true
    }

    /// Paint the now-decoded splash and emit log line A. Records
    /// `first_splash_frame` and resets `mod_timings` only after the logo frame
    /// presents; on a transient surface failure it returns `NeedsRedraw` so the
    /// caller holds the schedule and re-requests a redraw.
    fn paint_splash_after_black(&mut self, event_loop: &ActiveEventLoop) -> PresentOutcome {
        let outcome = self.paint_splash(event_loop);
        if outcome == PresentOutcome::NeedsRedraw {
            return outcome;
        }
        self.boot_timings.record("first_splash_frame");
        log::info!("{}", self.boot_timings.summary());
        self.mod_timings = StartupTimings::new();
        outcome
    }

    /// Run `mod_init` and commit its validated manifest into the engine-global
    /// `DataRegistry`, overlay persisted state once, and start the hot-reload
    /// watcher. Records `mod_init` into `mod_timings`. Errors log and leave the
    /// engine in a blank-mod state so the splash flow still completes.
    fn run_deferred_mod_init(&mut self) {
        // Mod init runs before the worker spawns so declarations and entity
        // descriptors commit together, then persistence overlays defaults once
        // before any level work begins.
        let script_root = self.content_root.join("scripts");
        self.script_runtime
            .compile_stale_scripts(&script_root, &self.content_root);
        if let Err(err) = self.script_runtime.run_mod_init(&self.content_root) {
            log::error!("[Scripting] mod_init failed: {err}");
        } else {
            let has_manifest = self.script_runtime.mod_manifest().is_some();
            if let Some(manifest) = self.script_runtime.mod_manifest_mut() {
                // Drain entity-type descriptors from the validated mod manifest
                // into the engine-global `DataRegistry`. Runtime parses; caller
                // owns lifecycle. See: context/lib/boot_sequence.md §3.
                let mut data_registry = self.script_ctx.data_registry.borrow_mut();
                for desc in std::mem::take(&mut manifest.entities) {
                    data_registry.upsert_entity_type(desc);
                }
                data_registry.replace_maps(std::mem::take(&mut manifest.maps));
                let global_reactions = validate_scoped_sequence_primitives(
                    std::mem::take(&mut manifest.reactions),
                    &self.sequence_registry,
                );
                data_registry.replace_global_reactions(global_reactions);
                data_registry.replace_global_crossings(std::mem::take(&mut manifest.crossings));
                drop(data_registry);

                // Register mod-scope UI trees into the tiered registry at `Mod`
                // tier, before the mod-init VM context drops.
                self.modal_stack.register_script_trees(
                    std::mem::take(&mut manifest.ui_trees),
                    render::ui::modal_stack::ScopeTier::Mod,
                );

                self.frontend = manifest.frontend.take();
                let mod_theme = std::mem::take(&mut manifest.theme);
                let mod_fonts = std::mem::take(&mut manifest.fonts);
                self.install_mod_ui_theme_and_fonts(mod_theme, mod_fonts);
            }

            if self
                .state_store_lifecycle
                .should_restore_after_mod_init(has_manifest)
            {
                let state_path = Path::new(STATE_FILE_PATH);
                match load_persisted_state(state_path) {
                    Ok(Some(persisted)) => {
                        let warnings = overlay_persisted_state(
                            &mut self.script_ctx.slot_table.borrow_mut(),
                            &persisted,
                        );
                        for warning in warnings {
                            log::warn!("[State] {warning}");
                        }
                        log::info!(
                            "[State] restored persistent slots from {}",
                            state_path.display()
                        );
                    }
                    Ok(None) => {}
                    Err(error) => log::warn!(
                        "[State] failed to load persistent slots from {}: {error}; using declared defaults",
                        state_path.display()
                    ),
                }
                self.state_store_lifecycle.mark_restore_completed();
            }
        }
        // Hot-reload watcher (debug-only); release builds no-op.
        if let Err(err) = self
            .script_runtime
            .start_watcher(&script_root, &self.content_root)
        {
            log::error!("[Scripting] start_watcher failed: {err}");
        }
        self.mod_timings.record("mod_init");
    }

    /// Swap the splash texture if a mod override was staged. Mod-side override
    /// wiring lands with the mod system; today `pending_splash_override` is
    /// always `None`, so this is a no-op. The branch is here so the flow is
    /// complete the moment the hook arrives.
    fn swap_mod_splash_override_if_pending(&mut self) {
        if let Some(source) = self.pending_splash_override.take() {
            match render::splash::load_splash(&source) {
                Ok(loaded) => {
                    if let Some(renderer) = self.renderer.as_mut() {
                        let dims = renderer.install_splash_pixels(&loaded);
                        log::info!("[Engine] Mod splash loaded: {}×{}", dims[0], dims[1]);
                    }
                    self.mod_timings.record("mod_splash_swap");
                }
                Err(err) => {
                    log::error!("[Engine] mod splash override failed: {err:#}");
                }
            }
        }
    }
}
