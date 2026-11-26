//! `sdk-check`: the shipped tool's full acceptance, scripted.
//!
//! Every workspace test has a `Cargo.toml` ancestor and a cargo binary — the
//! two things an SDK bundle's recipient lacks. This command builds the bundle
//! exactly as `sdk-dist` does, copies it to a temp directory with no repository
//! above it, makes an external game project there by following the bundle's own
//! docs, and runs the bundle's `bin/postretro-tool dist` in it. A pass proves a
//! game developer holding only a bundle can ship a level.
//!
//! It runs no engine. The bundle's engines carry no headless or capture mode,
//! and the one bounded windowed run (camera-route playback) waits behind the
//! first-launch accessibility panel of an app name that has never launched.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging (§SDK bundle)

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::dist::{self, binary_name};
use crate::{run_checked, workspace_root};

mod project;
mod verify;

use verify::{ExpectedPayload, verify_payload};

/// The SDK bundle's fixed name under the workspace's `dist/`: the tool's
/// `SDK_BUNDLE_NAME`, which a test below pins by source text.
const SDK_BUNDLE_NAME: &str = "postretro-sdk";

pub(crate) fn run(args: Vec<OsString>) -> Result<i32, String> {
    if let Some(extra) = args.first() {
        return Err(format!(
            "sdk-check takes no arguments, got `{}`\n\nUsage: cargo run -p xtask -- sdk-check",
            extra.to_string_lossy()
        ));
    }
    let started = Instant::now();

    println!("sdk-check 1/6: build the SDK bundle (sdk-dist)");
    let code = dist::sdk_dist(Vec::new())?;
    if code != 0 {
        return Err(format!("sdk-check: sdk-dist exited with {code}"));
    }
    let bundle = workspace_root()?.join("dist").join(SDK_BUNDLE_NAME);

    let scratch = scratch_root()?;
    match check_outside_repository(&bundle, &scratch) {
        Ok(()) => {
            fs::remove_dir_all(&scratch)
                .map_err(|error| format!("sdk-check: remove {}: {error}", scratch.display()))?;
            println!(
                "sdk-check passed in {:.0?}; removed {}",
                started.elapsed(),
                scratch.display()
            );
            Ok(0)
        }
        Err(error) => {
            eprintln!("sdk-check: kept {} for inspection", scratch.display());
            Err(error)
        }
    }
}

/// Steps 2 through 6, all inside `scratch`, which the caller keeps on failure.
fn check_outside_repository(bundle: &Path, scratch: &Path) -> Result<(), String> {
    println!("sdk-check 2/6: copy the bundle to {}", scratch.display());
    let installed = scratch.join(SDK_BUNDLE_NAME);
    copy_tree(bundle, &installed)?;

    println!("sdk-check 3/6: create an external project from the bundle's starter");
    let project_root = scratch.join(project::PACKAGE);
    project::create(&installed, &project_root)?;

    println!(
        "sdk-check 4/6: boot check skipped — the bundle's engines have no headless mode, \
         and camera-route playback waits behind a fresh app name's first-launch panel"
    );

    println!("sdk-check 5/6: bin/postretro-tool dist in the project");
    let tool = installed.join("bin").join(binary_name("postretro-tool"));
    let mut command = Command::new(&tool);
    command.current_dir(&project_root).arg("dist");
    // A recipient's shell has no cargo in it. Nothing the tool reads should
    // come from this process's cargo environment, so none of it is inherited.
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(|key| key.starts_with("CARGO")) {
            command.env_remove(key);
        }
    }
    run_checked(&mut command, "sdk-check: bundle postretro-tool dist")?;

    println!("sdk-check 6/6: verify the payload");
    let payload = project_root.join("dist").join(project::PACKAGE);
    verify_payload(
        &payload,
        &ExpectedPayload {
            package: project::PACKAGE,
            mod_name: project::MOD,
            levels: &[project::LEVEL],
        },
    )
    .map_err(|problems| {
        format!(
            "sdk-check: payload {} is incomplete:\n  {}",
            payload.display(),
            problems.join("\n  ")
        )
    })
}

/// A fresh directory under the system temp dir, proven to sit under no
/// repository or project: a `Cargo.toml` or `postretro.toml` above it would
/// let the check pass on something a recipient does not have.
fn scratch_root() -> Result<PathBuf, String> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("sdk-check: read the clock: {error}"))?
        .as_nanos();
    let temp = std::env::temp_dir();
    let temp = fs::canonicalize(&temp)
        .map_err(|error| format!("sdk-check: resolve {}: {error}", temp.display()))?;
    if let Some(marker) = enclosing_marker(&temp) {
        return Err(format!(
            "sdk-check: the temp directory sits under {}; point TMPDIR outside any \
             repository or project",
            marker.display()
        ));
    }
    let scratch = temp.join(format!(
        "postretro-sdk-check-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir(&scratch)
        .map_err(|error| format!("sdk-check: create {}: {error}", scratch.display()))?;
    Ok(scratch)
}

/// The nearest `Cargo.toml` or `postretro.toml` at or above `path`.
fn enclosing_marker(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .flat_map(|ancestor| ["Cargo.toml", "postretro.toml"].map(|name| ancestor.join(name)))
        .find(|marker| marker.exists())
}

/// Copy a directory tree, keeping each file's permissions so the bundle's
/// binaries stay executable. A bundle holds no symlinks; one is refused
/// rather than followed out of the tree.
fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination)
        .map_err(|error| format!("create {}: {error}", destination.display()))?;
    let entries =
        fs::read_dir(source).map_err(|error| format!("read {}: {error}", source.display()))?;
    for entry in entries {
        let entry = entry.map_err(|error| format!("read {}: {error}", source.display()))?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let kind = entry
            .file_type()
            .map_err(|error| format!("inspect {}: {error}", from.display()))?;
        if kind.is_dir() {
            copy_tree(&from, &to)?;
        } else if kind.is_file() {
            fs::copy(&from, &to)
                .map_err(|error| format!("copy {} to {}: {error}", from.display(), to.display()))?;
        } else {
            return Err(format!("refuse to copy non-file {}", from.display()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names this crate shares with the tool, pinned by the tool's source
    /// text: a rename there must reach here.
    #[test]
    fn shared_names_match_the_tools_declarations() {
        let root = workspace_root().expect("workspace root");
        let read = |path: &str| fs::read_to_string(root.join(path)).expect(path);
        assert!(read("crates/tool/src/sdk_dist/mod.rs").contains(&format!(
            "pub(crate) const SDK_BUNDLE_NAME: &str = \"{SDK_BUNDLE_NAME}\";"
        )));
        assert!(read("crates/tool/src/dist/payload.rs").contains(&format!(
            "pub(crate) const MARKER_NAME: &str = \"{}\";",
            verify::MARKER_NAME
        )));
    }

    #[test]
    fn the_workspace_is_inside_its_own_repository_marker() {
        let root = workspace_root().expect("workspace root");
        assert_eq!(
            enclosing_marker(&root.join("crates/xtask")),
            Some(root.join("crates/xtask/Cargo.toml"))
        );
    }

    #[test]
    fn a_tree_with_no_marker_above_it_has_none() {
        assert_eq!(enclosing_marker(Path::new("/")), None);
    }
}
