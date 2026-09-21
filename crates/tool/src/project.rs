//! Project discovery: the `postretro.toml` marker and every path derived from it.
//!
//! `postretro-tool` ships inside distributions, so it can resolve nothing at
//! compile time — `env!("CARGO_MANIFEST_DIR")` would bake the build machine's
//! absolute path into a binary a stranger runs (`ui.md` §5). Every path the tool
//! touches therefore hangs off one runtime anchor: the directory holding the
//! nearest `postretro.toml`, found by walking parents of the working directory
//! the way cargo finds `Cargo.toml`.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging

use std::path::{Path, PathBuf};

use crate::manifest::Manifest;

/// The project marker. Its directory is the project root.
pub(crate) const MARKER_FILE: &str = "postretro.toml";

/// Disposable build scratch, beside the project rather than inside its content
/// (`build_pipeline.md` §Build Cache). `prl-build`'s own fallback would land it
/// among the author's `.map` sources, so the tool always names it explicitly.
const CACHE_DIR: &str = ".build-caches";

/// A discovered project: its root, its marker, and the parsed manifest.
#[derive(Debug, Clone)]
pub(crate) struct Project {
    root: PathBuf,
    manifest_path: PathBuf,
    manifest: Manifest,
}

impl Project {
    /// Walk parents of `start` for the nearest `postretro.toml` and open it.
    pub(crate) fn discover(start: &Path) -> Result<Self, String> {
        let manifest_path = find_marker(start, |path| path.is_file()).ok_or_else(|| {
            format!(
                "no {MARKER_FILE} found in {} or any parent directory\n\n\
                 A Postretro project is a directory holding {MARKER_FILE}. Run the \
                 tool from inside one, or pass --manifest <path>.",
                start.display()
            )
        })?;
        Self::open(&manifest_path)
    }

    /// Open a project from an explicit marker path; its parent is the root.
    pub(crate) fn open(manifest_path: &Path) -> Result<Self, String> {
        let manifest = Manifest::read(manifest_path)?;
        let root = manifest_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .ok_or_else(|| {
                format!(
                    "project manifest {} has no parent directory to use as the project root",
                    manifest_path.display()
                )
            })?;
        Ok(Self {
            root,
            manifest_path: manifest_path.to_path_buf(),
            manifest,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    pub(crate) fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// Resolve a project-relative path recorded in the manifest.
    pub(crate) fn join(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// The authored mod tree this project builds from.
    pub(crate) fn mod_root(&self) -> PathBuf {
        self.join(&self.manifest.package.mod_root)
    }

    /// The directory that *contains* `materials/` — what both `prl-build` and
    /// the engine mean by `--baked-root` (`build_pipeline.md` §Baked texture
    /// mips). In the engine's own repository this is `<workspace>/baked`, the
    /// same directory the compiler's `Cargo.toml` walk resolves.
    pub(crate) fn baked_root(&self) -> PathBuf {
        self.join("baked")
    }

    pub(crate) fn materials_root(&self) -> PathBuf {
        self.baked_root().join("materials")
    }

    /// `prl-build`'s stage cache. The `prl-cache` leaf matches the compiler's own
    /// default layout, so naming it explicitly relocates nothing for an in-repo
    /// project — it only stops the fallback landing in the map's own directory.
    pub(crate) fn stage_cache_dir(&self) -> PathBuf {
        self.join(CACHE_DIR).join("prl-cache")
    }

    /// Scratch for the emitted entry script. Outside the payload root, so the
    /// stages before assembly still write nothing into it.
    pub(crate) fn scratch_root(&self) -> PathBuf {
        self.join(CACHE_DIR).join("dist-work")
    }

    /// The default distribution output root. Payload containment anchors here.
    pub(crate) fn output_root(&self) -> PathBuf {
        self.join("dist")
    }

    /// Build a project without touching a filesystem, for tests elsewhere in the
    /// crate that need one of its derived paths.
    #[cfg(test)]
    pub(crate) fn for_test(root: &str, name: &str, mod_root: &str) -> Self {
        let root = PathBuf::from(root);
        Self {
            manifest_path: root.join(MARKER_FILE),
            manifest: Manifest::parse(&format!(
                "[package]\nname = \"{name}\"\nmod_root = \"{mod_root}\"\n"
            ))
            .expect("test project manifest parses"),
            root,
        }
    }
}

/// Walk `start` and its parents for the first directory holding the marker.
///
/// `is_file` is injected so the walk itself is testable without a filesystem.
fn find_marker(start: &Path, is_file: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let mut candidate = Some(start);
    while let Some(directory) = candidate {
        let marker = directory.join(MARKER_FILE);
        if is_file(&marker) {
            return Some(marker);
        }
        candidate = directory.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_walk_stops_at_the_nearest_ancestor_holding_it() {
        let root = Path::new("/projects/game");
        let marker = root.join(MARKER_FILE);

        assert_eq!(
            find_marker(Path::new("/projects/game/levels/maps"), |path| path
                == marker),
            Some(marker.clone()),
        );
        assert_eq!(find_marker(root, |path| path == marker), Some(marker));
    }

    #[test]
    fn marker_walk_prefers_the_deepest_project_when_projects_nest() {
        let outer = Path::new("/projects/game").join(MARKER_FILE);
        let inner = Path::new("/projects/game/expansion").join(MARKER_FILE);

        assert_eq!(
            find_marker(Path::new("/projects/game/expansion/maps"), |path| path
                == outer
                || path == inner),
            Some(inner),
        );
    }

    #[test]
    fn marker_walk_reports_absence_rather_than_guessing_a_root() {
        assert_eq!(find_marker(Path::new("/projects/game"), |_| false), None);
    }

    fn project(root: &str, mod_root: &str) -> Project {
        Project::for_test(root, "game", mod_root)
    }

    #[test]
    fn derived_paths_all_hang_off_the_marker_directory() {
        let project = project("/projects/game", "levels/core");

        assert_eq!(project.root(), Path::new("/projects/game"));
        assert_eq!(project.mod_root(), Path::new("/projects/game/levels/core"));
        assert_eq!(project.baked_root(), Path::new("/projects/game/baked"));
        assert_eq!(
            project.materials_root(),
            Path::new("/projects/game/baked/materials")
        );
        assert_eq!(
            project.stage_cache_dir(),
            Path::new("/projects/game/.build-caches/prl-cache")
        );
        assert_eq!(
            project.scratch_root(),
            Path::new("/projects/game/.build-caches/dist-work")
        );
        assert_eq!(project.output_root(), Path::new("/projects/game/dist"));
    }

    /// The compiler's `Cargo.toml` walk resolves `<workspace>/baked/materials`
    /// and the engine's grandparent walk resolves `<tree>/baked/materials`. The
    /// tool's override must name that same directory, or it reintroduces the
    /// silent placeholder degradation the flag exists to close.
    #[test]
    fn baked_root_is_the_parent_of_materials_on_both_sides() {
        let project = project("/projects/game", "content/base");
        assert_eq!(
            project.baked_root().join("materials"),
            project.materials_root()
        );
    }
}
