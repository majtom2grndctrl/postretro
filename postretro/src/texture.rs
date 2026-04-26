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
                    log::info!("[Texture] No normal map for '{name}' — using neutral placeholder");
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
/// instead of substituting a checkerboard. Used for sidecar textures where the
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

    /// Write a PNG with a specific solid color. Used to verify that sibling
    /// data is preserved verbatim through the loader (e.g. that a normal-map
    /// sibling's pixels reach the consumer unmodified).
    fn write_solid_png(path: &Path, width: u32, height: u32, color: [u8; 4]) {
        use image::{ImageBuffer, Rgba};
        let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(width, height, Rgba(color));
        img.save(path).unwrap();
    }

    // -- Sibling probe: `_n.png` (tangent-space normal map) --
    //
    // Contract (resource_management.md §4.3):
    //   - Sibling absent              → `normal[i] == None`
    //   - Sibling present + dims OK   → `normal[i] == Some(loaded)` with pixels preserved
    //   - Dimension mismatch          → `normal[i] == None`
    //   - Sibling decode failure      → `normal[i] == None` (strict path; no checkerboard substitution)
    //   - Diffuse failed to load      → `normal[i] == None` (probe skipped entirely)

    #[test]
    fn normal_sibling_present_and_matching_produces_some() {
        let dir = tempdir("normal_present_match");
        let collection = dir.join("walls");
        fs::create_dir(&collection).unwrap();
        write_test_png(&collection.join("brick.png"), 32, 32);
        // Neutral tangent-space normal: (0.5, 0.5, 1.0) → (128, 128, 255).
        write_solid_png(&collection.join("brick_n.png"), 32, 32, [128, 128, 255, 255]);

        let result = load_textures(&[Some("brick".to_string())], &dir);

        let normal = result.normal[0]
            .as_ref()
            .expect("matching-dim normal sibling should be loaded");
        assert_eq!(normal.width, 32);
        assert_eq!(normal.height, 32);
        assert!(!normal.is_placeholder);
        // Pixel data must round-trip unmodified — normals are direction
        // vectors, not colors; any silent transform would corrupt lighting.
        assert_eq!(&normal.data[0..4], &[128, 128, 255, 255]);
    }

    #[test]
    fn normal_sibling_absent_produces_none() {
        let dir = tempdir("normal_absent");
        let collection = dir.join("walls");
        fs::create_dir(&collection).unwrap();
        write_test_png(&collection.join("brick.png"), 32, 32);

        let result = load_textures(&[Some("brick".to_string())], &dir);

        assert!(result.normal[0].is_none());
        // Diffuse still loads cleanly when the optional sibling is missing.
        assert!(!result.textures[0].is_placeholder);
    }

    #[test]
    fn normal_sibling_dimension_mismatch_produces_none() {
        let dir = tempdir("normal_dim_mismatch");
        let collection = dir.join("walls");
        fs::create_dir(&collection).unwrap();
        write_test_png(&collection.join("brick.png"), 32, 32);
        // Wrong size — must not be silently rescaled or accepted.
        write_solid_png(&collection.join("brick_n.png"), 16, 16, [128, 128, 255, 255]);

        let result = load_textures(&[Some("brick".to_string())], &dir);

        assert!(result.normal[0].is_none());
    }

    #[test]
    fn normal_sibling_corrupt_produces_none_not_checkerboard() {
        // The strict-load path is what distinguishes `_n` from the diffuse
        // loader: a malformed normal map must NOT be substituted with a
        // checkerboard placeholder (which would have valid 64x64 dims and
        // could surface as broken lighting if it reached the GPU). It must
        // become `None` so the renderer falls back to its neutral placeholder.
        let dir = tempdir("normal_corrupt");
        let collection = dir.join("walls");
        fs::create_dir(&collection).unwrap();
        write_test_png(&collection.join("brick.png"), 32, 32);
        fs::write(collection.join("brick_n.png"), b"not a real PNG file").unwrap();

        let result = load_textures(&[Some("brick".to_string())], &dir);

        assert!(result.normal[0].is_none());
    }

    #[test]
    fn normal_sibling_skipped_when_diffuse_is_placeholder() {
        // Documents the current contract: when the diffuse fails to load and
        // the loader substitutes a 64x64 checkerboard, the sibling probe is
        // skipped even if a valid `_n.png` exists. Rationale (per the source
        // comment): a normal map without a real diffuse is meaningless and
        // could spuriously dimension-match the placeholder.
        let dir = tempdir("normal_skip_when_diffuse_placeholder");
        let collection = dir.join("walls");
        fs::create_dir(&collection).unwrap();
        // No `brick.png` — diffuse will become a 64x64 checkerboard.
        // A valid 64x64 `_n.png` would otherwise dimension-match.
        write_solid_png(&collection.join("brick_n.png"), 64, 64, [128, 128, 255, 255]);

        let result = load_textures(&[Some("brick".to_string())], &dir);

        assert!(result.textures[0].is_placeholder);
        assert!(
            result.normal[0].is_none(),
            "normal probe must be skipped when diffuse is a placeholder"
        );
    }

    // -- Sibling probe: `_s.png` (specular intensity) --
    //
    // Contract (resource_management.md §4.1):
    //   - Sibling absent              → `specular[i] == None`
    //   - Sibling present + dims OK   → `specular[i] == Some(loaded)` with pixels preserved
    //   - Dimension mismatch          → `specular[i] == None`
    //   - Sibling decode failure      → `specular[i] == None` (loader uses non-strict
    //                                    path, then converts the resulting placeholder
    //                                    back to None — checkerboard must not leak through)
    //   - Diffuse failed to load      → `specular[i] == None` (probe skipped entirely)

    #[test]
    fn specular_sibling_present_and_matching_produces_some() {
        let dir = tempdir("specular_present_match");
        let collection = dir.join("metal");
        fs::create_dir(&collection).unwrap();
        write_test_png(&collection.join("plate.png"), 32, 32);
        // Specular intensity in R channel; G/B/A unused by the shader.
        write_solid_png(&collection.join("plate_s.png"), 32, 32, [200, 0, 0, 255]);

        let result = load_textures(&[Some("plate".to_string())], &dir);

        let spec = result.specular[0]
            .as_ref()
            .expect("matching-dim specular sibling should be loaded");
        assert_eq!(spec.width, 32);
        assert_eq!(spec.height, 32);
        assert!(!spec.is_placeholder);
        assert_eq!(spec.data[0], 200, "specular R channel must be preserved");
    }

    #[test]
    fn specular_sibling_absent_produces_none() {
        let dir = tempdir("specular_absent");
        let collection = dir.join("metal");
        fs::create_dir(&collection).unwrap();
        write_test_png(&collection.join("plate.png"), 32, 32);

        let result = load_textures(&[Some("plate".to_string())], &dir);

        assert!(result.specular[0].is_none());
        assert!(!result.textures[0].is_placeholder);
    }

    #[test]
    fn specular_sibling_dimension_mismatch_produces_none() {
        let dir = tempdir("specular_dim_mismatch");
        let collection = dir.join("metal");
        fs::create_dir(&collection).unwrap();
        write_test_png(&collection.join("plate.png"), 64, 64);
        write_solid_png(&collection.join("plate_s.png"), 32, 32, [200, 0, 0, 255]);

        let result = load_textures(&[Some("plate".to_string())], &dir);

        assert!(result.specular[0].is_none());
    }

    #[test]
    fn specular_sibling_corrupt_produces_none_not_checkerboard() {
        // The diffuse loader substitutes a 64x64 checkerboard on PNG decode
        // failure. The specular probe must recognize that placeholder and
        // emit `None` rather than letting a magenta checker reach the shader
        // as a "specular intensity map".
        let dir = tempdir("specular_corrupt");
        let collection = dir.join("metal");
        fs::create_dir(&collection).unwrap();
        write_test_png(&collection.join("plate.png"), 32, 32);
        fs::write(collection.join("plate_s.png"), b"definitely not a PNG").unwrap();

        let result = load_textures(&[Some("plate".to_string())], &dir);

        assert!(result.specular[0].is_none());
    }

    #[test]
    fn specular_sibling_skipped_when_diffuse_is_placeholder() {
        // Mirrors the `_n` contract: no real diffuse → no sibling probe, even
        // when the sibling happens to be a valid 64x64 PNG that would
        // dimension-match the checkerboard.
        let dir = tempdir("specular_skip_when_diffuse_placeholder");
        let collection = dir.join("metal");
        fs::create_dir(&collection).unwrap();
        write_solid_png(&collection.join("plate_s.png"), 64, 64, [200, 0, 0, 255]);

        let result = load_textures(&[Some("plate".to_string())], &dir);

        assert!(result.textures[0].is_placeholder);
        assert!(
            result.specular[0].is_none(),
            "specular probe must be skipped when diffuse is a placeholder"
        );
    }

    // -- Sibling independence --

    #[test]
    fn sibling_probes_are_independent() {
        // A broken `_s.png` must not poison the `_n.png` slot, and vice versa.
        // The two sibling channels are independent contracts.
        let dir = tempdir("sibling_independence");
        let collection = dir.join("mixed");
        fs::create_dir(&collection).unwrap();
        write_test_png(&collection.join("surface.png"), 32, 32);
        write_solid_png(&collection.join("surface_n.png"), 32, 32, [128, 128, 255, 255]);
        // Corrupt specular alongside a valid normal map.
        fs::write(collection.join("surface_s.png"), b"junk").unwrap();

        let result = load_textures(&[Some("surface".to_string())], &dir);

        assert!(result.specular[0].is_none(), "broken _s must be None");
        assert!(
            result.normal[0].is_some(),
            "valid _n must still load when sibling _s is broken"
        );
    }
}
