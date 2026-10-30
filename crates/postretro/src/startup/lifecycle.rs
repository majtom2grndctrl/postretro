//! Level load requests, the Loading-frame step, and load failure; declares the
//! `lifecycle_*` siblings that install, finish and rebuild a level.
//! See: context/lib/boot_sequence.md §1 · §4

#[path = "lifecycle_boot_state.rs"]
mod lifecycle_boot_state;
#[cfg(test)]
#[path = "lifecycle_environment_install_tests.rs"]
pub(crate) mod lifecycle_environment_install_tests;
#[path = "lifecycle_finish.rs"]
mod lifecycle_finish;
#[path = "lifecycle_install.rs"]
mod lifecycle_install;
#[path = "lifecycle_install_presentation.rs"]
mod lifecycle_install_presentation;
#[cfg(test)]
#[path = "lifecycle_install_tests.rs"]
mod lifecycle_install_tests;
#[path = "lifecycle_install_world.rs"]
mod lifecycle_install_world;
#[path = "lifecycle_model_sweep.rs"]
mod lifecycle_model_sweep;
#[path = "lifecycle_net.rs"]
mod lifecycle_net;
#[cfg(test)]
#[path = "lifecycle_progress_install_tests.rs"]
mod lifecycle_progress_install_tests;
#[cfg(test)]
#[path = "lifecycle_reaction_install_tests.rs"]
mod lifecycle_reaction_install_tests;
#[path = "lifecycle_reaction_rebuild.rs"]
mod lifecycle_reaction_rebuild;
#[cfg(test)]
#[path = "lifecycle_reaction_rebuild_tests.rs"]
mod lifecycle_reaction_rebuild_tests;
#[cfg(test)]
#[path = "lifecycle_request_tests.rs"]
mod lifecycle_request_tests;
#[path = "lifecycle_spawn_residency.rs"]
mod lifecycle_spawn_residency;
#[path = "lifecycle_spawners.rs"]
mod lifecycle_spawners;
#[path = "lifecycle_sprite_collections.rs"]
mod lifecycle_sprite_collections;
#[cfg(test)]
#[path = "lifecycle_test_fixtures.rs"]
pub(in crate::startup) mod lifecycle_test_fixtures;
#[cfg(test)]
#[path = "lifecycle_unload_tests.rs"]
mod lifecycle_unload_tests;
#[path = "lifecycle_world_cpu.rs"]
mod lifecycle_world_cpu;
#[path = "lifecycle_world_handles.rs"]
mod lifecycle_world_handles;

use lifecycle_model_sweep::ParsedSweepModel;
use lifecycle_reaction_rebuild::{
    SubscriberRebuild, build_trigger_bindings, capture_progress_membership,
    rebuild_reaction_subscribers,
};
pub(crate) use lifecycle_spawners::{resolve_spawners_for_level, resolved_spawner_mesh_models};
pub(crate) use lifecycle_world_cpu::{install_world_cpu, start_installed_presentation_animations};
pub(crate) use lifecycle_world_handles::{
    WorldInstallHandles, WorldInstallProducts, bind_installed_first_player,
    install_world_gravity_and_nav,
};

use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;

use glam::Vec3;
use winit::event_loop::ActiveEventLoop;

use crate::App;
use crate::render;
use crate::scripting::builtins::descriptor_materializes_ai_enemy;
use crate::startup::loading_screen::LoadingStep;
use crate::startup::{
    BootState, InFlightLevelLoad, LevelLoadEntry, LevelSource, LoadOutcome, StartupTimings,
    spawn_level_worker,
};
use crate::trigger_bindings::TriggerBindingTable;
use crate::trigger_pools::{
    TriggerPoolInstallReport, TriggerPoolSeedPolicy, install_trigger_pools,
};
use postretro_scripting_core::reaction_dispatch::validate_sequence_primitives;

#[cfg(test)]
use crate::camera::Camera;

