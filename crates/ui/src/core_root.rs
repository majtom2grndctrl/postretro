// Where the engine's own assets live, carried as a value instead of a convention.
//
// `core/` holds the four built-in UI descriptors and the boot splash. It is
// engine-owned: `--mod` never redirects it, and it is deliberately a single path
// component, outside the `content/<mod>` shape a mod root must have. What it
// is *not* is fixed to the working directory — a launcher that pins the cwd to a
// game project (where no `core/` exists, correctly) still has to tell the engine
// where the engine's own assets are, which is what `--core-root` names.
//
// The root is a type rather than a bare `PathBuf` so every engine-asset lookup
// has to name a root to compile. A future loader cannot reintroduce a bare
// working-directory lookup by writing a string literal, because no free function
// builds one: the only way back to the old behaviour is to ask for it out loud,
// through `CoreRoot::working_directory`.
//
// See: context/lib/ui.md §5

use std::path::{Path, PathBuf};

/// `core/`, relative to the working directory. The engine runs with the tree
/// holding `core/` and `content/` as its working directory — the workspace root
/// in a checkout, the payload root in a packaged build — so this is what every
/// launch resolved before `--core-root` existed, and what one without the flag
/// resolves still.
const WORKING_DIRECTORY_CORE: &str = "core";

/// Subdirectory holding the built-in UI descriptor trees.
const UI_SUBDIR: &str = "ui";

/// The directory holding the engine's own `ui/` and `textures/` trees.
///
/// Independent of the mod content root in both directions: relocating engine
/// assets never moves game content, and `--mod` never reaches these.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreRoot(PathBuf);

impl CoreRoot {
    /// The pre-flag convention: `core/` under the working directory.
    pub fn working_directory() -> Self {
        Self(PathBuf::from(WORKING_DIRECTORY_CORE))
    }

    /// The directory named by `--core-root <dir>`. It *holds* `ui/` and
    /// `textures/` — it is the `core/` directory itself, not the tree root
    /// containing it. `postretro-tool run` passes `<install>/core`.
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self(dir.into())
    }

    /// Resolve the flag: present names the root, absent keeps
    /// [`CoreRoot::working_directory`] byte for byte.
    pub fn from_flag(dir: Option<PathBuf>) -> Self {
        dir.map_or_else(Self::working_directory, Self::at)
    }

    /// The root itself, for logging and for callers assembling their own paths.
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// One engine asset, named by its path relative to `core/` — the boot
    /// splash's route. Descriptor trees use [`CoreRoot::ui_asset_path`].
    pub fn asset_path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    /// One built-in UI descriptor, by file name (`hud.json`, `pauseMenu.json`,
    /// `frontendMenu.json`, `keyboard.json`).
    pub fn ui_asset_path(&self, file_name: &str) -> PathBuf {
        self.0.join(UI_SUBDIR).join(file_name)
    }
}

impl Default for CoreRoot {
    fn default() -> Self {
        Self::working_directory()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The no-flag case is the whole compatibility contract: every existing
    /// launch, dev run and test resolves the same paths it resolved before the
    /// flag existed.
    #[test]
    fn without_a_flag_every_asset_resolves_where_it_always_did() {
        let root = CoreRoot::from_flag(None);
        assert_eq!(root, CoreRoot::working_directory());
        assert_eq!(root.path(), Path::new("core"));
        for file in [
            "hud.json",
            "pauseMenu.json",
            "frontendMenu.json",
            "keyboard.json",
        ] {
            assert_eq!(
                root.ui_asset_path(file),
                PathBuf::from("core/ui").join(file),
                "the pre-flag working-directory path, unchanged",
            );
        }
        assert_eq!(
            root.asset_path("textures/splash/postretro-ascii-art.png"),
            PathBuf::from("core/textures/splash/postretro-ascii-art.png"),
        );
    }

    /// With the flag, both consumers resolve under the named directory — and the
    /// value is the `core/` directory itself, not a tree root above it.
    #[test]
    fn a_named_root_relocates_both_the_descriptors_and_the_splash() {
        let root = CoreRoot::from_flag(Some(PathBuf::from("/install/core")));
        assert_eq!(
            root.ui_asset_path("pauseMenu.json"),
            PathBuf::from("/install/core/ui/pauseMenu.json"),
        );
        assert_eq!(
            root.asset_path("textures/splash/postretro-ascii-art.png"),
            PathBuf::from("/install/core/textures/splash/postretro-ascii-art.png"),
        );
    }
}
