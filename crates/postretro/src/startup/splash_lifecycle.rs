//! Splash-state boot frame driving: black/logo schedule, decode + upload
//! handoff, deferred mod init, and the boot-map / frontend transition.
//! See: context/lib/boot_sequence.md §1 (Splash state machine)

use std::time::Instant;

use winit::event_loop::ActiveEventLoop;

use crate::App;
use crate::render;
use crate::scripting::state_persistence::{
    load_persisted_state, overlay_persisted_faction_sentiment, overlay_persisted_state,
    persisted_state_version_is_supported, state_path,
};
use crate::startup::{BootState, LevelRequest, LevelSource, SplashSource, StartupTimings};

/// Splash frames 0 and 1 are the black and logo frames; this one repeats while
/// boot waits for the OS preference reader.
const SPLASH_FRAME_OS_WAIT: u32 = 2;

/// Where boot goes once the splash clears, directly or after the first-launch
/// hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum BootDestination {
    Frontend,
    BootMap(std::path::PathBuf),
}

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
    ///   swap splash on override; emit log line B; then wait for the OS
    ///   preference reader (below).
    /// - frame 2 and on while waiting: repaint the splash until the OS reader
    ///   replies or its wait expires, then enqueue the boot load or enter
    ///   Frontend when no map was supplied.
    pub(super) fn run_splash_frame(&mut self, event_loop: &ActiveEventLoop, frame_dt: f32) -> bool {
        match self.splash_frame {
            0 => self.run_splash_frame_zero(event_loop, frame_dt),
            1 => self.run_splash_frame_one(event_loop, frame_dt),
            SPLASH_FRAME_OS_WAIT => self.run_splash_os_wait_frame(event_loop, frame_dt),
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
        match render::splash::load_splash(&source, &self.core_root) {
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

        // The OS reader's wait counts from here, so a slow mod init still gets
        // its full wait. A reply already in adds no frames.
        self.os_wait_from = Some(Instant::now());
        self.splash_frame = SPLASH_FRAME_OS_WAIT;
        self.leave_splash_when_os_replied(event_loop)
    }

    /// A splash frame held for the OS reader's first reply: repaint so frames
    /// keep presenting, keep the transport alive, then leave the splash once
    /// the reply is in or the wait has expired.
    fn run_splash_os_wait_frame(&mut self, event_loop: &ActiveEventLoop, frame_dt: f32) -> bool {
        let painted = self.paint_splash(event_loop);
        let _ = self.poll_world_less_transport(frame_dt);
        if !painted {
            self.request_redraw();
            return false;
        }
        self.leave_splash_when_os_replied(event_loop)
    }

    /// Poll the OS reader, then report whether its wait gate is satisfied (a
    /// reply is in, or the deadline has passed). Split out from
    /// `leave_splash_when_os_replied` so the poll-before-read fix is
    /// testable without an `ActiveEventLoop`.
    ///
    /// The frame-top `poll_os_preferences` call (main.rs) no-ops on the logo
    /// frame: the session it needs is installed later that same frame, by
    /// `run_splash_frame_one` above, after that call already ran. Polling
    /// again here picks up a reply already sitting in the channel before the
    /// gate reads it — otherwise it costs a needless `SPLASH_FRAME_OS_WAIT`
    /// frame waiting for next frame's top-of-loop poll to see it.
    fn poll_os_wait_gate(&mut self) -> bool {
        self.poll_os_preferences();
        let replied = self
            .session
            .as_ref()
            .is_some_and(|session| session.os_preferences.has_replied());
        let mod_init_finished = self.os_wait_from.unwrap_or_else(Instant::now);
        crate::os_preferences::os_wait_complete(replied, mod_init_finished, Instant::now())
    }

    fn leave_splash_when_os_replied(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if !self.poll_os_wait_gate() {
            self.request_redraw();
            return false;
        }
        let replied = self
            .session
            .as_ref()
            .is_some_and(|session| session.os_preferences.has_replied());
        if !replied {
            log::info!(
                "[Options] no OS preference reply within {:?}; a later reply applies live",
                crate::os_preferences::OS_REPLY_WAIT
            );
        }
        self.os_wait_from = None;
        // Apply the reply before anything draws, so the first frame after the
        // splash shows OS-seeded values.
        self.update_player_options(0.0, false);
        self.leave_splash(event_loop)
    }

    /// Exit Splash: to the first-launch hold when the accessibility panel has
    /// never been closed on this profile, else straight to the boot
    /// destination (Frontend with no boot map, Loading with one).
    fn leave_splash(&mut self, _event_loop: &ActiveEventLoop) -> bool {
        let destination = match self.map_path.clone() {
            Some(map_path) => BootDestination::BootMap(map_path),
            None => BootDestination::Frontend,
        };
        self.splash_frame += 1;
        if self.first_launch_hold_required() {
            self.enter_first_launch_hold(destination);
        } else {
            self.start_boot_destination(destination);
        }
        // Final boot summary: the post-logo marks (session/audio/net/full-init,
        // and the boot-map worker dispatch) append after the
        // `first_splash_frame` line, so this logs the full auditable boot order
        // in one place. See: boot_sequence §1.
        log::info!("{}", self.boot_timings.summary());
        self.request_redraw();
        false
    }

    /// Leave boot for its destination. A level the host named — during the
    /// splash's OS-preference wait or the first-launch hold — is already queued
    /// and outranks both the frontend backdrop and a CLI boot map.
    pub(crate) fn start_boot_destination(&mut self, destination: BootDestination) {
        let host_load_queued = self
            .level_requests
            .iter()
            .any(|request| matches!(request, LevelRequest::Load(_)));
        match destination {
            BootDestination::Frontend => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.clear_splash();
                }
                self.boot_state = BootState::Frontend;
                if host_load_queued {
                    self.present_frontend_menu();
                } else {
                    self.populate_frontend();
                }
                self.drain_level_requests();
                log::info!("[Engine] no boot map supplied; entering frontend");
            }
            BootDestination::BootMap(_) if host_load_queued => {
                // The host's map replaces the CLI boot map; it loads as an
                // ordinary runtime request, so a failure returns to Frontend.
                // Present the menu first (mirrors the Frontend arm above): a
                // worker failure or an unresolved catalog id leaves Frontend
                // with no level, and the first-launch hold left the modal
                // stack empty, so without this the player would be stranded
                // on a blank frame. `drain_level_requests` overwrites
                // `boot_state` to `Loading` when the load is actually
                // dispatched, so the splash pass (which Loading still paints)
                // is left untouched here.
                self.boot_state = BootState::Frontend;
                self.present_frontend_menu();
                self.drain_level_requests();
                log::info!("[Engine] loading the host's map instead of the CLI boot map");
            }
            BootDestination::BootMap(map_path) => {
                // Route boot-map loading through the same request queue runtime
                // transitions use. PRL parse still runs off the main thread, and
                // `Loading` keeps painting while it waits. The boot worker
                // dispatch is recorded into `boot_timings` so the boot order line
                // proves first pixels precede the level-worker spawn. See:
                // boot_sequence §1.
                self.boot_load = true;
                self.boot_timings.record("boot_worker_dispatch");
                self.enqueue_level_request(LevelRequest::Load(LevelSource::Path(map_path)));
                self.boot_state = BootState::Loading;
                self.drain_level_requests();
            }
        }
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
        let (shadow_quality, fog_quality, surface_depth_quality) = self
            .session
            .as_ref()
            .map(|session| {
                (
                    session.player_options.shadow_quality,
                    session.player_options.fog_quality,
                    session.player_options.surface_depth_quality,
                )
            })
            .unwrap_or_default();
        self.configure_player_shadow_quality(shadow_quality);
        // Retained in renderer boot state BEFORE `ensure_full_ready`, so the
        // placeholder material this build creates already carries the player's
        // tier and no rewrite is needed to catch it up.
        self.apply_player_surface_depth_quality(surface_depth_quality);
        let Some(renderer) = self.renderer.as_mut() else {
            return true;
        };
        if let Err(err) = renderer.ensure_full_ready() {
            self.exit_result = Err(err);
            event_loop.exit();
            return false;
        }
        self.apply_player_fog_quality(fog_quality);
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
        let committed_presentation_templates: Vec<
            postretro_scripting_core::data_descriptors::PresentationTemplate,
        >;
        // Switching is App-owned input policy, so lift it out of the runtime
        // manifest before the registry drain mutably borrows the session.
        let committed_switching: postretro_foundation::SwitchingDescriptor;
        let committed_audio_profile: postretro_scripting_core::runtime::ModAudioProfile;
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
                committed_audio_profile = session
                    .scripting
                    .script_runtime
                    .mod_manifest()
                    .map(|manifest| manifest.audio)
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
                committed_presentation_templates = session.scripting.presentation_templates();
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
                crate::app::accessibility_panel::warn_missing_accessibility_entries(
                    &session.modal_stack,
                    session
                        .frontend
                        .as_ref()
                        .map_or(postretro_ui::demo::FRONTEND_MENU_NAME, |f| {
                            f.menu_tree.as_str()
                        }),
                    None,
                );

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
                                let is_connected_client = matches!(
                                    session.net_endpoint.as_ref(),
                                    Some(crate::netcode::NetEndpoint::Client { .. })
                                );
                                let local_player_id = if is_connected_client {
                                    None
                                } else {
                                    session.player_options.player_id
                                };
                                let warnings = overlay_persisted_state(
                                    &mut session.scripting.script_ctx.slot_table.borrow_mut(),
                                    &persisted,
                                    identity.as_ref(),
                                    &committed_store_slots,
                                    local_player_id,
                                    postretro_foundation::Seat(0),
                                );
                                for warning in warnings {
                                    log::warn!("[State] {warning}");
                                }
                                if persisted_state_version_is_supported(&persisted) {
                                    // Shared faction sentiment belongs to the host. A connected
                                    // client receives its overlay from the host snapshot path and
                                    // must never seed it from device-local campaign state.
                                    if !is_connected_client {
                                        let factions =
                                            session.scripting.script_ctx.data_registry.borrow();
                                        let mut faction_sentiment = session
                                            .scripting
                                            .script_ctx
                                            .faction_sentiment
                                            .borrow_mut();
                                        for warning in overlay_persisted_faction_sentiment(
                                            &mut faction_sentiment,
                                            &factions.factions,
                                            &persisted,
                                        ) {
                                            log::warn!("[State] {warning}");
                                        }
                                    }
                                    session.persisted_state = Some(persisted);
                                    log::info!(
                                        "[State] restored persistent slots from {}",
                                        state_path.display()
                                    );
                                }
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
        self.apply_mod_audio_profile(committed_audio_profile);
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_presentation_templates(committed_presentation_templates);
        }
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
            match render::splash::load_splash(&source, &self.core_root) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os_preferences::{OS_REPLY_WAIT, OsPreferenceFeed, OsUpdate};
    use crate::startup::lifecycle::tests::test_app;
    use std::path::PathBuf;

    // --- Finding A: the OS-wait gate must see a reply already queued before
    // it runs, not only on a later top-of-frame poll. ---

    #[test]
    fn poll_os_wait_gate_opens_immediately_when_a_reply_is_already_queued() {
        let mut app = test_app();
        let (feed, tx) = OsPreferenceFeed::fake();
        app.session.as_mut().unwrap().os_preferences = feed;
        // Sent, but never drained by a top-of-frame `poll_os_preferences` —
        // on the real logo frame that call runs before `Session::build`
        // installs the session this update would land on.
        tx.send(OsUpdate::Preferences {
            reduce_motion: Some(true),
            increased_contrast: None,
        })
        .unwrap();
        app.os_wait_from = Some(Instant::now());

        assert!(
            app.poll_os_wait_gate(),
            "a reply already queued before the gate runs must open it on the same check"
        );
        assert!(app.session.as_ref().unwrap().os_preferences.has_replied());
    }

    #[test]
    fn poll_os_wait_gate_waits_for_the_deadline_when_no_reply_arrives() {
        let mut app = test_app();
        let (feed, _tx) = OsPreferenceFeed::fake();
        app.session.as_mut().unwrap().os_preferences = feed;
        app.os_wait_from = Some(Instant::now());

        assert!(
            !app.poll_os_wait_gate(),
            "gate must stay closed before the reply or the deadline"
        );

        // No reply arrives; back-date the wait start past the deadline.
        app.os_wait_from = Some(Instant::now() - OS_REPLY_WAIT);
        assert!(
            app.poll_os_wait_gate(),
            "gate must open once the wait deadline has passed with no reply"
        );
    }

    // --- Finding B: a host map that replaces the CLI boot map must leave a
    // frontend menu behind, so a worker failure or an unresolved catalog id
    // doesn't strand the player on a blank Frontend. ---

    #[test]
    fn start_boot_destination_presents_frontend_menu_before_draining_a_queued_host_map() {
        let mut app = test_app();
        app.session
            .as_mut()
            .unwrap()
            .modal_stack
            .registry_mut()
            .register(
                postretro_ui::demo::FRONTEND_MENU_NAME,
                postretro_ui::demo::build_frontend_menu_descriptor(),
                postretro_ui::modal_stack::ScopeTier::Engine,
                false,
            );
        // The host named a catalog id this fixture's empty map catalog
        // doesn't carry, so `resolve_level_source` rejects it and the queued
        // load never spawns a worker — the failure case finding B guards.
        app.level_requests
            .push_back(LevelRequest::Load(LevelSource::Catalog(
                "host-map".to_string(),
            )));

        app.start_boot_destination(BootDestination::BootMap(PathBuf::from(
            "content/dev/maps/cli-boot.prl",
        )));

        assert_eq!(app.boot_state, BootState::Frontend);
        assert_eq!(
            app.session.as_ref().unwrap().modal_stack.active_name(),
            Some(postretro_ui::demo::FRONTEND_MENU_NAME),
            "the frontend menu must be on the stack even though the queued host map failed to resolve",
        );
    }
}
