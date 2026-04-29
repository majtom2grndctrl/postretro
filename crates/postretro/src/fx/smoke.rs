// Sprite-sheet loading and billboard layout constants for the scripted
// particle pipeline. The legacy CPU `SmokeEmitter` ring buffer was retired
// in scripting-foundation plan-3 sub-plan 8 — particle emission now flows
// through `BillboardEmitterComponent` + the entity registry. This module
// keeps the GPU-layout pins, the per-collection frame loader, and the
// `SpriteFrame` carrier so the renderer's billboard pass and the particle
// render collector still have one shared source of truth for the format.
//
// See: context/lib/rendering_pipeline.md §7.4

use std::path::Path;

// --- Constants ---

/// Maximum live sprites per emitter. Bound enforced by the emitter bridge
/// (`scripting/systems/emitter_bridge.rs`) when spawning particles.
pub const MAX_SPRITES: usize = 512;

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

/// Load all frames for a smoke collection (e.g., `smoke_00.png`, `smoke_01.png`, …)
/// from `textures/<collection>/`. Returns `None` if no frames are found; in that
/// case the renderer falls back to the checkerboard placeholder.
pub fn load_collection_frames(texture_root: &Path, collection: &str) -> Option<Vec<SpriteFrame>> {
    if collection.is_empty() {
        return None;
    }

    let collection_dir = texture_root.join(collection);
    let read_dir = match std::fs::read_dir(&collection_dir) {
        Ok(d) => d,
        Err(_) => {
            log::warn!(
                "[Smoke] Collection directory '{}' not found — no frames loaded",
                collection_dir.display()
            );
            return None;
        }
    };

    // Collect all `smoke_NN.png` paths and sort by numeric suffix.
    let mut frame_paths: Vec<(u32, std::path::PathBuf)> = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_lowercase(),
            None => continue,
        };
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !ext.eq_ignore_ascii_case("png") {
            continue;
        }
        if let Some(suffix) = stem.strip_prefix("smoke_") {
            if let Ok(n) = suffix.parse::<u32>() {
                frame_paths.push((n, path));
            }
        }
    }

    if frame_paths.is_empty() {
        log::warn!(
            "[Smoke] No smoke_NN.png frames found in '{}'",
            collection_dir.display()
        );
        return None;
    }

    frame_paths.sort_by_key(|(n, _)| *n);

    let frames: Vec<SpriteFrame> = frame_paths
        .iter()
        .filter_map(|(_, path)| match image::open(path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = rgba.dimensions();
                Some(SpriteFrame {
                    data: rgba.into_raw(),
                    width: w,
                    height: h,
                })
            }
            Err(err) => {
                log::warn!("[Smoke] Failed to load '{}': {err}", path.display());
                None
            }
        })
        .collect();

    if frames.is_empty() {
        None
    } else {
        Some(frames)
    }
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_duration_basic() {
        assert!((frame_duration(4, 2.0) - 0.5).abs() < 1e-6);
        assert!((frame_duration(0, 1.0) - 1.0).abs() < 1e-6);
    }
}
