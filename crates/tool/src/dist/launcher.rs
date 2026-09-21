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
    format!(
        "@echo off\r\nsetlocal DisableDelayedExpansion\r\ncd /d \"%~dp0\"\r\n\
         postretro.exe --mod \"{batch_mod_root}\"\r\n"
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
    use crate::dist::payload::PAYLOAD_MOD_ROOT;

    /// Whatever the project calls its own mod root, the payload's launcher
    /// mounts the one the payload published — and pins the working directory
    /// first, because a payload is correct only as a whole tree.
    #[test]
    fn launcher_pins_its_own_directory_and_mounts_the_published_mod_root() {
        let contents = launcher_contents(PAYLOAD_MOD_ROOT);
        assert!(contents.contains(PAYLOAD_MOD_ROOT), "{contents}");
        assert!(contents.contains("--mod"), "{contents}");
        assert!(
            contents.contains("%~dp0") || contents.contains("dirname"),
            "the launcher must pin its own directory: {contents}"
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
