// Boot splash assets: CPU-side PNG decode (`load_splash`), the GPU texture upload
// (`upload_splash_texture`), and the background fill color (`SPLASH_BG_COLOR`).
// `load_splash` is CPU-only so the caller can decode before `Renderer::new`
// completes. The actual splash *drawing* lives in the UI pass (`render/ui/`); the
// old `SplashPipeline` quad pipeline was retired into it.
// See: context/lib/rendering_pipeline.md · context/lib/boot_sequence.md §8

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

/// CPU-only decode; no fallback placeholder. Missing base splash = packaging bug;
/// missing mod splash = mod-author bug. Caller surfaces the error.
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

// sRGB decode-on-sample pairs with sRGB encode-on-write — no manual gamma in the shader.
const SPLASH_TEXTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Free function so the caller controls upload timing independently of pipeline
/// lifetime. All wgpu calls live here per the renderer-owns-GPU rule.
pub(crate) fn upload_splash_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    loaded: &UiTexture,
) -> (wgpu::Texture, [u32; 2]) {
    let size = wgpu::Extent3d {
        width: loaded.width,
        height: loaded.height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Splash Texture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SPLASH_TEXTURE_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &loaded.data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * loaded.width),
            rows_per_image: Some(loaded.height),
        },
        size,
    );
    (texture, [loaded.width, loaded.height])
}

// Linear-space sRGB(21, 27, 35). The fullscreen background fill behind the framed
// splash panel; the UI pass draws it as the first quad in the splash draw list.
pub(crate) const SPLASH_BG_COLOR: wgpu::Color = wgpu::Color {
    r: 0.00750,
    g: 0.01093,
    b: 0.01672,
    a: 1.0,
};

/// `SPLASH_BG_COLOR` as a linear-RGBA array for the UI background-fill quad.
pub(crate) fn splash_bg_rgba() -> [f32; 4] {
    [
        SPLASH_BG_COLOR.r as f32,
        SPLASH_BG_COLOR.g as f32,
        SPLASH_BG_COLOR.b as f32,
        SPLASH_BG_COLOR.a as f32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_splash_base_decodes_committed_png() {
        // Absolute path from CARGO_MANIFEST_DIR — avoids working-directory races.
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
        let bogus = PathBuf::from("/nonexistent/path/splash.png");
        let err = load_splash(&SplashSource::Mod(bogus)).expect_err("missing file errors");
        let msg = format!("{err:#}");
        assert!(msg.contains("splash"), "error mentions splash: {msg}");
    }
}
