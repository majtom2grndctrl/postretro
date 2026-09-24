//! Where the engine-owned trees a distribution ships come from.
//!
//! `core/` — and, for an SDK bundle, `sdk/`, `docs/` and `tools/` — belong to the
//! engine, not to the game, and they resolve under the **install root only**.
//! The project root is never consulted for them.
//!
//! That is not a convenience choice. `core/` is a single path component
//! precisely so `--mod` cannot redirect it (D1); letting a lookup fall back from
//! the project to the install would hand the same power back through a different
//! door, with a game tree able to shadow engine assets purely by existing. It
//! would also make the dev checkout the primary case and the shipped layout its
//! fallback, which is backwards.
//!
//! The install root is named by `--install-root <dir>` or derived from the
//! tool's own executable location. A workspace checkout is the one layout where
//! the derivation cannot see it — the tool sits in `target/debug` with no `core/`
//! beside it — so `xtask` passes `--install-root <workspace>` explicitly, which
//! keeps the rule here single and unconditional and puts the checkout-specific
//! knowledge in the one crate entitled to it.
//!
//! A missing tree is an error, never a skip: `load_named_tree` degrades a missing
//! descriptor to a `warn!` and keeps booting, so a payload assembled without
//! `core/` ships with the pause menu, frontend menu and on-screen keyboard simply
//! gone, and nothing in the exit code to say so.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging · ui.md §5

use std::path::{Path, PathBuf};

use crate::binaries::tool_directory;
use crate::sdk_dist::BIN_DIR;

/// The engine-owned tree `dist` ships. `sdk-dist` ships it too, plus its own.
pub(crate) const CORE_TREE: &str = "core";

/// The trees an SDK bundle carries beyond the payload's, in copy order.
pub(crate) const BUNDLE_TREES: [&str; 4] = ["sdk", "docs", "tools", CORE_TREE];

/// The flag that names the install root, quoted in every failure it explains.
pub(crate) const INSTALL_ROOT_FLAG: &str = "--install-root";

/// Resolve one engine-owned tree under `install_root`.
pub(crate) fn resolve(install_root: &Path, name: &str) -> Result<PathBuf, String> {
    let path = install_root.join(name);
    if path.is_dir() {
        return Ok(path);
    }
    Err(format!(
        "engine-owned tree `{name}/` not found at {}.\n\n\
         `{name}/` belongs to the engine install, not to your game — a project is \
         never consulted for it. Run the tool from an install that carries it, or \
         name the install with {INSTALL_ROOT_FLAG} <dir>.",
        path.display()
    ))
}

/// The install root implied by the tool's own location.
///
/// One deterministic step, not a search: a bundle puts the tool in `bin/`, so the
/// install root is that directory's parent; anywhere else the tool's own
/// directory is the install root. Deriving it from `BIN_DIR` keeps it in step
/// with the layout `sdk-dist` writes. Nothing here probes the filesystem for a
/// marker — a checkout that this cannot see names itself with
/// `--install-root`.
pub(crate) fn default_install_root() -> Result<PathBuf, String> {
    let directory = tool_directory()?;
    match directory.file_name().and_then(|name| name.to_str()) {
        Some(BIN_DIR) => directory.parent().map(Path::to_path_buf).ok_or_else(|| {
            format!(
                "could not derive an install root from {}; name it with {INSTALL_ROOT_FLAG} <dir>",
                directory.display()
            )
        }),
        _ => Ok(directory),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempRoot {
        root: PathBuf,
    }

    impl TempRoot {
        fn new(label: &str) -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time follows Unix epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "postretro-engine-trees-{label}-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("temporary tree created");
            Self { root }
        }

        fn dir(&self, relative: &str) -> PathBuf {
            let path = self.root.join(relative);
            fs::create_dir_all(&path).expect("directory created");
            path
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn a_tree_resolves_under_the_install_root() {
        let temp = TempRoot::new("install");
        let install = temp.dir("bundle");
        let core = temp.dir("bundle/core");

        assert_eq!(resolve(&install, CORE_TREE), Ok(core));
    }

    /// The shadowing case, and the reason the rule is install-only. A game that
    /// happens to have a `core/` of its own does not get to replace the engine's
    /// — that is exactly the power D1 removed by lifting these assets out of
    /// `content/base`, and a project-first lookup would hand it straight back.
    #[test]
    fn a_project_tree_of_the_same_name_is_never_consulted() {
        let temp = TempRoot::new("shadow");
        let project = temp.dir("game");
        let project_core = temp.dir("game/core");
        let install = temp.dir("bundle");
        let install_core = temp.dir("bundle/core");
        fs::write(project_core.join("marker"), "the game's own").unwrap();

        let resolved = resolve(&install, CORE_TREE).expect("the install supplies it");
        assert_eq!(resolved, install_core);
        assert_ne!(resolved, project_core);

        // And with the install lacking it, a project copy does not rescue the run.
        let bare_install = temp.dir("bare");
        let error = resolve(&bare_install, CORE_TREE)
            .expect_err("a project copy cannot stand in for the install's");
        assert!(
            error.contains(&bare_install.join(CORE_TREE).display().to_string()),
            "{error}"
        );
        let _ = project;
    }

    /// A tree missing under the install is an error, not a skip: the engine
    /// degrades a missing `core/` to warnings, so a silent pass ships a game with
    /// three screens gone.
    #[test]
    fn a_missing_tree_fails_loudly_naming_the_path_and_the_flag() {
        let temp = TempRoot::new("missing");
        let install = temp.dir("bundle");

        let error =
            resolve(&install, CORE_TREE).expect_err("a missing engine tree cannot pass silently");
        assert!(
            error.contains(&install.join(CORE_TREE).display().to_string()),
            "{error}"
        );
        assert!(error.contains(INSTALL_ROOT_FLAG), "{error}");
    }

    /// A file named `core` is not a tree. Without the directory check the copy
    /// would fail later with a bare read_dir error instead of this explanation.
    #[test]
    fn a_file_by_the_right_name_does_not_satisfy_a_tree() {
        let temp = TempRoot::new("file");
        let install = temp.dir("bundle");
        fs::write(install.join(CORE_TREE), "not a tree").expect("file written");

        assert!(resolve(&install, CORE_TREE).is_err());
    }

    #[test]
    fn the_bundle_tree_list_covers_every_engine_owned_directory_sdk_dist_ships() {
        assert!(BUNDLE_TREES.contains(&CORE_TREE));
        assert_eq!(BUNDLE_TREES, ["sdk", "docs", "tools", "core"]);
    }
}
