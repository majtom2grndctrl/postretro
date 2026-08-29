// Sprite-sheet loading and billboard layout constants for particles.
// See: context/lib/rendering_pipeline.md §7.4

use std::path::Path;

use postretro_level_format::sprite_collection::{SpriteSlot, collection_frame_paths};

// --- Constants ---

/// Soft upper bound on live sprites per emitter, enforced by the emitter
/// bridge (`scripting/systems/emitter_bridge.rs`) when spawning particles.
///
/// This is **not** a render-time cap: the billboard pass sizes its instance
/// buffer to the frame's total live sprites and draws each collection from its
/// own dynamic offset, so a single collection may exceed this value without the
/// silent per-collection truncation that the old fixed 4096-sprite buffer
/// imposed. It only bounds how many particles one emitter spawns.
pub const MAX_SPRITES: usize = 4096;

/// GPU-side sprite instance layout. Two `vec4<f32>` slots = 32 bytes.
/// Layout must match the WGSL `SpriteInstance` struct in `billboard.wgsl`.
/// The struct is two `vec4<f32>` slots to satisfy WGSL storage-buffer
/// alignment (a trailing scalar next to a `vec3<f32>` member lands in the
/// same 16-byte slot).
///
/// Offsets:
///   0..12   position       (vec3<f32>, world-space)
///   12..16  age            (f32, seconds since spawn)
///   16..20  size           (f32, world units, side of the quad)
///   20..24  rotation       (f32, radians)
///   24..28  opacity        (f32, 0..1)
///   28..32  _pad           (f32, zero)
pub const SPRITE_INSTANCE_SIZE: usize = 32;

// --- Frame-duration lookup ---

/// Given a total number of animation frames and a sprite lifetime, return the
/// duration of each frame in seconds. If the collection has zero frames, falls
/// back to 1 frame of infinite duration.
///
/// Mirrored by the inline computation in `billboard.wgsl` (the GPU version
/// derives it from draw_params.params.z / frame_count). Kept here for CPU
/// diagnostics and a future entity-system hook that reports per-emitter
/// frame cadence.
#[allow(dead_code)]
pub fn frame_duration(frame_count: usize, lifetime: f32) -> f32 {
    if frame_count == 0 {
        return lifetime;
    }
    lifetime / frame_count as f32
}

// --- Sprite sheet loading (CPU side) ---

