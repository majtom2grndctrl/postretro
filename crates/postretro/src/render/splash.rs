// Boot splash CPU decode: `load_splash` reads the built-in (or, later, mod)
// splash PNG into RGBA8 pixels before `Renderer::new` completes. CPU-only so the
// caller can decode on the boot thread and hand decoded pixels to the renderer;
// the actual GPU upload + draw lives in the renderer-owned `splash_pass`.
// See: context/lib/boot_sequence.md §1 · context/lib/rendering_pipeline.md §7.8

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::startup::SplashSource;
use crate::ui_texture::UiTexture;

fn resolve_path(source: &SplashSource) -> PathBuf {
    match source {
        SplashSource::Base => SplashSource::base_path(),
        SplashSource::Mod(p) => p.clone(),
    }
}

/// CPU-only decode of the splash PNG into RGBA8 pixels. Returns `Err` on a
/// missing or malformed file; the boot path treats that as graceful degradation
/// (warn + stay on the black frame), never a panic — a missing base splash is a
/// packaging bug, a missing mod splash a mod-author bug.
pub(crate) fn load_splash(source: &SplashSource) -> Result<UiTexture> {
    let path = resolve_path(source);

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

    #[test]
    fn load_splash_base_decodes_committed_png() {
        // Absolute path from CARGO_MANIFEST_DIR — avoids working-directory races.
        // `CARGO_MANIFEST_DIR` is test-only (cfg(test)); runtime resolution uses
        // the cwd-relative content convention via `SplashSource::base_path`.
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let splash_path = std::path::Path::new(manifest_dir)
            .ancestors()
            .nth(2)
            .expect("crates/postretro has a workspace root two levels up")
            .join("content/base/textures/splash/postretro-ascii-art.png");

        let tex = load_splash(&SplashSource::Mod(splash_path)).expect("base splash decodes");
        assert!(tex.width > 0 && tex.height > 0, "non-zero dimensions");
        assert_eq!(
            tex.data.len(),
            (tex.width * tex.height * 4) as usize,
            "RGBA8 byte count matches dimensions",
        );
    }

    #[test]
    fn load_splash_mod_returns_error_for_missing_path() {
        // Degradation path: a missing file errors rather than panics, so the boot
        // path can warn and keep the black frame.
        let bogus = PathBuf::from("/nonexistent/path/splash.png");
        let err = load_splash(&SplashSource::Mod(bogus)).expect_err("missing file errors");
        let msg = format!("{err:#}");
        assert!(msg.contains("splash"), "error mentions splash: {msg}");
    }
}
