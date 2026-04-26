// CPU-side texture loading: PNG files matched by BSP texture names.
// See: context/lib/resource_management.md

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A loaded texture's CPU-side data, ready for GPU upload by the renderer.
#[derive(Debug, Clone)]
pub struct LoadedTexture {
    /// RGBA8 pixel data.
    pub data: Vec<u8>,
    /// Texture width in pixels.
    pub width: u32,
    /// Texture height in pixels.
    pub height: u32,
    /// True if this is a checkerboard placeholder (missing or corrupt source).
    #[allow(dead_code)]
    pub is_placeholder: bool,
}

/// Result of loading all textures referenced by a BSP level.
/// Indexed by BSP miptexture array index for direct lookup.
#[derive(Debug)]
pub struct TextureSet {
    /// One entry per BSP miptexture index. The Vec index matches the BSP texture index.
    pub textures: Vec<LoadedTexture>,
    /// Optional per-texel specular intensity, sibling-loaded as `{name}_s.png`.
    /// R8-equivalent data unpacked into the R channel of RGBA8 (G/B/A unused
    /// by the shader). `None` when no `_s` sibling was present or when the
    /// sibling's dimensions did not match the diffuse. Same indexing as
    /// `textures`. See `context/lib/resource_management.md` §4.1.
    pub specular: Vec<Option<LoadedTexture>>,
    /// Optional tangent-space normal map, sibling-loaded as `{name}_n.png`.
    /// Stored as RGBA8 and uploaded as `Rgba8Unorm` (linear, NOT sRGB). `None`
    /// when no `_n` sibling was present, the sibling's dimensions did not
    /// match the diffuse, or the sibling failed to decode. Same indexing as
    /// `textures`. See `context/lib/resource_management.md` §4.3.
    pub normal: Vec<Option<LoadedTexture>>,
}

// --- Checkerboard placeholder ---

const PLACEHOLDER_SIZE: u32 = 64;
const CHECKER_SQUARE: u32 = 8;
const MAGENTA: [u8; 4] = [255, 0, 255, 255];
const BLACK: [u8; 4] = [0, 0, 0, 255];

/// Generate a 64x64 checkerboard placeholder for the renderer when no textures are available.
pub fn generate_placeholder() -> LoadedTexture {
    generate_checkerboard()
}

/// Generate a 64x64 checkerboard placeholder (magenta/black, 8x8 squares).
fn generate_checkerboard() -> LoadedTexture {
    let pixel_count = (PLACEHOLDER_SIZE * PLACEHOLDER_SIZE) as usize;
    let mut data = Vec::with_capacity(pixel_count * 4);

    for y in 0..PLACEHOLDER_SIZE {
        for x in 0..PLACEHOLDER_SIZE {
            let checker_x = x / CHECKER_SQUARE;
            let checker_y = y / CHECKER_SQUARE;
            let color = if (checker_x + checker_y) % 2 == 0 {
                &MAGENTA
            } else {
                &BLACK
            };
            data.extend_from_slice(color);
        }
    }

    LoadedTexture {
        data,
        width: PLACEHOLDER_SIZE,
        height: PLACEHOLDER_SIZE,
        is_placeholder: true,
    }
}

// --- Texture name to file path resolution ---

