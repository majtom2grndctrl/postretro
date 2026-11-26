//! What a finished player payload must hold, checked over a directory tree.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging (§Payload layout,
//! §Completion gate)

use std::fs;
use std::path::Path;

use crate::dist::binary_name;

/// The completion marker a payload carries while a run is still working in it.
pub(super) const MARKER_NAME: &str = ".dist-incomplete";

/// What the project that produced the payload declared.
pub(super) struct ExpectedPayload<'a> {
    /// `[package].name`: names the launcher.
    pub(super) package: &'a str,
    /// `[package].mod`: the payload publishes the mod at `content/<mod>`.
    pub(super) mod_name: &'a str,
    /// Level stems the catalog ships, each baked to `maps/<stem>.prl`.
    pub(super) levels: &'a [&'a str],
}

/// Every way `payload_root` falls short of a completed payload, or nothing.
///
/// Reports all problems at once rather than the first, so one slow run
/// diagnoses everything it can.
pub(super) fn verify_payload(
    payload_root: &Path,
    expected: &ExpectedPayload<'_>,
) -> Result<(), Vec<String>> {
    if !payload_root.is_dir() {
        return Err(vec![format!(
            "payload root {} is not a directory",
            payload_root.display()
        )]);
    }

    let mut problems = Vec::new();

    if payload_root.join(MARKER_NAME).exists() {
        problems.push(format!(
            "{MARKER_NAME} is still present: the run did not complete"
        ));
    }

    require_executable(
        &payload_root.join(binary_name("postretro")),
        "engine binary",
        &mut problems,
    );
    require_executable(
        &payload_root.join(host_launcher_name(expected.package)),
        "launcher",
        &mut problems,
    );

    let core = payload_root.join("core");
    if !is_non_empty_dir(&core) {
        problems.push(format!(
            "engine-owned tree {} is missing or empty",
            core.display()
        ));
    }

    let maps = payload_root
        .join("content")
        .join(expected.mod_name)
        .join("maps");
    for level in expected.levels {
        let prl = maps.join(format!("{level}.prl"));
        if !fs::metadata(&prl).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0) {
            problems.push(format!("baked level {} is missing or empty", prl.display()));
        }
    }

    let materials = payload_root.join("baked").join("materials");
    if !holds_prm(&materials) {
        problems.push(format!(
            "materials tree {} holds no .prm sidecar",
            materials.display()
        ));
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// The host's launcher name: `.bat` on Windows, `.sh` elsewhere.
pub(super) fn host_launcher_name(package: &str) -> String {
    if cfg!(windows) {
        format!("{package}.bat")
    } else {
        format!("{package}.sh")
    }
}

/// A regular file, and on Unix one with an execute bit: a launcher or engine
/// that cannot be run is as missing to a recipient as an absent one.
fn require_executable(path: &Path, what: &str, problems: &mut Vec<String>) {
    let Ok(metadata) = fs::metadata(path) else {
        problems.push(format!("{what} {} is missing", path.display()));
        return;
    };
    if !metadata.is_file() {
        problems.push(format!("{what} {} is not a file", path.display()));
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            problems.push(format!("{what} {} is not executable", path.display()));
        }
    }
}

fn is_non_empty_dir(path: &Path) -> bool {
    fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_some())
}

