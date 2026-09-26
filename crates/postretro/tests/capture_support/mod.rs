// Shared plumbing for the GPU capture integration tests: the workspace root,
// dev maps compiled beside their source, and the capture child process.
// See: context/lib/rendering_pipeline.md §7.8

use std::path::{Path, PathBuf};
use std::process::Command;

use image::RgbaImage;

pub(super) fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("postretro crate must be two levels below the workspace root")
        .to_path_buf()
}

/// Compile `content/dev/maps/<map_name>.map` with `prl-build` into a temporary
/// PRL beside its source. The path deletes itself on drop, even when an
/// assertion fails.
///
/// Keep the compiled PRL directly under content/dev/maps. Capture derives
/// content/dev, then <workspace>/baked/materials, from this standard layout.
/// Compiling into the generic temp directory makes that derivation point at
/// the wrong tree and silently replaces materials with placeholders — a
/// specular slot, for one, turns black.
pub(super) fn compile_dev_map(workspace: &Path, map_name: &str) -> tempfile::TempPath {
    let source_map = workspace.join(format!("content/dev/maps/{map_name}.map"));
    assert!(
        source_map.is_file(),
        "capture source map missing: {}",
        source_map.display()
    );
    let map = tempfile::Builder::new()
        .prefix(&format!(".{map_name}-"))
        .suffix(".prl")
        .tempfile_in(workspace.join("content/dev/maps"))
        .expect("reserve capture PRL path in content/dev/maps")
        .into_temp_path();
    let compile = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "run",
            "--quiet",
            "-p",
            "postretro-level-compiler",
            "--bin",
            "prl-build",
            "--",
        ])
        .arg(&source_map)
        .arg("-o")
        .arg(&map)
        .arg("--no-tui")
        .current_dir(workspace)
        .output()
        .expect("launch prl-build");
    assert!(
        compile.status.success(),
        "`{map_name}` capture map compile failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr),
    );
    map
}

/// The capture child for `scene_path`, run from the workspace root.
///
/// Static capture accepts streamed SH (ids 49/50, which `prl-build` emits for
/// dev maps) only in the deterministic `sync-proof` mode. A PRL without id 50
/// ignores the variable, so every capture sets it rather than relying on the
/// caller's environment.
pub(super) fn capture_command(workspace: &Path, scene_path: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_postretro"));
    command
        .arg("--capture")
        .arg(scene_path)
        .env("POSTRETRO_SH_STREAMING", "sync-proof")
        .current_dir(workspace);
    command
}

pub(super) fn load_capture_rgba(path: &Path) -> RgbaImage {
    image::ImageReader::open(path)
        .unwrap_or_else(|err| panic!("open capture PNG {}: {err}", path.display()))
        .with_guessed_format()
        .expect("detect capture image format")
        .decode()
        .expect("decode capture PNG")
        .to_rgba8()
}
