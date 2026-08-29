//! Runtime level lifecycle state-machine helpers.
//! See: context/lib/boot_sequence.md §1

#[path = "lifecycle_net.rs"]
mod lifecycle_net;
#[path = "lifecycle_world_cpu.rs"]
mod lifecycle_world_cpu;

#[cfg(test)]
pub(crate) use lifecycle_world_cpu::install_descriptor_player_health_range;
pub(crate) use lifecycle_world_cpu::install_world_cpu;

use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;

use glam::Vec3;
use winit::event_loop::ActiveEventLoop;

use crate::frame_timing::InterpolableState;
use crate::render;
use crate::scripting::builtins::data_archetype::{
    ProjectileSpriteCollection, projectile_presentation_assets,
};
use crate::scripting::builtins::descriptor_materializes_ai_enemy;
use crate::startup::{
    BootState, InFlightLevelLoad, LevelLoadEntry, LevelRequest, LevelSource, LoadOutcome,
    StartupTimings, spawn_level_worker,
};
use crate::trigger_bindings::TriggerBindingTable;
use crate::trigger_pools::{
    TriggerPoolInstallReport, TriggerPoolSeedPolicy, install_trigger_pools,
};
use crate::{App, weapon};
use postretro_scripting_core::reaction_dispatch::validate_sequence_primitives;

#[cfg(test)]
use crate::camera::Camera;

pub(crate) const FRONTEND_CLEAR_COLOR: render::ClearColor = render::ClearColor {
    r: 0.015,
    g: 0.018,
    b: 0.024,
    a: 1.0,
};

#[cfg(feature = "dev-tools")]
const DEV_LEVEL_CYCLE_TARGET: &str = "content/dev/maps/combat-demo.prl";

#[derive(Debug, Clone)]
struct SpriteCollectionCandidate {
    collection: String,
    lifetime: Option<f32>,
    emissive: f32,
    frame_duration_ms: Option<f32>,
    source: String,
}

impl From<ProjectileSpriteCollection> for SpriteCollectionCandidate {
    fn from(value: ProjectileSpriteCollection) -> Self {
        Self {
            collection: value.collection,
            lifetime: value.lifetime,
            emissive: value.emissive,
            frame_duration_ms: value.frame_duration_ms,
            source: value.source,
        }
    }
}

fn resolve_sprite_collection_draw_contract(
    collection: &str,
    candidates: &[SpriteCollectionCandidate],
    frame_count: usize,
) -> Result<(f32, f32), String> {
    let mut lifetime: Option<(f32, &str)> = None;
    let mut emissive: Option<(f32, &str)> = None;

    for candidate in candidates {
        let required_lifetime = candidate
            .frame_duration_ms
            .map_or(candidate.lifetime, |ms| {
                Some(ms / 1_000.0 * frame_count.max(1) as f32)
            });
        if let Some(required) = required_lifetime {
            if let Some((chosen, chosen_source)) = lifetime
                && chosen.to_bits() != required.to_bits()
            {
                return Err(format!(
                    "collection `{collection}` has conflicting loop periods from `{chosen_source}` ({chosen}s) and `{}` ({required}s)",
                    candidate.source,
                ));
            }
            lifetime.get_or_insert((required, &candidate.source));
        }

        if let Some((chosen, chosen_source)) = emissive
            && chosen.to_bits() != candidate.emissive.to_bits()
        {
            return Err(format!(
                "collection `{collection}` has conflicting emissive strengths from `{chosen_source}` ({chosen}) and `{}` ({})",
                candidate.source, candidate.emissive,
            ));
        }
        emissive.get_or_insert((candidate.emissive, &candidate.source));
    }

    Ok((
        lifetime.map_or(1.0, |(value, _)| value),
        emissive.map_or(0.0, |(value, _)| value),
    ))
}

fn map_billboard_sprite_collections(
    entities: &[postretro_level_format::map_entity::MapEntityRecord],
) -> std::collections::HashSet<String> {
    entities
        .iter()
        .filter(|entity| entity.classname == "billboard_emitter")
        .map(|entity| {
            entity
                .key_values
                .iter()
                .rev()
                .find_map(|(key, value)| (key == "sprite").then_some(value.as_str()))
                .filter(|sprite| !sprite.is_empty())
                .unwrap_or("smoke")
                .to_string()
        })
        .collect()
}

fn level_source_for_load_entry(entry: &LevelLoadEntry) -> LevelSource {
    if let Some(id) = entry.catalog_id.as_ref() {
        LevelSource::Catalog(id.clone())
    } else {
        LevelSource::Path(PathBuf::from(&entry.path))
    }
}

/// Stable logical identity for the level parity gate. Catalog loads use their
/// author-declared handle; raw paths are lexically normalized relative to the
/// content root (or retain an absolute identity when outside it). A `path:` tag
/// keeps the two addressing modes distinct even when they name the same file.
pub(crate) fn level_identity(source: &LevelSource, content_root: &Path) -> String {
    match source {
        LevelSource::Catalog(id) => id.clone(),
        LevelSource::Path(path) => {
            let cwd = std::env::current_dir().unwrap_or_default();
            let absolute = lexical_normalize(if path.is_absolute() {
                path.clone()
            } else {
                cwd.join(path)
            });
            let root = lexical_normalize(if content_root.is_absolute() {
                content_root.to_path_buf()
            } else {
                cwd.join(content_root)
            });
            let normalized = absolute.strip_prefix(&root).unwrap_or(&absolute);
            format!("path:{}", normalized.to_string_lossy().replace('\\', "/"))
        }
    }
}