fn holds_prm(path: &Path) -> bool {
    fs::read_dir(path).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_file())
                && entry.path().extension().is_some_and(|ext| ext == "prm")
        })
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const LEVELS: [&str; 1] = ["room"];
    const EXPECTED: ExpectedPayload<'static> = ExpectedPayload {
        package: "my-game",
        mod_name: "base",
        levels: &LEVELS,
    };

    /// A unique directory under the system temp dir, removed on drop.
    struct FakeTree(PathBuf);

    impl FakeTree {
        /// A tree holding everything a completed payload must.
        fn complete() -> Self {
            static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "xtask-sdk-check-verify-{}-{sequence}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            let tree = Self(root);
            tree.executable(&binary_name("postretro"));
            tree.executable(&host_launcher_name("my-game"));
            tree.file("core/ui/menu.json", b"{}");
            tree.file("content/base/maps/room.prl", b"PRL");
            tree.file("baked/materials/0123.prm", b"PRM");
            tree
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.0.join(relative)
        }

        fn file(&self, relative: &str, contents: &[u8]) {
            let path = self.path(relative);
            fs::create_dir_all(path.parent().expect("a parent")).expect("parent created");
            fs::write(&path, contents).expect("file written");
        }

        fn executable(&self, relative: &str) {
            self.file(relative, b"#!/bin/sh\n");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(self.path(relative), fs::Permissions::from_mode(0o755))
                    .expect("mode set");
            }
        }

        fn remove(&self, relative: &str) {
            let path = self.path(relative);
            if path.is_dir() {
                fs::remove_dir_all(&path).expect("directory removed");
            } else {
                fs::remove_file(&path).expect("file removed");
            }
        }

        fn problems(&self) -> Vec<String> {
            verify_payload(&self.0, &EXPECTED).err().unwrap_or_default()
        }
    }

    impl Drop for FakeTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Exactly one problem, naming `needle`.
    fn assert_one_problem(tree: &FakeTree, needle: &str) {
        let problems = tree.problems();
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains(needle), "{problems:?}");
    }

    #[test]
    fn a_complete_payload_passes() {
        let tree = FakeTree::complete();
        assert_eq!(verify_payload(&tree.0, &EXPECTED), Ok(()));
    }

    #[test]
    fn a_missing_payload_root_is_one_problem() {
        let tree = FakeTree::complete();
        let missing = tree.path("not-here");
        let problems = verify_payload(&missing, &EXPECTED).expect_err("absent root fails");
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("not a directory"), "{problems:?}");
    }

    #[test]
    fn a_completion_marker_means_the_run_did_not_finish() {
        let tree = FakeTree::complete();
        tree.file(MARKER_NAME, b"stage 6\nmaps/room.prl\n");
        assert_one_problem(&tree, MARKER_NAME);
    }

    #[test]
    fn a_missing_engine_binary_fails() {
        let tree = FakeTree::complete();
        tree.remove(&binary_name("postretro"));
        assert_one_problem(&tree, "engine binary");
    }

    #[test]
    fn a_missing_launcher_fails() {
        let tree = FakeTree::complete();
        tree.remove(&host_launcher_name("my-game"));
        assert_one_problem(&tree, "launcher");
    }

    #[cfg(unix)]
    #[test]
    fn a_launcher_without_an_execute_bit_fails() {
        let tree = FakeTree::complete();
        tree.file(&host_launcher_name("my-game"), b"#!/bin/sh\n");
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            tree.path(&host_launcher_name("my-game")),
            fs::Permissions::from_mode(0o644),
        )
        .expect("mode set");
        assert_one_problem(&tree, "not executable");
    }

    #[test]
    fn an_empty_core_tree_fails() {
        let tree = FakeTree::complete();
        tree.remove("core/ui");
        assert_one_problem(&tree, "core");
    }

    #[test]
    fn a_missing_core_tree_fails() {
        let tree = FakeTree::complete();
        tree.remove("core");
        assert_one_problem(&tree, "core");
    }

    #[test]
    fn a_missing_level_fails() {
        let tree = FakeTree::complete();
        tree.remove("content/base/maps/room.prl");
        assert_one_problem(&tree, "room.prl");
    }

    #[test]
    fn an_empty_level_fails() {
        let tree = FakeTree::complete();
        tree.file("content/base/maps/room.prl", b"");
        assert_one_problem(&tree, "room.prl");
    }

    /// The payload publishes the declared mod's name; a level under another
    /// mod's directory is not the one the launcher mounts.
    #[test]
    fn a_level_under_the_wrong_mod_fails() {
        let tree = FakeTree::complete();
        tree.remove("content/base");
        tree.file("content/dev/maps/room.prl", b"PRL");
        assert_one_problem(&tree, "room.prl");
    }

    #[test]
    fn a_materials_tree_without_a_sidecar_fails() {
        let tree = FakeTree::complete();
        tree.remove("baked/materials/0123.prm");
        tree.file("baked/materials/notes.txt", b"not a sidecar");
        assert_one_problem(&tree, "materials");
    }

    #[test]
    fn every_problem_is_reported_together() {
        let tree = FakeTree::complete();
        tree.file(MARKER_NAME, b"stage 5\n");
        tree.remove(&binary_name("postretro"));
        tree.remove("baked");
        assert_eq!(tree.problems().len(), 3, "{:?}", tree.problems());
    }
}
