// Shared glTF base-color URI resolution for runtime and level compilation.
// See: context/lib/build_pipeline.md §Baked texture mips

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Resolves a material's external base-color image URI relative to its glTF.
///
/// Materials without a base-color texture and images embedded in a buffer view
/// return `None`.
pub fn resolve_material_base_color_path(
    material: &gltf::Material,
    parent_dir: &Path,
) -> Option<PathBuf> {
    let uri = material
        .pbr_metallic_roughness()
        .base_color_texture()
        .and_then(|info| match info.texture().source().source() {
            gltf::image::Source::Uri { uri, .. } => Some(uri),
            gltf::image::Source::View { .. } => None,
        })?;

    let decoded = percent_encoding::percent_decode_str(uri)
        .decode_utf8()
        .map(|value| value.into_owned())
        .unwrap_or_else(|_| uri.to_string());
    Some(parent_dir.join(decoded))
}

/// Opens a glTF document and returns its distinct external base-color paths.
///
/// This parses only the glTF document; it does not import buffers or images.
/// Paths retain first-material order.
pub fn resolve_document_base_color_paths(gltf_path: &Path) -> Result<Vec<PathBuf>, gltf::Error> {
    let document = gltf::Gltf::open(gltf_path)?;
    let parent_dir = gltf_path.parent().unwrap_or_else(|| Path::new(""));
    let mut seen = HashSet::new();
    let mut paths = Vec::new();

    for material in document.materials() {
        let Some(path) = resolve_material_base_color_path(&material, parent_dir) else {
            continue;
        };
        if seen.insert(path.clone()) {
            paths.push(path);
        }
    }

    Ok(paths)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    fn parse_gltf(json: &str) -> gltf::Gltf {
        gltf::Gltf::from_slice(json.as_bytes()).expect("fixture must be valid glTF")
    }

    fn temp_dir(test_name: &str) -> PathBuf {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "postretro_gltf_resolve_{test_name}_{}_{}",
            std::process::id(),
            id
        ))
    }

    #[test]
    fn material_resolver_decodes_external_uri_and_joins_parent() {
        let json = r#"{
            "asset": {"version": "2.0"},
            "images": [{"uri": "textures/base%20color.png"}],
            "textures": [{"source": 0}],
            "materials": [{
                "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}
            }]
        }"#;
        let document = parse_gltf(json);
        let material = document
            .materials()
            .next()
            .expect("fixture must contain a material");

        assert_eq!(
            resolve_material_base_color_path(&material, Path::new("/content/models")),
            Some(PathBuf::from("/content/models/textures/base color.png"))
        );
    }

    #[test]
    fn material_resolver_returns_none_for_embedded_view() {
        let json = r#"{
            "asset": {"version": "2.0"},
            "buffers": [{"byteLength": 4, "uri": "scene.bin"}],
            "bufferViews": [{"buffer": 0, "byteLength": 4}],
            "images": [{"bufferView": 0, "mimeType": "image/png"}],
            "textures": [{"source": 0}],
            "materials": [{
                "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}
            }]
        }"#;
        let document = parse_gltf(json);
        let material = document
            .materials()
            .next()
            .expect("fixture must contain a material");

        assert_eq!(
            resolve_material_base_color_path(&material, Path::new("/content/models")),
            None
        );
    }

    #[test]
    fn document_resolver_deduplicates_paths_in_material_order() {
        let dir = temp_dir("dedup");
        fs::create_dir_all(&dir).expect("temp directory must be created");
        let gltf_path = dir.join("scene.gltf");
        let json = r#"{
            "asset": {"version": "2.0"},
            "images": [
                {"uri": "first.png"},
                {"uri": "second%20image.png"}
            ],
            "textures": [
                {"source": 0},
                {"source": 1}
            ],
            "materials": [
                {"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}},
                {"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}},
                {"pbrMetallicRoughness": {"baseColorTexture": {"index": 1}}},
                {}
            ]
        }"#;
        fs::write(&gltf_path, json).expect("glTF fixture must be written");

        let paths = resolve_document_base_color_paths(&gltf_path).expect("fixture must resolve");

        assert_eq!(
            paths,
            vec![dir.join("first.png"), dir.join("second image.png")]
        );
        fs::remove_dir_all(&dir).expect("temp fixture must be removed");
    }
}
