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

/// How the caller named the project, before any of it is opened.
///
/// A project is passable as an argument — `--project <dir>` names the directory,
/// `--manifest <file>` names the marker itself. The walk up from the working
/// directory is the convenience for when you are already standing inside one,
/// not the contract. This deliberately has nothing to do with the install root
/// (`engine_trees.rs`): the two lookups are independent and neither falls back
/// to the other.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ProjectLocation {
    manifest: Option<PathBuf>,
    directory: Option<PathBuf>,
}

impl ProjectLocation {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn manifest(&self) -> Option<&Path> {
        self.manifest.as_deref()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn directory(&self) -> Option<&Path> {
        self.directory.as_deref()
    }

    /// Record a recognized project flag. Returns `false` when `flag` names
    /// neither, leaving the caller's own flag handling to run.
    pub(crate) fn absorb(
        &mut self,
        flag: &str,
        value: Option<&std::ffi::OsString>,
    ) -> Result<bool, String> {
        let slot = match flag {
            "--manifest" => &mut self.manifest,
            "--project" => &mut self.directory,
            _ => return Ok(false),
        };
        let value = value.ok_or_else(|| format!("{flag} requires a path"))?;
        if slot.replace(PathBuf::from(value)).is_some() {
            return Err(format!("{flag} may be given only once"));
        }
        Ok(true)
    }

    /// Resolve relative flag values against the directory the caller stood in.
    pub(crate) fn rebase(&mut self, invocation_dir: &Path) {
        for slot in [&mut self.manifest, &mut self.directory] {
            if let Some(path) = slot.as_ref().filter(|path| !path.is_absolute()) {
                *slot = Some(invocation_dir.join(path));
            }
        }
    }

    /// Open the named project, or walk up from `working_directory`.
    pub(crate) fn open(&self, working_directory: &Path) -> Result<Project, String> {
        match (&self.manifest, &self.directory) {
            (Some(_), Some(_)) => Err(
                "--manifest and --project name the same thing two ways; give only one".to_string(),
            ),
            (Some(manifest), None) => Project::open(manifest),
            (None, Some(directory)) => {
                let manifest = directory.join(MARKER_FILE);
                if !manifest.is_file() {
                    return Err(format!(
                        "--project {} holds no {MARKER_FILE}",
                        directory.display()
                    ));
                }
                Project::open(&manifest)
            }
            (None, None) => Project::discover(working_directory),
        }
    }
}

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
    use std::ffi::OsString;
    use std::fs;

    struct TempProject {
        root: PathBuf,
    }

    impl TempProject {
        fn new() -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time follows Unix epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "postretro-project-location-{}-{unique}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("temporary project created");
            Self { root }
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

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

    /// A project is passable as an argument; the walk up is the convenience for
    /// standing inside one, not the contract.
    #[test]
    fn a_named_project_directory_is_opened_without_any_walk() {
        let temp = TempProject::new();
        let project_dir = temp.root.join("game");
        fs::create_dir_all(&project_dir).unwrap();
        fs::write(
            project_dir.join(MARKER_FILE),
            "[package]\nname = \"g\"\nmod_root = \"content/base\"\n",
        )
        .unwrap();

        let mut location = ProjectLocation::default();
        assert_eq!(
            location.absorb("--project", Some(&OsString::from(&project_dir))),
            Ok(true)
        );
        // Opened from a directory that is not inside the project at all.
        let project = location
            .open(&temp.root)
            .expect("a named project needs no walk");
        assert_eq!(project.root(), project_dir);
    }

    #[test]
    fn a_named_project_directory_without_a_marker_says_so() {
        let temp = TempProject::new();
        let bare = temp.root.join("not-a-project");
        fs::create_dir_all(&bare).unwrap();

        let mut location = ProjectLocation::default();
        location
            .absorb("--project", Some(&OsString::from(&bare)))
            .unwrap();
        let error = location
            .open(&temp.root)
            .expect_err("a directory without the marker is not a project");
        assert!(error.contains(MARKER_FILE), "{error}");
        assert!(error.contains(&bare.display().to_string()), "{error}");
    }

    /// `--project` and `--manifest` name the same thing two ways. Accepting both
    /// would mean silently picking one.
    #[test]
    fn naming_the_project_two_ways_at_once_is_refused() {
        let mut location = ProjectLocation::default();
        location
            .absorb("--manifest", Some(&OsString::from("/a/postretro.toml")))
            .unwrap();
        location
            .absorb("--project", Some(&OsString::from("/b")))
            .unwrap();

        let error = location
            .open(Path::new("/cwd"))
            .expect_err("two names for one project is ambiguous");
        assert!(error.contains("--manifest"), "{error}");
        assert!(error.contains("--project"), "{error}");
    }

    #[test]
    fn project_flags_rebase_onto_the_directory_the_caller_stood_in() {
        let mut location = ProjectLocation::default();
        location
            .absorb("--project", Some(&OsString::from("game")))
            .unwrap();
        location.rebase(Path::new("/work"));
        assert_eq!(
            location.directory(),
            Some(Path::new("/work").join("game").as_path())
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
