// Shared glTF relative-URI resolution for runtime, level compilation and packaging.
// See: context/lib/build_pipeline.md §Baked texture mips · context/lib/resource_management.md §7

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Resolves a reference relative to a glTF document, for references the
/// document's own schema does not type as a URI, such as `extras.clipLibrary`.
///
/// Returns `None` for anything but a non-empty relative path: URI schemes,
/// including `data:` and `file:`, any other colon, a backslash, absolute or
/// rooted paths, and percent escapes that do not decode to UTF-8. Every rule
/// applies after percent decoding.
///
/// Parent segments are joined unchanged: glTF content is trusted and may
/// deliberately reference a sibling directory.
pub fn resolve_relative_uri(uri: &str, parent_dir: &Path) -> Option<PathBuf> {
    let decoded = percent_encoding::percent_decode_str(uri)
        .decode_utf8()
        .ok()?;
    // A backslash separates on Windows and names a file elsewhere, so accepting
    // one would give a document a different path and failure mode per platform.
    (!decoded.is_empty() && !decoded.contains('\\') && is_relative_reference(&decoded))
        .then(|| parent_dir.join(&*decoded))
}

fn is_relative_reference(decoded: &str) -> bool {
    // `has_root`, not `is_absolute`. On Windows a leading-slash path carries a
    // root but no drive prefix, so `is_absolute` is false for it — and a join
    // then *replaces* the parent directory rather than extending it, which is
    // the one outcome this guard exists to prevent. A glTF URI is a relative
    // reference by specification, so a rooted one is unsupported on both
    // platforms.
    let decoded_path = Path::new(decoded);
    !(decoded.contains(':') || decoded_path.is_absolute() || decoded_path.has_root())
}

/// Resolves a material's relative filesystem base-color URI beside its glTF.
///
/// URI schemes, including `data:` and `file:`, absolute paths, materials without
/// base-color textures, and buffer-view images return `None`. Callers treat
/// `None` as unavailable and degrade to a placeholder or skip its offline bake.
///
/// Decoding and rejection are [`resolve_relative_uri`]'s, with three exceptions
/// that leave failure to the later read. Invalid UTF-8 falls back to the literal
/// URI, an empty URI joins as the parent directory, and a backslash passes through.
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
        .unwrap_or(Cow::Borrowed(uri));
    is_relative_reference(&decoded).then(|| parent_dir.join(&*decoded))
}