/// Build a map from lowercase texture name stem to file path by scanning the
/// texture root directory. The texture root contains collection subdirectories,
/// each holding PNG files: `<texture_root>/<collection>/<name>.png`.
fn build_name_to_path_map(texture_root: &Path) -> HashMap<String, PathBuf> {
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

            // Only consider .png files.
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

/// Load a single PNG file and convert to RGBA8. Returns a placeholder on failure.
fn load_png(path: &Path, texture_name: &str) -> LoadedTexture {
    let img = match image::open(path) {
        Ok(img) => img,
        Err(err) => {
            log::warn!(
                "[Texture] Failed to load '{}' from {}: {err} - using checkerboard placeholder",
                texture_name,
                path.display(),
            );
            return generate_checkerboard();
        }
    };

    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();

    LoadedTexture {
        data: rgba.into_raw(),
        width,
        height,
        is_placeholder: false,
    }
}

/// Load all textures referenced by a BSP file.
///
/// `texture_names` is the list of texture names extracted from the BSP miptexture
/// array. Each entry is `Option<String>` because BSP texture entries can be `None`.
/// The returned `TextureSet` is indexed identically: index `i` in the result
/// corresponds to BSP miptexture index `i`.
///
/// `texture_root` is the directory to search for PNG files, typically
/// `<asset_root>/textures/`.
pub fn load_textures(texture_names: &[Option<String>], texture_root: &Path) -> TextureSet {
    let name_to_path = build_name_to_path_map(texture_root);

    let mut textures: Vec<LoadedTexture> = Vec::with_capacity(texture_names.len());
    let mut specular: Vec<Option<LoadedTexture>> = Vec::with_capacity(texture_names.len());
    let mut normal: Vec<Option<LoadedTexture>> = Vec::with_capacity(texture_names.len());

    for (idx, name_opt) in texture_names.iter().enumerate() {
        let name = match name_opt {
            Some(n) => n,
            None => {
                log::warn!(
                    "[Texture] BSP texture index {idx} has no texture entry - using checkerboard placeholder"
                );
                textures.push(generate_checkerboard());
                specular.push(None);
                normal.push(None);
                continue;
            }
        };

        let lookup_key = name.to_lowercase();
        let diffuse = match name_to_path.get(&lookup_key) {
            Some(path) => load_png(path, name),
            None => {
                log::warn!(
                    "[Texture] Texture '{name}' not found in {} - using checkerboard placeholder",
                    texture_root.display(),
                );
                generate_checkerboard()
            }
        };

        // Probe for `{name}_s.png` sibling. Absent → None → shader binds the
        // shared 1×1 black fallback (zero specular). Size mismatch → warn +
        // None. When the diffuse is itself a placeholder (load failed), skip
        // the sibling probe — specular without a real diffuse is meaningless
        // and could spuriously match a placeholder-sized sibling.
        // See context/lib/resource_management.md §4.1.
        let spec_key = format!("{lookup_key}_s");
        let spec = if diffuse.is_placeholder {
            None
        } else {
            match name_to_path.get(&spec_key) {
                Some(path) => {
                    let loaded = load_png(path, &spec_key);
                    if loaded.is_placeholder {
                        None
                    } else if loaded.width != diffuse.width || loaded.height != diffuse.height {
                        log::warn!(
                            "[Texture] Specular '{spec_key}' dimensions {}x{} do not match diffuse '{name}' {}x{} - ignoring",
                            loaded.width,
                            loaded.height,
                            diffuse.width,
                            diffuse.height,
                        );
                        None
                    } else {
                        Some(loaded)
                    }
                }
                None => None,
            }
        };

        // Probe for `{name}_n.png` sibling (tangent-space normal map). Absent
        // → log info once and the renderer binds the shared neutral-normal
        // placeholder. Dimension mismatch → warn + None. Decode failure →
        // error + None. As with `_s`, skip the probe when the diffuse itself
        // failed to load. See context/lib/resource_management.md §4.3.
        let normal_key = format!("{lookup_key}_n");
        let normal_loaded = if diffuse.is_placeholder {
            None
        } else {
            match name_to_path.get(&normal_key) {
                Some(path) => match load_png_strict(path) {
                    Ok(loaded) => {
                        if loaded.width != diffuse.width || loaded.height != diffuse.height {
                            log::warn!(
                                "[Texture] Normal map '{normal_key}' dimensions {}x{} do not match diffuse '{name}' {}x{} - ignoring",
                                loaded.width,
                                loaded.height,
                                diffuse.width,
                                diffuse.height,
                            );
                            None
                        } else {
                            Some(loaded)
                        }
                    }
                    Err(err) => {
                        log::error!(
                            "[Texture] Failed to decode normal map '{}' from {}: {err} - using neutral placeholder",
                            normal_key,
                            path.display(),
                        );
                        None
                    }
                },
                None => {
                    log::info!("[Texture] no normal map for {name}");
                    None
                }
            }
        };

        textures.push(diffuse);
        specular.push(spec);
        normal.push(normal_loaded);
    }

    TextureSet {
        textures,
        specular,
        normal,
    }
}

/// Strict variant of `load_png` that surfaces decode errors to the caller
/// instead of substituting a checkerboard. Used for sidecar maps where the
/// caller wants to log a sidecar-specific message and fall back to a shared
/// placeholder rather than to a per-texture checkerboard.
fn load_png_strict(path: &Path) -> Result<LoadedTexture, image::ImageError> {
    let img = image::open(path)?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(LoadedTexture {
        data: rgba.into_raw(),
        width,
        height,
        is_placeholder: false,
    })
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // -- Checkerboard generation --

    #[test]
    fn checkerboard_has_correct_dimensions() {
        let tex = generate_checkerboard();
        assert_eq!(tex.width, 64);
        assert_eq!(tex.height, 64);
        assert_eq!(tex.data.len(), 64 * 64 * 4);
        assert!(tex.is_placeholder);
    }

    #[test]
    fn checkerboard_top_left_is_magenta() {
        let tex = generate_checkerboard();
        // Pixel (0,0) is in checker square (0,0) which is even+even = magenta.
        assert_eq!(&tex.data[0..4], &MAGENTA);
    }

    #[test]
    fn checkerboard_alternates_correctly() {
        let tex = generate_checkerboard();
        // Pixel (0,0) -> checker (0,0) -> magenta
        assert_eq!(&tex.data[0..4], &MAGENTA);
        // Pixel (8,0) -> checker (1,0) -> black
        let offset_8_0 = (8 * 4) as usize;
        assert_eq!(&tex.data[offset_8_0..offset_8_0 + 4], &BLACK);
        // Pixel (16,0) -> checker (2,0) -> magenta
        let offset_16_0 = (16 * 4) as usize;
        assert_eq!(&tex.data[offset_16_0..offset_16_0 + 4], &MAGENTA);
        // Pixel (0,8) -> checker (0,1) -> black
        let offset_0_8 = (8 * 64 * 4) as usize;
        assert_eq!(&tex.data[offset_0_8..offset_0_8 + 4], &BLACK);
        // Pixel (8,8) -> checker (1,1) -> magenta
        let offset_8_8 = ((8 * 64 + 8) * 4) as usize;
        assert_eq!(&tex.data[offset_8_8..offset_8_8 + 4], &MAGENTA);
    }

    #[test]
    fn checkerboard_all_pixels_are_magenta_or_black() {
        let tex = generate_checkerboard();
        for pixel in tex.data.chunks(4) {
            assert!(
                pixel == MAGENTA || pixel == BLACK,
                "unexpected pixel: {pixel:?}"
            );
        }
    }

    // -- Name-to-path mapping --

    #[test]
    fn build_name_map_finds_pngs_in_collections() {
        let dir = tempdir("name_map_basic");
        let collection = dir.join("metal");
        fs::create_dir(&collection).unwrap();
        fs::write(collection.join("floor_01.png"), minimal_png()).unwrap();
        fs::write(collection.join("wall_02.png"), minimal_png()).unwrap();

        let map = build_name_to_path_map(&dir);

        assert_eq!(map.len(), 2);
        assert!(map.contains_key("floor_01"));
        assert!(map.contains_key("wall_02"));
    }

    #[test]
    fn build_name_map_is_case_insensitive_on_stem() {
        let dir = tempdir("name_map_case");
        let collection = dir.join("concrete");
        fs::create_dir(&collection).unwrap();
        fs::write(collection.join("BRICK_Wall.PNG"), minimal_png()).unwrap();

        let map = build_name_to_path_map(&dir);

        assert!(map.contains_key("brick_wall"));
    }

    #[test]
    fn build_name_map_ignores_non_png_files() {
        let dir = tempdir("name_map_filter");
        let collection = dir.join("stuff");
        fs::create_dir(&collection).unwrap();
        fs::write(collection.join("notes.txt"), b"not a texture").unwrap();
        fs::write(collection.join("data.jpg"), b"not png").unwrap();
        fs::write(collection.join("real.png"), minimal_png()).unwrap();

        let map = build_name_to_path_map(&dir);

        assert_eq!(map.len(), 1);
        assert!(map.contains_key("real"));
    }

    #[test]
    fn build_name_map_handles_missing_directory() {
        let map = build_name_to_path_map(Path::new("/nonexistent/path/that/does/not/exist"));
        assert!(map.is_empty());
    }

    #[test]
    fn build_name_map_ignores_files_at_root_level() {
        // Files directly in the texture root (not in a collection subdirectory)
        // should be ignored.
        let dir = tempdir("name_map_root_files");
        fs::write(dir.join("stray.png"), minimal_png()).unwrap();

        let map = build_name_to_path_map(&dir);
        assert!(map.is_empty());
    }

    // -- load_textures integration --

    #[test]
    fn load_textures_loads_matching_pngs() {
        let dir = tempdir("load_match");
        let collection = dir.join("metal");
        fs::create_dir(&collection).unwrap();
        write_test_png(&collection.join("floor_01.png"), 32, 32);

        let names = vec![Some("floor_01".to_string())];
        let result = load_textures(&names, &dir);

        assert_eq!(result.textures.len(), 1);
        assert!(!result.textures[0].is_placeholder);
        assert_eq!(result.textures[0].width, 32);
        assert_eq!(result.textures[0].height, 32);
        // RGBA8: 32 * 32 * 4 bytes
        assert_eq!(result.textures[0].data.len(), 32 * 32 * 4);
    }

    #[test]
    fn load_textures_case_insensitive_match() {
        let dir = tempdir("load_case");
        let collection = dir.join("concrete");
        fs::create_dir(&collection).unwrap();
        write_test_png(&collection.join("metal_floor_01.png"), 16, 16);

        // BSP name is uppercase; file is lowercase.
        let names = vec![Some("METAL_FLOOR_01".to_string())];
        let result = load_textures(&names, &dir);

        assert_eq!(result.textures.len(), 1);
        assert!(!result.textures[0].is_placeholder);
    }

    #[test]
    fn load_textures_missing_produces_checkerboard() {
        let dir = tempdir("load_missing");
        fs::create_dir(dir.join("empty_collection")).unwrap();

        let names = vec![Some("nonexistent_texture".to_string())];
        let result = load_textures(&names, &dir);

        assert_eq!(result.textures.len(), 1);
        assert!(result.textures[0].is_placeholder);
        assert_eq!(result.textures[0].width, 64);
        assert_eq!(result.textures[0].height, 64);
    }

    #[test]
    fn load_textures_none_entry_produces_checkerboard() {
        let dir = tempdir("load_none");
        fs::create_dir(dir.join("collection")).unwrap();

        let names: Vec<Option<String>> = vec![None];
        let result = load_textures(&names, &dir);

        assert_eq!(result.textures.len(), 1);
        assert!(result.textures[0].is_placeholder);
    }

    #[test]
    fn load_textures_corrupt_png_produces_checkerboard() {
        let dir = tempdir("load_corrupt");
        let collection = dir.join("broken");
        fs::create_dir(&collection).unwrap();
        // Write invalid data as a "PNG" file.
        fs::write(collection.join("bad_texture.png"), b"this is not a PNG").unwrap();

        let names = vec![Some("bad_texture".to_string())];
        let result = load_textures(&names, &dir);

        assert_eq!(result.textures.len(), 1);
        assert!(result.textures[0].is_placeholder);
    }

    #[test]
    fn load_textures_preserves_index_order() {
        let dir = tempdir("load_order");
        let collection = dir.join("set");
        fs::create_dir(&collection).unwrap();
        write_test_png(&collection.join("alpha.png"), 16, 16);
        write_test_png(&collection.join("beta.png"), 32, 32);

        let names = vec![
            Some("alpha".to_string()),
            None,
            Some("beta".to_string()),
            Some("missing".to_string()),
        ];
        let result = load_textures(&names, &dir);

        assert_eq!(result.textures.len(), 4);
        assert!(!result.textures[0].is_placeholder); // alpha found
        assert_eq!(result.textures[0].width, 16);
        assert!(result.textures[1].is_placeholder); // None entry
        assert!(!result.textures[2].is_placeholder); // beta found
        assert_eq!(result.textures[2].width, 32);
        assert!(result.textures[3].is_placeholder); // missing
    }

    #[test]
    fn load_textures_empty_names_produces_empty_set() {
        let dir = tempdir("load_empty");
        fs::create_dir(dir.join("collection")).unwrap();

        let names: Vec<Option<String>> = vec![];
        let result = load_textures(&names, &dir);

        assert!(result.textures.is_empty());
    }

    // -- Test helpers --

    /// Create a temporary directory for tests. Uses the system temp dir to avoid
    /// polluting the project tree.
    fn tempdir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("postretro_texture_tests")
            .join(label);
        // Clean up any prior run.
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Generate a minimal valid PNG in memory (1x1 red pixel). Used for
    /// name-to-path mapping tests where actual pixel content doesn't matter.
    fn minimal_png() -> Vec<u8> {
        use image::{ImageBuffer, Rgba};
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(1, 1, Rgba([255, 0, 0, 255]));
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        img.write_to(&mut cursor, image::ImageFormat::Png).unwrap();
        buf
    }

    /// Write a solid-color test PNG with the given dimensions.
    fn write_test_png(path: &Path, width: u32, height: u32) {
        use image::{ImageBuffer, Rgba};
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(width, height, Rgba([128, 128, 128, 255]));
        img.save(path).unwrap();
    }
}
