// Top-level scripting runtime types: the `ScriptRuntime` handle, its config and
// result types, and the hot-reload dependency classifier.
// See: context/lib/scripting.md

#[cfg(debug_assertions)]
use std::collections::BTreeSet;
#[cfg(debug_assertions)]
use std::ffi::OsString;
#[cfg(debug_assertions)]
use std::path::Path;
#[cfg(debug_assertions)]
use std::path::PathBuf;

use crate::ctx::ScriptCtx;
use crate::data_descriptors::{
    EntityTypeDescriptor, ModFontAssets, ModThemeTokens, RegisteredUiTree,
};
use crate::data_registry::{ScopedCrossing, ScopedReaction};
pub use crate::foundation_pods::ModMapEntry;
use crate::luau::{LuauConfig, LuauSubsystem, Which as LuauWhich};
use crate::quickjs::{QuickJsConfig, QuickJsSubsystem};
use crate::slot_table::StoreDeclarationSet;
#[cfg(debug_assertions)]
use crate::staged_manifest::StagedManifestBuildLane;

#[derive(Clone, Debug, PartialEq)]
pub struct MenuCamera {
    pub position: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Frontend {
    pub menu_tree: String,
    pub background_level: Option<String>,
    pub camera: MenuCamera,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModManifestResult {
    pub name: String,
    /// Entity-type descriptors returned by the mod manifest. Empty when the
    /// returned object omits the `entities` field. Drained into `DataRegistry`
    /// by the boot caller after `run_mod_init` returns.
    pub entities: Vec<EntityTypeDescriptor>,
    /// UI trees registered via the mod manifest's `uiTrees` field (each a name +
    /// `AnchoredTree` + `alwaysOn`). Empty when absent. A malformed entry is
    /// logged and skipped at parse time (`ui.md` §1.1). Drained into the app-side
    /// `UiTreeRegistry` at `ScopeTier::Mod` by the boot caller in `main.rs`.
    pub ui_trees: Vec<RegisteredUiTree>,
    /// Theme tokens from the mod manifest's `theme` field. Default (empty) when
    /// absent. Drained into the `ThemeDescriptor` merge by the boot caller.
    pub theme: ModThemeTokens,
    /// Mod frontend declaration from the manifest's `frontend` field. A
    /// successful staged mod-init commit replaces this snapshot whole; omission
    /// returns the app to its fallback frontend.
    pub frontend: Option<Frontend>,
    /// Font assets (family → TTF path) from the mod manifest's `fonts` field.
    /// Default (empty) when absent. Installed via `register_ui_font` by the
    /// boot caller.
    pub fonts: ModFontAssets,
    /// Map catalog entries from the mod manifest's `maps` field. Empty when absent.
    /// Drained into `DataRegistry` by the boot caller after `run_mod_init`
    /// returns.
    pub maps: Vec<ModMapEntry>,
    /// Engine-global reaction definitions from the mod manifest's `reactions`
    /// field. Empty when absent. Drained into `DataRegistry` by the boot caller.
    pub reactions: Vec<ScopedReaction>,
    /// Engine-global crossing definitions from the mod manifest's `crossings`
    /// field. Empty when absent. Drained into `DataRegistry` by the boot caller.
    pub crossings: Vec<ScopedCrossing>,
    /// Validated state-store declarations collected during this mod-init
    /// attempt. This is engine metadata, not a `ModManifest` script field.
    pub store_declarations: StoreDeclarationSet,
}

/// Aggregated reload signal returned by
/// [`ScriptRuntime::drain_reload_requests`]. Defined here (rather than under
/// the debug-only `watcher` module) so release builds can refer to it
/// without `cfg` gates at every call site.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReloadSummary {
    /// At least one changed path matched the active mod-init dependency set;
    /// the engine should queue a staged manifest build.
    pub mod_init: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagedManifestCommitOutcome {
    Committed {
        generation: u64,
        descriptor_count: usize,
        applied_actions: usize,
        dropped_missing_targets: usize,
    },
    DiscardedStale {
        generation: u64,
        latest_requested: Option<u64>,
    },
    FailedBuild {
        generation: u64,
    },
    Rejected {
        generation: u64,
        reason: String,
    },
    ReleaseNoop,
}

#[cfg(debug_assertions)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ActiveModInitDependencies {
    mod_root: PathBuf,
    state: ActiveModInitDependencyState,
}

#[cfg(debug_assertions)]
#[derive(Clone, Debug, PartialEq, Eq)]
enum ActiveModInitDependencyState {
    Active {
        dependencies: BTreeSet<PathBuf>,
    },
    NoStartScript {
        candidate_entries: BTreeSet<PathBuf>,
    },
}

#[cfg(debug_assertions)]
impl ActiveModInitDependencies {
    pub(super) fn from_dependencies<'a>(
        mod_root: &Path,
        dependency_paths: impl IntoIterator<Item = &'a PathBuf>,
    ) -> Result<Self, String> {
        let mod_root = normalize_existing_path(mod_root)?;
        let mut dependencies = BTreeSet::new();
        for path in dependency_paths {
            let normalized = normalize_changed_path(path)?;
            // Out-of-root deps (e.g. shared `sdk/behaviors/` code) are dropped, not
            // committed: editing them will not hot-reload, but they must not fail the
            // whole staged build and disable hot reload for the in-root mod scripts.
            if !normalized.starts_with(&mod_root) {
                log::debug!(
                    "[Scripting] dependency `{}` dropped: outside active mod root `{}`; edits to it will not trigger hot reload",
                    normalized.display(),
                    mod_root.display(),
                );
                continue;
            }
            dependencies.insert(normalized);
        }
        Ok(Self {
            mod_root,
            state: ActiveModInitDependencyState::Active { dependencies },
        })
    }

