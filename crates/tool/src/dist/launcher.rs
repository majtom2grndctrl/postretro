//! Host-native launcher emission for an assembled distribution payload.
//!
//! The launcher's job is the working directory. Every content path the engine
//! resolves is joined against it, so the launcher pins it to its own directory
//! rather than trusting the caller's, and mounts the mod the payload published
//! under the package's app name. It passes no map argument, so the mod's
//! frontend drives the first screen.
//!
//! See: context/lib/build_pipeline.md §Distribution packaging

use std::fs;
use std::path::Path;

/// The shell a launcher is written for. Only the host's is ever emitted — a
/// payload targets the machine that produced it — but both render on every
/// host, so each one's quoting is tested wherever the suite runs.
#[derive(Debug, Clone, Copy)]
enum LauncherShell {
    Posix,
    Batch,
}

const HOST_SHELL: LauncherShell = if cfg!(windows) {
    LauncherShell::Batch
} else {
    LauncherShell::Posix
};

impl LauncherShell {
    fn extension(self) -> &'static str {
        match self {
            Self::Posix => "sh",
            Self::Batch => "bat",
        }
    }

    fn contents(self, app_name: &str, mod_name: &str) -> String {
        match self {
            Self::Posix => posix_launcher_contents(app_name, mod_name),
            Self::Batch => batch_launcher_contents(app_name, mod_name),
        }
    }
}

/// Emit the host-native launcher for a completed distribution payload.
///
/// `package_name` names the launcher file and is the `--app-name` the engine
/// keeps this game's settings and saves under. `mod_name` is the mod's name,
/// not its path: the engine's `--mod` places it under `content/` itself.
pub(crate) fn emit_launcher(
    payload_root: &Path,
    package_name: &str,
    mod_name: &str,
) -> Result<(), String> {
    let path = payload_root.join(launcher_file_name(package_name));
    fs::write(&path, HOST_SHELL.contents(package_name, mod_name))
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

/// The host launcher's file name for a package, as [`emit_launcher`] writes it.
pub(crate) fn launcher_file_name(package_name: &str) -> String {
    format!("{package_name}.{}", HOST_SHELL.extension())
}

fn batch_launcher_contents(app_name: &str, mod_name: &str) -> String {
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
         \"%~dp0postretro.exe\" --mod \"{}\" --app-name \"{}\"\r\n",
        batch_escaped(mod_name),
        batch_escaped(app_name)
    )
}

fn posix_launcher_contents(app_name: &str, mod_name: &str) -> String {
    format!(
        "#!/bin/sh\nset -eu\ncd \"$(dirname \"$0\")\"\n\
         exec ./postretro --mod '{}' --app-name '{}'\n",
        posix_single_quoted(mod_name),
        posix_single_quoted(app_name)
    )
}

fn batch_escaped(value: &str) -> String {
    // `%` expands environment variables in a batch file even inside quotes.
    // Doubling it keeps the manifest value intact when cmd executes the launcher.
    value.replace('%', "%%")
}

fn posix_single_quoted(value: &str) -> String {
    // A single quote ends the surrounding shell string. Reopen it after emitting the
    // quote itself from a double-quoted fragment: 'foo'"'"'bar'.
    value.replace('\'', "'\"'\"'")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A representative package and mod; the launcher just echoes whatever the
    /// payload published.
    const SAMPLE_PACKAGE_NAME: &str = "my-game";
    const SAMPLE_MOD_NAME: &str = "dev";

    /// Whatever the project calls its mod, the payload's launcher mounts it by
    /// name — never by a `content/` path, which the engine's `--mod` refuses —
    /// names the package as the app name the engine keeps player data under,
    /// and pins the working directory first, because a payload is correct only
    /// as a whole tree.
    #[test]
    fn launcher_pins_its_own_directory_and_mounts_the_published_mod_under_the_package() {
        let posix = LauncherShell::Posix.contents(SAMPLE_PACKAGE_NAME, SAMPLE_MOD_NAME);
        assert!(
            posix.contains("--mod 'dev' --app-name 'my-game'"),
            "{posix}"
        );
        assert!(posix.contains("dirname"), "{posix}");

        let batch = LauncherShell::Batch.contents(SAMPLE_PACKAGE_NAME, SAMPLE_MOD_NAME);
        assert!(
            batch.contains("--mod \"dev\" --app-name \"my-game\""),
            "{batch}"
        );
        assert!(batch.contains("cd /d \"%~dp0\""), "{batch}");

        for contents in [&posix, &batch] {
            assert!(!contents.contains("content/"), "{contents}");
        }
    }

    /// Regression: the launcher named the engine bare, so it resolved against
    /// PATH plus — conditionally — the current directory. Git for Windows
    /// exports `NoDefaultCurrentDirectoryInExePath`, which removes that second
    /// half, and the launcher died with "not recognized as an internal or
    /// external command" while the payload beside it was perfectly good.
    #[test]
    fn the_launcher_names_the_engine_by_path_not_by_bare_name() {
        let batch = LauncherShell::Batch.contents(SAMPLE_PACKAGE_NAME, SAMPLE_MOD_NAME);
        assert!(
            batch.contains("\"%~dp0postretro.exe\""),
            "the engine must be named relative to the launcher: {batch}"
        );
        let posix = LauncherShell::Posix.contents(SAMPLE_PACKAGE_NAME, SAMPLE_MOD_NAME);
        assert!(
            posix.contains("exec ./postretro "),
            "the engine must be named relative to the launcher: {posix}"
        );
    }

    /// Both values cross the same shell, so both take the same quoting.
    #[test]
    fn percent_signs_survive_batch_expansion_in_both_values() {
        let batch = LauncherShell::Batch.contents("50%game", "50%off");
        assert!(
            batch.contains("--mod \"50%%off\" --app-name \"50%%game\""),
            "{batch}"
        );
    }

    #[test]
    fn apostrophes_survive_the_posix_shell_in_both_values() {
        assert_eq!(posix_single_quoted("runner's-mod"), "runner'\"'\"'s-mod");
        let posix = LauncherShell::Posix.contents("runner's-game", "runner's-mod");
        assert!(
            posix.contains("--mod 'runner'\"'\"'s-mod' --app-name 'runner'\"'\"'s-game'"),
            "{posix}"
        );
    }
}
