//! Host-native launcher emission for an assembled distribution payload.
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
    #[cfg(windows)]
    {
        let path = payload_root.join(format!("{package_name}.bat"));
        // `%` expands environment variables in a batch file even inside quotes.
        // Doubling it keeps the manifest value intact when cmd executes the launcher.
        let batch_mod_root = mod_root.replace('%', "%%");
        let contents = format!(
            "@echo off\r\nsetlocal DisableDelayedExpansion\r\ncd /d \"%~dp0\"\r\npostretro.exe --mod \"{batch_mod_root}\"\r\n"
        );
        fs::write(&path, contents)
            .map_err(|error| format!("stage 5: write launcher {}: {error}", path.display()))?;
    }

    #[cfg(not(windows))]
    {
        let path = payload_root.join(format!("{package_name}.sh"));
        let contents = format!(
            "#!/bin/sh\nset -eu\ncd \"$(dirname \"$0\")\"\nexec ./postretro --mod '{}'\n",
            mod_root.replace('\'', "'\\\"'\\\"'")
        );
        fs::write(&path, contents)
            .map_err(|error| format!("stage 5: write launcher {}: {error}", path.display()))?;

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