    pub(super) fn no_start_script(mod_root: &Path) -> Result<Self, String> {
        let mod_root = normalize_existing_path(mod_root)?;
        let candidate_entries = candidate_start_script_paths(&mod_root);
        Ok(Self {
            mod_root,
            state: ActiveModInitDependencyState::NoStartScript { candidate_entries },
        })
    }

    pub(super) fn len(&self) -> usize {
        match &self.state {
            ActiveModInitDependencyState::Active { dependencies } => dependencies.len(),
            ActiveModInitDependencyState::NoStartScript { candidate_entries } => {
                candidate_entries.len()
            }
        }
    }

    pub(super) fn changed_paths_affect_mod_init(&self, paths: &[PathBuf]) -> bool {
        let mut matched = false;
        for raw_path in paths {
            let normalized = match normalize_changed_path(raw_path) {
                Ok(path) => path,
                Err(err) => {
                    log::debug!(
                        "[Scripting] changed path `{}` ignored: could not normalize path: {err}",
                        raw_path.display(),
                    );
                    continue;
                }
            };

            if !normalized.starts_with(&self.mod_root) {
                log::debug!(
                    "[Scripting] changed path `{}` ignored: outside active mod root `{}`",
                    normalized.display(),
                    self.mod_root.display(),
                );
                continue;
            }

            match &self.state {
                ActiveModInitDependencyState::Active { dependencies } => {
                    if dependencies.contains(&normalized) {
                        log::debug!(
                            "[Scripting] changed path `{}` triggers staged mod-init: active dependency",
                            normalized.display(),
                        );
                        matched = true;
                    } else {
                        log::debug!(
                            "[Scripting] changed path `{}` ignored: not in active mod-init dependency set",
                            normalized.display(),
                        );
                    }
                }
                ActiveModInitDependencyState::NoStartScript { candidate_entries } => {
                    if candidate_entries.contains(&normalized) {
                        log::debug!(
                            "[Scripting] changed path `{}` triggers staged mod-init: start-script appeared after debug no-op",
                            normalized.display(),
                        );
                        matched = true;
                    } else {
                        log::debug!(
                            "[Scripting] changed path `{}` ignored: no active start-script and not an entry candidate",
                            normalized.display(),
                        );
                    }
                }
            }
        }
        matched
    }
}

#[cfg(debug_assertions)]
fn candidate_start_script_paths(mod_root: &Path) -> BTreeSet<PathBuf> {
    ["start-script.ts", "start-script.js", "start-script.luau"]
        .into_iter()
        .map(|name| mod_root.join(name))
        .collect()
}

#[cfg(debug_assertions)]
fn normalize_existing_path(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|e| format!("failed to canonicalize `{}`: {e}", path.display()))
}

#[cfg(debug_assertions)]
pub(super) fn normalize_changed_path(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| format!("failed to read current directory: {e}"))?
            .join(path)
    };

    if let Ok(canonical) = absolute.canonicalize() {
        return Ok(canonical);
    }

    let mut probe = absolute.clone();
    let mut missing_segments: Vec<OsString> = Vec::new();
    loop {
        if probe.exists() {
            let mut normalized = probe
                .canonicalize()
                .map_err(|e| format!("failed to canonicalize `{}`: {e}", probe.display()))?;
            for segment in missing_segments.iter().rev() {
                normalized.push(segment);
            }
            return Ok(normalized);
        }

        if let Some(name) = probe.file_name() {
            missing_segments.push(name.to_os_string());
        }

        if !probe.pop() {
            break;
        }
    }

    Err(format!(
        "no existing parent found while normalizing `{}`",
        path.display()
    ))
}

/// Which scripting scope a given call targets. The subsystem-level `Which`
/// types (QuickJS, Luau) are private to their modules; this is the
/// engine-facing selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Definition,
}

impl From<Which> for LuauWhich {
    fn from(w: Which) -> Self {
        match w {
            Which::Definition => LuauWhich::Definition,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ScriptRuntimeConfig {
    pub quickjs: QuickJsConfig,
    pub luau: LuauConfig,
}

pub struct ScriptRuntime {
    pub(super) quickjs: QuickJsSubsystem,
    pub(super) luau: LuauSubsystem,
    /// Validated mod manifest value, populated by `run_mod_init`.
    /// `None` until `run_mod_init` succeeds; in debug builds may also remain
    /// `None` if no `start-script.{js,luau}` was found at the mod root.
    pub(super) mod_manifest: Option<ModManifestResult>,
    /// Dev-mode hot-reload watcher. Debug builds only; release builds omit
    /// the field so `drain_reload_requests` is a no-op with no extra code.
    #[cfg(debug_assertions)]
    pub(super) watcher: Option<crate::watcher::ScriptWatcher>,
    #[cfg(debug_assertions)]
    pub(super) staged_manifest_lane: Option<StagedManifestBuildLane>,
    #[cfg(debug_assertions)]
    pub(super) active_mod_init_dependencies: Option<ActiveModInitDependencies>,
    pub(super) script_ctx: ScriptCtx,
    pub(super) cfg: ScriptRuntimeConfig,
}
