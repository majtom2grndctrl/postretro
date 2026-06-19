//! Runtime level lifecycle state-machine helpers.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use glam::Vec3;
use winit::event_loop::ActiveEventLoop;

use crate::camera::Camera;
use crate::frame_timing::InterpolableState;
use crate::render;
use crate::scripting::builtins::{
    PLAYER_START_CLASSNAME, apply_classname_dispatch, apply_data_archetype_dispatch,
    spawn_from_player_starts,
};
use crate::scripting::reaction_dispatch::{
    fire_named_event_with_sequences, validate_scoped_sequence_primitives,
    validate_sequence_primitives,
};
use crate::scripting::state_persistence::{
    STATE_FILE_PATH, load_persisted_state, overlay_persisted_state,
};
use crate::startup::{
    BootState, InFlightLevelLoad, LevelLoadEntry, LevelRequest, LevelSource, LoadOutcome,
    SplashSource, StartupTimings, spawn_level_worker,
};
use crate::{App, fx, prl, weapon};

pub(crate) const FRONTEND_CLEAR_COLOR: render::ClearColor = render::ClearColor {
    r: 0.015,
    g: 0.018,
    b: 0.024,
    a: 1.0,
};

#[cfg(feature = "dev-tools")]
const DEV_LEVEL_CYCLE_TARGET: &str = "content/dev/maps/combat-demo.prl";

fn level_source_for_load_entry(entry: &LevelLoadEntry) -> LevelSource {
    if let Some(id) = entry.catalog_id.as_ref() {
        LevelSource::Catalog(id.clone())
    } else {
        LevelSource::Path(PathBuf::from(&entry.path))
    }
}

