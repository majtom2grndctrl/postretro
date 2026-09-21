//! Host-native launcher emission for an assembled distribution payload.
//!
//! The launcher's job is the working directory. Every content path the engine
//! resolves is joined against it, so the launcher pins it to its own directory
//! rather than trusting the caller's, and mounts the mod the payload published.
//! It passes no map argument, so the mod's frontend drives the first screen.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging

use std::fs;
use std::path::Path;

/// Emit the host-native launcher for a completed distribution payload.
pub(crate) fn emit_launcher(
    payload_root: &Path,
    package_name: &str,
    mod_root: &str,
) -> Result<(), String> {
    let path = payload_root.join(format!("{package_name}.{}", launcher_extension()));
    fs::write(&path, launcher_contents(mod_root))
        .map_err(|error| format!("stage 5: write launcher {}: {error}", path.display()))?;

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path)
            .map_err(|error| {
                format!(
                    "stage 5: read launcher permissions {}: {error}",
                    path.display()
                )
            })?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).map_err(|error| {
            format!(
                "stage 5: mark launcher executable {}: {error}",
                path.display()
            )
        })?;
    }

    Ok(())
}

fn launcher_extension() -> &'static str {
    if cfg!(windows) { "bat" } else { "sh" }
}

#[cfg(windows)]
fn launcher_contents(mod_root: &str) -> String {
    // `%` expands environment variables in a batch file even inside quotes.
    // Doubling it keeps the manifest value intact when cmd executes the launcher.
    let batch_mod_root = mod_root.replace('%', "%%");
    // The engine is named by `%~dp0`, the launcher's own directory, rather than
    // bare: a bare name is resolved against PATH and — only sometimes — the
    // current directory. Git for Windows exports
    // `NoDefaultCurrentDirectoryInExePath`, which switches that second half off,
    // so a bare name makes the launcher fail with "not recognized as an internal
    // or external command" for anyone double-clicking it from a Git Bash shell
    // while working from Explorer or plain cmd. The `cd /d` still matters
    // independently: the working directory is what every content path resolves
    // against.
    format!(
        "@echo off\r\nsetlocal DisableDelayedExpansion\r\ncd /d \"%~dp0\"\r\n\
         \"%~dp0postretro.exe\" --mod \"{batch_mod_root}\"\r\n"
    )
}

#[cfg(not(windows))]
fn launcher_contents(mod_root: &str) -> String {
    format!(
        "#!/bin/sh\nset -eu\ncd \"$(dirname \"$0\")\"\nexec ./postretro --mod '{}'\n",
        posix_single_quoted(mod_root)
    )
}

#[cfg(not(windows))]
fn posix_single_quoted(value: &str) -> String {
    // A single quote ends the surrounding shell string. Reopen it after emitting the
    // quote itself from a double-quoted fragment: 'content/foo'"'"'bar'.
    value.replace('\'', "'\"'\"'")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative two-component mod root; the launcher just echoes
    /// whatever name the payload published under.
    const SAMPLE_MOD_ROOT: &str = "content/dev";

    /// Whatever the project calls its own mod root, the payload's launcher
    /// mounts the one the payload published — and pins the working directory
    /// first, because a payload is correct only as a whole tree.
    #[test]
    fn launcher_pins_its_own_directory_and_mounts_the_published_mod_root() {
        let contents = launcher_contents(SAMPLE_MOD_ROOT);
        assert!(contents.contains(SAMPLE_MOD_ROOT), "{contents}");
        assert!(contents.contains("--mod"), "{contents}");
        assert!(
            contents.contains("%~dp0") || contents.contains("dirname"),
            "the launcher must pin its own directory: {contents}"
        );
    }

    /// Regression: the launcher named the engine bare, so it resolved against
    /// PATH plus — conditionally — the current directory. Git for Windows
    /// exports `NoDefaultCurrentDirectoryInExePath`, which removes that second
    /// half, and the launcher died with "not recognized as an internal or
    /// external command" while the payload beside it was perfectly good.
    #[test]
    fn the_launcher_names_the_engine_by_path_not_by_bare_name() {
        let contents = launcher_contents(SAMPLE_MOD_ROOT);
        #[cfg(windows)]
        assert!(
            contents.contains("\"%~dp0postretro.exe\""),
            "the engine must be named relative to the launcher: {contents}"
        );
        #[cfg(not(windows))]
        assert!(
            contents.contains("./postretro"),
            "the engine must be named relative to the launcher: {contents}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn percent_signs_survive_batch_expansion() {
        assert!(launcher_contents("content/50%off").contains("content/50%%off"));
    }

    #[cfg(not(windows))]
    #[test]
    fn quotes_apostrophes_for_posix_shell() {
        assert_eq!(
            posix_single_quoted("content/runner's-mod"),
            "content/runner'\"'\"'s-mod"
        );
    }
}