fn lexical_normalize(path: PathBuf) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// Bind PRL member-light links only after the complete mover spawn result is
/// available. This is intentionally windowed: [`App::install_level_payload`]
/// owns both the map-light bridge and the mover spawn result, while the
/// renderer-free installer owns neither bridge nor a reason to bind lights.
///
/// A failed all-or-nothing mover spawn leaves every member unbound. The next
/// level install builds a new bridge and registry, then calls this pass again;
/// raw entity ids never survive a reload.
fn resolve_carried_light_bindings_after_mover_spawn(
    geometry: &postretro_level_loader::KinematicGeometry,
    spawned_mover_entities: &[postretro_entities::EntityId],
    light_bridge: &crate::scripting_systems::light_bridge::LightBridge,
    registry: &mut postretro_entities::EntityRegistry,
) {
    use postretro_entities::components::light::{LightCarrier, LightComponent};

    // The spawner is all-or-nothing and returns ids order-aligned to mover
    // records. Never zip a partial result: that could bind a light to the
    // wrong mover after a load fault.
    let mover_entity_by_id = if spawned_mover_entities.len() == geometry.movers.len() {
        geometry
            .movers
            .iter()
            .zip(spawned_mover_entities.iter().copied())
            .map(|(mover, entity)| (mover.mover_id, entity))
            .collect::<std::collections::HashMap<_, _>>()
    } else {
        log::warn!(
            "[Loader] kinematic mover spawn count did not match level geometry; carried lights remain unbound"
        );
        std::collections::HashMap::new()
    };

    for mover in &geometry.movers {
        for member in &mover.carried_lights {
            let Some(&mover_entity) = mover_entity_by_id.get(&mover.mover_id) else {
                log::warn!(
                    "[Loader] carried AlphaLight {} could not bind: mover {} was not spawned",
                    member.alpha_light_index,
                    mover.mover_id,
                );
                continue;
            };
            let Some(light_entity) =
                light_bridge.entity_for_map_index(member.alpha_light_index as usize)
            else {
                log::warn!(
                    "[Loader] carried AlphaLight {} could not bind: map light entity is unavailable",
                    member.alpha_light_index,
                );
                continue;
            };
            let Ok(mut light) = registry
                .get_component::<LightComponent>(light_entity)
                .cloned()
            else {
                log::warn!(
                    "[Loader] carried AlphaLight {} could not bind: light component is unavailable",
                    member.alpha_light_index,
                );
                continue;
            };
            light.carrier = Some(LightCarrier {
                mover_entity,
                local_offset: member.local_offset,
            });
            if let Err(error) = registry.set_component(light_entity, light) {
                log::warn!(
                    "[Loader] carried AlphaLight {} could not bind: {error}",
                    member.alpha_light_index,
                );
            }
        }
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
        session
            .scripting
            .slot_accumulator_bindings
            .rebuild(&session.scripting.script_ctx);
    }

    /// Rebind inline `setState` IR after the active reaction set changes. The
    /// app-side command queue carries raw JSON across the entities boundary;
    /// only this binary-side table holds `BoundProgram<StoreScope>` values and
    /// known rejected command identities.
    pub(crate) fn rebuild_active_system_reaction_bindings(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let script_ctx = session.scripting.script_ctx.clone();
        session
            .scripting
            .system_reaction_ir_bindings
            .rebuild(&script_ctx.data_registry.borrow(), &script_ctx);
    }

    /// Rebind trigger events after staged mod-init recomposes the active reaction
    /// set. Tick dispatch holds bound commands, never reaction names, so it must
    /// be refreshed alongside the other active reaction consumers.
    pub(crate) fn rebuild_active_trigger_bindings(&mut self) {
        let bindings = {
            let Some(session) = self.session.as_ref() else {
                return;
            };
            let mut bindings = build_trigger_bindings(
                &session.scripting.script_ctx,
                session.scripting.command_diagnostics.clone(),
                session.scripting.spawn_context.clone(),
            );
            {
                let registry = session.scripting.script_ctx.registry.borrow();
                let data_registry = session.scripting.script_ctx.data_registry.borrow();
                bindings.install_manifest_events(
                    &registry,
                    &data_registry,
                    &session.scripting.script_ctx,
                );
            }
            bindings
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

    pub(super) fn run_loading_frame(
        &mut self,
        event_loop: &ActiveEventLoop,
        frame_dt: f32,
    ) -> bool {
        // A worker may take longer than the netcode timeout. Poll the live
        // endpoint before checking its channel, without touching level state.
        let _ = self.poll_world_less_transport(frame_dt);
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
        let join_seed = {
            let session = self
                .session
                .as_ref()
                .expect("session installed before level install");
            crate::scripting::state_persistence::join_seed_from_persisted_state(
                session.persisted_state.as_ref(),
                session.player_options.player_id,
            )
        };
        if let Some(endpoint) = self
            .session
            .as_mut()
            .expect("session installed before level install")
            .net_endpoint
            .as_mut()
        {
            let level_content_digest =
                crate::runtime_movers::level_content_digest(&world.kinematic_geometry, &world);
            let source = self
                .active_level_source
                .as_ref()
                .expect("active level source retained before parity installation");
            endpoint.set_join_seed(join_seed);
            endpoint.set_level_parity(Some((
                level_identity(source, &self.content_root),
                level_content_digest,
            )));
            endpoint.set_relevel_catalog_id(match source {
                LevelSource::Catalog(id) => Some(id.clone()),
                LevelSource::Path(_) => None,
            });
        }
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
        if self
            .session
            .as_mut()
            .expect("session installed before level install")
            .scripting
            .script_runtime
            .install_deferred_mesh_descriptors(&script_ctx)
        {
            log::info!(
                "[Scripting] installed deferred presentation descriptors for the next level model sweep"
            );
        }
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

        let (map_lights, map_light_influences, baked_light_descriptors, fgd_sample_float_count) = {
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
                world.lights.clone(),
                world.light_influences.clone(),
                world
                    .sh_volume
                    .as_ref()
                    .map(|section| section.animation_descriptors.clone())
                    .unwrap_or_default(),
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
                .populate_from_level_with_influences(
                    &map_lights,
                    &map_light_influences,
                    &baked_light_descriptors,
                    &mut registry,
                    fgd_sample_float_count,
                );
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
                crate::scripting_systems::hit_zones::ModelLoadWarningOwner::Renderer
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
            command_diagnostics: session.scripting.command_diagnostics.clone(),
            mover_auto_close_ms: session.scripting.mover_auto_close_ms,
            spawn_context: session.scripting.spawn_context.clone(),
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
            slot_accumulator_bindings: &mut session.scripting.slot_accumulator_bindings,
            impact_policy_runtime: &mut session.scripting.impact_policy_runtime,
            mesh_clip_tables: &mut session.mesh_clip_tables,
            hit_zone_store: &mut session.hit_zone_store,
            trigger_pool_policy: self.session_boot_config.windowed_trigger_pool_policy(),
            suppress_ai_enemies: suppress,
            suppress_boot_pawn: suppress,
            local_carried_loadout: session
                .seat_table
                .as_ref()
                .and_then(|seats| seats.carried_state(postretro_foundation::Seat(0)))
                .cloned(),
        };
        let products = install_world_cpu(
            handles,
            &mut self.level_timings,
            upload_mesh_models,
            |spawn_points| {
                let Some(seats) = session.seat_table.as_mut() else {
                    return;
                };
                let local_pawn = {
                    let registry = script_ctx.registry.borrow();
                    crate::capture_player_spawn_placements(&registry, spawn_points, seats);
                    registry.local_player_pawn()
                };
                if let Some(pawn) = local_pawn {
                    seats.bind_pawn(
                        &mut script_ctx.registry.borrow_mut(),
                        postretro_foundation::Seat(0),
                        pawn,
                    );
                }
            },
        );

        // Lights are installed before movers. This synchronous pass is the
        // only windowed binding funnel, so it runs before the first
        // LightBridge::update on both initial loads and later reloads.
        {
            let level = self
                .level
                .as_ref()
                .expect("level installed before carried-light resolution");
            let mut registry = script_ctx.registry.borrow_mut();
            resolve_carried_light_bindings_after_mover_spawn(
                &level.kinematic_geometry,
                &products.spawned_mover_entities,
                &session.light_bridge,
                &mut registry,
            );
        }

        // `levelLoad` may already have queued system commands during the CPU
        // install. Bind the final composed reaction set before that queue is
        // next drained, so an inline setState IR is evaluated rather than
        // treated as a literal JSON value.
        self.rebuild_active_system_reaction_bindings();

        self.kinematic_mover_colliders = products.mover_colliders;
        self.trigger_bindings = products.trigger_bindings;
        self.trigger_pool_report = products.trigger_pool_report;
        // Retain spawn-point placements for the host's runtime seat-accept path:
        // each accepted client's descriptor pawn materializes from them later.
        self.host_spawn_points = products.spawn_points;

        // The boot pawn exists before the regular host snapshot cadence (and in
        // single-player there is no `WeaponOwners` table at all), so establish its
        // shadow-only third-person weapon prop at install time. The model sweep above
        // has already built both CPU tables; resolve only this changed pawn through
        // the standard socket-binding path.
        let local_pawn = script_ctx.registry.borrow().local_player_pawn();
        let descriptors = script_ctx.data_registry.borrow().entities.clone();
        if let Some(pawn) = local_pawn {
            let session = self
                .session
                .as_mut()
                .expect("session installed before local weapon presentation install");
            let mut registry = script_ctx.registry.borrow_mut();
            if crate::netcode::synchronize_weapon_attachment_for_pawn(
                &mut registry,
                pawn,
                &descriptors,
                &session.hit_zone_store,
            ) {
                crate::resolve_mesh_entity_bindings_for_entities(
                    &mut registry,
                    &session.mesh_clip_tables,
                    &session.hit_zone_store,
                    [pawn],
                );
            }
        }

        // Register host-authoritative map entities and PRL-loaded movers for outbound
        // replication. Host-gated (a no-op off a listen host) and reload-safe; each
        // takes its own registry borrow.
        self.host_register_map_enemies_after_install();
        self.host_register_world_items_after_install();
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
        // registry — map-spawned and descriptor-spawned alike — plus descriptor
        // projectile bodies/trails and the weapon impact collection. A projectile
        // materializes only after a fire command, so descriptor discovery is the
        // required level-install path that keeps GPU uploads renderer-owned.
        if let Some(renderer) = self.renderer.as_mut() {
            use postretro_entities::{ComponentKind, ComponentValue};
            let texture_root = self.content_root.join("textures");
            let map_billboard_collections = self
                .level
                .as_ref()
                .map(|world| map_billboard_sprite_collections(&world.map_entities))
                .unwrap_or_default();
            let projectile_sprites = {
                let data_registry = script_ctx.data_registry.borrow();
                projectile_presentation_assets(&data_registry.entities).1
            };
            let registry = script_ctx.registry.borrow();
            let particle_render = &mut self
                .session
                .as_mut()
                .expect("session installed before level install")
                .particle_render;
            let mut collections: Vec<(String, Vec<SpriteCollectionCandidate>)> = Vec::new();
            let mut collection_indices = std::collections::HashMap::<String, usize>::new();
            {
                let mut add_candidate = |candidate: SpriteCollectionCandidate| {
                    if candidate.collection.is_empty() {
                        return;
                    }
                    let index = match collection_indices.get(&candidate.collection).copied() {
                        Some(index) => index,
                        None => {
                            let index = collections.len();
                            collection_indices.insert(candidate.collection.clone(), index);
                            collections.push((candidate.collection.clone(), Vec::new()));
                            index
                        }
                    };
                    collections[index].1.push(candidate);
                };
                for (id, value) in registry.iter_with_kind(ComponentKind::BillboardEmitter) {
                    let ComponentValue::BillboardEmitter(c) = value else {
                        continue;
                    };
                    add_candidate(SpriteCollectionCandidate {
                        collection: c.sprite.clone(),
                        lifetime: Some(c.lifetime),
                        emissive: 0.0,
                        frame_duration_ms: None,
                        source: format!("billboard emitter {id}"),
                    });
                }
                for sprite in projectile_sprites {
                    add_candidate(sprite.into());
                }
            }

            for (collection, candidates) in collections {
                // Keep the draw-contract frame count sourced from the runtime
                // sprite loader. A direct `.png` remains one frame here; baked
                // collection sidecars never become a second shader-facing count.
                let frame_count =
                    postretro_render_cpu::smoke::load_sprite_frames(&texture_root, &collection)
                        .map_or(1, |frames| frames.len());
                let (lifetime, emissive) = match resolve_sprite_collection_draw_contract(
                    &collection,
                    &candidates,
                    frame_count,
                ) {
                    Ok(contract) => contract,
                    Err(reason) => {
                        log::warn!(
                            "[Loader] {reason}; skipping the collection so no accepted descriptor is silently overridden"
                        );
                        continue;
                    }
                };
                renderer.register_smoke_collection(
                    &collection,
                    &texture_root,
                    &prm_cache_root,
                    render::SpriteCollectionRegistration {
                        baked_sidecar_eligible: map_billboard_collections.contains(&collection),
                        spec_intensity: 0.3,
                        lifetime,
                        emissive,
                    },
                );
                particle_render.register_sprite(&collection);
            }

            let collection = weapon::impact_sprite_collection();
            renderer.register_smoke_collection(
                collection,
                &texture_root,
                &prm_cache_root,
                render::SpriteCollectionRegistration {
                    baked_sidecar_eligible: false,
                    spec_intensity: 0.45,
                    lifetime: weapon::impact_lifetime(),
                    emissive: 0.0,
                },
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
    {
        let mut data_registry = script_ctx.data_registry.borrow_mut();
        // Group reactions by dispatch address (name). Addressing is many-to-one
        // (scripting.md §12): one address may hold several reactions, and firing
        // it runs all of them. An address is incompatible with crossing dispatch
        // only when EVERY reaction there needs trigger-fire context; if one
        // sibling is sentinel-free, the runtime dispatch path skips just the
        // incompatible commands, so the compatible reactions must keep their
        // crossing subscription. Stripping the whole address by name would
        // silence those benign siblings.
        let (fully_sentinel_bound, level_load_has_sentinel): (
            std::collections::HashSet<String>,
            bool,
        ) = {
            // Per address: (any reaction sentinel-bound, all reactions sentinel-bound).
            let mut per_address: std::collections::HashMap<&str, (bool, bool)> =
                std::collections::HashMap::new();
            for reaction in &data_registry.reactions {
                let uses_sentinel = reaction_uses_trigger_sentinel(reaction);
                let (any, all) = per_address
                    .entry(reaction.name.as_str())
                    .or_insert((false, true));
                *any |= uses_sentinel;
                *all &= uses_sentinel;
            }
            let level_load_has_sentinel = per_address.get("levelLoad").is_some_and(|(any, _)| *any);
            // A name is strippable only when every reaction registered there is
            // sentinel-bound (each map key has at least one reaction, so the
            // non-empty requirement holds by construction).
            let fully_sentinel_bound = per_address
                .into_iter()
                .filter(|(_, (_, all))| *all)
                .map(|(name, _)| name.to_string())
                .collect();
            (fully_sentinel_bound, level_load_has_sentinel)
        };
        data_registry.crossings.retain_mut(|crossing| {
            let crossing_id = crossing.slot.as_deref().unwrap_or("<predicate>");
            crossing.fire.retain(|address| {
                let strip = fully_sentinel_bound.contains(address);
                if strip {
                    log::warn!(
                        "[Scripting] crossing on `{crossing_id}` drops address `{address}`: every reaction at that address needs trigger-fire context, incompatible with crossing dispatch"
                    );
                }
                !strip
            });
            !crossing.fire.is_empty()
        });
        if level_load_has_sentinel {
            log::warn!(
                "[Scripting] levelLoad references trigger-sentinel work; incompatible commands will be skipped"
            );
        }
    }
    progress_tracker.clear();
    progress_tracker.initialize(
        &script_ctx.data_registry.borrow(),
        &script_ctx.registry.borrow(),
    );
    crossing_detector.clear();
    crossing_detector.initialize(
        &script_ctx.data_registry.borrow(),
        &script_ctx.slot_table.borrow(),
        script_ctx,
    );
}

fn reaction_uses_trigger_sentinel(
    reaction: &postretro_scripting_core::data_descriptors::NamedReaction,
) -> bool {
    use postretro_scripting_core::data_descriptors::{ReactionDescriptor, SequenceTarget};

    match &reaction.descriptor {
        ReactionDescriptor::Primitive(primitive) => primitive.target.is_some(),
        // A control step (`@wait`/`@fire`) is not trigger-sentinel work: a
        // `levelLoad` body containing a wait must keep its crossing subscriptions
        // and must not warn "levelLoad references trigger-sentinel work".
        ReactionDescriptor::Sequence(steps) => steps.iter().any(|step| {
            !matches!(
                step.id,
                SequenceTarget::Entity(_) | SequenceTarget::Wait | SequenceTarget::Fire
            )
        }),
        ReactionDescriptor::Progress(_) => false,
    }
}

fn build_trigger_bindings(
    script_ctx: &postretro_entities::ScriptCtx,
    command_diagnostics: crate::kinematic_mover::MoverCommandDiagnostics,
    spawn_context: crate::spawner::SpawnContext,
) -> TriggerBindingTable {
    TriggerBindingTable::build_with_script_ctx_and_diagnostics(
        &script_ctx.registry.borrow(),
        &script_ctx.data_registry.borrow(),
        script_ctx,
        command_diagnostics,
        spawn_context,
    )
}

/// Test-observable outcome of validating map-authored spawners after classname
/// dispatch. Bad authoring never aborts a level install; it leaves the spawner
/// unresolved, so the fixed-tick executor will skip it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpawnerInstallDiagnostics {
    pub(crate) missing_archetype: usize,
    pub(crate) non_ai_archetype: usize,
}

impl SpawnerInstallDiagnostics {
    pub(crate) fn invalid_total(self) -> usize {
        self.missing_archetype + self.non_ai_archetype
    }
}

/// Resolve every map spawner against this level's descriptor registry and
/// replace the session's fixed-tick spawn cache. The cache contains only AI
/// descriptors, keyed by canonical name, and carries the nav bake that later
/// runtime spawns must use for their agent capsule.
pub(crate) fn resolve_spawners_for_level(
    registry: &mut postretro_entities::EntityRegistry,
    descriptors: &[postretro_scripting_core::data_descriptors::EntityTypeDescriptor],
    agent_params: Option<postretro_foundation::NavAgentParams>,
    spawn_context: &crate::spawner::SpawnContext,
) -> SpawnerInstallDiagnostics {
    use postretro_entities::components::spawner::SpawnerComponent;

    let mut diagnostics = SpawnerInstallDiagnostics::default();
    let mut resolved_descriptors = std::collections::HashMap::new();
    let spawners: Vec<_> = registry
        .iter_with_kind(postretro_entities::ComponentKind::Spawner)
        .map(|(id, _)| id)
        .collect();

    for id in spawners {
        let mut spawner = registry
            .get_component::<SpawnerComponent>(id)
            .expect("iter_with_kind(ComponentKind::Spawner) must yield SpawnerComponent")
            .clone();
        let Some(descriptor) = crate::scripting::builtins::data_archetype::find_descriptor(
            descriptors,
            &spawner.archetype_name,
        ) else {
            diagnostics.missing_archetype += 1;
            // Empty archetype_name already warned (absent/empty key) when the
            // spawner was parsed; don't repeat it as a confusing empty-name
            // "unknown archetype" here.
            if !spawner.archetype_name.is_empty() {
                log::warn!(
                    "[Loader] entity_spawner {id}: unknown archetype `{}`; it will spawn nothing",
                    spawner.archetype_name
                );
            }
            spawner.resolved = false;
            let _ = registry.set_component(id, spawner);
            continue;
        };
        if !descriptor_materializes_ai_enemy(descriptor) {
            diagnostics.non_ai_archetype += 1;
            log::warn!(
                "[Loader] entity_spawner {id}: archetype `{}` is not an AI enemy; it will spawn nothing",
                spawner.archetype_name
            );
            spawner.resolved = false;
            let _ = registry.set_component(id, spawner);
            continue;
        }

        let canonical_name = descriptor
            .canonical_name
            .as_ref()
            .expect("find_descriptor matches only descriptors with canonical names")
            .clone();
        resolved_descriptors.insert(canonical_name, descriptor.clone());
        spawner.resolved = true;
        let _ = registry.set_component(id, spawner);
    }

    spawn_context.replace_level_data(resolved_descriptors, agent_params);
    diagnostics
}

/// Collect renderable mesh handles referenced by successfully resolved map
/// spawners. Spawner-only archetypes do not materialize a `MeshComponent` until
/// a reaction fires, so the normal registry sweep cannot discover their models.
/// Keep this tied to the resolved component rather than every descriptor: only
/// archetypes this map can actually spawn consume the level's upload budget.
pub(crate) fn resolved_spawner_mesh_models(
    registry: &postretro_entities::EntityRegistry,
    descriptors: &[postretro_scripting_core::data_descriptors::EntityTypeDescriptor],
) -> Vec<String> {
    use postretro_entities::components::spawner::SpawnerComponent;

    let mut seen = std::collections::HashSet::new();
    let mut models = Vec::new();
    let mut add_model = |model: &str| {
        if !model.is_empty() && seen.insert(model.to_string()) {
            models.push(model.to_string());
        }
    };
    for (id, _) in registry.iter_with_kind(postretro_entities::ComponentKind::Spawner) {
        let Ok(spawner) = registry.get_component::<SpawnerComponent>(id) else {
            continue;
        };
        if !spawner.resolved {
            continue;
        }
        let Some(descriptor) = crate::scripting::builtins::data_archetype::find_descriptor(
            descriptors,
            &spawner.archetype_name,
        ) else {
            continue;
        };
        let Some(mesh) = descriptor.mesh.as_ref() else {
            continue;
        };
        add_model(&mesh.model);
        let mut attachment_models: Vec<&str> =
            mesh.attachments.values().map(String::as_str).collect();
        attachment_models.sort_unstable();
        for attachment_model in attachment_models {
            add_model(attachment_model);
        }
    }
    models
}

/// CPU world-install products the tick loop consumes. [`install_world_cpu`]
/// fills these from a level payload without touching the renderer, so a headless
/// caller assembles `simulate_tick`'s arguments from the return value plus the
/// nav graph produced by [`install_world_gravity_and_nav`] and the handles it
/// passed in (populated registry, collision world, hit-zone store).
pub(crate) struct WorldInstallProducts {
    /// Static colliders for every loaded kinematic mover.
    pub(crate) mover_colliders: Vec<crate::collision::moving::MoverCollider>,
    /// Spawned mover entity ids aligned with `KinematicGeometry::movers`.
    /// A failed all-or-nothing mover spawn returns an empty vector.
    pub(crate) spawned_mover_entities: Vec<postretro_entities::registry::EntityId>,
    /// Trigger reactions partitioned from the final composed active set.
    pub(crate) trigger_bindings: TriggerBindingTable,
    /// Host-only trigger-pool outcome retained by `App` for diagnostics and
    /// tests. Connected clients receive the default empty report.
    pub(crate) trigger_pool_report: TriggerPoolInstallReport,
    /// A fresh, empty mover tick-state table. Not an install product — it is
    /// caller-owned per-tick state — returned only so the headless batch runner
    /// has one to hand `simulate_tick` without reaching into `App` (the windowed
    /// `App` field is its own `MoverTickStateTable::default()` and ignores this).
    /// The headless driver (`observability::driver`) is the sole reader, so the
    /// field is dead only in a build without that feature.
    #[cfg_attr(not(feature = "observability"), allow(dead_code))]
    pub(crate) mover_tick_states: crate::kinematic_mover::MoverTickStateTable,
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
    pub(crate) command_diagnostics: crate::kinematic_mover::MoverCommandDiagnostics,
    /// Current mod-wide auto-close default. Static level-install input only.
    pub(crate) mover_auto_close_ms: f32,
    /// Session-owned VM-free resolved descriptor cache for entity spawners.
    pub(crate) spawn_context: crate::spawner::SpawnContext,
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
    pub(crate) slot_accumulator_bindings:
        &'a mut crate::scripting_systems::slot_accumulators::SlotAccumulatorBindings,
    /// Runtime-owned impact policy registry. Level descriptors are installed
    /// after the data script returns its manifest.
    pub(crate) impact_policy_runtime: &'a mut crate::impact_policy::ImpactPolicyRuntime,
    pub(crate) mesh_clip_tables: &'a mut crate::scripting_systems::mesh_anim::MeshClipTables,
    pub(crate) hit_zone_store: &'a mut crate::scripting_systems::hit_zones::HitZoneStore,
    /// Resolved separately for each install. A pinned seed repeats exactly;
    /// arm-all is the deterministic unpinned headless default.
    pub(crate) trigger_pool_policy: TriggerPoolSeedPolicy,
    /// Connected-client setup flag: skip host-authoritative map-placement
    /// materialization and boot-pawn spawn. Both `false` off a connected client
    /// (single-player, listen host, headless).
    pub(crate) suppress_ai_enemies: bool,
    pub(crate) suppress_boot_pawn: bool,
    /// Seat-zero carried record, resolved by the caller before descriptor spawn. The
    /// world installer remains seat-table agnostic.
    pub(crate) local_carried_loadout: Option<crate::netcode::CarriedState>,
}

/// Connected-client trigger-pool install result exposed only to cross-subsystem
/// tests. The registry is the one populated by [`install_world_cpu`].
#[cfg(test)]
pub(crate) struct ConnectedClientTriggerPoolInstallFixture {
    pub(crate) registry: postretro_entities::EntityRegistry,
    pub(crate) trap: postretro_entities::EntityId,
    pub(crate) report: TriggerPoolInstallReport,
}

#[cfg(test)]
pub(crate) fn install_connected_client_trigger_pool_fixture_for_test()
-> ConnectedClientTriggerPoolInstallFixture {
    tests::install_connected_client_trigger_pool_fixture()
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
    use postretro_entities::SystemReactionCommand;
    use postretro_entities::{
        CrossingCondition, CrossingDescriptor, EntityId, EntityTypeDescriptor, MoverCommand,
        NamedReaction, PrimitiveDescriptor, ProgressDescriptor, ReactionDescriptor,
        TriggerActivation, TriggerEventDescriptor, TriggerFireMode, TriggerPoolArm,
        TriggerPoolDescriptor, TriggerVolumeComponent,
    };
    use postretro_entities::{
        ScriptCtx, SlotOwnership, SlotRecord, SlotSchema, SlotType, SlotValue, Transform,
    };
    use postretro_foundation::ModMapEntry;
    use postretro_level_format::trigger_volumes::TriggerVolumeRecord;
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

    fn sprite_candidate(
        source: &str,
        lifetime: Option<f32>,
        emissive: f32,
        frame_duration_ms: Option<f32>,
    ) -> SpriteCollectionCandidate {
        SpriteCollectionCandidate {
            collection: "sprites/shared".to_string(),
            lifetime,
            emissive,
            frame_duration_ms,
            source: source.to_string(),
        }
    }

    #[test]
    fn shared_projectile_collection_rejects_conflicting_draw_contracts() {
        let candidates = [
            sprite_candidate("plasma.projectile.visual.body", None, 2.0, Some(50.0)),
            sprite_candidate("rocket.projectile.visual.body", None, 1.0, Some(80.0)),
        ];

        let error = resolve_sprite_collection_draw_contract("sprites/shared", &candidates, 4)
            .expect_err("conflicting projectile consumers must reject the collection");

        assert!(error.contains("conflicting loop periods"));
        assert!(error.contains("plasma.projectile.visual.body"));
        assert!(error.contains("rocket.projectile.visual.body"));
    }

    #[test]
    fn projectile_and_emitter_collection_rejects_conflicting_emissive_contracts() {
        let candidates = [
            sprite_candidate("billboard emitter 7", Some(0.2), 0.0, None),
            sprite_candidate("plasma.projectile.visual.trail", Some(0.2), 3.0, None),
        ];

        let error = resolve_sprite_collection_draw_contract("sprites/shared", &candidates, 4)
            .expect_err("emitter and projectile conflicts must not depend on collection order");

        assert!(error.contains("conflicting emissive strengths"));
        assert!(error.contains("billboard emitter 7"));
        assert!(error.contains("plasma.projectile.visual.trail"));
    }

    #[test]
    fn compatible_shared_sprite_consumers_resolve_one_draw_contract() {
        let candidates = [
            sprite_candidate("billboard emitter 7", Some(0.2), 0.0, None),
            sprite_candidate("plasma.projectile.visual.trail", Some(0.2), 0.0, None),
            sprite_candidate("plasma.projectile.visual.body", None, 0.0, None),
        ];

        let (lifetime, emissive) =
            resolve_sprite_collection_draw_contract("sprites/shared", &candidates, 4)
                .expect("identical consumers and a cadence-less body are compatible");

        assert!((lifetime - 0.2).abs() <= f32::EPSILON);
        assert!(emissive.abs() <= f32::EPSILON);
    }

    #[test]
    fn baked_sprite_eligibility_comes_only_from_map_billboard_emitters() {
        use postretro_level_format::map_entity::MapEntityRecord;

        let entities = [
            MapEntityRecord {
                classname: "billboard_emitter".to_string(),
                key_values: vec![("sprite".to_string(), "smoke".to_string())],
                ..Default::default()
            },
            MapEntityRecord {
                classname: "billboard_emitter".to_string(),
                key_values: vec![
                    ("sprite".to_string(), "ignored".to_string()),
                    ("sprite".to_string(), "sparks".to_string()),
                ],
                ..Default::default()
            },
            MapEntityRecord {
                classname: "billboard_emitter".to_string(),
                key_values: vec![("sprite".to_string(), String::new())],
                ..Default::default()
            },
            MapEntityRecord {
                classname: "data_archetype".to_string(),
                key_values: vec![("sprite".to_string(), "descriptor_only".to_string())],
                ..Default::default()
            },
        ];

        let collections = map_billboard_sprite_collections(&entities);
        assert!(collections.contains("smoke"));
        assert!(collections.contains("sparks"));
        assert!(!collections.contains("ignored"));
        assert!(!collections.contains("descriptor_only"));
    }

    #[test]
    fn explicit_sprite_cadence_uses_the_normalized_frame_count() {
        // Regression: three decoded frames produced a three-frame loop period
        // even when only two shared the renderer's array extent.
        let frames = vec![
            postretro_render_cpu::smoke::SpriteFrame {
                data: vec![0; 16],
                width: 2,
                height: 2,
            },
            postretro_render_cpu::smoke::SpriteFrame {
                data: vec![0; 4],
                width: 1,
                height: 1,
            },
            postretro_render_cpu::smoke::SpriteFrame {
                data: vec![0; 16],
                width: 2,
                height: 2,
            },
        ];
        let frames = postretro_render_cpu::smoke::normalize_sprite_frames(frames)
            .expect("two frames share the collection extent");
        let candidates = [sprite_candidate(
            "plasma.projectile.visual.body",
            None,
            0.0,
            Some(50.0),
        )];

        let (lifetime, _) =
            resolve_sprite_collection_draw_contract("sprites/shared", &candidates, frames.len())
                .expect("one consumer resolves");

        assert!((lifetime - 0.1).abs() <= f32::EPSILON);
        assert!((lifetime / frames.len() as f32 - 0.05).abs() <= f32::EPSILON);
    }

    #[test]
    fn spawner_install_resolves_only_ai_descriptors_and_replaces_prior_level_data() {
        use crate::scripting::builtins::data_archetype_test_fixtures::{
            behavior_enemy_descriptor, mesh_descriptor,
        };
        use crate::scripting::builtins::entity_spawner;
        use crate::scripting::map_entity::MapEntity;
        use postretro_entities::components::spawner::SpawnerComponent;

        let mut registry = postretro_entities::EntityRegistry::new();
        let map_entity = |archetype: &str| MapEntity {
            classname: entity_spawner::CLASSNAME.to_string(),
            origin: Vec3::ZERO,
            angles: Vec3::ZERO,
            key_values: [
                ("archetype".to_string(), archetype.to_string()),
                ("count".to_string(), "2".to_string()),
            ]
            .into_iter()
            .collect(),
            tags: vec![],
        };
        let enemy = entity_spawner::handle(&map_entity("cultist"), &mut registry).unwrap();
        let non_ai = entity_spawner::handle(&map_entity("crate"), &mut registry).unwrap();
        let missing = entity_spawner::handle(&map_entity("absent"), &mut registry).unwrap();
        let context = crate::spawner::SpawnContext::default();
        context.replace_level_data(std::collections::HashMap::new(), None);

        let params = postretro_foundation::NavAgentParams {
            radius: 0.4,
            height: 2.0,
            step_height: 0.45,
            max_slope_deg: 50.0,
        };
        let diagnostics = resolve_spawners_for_level(
            &mut registry,
            &[
                behavior_enemy_descriptor("cultist"),
                mesh_descriptor("crate", false),
            ],
            Some(params),
            &context,
        );

        assert!(
            registry
                .get_component::<SpawnerComponent>(enemy)
                .unwrap()
                .resolved
        );
        assert!(
            !registry
                .get_component::<SpawnerComponent>(non_ai)
                .unwrap()
                .resolved
        );
        assert!(
            !registry
                .get_component::<SpawnerComponent>(missing)
                .unwrap()
                .resolved
        );
        assert_eq!(
            diagnostics,
            SpawnerInstallDiagnostics {
                missing_archetype: 1,
                non_ai_archetype: 1,
            }
        );
        let state = context.state();
        assert_eq!(state.resolved_enemy_descriptors.len(), 1);
        assert!(state.resolved_enemy_descriptors.contains_key("cultist"));
        assert_eq!(state.agent_params, Some(params));
        drop(state);

        // A new level drops stale descriptor entries and warning dedup state.
        resolve_spawners_for_level(&mut registry, &[], None, &context);
        assert!(context.state().resolved_enemy_descriptors.is_empty());
    }

    #[test]
    fn resolved_spawner_mesh_models_include_only_renderable_resolved_archetypes() {
        use crate::scripting::builtins::data_archetype_test_fixtures::behavior_enemy_descriptor;
        use postretro_entities::components::spawner::SpawnerComponent;

        let mut registry = postretro_entities::EntityRegistry::new();
        let add_spawner =
            |registry: &mut postretro_entities::EntityRegistry, archetype: &str, resolved: bool| {
                let id = registry.spawn(Transform::default());
                registry
                    .set_component(
                        id,
                        SpawnerComponent {
                            archetype_name: archetype.to_string(),
                            count: 1,
                            resolved,
                        },
                    )
                    .unwrap();
            };
        add_spawner(&mut registry, "spawner_only", true);
        add_spawner(&mut registry, "spawner_only", true);
        add_spawner(&mut registry, "unresolved", false);
        add_spawner(&mut registry, "non_mesh", true);
        add_spawner(&mut registry, "absent", true);

        let mut spawner_only = behavior_enemy_descriptor("spawner_only");
        spawner_only.mesh.as_mut().unwrap().model = "models/spawner_only.gltf".to_string();
        spawner_only.mesh.as_mut().unwrap().attachments =
            [("hand".to_string(), "models/spawner_prop.gltf".to_string())]
                .into_iter()
                .collect();
        let mut non_mesh = behavior_enemy_descriptor("non_mesh");
        non_mesh.mesh = None;

        let models = resolved_spawner_mesh_models(&registry, &[spawner_only, non_mesh]);
        assert_eq!(
            models,
            vec![
                "models/spawner_only.gltf".to_string(),
                "models/spawner_prop.gltf".to_string(),
            ]
        );
        assert!(crate::distinct_mesh_models(&registry).is_empty());
    }

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
                    command_diagnostics: Default::default(),
                    auto_close_timers: Default::default(),
                    mover_auto_close_ms: crate::runtime_movers::ENGINE_AUTO_CLOSE_MS,
                    spawn_context: Default::default(),
                    script_runtime,
                    script_ctx: script_ctx.clone(),
                    impact_policy_runtime: crate::impact_policy::ImpactPolicyRuntime::new(
                        script_ctx.clone(),
                    ),
                    sequence_registry: SequencedPrimitiveRegistry::new(),
                    reaction_registry:
                        scripting::reactions::registry::ReactionPrimitiveRegistry::new(),
                    system_registry:
                        scripting::reactions::system_commands::SystemReactionRegistry::new(),
                    system_reaction_ir_bindings: Default::default(),
                    slot_accumulator_bindings: Default::default(),
                    scheduler: Default::default(),
                    player_hud_state: scripting_systems::ui_proxy::PlayerHudStatePublisher::new(
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
                presentation_pool: crate::presentation_pool::PresentationPool::default(),
                client_overlay_facts: crate::netcode::ClientOverlayFactState::default(),
                host_overlay_fact_tracker: crate::netcode::HostOverlayFactTracker::default(),
                state_store_lifecycle: Default::default(),
                persisted_state: None,
                per_owner_save_timer: Default::default(),
                progress_tracker: ProgressTracker::new(),
                pending_death_events: Vec::new(),
                crossing_detector: CrossingDetector::new(),
                classname_dispatch: scripting::builtins::ClassnameDispatch::new(),
                light_bridge: scripting_systems::light_bridge::LightBridge::new(),
                fog_volume_bridge: scripting_systems::fog_volume_bridge::FogVolumeBridge::new(),
                trigger_volume_bridge:
                    scripting_systems::trigger_volume_bridge::TriggerVolumeBridge::new(),
                trigger_system: crate::trigger_system::TriggerSystem::default(),
                touch_system: crate::sim::touch::TouchSystem::default(),
                emitter_bridge: scripting_systems::emitter_bridge::EmitterBridge::new(),
                particle_render: scripting_systems::particle_render::ParticleRenderCollector::new(),
                mesh_render: scripting_systems::mesh_render::MeshRenderCollector::new(),
                mesh_clip_tables: scripting_systems::mesh_anim::MeshClipTables::new(),
                hit_zone_store: scripting_systems::hit_zones::HitZoneStore::new(),
                player_options: options::PlayerOptions::default(),
                settings_path: None,
                frontend: None,
                net_endpoint: None,
                seat_table: None,
                audio: None,
                #[cfg(feature = "dev-tools")]
                debug_ui: None,
            }),
            remote_player_presentation: crate::netcode::ClientPresentationInputs::default(),
            crouch_toggle_active: false,
            ai_runtime: crate::scripting_systems::ai::AiRuntime::new(),
            cursor_pos: None,
            nav_stick_tracker: input::StickNavTracker::new(),
            frame_timing: FrameTiming::new(initial_state),
            view_feel_state: view_feel::ViewFeelState::default(),
            diagnostic_inputs: input::DiagnosticInputs::new(input::default_diagnostic_chords()),
            capture_portal_walk_next_frame: false,
            scratch_cells: Vec::new(),
            blocked_portals: Vec::new(),
            frame_rate_meter: FrameRateMeter::new(),
            title_buffer: String::new(),
            last_title_update: Instant::now(),
            mod_theme_override: Default::default(),
            switching: Default::default(),
            pending_mode_signal: None,
            pending_menu_toggle: false,
            pending_exit_to_desktop: false,
            ui_focused_id: None,
            particle_live_counts: std::collections::HashMap::new(),
            collision_world: crate::collision::CollisionWorld::new(),
            kinematic_mover_colliders: Vec::new(),
            kinematic_mover_tick_states: crate::kinematic_mover::MoverTickStateTable::default(),
            mover_yaw_carry_ground: postretro_foundation::GroundRef::Airborne,
            kinematic_mover_render: crate::runtime_movers::KinematicMoverRenderCollector::new(),
            trigger_bindings: crate::trigger_bindings::TriggerBindingTable::default(),
            trigger_pool_report: TriggerPoolInstallReport::default(),
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
            session_boot_config: crate::startup::session::SessionBootConfig::default(),
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
            inventory: None,
            light: None,
            emitter: None,
            movement: None,
            weapon: None,
            touchable: None,
            mesh: None,
            health: None,
            behavior: None,
        }
    }

    fn named_reaction(name: &str) -> NamedReaction {
        NamedReaction {
            name: name.to_string(),
            descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                primitive: "testPrimitive".to_string(),
                target: None,
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
                    target: None,
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
                slot: Some(slot.to_string()),
                condition: CrossingCondition::Below { threshold: 0.5 },
                max: 100.0,
                edge: None,
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
            per_owner: false,
            accumulate: None,
        });
        record.value = Some(SlotValue::Number(value));
        record
    }

    #[test]
    fn app_system_command_drain_accumulates_bound_runtime_set_state_writes_in_queue_order() {
        let mut app = test_app();
        let ctx = script_ctx(&app);
        ctx.slot_table
            .borrow_mut()
            .insert("puzzle.count".to_string(), number_slot(0.0))
            .expect("fixture slot should be vacant");
        let increment = serde_json::json!({
            "op": "add",
            "a": { "op": "input", "name": "puzzle.count" },
            "b": { "op": "const", "value": 1.0 }
        });
        ctx.data_registry.borrow_mut().populate_level(
            vec![NamedReaction {
                name: "increment".to_string(),
                descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                    primitive: "setState".to_string(),
                    target: None,
                    tag: None,
                    on_complete: None,
                    args: serde_json::json!({
                        "slot": "puzzle.count",
                        "value": increment.clone(),
                    }),
                }),
            }],
            Vec::new(),
            &[],
        );
        app.rebuild_active_system_reaction_bindings();

        // Exercise App::dispatch_system_commands, the production app-side
        // drain. The session-owned table must evaluate both installed programs
        // in FIFO order so the second read observes the first write.
        for _ in 0..2 {
            ctx.system_commands.push(SystemReactionCommand::SetState {
                slot: "puzzle.count".to_string(),
                value: increment.clone(),
                dispatch_source: "test.levelLoad".to_string(),
                dispatch_values: Vec::new(),
            });
        }
        app.dispatch_system_commands();

        let count = match ctx
            .slot_table
            .borrow()
            .get("puzzle.count")
            .and_then(|record| record.value.as_ref())
        {
            Some(SlotValue::Number(value)) => *value,
            other => panic!("expected numeric puzzle count, got {other:?}"),
        };
        assert!(
            (count - 2.0).abs() <= 1.0e-5,
            "two self-referential IR writes must accumulate at the app drain: got {count}"
        );
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
            cell_visibility: None,
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
            animated_direct_sh_delta_volumes: None,
            billboard_direct_scatter_volume: None,
            animated_billboard_direct_scatter_delta_volumes: None,
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

    #[test]
    fn windowed_carrier_binding_reresolves_before_first_bridge_update_after_reload() {
        use postretro_entities::components::light::LightComponent;
        use postretro_level_loader::{KinematicGeometry, LoadedKinematicMover, LoadedMemberLight};

        fn geometry() -> KinematicGeometry {
            KinematicGeometry {
                movers: vec![LoadedKinematicMover {
                    mover_id: 71,
                    name: "reload_lift".to_string(),
                    tags: Vec::new(),
                    origin: Vec3::ZERO,
                    path: "reload_lift_start".to_string(),
                    speed_mps: 1.0,
                    wait_ms: 0.0,
                    move_mode: 0,
                    start_on_spawn: false,
                    vertices: Vec::new(),
                    indices: Vec::new(),
                    face_meta: Vec::new(),
                    spin_axis: Vec3::ZERO,
                    spin_speed_deg_s: 0.0,
                    spin_accel_deg_s2: 0.0,
                    carry_yaw: false,
                    block_policy: "displace".to_string(),
                    crush_damage: 0.0,
                    crush_interval_ms: 0.0,
                    auto_close_ms: None,
                    open_event: None,
                    close_event: None,
                    blocked_event: None,
                    crush_event: None,
                    sealed_portal_ids: Vec::new(),
                    carried_lights: vec![LoadedMemberLight {
                        alpha_light_index: 0,
                        local_offset: Vec3::new(2.0, 0.0, 0.0),
                    }],
                }],
                waypoints: Vec::new(),
            }
        }

        let mut carried_light = map_light("e22_carried", [2.0, 0.0, 0.0]);
        carried_light.is_dynamic = true;

        // This is the same windowed half of install_level_payload: the bridge
        // creates light entities first, mover spawn completes, then the pass
        // binds before LightBridge::update gets a frame.
        let first_geometry = geometry();
        let mut first_registry = postretro_entities::EntityRegistry::new();
        let mut first_bridge = crate::scripting_systems::light_bridge::LightBridge::new();
        first_bridge.populate_from_level(&[carried_light.clone()], &mut first_registry, 0);
        let first_mover = first_registry.spawn(Transform {
            position: Vec3::new(10.0, 0.0, 0.0),
            ..Transform::default()
        });
        resolve_carried_light_bindings_after_mover_spawn(
            &first_geometry,
            &[first_mover],
            &first_bridge,
            &mut first_registry,
        );
        let first_light = first_bridge.entity_for_map_index(0).unwrap();
        assert_eq!(
            first_registry
                .get_component::<LightComponent>(first_light)
                .unwrap()
                .carrier
                .as_ref()
                .unwrap()
                .mover_entity,
            first_mover,
            "the first windowed install binds before its first bridge frame"
        );
        assert!(
            first_bridge.update(&mut first_registry, 0.0, 0.0).is_some(),
            "the first bridge update receives the already-bound carrier"
        );

        // Level unload clears bridge + registry. Make the replacement mover
        // occupy a distinct raw id so this asserts an actual re-resolution,
        // rather than accidentally passing because an allocator reused it.
        let reloaded_geometry = geometry();
        let mut reloaded_registry = postretro_entities::EntityRegistry::new();
        let mut reloaded_bridge = crate::scripting_systems::light_bridge::LightBridge::new();
        reloaded_bridge.populate_from_level(&[carried_light], &mut reloaded_registry, 0);
        let _unrelated = reloaded_registry.spawn(Transform::default());
        let reloaded_mover = reloaded_registry.spawn(Transform {
            position: Vec3::new(30.0, 0.0, 0.0),
            ..Transform::default()
        });
        assert_ne!(first_mover, reloaded_mover, "fixture needs fresh mover ids");
        resolve_carried_light_bindings_after_mover_spawn(
            &reloaded_geometry,
            &[reloaded_mover],
            &reloaded_bridge,
            &mut reloaded_registry,
        );

        let reloaded_light = reloaded_bridge.entity_for_map_index(0).unwrap();
        assert_eq!(
            reloaded_registry
                .get_component::<LightComponent>(reloaded_light)
                .unwrap()
                .carrier
                .as_ref()
                .unwrap()
                .mover_entity,
            reloaded_mover,
            "reload must not retain the first level's raw mover entity id"
        );
        assert!(
            reloaded_bridge
                .update(&mut reloaded_registry, 0.0, 0.0)
                .is_some(),
            "the replacement carrier is also present before its first bridge update"
        );
        let reloaded_position = Vec3::from_array(
            reloaded_bridge.collect_all_as_map_lights(&reloaded_registry, 0.0)[0]
                .0
                .origin
                .map(|value| value as f32),
        );
        assert!(
            reloaded_position.distance(Vec3::new(32.0, 0.0, 0.0)) <= 1.0e-6,
            "the reloaded bridge composes the fresh mover pose, not the authored origin"
        );
    }

    fn pool_descriptor(tag: &str, arm: TriggerPoolArm, levels: &[&str]) -> TriggerPoolDescriptor {
        TriggerPoolDescriptor {
            tag: tag.to_string(),
            arm,
            levels: levels.iter().map(|level| (*level).to_string()).collect(),
        }
    }

    fn trap_pool_trigger_record(
        name: &str,
        tag: &str,
        index: usize,
        enabled_on_spawn: bool,
    ) -> TriggerVolumeRecord {
        let x = index as f32 * 4.0;
        TriggerVolumeRecord {
            name: name.to_string(),
            tags: vec![tag.to_string()],
            aabb_min: [x, 0.0, 0.0],
            aabb_max: [x + 1.0, 1.0, 1.0],
            activation: 0,
            target_tag: String::new(),
            command: 0,
            command_arg: String::new(),
            fire_mode: 0,
            rearm_ms: 0.0,
            enabled_on_spawn,
            on_fire: String::new(),
            on_exit: String::new(),
        }
    }

    fn trap_pool_fixture_world() -> postretro_level_loader::LevelWorld {
        let mut world = level_world("trap_pools", 1);
        world.trigger_volumes = (0..4)
            .map(|index| {
                trap_pool_trigger_record(&format!("closet-{index}"), "closet_trap", index, true)
            })
            .chain((0..4).map(|index| {
                trap_pool_trigger_record(
                    &format!("ambush-{index}"),
                    "ambush_trap",
                    index + 4,
                    false,
                )
            }))
            .collect();
        world
    }

    struct TrapPoolFixtureInstall {
        report: TriggerPoolInstallReport,
        armed_by_tag: BTreeMap<String, Vec<EntityId>>,
    }

    struct TriggerPoolWorldInstall {
        report: TriggerPoolInstallReport,
        registry: postretro_entities::EntityRegistry,
    }

    fn install_trigger_pool_world(
        world: postretro_level_loader::LevelWorld,
        policy: TriggerPoolSeedPolicy,
        active_level_tags: &[&str],
        global_pools: Vec<TriggerPoolDescriptor>,
        suppress_ai_enemies: bool,
    ) -> TriggerPoolWorldInstall {
        let mut app = test_app();
        let ctx = script_ctx(&app);
        ctx.data_registry
            .borrow_mut()
            .replace_global_trigger_pools(global_pools);
        let active_level_tags: Vec<String> = active_level_tags
            .iter()
            .map(|tag| (*tag).to_string())
            .collect();
        let mut timings = StartupTimings::new();
        let products = {
            let session = app.session.as_mut().expect("test app session installed");
            let handles = WorldInstallHandles {
                command_diagnostics: Default::default(),
                mover_auto_close_ms: crate::runtime_movers::ENGINE_AUTO_CLOSE_MS,
                spawn_context: Default::default(),
                world: &world,
                script_ctx: &ctx,
                content_root: std::path::Path::new("content/dev"),
                active_level_tags: &active_level_tags,
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
                slot_accumulator_bindings: &mut session.scripting.slot_accumulator_bindings,
                impact_policy_runtime: &mut session.scripting.impact_policy_runtime,
                mesh_clip_tables: &mut session.mesh_clip_tables,
                hit_zone_store: &mut session.hit_zone_store,
                trigger_pool_policy: policy,
                suppress_ai_enemies,
                suppress_boot_pawn: suppress_ai_enemies,
                local_carried_loadout: None,
            };
            install_world_cpu(
                handles,
                &mut timings,
                |_models, _clip_tables| {
                    crate::scripting_systems::hit_zones::ModelLoadWarningOwner::GameSide
                },
                |_spawn_points| {},
            )
        };

        let registry = std::mem::take(&mut *ctx.registry.borrow_mut());
        TriggerPoolWorldInstall {
            report: products.trigger_pool_report,
            registry,
        }
    }

    /// Run the real renderer-free installation seam with a policy pinned on
    /// `WorldInstallHandles`, never through argv. The synthetic records mirror
    /// the authored fixture's two four-member pools while keeping this QA gate
    /// free of a cold PRL bake.
    fn install_trap_pool_fixture(
        policy: TriggerPoolSeedPolicy,
        active_level_tags: &[&str],
        global_pools: Vec<TriggerPoolDescriptor>,
        suppress_ai_enemies: bool,
    ) -> TrapPoolFixtureInstall {
        let installed = install_trigger_pool_world(
            trap_pool_fixture_world(),
            policy,
            active_level_tags,
            global_pools,
            suppress_ai_enemies,
        );
        let registry = &installed.registry;
        let armed_by_tag = ["closet_trap", "ambush_trap"]
            .into_iter()
            .map(|tag| {
                let mut armed: Vec<EntityId> = registry
                    .query_by_component_and_tag(
                        postretro_entities::ComponentKind::TriggerVolume,
                        Some(tag),
                    )
                    .filter_map(|(id, _)| {
                        registry
                            .get_component::<TriggerVolumeComponent>(id)
                            .is_ok_and(|trigger| trigger.armed)
                            .then_some(id)
                    })
                    .collect();
                armed.sort_unstable();
                (tag.to_string(), armed)
            })
            .collect();
        TrapPoolFixtureInstall {
            report: installed.report,
            armed_by_tag,
        }
    }

    pub(super) fn install_connected_client_trigger_pool_fixture()
    -> ConnectedClientTriggerPoolInstallFixture {
        let mut world = level_world("connected_client_trap_pool", 1);
        let mut trap = trap_pool_trigger_record("network-trap", "trap-pool", 0, false);
        trap.on_fire = "trapPools.spawn".to_string();
        world.trigger_volumes = vec![trap];
        let installed = install_trigger_pool_world(
            world,
            TriggerPoolSeedPolicy::Seeded(17),
            &[],
            vec![pool_descriptor("trap-pool", TriggerPoolArm::Count(1), &[])],
            true,
        );
        let trap = installed
            .registry
            .query_by_component_and_tag(
                postretro_entities::ComponentKind::TriggerVolume,
                Some("trap-pool"),
            )
            .map(|(id, _)| id)
            .next()
            .expect("real client install materializes the authored trap");

        ConnectedClientTriggerPoolInstallFixture {
            registry: installed.registry,
            trap,
            report: installed.report,
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
        app.session
            .as_mut()
            .expect("test app session installed")
            .scripting
            .slot_accumulator_bindings
            .rebuild(&ctx);

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
                        per_owner: false,
                        accumulate: Some(postretro_foundation::IrNode::Input {
                            name: "@dt".to_string(),
                            owner: None,
                        }),
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
        assert!(
            !app.session
                .as_ref()
                .expect("test app session installed")
                .scripting
                .slot_accumulator_bindings
                .is_empty(),
            "level install must bind the declared accumulator"
        );

        let presentation_target = {
            let ctx = script_ctx(&app);
            let mut registry = ctx.registry.borrow_mut();
            let target = registry.spawn(Transform::default());
            registry.push_presentation_spawn(postretro_entities::PresentationSpawn {
                world_anchor: Vec3::ZERO,
                template: "old-level-number".into(),
                facts: BTreeMap::new(),
                presenter: None,
                lifetime_seconds: 1.0,
                motion: postretro_foundation::PresentationMotion::default(),
                fade: postretro_foundation::PresentationFade::default(),
                scatter_radius: 0.0,
            });
            target
        };
        {
            let session = app.session.as_mut().expect("test app session installed");
            let mut registry = session.scripting.script_ctx.registry.borrow_mut();
            let _ = session.presentation_pool.advance_and_collect_inputs(
                &mut registry,
                0.0,
                glam::Mat4::IDENTITY,
                [800, 600],
            );
            session.presentation_pool.refresh_overlay(
                presentation_target,
                postretro_entities::PresentationTemplateHandle::from("old-level-overlay"),
                1.0,
                1,
                u64::from(presentation_target.to_raw()),
            );
            registry.push_presentation_spawn(postretro_entities::PresentationSpawn {
                world_anchor: Vec3::ZERO,
                template: "queued-old-level-number".into(),
                facts: BTreeMap::new(),
                presenter: None,
                lifetime_seconds: 1.0,
                motion: postretro_foundation::PresentationMotion::default(),
                fade: postretro_foundation::PresentationFade::default(),
                scatter_radius: 0.0,
            });
            crate::netcode::ingest_client_overlay_fact(
                &mut session.client_overlay_facts,
                &mut session.presentation_pool,
                crate::netcode::ClientOverlayFact::new(
                    postretro_net::wire::NetworkId(91),
                    0.0,
                    0.0,
                    false,
                    false,
                ),
                None,
                None,
                None,
            );
        }

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
        assert!(
            app.session
                .as_ref()
                .expect("test app session installed")
                .scripting
                .slot_accumulator_bindings
                .is_empty(),
            "level unload must drop every bound accumulator program"
        );
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
        let session = app.session.as_ref().expect("test app session installed");
        assert_eq!(session.presentation_pool.live_counts(), (0, 0));
        assert_eq!(session.client_overlay_facts.terminal_len(), 0);
        assert!(
            session
                .scripting
                .script_ctx
                .registry
                .borrow_mut()
                .take_presentation_spawns()
                .is_empty(),
            "level unload must discard queued world presentation intake",
        );
    }

    // Regression: a frame-end removal from the old level could dispatch its
    // carried kill event after the next level installed.
    #[test]
    fn unload_level_clears_pending_death_event_carryover() {
        let mut app = test_app();
        app.session
            .as_mut()
            .expect("test app session installed")
            .pending_death_events
            .push("oldLevelKill".to_string());

        app.unload_level();

        assert!(
            app.session
                .as_ref()
                .expect("test app session installed")
                .pending_death_events
                .is_empty(),
            "level unload must discard deferred death events from the old level",
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
                id: "replacement".to_string(),
                version: "1".to_string(),
                render: Default::default(),
                movers: Default::default(),
                switching: Default::default(),
                default_weapon_placement: None,
                entities: Vec::new(),
                maps: Vec::new(),
                reactions: Vec::new(),
                crossings: Vec::new(),
                events: Vec::new(),
                trigger_events: Vec::new(),
                trigger_pools: Vec::new(),
                ui_trees: vec![RegisteredUiTree {
                    name: "newMenu".to_string(),
                    tree: postretro_ui::demo::build_frontend_menu_descriptor(),
                    always_on: false,
                }],
                presentation_templates: Vec::new(),
                presentation_overlays: Vec::new(),
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
                    target: None,
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
    fn level_identity_distinguishes_catalog_and_normalizes_raw_paths() {
        let root = PathBuf::from("/mods/demo");
        assert_eq!(
            level_identity(&LevelSource::Catalog("e1m1".to_string()), &root),
            "e1m1"
        );
        assert_eq!(
            level_identity(
                &LevelSource::Path(PathBuf::from("/mods/demo/maps/./episode/../e1m1.prl")),
                &root,
            ),
            "path:maps/e1m1.prl"
        );
    }

    #[test]
    fn level_identity_keeps_outside_content_root_absolute() {
        let root = PathBuf::from("/mods/demo");
        assert_eq!(
            level_identity(&LevelSource::Path(PathBuf::from("/tmp/test.prl")), &root),
            "path:/tmp/test.prl"
        );
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
    fn relevel_does_not_restart_an_active_or_already_selected_catalog_load() {
        let mut app = test_app();
        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .replace_maps(vec![catalog_map("e1m1", "maps/e1m1.prl", "Entryway", &[])]);

        app.active_level_source = Some(LevelSource::Catalog("e1m1".to_string()));
        app.follow_relevel_catalog("e1m1".to_string());
        assert!(
            app.level_requests.is_empty(),
            "a relevel naming the active catalog map must not restart it"
        );

        app.active_level_source = None;
        app.level_load = Some(InFlightLevelLoad {
            map_path: PathBuf::from("content/dev/maps/e1m1.prl"),
            content_root: PathBuf::from("content/dev"),
            entry: LevelLoadEntry {
                catalog_id: Some("e1m1".to_string()),
                path: "maps/e1m1.prl".to_string(),
                name: "Entryway".to_string(),
                tags: Vec::new(),
            },
        });
        app.follow_relevel_catalog("e1m1".to_string());
        assert!(
            app.level_requests.is_empty(),
            "a relevel naming the in-flight catalog map must not restart it"
        );

        app.level_load = None;
        app.follow_relevel_catalog("e1m1".to_string());
        app.follow_relevel_catalog("e1m1".to_string());
        assert_eq!(
            app.level_requests,
            VecDeque::from([LevelRequest::Load(LevelSource::Catalog("e1m1".to_string()))]),
            "duplicate relevels queued before the lifecycle drain coalesce too"
        );
    }

    #[test]
    fn unknown_relevel_catalog_warns_and_does_not_queue_a_load() {
        let mut app = test_app();
        let logs = crate::scripting::reactions::log_capture::capture(|| {
            app.follow_relevel_catalog("missing-map".to_string());
        });

        assert!(
            logs.iter().any(|(level, message)| {
                *level == log::Level::Warn
                    && message.contains("[Net] relevel names unknown catalog id")
                    && message.contains("missing-map")
            }),
            "unknown catalog id must produce the pinned recoverable relevel warning: {logs:?}"
        );
        assert!(
            app.level_requests.is_empty(),
            "an unknown relevel catalog must not queue a load or otherwise alter the client"
        );
    }

    #[test]
    fn closing_control_surfaces_a_client_side_incompatible_host_diagnostic() {
        let mut app = test_app();
        let expected = postretro_net::wire::ProtocolVersion {
            app_protocol_id: 7,
            wire_version: 3,
        };
        let received = postretro_net::wire::ProtocolVersion {
            app_protocol_id: 8,
            wire_version: 3,
        };
        let logs = crate::scripting::reactions::log_capture::capture(|| {
            crate::netcode::client_drain_control(
                &mut app,
                vec![postretro_net::wire::ServerControlMessage::Divergence(
                    postretro_net::wire::DivergenceReason::Closing(
                        postretro_net::wire::ClosingCause::Protocol { expected, received },
                    ),
                )],
            );
        });

        assert!(
            logs.iter().any(|(level, message)| {
                *level == log::Level::Error
                    && message.contains("[Net] incompatible host")
                    && message.contains("protocol mismatch")
            }),
            "a typed admission refusal must reach the client-side incompatible-host diagnostic: {logs:?}"
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

    // Regression: staged mod-init reload omitted manifest trigger events, leaving
    // script-authored bindings absent while brush KVP bindings were restored.
    #[test]
    fn staged_recomposition_rebuilds_brush_and_manifest_trigger_bindings() {
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
            entities
                .set_tags(id, vec!["trap".to_string()])
                .expect("trigger tags attach");
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
            .replace_global_reactions(vec![
                scoped_global_set_state("plate_pressed", 1.0),
                scoped_global_set_state("script_pressed", 10.0),
            ]);
        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .replace_global_trigger_events(vec![TriggerEventDescriptor {
                tag: "trap".to_string(),
                event: "enter".to_string(),
                fire: vec!["script_pressed".to_string()],
                levels: Vec::new(),
            }]);
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
                        crate::trigger_system::TriggerEventEdge::Enter,
                        &mut entities,
                        &mut slots,
                        &crate::trigger_commands::TriggerFireContext::default(),
                    )
                    .residual()
                    .is_none(),
                "the direct bindings have no app-side residual"
            );
        }
        assert_eq!(
            script_ctx(&app)
                .slot_table
                .borrow()
                .get("trigger.flag")
                .and_then(|record| record.value.clone()),
            Some(SlotValue::Number(10.0)),
            "brush KVP binding runs before the appended manifest binding",
        );

        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .replace_global_reactions(vec![
                scoped_global_set_state("plate_pressed", 2.0),
                scoped_global_set_state("script_pressed", 20.0),
            ]);
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
                        crate::trigger_system::TriggerEventEdge::Enter,
                        &mut entities,
                        &mut slots,
                        &crate::trigger_commands::TriggerFireContext::default(),
                    )
                    .residual()
                    .is_none(),
                "the replacement direct bindings have no app-side residual"
            );
        }
        assert_eq!(
            script_ctx(&app)
                .slot_table
                .borrow()
                .get("trigger.flag")
                .and_then(|record| record.value.clone()),
            Some(SlotValue::Number(20.0)),
            "the post-reload binding must retain both the KVP and manifest commands",
        );
    }

    // Regression: filtering a trigger-scoped reaction from a crossing used to
    // discard all of the crossing's compatible reactions.
    #[test]
    fn crossing_filter_preserves_compatible_reactions_in_original_order() {
        let mut app = test_app();
        script_ctx(&app)
            .slot_table
            .borrow_mut()
            .insert("test.health".to_string(), number_slot(75.0))
            .expect("test slot should be vacant");
        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .replace_global_reactions(vec![
                postretro_entities::ScopedReaction {
                    reaction: NamedReaction {
                        name: "trigger_only".to_string(),
                        descriptor: ReactionDescriptor::Primitive(PrimitiveDescriptor {
                            primitive: "applyDamage".to_string(),
                            target: Some("@activators".to_string()),
                            tag: None,
                            on_complete: None,
                            args: serde_json::json!({ "amount": 10.0 }),
                        }),
                    },
                    levels: Vec::new(),
                },
                scoped_global_set_state("ordinary", 1.0),
            ]);
        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .replace_global_crossings(vec![postretro_entities::ScopedCrossing {
                crossing: CrossingDescriptor {
                    slot: Some("test.health".to_string()),
                    condition: CrossingCondition::Below { threshold: 0.5 },
                    max: 100.0,
                    edge: None,
                    fire: vec!["trigger_only".to_string(), "ordinary".to_string()],
                },
                levels: Vec::new(),
            }]);
        script_ctx(&app)
            .data_registry
            .borrow_mut()
            .recompose_active_sets(&[]);
        app.rebuild_active_reaction_subscribers();

        assert_eq!(
            script_ctx(&app).data_registry.borrow().crossings[0].fire,
            vec!["ordinary".to_string()],
        );
        script_ctx(&app)
            .slot_table
            .borrow_mut()
            .get_mut("test.health")
            .expect("test slot remains installed")
            .value = Some(SlotValue::Number(25.0));
        let ctx = script_ctx(&app);
        assert_eq!(
            app.session
                .as_mut()
                .expect("test app session installed")
                .crossing_detector
                .detect(&ctx.slot_table.borrow()),
            vec!["ordinary".to_string()],
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

    // The install path keys host-authoritative placement suppression off
    // `is_connected_client()`. Prove the role gate resolves correctly for each role:
    // only the connected client suppresses map placements.
    #[test]
    fn host_replicated_placement_suppression_gate_is_connected_client_only() {
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
            NetEndpoint::from_role(&NetRole::Host { port: 0 }, None)
                .expect("host endpoint constructs")
                .expect("host role yields an endpoint"),
        );
        assert!(
            !app.is_connected_client(),
            "listen host must keep map-placed AI enemies (it owns + replicates them)"
        );

        // Connected client: the only role that suppresses the local spawn.
        app.session.as_mut().unwrap().net_endpoint = Some(
            NetEndpoint::from_role(
                &NetRole::Connect {
                    addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 1)),
                },
                None,
            )
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
                command_diagnostics: Default::default(),
                mover_auto_close_ms: crate::runtime_movers::ENGINE_AUTO_CLOSE_MS,
                spawn_context: Default::default(),
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
                slot_accumulator_bindings: &mut session.scripting.slot_accumulator_bindings,
                impact_policy_runtime: &mut session.scripting.impact_policy_runtime,
                mesh_clip_tables: &mut session.mesh_clip_tables,
                hit_zone_store: &mut session.hit_zone_store,
                trigger_pool_policy: TriggerPoolSeedPolicy::ArmAll,
                suppress_ai_enemies: false,
                suppress_boot_pawn: false,
                local_carried_loadout: None,
            };
            // No-op mesh hook: headless-shaped, no renderer to upload models.
            let _ = install_world_cpu(
                handles,
                &mut timings,
                |_models, _clip_tables| {
                    crate::scripting_systems::hit_zones::ModelLoadWarningOwner::GameSide
                },
                |_spawn_points| {},
            );
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

    #[test]
    fn pinned_trigger_pool_install_selects_exact_counts_and_restarts_identically() {
        let pools = vec![
            pool_descriptor("closet_trap", TriggerPoolArm::Count(2), &[]),
            pool_descriptor(
                "ambush_trap",
                TriggerPoolArm::Percentage(50.0),
                &["trap-pools"],
            ),
        ];

        let first = install_trap_pool_fixture(
            TriggerPoolSeedPolicy::Seeded(17),
            &["trap-pools"],
            pools.clone(),
            false,
        );
        let restarted = install_trap_pool_fixture(
            TriggerPoolSeedPolicy::Seeded(17),
            &["trap-pools"],
            pools.clone(),
            false,
        );
        let different_seed = install_trap_pool_fixture(
            TriggerPoolSeedPolicy::Seeded(18),
            &["trap-pools"],
            pools,
            false,
        );

        assert_eq!(first.report.seed, Some(17));
        assert_eq!(
            first
                .report
                .pools
                .iter()
                .map(|pool| (pool.tag.as_str(), pool.members.len(), pool.selected.len()))
                .collect::<Vec<_>>(),
            [("closet_trap", 4, 2), ("ambush_trap", 4, 2)],
            "count and percentage pools must resolve against their actual installed members",
        );
        assert_eq!(first.armed_by_tag["closet_trap"].len(), 2);
        assert_eq!(first.armed_by_tag["ambush_trap"].len(), 2);
        assert_eq!(
            first.report, restarted.report,
            "the pinned policy reproduces the same armed identities on restart",
        );
        assert_ne!(
            first.report.pools[0].selected, different_seed.report.pools[0].selected,
            "the fixed distinct seeds deliberately select different closet members",
        );
    }

    #[test]
    fn headless_arm_all_fixture_ignores_pool_counts_and_keeps_no_seed() {
        let fixture = install_trap_pool_fixture(
            TriggerPoolSeedPolicy::ArmAll,
            &["trap-pools"],
            vec![
                pool_descriptor("closet_trap", TriggerPoolArm::Count(0), &[]),
                pool_descriptor(
                    "ambush_trap",
                    TriggerPoolArm::Percentage(0.0),
                    &["trap-pools"],
                ),
            ],
            false,
        );

        assert_eq!(fixture.report.seed, None);
        assert_eq!(fixture.armed_by_tag["closet_trap"].len(), 4);
        assert_eq!(fixture.armed_by_tag["ambush_trap"].len(), 4);
        assert!(
            fixture
                .report
                .pools
                .iter()
                .all(|pool| pool.members == pool.selected),
            "the headless default bypass arms every installed member without rolling",
        );
    }

    #[test]
    fn scoped_global_trigger_pool_matches_catalog_tags_but_not_direct_prl_paths() {
        let pools = vec![
            pool_descriptor("closet_trap", TriggerPoolArm::Count(2), &[]),
            pool_descriptor(
                "ambush_trap",
                TriggerPoolArm::Percentage(50.0),
                &["trap-pools"],
            ),
        ];
        let catalog_install = install_trap_pool_fixture(
            TriggerPoolSeedPolicy::Seeded(17),
            &["trap-pools"],
            pools.clone(),
            false,
        );
        let direct_prl_install =
            install_trap_pool_fixture(TriggerPoolSeedPolicy::Seeded(17), &[], pools, false);

        assert_eq!(catalog_install.report.pools.len(), 2);
        assert_eq!(catalog_install.armed_by_tag["ambush_trap"].len(), 2);
        assert_eq!(
            direct_prl_install
                .report
                .pools
                .iter()
                .map(|pool| pool.tag.as_str())
                .collect::<Vec<_>>(),
            ["closet_trap"],
            "an untagged direct .prl load must not match a levels-scoped mod pool",
        );
        assert!(
            direct_prl_install.armed_by_tag["ambush_trap"].is_empty(),
            "the client-authored false state remains untouched when the scoped pool is absent",
        );
    }

    #[test]
    fn connected_client_install_skips_pool_roll_and_preserves_authored_trigger_state() {
        let fixture = install_trap_pool_fixture(
            TriggerPoolSeedPolicy::Seeded(17),
            &["trap-pools"],
            vec![
                pool_descriptor("closet_trap", TriggerPoolArm::Count(2), &[]),
                pool_descriptor(
                    "ambush_trap",
                    TriggerPoolArm::Percentage(50.0),
                    &["trap-pools"],
                ),
            ],
            true,
        );

        assert!(fixture.report.pools.is_empty());
        assert_eq!(fixture.report.seed, None);
        assert_eq!(
            fixture.armed_by_tag["closet_trap"].len(),
            4,
            "the client keeps authored enabled_on_spawn=true rather than running the host roll",
        );
        assert!(
            fixture.armed_by_tag["ambush_trap"].is_empty(),
            "the client keeps authored enabled_on_spawn=false rather than running the host roll",
        );
    }
}