enum LoadingPoll {
    Pending,
    Disconnected,
    Ready(Box<LoadOutcome>),
}

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
        self.pending_level_log = false;
        self.level_load = None;
        self.active_level_tags.clear();
        self.active_level_source = None;
        self.level_requests.clear();
        self.boot_load = false;
    }

    pub(crate) fn clear_surface_lifetime_level_state(&mut self) {
        // Fog-volume entities live in the script registry; clearing the
        // bridge's id table here keeps it from referencing stale slots if a
        // future surface re-creation re-runs `populate_from_level`.
        // collision_world is reset for the same reason — it must be in a
        // clean placeholder state before populate_from_level runs on resume.
        self.fog_volume_bridge.clear();
        self.collision_world.clear();
        self.active_wieldable = None;
        self.active_wieldable_descriptor = None;
    }

    /// Unload the active level without dropping renderer/window ownership.
    ///
    /// | Cleared on unload | Kept across unload |
    /// |---|---|
    /// | `self.level` (LevelWorld) | renderer device/queue, window |
    /// | per-level GPU resources (textures, geometry) | `script_ctx`, `ScriptRuntime` |
    /// | light bridge, fog bridge, collision world | slot table (no clear method — engine-global) |
    /// | level sounds, sprite collections | entity-type registry (`data_registry.entities`), mod map catalog (`data_registry.maps`) |
    /// | `data_registry` reactions + crossings, presentation cells | persisted-state save path |
    /// | progress tracker, active wieldable, camera pose | |
    pub(crate) fn unload_level(&mut self) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.release_level_resources();
        }
        if let Some(audio) = &mut self.audio {
            audio.release_level_sounds();
        }

        self.level = None;
        self.clear_surface_lifetime_level_state();
        self.light_bridge.clear();
        self.particle_render.reset_for_level();
        self.mesh_clip_tables.clear();
        self.hit_zone_store.clear();
        self.nav_graph = None;
        // The registry is cleared below, retiring the chase agent's entity slot;
        // drop the handle so a stale id is never re-targeted after unload.
        #[cfg(feature = "dev-tools")]
        {
            self.debug_chase_agent = None;
        }
        self.mesh_render.clear();
        self.particle_live_counts.clear();
        self.emitter_bridge.clear();
        self.progress_tracker.clear();
        self.crossing_detector.clear();
        self.script_ctx.data_registry.borrow_mut().clear();
        self.active_level_tags.clear();
        self.active_level_source = None;
        self.script_ctx
            .registry
            .borrow_mut()
            .clear_for_level_unload();
        self.presentation_cells.clear();
        self.modal_stack
            .clear_script_tree_tier(render::ui::modal_stack::ScopeTier::Level);

        self.builtin_handled = None;
        self.pending_spawn_points = None;
        self.pending_map_entities = None;
        self.pending_level_log = false;
        self.camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        self.frame_timing
            .push_state(InterpolableState::new(Vec3::ZERO));
        self.script_time = 0.0;
        self.anim_time = 0.0;
        self.boot_state = BootState::Frontend;
    }

    pub(crate) fn drive_boot_state_for_redraw(&mut self, event_loop: &ActiveEventLoop) -> bool {
        if matches!(
            self.boot_state,
            BootState::Loading | BootState::Frontend | BootState::Running
        ) {
            self.drain_level_requests();
        }

        match self.boot_state {
            BootState::Booting => {
                // A `RedrawRequested` queued before `resumed()` (or after
                // `suspended()` resets boot_state back to `Booting`) can
                // legally arrive here. Drop it silently — `resumed()` will
                // rebuild and request a fresh redraw.
                false
            }
            BootState::Splash => self.run_splash_frame(event_loop),
            BootState::Loading => self.run_loading_frame(event_loop),
            BootState::Frontend => {
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
    fn run_splash_frame(&mut self, event_loop: &ActiveEventLoop) -> bool {
        match self.splash_frame {
            0 => {
                // First Splash frame: paint a black screen. The splash texture
                // is not yet decoded; the splash pass clears to black and draws
                // nothing.
                self.paint_splash(event_loop);
                self.boot_timings.record("first_black_frame");

                // Now that the OS window is showing a black frame, decode and
                // upload the splash synchronously. PNG decode is bounded CPU
                // work (~ms); doing it here keeps the boot path single-threaded
                // and ordering causal.
                let source = SplashSource::Base;
                match render::splash::load_splash(&source) {
                    Ok(loaded) => {
                        self.boot_timings.record("splash_decoded");
                        if let Some(renderer) = self.renderer.as_mut() {
                            let dims = renderer.install_splash_from_loaded(&loaded);
                            log::info!("[Engine] Splash loaded: {}×{}", dims[0], dims[1]);
                        }
                        self.boot_timings.record("splash_uploaded");
                    }
                    Err(err) => {
                        // Missing base splash is a packaging bug; record both
                        // stages so log line A always lists the same set of
                        // stage names regardless of success/failure. Subsequent
                        // splash frames stay black.
                        self.boot_timings.record("splash_decoded");
                        self.boot_timings.record("splash_uploaded");
                        log::warn!("[Engine] failed to decode base splash: {err:#}");
                    }
                }

                self.splash_frame += 1;
                self.request_redraw();
                false
            }
            1 => {
                // Second Splash frame: paint the splash so the user sees it
                // before mod scripts touch the engine.
                self.paint_splash(event_loop);
                self.boot_timings.record("first_splash_frame");
                log::info!("{}", self.boot_timings.summary());

                // Reset so the cursor starts at the top of this frame, not at
                // App construction time.
                self.mod_timings = StartupTimings::new();

                // Mod init runs before the worker spawns so declarations and
                // entity descriptors commit together, then persistence overlays
                // defaults once before any level work begins.
                let script_root = self.content_root.join("scripts");
                self.script_runtime
                    .compile_stale_scripts(&script_root, &self.content_root);
                if let Err(err) = self.script_runtime.run_mod_init(&self.content_root) {
                    log::error!("[Scripting] mod_init failed: {err}");
                } else {
                    let has_manifest = self.script_runtime.mod_manifest().is_some();
                    if let Some(manifest) = self.script_runtime.mod_manifest_mut() {
                        // Drain entity-type descriptors from the validated
                        // mod manifest into the engine-global `DataRegistry`.
                        // Runtime parses; caller owns lifecycle.
                        // See: context/lib/boot_sequence.md §3.
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
                        data_registry
                            .replace_global_crossings(std::mem::take(&mut manifest.crossings));
                        drop(data_registry);

                        // Register mod-scope UI trees into the tiered registry
                        // at `Mod` tier, before the mod-init VM context drops.
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

                // Mod-side override wiring lands with the mod system; today
                // `pending_splash_override` is always `None`. The branch is here
                // so the flow is complete the moment the hook arrives.
                if let Some(source) = self.pending_splash_override.take() {
                    match render::splash::load_splash(&source) {
                        Ok(loaded) => {
                            if let Some(renderer) = self.renderer.as_mut() {
                                let dims = renderer.install_splash_from_loaded(&loaded);
                                log::info!("[Engine] Mod splash loaded: {}×{}", dims[0], dims[1]);
                            }
                            self.mod_timings.record("mod_splash_swap");
                        }
                        Err(err) => {
                            log::error!("[Engine] mod splash override failed: {err:#}");
                        }
                    }
                }

                log::info!("{}", self.mod_timings.summary());

                let Some(map_path) = self.map_path.clone() else {
                    if let Some(renderer) = self.renderer.as_mut() {
                        renderer.clear_splash();
                    }
                    self.boot_state = BootState::Frontend;
                    self.populate_frontend();
                    self.drain_level_requests();
                    self.splash_frame += 1;
                    log::info!("[Engine] no boot map supplied; entering frontend");
                    self.request_redraw();
                    return false;
                };

                // Route boot-map loading through the same request queue runtime
                // transitions use. PRL parse still runs off the main thread, and
                // `Loading` keeps painting while it waits.
                self.boot_load = true;
                self.enqueue_level_request(LevelRequest::Load(LevelSource::Path(map_path)));
                self.boot_state = BootState::Loading;
                self.drain_level_requests();

                self.splash_frame += 1;
                self.request_redraw();
                false
            }
            _ => {
                self.boot_state = BootState::Loading;
                self.run_loading_frame(event_loop)
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

    #[cfg(feature = "dev-tools")]
    pub(crate) fn enqueue_dev_level_cycle(&mut self) {
        self.enqueue_dev_level_cycle_target(PathBuf::from(DEV_LEVEL_CYCLE_TARGET));
    }

    #[cfg(feature = "dev-tools")]
    fn enqueue_dev_level_cycle_target(&mut self, target: PathBuf) {
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

    fn drain_level_requests(&mut self) {
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

    fn level_load_in_flight(&self) -> bool {
        self.level_rx.is_some() || self.level_worker.is_some()
    }

    fn retain_active_level_tags_for_install(&mut self) {
        if let Some(load) = self.level_load.as_ref() {
            self.active_level_tags = load.entry.tags.clone();
            self.active_level_source = Some(level_source_for_load_entry(&load.entry));
        } else {
            self.active_level_tags.clear();
            self.active_level_source = None;
        }
    }

    pub(crate) fn has_installed_level(&self) -> bool {
        self.boot_state == BootState::Running && self.level.is_some()
    }

    pub(crate) fn rebuild_active_reaction_subscribers(&mut self) {
        self.progress_tracker.clear();
        self.progress_tracker.initialize(
            &self.script_ctx.data_registry.borrow(),
            &self.script_ctx.registry.borrow(),
        );
        self.crossing_detector.clear();
        self.crossing_detector.initialize(
            &self.script_ctx.data_registry.borrow(),
            &self.script_ctx.slot_table.borrow(),
        );
    }

    fn resolve_level_source(&self, source: LevelSource) -> Option<InFlightLevelLoad> {
        match source {
            LevelSource::Catalog(id) => {
                let entry = {
                    let data_registry = self.script_ctx.data_registry.borrow();
                    data_registry
                        .maps
                        .iter()
                        .find(|entry| entry.id == id)
                        .cloned()
                };

                let Some(entry) = entry else {
                    log::warn!(
                        "[Loader] catalog level load ignored: map id `{id}` is not registered"
                    );
                    return None;
                };

                let map_path = self.content_root.join(&entry.path);
                Some(InFlightLevelLoad {
                    map_path,
                    content_root: self.content_root.clone(),
                    entry: LevelLoadEntry {
                        catalog_id: Some(entry.id),
                        path: entry.path,
                        name: entry.name,
                        tags: entry.tags,
                    },
                })
            }
            LevelSource::Path(map_path) => {
                let name = map_path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .filter(|stem| !stem.is_empty())
                    .unwrap_or_else(|| map_path.display().to_string());
                Some(InFlightLevelLoad {
                    content_root: self.content_root.clone(),
                    entry: LevelLoadEntry {
                        catalog_id: None,
                        path: map_path.to_string_lossy().into_owned(),
                        name,
                        tags: Vec::new(),
                    },
                    map_path,
                })
            }
        }
    }

    fn begin_level_load(&mut self, load: InFlightLevelLoad) {
        self.level_timings = StartupTimings::new();
        let (tx, rx) = mpsc::channel();
        let handle = spawn_level_worker(load.map_path.clone(), load.content_root.clone(), tx);
        self.level_load = Some(load);
        self.level_rx = Some(rx);
        self.level_worker = Some(handle);
        // Recorded after the spawn call so the delta covers channel creation
        // and thread spawn overhead.
        self.level_timings.record("worker_dispatch");
        self.boot_state = BootState::Loading;
    }

    fn run_loading_frame(&mut self, event_loop: &ActiveEventLoop) -> bool {
        match self.poll_loading_level_worker() {
            LoadingPoll::Ready(outcome) => match *outcome {
                Ok(payload) => self.finish_level_payload(payload, event_loop),
                Err(err) => {
                    self.finish_level_failure(format!("worker failed: {err:#}"), event_loop);
                    false
                }
            },
            LoadingPoll::Disconnected => {
                self.finish_level_failure(
                    "worker channel disconnected before delivery".to_string(),
                    event_loop,
                );
                false
            }
            LoadingPoll::Pending => {
                self.paint_splash(event_loop);
                self.request_redraw();
                false
            }
        }
    }

    fn poll_loading_level_worker(&mut self) -> LoadingPoll {
        use std::sync::mpsc::TryRecvError;

        let Some(rx) = self.level_rx.as_ref() else {
            return LoadingPoll::Pending;
        };

        match rx.try_recv() {
            Ok(outcome) => {
                self.level_rx = None;
                self.level_worker = None;
                LoadingPoll::Ready(Box::new(outcome))
            }
            Err(TryRecvError::Empty) => LoadingPoll::Pending,
            Err(TryRecvError::Disconnected) => {
                self.level_rx = None;
                self.level_worker = None;
                LoadingPoll::Disconnected
            }
        }
    }

    fn finish_level_payload(
        &mut self,
        mut payload: crate::startup::worker::LevelPayload,
        event_loop: &ActiveEventLoop,
    ) -> bool {
        self.level_timings.record("worker_delivered");
        // Splice worker-thread entries between dispatch and delivered so the
        // summary reads chronologically.
        let delivered_idx = self.level_timings.entries.len() - 1;
        for (i, entry) in payload.timings.drain(..).enumerate() {
            self.level_timings.entries.insert(delivered_idx + i, entry);
        }

        match payload.level {
            Some(world) => {
                self.install_level_payload(world, payload.prm_cache_root);
                self.level_load = None;
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.clear_splash();
                }
                self.boot_state = BootState::Running;
                self.boot_load = false;
                // Defer log line C until after the first level frame's render
                // returns, so `first_level_frame` captures GPU work the user
                // actually sees.
                self.pending_level_log = true;
                true
            }
            None => {
                self.finish_level_failure(
                    "worker delivered no level payload".to_string(),
                    event_loop,
                );
                false
            }
        }
    }

    fn finish_level_failure(&mut self, reason: String, event_loop: &ActiveEventLoop) {
        self.level_load = None;
        let was_boot_load = std::mem::take(&mut self.boot_load);
        if was_boot_load {
            log::error!("[Loader] {reason}; boot map load failed");
            self.exit_result = Err(anyhow::anyhow!("{reason}"));
            event_loop.exit();
            return;
        }

        log::error!("[Loader] {reason}; entering frontend");
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.clear_splash();
        }
        self.boot_state = BootState::Frontend;
        self.request_redraw();
    }

    /// Install a delivered level payload on the main thread: GPU texture upload
    /// (from baked `.prm` mip sidecars), UV normalization, GPU geometry upload,
    /// bridge / fog / collision populate, classname dispatch, data script,
    /// archetype sweep, and `levelLoad` fire. Each stage is recorded into
    /// `self.level_timings` for log line C.
    ///
    /// Texture upload now runs before geometry upload: `.prm` slot dimensions
    /// drive UV normalization, so the renderer must have produced
    /// `LoadedTexture`s before the per-leaf texel-space UVs can be converted to
    /// `[0,1]`.
    ///
    /// Called after a level worker delivers a payload; assumes `self.renderer`
    /// is `Some` and `world` is populated.
    fn install_level_payload(&mut self, mut world: prl::LevelWorld, prm_cache_root: PathBuf) {
        self.retain_active_level_tags_for_install();
        // Reset world gravity to the freshly-loaded level's authored value
        // before the data script runs, so any `world.getGravity()` call inside
        // `setupLevel` / `levelLoad` reactions sees the new value.
        self.script_ctx.gravity.set(world.initial_gravity);
        // Clear any in-flight `screen.flash` decay so a flash never bleeds
        // across a level load.
        self.flash_decay.reset();
        // Clear any in-flight vignette/shake (SE) so neither bleeds across a
        // level load — the slots reset to their identity rest values.
        self.vignette_decay.reset();
        self.shake_decay.reset();
        // Reset the input-mode tracker so a mid-transition mode never bleeds
        // across levels.
        self.input_mode_tracker.reset();
        self.active_wieldable = None;
        self.active_wieldable_descriptor = None;

        // Derive material properties from texture names so the renderer can
        // populate per-material uniforms (shininess) without re-parsing.
        let texture_materials: Vec<crate::material::Material> = {
            let mut warned = std::collections::HashSet::new();
            world
                .texture_names
                .iter()
                .map(|n| crate::material::derive_material(n, &mut warned))
                .collect()
        };

        let renderer = match self.renderer.as_mut() {
            Some(r) => r,
            None => {
                log::error!("[Engine] install_level_payload called with no renderer");
                self.level = Some(world);
                return;
            }
        };

        // 1. Textures first — uploaded from the .prm sidecars; their slot
        //    dimensions feed the UV normalize pass.
        renderer.install_textures(
            &world.texture_names,
            &world.texture_cache_keys,
            &prm_cache_root,
            &texture_materials,
        );
        self.level_timings.record("texture_upload");

        // 2. UV normalize using freshly-uploaded diffuse-texture dimensions.
        //    Texel-space UVs on the worker side; converted to `[0,1]` here so
        //    install_level_geometry uploads the final values.
        renderer.normalize_world_uvs(&mut world);
        self.level_timings.record("uv_normalize");

        // 3. Now geometry: vertex_buffer + index_buffer upload to GPU.
        let geometry = render::level_world_to_geometry(&world, &texture_materials);
        renderer.install_level_geometry(&geometry);
        self.level_timings.record("geometry_upload");

        // Reseed the SH diagnostic per-light visibility bitmap to match the
        // freshly-installed level's animated-light count. Reset `seeded` so the
        // panel re-pulls defaults on the next open.
        #[cfg(feature = "dev-tools")]
        if let Some(debug_ui) = self.debug_ui.as_mut() {
            let delta_count = renderer.sh_delta_volumes().len();
            debug_ui.sh_diagnostics_state.per_light_visible.clear();
            debug_ui
                .sh_diagnostics_state
                .per_light_visible
                .resize(delta_count, false);
            debug_ui.sh_diagnostics_state.seeded = false;
        }

        // Build the runtime navigation graph once, from the baked navmesh
        // section. `None` when the map has no navmesh bake.
        self.nav_graph = world
            .navmesh
            .as_ref()
            .map(crate::nav::NavGraph::from_section);

        // Stash the world after the mutations so downstream code paths that
        // read from `self.level` see the normalized vertices.
        self.level = Some(world);

        // One `LightComponent` entity per map-authored light; stable
        // `EntityId`s the bridge's dirty tracker keys off for the level's
        // lifetime.
        {
            let level_lights = renderer.level_lights().to_vec();
            let fgd_sample_float_count = (renderer.scripted_sample_byte_offset() / 4) as u32;
            let mut registry = self.script_ctx.registry.borrow_mut();
            self.light_bridge.populate_from_level(
                &level_lights,
                &mut registry,
                fgd_sample_float_count,
            );
        }

        // Fog volumes — one entity per record + a renderer-side pixel-scale
        // push. Done after light bridge populate so the registry's first fog
        // entity-id always lands after the light entities.
        if let Some(world) = self.level.as_ref() {
            let mut registry = self.script_ctx.registry.borrow_mut();
            self.fog_volume_bridge
                .populate_from_level(&mut registry, &world.fog_volumes);
            renderer.set_fog_pixel_scale(world.fog_pixel_scale);
            renderer.install_fog_cell_masks_for_level(world.fog_cell_masks.clone());
        }

        // Populate before the first game tick so movement collision is ready.
        if let Some(world) = self.level.as_ref() {
            self.collision_world.populate_from_level(world);
        }
        self.level_timings.record("bridges_populated");

        // Sound registry follows level lifetime, parallel to textures: load the
        // level's sounds from `sounds/` here, release them at unload. Fault-
        // tolerant — a missing directory or undecodable file warns and is
        // skipped. Silent if audio init failed (`audio` is `None`).
        if let Some(audio) = &mut self.audio {
            audio.load_level_sounds(&self.content_root);
        }
        self.level_timings.record("audio_load");

        // Sweep map entities through classname dispatch. The returned set of
        // handled classnames is stashed and consumed by the data-archetype sweep
        // below, after the data script populates `data_registry.entities`.
        if let Some(world) = self.level.as_ref() {
            let mut registry = self.script_ctx.registry.borrow_mut();
            let all_entities: Vec<crate::scripting::map_entity::MapEntity> =
                world.map_entities.iter().cloned().map(Into::into).collect();
            let (spawn_points, map_entities): (Vec<_>, Vec<_>) = all_entities
                .into_iter()
                .partition(|e| e.classname == PLAYER_START_CLASSNAME);
            self.pending_spawn_points = Some(spawn_points);
            let handled =
                apply_classname_dispatch(&map_entities, &self.classname_dispatch, &mut registry);
            if !map_entities.is_empty() {
                log::info!(
                    "[Loader] dispatched {total} map entities; {built_in} classname(s) handled by built-in handlers",
                    built_in = handled.len(),
                    total = map_entities.len(),
                );
            }
            self.builtin_handled = Some(handled);
            self.pending_map_entities = Some(map_entities);
        }
        self.level_timings.record("classname_dispatch");

        // Register sprite collections for every distinct `sprite` name in the
        // registry. Covers map-spawned emitters; descriptor-spawned emitters get
        // a second pass after the data script runs.
        let texture_root = self.content_root.join("textures");
        {
            use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;
            use crate::scripting::registry::{ComponentKind, ComponentValue};
            let registry = self.script_ctx.registry.borrow();
            let mut registered: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            for (_id, value) in registry.iter_with_kind(ComponentKind::BillboardEmitter) {
                let ComponentValue::BillboardEmitter(c) = value else {
                    continue;
                };
                let _: &BillboardEmitterComponent = c;
                let collection = c.sprite.clone();
                if collection.is_empty() || !registered.insert(collection.clone()) {
                    continue;
                }
                let frames = fx::smoke::load_collection_frames(&texture_root, &collection)
                    .unwrap_or_else(|| {
                        vec![fx::smoke::SpriteFrame {
                            data: vec![255, 255, 255, 255],
                            width: 1,
                            height: 1,
                        }]
                    });
                renderer.register_smoke_collection(&collection, &frames, 0.3, c.lifetime);
                self.particle_render.register_sprite(&collection);
            }
        }

        // Data script runs once at level open. Errors surface as an empty
        // manifest so the level still loads. Even levels without a data script
        // compose against mod-global reactions/crossings.
        if let Some(world) = &self.level {
            let mut manifest = if let Some(data_script) = &world.data_script {
                self.script_runtime
                    .run_data_script(data_script, &self.content_root)
            } else {
                crate::scripting::data_descriptors::LevelManifest::default()
            };
            if world.data_script.is_some() {
                manifest.reactions =
                    validate_sequence_primitives(manifest.reactions, &self.sequence_registry);
                // Register level-scope UI trees before the data-script VM context
                // drops and before the manifest is consumed by the data registry.
                self.modal_stack.register_script_trees(
                    std::mem::take(&mut manifest.ui_trees),
                    render::ui::modal_stack::ScopeTier::Level,
                );
            }
            self.script_ctx
                .data_registry
                .borrow_mut()
                .populate_from_manifest(manifest, &self.active_level_tags);
            self.rebuild_active_reaction_subscribers();
        }
        self.level_timings.record("data_script");

        // Data-archetype sweep: `data_registry.entities` was populated from
        // `ModManifest.entities` at mod-init. Materialize every matching map
        // placement that the built-in dispatch did not already handle.
        if self.level.is_some() {
            let handled = self.builtin_handled.take().unwrap_or_default();
            let descriptors = self.script_ctx.data_registry.borrow().entities.clone();
            let mut registry = self.script_ctx.registry.borrow_mut();
            let map_entities = self.pending_map_entities.take().unwrap_or_default();
            let descriptor_handled =
                apply_data_archetype_dispatch(&map_entities, &descriptors, &handled, &mut registry);
            if !descriptor_handled.is_empty() {
                log::info!(
                    "[Loader] dispatched {} map entities through descriptor archetypes",
                    descriptor_handled.len(),
                );
            }

            // Capture the first spawn-point position and facing before take()
            // consumes the vec. Camera move is independent of spawn success.
            let first_spawn: Option<(glam::Vec3, glam::Vec3)> = self
                .pending_spawn_points
                .as_ref()
                .and_then(|v| v.first())
                .map(|e| (e.origin, e.angles));

            // Spawn one entity per `player_spawn` placement, routing each through
            // its `entity_class` (default `"player"`).
            let (active_wieldable, active_wieldable_descriptor) =
                match self.pending_spawn_points.take() {
                    Some(spawn_points) if !spawn_points.is_empty() => {
                        let result =
                            spawn_from_player_starts(&spawn_points, &descriptors, &mut registry);
                        (result.active_wieldable, result.active_wieldable_descriptor)
                    }
                    _ => {
                        log::info!("[Loader] no player_spawn in map; skipping player spawn");
                        (None, None)
                    }
                };

            // Attach the `player.health` slot's declared range `[0, max]` now
            // that the pawn (and its health component) has materialized. `max`
            // is mod data, so it cannot be declared at `SlotTable` construction.
            if let Some((_, health)) =
                crate::scripting::components::health::pawn_with_health(&registry)
            {
                use crate::scripting::slot_table::NumericRange;
                if let Err(err) = self
                    .script_ctx
                    .slot_table
                    .borrow_mut()
                    .set_engine_numeric_range(
                        "player.health",
                        NumericRange {
                            min: 0.0,
                            max: health.max,
                        },
                    )
                {
                    log::warn!("[Loader] failed to set player.health range: {err}");
                }
            }

            // Drop the registry borrow before touching `self.level` /
            // `self.camera`.
            drop(registry);
            self.active_wieldable = active_wieldable;
            self.active_wieldable_descriptor = active_wieldable_descriptor;

            if let Some((pos, angles)) = first_spawn {
                self.camera.position = pos;
                // angles is engine-convention radians (YXZ): x=pitch, y=yaw.
                self.camera.yaw = angles.y;
                self.camera.pitch = angles.x;
                self.frame_timing.push_state(InterpolableState::new(pos));
            } else if let Some(world) = self.level.as_ref() {
                // Fallback when no player_spawn: center on level geometry.
                self.camera.position = world.spawn_position();
                self.frame_timing
                    .push_state(InterpolableState::new(self.camera.position));
            }

            // Re-borrow for the dynamic-light absorb step below.
            let registry = self.script_ctx.registry.borrow();

            // Pick up any descriptor-spawned `LightComponent`s so they
            // participate in the per-frame light bridge pack.
            self.light_bridge.absorb_dynamic_lights(&registry);
        }

        // Descriptor-spawned emitters may carry sprite collections not seen
        // during the install-time sweep above. Re-register any new collections
        // so the renderer pass has them ready before the first frame draws.
        if let Some(renderer) = self.renderer.as_mut() {
            use crate::scripting::components::billboard_emitter::BillboardEmitterComponent;
            use crate::scripting::registry::{ComponentKind, ComponentValue};
            let texture_root = self.content_root.join("textures");
            let registry = self.script_ctx.registry.borrow();
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            for (_id, value) in registry.iter_with_kind(ComponentKind::BillboardEmitter) {
                let ComponentValue::BillboardEmitter(c) = value else {
                    continue;
                };
                let _: &BillboardEmitterComponent = c;
                let collection = c.sprite.clone();
                if collection.is_empty() || !seen.insert(collection.clone()) {
                    continue;
                }
                let frames = fx::smoke::load_collection_frames(&texture_root, &collection)
                    .unwrap_or_else(|| {
                        vec![fx::smoke::SpriteFrame {
                            data: vec![255, 255, 255, 255],
                            width: 1,
                            height: 1,
                        }]
                    });
                renderer.register_smoke_collection(&collection, &frames, 0.3, c.lifetime);
                self.particle_render.register_sprite(&collection);
            }

            let collection = weapon::impact_sprite_collection();
            let frames = fx::smoke::load_collection_frames(&texture_root, collection)
                .unwrap_or_else(|| {
                    vec![fx::smoke::SpriteFrame {
                        data: vec![255, 255, 255, 255],
                        width: 1,
                        height: 1,
                    }]
                });
            renderer.register_smoke_collection(
                collection,
                &frames,
                0.45,
                weapon::impact_lifetime(),
            );
            self.particle_render.register_sprite(collection);
        }
        self.level_timings.record("archetype_sweep");

        // Level-load model sweep. Runs AFTER both classname dispatch and the
        // data-archetype sweep so this single sweep sees EVERY mesh entity.
        if let Some(renderer) = self.renderer.as_mut() {
            // Clear per-level transient mesh-pass state at the model-cache install
            // seam, and reset the game-side clip/hit-zone tables before rebuilding
            // them for this level.
            renderer.clear_mesh_pass_for_level_load();
            self.mesh_clip_tables.clear();
            self.hit_zone_store.clear();

            let models = {
                let registry = self.script_ctx.registry.borrow();
                crate::distinct_mesh_models(&registry)
            };
            for model in &models {
                renderer.load_skinned_model(model, &self.content_root, &prm_cache_root);
                // Build this model's game-side clip table from the renderer's clip
                // metadata (glTF index order). A failed load cached nothing, so the
                // metadata is empty and the table maps no clips.
                let meta = renderer.skinned_model_clip_metadata(model);
                self.mesh_clip_tables
                    .insert(crate::model::ModelHandle::from(model.clone()), &meta);
                // Build this model's game-side hit-zone entry by re-loading the
                // glTF independently.
                self.hit_zone_store
                    .insert_from_load(model, &self.content_root);
            }
            if !models.is_empty() {
                log::info!(
                    "[Model] uploaded {} distinct mesh model(s) for this level",
                    models.len(),
                );
            }

            // Resolve every animated mesh entity's state map against its model's
            // clip table.
            crate::resolve_mesh_entity_clips(
                &mut self.script_ctx.registry.borrow_mut(),
                &self.mesh_clip_tables,
            );

            // Warn once per archetype per declared `health.zoneMultipliers` tag
            // that names no zone on its mesh model.
            crate::warn_unknown_zone_multipliers(
                &self.script_ctx.data_registry.borrow().entities,
                &self.hit_zone_store,
            );
        }
        self.level_timings.record("model_load");

        fire_named_event_with_sequences(
            "levelLoad",
            &self.script_ctx.data_registry.borrow(),
            &self.sequence_registry,
            &self.reaction_registry,
            &self.system_registry,
            &self.script_ctx,
        );
        self.level_timings.record("level_load_event");
        self.script_time = 0.0;
        // Animation clock is level-relative like `script_time`. The scale field
        // is engine config, not level state, so it is not reset here.
        self.anim_time = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};
    use std::time::Instant;

    use crate::frame_timing::{FrameRateMeter, FrameTiming};
    use crate::input::InputFocus;
    use crate::scripting::ctx::ScriptCtx;
    use crate::scripting::data_descriptors::{
        CrossingCondition, CrossingDescriptor, EntityTypeDescriptor, NamedReaction,
        PrimitiveDescriptor, ProgressDescriptor, ReactionDescriptor,
    };
    use crate::scripting::primitives::register_all;
    use crate::scripting::primitives_registry::PrimitiveRegistry;
    use crate::scripting::registry::Transform;
    use crate::scripting::runtime::{
        Frontend, MenuCamera, ModMapEntry, ScriptRuntime, ScriptRuntimeConfig,
    };
    use crate::scripting::slot_table::{
        SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue,
    };
    use crate::scripting::{self, reaction_dispatch};
    use crate::{collision, input, options, render, scripting_systems, view_feel};

    const FIXTURE_MAP_A: &str = "fixture_map_a_reactor_room";
    const FIXTURE_MAP_B: &str = "fixture_map_b_combat_lab";

    fn test_runtime(ctx: &ScriptCtx) -> ScriptRuntime {
        let mut registry = PrimitiveRegistry::new();
        register_all(&mut registry, ctx.clone());
        ScriptRuntime::new(&registry, &ScriptRuntimeConfig::default(), ctx).unwrap()
    }

    fn test_app() -> App {
        let script_ctx = ScriptCtx::new();
        let script_runtime = test_runtime(&script_ctx);
        let initial_state = InterpolableState::new(Vec3::ZERO);
        App {
            renderer: None,
            audio: None,
            window_state: None,
            level: None,
            nav_graph: None,
            map_path: None,
            content_root: PathBuf::from("content/dev"),
            exit_result: Ok(()),
            camera: Camera::new(Vec3::ZERO, 0.0, 0.0),
            input_system: input::InputSystem::new(input::default_bindings()),
            gameplay_input_latch: input::GameplayInputLatch::new(),
            crouch_toggle_active: false,
            player_options: options::PlayerOptions::default(),
            settings_path: None,
            input_focus: InputFocus::Gameplay,
            ui_dispatch: input::UiDispatch::new(),
            gamepad_system: None,
            cursor_pos: None,
            nav_stick_tracker: input::StickNavTracker::new(),
            frame_timing: FrameTiming::new(initial_state),
            view_feel_state: view_feel::ViewFeelState::default(),
            diagnostic_inputs: input::DiagnosticInputs::new(input::default_diagnostic_chords()),
            capture_portal_walk_next_frame: false,
            scratch_cells: Vec::new(),
            frame_rate_meter: FrameRateMeter::new(),
            title_buffer: String::new(),
            last_title_update: Instant::now(),
            script_runtime,
            script_ctx: script_ctx.clone(),
            player_hud_state: scripting_systems::ui_proxy::PlayerHudStatePublisher::new(
                script_ctx.clone(),
            ),
            flash_decay: scripting_systems::flash_decay::FlashDecay::new(script_ctx.clone()),
            vignette_decay: scripting_systems::vignette_decay::VignetteDecay::new(
                script_ctx.clone(),
            ),
            shake_decay: scripting_systems::shake_decay::ShakeDecay::new(script_ctx.clone()),
            presentation_cells: scripting_systems::presentation_cells::PresentationCellStore::new(),
            modal_stack: render::ui::modal_stack::ModalStack::new(),
            mod_theme_override: Default::default(),
            frontend: None,
            ui_focus: input::UiFocusEngine::new(),
            ui_focus_rects: None,
            ui_input_mode: input::InputMode::default(),
            input_mode_tracker: scripting_systems::input_mode::InputModeTracker::new(script_ctx),
            pending_mode_signal: None,
            pending_menu_toggle: false,
            pending_exit_to_desktop: false,
            ui_focused_id: None,
            state_store_lifecycle: Default::default(),
            sequence_registry: scripting::sequence::SequencedPrimitiveRegistry::new(),
            reaction_registry: scripting::reactions::registry::ReactionPrimitiveRegistry::new(),
            system_registry: scripting::reactions::system_commands::SystemReactionRegistry::new(),
            progress_tracker: reaction_dispatch::ProgressTracker::new(),
            crossing_detector: scripting::state_crossings::CrossingDetector::new(),
            classname_dispatch: scripting::builtins::ClassnameDispatch::new(),
            light_bridge: scripting_systems::light_bridge::LightBridge::new(),
            fog_volume_bridge: scripting_systems::fog_volume_bridge::FogVolumeBridge::new(),
            emitter_bridge: scripting_systems::emitter_bridge::EmitterBridge::new(),
            particle_live_counts: std::collections::HashMap::new(),
            collision_world: collision::CollisionWorld::new(),
            particle_render: scripting_systems::particle_render::ParticleRenderCollector::new(),
            mesh_render: scripting_systems::mesh_render::MeshRenderCollector::new(),
            mesh_clip_tables: scripting_systems::mesh_anim::MeshClipTables::new(),
            hit_zone_store: scripting_systems::hit_zones::HitZoneStore::new(),
            active_wieldable: None,
            active_wieldable_descriptor: None,
            boot_state: BootState::Running,
            splash_frame: 0,
            pending_level_log: false,
            pending_splash_override: None,
            builtin_handled: None,
            pending_spawn_points: None,
            pending_map_entities: None,
            script_time: 0.0,
            anim_time: 0.0,
            anim_time_scale: 1.0,
            boot_timings: StartupTimings::new(),
            mod_timings: StartupTimings::new(),
            level_timings: StartupTimings::new(),
            active_level_tags: Vec::new(),
            active_level_source: None,
            level_load: None,
            level_rx: None,
            level_worker: None,
            level_requests: VecDeque::new(),
            boot_load: false,
            #[cfg(feature = "dev-tools")]
            debug_ui: None,
            #[cfg(feature = "dev-tools")]
            debug_chase_agent: None,
        }
    }

    fn slot_snapshot(app: &App) -> BTreeMap<String, SlotRecord> {
        let slots = app.script_ctx.slot_table.borrow();
        let first_name = slots
            .iter()
            .next()
            .map(|(name, _)| name.to_string())
            .expect("slot table should carry engine slots");
        assert!(
            slots.get(&first_name).is_some(),
            "slot snapshot must exercise SlotTable::get"
        );
        slots
            .iter()
            .map(|(name, record)| (name.to_string(), record.clone()))
            .collect()
    }

    fn descriptor(name: &str) -> EntityTypeDescriptor {
        EntityTypeDescriptor {
            canonical_name: Some(name.to_string()),
            default_weapon: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            mesh: None,
            health: None,
        }
    }

    fn named_reaction(name: &str) -> NamedReaction {
        NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "testPrimitive".to_string(),
                tag: None,
                on_complete: None,
                args: serde_json::Value::Object(Default::default()),
            }),
        }
    }

    fn progress_reaction(name: &str, tag: &str, at: f32, fire: &str) -> NamedReaction {
        NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Progress(ProgressDescriptor {
                tag: tag.to_string(),
                at,
                fire: fire.to_string(),
            }),
        }
    }

    fn scoped_global_progress(
        name: &str,
        tag: &str,
        fire: &str,
    ) -> scripting::data_registry::ScopedReaction {
        scripting::data_registry::ScopedReaction {
            reaction: progress_reaction(name, tag, 1.0, fire),
            levels: Vec::new(),
        }
    }

    fn scoped_global_crossing(slot: &str, fire: &str) -> scripting::data_registry::ScopedCrossing {
        scripting::data_registry::ScopedCrossing {
            crossing: CrossingDescriptor {
                slot: slot.to_string(),
                condition: CrossingCondition::Below { threshold: 0.5 },
                max: 100.0,
                fire: vec![fire.to_string()],
            },
            levels: Vec::new(),
        }
    }

    fn number_slot(value: f32) -> SlotRecord {
        let mut record = SlotRecord::new(SlotSchema {
            slot_type: SlotType::Number,
            default: None,
            range: None,
            persist: false,
            readonly: false,
            ownership: SlotOwnership::Mod,
        });
        record.value = Some(SlotValue::Number(value));
        record
    }

    fn catalog_map(id: &str, path: &str, name: &str, tags: &[&str]) -> ModMapEntry {
        ModMapEntry {
            id: id.to_string(),
            path: path.to_string(),
            name: name.to_string(),
            tags: tags.iter().map(|tag| tag.to_string()).collect(),
        }
    }

    fn drop_in_flight_worker(app: &mut App) {
        app.level_rx = None;
        if let Some(handle) = app.level_worker.take() {
            handle
                .join()
                .expect("level worker should not panic during lifecycle test");
        }
        app.level_load = None;
    }

    fn map_light(tag: &str, origin: [f64; 3]) -> prl::MapLight {
        prl::MapLight {
            origin,
            light_type: prl::LightType::Point,
            intensity: 1.0,
            color: [1.0, 0.8, 0.6],
            falloff_model: prl::FalloffModel::InverseSquared,
            falloff_range: 16.0,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, 0.0, 0.0],
            is_dynamic: false,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![tag.to_string()],
            leaf_index: 0,
            shadow_type: prl::ShadowType::StaticLightMap,
        }
    }

    fn fog_record(
        tag: &str,
        center_x: f32,
    ) -> postretro_level_format::fog_volumes::FogVolumeRecord {
        postretro_level_format::fog_volumes::FogVolumeRecord {
            min: [center_x - 1.0, 0.0, -1.0],
            density: 0.5,
            max: [center_x + 1.0, 2.0, 1.0],
            edge_softness: 0.25,
            glow: 0.0,
            radial_falloff: 1.0,
            center: [center_x, 1.0, 0.0],
            inv_half_ext: [1.0, 1.0, 1.0],
            half_diag: 2.0,
            shape_mode: 0.0,
            tint: [1.0, 1.0, 1.0],
            saturation: 1.0,
            min_brightness: 0.0,
            light_range: 1.0,
            anisotropy: 0.0,
            ambient_scatter: 1.0,
            plane_count: 0,
            planes: vec![],
            tags: vec![tag.to_string()],
        }
    }

    fn vertex(position: [f32; 3]) -> crate::geometry::WorldVertex {
        crate::geometry::WorldVertex {
            position,
            base_uv: [0.0, 0.0],
            normal_oct: [0, 0],
            tangent_packed: [0, 0],
            lightmap_uv: [0, 0],
        }
    }

    fn level_world(_name: &str, triangle_count: usize) -> prl::LevelWorld {
        let mut vertices = vec![
            vertex([0.0, 0.0, 0.0]),
            vertex([1.0, 0.0, 0.0]),
            vertex([0.0, 1.0, 0.0]),
        ];
        let mut indices = vec![0, 1, 2];
        if triangle_count > 1 {
            vertices.extend([
                vertex([2.0, 0.0, 0.0]),
                vertex([3.0, 0.0, 0.0]),
                vertex([2.0, 1.0, 0.0]),
            ]);
            indices.extend([3, 4, 5]);
        }

        prl::LevelWorld {
            vertices,
            indices,
            face_meta: Vec::new(),
            leaves: vec![prl::LeafData {
                bounds_min: Vec3::ZERO,
                bounds_max: Vec3::ONE,
                face_start: 0,
                face_count: 0,
                is_solid: false,
            }],
            nodes: Vec::new(),
            root: prl::BspChild::Leaf(0),
            portals: Vec::new(),
            leaf_portals: vec![Vec::new()],
            has_portals: false,
            texture_names: Vec::new(),
            texture_cache_keys: Default::default(),
            bvh: crate::geometry::BvhTree {
                nodes: Vec::new(),
                leaves: Vec::new(),
                root_node_index: 0,
            },
            lights: Vec::new(),
            light_influences: Vec::new(),
            sh_volume: None,
            lightmap: None,
            lightmap_mode: Default::default(),
            sdf_atlas: None,
            chunk_light_list: None,
            animated_light_chunks: None,
            animated_light_weight_maps: None,
            delta_sh_volumes: None,
            direct_sh_volume: None,
            data_script: None,
            map_entities: Vec::new(),
            fog_volumes: Vec::new(),
            fog_pixel_scale: 4,
            initial_gravity: -9.8,
            fog_cell_masks: None,
            navmesh: None,
        }
    }

    struct CpuFixture {
        name: &'static str,
        reaction_name: &'static str,
        light_tag: &'static str,
        fog_tag: &'static str,
        light_count: usize,
        fog_count: usize,
        triangle_count: usize,
    }

    fn install_cpu_fixture(app: &mut App, fixture: CpuFixture) {
        app.level = Some(level_world(fixture.name, fixture.triangle_count));
        app.script_ctx
            .data_registry
            .borrow_mut()
            .populate_from_manifest(
                crate::scripting::data_descriptors::LevelManifest {
                    reactions: vec![named_reaction(fixture.reaction_name)],
                    crossings: Vec::new(),
                    ui_trees: Vec::new(),
                },
                &[],
            );

        let lights = (0..fixture.light_count)
            .map(|i| map_light(fixture.light_tag, [i as f64, 2.0, 3.0]))
            .collect::<Vec<_>>();
        app.light_bridge
            .populate_from_level(&lights, &mut app.script_ctx.registry.borrow_mut(), 0);

        let fog_records = (0..fixture.fog_count)
            .map(|i| fog_record(fixture.fog_tag, i as f32 * 4.0))
            .collect::<Vec<_>>();
        app.fog_volume_bridge
            .populate_from_level(&mut app.script_ctx.registry.borrow_mut(), &fog_records);

        if let Some(world) = app.level.as_ref() {
            app.collision_world.populate_from_level(world);
        }
    }

    #[test]
    fn unload_level_preserves_slot_table_and_entity_type_registry() {
        let mut app = test_app();
        app.script_ctx
            .slot_table
            .borrow_mut()
            .insert_namespace(
                "test.global",
                vec![(
                    "score".to_string(),
                    SlotRecord::new(SlotSchema {
                        slot_type: SlotType::Number,
                        default: Some(SlotValue::Number(42.0)),
                        range: None,
                        persist: true,
                        readonly: false,
                        ownership: SlotOwnership::Mod,
                    }),
                )],
            )
            .unwrap();
        app.script_ctx
            .data_registry
            .borrow_mut()
            .upsert_entity_type(descriptor("global_grunt"));

        install_cpu_fixture(
            &mut app,
            CpuFixture {
                name: FIXTURE_MAP_A,
                reaction_name: "reactorWave",
                light_tag: "reactor_only",
                fog_tag: "reactor_fog",
                light_count: 1,
                fog_count: 1,
                triangle_count: 2,
            },
        );
        app.script_time = 12.5;
        app.anim_time = 3.25;
        app.presentation_cells.write(
            "level_panel".to_string(),
            "count".to_string(),
            SlotValue::Number(11.0),
        );
        assert!(!app.presentation_cells.snapshot().is_empty());

        let slots_before = slot_snapshot(&app);
        app.script_ctx
            .data_registry
            .borrow_mut()
            .replace_maps(vec![ModMapEntry {
                id: "e1m1".to_string(),
                path: "maps/e1m1.prl".to_string(),
                name: "Entryway".to_string(),
                tags: vec!["campaign".to_string()],
            }]);
        let data_before = {
            let data_registry = app.script_ctx.data_registry.borrow();
            (data_registry.entities.clone(), data_registry.maps.clone())
        };

        app.unload_level();

        assert_eq!(slot_snapshot(&app), slots_before);
        let data_after = {
            let data_registry = app.script_ctx.data_registry.borrow();
            (data_registry.entities.clone(), data_registry.maps.clone())
        };
        assert_eq!(data_after, data_before);
        assert!(app.boot_state == BootState::Frontend);
        assert!(app.level.is_none());
        assert_eq!(app.script_time, 0.0);
        assert_eq!(app.anim_time, 0.0);
        assert!(app.presentation_cells.snapshot().is_empty());
    }

    #[test]
    fn reinstall_after_unload_leaves_no_fixture_a_cpu_residue() {
        let mut app = test_app();

        install_cpu_fixture(
            &mut app,
            CpuFixture {
                name: FIXTURE_MAP_A,
                reaction_name: "reactorWave",
                light_tag: "reactor_only",
                fog_tag: "reactor_fog",
                light_count: 2,
                fog_count: 2,
                triangle_count: 2,
            },
        );
        assert_eq!(app.light_bridge.light_count(), 2);
        assert_eq!(app.fog_volume_bridge.entity_count(), 2);
        assert_eq!(app.collision_world.triangle_count(), 2);

        app.unload_level();

        install_cpu_fixture(
            &mut app,
            CpuFixture {
                name: FIXTURE_MAP_B,
                reaction_name: "combatWave",
                light_tag: "combat_only",
                fog_tag: "combat_fog",
                light_count: 1,
                fog_count: 1,
                triangle_count: 1,
            },
        );

        let data_registry = app.script_ctx.data_registry.borrow();
        assert_eq!(data_registry.reactions.len(), 1);
        assert_eq!(data_registry.reactions[0].name, "combatWave");
        assert!(
            data_registry
                .reactions
                .iter()
                .all(|r| r.name != "reactorWave"),
            "{FIXTURE_MAP_A} reaction leaked into {FIXTURE_MAP_B}"
        );
        drop(data_registry);

        assert_eq!(app.light_bridge.light_count(), 1);
        let light_id = app.light_bridge.entity_for_map_index(0).unwrap();
        assert_eq!(
            app.script_ctx.registry.borrow().get_tags(light_id).unwrap(),
            &["combat_only"],
        );

        assert_eq!(app.fog_volume_bridge.entity_count(), 1);
        assert_eq!(app.fog_volume_bridge.cached_aabb_count(), 1);
        assert!(app.fog_volume_bridge.active_aabbs().is_empty());
        assert_eq!(app.collision_world.triangle_count(), 1);
        assert_eq!(app.collision_world.vertex_count(), 3);
    }

    #[test]
    fn loading_state_defers_and_coalesces_runtime_load_requests() {
        let mut app = test_app();
        let (_tx, rx) = std::sync::mpsc::channel();
        app.boot_state = BootState::Loading;
        app.level_rx = Some(rx);

        app.enqueue_level_request(LevelRequest::Load(LevelSource::Path(PathBuf::from(
            "content/dev/maps/first.prl",
        ))));
        app.enqueue_level_request(LevelRequest::Load(LevelSource::Path(PathBuf::from(
            "content/dev/maps/second.prl",
        ))));
        app.drain_level_requests();

        assert!(
            app.level_rx.is_some(),
            "the active worker receiver remains owned while Loading",
        );
        assert_eq!(
            app.level_requests.len(),
            1,
            "repeated load requests coalesce while a worker is in flight",
        );
        let Some(LevelRequest::Load(LevelSource::Path(path))) = app.level_requests.front() else {
            panic!("queued request should be the coalesced load");
        };
        assert_eq!(path, &PathBuf::from("content/dev/maps/second.prl"));
    }

    #[test]
    fn boot_load_rejects_runtime_requests_while_loading() {
        let mut app = test_app();
        let (_tx, rx) = std::sync::mpsc::channel();
        app.boot_state = BootState::Loading;
        app.boot_load = true;
        app.level_rx = Some(rx);

        app.enqueue_level_request(LevelRequest::Unload);
        app.enqueue_level_request(LevelRequest::Load(LevelSource::Path(PathBuf::from(
            "content/dev/maps/runtime.prl",
        ))));
        app.drain_level_requests();

        assert!(app.level_rx.is_some());
        assert!(
            app.level_requests.is_empty(),
            "runtime requests cannot cancel or replace the active boot load",
        );
        assert!(
            app.boot_load,
            "boot fatality marker stays with the active load"
        );
    }

    #[test]
    fn catalog_level_load_resolves_path_and_stores_in_flight_entry() {
        let mut app = test_app();
        app.boot_state = BootState::Frontend;
        app.content_root = PathBuf::from("content/mod");
        app.script_ctx
            .data_registry
            .borrow_mut()
            .replace_maps(vec![catalog_map(
                "e1m1",
                "maps/e1m1.prl",
                "Entryway",
                &["campaign", "intro"],
            )]);

        app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog("e1m1".to_string())));
        app.drain_level_requests();

        let load = app
            .level_load
            .as_ref()
            .expect("catalog load should start and store in-flight metadata");
        assert_eq!(load.map_path, PathBuf::from("content/mod/maps/e1m1.prl"));
        assert_eq!(load.content_root, PathBuf::from("content/mod"));
        assert_eq!(load.entry.catalog_id.as_deref(), Some("e1m1"));
        assert_eq!(load.entry.path, "maps/e1m1.prl");
        assert_eq!(load.entry.name, "Entryway");
        assert_eq!(load.entry.tags, ["campaign", "intro"]);
        assert!(matches!(app.boot_state, BootState::Loading));
        assert!(app.level_load_in_flight());

        drop_in_flight_worker(&mut app);
    }

    #[test]
    fn missing_catalog_level_load_is_rejected_without_unloading_running_level() {
        let mut app = test_app();
        app.boot_state = BootState::Running;
        app.level = Some(level_world(FIXTURE_MAP_A, 1));
        app.script_ctx
            .data_registry
            .borrow_mut()
            .replace_maps(vec![catalog_map(
                "known",
                "maps/known.prl",
                "Known Map",
                &["campaign"],
            )]);

        app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog(
            "missing".to_string(),
        )));
        app.drain_level_requests();

        assert!(
            app.level.is_some(),
            "missing id must not unload active level"
        );
        assert!(app.level_load.is_none());
        assert!(app.level_rx.is_none());
        assert!(app.level_worker.is_none());
        assert!(app.level_requests.is_empty());
        assert!(matches!(app.boot_state, BootState::Running));
    }

    #[test]
    fn raw_path_level_load_synthesizes_non_catalog_metadata() {
        let mut app = test_app();
        app.boot_state = BootState::Frontend;
        app.content_root = PathBuf::from("content/dev");
        let raw_path = PathBuf::from("content/dev/maps/raw-dev-map.prl");

        app.enqueue_level_request(LevelRequest::Load(LevelSource::Path(raw_path.clone())));
        app.drain_level_requests();

        let load = app
            .level_load
            .as_ref()
            .expect("raw path load should start with synthesized metadata");
        assert_eq!(load.map_path, raw_path);
        assert_eq!(load.content_root, PathBuf::from("content/dev"));
        assert_eq!(load.entry.catalog_id, None);
        assert_eq!(load.entry.path, "content/dev/maps/raw-dev-map.prl");
        assert_eq!(load.entry.name, "raw-dev-map");
        assert!(load.entry.tags.is_empty());
        assert!(matches!(app.boot_state, BootState::Loading));

        drop_in_flight_worker(&mut app);
    }

    #[test]
    fn frontend_population_pushes_menu_and_enqueues_one_background_catalog_load() {
        let mut app = test_app();
        app.boot_state = BootState::Frontend;
        app.modal_stack.registry_mut().register(
            "mainMenu",
            render::ui::demo::build_frontend_menu_descriptor(),
            render::ui::modal_stack::ScopeTier::Mod,
            false,
        );
        app.frontend = Some(Frontend {
            menu_tree: "mainMenu".to_string(),
            background_level: Some("menu_backdrop".to_string()),
            camera: MenuCamera {
                position: [4.0, 2.0, 8.0],
                yaw: -0.6,
                pitch: -0.1,
            },
        });

        app.populate_frontend();
        app.populate_frontend();

        assert_eq!(app.modal_stack.active_name(), Some("mainMenu"));
        assert_eq!(
            app.modal_stack.top_capture_mode(),
            input::UiCaptureMode::Capture,
            "frontend menu must suppress gameplay through the capture-mode path",
        );
        assert_eq!(
            app.level_requests.len(),
            1,
            "frontend population enqueues the declared backdrop exactly once",
        );
        let Some(LevelRequest::Load(LevelSource::Catalog(id))) = app.level_requests.front() else {
            panic!("frontend backdrop request should be a catalog load");
        };
        assert_eq!(id, "menu_backdrop");
    }

    #[test]
    fn frontend_population_falls_back_before_loading_backdrop_when_menu_is_unknown() {
        let mut app = test_app();
        app.boot_state = BootState::Frontend;
        app.modal_stack.registry_mut().register(
            render::ui::demo::FRONTEND_MENU_NAME,
            render::ui::demo::build_frontend_menu_descriptor(),
            render::ui::modal_stack::ScopeTier::Engine,
            false,
        );
        app.frontend = Some(Frontend {
            menu_tree: "missingMenu".to_string(),
            background_level: Some("menu_backdrop".to_string()),
            camera: MenuCamera {
                position: [4.0, 2.0, 8.0],
                yaw: -0.6,
                pitch: -0.1,
            },
        });

        app.populate_frontend();

        assert_eq!(
            app.modal_stack.active_name(),
            Some(render::ui::demo::FRONTEND_MENU_NAME),
            "unknown mod frontend menus must reveal the engine fallback",
        );
        assert_eq!(
            app.modal_stack.top_capture_mode(),
            input::UiCaptureMode::Capture
        );
        assert_eq!(
            app.level_requests.pop_front(),
            Some(LevelRequest::Load(LevelSource::Catalog(
                "menu_backdrop".to_string()
            ))),
            "backdrops load only after a capturing frontend modal is present",
        );
    }

    #[test]
    fn staged_frontend_commit_replaces_active_frontend_modal() {
        use crate::scripting::data_descriptors::RegisteredUiTree;
        use crate::scripting::runtime::StagedManifestCommitOutcome;
        use crate::scripting::staged_manifest::{StagedManifest, StagedManifestBuildResult};

        let mut app = test_app();
        app.boot_state = BootState::Frontend;
        app.modal_stack.registry_mut().register(
            render::ui::demo::FRONTEND_MENU_NAME,
            render::ui::demo::build_frontend_menu_descriptor(),
            render::ui::modal_stack::ScopeTier::Engine,
            false,
        );
        app.modal_stack.registry_mut().register(
            "oldMenu",
            render::ui::demo::build_frontend_menu_descriptor(),
            render::ui::modal_stack::ScopeTier::Mod,
            false,
        );
        app.frontend = Some(Frontend {
            menu_tree: "oldMenu".to_string(),
            background_level: None,
            camera: MenuCamera {
                position: [0.0, 0.0, 0.0],
                yaw: 0.0,
                pitch: 0.0,
            },
        });
        app.present_frontend_menu();
        assert_eq!(app.modal_stack.active_name(), Some("oldMenu"));

        let staged = StagedManifestBuildResult {
            generation: 4,
            mod_root: PathBuf::from("content/dev"),
            status: crate::scripting::staged_manifest::StagedManifestBuildStatus::Built(Box::new(
                StagedManifest {
                    name: "Replacement".to_string(),
                    entities: Vec::new(),
                    maps: Vec::new(),
                    reactions: Vec::new(),
                    crossings: Vec::new(),
                    ui_trees: vec![RegisteredUiTree {
                        name: "newMenu".to_string(),
                        tree: render::ui::demo::build_frontend_menu_descriptor(),
                        always_on: false,
                    }],
                    theme: Default::default(),
                    frontend: Some(Frontend {
                        menu_tree: "newMenu".to_string(),
                        background_level: None,
                        camera: MenuCamera {
                            position: [1.0, 2.0, 3.0],
                            yaw: 0.25,
                            pitch: -0.5,
                        },
                    }),
                    store_declarations: Default::default(),
                    dependency_paths: Vec::new(),
                },
            )),
            diagnostics: Vec::new(),
        };
        let committed = StagedManifestCommitOutcome::Committed {
            generation: 4,
            descriptor_count: 0,
            applied_actions: 0,
            dropped_missing_targets: 0,
        };

        app.commit_staged_ui_manifest(&staged, &committed);
        assert_eq!(
            app.modal_stack.active_name(),
            Some("newMenu"),
            "staged replacement updates the active frontend modal clone",
        );

        let omitted = StagedManifestBuildResult {
            generation: 5,
            mod_root: PathBuf::from("content/dev"),
            status: crate::scripting::staged_manifest::StagedManifestBuildStatus::NoStartScript,
            diagnostics: Vec::new(),
        };
        let omitted_committed = StagedManifestCommitOutcome::Committed {
            generation: 5,
            descriptor_count: 0,
            applied_actions: 0,
            dropped_missing_targets: 0,
        };

        app.commit_staged_ui_manifest(&omitted, &omitted_committed);
        assert_eq!(
            app.modal_stack.active_name(),
            Some(render::ui::demo::FRONTEND_MENU_NAME),
            "staged omission replaces the active frontend modal with the engine fallback",
        );
        assert_eq!(
            app.modal_stack.top_capture_mode(),
            input::UiCaptureMode::Capture
        );
    }

    #[test]
    fn no_backdrop_frontend_button_activation_dispatches_load_command() {
        use crate::render::ui::tree::{FocusNeighbors, FocusRect, FocusRectList, NodeInteraction};

        let mut app = test_app();
        app.boot_state = BootState::Frontend;
        crate::scripting::reactions::system_commands::register_system_reaction_primitives(
            &mut app.system_registry,
        );
        app.modal_stack.registry_mut().register(
            render::ui::demo::FRONTEND_MENU_NAME,
            render::ui::demo::build_frontend_menu_descriptor(),
            render::ui::modal_stack::ScopeTier::Engine,
            false,
        );
        app.present_frontend_menu();
        app.ui_focus_rects = Some(FocusRectList {
            rects: vec![FocusRect {
                id: "play".to_string(),
                rect: [0.0, 0.0, 100.0, 32.0],
                z: 0,
                group: None,
                neighbors: FocusNeighbors::default(),
                interaction: Some(NodeInteraction::Button {
                    on_press: "startCampaign".to_string(),
                    repeat_on_hold: None,
                }),
                selected: None,
                checked: None,
                disabled: false,
            }],
            groups: Vec::new(),
            initial_focus: Some("play".to_string()),
            restore_on_return: false,
        });
        app.script_ctx
            .data_registry
            .borrow_mut()
            .reactions
            .push(NamedReaction {
                name: "startCampaign".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "loadLevel".to_string(),
                    tag: None,
                    args: serde_json::json!({ "map": "e1m1" }),
                    on_complete: None,
                }),
            });

        app.fire_focused_button_activation(Some("play"));
        app.dispatch_system_commands();

        assert_eq!(
            app.level_requests.pop_front(),
            Some(LevelRequest::Load(LevelSource::Catalog("e1m1".to_string()))),
        );
        assert!(
            app.modal_stack.is_empty(),
            "frontend activation clears the menu before gameplay load starts",
        );
    }

    #[test]
    fn catalog_tags_are_available_on_in_flight_load_before_data_script_runs() {
        let mut app = test_app();
        app.boot_state = BootState::Frontend;
        app.content_root = PathBuf::from("content/mod");
        app.script_ctx
            .data_registry
            .borrow_mut()
            .replace_maps(vec![catalog_map(
                "arena",
                "maps/arena.prl",
                "Arena",
                &["deathmatch", "night"],
            )]);

        app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog(
            "arena".to_string(),
        )));
        app.drain_level_requests();

        assert_eq!(
            app.level_load
                .as_ref()
                .expect("catalog load should be in flight before install")
                .entry
                .tags,
            ["deathmatch", "night"],
            "catalog tags must be present before worker delivery and data-script install",
        );
        assert!(
            app.script_ctx.data_registry.borrow().reactions.is_empty(),
            "data script has not run while load metadata is already available",
        );

        drop_in_flight_worker(&mut app);
    }

    #[test]
    fn catalog_level_install_retains_active_tags_from_in_flight_load() {
        let mut app = test_app();
        app.boot_state = BootState::Frontend;
        app.content_root = PathBuf::from("content/mod");
        app.script_ctx
            .data_registry
            .borrow_mut()
            .replace_maps(vec![catalog_map(
                "e1m1",
                "maps/e1m1.prl",
                "Entryway",
                &["campaign", "intro"],
            )]);

        app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog("e1m1".to_string())));
        app.drain_level_requests();
        app.retain_active_level_tags_for_install();

        assert_eq!(app.active_level_tags, ["campaign", "intro"]);
        assert_eq!(
            app.active_level_source,
            Some(LevelSource::Catalog("e1m1".to_string()))
        );

        drop_in_flight_worker(&mut app);
    }

    #[test]
    fn raw_path_level_install_retains_empty_active_tags() {
        let mut app = test_app();
        app.boot_state = BootState::Frontend;
        app.content_root = PathBuf::from("content/dev");

        app.enqueue_level_request(LevelRequest::Load(LevelSource::Path(PathBuf::from(
            "content/dev/maps/raw-dev-map.prl",
        ))));
        app.drain_level_requests();
        app.retain_active_level_tags_for_install();

        assert!(app.active_level_tags.is_empty());
        assert_eq!(
            app.active_level_source,
            Some(LevelSource::Path(PathBuf::from(
                "content/dev/maps/raw-dev-map.prl"
            )))
        );

        drop_in_flight_worker(&mut app);
    }

    #[test]
    fn queued_load_requests_coalesce_before_lifecycle_drain() {
        let mut app = test_app();

        app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog(
            "intermediate".to_string(),
        )));
        app.enqueue_level_request(LevelRequest::Load(LevelSource::Catalog(
            "final".to_string(),
        )));

        assert_eq!(app.level_requests.len(), 1);
        assert_eq!(
            app.level_requests.front(),
            Some(&LevelRequest::Load(LevelSource::Catalog(
                "final".to_string()
            ))),
            "rapid frontend activations should not install intermediate maps",
        );
    }

    #[test]
    fn load_level_system_command_queues_catalog_load_request() {
        let mut app = test_app();
        app.modal_stack.registry_mut().register(
            "deathScreen",
            render::ui::demo::build_frontend_menu_descriptor(),
            render::ui::modal_stack::ScopeTier::Mod,
            false,
        );
        app.modal_stack.push_named("deathScreen", None);

        app.script_ctx.system_commands.push(
            scripting::reactions::system_commands::SystemReactionCommand::LoadLevel {
                map: "e1m1".to_string(),
            },
        );
        app.dispatch_system_commands();

        assert_eq!(
            app.level_requests.pop_front(),
            Some(LevelRequest::Load(LevelSource::Catalog("e1m1".to_string())))
        );
        assert!(app.level_requests.is_empty());
        assert!(
            app.modal_stack.is_empty(),
            "starting gameplay clears the initiating modal before controls return",
        );
    }

    #[test]
    fn restart_level_system_command_requeues_retained_active_source() {
        let mut app = test_app();
        app.active_level_source = Some(LevelSource::Path(PathBuf::from(
            "content/dev/maps/raw-dev-map.prl",
        )));

        app.script_ctx
            .system_commands
            .push(scripting::reactions::system_commands::SystemReactionCommand::RestartLevel);
        app.dispatch_system_commands();

        assert_eq!(
            app.level_requests.pop_front(),
            Some(LevelRequest::Load(LevelSource::Path(PathBuf::from(
                "content/dev/maps/raw-dev-map.prl"
            ))))
        );
        assert!(app.level_requests.is_empty());
    }

    #[test]
    fn return_to_frontend_system_command_queues_unload_then_backdrop_load() {
        let mut app = test_app();
        app.modal_stack.registry_mut().register(
            "mainMenu",
            render::ui::demo::build_frontend_menu_descriptor(),
            render::ui::modal_stack::ScopeTier::Mod,
            false,
        );
        app.frontend = Some(Frontend {
            menu_tree: "mainMenu".to_string(),
            background_level: Some("menuBackdrop".to_string()),
            camera: MenuCamera {
                position: [0.0, 0.0, 0.0],
                yaw: 0.0,
                pitch: 0.0,
            },
        });

        app.script_ctx
            .system_commands
            .push(scripting::reactions::system_commands::SystemReactionCommand::ReturnToFrontend);
        app.dispatch_system_commands();

        assert_eq!(
            app.modal_stack.active_name(),
            Some("mainMenu"),
            "returning to frontend presents the menu before backdrop reload",
        );
        assert_eq!(app.level_requests.pop_front(), Some(LevelRequest::Unload));
        assert_eq!(
            app.level_requests.pop_front(),
            Some(LevelRequest::Load(LevelSource::Catalog(
                "menuBackdrop".to_string()
            )))
        );
        assert!(app.level_requests.is_empty());
    }

    #[test]
    fn staged_commit_guard_does_not_recompose_when_no_level_is_installed() {
        let mut app = test_app();
        app.boot_state = BootState::Frontend;
        app.level = None;
        app.active_level_tags.clear();
        app.script_ctx
            .data_registry
            .borrow_mut()
            .replace_global_reactions(vec![scoped_global_progress("waveDone", "wave1", "powerOn")]);
        app.script_ctx
            .data_registry
            .borrow_mut()
            .replace_global_crossings(vec![scoped_global_crossing("test.health", "healthLow")]);

        if app.has_installed_level() {
            app.script_ctx
                .data_registry
                .borrow_mut()
                .recompose_active_sets(&app.active_level_tags);
            app.rebuild_active_reaction_subscribers();
        }

        let registry = app.script_ctx.data_registry.borrow();
        assert!(
            registry.reactions.is_empty(),
            "unscoped globals must not repopulate active reactions after unload",
        );
        assert!(
            registry.crossings.is_empty(),
            "unscoped globals must not repopulate active crossings after unload",
        );
    }

    #[test]
    fn staged_commit_rebuilds_active_subscribers_for_installed_raw_path_level() {
        let mut app = test_app();
        app.boot_state = BootState::Running;
        app.level = Some(level_world("raw_dev_level", 1));
        app.active_level_tags.clear();
        app.script_ctx
            .data_registry
            .borrow_mut()
            .replace_global_reactions(vec![scoped_global_progress("waveDone", "wave1", "powerOn")]);
        app.script_ctx
            .data_registry
            .borrow_mut()
            .replace_global_crossings(vec![scoped_global_crossing("test.health", "healthLow")]);
        {
            let mut entities = app.script_ctx.registry.borrow_mut();
            let id = entities.spawn(Transform::default());
            entities.set_tags(id, vec!["wave1".to_string()]).unwrap();
        }
        app.script_ctx
            .slot_table
            .borrow_mut()
            .insert("test.health".to_string(), number_slot(75.0))
            .expect("test slot should be vacant");

        if app.has_installed_level() {
            app.script_ctx
                .data_registry
                .borrow_mut()
                .recompose_active_sets(&app.active_level_tags);
            app.rebuild_active_reaction_subscribers();
        }

        assert_eq!(
            app.progress_tracker
                .on_entity_killed(&["wave1".to_string()]),
            vec!["powerOn".to_string()],
        );
        app.script_ctx
            .slot_table
            .borrow_mut()
            .get_mut("test.health")
            .expect("test slot should exist")
            .value = Some(SlotValue::Number(25.0));
        assert_eq!(
            app.crossing_detector
                .detect(&app.script_ctx.slot_table.borrow()),
            vec!["healthLow".to_string()],
        );
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn dev_level_cycle_ignores_missing_target_without_unloading() {
        let mut app = test_app();
        app.level = Some(level_world(FIXTURE_MAP_A, 1));
        app.boot_state = BootState::Running;

        let mut missing_target =
            std::env::temp_dir().join("postretro-missing-dev-level-cycle-target.prl");
        let mut salt = 0;
        while missing_target.exists() {
            salt += 1;
            missing_target = std::env::temp_dir().join(format!(
                "postretro-missing-dev-level-cycle-target-{salt}.prl"
            ));
        }

        app.enqueue_dev_level_cycle_target(missing_target);

        assert!(
            app.level.is_some(),
            "missing generated dev PRL must not unload the active level",
        );
        assert!(app.level_requests.is_empty());
        assert!(matches!(app.boot_state, BootState::Running));
    }

    #[cfg(feature = "dev-tools")]
    #[test]
    fn dev_level_cycle_ignores_runtime_load_in_flight_without_queueing_duplicate() {
        let mut app = test_app();
        let (_tx, rx) = std::sync::mpsc::channel();
        app.boot_state = BootState::Loading;
        app.boot_load = false;
        app.level_rx = Some(rx);

        let target = std::env::temp_dir().join(format!(
            "postretro-existing-dev-level-cycle-target-{}.prl",
            std::process::id()
        ));
        std::fs::write(&target, b"test target exists").expect("create dev cycle test target");

        app.enqueue_dev_level_cycle_target(target.clone());

        let _ = std::fs::remove_file(&target);
        assert!(
            app.level_requests.is_empty(),
            "duplicate dev lifecycle cycle must not queue behind an active runtime load",
        );
        assert!(matches!(app.boot_state, BootState::Loading));
        assert!(app.level_rx.is_some());
    }
}
