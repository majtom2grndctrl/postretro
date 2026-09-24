// Boot splash CPU decode: `load_splash` reads the built-in (or, later, mod)
// splash PNG into RGBA8 pixels before `Renderer::new` completes. CPU-only so the
// caller can decode on the boot thread and hand decoded pixels to the renderer;
// the actual GPU upload + draw lives in the renderer-owned `splash_pass`.
// See: context/lib/boot_sequence.md §1 · context/lib/rendering_pipeline.md §7.8

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::startup::SplashSource;
use postretro_ui::{CoreRoot, UiTexture};

fn resolve_path(source: &SplashSource, core_root: &CoreRoot) -> PathBuf {
    match source {
        SplashSource::Base => SplashSource::base_path(core_root),
        SplashSource::Mod(p) => p.clone(),
    }
}

/// CPU-only decode of the splash PNG into RGBA8 pixels. Returns `Err` on a
/// missing or malformed file; the boot path treats that as graceful degradation
/// (warn + stay on the black frame), never a panic — a missing base splash is a
/// packaging bug, a missing mod splash a mod-author bug.
pub(crate) fn load_splash(source: &SplashSource, core_root: &CoreRoot) -> Result<UiTexture> {
    let path = resolve_path(source, core_root);

    let img = image::open(&path)
        .with_context(|| format!("decoding splash PNG at {}", path.display()))?
        .to_rgba8();
    let (width, height) = img.dimensions();

    Ok(UiTexture {
        data: img.into_raw(),
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The workspace's own `core/`, by absolute path — `cargo test` runs from the
    /// crate directory, and a relative root would race the working directory.
    /// `CARGO_MANIFEST_DIR` is test-only (`cfg(test)`); runtime resolution takes
    /// the root the engine parsed from argv.
    fn workspace_core_root() -> CoreRoot {
        CoreRoot::at(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .ancestors()
                .nth(2)
                .expect("crates/postretro has a workspace root two levels up")
                .join("core"),
        )
    }

    #[test]
    fn load_splash_base_decodes_committed_png() {
        let tex =
            load_splash(&SplashSource::Base, &workspace_core_root()).expect("base splash decodes");
        assert!(tex.width > 0 && tex.height > 0, "non-zero dimensions");
        assert_eq!(
            tex.data.len(),
            (tex.width * tex.height * 4) as usize,
            "RGBA8 byte count matches dimensions",
        );
    }

    /// Flag absent resolves the path every launch resolved before `--core-root`
    /// existed; flag present moves the same asset under the named root. The mod
    /// override is an absolute path and ignores the root entirely.
    #[test]
    fn core_root_selects_where_the_base_splash_is_read_from() {
        assert_eq!(
            resolve_path(&SplashSource::Base, &CoreRoot::from_flag(None)),
            PathBuf::from("core/textures/splash/postretro-ascii-art.png"),
        );
        assert_eq!(
            resolve_path(
                &SplashSource::Base,
                &CoreRoot::from_flag(Some(PathBuf::from("/install/core"))),
            ),
            PathBuf::from("/install/core/textures/splash/postretro-ascii-art.png"),
        );

        let authored = PathBuf::from("/mod/splash.png");
        assert_eq!(
            resolve_path(
                &SplashSource::Mod(authored.clone()),
                &CoreRoot::from_flag(Some(PathBuf::from("/install/core"))),
            ),
            authored,
            "a mod override is absolute and never joins the core root",
        );
    }

    #[test]
    fn load_splash_mod_returns_error_for_missing_path() {
        // Degradation path: a missing file errors rather than panics, so the boot
        // path can warn and keep the black frame.
        let bogus = PathBuf::from("/nonexistent/path/splash.png");
        let err = load_splash(&SplashSource::Mod(bogus), &CoreRoot::working_directory())
            .expect_err("missing file errors");
        let msg = format!("{err:#}");
        assert!(msg.contains("splash"), "error mentions splash: {msg}");
    }
}