/// One loaded animation frame for a smoke collection.
#[derive(Debug, Clone)]
pub struct SpriteFrame {
    /// RGBA8 pixel data.
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Keep the frames that can share one texture-array extent.
///
/// Collection loaders call this before returning, so downstream animation
/// timing and renderer upload observe the same surviving frame count.
pub fn normalize_sprite_frames(mut frames: Vec<SpriteFrame>) -> Option<Vec<SpriteFrame>> {
    let first = frames.first()?;
    let width = first.width;
    let height = first.height;
    if width == 0 || height == 0 {
        return None;
    }

    let mut frame_index = 0usize;
    frames.retain(|frame| {
        let keep = frame.width == width && frame.height == height;
        if !keep {
            log::warn!(
                "[Smoke] Frame {frame_index} size {}x{} differs from frame 0 {}x{} — dropping",
                frame.width,
                frame.height,
                width,
                height,
            );
        }
        frame_index += 1;
        keep
    });

    (!frames.is_empty()).then_some(frames)
}

/// Load an authored sprite reference relative to the texture root. A `.png`
/// reference names one exact frame; every other reference names a sequential
/// collection directory.
pub fn load_sprite_frames(texture_root: &Path, sprite: &str) -> Option<Vec<SpriteFrame>> {
    if sprite.is_empty() {
        return None;
    }
    if Path::new(sprite)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
    {
        let path = texture_root.join(sprite);
        return load_frame(&path).map(|frame| vec![frame]);
    }
    load_collection_frames(texture_root, sprite)
}

fn load_frame(path: &Path) -> Option<SpriteFrame> {
    match image::open(path) {
        Ok(image) => {
            let rgba = image.to_rgba8();
            let (width, height) = rgba.dimensions();
            Some(SpriteFrame {
                data: rgba.into_raw(),
                width,
                height,
            })
        }
        Err(err) => {
            log::warn!("[Smoke] Failed to load '{}': {err}", path.display());
            None
        }
    }
}

/// Load all frames for a sprite collection (e.g., `smoke_00.png`, `spark_01.png`, …)
/// from `textures/<collection>/`. Returns `None` if no frames are found; startup
/// callers substitute a 1x1 white frame before renderer registration. Returned
/// frames share frame zero's dimensions; mismatches are dropped before return.
pub fn load_collection_frames(texture_root: &Path, collection: &str) -> Option<Vec<SpriteFrame>> {
    if collection.is_empty() {
        return None;
    }

    let collection_dir = texture_root.join(collection);
    let frame_paths = collection_frame_paths(texture_root, collection, SpriteSlot::Diffuse);

    if frame_paths.is_empty() {
        if collection_dir.is_dir() {
            log::warn!(
                "[Smoke] No {collection}_NN.png frames found in '{}'",
                collection_dir.display()
            );
        } else {
            log::warn!(
                "[Smoke] Collection directory '{}' not found — no frames loaded",
                collection_dir.display()
            );
        }
        return None;
    }

    let frames: Vec<SpriteFrame> = frame_paths
        .iter()
        .filter_map(|path| load_frame(path))
        .collect();

    normalize_sprite_frames(frames)
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn png_bytes(color: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(1, 1, image::Rgba(color));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn frame_duration_basic() {
        assert!((frame_duration(4, 2.0) - 0.5).abs() < 1e-6);
        assert!((frame_duration(0, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn sprite_frame_normalization_keeps_only_the_first_frame_extent() {
        // Regression: startup resolved explicit frame cadence from three decoded
        // frames while the renderer uploaded only the two matching layers.
        let frames = vec![
            SpriteFrame {
                data: vec![0; 16],
                width: 2,
                height: 2,
            },
            SpriteFrame {
                data: vec![0; 4],
                width: 1,
                height: 1,
            },
            SpriteFrame {
                data: vec![0; 16],
                width: 2,
                height: 2,
            },
        ];

        let normalized = normalize_sprite_frames(frames).expect("two frames survive");

        assert_eq!(normalized.len(), 2);
        assert!(
            normalized
                .iter()
                .all(|frame| (frame.width, frame.height) == (2, 2))
        );
    }

    #[test]
    fn collection_decode_uses_shared_tie_break_order_for_duplicate_numeric_suffixes() {
        // Regression: the fallback sorted only by numeric suffix, so equal
        // suffixes could disagree with baked array-layer order.
        let texture_root = std::env::temp_dir().join(format!(
            "postretro_smoke_shared_order_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let collection_dir = texture_root.join("smoke");
        fs::create_dir_all(&collection_dir).unwrap();
        fs::write(
            collection_dir.join("smoke_1.png"),
            png_bytes([0, 0, 255, 255]),
        )
        .unwrap();
        fs::write(
            collection_dir.join("smoke_01.png"),
            png_bytes([255, 0, 0, 255]),
        )
        .unwrap();

        let frames = load_collection_frames(&texture_root, "smoke").expect("frames decode");

        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].data, [255, 0, 0, 255]);
        assert_eq!(frames[1].data, [0, 0, 255, 255]);
        fs::remove_dir_all(texture_root).unwrap();
    }

    #[test]
    fn reference_projectile_body_and_trail_paths_decode_real_frames() {
        // Regression: documented texture-relative PNG paths were interpreted as
        // collection directories and silently registered the white fallback.
        let texture_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/dev/textures");
        for sprite in [
            "projectiles/plasma_blue_orb.png",
            "smoke_puff/smoke_puff_00.png",
        ] {
            let frames = load_sprite_frames(&texture_root, sprite)
                .unwrap_or_else(|| panic!("reference projectile sprite `{sprite}` must decode"));
            assert_eq!(frames.len(), 1);
            let frame = &frames[0];
            assert!(frame.width > 1 || frame.height > 1);
            assert_eq!(frame.data.len(), (frame.width * frame.height * 4) as usize);
            assert_ne!(frame.data.as_slice(), &[255, 255, 255, 255]);
        }
    }
}