/// Opens a glTF document and returns its distinct relative base-color paths.
///
/// This parses only the glTF document; it does not import buffers or images.
/// Unsupported image sources are omitted. Paths retain first-material order.
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
    fn relative_uri_resolver_decodes_and_joins_relative_paths() {
        let parent = Path::new("/content/models/trooper");
        for (uri, expected) in [
            ("library.gltf", "/content/models/trooper/library.gltf"),
            (
                "../clip%20libraries/rifle.gltf",
                "/content/models/trooper/../clip libraries/rifle.gltf",
            ),
            (
                "%2E%2E%2Frifle.gltf",
                "/content/models/trooper/../rifle.gltf",
            ),
        ] {
            assert_eq!(
                resolve_relative_uri(uri, parent),
                Some(PathBuf::from(expected)),
                "relative reference must resolve: {uri}"
            );
        }
    }

    #[test]
    fn relative_uri_resolver_rejects_everything_but_a_relative_path() {
        for uri in [
            "",
            "data:model/gltf+json;base64,AAAA",
            "file:///library.gltf",
            "file%3A%2F%2F%2Flibrary.gltf",
            "http://x",
            "C:lib.gltf",
            "C%3Alib.gltf",
            "/library.gltf",
            "%2Flibrary.gltf",
            "clips\\library.gltf",
            "..\\library.gltf",
            "\\library.gltf",
            "clips%5Clibrary.gltf",
            "library%FF.gltf",
        ] {
            assert_eq!(
                resolve_relative_uri(uri, Path::new("/content/models")),
                None,
                "reference must be rejected: {uri:?}"
            );
        }
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

    /// Regression: `/assets/base.png` was accepted on Windows, where a rooted
    /// path is not *absolute* without a drive prefix. `Path::join` still
    /// replaces on a rooted argument, so the resolver returned the URI itself
    /// and the parent directory vanished — on the one platform where the guard
    /// reads as satisfied.
    #[test]
    fn material_resolver_rejects_uri_schemes_and_absolute_paths() {
        for uri in [
            "data:image/png;base64,AAAA",
            "https://example.com/base.png",
            "file:///assets/base.png",
            "/assets/base.png",
        ] {
            let json = format!(
                r#"{{
                    "asset": {{"version": "2.0"}},
                    "images": [{{"uri": "{uri}"}}],
                    "textures": [{{"source": 0}}],
                    "materials": [{{
                        "pbrMetallicRoughness": {{"baseColorTexture": {{"index": 0}}}}
                    }}]
                }}"#
            );
            let document = parse_gltf(&json);
            let material = document
                .materials()
                .next()
                .expect("fixture must contain a material");

            assert_eq!(
                resolve_material_base_color_path(&material, Path::new("/content/models")),
                None,
                "URI must be unsupported: {uri}"
            );
        }
    }

    // Regression: percent-encoded schemes bypassed validation before URI decoding.
    #[test]
    fn material_resolver_rejects_percent_encoded_uri_schemes() {
        for uri in [
            "https%3A%2F%2Fexample.com%2Fbase.png",
            "file%3A%2F%2F%2Fcontent%2Fbase.png",
        ] {
            let json = format!(
                r#"{{
                    "asset": {{"version": "2.0"}},
                    "images": [{{"uri": "{uri}"}}],
                    "textures": [{{"source": 0}}],
                    "materials": [{{
                        "pbrMetallicRoughness": {{"baseColorTexture": {{"index": 0}}}}
                    }}]
                }}"#
            );
            let document = parse_gltf(&json);
            let material = document
                .materials()
                .next()
                .expect("fixture must contain a material");

            assert_eq!(
                resolve_material_base_color_path(&material, Path::new("/content/models")),
                None,
                "URI must be unsupported after decoding: {uri}"
            );
        }
    }

    #[test]
    fn material_resolver_keeps_backslashes_and_invalid_utf8_literal() {
        let resolve = |uri: &str| {
            let json = format!(
                r#"{{
                    "asset": {{"version": "2.0"}},
                    "images": [{{"uri": "{uri}"}}],
                    "textures": [{{"source": 0}}],
                    "materials": [{{
                        "pbrMetallicRoughness": {{"baseColorTexture": {{"index": 0}}}}
                    }}]
                }}"#
            );
            let document = parse_gltf(&json);
            let material = document
                .materials()
                .next()
                .expect("fixture must contain a material");
            resolve_material_base_color_path(&material, Path::new("/content/models"))
        };

        assert_eq!(
            resolve("textures%5Cbase.png"),
            Some(Path::new("/content/models").join("textures\\base.png")),
        );
        assert_eq!(
            resolve("base%FF.png"),
            Some(PathBuf::from("/content/models/base%FF.png")),
            "the later read owns an undecodable URI"
        );
    }

    #[test]
    fn material_resolver_preserves_trusted_parent_segments() {
        for (uri, expected) in [
            (
                "../textures/base.png",
                "/content/models/../textures/base.png",
            ),
            (
                "%2E%2E%2Ftextures%2Fbase%20color.png",
                "/content/models/../textures/base color.png",
            ),
        ] {
            let json = format!(
                r#"{{
                    "asset": {{"version": "2.0"}},
                    "images": [{{"uri": "{uri}"}}],
                    "textures": [{{"source": 0}}],
                    "materials": [{{
                        "pbrMetallicRoughness": {{"baseColorTexture": {{"index": 0}}}}
                    }}]
                }}"#
            );
            let document = parse_gltf(&json);
            let material = document
                .materials()
                .next()
                .expect("fixture must contain a material");

            assert_eq!(
                resolve_material_base_color_path(&material, Path::new("/content/models")),
                Some(PathBuf::from(expected)),
                "relative URI must remain supported: {uri}"
            );
        }
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
