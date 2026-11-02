//! Level source resolution, level identity and the one load-failure route;
//! declares the `lifecycle_*` siblings that load, install, finish and rebuild a level.
//! See: context/lib/boot_sequence.md §1 · §4

#[path = "lifecycle_beside.rs"]
mod lifecycle_beside;
#[path = "lifecycle_boot_state.rs"]
mod lifecycle_boot_state;
#[path = "lifecycle_commit.rs"]
mod lifecycle_commit;
#[cfg(test)]
#[path = "lifecycle_commit_tests.rs"]
mod lifecycle_commit_tests;
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
#[cfg(test)]
#[path = "lifecycle_life_state_tests.rs"]
mod lifecycle_life_state_tests;
#[path = "lifecycle_load.rs"]
mod lifecycle_load;
#[cfg(test)]
#[path = "lifecycle_load_tests.rs"]
mod lifecycle_load_tests;
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
#[path = "lifecycle_relevel_tests.rs"]
mod lifecycle_relevel_tests;
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

#[cfg(test)]
pub(crate) use lifecycle_model_sweep::parse_sweep_models;
pub(crate) use lifecycle_model_sweep::{ModelClipEntry, model_clip_entry};
use lifecycle_reaction_rebuild::{
    SubscriberRebuild, build_trigger_bindings, capture_progress_membership,
    rebuild_reaction_subscribers,
};
pub(crate) use lifecycle_spawners::{resolve_spawners_for_level, resolved_spawner_mesh_models};
#[cfg(any(test, feature = "observability"))]
pub(crate) use lifecycle_world_cpu::install_world_cpu;
pub(crate) use lifecycle_world_cpu::{
    install_world_cpu_with_models, is_region_companion, start_installed_presentation_animations,
};
pub(crate) use lifecycle_world_handles::{
    WorldInstallHandles, WorldInstallProducts, bind_installed_first_player,
    install_world_gravity_and_nav,
};

use std::path::{Component, Path, PathBuf};

use glam::Vec3;
use winit::event_loop::ActiveEventLoop;

use crate::App;
use crate::render;
use crate::scripting::builtins::descriptor_materializes_ai_enemy;
use crate::startup::{
    BootState, InFlightLevelLoad, LevelLoadEntry, LevelSource, LoopExit, StartupTimings,
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

    /// The one failure route every level load ends through, whatever failed
    /// (a stage, the upload, an uncaptured wgpu error, the commit's checks,
    /// or the main-thread oracle's install): the load's stages stop and
    /// retire, so nothing reports twice, then a CLI boot load exits non-zero
    /// and a runtime load enters Frontend. A load beside a running level
    /// takes the route's beside half instead: reported once, its stages
    /// retiring, and the running level left Running (pin P31).
    fn finish_level_failure(&mut self, reason: String, event_loop: &impl LoopExit) {
        if self.level_loads.beside_active() {
            self.finish_beside_level_failure(&reason);
            return;
        }
        self.retire_active_level_load();
        let level = self
            .level_load
            .take()
            .map(|load| failed_level_label(&load.entry))
            .unwrap_or_else(|| "the level".to_string());
        self.end_loading_screen();
        let was_boot_load = std::mem::take(&mut self.boot_load);
        if was_boot_load {
            log::error!(
                "[Loader] loading {level} failed: {reason}; boot map load failed, exiting non-zero"
            );
            self.exit_result = Err(anyhow::anyhow!("{reason}"));
            event_loop.exit();
            return;
        }

        // Say why the player lands on Frontend: a loading-screen load
        // unloads the previous level before it starts, so a failure leaves
        // no level to return to.
        log::error!(
            "[Loader] loading {level} failed: {reason}; entering frontend, since no level is \
             installed to return to (a loading-screen load unloads the previous level before \
             it starts)"
        );
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.clear_splash();
        }
        self.boot_state = BootState::Frontend;
        self.request_redraw();
    }
}

/// How a failure line names the level that failed: its catalog id and path,
/// or the path alone for a raw-path load.
fn failed_level_label(entry: &LevelLoadEntry) -> String {
    match &entry.catalog_id {
        Some(id) => format!("`{id}` ({})", entry.path),
        None => format!("`{}`", entry.path),
    }
}

/// Fixture paths the crate's tests import as `lifecycle::tests::*`.
#[cfg(test)]
pub(crate) mod tests {
    pub(super) use super::lifecycle_test_fixtures::descriptor;
    pub(crate) use super::lifecycle_test_fixtures::{
        drop_in_flight_load, install_capture_world, install_descriptor_world, level_world,
        runtime_audience_manifest, script_ctx, test_app,
    };
}
