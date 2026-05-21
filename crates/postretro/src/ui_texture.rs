// CPU-side UI texture data: PNG-decoded RGBA8 used by splash and 2D blits.
// World-material textures live in `render::loaded_texture` (wgpu handles).
// See: context/lib/resource_management.md

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// CPU-side decoded RGBA8 texture for UI surfaces (splash, HUD, 2D blits).
/// World materials use `render::loaded_texture::LoadedTexture` instead.
#[derive(Debug, Clone)]
pub struct UiTexture {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Build a map from lowercase texture name stem to file path.
/// Layout: `<texture_root>/<collection>/<name>.png`.
///
/// Task 4 removes this runtime copy once the compiler owns the equivalent
/// path index. Retained here so the engine still resolves sprite collections
/// and ad-hoc UI lookups by name in the interim.
#[allow(dead_code)]
pub fn build_name_to_path_map(texture_root: &Path) -> HashMap<String, PathBuf> {
    let mut map: HashMap<String, PathBuf> = HashMap::new();

    let collections = match std::fs::read_dir(texture_root) {
        Ok(entries) => entries,
        Err(err) => {
            log::warn!(
                "[Texture] Cannot read texture root {}: {err}",
                texture_root.display()
            );
            return map;
        }
    };

    for entry in collections.flatten() {
        let collection_path = entry.path();
        if !collection_path.is_dir() {
            continue;
        }

        let files = match std::fs::read_dir(&collection_path) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for file_entry in files.flatten() {
            let file_path = file_entry.path();

            let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !ext.eq_ignore_ascii_case("png") {
                continue;
            }

            let stem = match file_path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_lowercase(),
                None => continue,
            };

            if let Some(existing) = map.get(&stem) {
                log::warn!(
                    "[Texture] Duplicate texture name '{stem}': found in {} and {}. Using first found.",
                    existing.display(),
                    file_path.display(),
                );
            } else {
                map.insert(stem, file_path);
            }
        }
    }

    map
}
