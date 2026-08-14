//! Splash-state boot frame driving: black/logo schedule, decode + upload
//! handoff, deferred mod init, and the boot-map / frontend transition.
//! See: context/lib/boot_sequence.md §1 (Splash state machine)

use winit::event_loop::ActiveEventLoop;

use crate::App;
use crate::render;
use crate::scripting::state_persistence::{
    load_persisted_state, overlay_persisted_state, state_path,
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
    pub(super) fn run_splash_frame(&mut self, event_loop: &ActiveEventLoop, frame_dt: f32) -> bool {
        match self.splash_frame {
            0 => self.run_splash_frame_zero(event_loop, frame_dt),
            1 => self.run_splash_frame_one(event_loop, frame_dt),
            _ => {
                self.boot_state = BootState::Loading;
                self.run_loading_frame(event_loop, frame_dt)
            }
        }
    }

    /// First Splash frame: paint the splash-color clear (no logo bound yet), then
    /// decode + upload the base splash so a subsequent frame can show it. The
    /// splash texture is not yet decoded, so the splash pass clears to
    /// `SPLASH_CLEAR_COLOR` and draws nothing.
    ///
    /// `first_black_frame` is recorded — and the decode/upload runs — only after
    /// this frame actually presents. The window is visible, so this presented
    /// frame is the splash-color clear the user sees after a brief
    /// pre-first-present white flash on Windows (a known cosmetic artifact — see
    /// `window_attributes` for why a hidden-window suppression was reverted). A
    /// transient surface failure requests another redraw WITHOUT advancing the
    /// schedule, so the timing marks a real presented frame.
    fn run_splash_frame_zero(&mut self, event_loop: &ActiveEventLoop, frame_dt: f32) -> bool {
        if !self.paint_splash(event_loop) {
            // Resume keeps the session endpoint alive. Surface acquisition may
            // fail for an unbounded number of redraws, so transport still advances
            // before retrying this same splash frame.
            let _ = self.poll_world_less_transport(frame_dt);
            self.request_redraw();
            return false;
        }
        // A resumed session can already own an endpoint while Splash replays
        // frame zero. Poll only after a successful paint so pixels-first
        // scheduling remains unchanged.
        let _ = self.poll_world_less_transport(frame_dt);
        self.boot_timings.record("first_black_frame");

        // Now that the OS window is showing a splash-color frame, decode and upload the
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
    fn run_splash_frame_one(&mut self, event_loop: &ActiveEventLoop, frame_dt: f32) -> bool {
        // Run deferred mod init + the boot transition only after the splash
        // (logo) frame actually presents — a transient surface failure just
        // re-requests the redraw, holding the schedule on frame 1.
        if !self.paint_splash_after_black(event_loop) {
            let _ = self.poll_world_less_transport(frame_dt);
            self.request_redraw();
            return false;
        }
        // On resume the session (and its endpoint) survives while Splash frame
        // one replays. Normal first boot has no endpoint yet, so this is a no-op.
        let _ = self.poll_world_less_transport(frame_dt);
        // First pixels are now on screen (black frame 0, logo frame 1). Build +
        // install the whole `Session` (options, audio, scripting core,
        // input/UI/modal group, net endpoint) ahead of the renderer full-init
        // check (and the mod-init / frontend / loading transitions that follow),
        // so a build failure exits boot before any later step runs against a
        // `None` session. Session install is `Option::take`-guarded single-commit;
        // audio + net are built inside it once. Mirrors
        // `finish_renderer_full_init`'s early return.
        // See: context/lib/boot_sequence.md §1.
        if !self.install_pending_session(event_loop) {
            return false;
        }
        // Lazy-init the dev-tools debug UI now that the session exists and the
        // renderer/window are ready. Rebuilds on resume (which drops it), so a
        // suspend/resume re-entering this frame restores it without re-running the
        // single-commit session install. See: context/lib/boot_sequence.md §1, §5.
        self.ensure_debug_ui();
        // The session is installed with `InputFocus::Gameplay`; capture the
        // cursor now (the work `resumed` used to do pre-install, deferred here
        // since focus is session-owned). A capturing frontend tree releases it
        // again on the first `reconcile_ui_focus`.
        self.set_input_focus(crate::input::InputFocus::Gameplay);

        // Full renderer initialization runs after the first visible logo frame
        // and completes BEFORE the splash clears and before any Frontend /
        // Loading-completion / Running / UI / scene path executes — AND before
        // `run_deferred_mod_init`, whose mod-theme / mod-font install drains
        // (`set_ui_theme` / `register_ui_font`) are full-ready renderer paths
        // that touch `Renderer::full` and panic if it is not yet built
        // (renderer_splash.rs full-ready guard). Session build is CPU-side state
        // and stays ahead of this; only the full-ready-dependent mod-init step
        // had to move behind it (boot_sequence §1, rendering_pipeline §7.8).
        // Idempotent: a suspend→resume that recreated the surface re-runs this
        // without re-running deferred session init. A hard failure here is a
        // renderer init failure — exit non-zero.
        if !self.finish_renderer_full_init(event_loop) {
            return false;
        }
        if !self.run_deferred_mod_init(event_loop) {
            return false;
        }
        self.swap_mod_splash_override_if_pending();
        log::info!("{}", self.mod_timings.summary());

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
    /// presents; on a transient surface failure it returns false so the caller
    /// holds the schedule and re-requests a redraw.
    fn paint_splash_after_black(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if !self.paint_splash(event_loop) {
            return false;
        }
        self.boot_timings.record("first_splash_frame");
        log::info!("{}", self.boot_timings.summary());
        self.mod_timings = StartupTimings::new();
        true
    }

    /// Run `mod_init` and commit its validated manifest into the engine-global
    /// `DataRegistry`, install the manifest's UI theme/fonts and bloom render
    /// profile, overlay persisted state once, and start the hot-reload watcher.
    /// Records `mod_init` into `mod_timings`. A mod-init failure is fatal: a
    /// running engine with rejected declarations would otherwise silently boot
    /// without the content that owns its durable state.
    ///
    /// This is also the resume seam: suspend drops the renderer, and the resumed
    /// splash loop replays this after `finish_renderer_full_init`, so the
    /// recreated renderer re-receives the committed profile.
    fn run_deferred_mod_init(&mut self, event_loop: &ActiveEventLoop) -> bool {
        // Mod init runs before the worker spawns so declarations and entity
        // descriptors commit together, then persistence overlays defaults once
        // before any level work begins.
        // The script runtime + context now live on `Session` (built earlier this
        // frame by `install_pending_session`). Borrow the session for the manifest
        // drain; theme/font install needs `&mut self`, so the manifest's theme/font
        // payload is lifted into locals and applied after the session borrow ends.
        let script_root = self.content_root.join("scripts");
        let content_root = self.content_root.clone();
        let mut deferred_theme_fonts: Option<(
            postretro_foundation::ModThemeTokens,
            postretro_foundation::ModFontAssets,
        )> = None;
        // Same deferral as theme/fonts and `frontend`: the renderer setter needs
        // `&mut self`, which the session borrow below forbids. A failed mod init
        // leaves this `None` and therefore makes NO setter call, so the renderer
        // keeps its active profile.
        let committed_render_profile: postretro_scripting_core::runtime::ModRenderProfile;
        // Switching is App-owned input policy, so lift it out of the runtime
        // manifest before the registry drain mutably borrows the session.
        let committed_switching: postretro_foundation::SwitchingDescriptor;
        let committed_mover_auto_close_ms: f32;
        {
            let session = self
                .session
                .as_mut()
                .expect("session installed before mod init");
            session
                .scripting
                .script_runtime
                .compile_stale_scripts(&script_root, &content_root);
            if let Err(err) = session.scripting.script_runtime.run_mod_init(&content_root) {
                log::error!("[Scripting] mod_init failed: {err}");
                self.exit_result = Err(anyhow::anyhow!("mod_init failed: {err}"));
                event_loop.exit();
                return false;
            } else {
                let has_manifest = session.scripting.script_runtime.mod_manifest().is_some();
                // Read the render profile before anything drains or `take`s the
                // manifest below — `ModRenderProfile` is `Copy`, so this costs a
                // read and cannot be invalidated by the later mutations. A
                // successful init with no start script commits the default,
                // matching the staged `NoStartScript` rule.
                committed_render_profile = session
                    .scripting
                    .script_runtime
                    .mod_manifest()
                    .map(|manifest| manifest.render)
                    .unwrap_or_default();
                committed_switching = session
                    .scripting
                    .script_runtime
                    .mod_manifest()
                    .map(|manifest| manifest.switching)
                    .unwrap_or_default();
                committed_mover_auto_close_ms = session
                    .scripting
                    .script_runtime
                    .mod_manifest()
                    .map(|manifest| manifest.movers.auto_close_ms)
                    .unwrap_or(crate::runtime_movers::ENGINE_AUTO_CLOSE_MS);
                // Drain the manifest's engine-global `DataRegistry` registrations
                // (entity types, maps, global reactions/crossings) through the
                // shared extractor also used by the headless observability path, so
                // the two cannot drift. See: context/lib/boot_sequence.md §3.
                session.scripting.drain_manifest_registrations();
                // `frontend` is session-owned now; the `manifest` borrow below
                // aliases `session.scripting.script_runtime`, so lift the committed frontend
                // into a local and assign `session.frontend` after that borrow ends
                // (mirroring the theme/font deferral).
                let mut committed_frontend: Option<
                    Option<postretro_scripting_core::runtime::Frontend>,
                > = None;
                if let Some(manifest) = session.scripting.script_runtime.mod_manifest_mut() {
                    // UI trees / theme / fonts / frontend are windowed-only surfaces
                    // (headless has no modal stack), so they stay here rather than in
                    // the shared drain above. Register mod-scope UI trees into the
                    // tiered registry at `Mod` tier, before the mod-init VM drops.
                    session.modal_stack.register_script_trees(
                        std::mem::take(&mut manifest.ui_trees),
                        postretro_ui::modal_stack::ScopeTier::Mod,
                    );

                    committed_frontend = Some(manifest.frontend.take());
                    let mod_theme = std::mem::take(&mut manifest.theme);
                    let mod_fonts = std::mem::take(&mut manifest.fonts);
                    deferred_theme_fonts = Some((mod_theme, mod_fonts));
                }
                // The `manifest` borrow has ended; commit the frontend onto the
                // session.
                if let Some(frontend) = committed_frontend {
                    session.frontend = frontend;
                }

                if session
                    .state_store_lifecycle
                    .should_restore_after_mod_init(has_manifest)
                {
                    let mod_id = session
                        .scripting
                        .script_runtime
                        .committed_mod_identity()
                        .map(|(id, _)| id.to_owned())
                        .expect("restore only runs after a committed manifest");
                    let identity = session.scripting.script_runtime.store_identity().cloned();
                    let committed_store_slots = session
                        .scripting
                        .script_runtime
                        .committed_store_slots()
                        .clone();
                    if let Some(state_path) = state_path(&mod_id) {
                        match load_persisted_state(&state_path) {
                            Ok(Some(persisted)) => {
                                let warnings = overlay_persisted_state(
                                    &mut session.scripting.script_ctx.slot_table.borrow_mut(),
                                    &persisted,
                                    identity.as_ref(),
                                    &committed_store_slots,
                                );
                                for warning in warnings {
                                    log::warn!("[State] {warning}");
                                }
                                session.persisted_state = Some(persisted);
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
                    } else if session.state_store_lifecycle.disable_persistence() {
                        log::warn!(
                            "[State] platform data directory is unavailable; persistent state is disabled for this run"
                        );
                    }
                    session.state_store_lifecycle.mark_restore_completed();
                }
            }
            // Hot-reload watcher (debug-only); release builds no-op.
            if let Err(err) = session
                .scripting
                .script_runtime
                .start_watcher(&script_root, &content_root)
            {
                log::error!("[Scripting] start_watcher failed: {err}");
            }
        }
        // Theme/font install borrows `&mut self` (renderer, etc.), so it runs after
        // the session borrow above has ended.
        if let Some((mod_theme, mod_fonts)) = deferred_theme_fonts {
            self.install_mod_ui_theme_and_fonts(mod_theme, mod_fonts);
        }
        self.apply_mod_bloom_render_profile(committed_render_profile);
        self.switching = committed_switching;
        if let Some(session) = self.session.as_mut() {
            session.scripting.mover_auto_close_ms = committed_mover_auto_close_ms;
        }
        // Admission identity is frozen by the scripting runtime; the digest is
        // recomputed from the committed registry each time this deferred init runs.
        self.install_network_mod_content();
        self.mod_timings.record("mod_init");
        true
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