pub(crate) const FRONTEND_CLEAR_COLOR: render::ClearColor = render::ClearColor {
    r: 0.015,
    g: 0.018,
    b: 0.024,
    a: 1.0,
};

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

enum LoadingPoll {
    Pending,
    Disconnected,
    Ready(Box<LoadOutcome>),
}

impl App {
    pub(crate) fn has_installed_level(&self) -> bool {
        self.level_is_installed_state() && self.level.is_some()
    }

    pub(super) fn resolve_level_source(&self, source: LevelSource) -> Option<InFlightLevelLoad> {
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
                        loading_tree: entry.loading_tree,
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
                        loading_tree: Vec::new(),
                    },
                    map_path,
                })
            }
        }
    }

    pub(super) fn begin_level_load(&mut self, load: InFlightLevelLoad) {
        self.level_timings = StartupTimings::new();
        let progress = self.begin_loading_screen(&load.entry);
        let (tx, rx) = mpsc::channel();
        let handle = spawn_level_worker(
            load.map_path.clone(),
            load.content_root.clone(),
            self.baked_root.clone(),
            progress,
            tx,
        );
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
        match self.next_loading_step() {
            LoadingStep::Install(payload) => {
                self.finish_level_payload(*payload, event_loop);
                false
            }
            LoadingStep::Fail(reason) => {
                self.finish_level_failure(reason, event_loop);
                false
            }
            LoadingStep::Paint => {
                // Loading redraws unconditionally; painting never advances state.
                self.paint_loading_frame(event_loop, frame_dt);
                self.request_redraw();
                false
            }
        }
    }

    /// Decide one Loading frame. A delivered payload is held for one painted
    /// frame (the bar at its parse share), then installs on the next. A
    /// payload without a level has nothing to install, so it goes straight to
    /// the failure path without a held frame.
    pub(super) fn next_loading_step(&mut self) -> LoadingStep {
        if let Some(payload) = self.take_deferred_level_payload() {
            // The held frame is its own stage, neither worker nor install time.
            self.level_timings.record("install_deferral");
            return LoadingStep::Install(Box::new(payload));
        }
        match self.poll_loading_level_worker() {
            LoadingPoll::Ready(outcome) => match *outcome {
                Ok(mut payload) => {
                    self.record_worker_delivery(&mut payload);
                    if payload.level.is_none() {
                        return LoadingStep::Install(Box::new(payload));
                    }
                    match self.defer_level_payload(payload) {
                        Some(payload) => LoadingStep::Install(Box::new(payload)),
                        None => LoadingStep::Paint,
                    }
                }
                Err(err) => LoadingStep::Fail(format!("worker failed: {err:#}")),
            },
            LoadingPoll::Disconnected => {
                LoadingStep::Fail("worker channel disconnected before delivery".to_string())
            }
            LoadingPoll::Pending => LoadingStep::Paint,
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

    /// Mark the worker's delivery in the level timings, on the frame it lands.
    fn record_worker_delivery(&mut self, payload: &mut crate::startup::worker::LevelPayload) {
        // The worker's own stages (`prl_parse`) ran inside the interval
        // `worker_delivered` measures, so they attach to it rather than join the
        // entries: summing the line's stages must not count the parse twice.
        let inside = std::mem::take(&mut payload.timings);
        self.level_timings
            .record_containing("worker_delivered", inside);
    }

    fn finish_level_failure(&mut self, reason: String, event_loop: &ActiveEventLoop) {
        self.level_load = None;
        self.end_loading_screen();
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
}

/// Fixture paths the crate's tests import as `lifecycle::tests::*`.
#[cfg(test)]
pub(crate) mod tests {
    pub(super) use super::lifecycle_test_fixtures::descriptor;
    pub(crate) use super::lifecycle_test_fixtures::{
        install_capture_world, install_descriptor_world, level_world, runtime_audience_manifest,
        script_ctx, test_app,
    };
}
