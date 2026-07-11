//! Runtime level lifecycle state-machine helpers.
//! See: context/lib/boot_sequence.md §1

use std::path::PathBuf;
use std::sync::mpsc;

use glam::Vec3;
use winit::event_loop::ActiveEventLoop;

use crate::camera::Camera;
use crate::frame_timing::InterpolableState;
use crate::render;
use crate::scripting::builtins::{
    PLAYER_START_CLASSNAME, apply_classname_dispatch, apply_data_archetype_dispatch,
    filter_out_client_ai_enemies, spawn_from_player_starts, suppressed_ai_enemy_mesh_models,
};
use crate::startup::{
    BootState, InFlightLevelLoad, LevelLoadEntry, LevelRequest, LevelSource, LoadOutcome,
    StartupTimings, spawn_level_worker,
};
use crate::trigger_bindings::TriggerBindingTable;
use crate::{App, weapon};
use postretro_scripting_core::data_descriptors::LevelManifest;
use postretro_scripting_core::reaction_dispatch::{
    fire_named_event_with_sequences, validate_sequence_primitives,
};

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
        // Fog- and trigger-volume entities live in the script registry;
        // clearing their bridge id tables and trigger state prevents stale
        // slots or bindings if a future surface re-creation re-runs
        // `populate_from_level`. collision_world is reset for the same
        // reason — it must be a clean placeholder before resume populates it.
        // Called both from `unload_level` (session installed) and the suspend
        // path (session may be absent if suspend arrives pre-install), so the
        // session-owned state clears are guarded — a no-op with no session yet.
        if let Some(session) = self.session.as_mut() {
            session.fog_volume_bridge.clear();
            session.trigger_volume_bridge.clear();
            session.trigger_system.clear();
        }
        self.collision_world.clear();
        self.kinematic_mover_colliders.clear();
        self.kinematic_mover_tick_states.clear();
        self.kinematic_mover_render.clear();
        self.trigger_bindings = TriggerBindingTable::default();
        self.active_wieldable = None;
        self.active_wieldable_descriptor = None;
        self.client_weapon_state = None;
        self.client_fire_resolutions.clear();
        self.client_predicted_shots.clear();
    }

    /// Unload the active level without dropping renderer/window ownership.
    ///
    /// | Cleared on unload | Kept across unload |
    /// |---|---|
    /// | `self.level` (LevelWorld) | renderer device/queue, window |
    /// | per-level GPU resources (textures, geometry) | `script_ctx`, `ScriptRuntime` |
    /// | light bridge, fog bridge, trigger-volume bridge, trigger system, trigger bindings, collision world | slot table (no clear method — engine-global) |
    /// | level sounds, sprite collections, `emitter_bridge`, `mesh_render`, `mesh_clip_tables`, `hit_zone_store` | entity-type registry (`data_registry.entities`), mod map catalog (`data_registry.maps`) |
    /// | `data_registry` reactions + crossings, presentation cells | persisted-state save path |
    /// | level-scope UI trees (`modal_stack` `ScopeTier::Level`) | |
    /// | progress tracker, active wieldable, client weapon prediction state, camera pose | |
    pub(crate) fn unload_level(&mut self) {
        // `net_endpoint` and `audio` are session-owned; reset/release them through
        // the session borrow.
        if let Some(session) = self.session.as_mut() {
            if let Some(endpoint) = session.net_endpoint.as_mut() {
                endpoint.reset_level_scoped_client_state();
            }
            if let Some(audio) = session.audio.as_mut() {
                audio.release_level_sounds();
            }
        }

        if let Some(renderer) = self.renderer.as_mut() {
            renderer.release_level_resources();
        }

        self.level = None;
        self.clear_surface_lifetime_level_state();
        self.nav_graph = None;
        // The registry is cleared below, retiring the chase agent's entity slot;
        // drop the handle so a stale id is never re-targeted after unload.
        #[cfg(feature = "dev-tools")]
        {
            self.debug_chase_agent = None;
        }
        self.particle_live_counts.clear();
        if let Some(session) = self.session.as_mut() {
            session.light_bridge.clear();
            session.particle_render.reset_for_level();
            session.mesh_clip_tables.clear();
            session.hit_zone_store.clear();
            session.mesh_render.clear();
            session.emitter_bridge.clear();
            session.progress_tracker.clear();
            session.crossing_detector.clear();
            session
                .scripting
                .script_ctx
                .data_registry
                .borrow_mut()
                .clear();
            session
                .scripting
                .script_ctx
                .registry
                .borrow_mut()
                .clear_for_level_unload();
            session.presentation_cells.clear();
            session
                .modal_stack
                .clear_script_tree_tier(postretro_ui::modal_stack::ScopeTier::Level);
        }
        self.active_level_tags.clear();
        self.active_level_source = None;

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

    pub(super) fn drain_level_requests(&mut self) {
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
        let Some(session) = self.session.as_mut() else {
            return;
        };
        rebuild_reaction_subscribers(
            &mut session.progress_tracker,
            &mut session.crossing_detector,
            &session.scripting.script_ctx,
        );
    }

    /// Rebind trigger events after staged mod-init recomposes the active reaction
    /// set. Tick dispatch holds bound commands, never reaction names, so it must
    /// be refreshed alongside the other active reaction consumers.
    pub(crate) fn rebuild_active_trigger_bindings(&mut self) {
        let bindings = {
            let Some(session) = self.session.as_ref() else {
                return;
            };
            build_trigger_bindings(&session.scripting.script_ctx)
        };
        self.trigger_bindings = bindings;
    }

    fn resolve_level_source(&self, source: LevelSource) -> Option<InFlightLevelLoad> {
        match source {
            LevelSource::Catalog(id) => {
                let entry = {
                    let session = self.session.as_ref()?;
                    let data_registry = session.scripting.script_ctx.data_registry.borrow();
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

    pub(super) fn run_loading_frame(&mut self, event_loop: &ActiveEventLoop) -> bool {
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
                let _ = self.paint_splash(event_loop); // Loading redraws unconditionally; the outcome doesn't drive state advance here.
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
                // M15 Phase 3 (issue 3b): register the listen host's own boot pawn for
                // outbound replication now that the install has spawned + marked it the
                // local player. Reload-safe and a no-op off the host / on a map without a
                // player_spawn. The host pawn stays driven locally by `simulate_tick`.
                self.host_register_own_pawn_after_install();
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
    fn install_level_payload(
        &mut self,
        mut world: postretro_level_loader::LevelWorld,
        prm_cache_root: PathBuf,
    ) {
        self.retain_active_level_tags_for_install();
        // The whole script tranche lives on `Session` (built post-first-pixel).
        // Level install only runs in Loading/Running, where the session is
        // installed. Clone the `ScriptCtx` handle (cheap `Rc` bump) so the many
        // `script_ctx.*` reads below borrow nothing of `self` — the non-`Clone`
        // session subsystems (bridges, collectors, registries) are reached
        // through disjoint `self.session.as_mut()` borrows at each site, kept
        // disjoint from the long-lived `renderer` borrow.
        let script_ctx = self
            .session
            .as_ref()
            .expect("session installed before level install")
            .scripting
            .script_ctx
            .clone();
        // Segment A of the CPU world install: seed gravity from the level's
        // authored value (before the data script runs, so a `world.getGravity()`
        // in `setupLevel` / `levelLoad` reactions sees it) and build the runtime
        // navigation graph. Renderer-free; the nav build reads the un-normalized
        // navmesh section, which the renderer UV pass below does not touch. The
        // windowed path runs its renderer upload and — critically — the
        // light-bridge populate BETWEEN segments A and B, so light entity ids
        // precede the fog entity ids segment B creates (both bridges key dirty
        // tracking on `EntityId`).
        self.nav_graph = install_world_gravity_and_nav(&world, &script_ctx);
        let session = self
            .session
            .as_mut()
            .expect("session installed before level install");
        // Clear any in-flight `screen.flash` decay so a flash never bleeds
        // across a level load.
        session.scripting.flash_decay.reset();
        // Clear any in-flight vignette/shake (SE) so neither bleeds across a
        // level load — the slots reset to their identity rest values.
        session.scripting.vignette_decay.reset();
        session.scripting.shake_decay.reset();
        // Reset the input-mode tracker so a mid-transition mode never bleeds
        // across levels.
        session.scripting.input_mode_tracker.reset();
        self.active_wieldable = None;
        self.active_wieldable_descriptor = None;

        // Derive material properties from texture names so the renderer can
        // populate per-material uniforms (shininess) without re-parsing.
        let texture_materials: Vec<postretro_render_data::material::Material> = {
            let mut warned = std::collections::HashSet::new();
            world
                .texture_names
                .iter()
                .map(|n| {
                    let warned_count = warned.len();
                    let mat = postretro_render_data::material::derive_material(n, &mut warned);
                    let prefix = postretro_render_data::material::parse_prefix(n);
                    if mat == postretro_render_data::material::Material::Default
                        && !prefix.is_empty()
                        && warned.len() > warned_count
                    {
                        log::warn!(
                            "[Material] Unknown prefix '{}' in texture '{}' — using default material",
                            prefix,
                            n,
                        );
                    }
                    mat
                })
                .collect()
        };

        let (level_lights, fgd_sample_float_count) = {
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
            // panel re-pulls defaults on the next open. `debug_ui` is session-owned;
            // read the renderer's delta count (disjoint `self.renderer` borrow) first.
            #[cfg(feature = "dev-tools")]
            {
                let delta_count = renderer.sh_delta_volumes().len();
                if let Some(debug_ui) = self
                    .session
                    .as_mut()
                    .and_then(|session| session.debug_ui.as_mut())
                {
                    debug_ui.sh_diagnostics_state.per_light_visible.clear();
                    debug_ui
                        .sh_diagnostics_state
                        .per_light_visible
                        .resize(delta_count, false);
                    debug_ui.sh_diagnostics_state.seeded = false;
                }
            }

            (
                renderer.level_lights().to_vec(),
                (renderer.scripted_sample_byte_offset() / 4) as u32,
            )
        };

        // Stash the world after the renderer mutations so downstream reads of
        // `self.level` (and segment B) see the normalized vertices.
        self.level = Some(world);

        // One `LightComponent` entity per map-authored light; stable
        // `EntityId`s the bridge's dirty tracker keys off for the level's
        // lifetime.
        {
            let mut registry = script_ctx.registry.borrow_mut();
            self.session
                .as_mut()
                .expect("session installed before level install")
                .light_bridge
                .populate_from_level(&level_lights, &mut registry, fgd_sample_float_count);
        }

        // Segment B of the CPU world install: fog-volume entities, trigger-volume
        // entities, collision + kinematic movers, classname dispatch, the data
        // script, the data-archetype sweep (incl. player spawn), the mesh sweep's
        // CPU half, and the `levelLoad` fire — all renderer-free. The one
        // renderer-coupled step (skinned-model upload + clip-table build) is
        // injected as the `upload_mesh_models` hook so it stays windowed; a
        // headless caller passes a no-op and its clip tables stay empty (the
        // documented headless shape).
        // `suppress` gates the connected-client spawn / AI-enemy suppression
        // (`false` off a connected client — single-player, listen host, headless).
        let suppress = self.is_connected_client();
        // Cloned for the mesh hook and the segment-B handles so neither aliases a
        // `self.content_root` borrow held across the call.
        let install_content_root = self.content_root.clone();
        let renderer = self
            .renderer
            .as_mut()
            .expect("renderer installed before level install");
        let upload_mesh_models =
            |models: &[String],
             clip_tables: &mut crate::scripting_systems::mesh_anim::MeshClipTables| {
                // Clear per-level transient mesh-pass state at the model-cache
                // install seam, then upload each distinct model and build its
                // game-side clip table from the renderer's clip metadata (glTF
                // index order). A failed load cached nothing, so the metadata is
                // empty and the table maps no clips.
                renderer.clear_mesh_pass_for_level_load();
                for model in models {
                    renderer.load_skinned_model(model, &install_content_root, &prm_cache_root);
                    let meta = renderer.skinned_model_clip_metadata(model);
                    let bounds = renderer.skinned_model_local_bounds(model);
                    clip_tables.insert_with_bounds(
                        postretro_model::ModelHandle::from(model.clone()),
                        &meta,
                        bounds,
                    );
                }
                if !models.is_empty() {
                    log::info!(
                        "[Model] uploaded {} distinct mesh model(s) for this level",
                        models.len(),
                    );
                }
            };

        let session = self
            .session
            .as_mut()
            .expect("session installed before level install");
        let handles = WorldInstallHandles {
            world: self
                .level
                .as_ref()
                .expect("level installed before segment B"),
            script_ctx: &script_ctx,
            content_root: install_content_root.as_path(),
            active_level_tags: &self.active_level_tags,
            nav_graph: self.nav_graph.as_ref(),
            collision_world: &mut self.collision_world,
            fog_volume_bridge: &mut session.fog_volume_bridge,
            trigger_volume_bridge: &mut session.trigger_volume_bridge,
            classname_dispatch: &session.classname_dispatch,
            script_runtime: &session.scripting.script_runtime,
            sequence_registry: &session.scripting.sequence_registry,
            reaction_registry: &session.scripting.reaction_registry,
            system_registry: &session.scripting.system_registry,
            modal_stack: &mut session.modal_stack,
            progress_tracker: &mut session.progress_tracker,
            crossing_detector: &mut session.crossing_detector,
            mesh_clip_tables: &mut session.mesh_clip_tables,
            hit_zone_store: &mut session.hit_zone_store,
            suppress_ai_enemies: suppress,
            suppress_boot_pawn: suppress,
        };
        let products = install_world_cpu(handles, &mut self.level_timings, upload_mesh_models);

        self.kinematic_mover_colliders = products.mover_colliders;
        self.trigger_bindings = products.trigger_bindings;
        // Retain the spawn-point placements for the host's runtime net-slot accept
        // path (M15 Phase 3 Task 4): the host materializes each accepted client's
        // descriptor pawn from them later.
        self.host_spawn_points = products.spawn_points;
        self.active_wieldable = products.active_wieldable;
        self.active_wieldable_descriptor = products.active_wieldable_descriptor;

        // E10 Task 4 / M15 Phase 3: register this level's map-placed AI enemies and
        // PRL-loaded movers for outbound replication. Host-gated (a no-op off a
        // listen host) and reload-safe; each takes its own registry borrow.
        self.host_register_map_enemies_after_install();
        self.host_register_loaded_movers_after_install();

        // Pick up any descriptor-spawned `LightComponent`s so they participate in
        // the per-frame light bridge pack.
        {
            let registry = script_ctx.registry.borrow();
            self.session
                .as_mut()
                .expect("session installed before level install")
                .light_bridge
                .absorb_dynamic_lights(&registry);
        }

        // Teleport the camera to the first player spawn (or the geometry center
        // when the map has none). Independent of spawn success.
        if let Some((pos, angles)) = products.first_spawn {
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

        // Renderer-side fog: pixel scale + per-cell masks. The fog-volume entities
        // were created in segment B; this is the windowed GPU half.
        if let Some(world) = self.level.as_ref() {
            if let Some(renderer) = self.renderer.as_mut() {
                renderer.set_fog_pixel_scale(world.fog_pixel_scale);
                renderer.install_fog_cell_masks_for_level(world.fog_cell_masks.clone());
            }
        }

        // Register sprite collections for every distinct emitter `sprite` in the
        // registry — map-spawned and descriptor-spawned alike — plus the weapon
        // impact collection. One pass after segment B populated the full registry
        // (both dispatch sweeps), folding what were two interleaved passes before
        // the extraction; the registered-collection set is identical.
        if let Some(renderer) = self.renderer.as_mut() {
            use postretro_entities::components::billboard_emitter::BillboardEmitterComponent;
            use postretro_entities::{ComponentKind, ComponentValue};
            let texture_root = self.content_root.join("textures");
            let registry = script_ctx.registry.borrow();
            let particle_render = &mut self
                .session
                .as_mut()
                .expect("session installed before level install")
                .particle_render;
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
                let frames =
                    postretro_render_cpu::smoke::load_collection_frames(&texture_root, &collection)
                        .unwrap_or_else(|| {
                            vec![postretro_render_cpu::smoke::SpriteFrame {
                                data: vec![255, 255, 255, 255],
                                width: 1,
                                height: 1,
                            }]
                        });
                renderer.register_smoke_collection(&collection, &frames, 0.3, c.lifetime);
                particle_render.register_sprite(&collection);
            }

            let collection = weapon::impact_sprite_collection();
            let frames =
                postretro_render_cpu::smoke::load_collection_frames(&texture_root, collection)
                    .unwrap_or_else(|| {
                        vec![postretro_render_cpu::smoke::SpriteFrame {
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
            particle_render.register_sprite(collection);
        }

        // Sound registry follows level lifetime, parallel to textures: load the
        // level's sounds from `sounds/`, released at unload. Fault-tolerant — a
        // missing directory or undecodable file warns and is skipped; silent if
        // audio init failed (`audio` is `None`). Session-owned; clone the content
        // root so the `self.session` borrow does not alias the read. Deliberately
        // last in install, after `levelLoad`: safe because `playSound` enqueues an
        // async `SystemReactionCommand` drained a frame later, after install
        // completes, so no reaction observes unloaded sounds.
        let content_root = self.content_root.clone();
        if let Some(audio) = self
            .session
            .as_mut()
            .and_then(|session| session.audio.as_mut())
        {
            audio.load_level_sounds(&content_root);
        }
        self.level_timings.record("audio_load");

        self.script_time = 0.0;
        // Animation clock is level-relative like `script_time`. The scale field
        // is engine config, not level state, so it is not reset here.
        self.anim_time = 0.0;
    }
}

/// Segment A of the CPU world install (renderer-free): seed world gravity from
/// the level's authored value and build the runtime navigation graph from the
/// baked navmesh section (`None` when the map has no navmesh bake). Split from
/// segment B ([`install_world_cpu`]) so the windowed path can run its renderer
/// texture/UV/geometry upload and the light-bridge populate BETWEEN the two:
/// light entities must take registry ids before the fog entities segment B
/// creates (both bridges key dirty tracking on `EntityId`). Headless calls A
/// then B back-to-back — no light entities exist, so the fog ids land first,
/// the documented headless entity-id shape.
pub(crate) fn install_world_gravity_and_nav(
    world: &postretro_level_loader::LevelWorld,
    script_ctx: &postretro_entities::ScriptCtx,
) -> Option<crate::nav::NavGraph> {
    script_ctx.gravity.set(world.initial_gravity);
    world
        .navmesh
        .as_ref()
        .map(crate::nav::NavGraph::from_section)
}

/// Rebuild the level's reaction subscribers: reinitialize the kill-progress
/// tracker and the state-crossing detector from the current data + entity
/// registries and slot table. Free function so both [`App`]'s method and segment
/// B drive it without an `App`.
fn rebuild_reaction_subscribers(
    progress_tracker: &mut postretro_scripting_core::reaction_dispatch::ProgressTracker,
    crossing_detector: &mut postretro_scripting_core::state_crossings::CrossingDetector,
    script_ctx: &postretro_entities::ScriptCtx,
) {
    progress_tracker.clear();
    progress_tracker.initialize(
        &script_ctx.data_registry.borrow(),
        &script_ctx.registry.borrow(),
    );
    crossing_detector.clear();
    crossing_detector.initialize(
        &script_ctx.data_registry.borrow(),
        &script_ctx.slot_table.borrow(),
    );
}

fn build_trigger_bindings(script_ctx: &postretro_entities::ScriptCtx) -> TriggerBindingTable {
    TriggerBindingTable::build(
        &script_ctx.registry.borrow(),
        &script_ctx.data_registry.borrow(),
        &script_ctx.slot_table.borrow(),
    )
}

/// CPU world-install products the tick loop consumes. [`install_world_cpu`]
/// fills these from a level payload without touching the renderer, so a headless
/// caller assembles `simulate_tick`'s arguments from the return value plus the
/// nav graph produced by [`install_world_gravity_and_nav`] and the handles it
/// passed in (populated registry, collision world, hit-zone store).
pub(crate) struct WorldInstallProducts {
    /// Static colliders for every loaded kinematic mover.
    pub(crate) mover_colliders: Vec<crate::collision::moving::MoverCollider>,
    /// Trigger reactions partitioned from the final composed active set.
    pub(crate) trigger_bindings: TriggerBindingTable,
    /// A fresh, empty mover tick-state table. Not an install product — it is
    /// caller-owned per-tick state — returned only so the headless batch runner
    /// has one to hand `simulate_tick` without reaching into `App` (the windowed
    /// `App` field is its own `MoverTickStateTable::default()` and ignores this).
    /// The headless driver (`observability::driver`) is the sole reader, so the
    /// field is dead only in a build without that feature.
    #[cfg_attr(not(feature = "observability"), allow(dead_code))]
    pub(crate) mover_tick_states: crate::kinematic_mover::MoverTickStateTable,
    /// The spawned player pawn's active weapon instance and its descriptor name,
    /// if a boot pawn spawned.
    pub(crate) active_wieldable: Option<postretro_entities::EntityId>,
    pub(crate) active_wieldable_descriptor: Option<String>,
    /// First `player_spawn` origin + engine-convention YXZ angles (x=pitch,
    /// y=yaw), for the windowed camera teleport. `None` when the map has none.
    pub(crate) first_spawn: Option<(Vec3, Vec3)>,
    /// The map's `player_spawn` placements, retained by the windowed host for the
    /// runtime net-slot accept path.
    pub(crate) spawn_points: Vec<crate::scripting::map_entity::MapEntity>,
}

/// Borrowed handles segment B ([`install_world_cpu`]) reads or mutates. Bundled
/// so the windowed caller and a future headless caller pass one context rather
/// than ~18 positional args; every field is a live borrow held only for the one
/// call. The registry, data registry, and slot table are reached through
/// `script_ctx`'s `RefCell`s at runtime, so borrowing them never conflicts with
/// the session field borrows below.
pub(crate) struct WorldInstallHandles<'a> {
    pub(crate) world: &'a postretro_level_loader::LevelWorld,
    pub(crate) script_ctx: &'a postretro_entities::ScriptCtx,
    pub(crate) content_root: &'a std::path::Path,
    pub(crate) active_level_tags: &'a [String],
    /// The nav graph produced by segment A; supplies the descriptor-spawn agent
    /// capsule params. `None` on maps without a navmesh bake.
    pub(crate) nav_graph: Option<&'a crate::nav::NavGraph>,
    pub(crate) collision_world: &'a mut crate::collision::CollisionWorld,
    pub(crate) fog_volume_bridge:
        &'a mut crate::scripting_systems::fog_volume_bridge::FogVolumeBridge,
    pub(crate) trigger_volume_bridge:
        &'a mut crate::scripting_systems::trigger_volume_bridge::TriggerVolumeBridge,
    pub(crate) classname_dispatch: &'a crate::scripting::builtins::ClassnameDispatch,
    pub(crate) script_runtime: &'a postretro_scripting_core::runtime::ScriptRuntime,
    pub(crate) sequence_registry:
        &'a postretro_scripting_core::sequence::SequencedPrimitiveRegistry,
    pub(crate) reaction_registry:
        &'a crate::scripting::reactions::registry::ReactionPrimitiveRegistry,
    pub(crate) system_registry:
        &'a crate::scripting::reactions::system_commands::SystemReactionRegistry,
    pub(crate) modal_stack: &'a mut postretro_ui::modal_stack::ModalStack,
    pub(crate) progress_tracker:
        &'a mut postretro_scripting_core::reaction_dispatch::ProgressTracker,
    pub(crate) crossing_detector:
        &'a mut postretro_scripting_core::state_crossings::CrossingDetector,
    pub(crate) mesh_clip_tables: &'a mut crate::scripting_systems::mesh_anim::MeshClipTables,
    pub(crate) hit_zone_store: &'a mut crate::scripting_systems::hit_zones::HitZoneStore,
    /// Connected-client suppression: skip local AI-enemy materialization / boot
    /// pawn spawn. Both `false` off a connected client (single-player, listen
    /// host, headless).
    pub(crate) suppress_ai_enemies: bool,
    pub(crate) suppress_boot_pawn: bool,
}

/// Segment B of the CPU world install (renderer-free): fog-volume entities,
/// collision world + kinematic movers, classname dispatch, the data script, the
/// data-archetype sweep (incl. player-pawn spawn), the mesh sweep's CPU half
/// (hit-zone store build + clip-index resolve), and the `levelLoad` fire. The
/// sole renderer-coupled step — skinned-model upload + clip-table build — is
/// injected as `upload_mesh_models`, called between the archetype sweep and the
/// clip-index resolve: the windowed caller uploads models and fills the clip
/// tables; a headless caller passes a no-op, leaving clips unresolved (the
/// documented headless shape). Stage durations record into `timings`, matching
/// the windowed log-line-C labels.
pub(crate) fn install_world_cpu(
    handles: WorldInstallHandles<'_>,
    timings: &mut StartupTimings,
    mut upload_mesh_models: impl FnMut(
        &[String],
        &mut crate::scripting_systems::mesh_anim::MeshClipTables,
    ),
) -> WorldInstallProducts {
    let WorldInstallHandles {
        world,
        script_ctx,
        content_root,
        active_level_tags,
        nav_graph,
        collision_world,
        fog_volume_bridge,
        trigger_volume_bridge,
        classname_dispatch,
        script_runtime,
        sequence_registry,
        reaction_registry,
        system_registry,
        modal_stack,
        progress_tracker,
        crossing_detector,
        mesh_clip_tables,
        hit_zone_store,
        suppress_ai_enemies,
        suppress_boot_pawn,
    } = handles;

    // Fog volumes — one entity per record. Runs after the windowed light-bridge
    // populate so the first fog entity-id lands after the light entities.
    {
        let mut registry = script_ctx.registry.borrow_mut();
        fog_volume_bridge.populate_from_level(&mut registry, &world.fog_volumes);
    }

    // Trigger volumes — one entity per record. Kept directly after fog so the
    // fog → trigger → mover entity-id order matches the pre-extraction windowed
    // install. The host-authoritative trigger system evaluates these each tick
    // (windowed / listen host); a headless run populates them but passes no
    // trigger context to `simulate_tick`, so they are inert there.
    {
        let mut registry = script_ctx.registry.borrow_mut();
        trigger_volume_bridge.populate_from_level(&mut registry, &world.trigger_volumes);
    }

    // Collision + kinematic movers. Populate before the first game tick so
    // movement collision is ready.
    collision_world.populate_from_level(world);
    let mover_colliders = crate::runtime_movers::build_loaded_mover_colliders(world);
    if !world.kinematic_geometry.movers.is_empty() {
        let mut registry = script_ctx.registry.borrow_mut();
        match crate::runtime_movers::spawn_loaded_kinematic_movers(&mut registry, world) {
            Ok(spawned) => {
                log::info!(
                    "[Loader] spawned {} kinematic mover entity/entities",
                    spawned.len()
                );
            }
            Err(err) => {
                log::warn!("[Loader] failed to spawn kinematic movers: {err}");
            }
        }
    }
    timings.record("bridges_populated");

    // Classname dispatch: partition player-start placements out (retained for the
    // caller), dispatch the remainder through built-in handlers. The handled set
    // feeds the data-archetype sweep below.
    let all_entities: Vec<crate::scripting::map_entity::MapEntity> =
        world.map_entities.iter().cloned().map(Into::into).collect();
    let (spawn_points, map_entities): (Vec<_>, Vec<_>) = all_entities
        .into_iter()
        .partition(|e| e.classname == PLAYER_START_CLASSNAME);
    let builtin_handled = {
        let mut registry = script_ctx.registry.borrow_mut();
        let handled = apply_classname_dispatch(&map_entities, classname_dispatch, &mut registry);
        if !map_entities.is_empty() {
            log::info!(
                "[Loader] dispatched {total} map entities; {built_in} classname(s) handled by built-in handlers",
                built_in = handled.len(),
                total = map_entities.len(),
            );
        }
        handled
    };
    timings.record("classname_dispatch");

    // Data script runs once at level open. Errors surface as an empty manifest so
    // the level still loads; even levels without one compose against mod-global
    // reactions/crossings. Composed before progress/crossing subscriber rebuild.
    {
        let mut manifest = if let Some(data_script) = &world.data_script {
            script_runtime.run_data_script(data_script, content_root)
        } else {
            LevelManifest::default()
        };
        if world.data_script.is_some() {
            manifest.reactions =
                validate_sequence_primitives(manifest.reactions, sequence_registry);
            // Register level-scope UI trees before the data-script VM context drops
            // and before the manifest is consumed by the data registry.
            modal_stack.register_script_trees(
                std::mem::take(&mut manifest.ui_trees),
                postretro_ui::modal_stack::ScopeTier::Level,
            );
        }
        script_ctx.data_registry.borrow_mut().populate_level(
            manifest.reactions,
            manifest.crossings,
            active_level_tags,
        );
        // CROSSING-CHANNEL INSTALL ORDER (E18): the detector must capture this
        // level's local slot defaults before any connected-client network baseline is
        // applied. A late join then observes the host's persistent state as one real
        // crossing instead of silently arming at the already-replicated value.
        // Network baseline application begins only after world install returns.
        rebuild_reaction_subscribers(progress_tracker, crossing_detector, script_ctx);
    }
    // Bind after subscriber rebuild: `populate_level` has committed the final
    // composed reaction set, so tick dispatch never re-matches a name later.
    let trigger_bindings = build_trigger_bindings(script_ctx);
    timings.record("data_script");

    // Data-archetype sweep: materialize every matching map placement the built-in
    // dispatch did not handle, then spawn the boot pawn from the player starts.
    let descriptors = script_ctx.data_registry.borrow().entities.clone();
    // Read the baked navmesh agent params into an owned local BEFORE borrowing the
    // registry (dispatch borrows it mutably); `None` when the map has no navmesh —
    // the agent then falls back to an engine-default capsule and cannot path.
    let agent_params: Option<postretro_foundation::NavAgentParams> =
        nav_graph.map(|g| g.agent_params());
    // E10 AC #3: mesh models of AI-enemy placements a connected client suppresses.
    // They never spawn a local `MeshComponent`, so the registry-driven model sweep
    // cannot see them; unioned into the mesh model list below so the host-replicated
    // remote enemy is drawable. Empty off a connected client.
    let mut suppressed_enemy_models: Vec<String> = Vec::new();
    let (active_wieldable, active_wieldable_descriptor, first_spawn) = {
        let mut registry = script_ctx.registry.borrow_mut();
        let mut map_entities = map_entities;
        if suppress_ai_enemies {
            // E10 Task 5: a connected client must NOT spawn local authoritative
            // copies of map-placed AI enemies — they are host-authoritative and
            // arrive via snapshots. Filter before dispatch using the descriptor's
            // `ai` block (components do not exist pre-materialization).
            suppressed_enemy_models = suppressed_ai_enemy_mesh_models(&map_entities, &descriptors);
            let kept = filter_out_client_ai_enemies(&map_entities, &descriptors);
            let dropped = map_entities.len() - kept.len();
            if dropped > 0 {
                log::info!(
                    "[Loader] connected client: suppressing {dropped} map-placed AI enemy \
                     placement(s); they arrive via host snapshots"
                );
            }
            map_entities = kept;
        }
        let descriptor_handled = apply_data_archetype_dispatch(
            &map_entities,
            &descriptors,
            &builtin_handled,
            &mut registry,
            agent_params,
        );
        if !descriptor_handled.is_empty() {
            log::info!(
                "[Loader] dispatched {} map entities through descriptor archetypes",
                descriptor_handled.len(),
            );
        }

        // Camera pose is seeded from the first spawn regardless of spawn success
        // (a connected client holds it until the net baseline arms its pawn).
        let first_spawn: Option<(Vec3, Vec3)> = spawn_points.first().map(|e| (e.origin, e.angles));

        // A connected client must NOT spawn a boot pawn (its authoritative pawn
        // arrives as a host-replicated baseline); single-player and the listen host
        // keep spawning theirs.
        let (active_wieldable, active_wieldable_descriptor) = if suppress_boot_pawn {
            log::info!("[Loader] connected client: deferring player spawn to host baseline");
            (None, None)
        } else if !spawn_points.is_empty() {
            let result =
                spawn_from_player_starts(&spawn_points, &descriptors, &mut registry, agent_params);
            (result.active_wieldable, result.active_wieldable_descriptor)
        } else {
            log::info!("[Loader] no player_spawn in map; skipping player spawn");
            (None, None)
        };

        // Attach the `player.health` slot's declared range `[0, max]` now the pawn
        // (and its health component) has materialized. `max` is mod data, so it
        // cannot be declared at `SlotTable` construction.
        if let Some((_, health)) =
            postretro_entities::components::health::pawn_with_health(&registry)
        {
            use postretro_entities::NumericRange;
            if let Err(err) = script_ctx.slot_table.borrow_mut().set_engine_numeric_range(
                "player.health",
                NumericRange {
                    min: 0.0,
                    max: health.max,
                },
            ) {
                log::warn!("[Loader] failed to set player.health range: {err}");
            }
        }

        (active_wieldable, active_wieldable_descriptor, first_spawn)
    };
    timings.record("archetype_sweep");

    // Mesh model sweep, CPU half. Runs AFTER both dispatch sweeps so it sees every
    // mesh entity. Reset the game-side tables, compute the distinct model list
    // (unioning the suppressed remote-enemy models), then the renderer-coupled
    // upload + clip-table build runs via the injected hook, followed by the CPU
    // hit-zone build, clip-index resolve, and zone-multiplier cross-check.
    mesh_clip_tables.clear();
    hit_zone_store.clear();
    let models = {
        let registry = script_ctx.registry.borrow();
        let mut models = crate::distinct_mesh_models(&registry);
        let mut seen: std::collections::HashSet<String> = models.iter().cloned().collect();
        for model in &suppressed_enemy_models {
            if seen.insert(model.clone()) {
                models.push(model.clone());
            }
        }
        models
    };
    upload_mesh_models(&models, mesh_clip_tables);
    for model in &models {
        // Build this model's game-side hit-zone entry by re-loading the glTF
        // independently of the renderer (CPU-only).
        hit_zone_store.insert_from_load(model, content_root);
    }
    crate::resolve_mesh_entity_clips(&mut script_ctx.registry.borrow_mut(), mesh_clip_tables);
    crate::warn_unknown_zone_multipliers(
        &script_ctx.data_registry.borrow().entities,
        hit_zone_store,
    );
    timings.record("model_load");

    // Fire `levelLoad`. Headless fires it too so data-script reactions and
    // crossings compose identically; runs after the clip resolve so a
    // `setAnimationState` reaction sees concrete clip indices. This fire now
    // precedes the windowed light/sprite enrollment passes
    // (`absorb_dynamic_lights`, sprite-collection registration), which run after
    // this function returns — so a `levelLoad` reaction that spawns a dynamic
    // light or emitter is enrolled and renders (previously dropped). Intentional,
    // accepted improvement.
    fire_named_event_with_sequences(
        "levelLoad",
        &script_ctx.data_registry.borrow(),
        sequence_registry,
        reaction_registry,
        system_registry,
        script_ctx,
    );
    timings.record("level_load_event");

    WorldInstallProducts {
        mover_colliders,
        trigger_bindings,
        mover_tick_states: crate::kinematic_mover::MoverTickStateTable::default(),
        active_wieldable,
        active_wieldable_descriptor,
        first_spawn,
        spawn_points,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, VecDeque};
    use std::time::Instant;

    use crate::frame_timing::{FrameRateMeter, FrameTiming};
    use crate::input::InputFocus;
    use crate::scripting;
    use crate::scripting::primitives::register_all;
    use crate::{input, options, scripting_systems, view_feel};
    use postretro_entities::{
        CrossingCondition, CrossingDescriptor, EntityTypeDescriptor, MoverCommand, NamedReaction,
        PrimitiveDescriptor, ProgressDescriptor, ReactionDescriptor, TriggerActivation,
        TriggerFireMode, TriggerVolumeComponent,
    };
    use postretro_entities::{
        ScriptCtx, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue, Transform,
    };
    use postretro_foundation::ModMapEntry;
    use postretro_scripting_core::data_descriptors::RegisteredUiTree;
    use postretro_scripting_core::primitives_registry::PrimitiveRegistry;
    use postretro_scripting_core::reaction_dispatch::ProgressTracker;
    use postretro_scripting_core::runtime::{
        Frontend, MenuCamera, ScriptRuntime, ScriptRuntimeConfig, StagedManifestCommitOutcome,
    };
    use postretro_scripting_core::sequence::SequencedPrimitiveRegistry;
    use postretro_scripting_core::staged_manifest::{
        StagedManifest, StagedManifestBuildResult, StagedManifestBuildStatus,
    };
    use postretro_scripting_core::state_crossings::CrossingDetector;

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
            window_state: None,
            level: None,
            nav_graph: None,
            map_path: None,
            content_root: PathBuf::from("content/dev"),
            exit_result: Ok(()),
            camera: Camera::new(Vec3::ZERO, 0.0, 0.0),
            // Tests exercise level load/unload in the Running state, which touches
            // the session-owned modal stack and the whole script tranche; construct
            // a minimal `Session` inline. The registries (`classname_dispatch`,
            // `scripting.sequence_registry`, `scripting.reaction_registry`,
            // `scripting.system_registry`) are intentionally minimal/empty — these
            // lifecycle tests exercise level load/unload plumbing, not
            // reaction/classname dispatch; the real `Session::build` populates them.
            session: Some(crate::session::Session {
                input_system: input::InputSystem::new(input::default_bindings()),
                gameplay_input_latch: input::GameplayInputLatch::new(),
                ui_dispatch: input::UiDispatch::new(),
                gamepad_system: None,
                input_focus: InputFocus::Gameplay,
                ui_focus: input::UiFocusEngine::new(),
                ui_focus_rects: None,
                ui_input_mode: input::InputMode::default(),
                modal_stack: postretro_ui::modal_stack::ModalStack::new(),
                font_system: postretro_ui::text::build_font_system(),
                scripting: crate::session::ScriptingCore {
                    script_runtime,
                    script_ctx: script_ctx.clone(),
                    sequence_registry: SequencedPrimitiveRegistry::new(),
                    reaction_registry:
                        scripting::reactions::registry::ReactionPrimitiveRegistry::new(),
                    system_registry:
                        scripting::reactions::system_commands::SystemReactionRegistry::new(),
                    player_hud_state: scripting_systems::ui_proxy::PlayerHudStatePublisher::new(
                        script_ctx.clone(),
                    ),
                    #[cfg(feature = "dev-tools")]
                    dev_reload_progress:
                        scripting_systems::reload_progress::DevReloadProgressDriver::new(
                            script_ctx.clone(),
                        ),
                    flash_decay: scripting_systems::flash_decay::FlashDecay::new(
                        script_ctx.clone(),
                    ),
                    vignette_decay: scripting_systems::vignette_decay::VignetteDecay::new(
                        script_ctx.clone(),
                    ),
                    shake_decay: scripting_systems::shake_decay::ShakeDecay::new(
                        script_ctx.clone(),
                    ),
                    input_mode_tracker: scripting_systems::input_mode::InputModeTracker::new(
                        script_ctx.clone(),
                    ),
                },
                presentation_cells:
                    scripting_systems::presentation_cells::PresentationCellStore::new(),
                state_store_lifecycle: Default::default(),
                progress_tracker: ProgressTracker::new(),
                crossing_detector: CrossingDetector::new(),
                classname_dispatch: scripting::builtins::ClassnameDispatch::new(),
                light_bridge: scripting_systems::light_bridge::LightBridge::new(),
                fog_volume_bridge: scripting_systems::fog_volume_bridge::FogVolumeBridge::new(),
                trigger_volume_bridge:
                    scripting_systems::trigger_volume_bridge::TriggerVolumeBridge::new(),
                trigger_system: crate::trigger_system::TriggerSystem::default(),
                emitter_bridge: scripting_systems::emitter_bridge::EmitterBridge::new(),
                particle_render: scripting_systems::particle_render::ParticleRenderCollector::new(),
                mesh_render: scripting_systems::mesh_render::MeshRenderCollector::new(),
                mesh_clip_tables: scripting_systems::mesh_anim::MeshClipTables::new(),
                hit_zone_store: scripting_systems::hit_zones::HitZoneStore::new(),
                player_options: options::PlayerOptions::default(),
                settings_path: None,
                frontend: None,
                net_endpoint: None,
                audio: None,
                #[cfg(feature = "dev-tools")]
                debug_ui: None,
            }),
            crouch_toggle_active: false,
            ai_warned: std::collections::HashSet::new(),
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
            mod_theme_override: Default::default(),
            pending_mode_signal: None,
            pending_menu_toggle: false,
            pending_exit_to_desktop: false,
            ui_focused_id: None,
            particle_live_counts: std::collections::HashMap::new(),
            collision_world: crate::collision::CollisionWorld::new(),
            kinematic_mover_colliders: Vec::new(),
            kinematic_mover_tick_states: crate::kinematic_mover::MoverTickStateTable::default(),
            kinematic_mover_render: crate::runtime_movers::KinematicMoverRenderCollector::new(),
            trigger_bindings: crate::trigger_bindings::TriggerBindingTable::default(),
            active_wieldable: None,
            active_wieldable_descriptor: None,
            client_weapon_state: None,
            client_fire_resolutions: Vec::new(),
            client_predicted_shots: crate::weapon::ClientPredictedShots::new(),
            boot_state: BootState::Running,
            splash_frame: 0,
            pending_level_log: false,
            pending_splash_override: None,
            host_spawn_points: Vec::new(),
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
            pending_session: None,
            #[cfg(feature = "dev-tools")]
            debug_chase_agent: None,
        }
    }

    /// Clone the session-owned `ScriptCtx` handle (cheap `Rc` bump) for tests.
    /// The scripting core lives on `Session`; this keeps the many test reads of
    /// the shared registries one short call away without a borrow fight against
    /// the non-`Clone` session subsystems.
    fn script_ctx(app: &App) -> ScriptCtx {
        app.session
            .as_ref()
            .expect("test app session installed")
            .scripting
            .script_ctx
            .clone()
    }

    fn slot_snapshot(app: &App) -> BTreeMap<String, SlotRecord> {
        let ctx = script_ctx(app);
        let slots = ctx.slot_table.borrow();
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
            ai: None,
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
    ) -> postretro_entities::ScopedReaction {
        postretro_entities::ScopedReaction {
            reaction: progress_reaction(name, tag, 1.0, fire),
            levels: Vec::new(),
        }
    }

    fn scoped_global_set_state(name: &str, value: f32) -> postretro_entities::ScopedReaction {
        postretro_entities::ScopedReaction {
            reaction: NamedReaction {
                name: name.to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "setState".to_string(),
                    tag: None,
                    on_complete: None,
                    args: serde_json::json!({ "slot": "trigger.flag", "value": value }),
                }),
            },
            levels: Vec::new(),
        }
    }

    fn scoped_global_crossing(slot: &str, fire: &str) -> postretro_entities::ScopedCrossing {
        postretro_entities::ScopedCrossing {
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
            network: postretro_entities::ReplicationScope::None,
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

    fn map_light(tag: &str, origin: [f64; 3]) -> postretro_level_loader::MapLight {
        postretro_level_loader::MapLight {
            origin,
            light_type: postretro_level_loader::LightType::Point,
            intensity: 1.0,
            color: [1.0, 0.8, 0.6],
            falloff_model: postretro_level_loader::FalloffModel::InverseSquared,
            falloff_range: 16.0,
            cone_angle_inner: 0.0,
            cone_angle_outer: 0.0,
            cone_direction: [0.0, 0.0, 0.0],
            is_dynamic: false,
            casts_entity_shadows: false,
            animated_slot: None,
            tags: vec![tag.to_string()],
            cell_index: 0,
            shadow_type: postretro_level_loader::ShadowType::StaticLightMap,
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

    fn vertex(position: [f32; 3]) -> postretro_render_data::geometry::WorldVertex {
        postretro_render_data::geometry::WorldVertex {
            position,
            base_uv: [0.0, 0.0],
            normal_oct: [0, 0],
            tangent_packed: [0, 0],
            lightmap_uv: [0, 0],
            lightmap_layer: 0,
        }
    }

    fn level_world(_name: &str, triangle_count: usize) -> postretro_level_loader::LevelWorld {
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

        postretro_level_loader::LevelWorld {
            vertices,
            indices,
            face_meta: Vec::new(),
            cells: vec![postretro_level_loader::CellData {
                bounds_min: Vec3::ZERO,
                bounds_max: Vec3::ONE,
                face_start: 0,
                face_count: 0,
                portal_ref_start: 0,
                portal_ref_count: 0,
                is_solid: false,
                is_exterior: false,
                is_drawable: false,
            }],
            cell_portal_refs: vec![],
            cell_locator_root: postretro_level_loader::CellLocatorChild::Cell(0),
            cell_locator_nodes: vec![],
            portals: Vec::new(),
            has_portals: false,
            texture_names: Vec::new(),
            texture_cache_keys: Default::default(),
            bvh: postretro_render_data::geometry::BvhTree {
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
            direct_sh_delta_volumes: None,
            entity_shadow_lights: vec![],
            shadowmask_atlas: None,
            data_script: None,
            map_entities: Vec::new(),
            kinematic_geometry: postretro_level_loader::KinematicGeometry::default(),
            trigger_volumes: Vec::new(),
            fog_volumes: Vec::new(),
            fog_pixel_scale: 4,
            initial_gravity: -9.8,
            fog_cell_masks: None,
            navmesh: None,
            cell_draw_index: None,
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
        let ctx = script_ctx(app);
        ctx.data_registry.borrow_mut().populate_level(
            vec![named_reaction(fixture.reaction_name)],
            Vec::new(),
            &[],
        );

        let lights = (0..fixture.light_count)
            .map(|i| map_light(fixture.light_tag, [i as f64, 2.0, 3.0]))
            .collect::<Vec<_>>();
        let fog_records = (0..fixture.fog_count)
            .map(|i| fog_record(fixture.fog_tag, i as f32 * 4.0))
            .collect::<Vec<_>>();
        {
            let session = app.session.as_mut().expect("test app session installed");
            session
                .light_bridge
                .populate_from_level(&lights, &mut ctx.registry.borrow_mut(), 0);
            session
                .fog_volume_bridge
                .populate_from_level(&mut ctx.registry.borrow_mut(), &fog_records);
        }

        if let Some(world) = app.level.as_ref() {
            app.collision_world.populate_from_level(world);
        }
    }

    #[test]
    fn unload_level_preserves_slot_table_and_entity_type_registry() {
        let mut app = test_app();
        script_ctx(&app)
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
                        network: postretro_entities::ReplicationScope::None,
                    }),
                )],
            )
            .unwrap();
        script_ctx(&app)
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
        app.session
            .as_mut()
            .expect("test app session installed")
            .presentation_cells
            .write(
                "level_panel".to_string(),
                "count".to_string(),
                SlotValue::Number(11.0),
            );
        assert!(
            !app.session
                .as_ref()
                .expect("test app session installed")
                .presentation_cells
                .snapshot()
                .is_empty()
        );

        let slots_before = slot_snapshot(&app);
        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .replace_maps(vec![ModMapEntry {
                id: "e1m1".to_string(),
                path: "maps/e1m1.prl".to_string(),
                name: "Entryway".to_string(),
                tags: vec!["campaign".to_string()],
            }]);
        let data_before = {
            let ctx = script_ctx(&app);
            let data_registry = ctx.data_registry.borrow();
            (data_registry.entities.clone(), data_registry.maps.clone())
        };

        app.unload_level();

        assert_eq!(slot_snapshot(&app), slots_before);
        let data_after = {
            let ctx = script_ctx(&app);
            let data_registry = ctx.data_registry.borrow();
            (data_registry.entities.clone(), data_registry.maps.clone())
        };
        assert_eq!(data_after, data_before);
        assert!(app.boot_state == BootState::Frontend);
        assert!(app.level.is_none());
        assert_eq!(app.script_time, 0.0);
        assert_eq!(app.anim_time, 0.0);
        assert!(
            app.session
                .as_ref()
                .expect("test app session installed")
                .presentation_cells
                .snapshot()
                .is_empty()
        );
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
        assert_eq!(
            app.session
                .as_ref()
                .expect("test app session installed")
                .light_bridge
                .light_count(),
            2
        );
        assert_eq!(
            app.session
                .as_ref()
                .expect("test app session installed")
                .fog_volume_bridge
                .entity_count(),
            2
        );
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

        let ctx = script_ctx(&app);
        let data_registry = ctx.data_registry.borrow();
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

        let session = app.session.as_ref().expect("test app session installed");
        assert_eq!(session.light_bridge.light_count(), 1);
        let light_id = session.light_bridge.entity_for_map_index(0).unwrap();
        assert_eq!(
            ctx.registry.borrow().get_tags(light_id).unwrap(),
            &["combat_only"],
        );

        assert_eq!(session.fog_volume_bridge.entity_count(), 1);
        assert_eq!(session.fog_volume_bridge.cached_aabb_count(), 1);
        assert!(session.fog_volume_bridge.active_aabbs().is_empty());
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
        script_ctx(&app)
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
        script_ctx(&app)
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
        app.session
            .as_mut()
            .unwrap()
            .modal_stack
            .registry_mut()
            .register(
                "mainMenu",
                postretro_ui::demo::build_frontend_menu_descriptor(),
                postretro_ui::modal_stack::ScopeTier::Mod,
                false,
            );
        app.session.as_mut().unwrap().frontend = Some(Frontend {
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

        assert_eq!(
            app.session.as_mut().unwrap().modal_stack.active_name(),
            Some("mainMenu")
        );
        assert_eq!(
            app.session.as_mut().unwrap().modal_stack.top_capture_mode(),
            postretro_ui::descriptor::CaptureMode::Capture,
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
        app.session.as_mut().unwrap().frontend = Some(Frontend {
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
            app.session.as_mut().unwrap().modal_stack.active_name(),
            Some(postretro_ui::demo::FRONTEND_MENU_NAME),
            "unknown mod frontend menus must reveal the engine fallback",
        );
        assert_eq!(
            app.session.as_mut().unwrap().modal_stack.top_capture_mode(),
            postretro_ui::descriptor::CaptureMode::Capture
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
        let mut app = test_app();
        app.boot_state = BootState::Frontend;
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
        app.session
            .as_mut()
            .unwrap()
            .modal_stack
            .registry_mut()
            .register(
                "oldMenu",
                postretro_ui::demo::build_frontend_menu_descriptor(),
                postretro_ui::modal_stack::ScopeTier::Mod,
                false,
            );
        app.session.as_mut().unwrap().frontend = Some(Frontend {
            menu_tree: "oldMenu".to_string(),
            background_level: None,
            camera: MenuCamera {
                position: [0.0, 0.0, 0.0],
                yaw: 0.0,
                pitch: 0.0,
            },
        });
        app.present_frontend_menu();
        assert_eq!(
            app.session.as_mut().unwrap().modal_stack.active_name(),
            Some("oldMenu")
        );

        let staged = StagedManifestBuildResult {
            generation: 4,
            mod_root: PathBuf::from("content/dev"),
            status: StagedManifestBuildStatus::Built(Box::new(StagedManifest {
                name: "Replacement".to_string(),
                entities: Vec::new(),
                maps: Vec::new(),
                reactions: Vec::new(),
                crossings: Vec::new(),
                ui_trees: vec![RegisteredUiTree {
                    name: "newMenu".to_string(),
                    tree: postretro_ui::demo::build_frontend_menu_descriptor(),
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
            })),
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
            app.session.as_mut().unwrap().modal_stack.active_name(),
            Some("newMenu"),
            "staged replacement updates the active frontend modal clone",
        );

        let omitted = StagedManifestBuildResult {
            generation: 5,
            mod_root: PathBuf::from("content/dev"),
            status: StagedManifestBuildStatus::NoStartScript,
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
            app.session.as_mut().unwrap().modal_stack.active_name(),
            Some(postretro_ui::demo::FRONTEND_MENU_NAME),
            "staged omission replaces the active frontend modal with the engine fallback",
        );
        assert_eq!(
            app.session.as_mut().unwrap().modal_stack.top_capture_mode(),
            postretro_ui::descriptor::CaptureMode::Capture
        );
    }

    #[test]
    fn no_backdrop_frontend_button_activation_dispatches_load_command() {
        use postretro_ui::tree::{FocusNeighbors, FocusRect, FocusRectList, NodeInteraction};

        let mut app = test_app();
        app.boot_state = BootState::Frontend;
        crate::scripting::reactions::system_commands::register_system_reaction_primitives(
            &mut app.session.as_mut().unwrap().scripting.system_registry,
        );
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
        app.present_frontend_menu();
        app.session.as_mut().unwrap().ui_focus_rects = Some(FocusRectList {
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
        script_ctx(&app)
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
            app.session.as_mut().unwrap().modal_stack.is_empty(),
            "frontend activation clears the menu before gameplay load starts",
        );
    }

    #[test]
    fn catalog_tags_are_available_on_in_flight_load_before_data_script_runs() {
        let mut app = test_app();
        app.boot_state = BootState::Frontend;
        app.content_root = PathBuf::from("content/mod");
        script_ctx(&app)
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
            script_ctx(&app).data_registry.borrow().reactions.is_empty(),
            "data script has not run while load metadata is already available",
        );

        drop_in_flight_worker(&mut app);
    }

    #[test]
    fn catalog_level_install_retains_active_tags_from_in_flight_load() {
        let mut app = test_app();
        app.boot_state = BootState::Frontend;
        app.content_root = PathBuf::from("content/mod");
        script_ctx(&app)
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
        app.session
            .as_mut()
            .unwrap()
            .modal_stack
            .registry_mut()
            .register(
                "deathScreen",
                postretro_ui::demo::build_frontend_menu_descriptor(),
                postretro_ui::modal_stack::ScopeTier::Mod,
                false,
            );
        app.session
            .as_mut()
            .unwrap()
            .modal_stack
            .push_named("deathScreen", None);

        script_ctx(&app).system_commands.push(
            postretro_entities::SystemReactionCommand::LoadLevel {
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
            app.session.as_mut().unwrap().modal_stack.is_empty(),
            "starting gameplay clears the initiating modal before controls return",
        );
    }

    #[test]
    fn restart_level_system_command_requeues_retained_active_source() {
        let mut app = test_app();
        app.active_level_source = Some(LevelSource::Path(PathBuf::from(
            "content/dev/maps/raw-dev-map.prl",
        )));

        script_ctx(&app)
            .system_commands
            .push(postretro_entities::SystemReactionCommand::RestartLevel);
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
        app.session
            .as_mut()
            .unwrap()
            .modal_stack
            .registry_mut()
            .register(
                "mainMenu",
                postretro_ui::demo::build_frontend_menu_descriptor(),
                postretro_ui::modal_stack::ScopeTier::Mod,
                false,
            );
        app.session.as_mut().unwrap().frontend = Some(Frontend {
            menu_tree: "mainMenu".to_string(),
            background_level: Some("menuBackdrop".to_string()),
            camera: MenuCamera {
                position: [0.0, 0.0, 0.0],
                yaw: 0.0,
                pitch: 0.0,
            },
        });

        script_ctx(&app)
            .system_commands
            .push(postretro_entities::SystemReactionCommand::ReturnToFrontend);
        app.dispatch_system_commands();

        assert_eq!(
            app.session.as_mut().unwrap().modal_stack.active_name(),
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
        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .replace_global_reactions(vec![scoped_global_progress("waveDone", "wave1", "powerOn")]);
        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .replace_global_crossings(vec![scoped_global_crossing("test.health", "healthLow")]);

        if app.has_installed_level() {
            script_ctx(&app)
                .data_registry
                .borrow_mut()
                .recompose_active_sets(&app.active_level_tags);
            app.rebuild_active_reaction_subscribers();
        }

        let ctx = script_ctx(&app);
        let registry = ctx.data_registry.borrow();
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
        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .replace_global_reactions(vec![scoped_global_progress("waveDone", "wave1", "powerOn")]);
        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .replace_global_crossings(vec![scoped_global_crossing("test.health", "healthLow")]);
        {
            let ctx = script_ctx(&app);
            let mut entities = ctx.registry.borrow_mut();
            let id = entities.spawn(Transform::default());
            entities.set_tags(id, vec!["wave1".to_string()]).unwrap();
        }
        script_ctx(&app)
            .slot_table
            .borrow_mut()
            .insert("test.health".to_string(), number_slot(75.0))
            .expect("test slot should be vacant");

        if app.has_installed_level() {
            script_ctx(&app)
                .data_registry
                .borrow_mut()
                .recompose_active_sets(&app.active_level_tags);
            app.rebuild_active_reaction_subscribers();
        }

        assert_eq!(
            app.session
                .as_mut()
                .expect("test app session installed")
                .progress_tracker
                .on_entity_killed(&["wave1".to_string()]),
            vec!["powerOn".to_string()],
        );
        script_ctx(&app)
            .slot_table
            .borrow_mut()
            .get_mut("test.health")
            .expect("test slot should exist")
            .value = Some(SlotValue::Number(25.0));
        let ctx = script_ctx(&app);
        assert_eq!(
            app.session
                .as_mut()
                .expect("test app session installed")
                .crossing_detector
                .detect(&ctx.slot_table.borrow()),
            vec!["healthLow".to_string()],
        );
    }

    // Regression: staged global reaction reload left trigger bindings pointing at the prior active set.
    #[test]
    fn staged_recomposition_rebinds_trigger_commands_to_the_committed_reactions() {
        let mut app = test_app();
        app.level = Some(level_world("trigger_reload_level", 1));
        let trigger = {
            let ctx = script_ctx(&app);
            let mut entities = ctx.registry.borrow_mut();
            let id = entities.spawn(Transform::default());
            entities
                .set_component(
                    id,
                    TriggerVolumeComponent::new(
                        TriggerActivation::Touch,
                        String::new(),
                        "plate_pressed".to_string(),
                        String::new(),
                        MoverCommand::Start,
                        TriggerFireMode::Multiple,
                        0.0,
                        true,
                    ),
                )
                .expect("trigger component attaches");
            id
        };
        script_ctx(&app)
            .slot_table
            .borrow_mut()
            .insert("trigger.flag".to_string(), number_slot(0.0))
            .expect("trigger fixture slot should be vacant");

        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .replace_global_reactions(vec![scoped_global_set_state("plate_pressed", 1.0)]);
        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .recompose_active_sets(&app.active_level_tags);
        app.rebuild_active_reaction_subscribers();
        app.rebuild_active_trigger_bindings();
        {
            let ctx = script_ctx(&app);
            let mut entities = ctx.registry.borrow_mut();
            let mut slots = ctx.slot_table.borrow_mut();
            assert!(
                app.trigger_bindings
                    .execute(
                        trigger,
                        crate::trigger_bindings::TriggerBindingEdge::Enter,
                        &mut entities,
                        &mut slots,
                    )
                    .residual()
                    .is_none(),
                "the setState-only binding has no app-side residual"
            );
        }
        assert_eq!(
            script_ctx(&app)
                .slot_table
                .borrow()
                .get("trigger.flag")
                .and_then(|record| record.value.clone()),
            Some(SlotValue::Number(1.0)),
        );

        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .replace_global_reactions(vec![scoped_global_set_state("plate_pressed", 2.0)]);
        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .recompose_active_sets(&app.active_level_tags);
        app.rebuild_active_reaction_subscribers();
        app.rebuild_active_trigger_bindings();
        {
            let ctx = script_ctx(&app);
            let mut entities = ctx.registry.borrow_mut();
            let mut slots = ctx.slot_table.borrow_mut();
            assert!(
                app.trigger_bindings
                    .execute(
                        trigger,
                        crate::trigger_bindings::TriggerBindingEdge::Enter,
                        &mut entities,
                        &mut slots,
                    )
                    .residual()
                    .is_none(),
                "the replacement setState-only binding has no app-side residual"
            );
        }
        assert_eq!(
            script_ctx(&app)
                .slot_table
                .borrow()
                .get("trigger.flag")
                .and_then(|record| record.value.clone()),
            Some(SlotValue::Number(2.0)),
            "the post-reload binding must not retain the old command",
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

    // E10 Task 5: the install path keys AI-enemy spawn suppression off
    // `is_connected_client()`. Prove the role gate that drives the
    // `filter_out_client_ai_enemies` branch in `install_level_payload` resolves
    // correctly for each role — single-player and listen host keep every
    // placement (no suppression), only the connected client suppresses.
    #[test]
    fn ai_enemy_suppression_gate_is_connected_client_only() {
        use std::net::{Ipv4Addr, SocketAddr};

        use crate::netcode::{NetEndpoint, NetRole};

        // Single-player: net inert, no suppression.
        let mut app = test_app();
        app.session.as_mut().unwrap().net_endpoint = None;
        assert!(
            !app.is_connected_client(),
            "single-player must keep map-placed AI enemies (no suppression)"
        );

        // Listen host: authoritative, keeps every placement and replicates them.
        app.session.as_mut().unwrap().net_endpoint = Some(
            NetEndpoint::from_role(&NetRole::Host { port: 0 })
                .expect("host endpoint constructs")
                .expect("host role yields an endpoint"),
        );
        assert!(
            !app.is_connected_client(),
            "listen host must keep map-placed AI enemies (it owns + replicates them)"
        );

        // Connected client: the only role that suppresses the local spawn.
        app.session.as_mut().unwrap().net_endpoint = Some(
            NetEndpoint::from_role(&NetRole::Connect {
                addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 1)),
            })
            .expect("client endpoint constructs")
            .expect("connect role yields an endpoint"),
        );
        assert!(
            app.is_connected_client(),
            "connected client must suppress local authoritative AI-enemy spawns"
        );
    }

    #[test]
    fn windowed_install_assigns_light_entity_ids_before_fog_entity_ids() {
        // Drift guard for segment B's fog-after-preexisting-lights behavior: given
        // light entities already registered (as the windowed caller does BETWEEN
        // segment A and segment B), segment B's fog-volume populate must land its
        // ids after them — both bridges key dirty tracking on `EntityId`. This test
        // reconstructs that sequence directly rather than driving
        // `install_level_payload`, so it does NOT guard the real caller's call
        // order — a future reorder there would not fail this test.
        let mut app = test_app();
        let ctx = script_ctx(&app);

        let mut world = level_world(FIXTURE_MAP_A, 1);
        world.fog_volumes = vec![fog_record("fog", 0.0), fog_record("fog", 4.0)];

        // Windowed step run between segments A and B: light entities take registry
        // ids first.
        let lights = vec![
            map_light("light", [0.0, 2.0, 0.0]),
            map_light("light", [4.0, 2.0, 0.0]),
        ];
        {
            let session = app.session.as_mut().expect("test app session installed");
            session
                .light_bridge
                .populate_from_level(&lights, &mut ctx.registry.borrow_mut(), 0);
        }

        // Segment B creates the fog-volume entities first thing, after the light
        // populate above — mirroring the windowed call order.
        let mut timings = StartupTimings::new();
        {
            let session = app.session.as_mut().expect("test app session installed");
            let handles = WorldInstallHandles {
                world: &world,
                script_ctx: &ctx,
                content_root: std::path::Path::new("content/dev"),
                active_level_tags: &[],
                nav_graph: None,
                collision_world: &mut app.collision_world,
                fog_volume_bridge: &mut session.fog_volume_bridge,
                trigger_volume_bridge: &mut session.trigger_volume_bridge,
                classname_dispatch: &session.classname_dispatch,
                script_runtime: &session.scripting.script_runtime,
                sequence_registry: &session.scripting.sequence_registry,
                reaction_registry: &session.scripting.reaction_registry,
                system_registry: &session.scripting.system_registry,
                modal_stack: &mut session.modal_stack,
                progress_tracker: &mut session.progress_tracker,
                crossing_detector: &mut session.crossing_detector,
                mesh_clip_tables: &mut session.mesh_clip_tables,
                hit_zone_store: &mut session.hit_zone_store,
                suppress_ai_enemies: false,
                suppress_boot_pawn: false,
            };
            // No-op mesh hook: headless-shaped, no renderer to upload models.
            let _ = install_world_cpu(handles, &mut timings, |_models, _clip_tables| {});
        }

        // Fresh registry (no despawns): `to_raw()` low bits are the allocation
        // index, so id order is creation order.
        let registry = ctx.registry.borrow();
        let max_light_id = registry
            .iter_with_kind(postretro_entities::ComponentKind::Light)
            .map(|(id, _)| id.to_raw())
            .max()
            .expect("light entities were populated");
        let min_fog_id = registry
            .iter_with_kind(postretro_entities::ComponentKind::FogVolume)
            .map(|(id, _)| id.to_raw())
            .min()
            .expect("fog entities were populated");
        assert!(
            max_light_id < min_fog_id,
            "light entity ids (max {max_light_id}) must precede fog entity ids (min {min_fog_id})",
        );
    }
}
