// Shared glTF document parsing and relative-URI resolution.
// See: context/lib/build_pipeline.md §Baked texture mips · context/lib/resource_management.md §7

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gltf::json::mesh::Semantic;
use gltf::json::validation::{self, Checked};
use gltf::json::{Path as JsonPath, Root};

/// Parses and validates a glTF document. Every document read goes through here,
/// because gltf panics on input it should reject.
///
/// Screened on the parsed JSON, and reported as gltf's own validation errors: a
/// primitive's POSITION index, which gltf's validator uses unchecked; a relative
/// buffer URI that does not percent-decode, which its import unwraps; and an
/// image missing the source fields `Image::source` unwraps. A GLB length inside
/// its own header is refused before the parse, which subtracts unchecked.
///
/// Malformed animation channel targets pass: validation never reaches them, and
/// a skipped channel is the caller's policy.
pub fn parse_document(bytes: &[u8]) -> Result<gltf::Document, gltf::Error> {
    if let Some(error) = short_glb_header(bytes) {
        return Err(error);
    }
    let root = gltf::Gltf::from_slice_without_validation(bytes)?
        .document
        .into_json();
    let errors = panicking_inputs(&root);
    if !errors.is_empty() {
        return Err(gltf::Error::Validation(errors));
    }
    gltf::Document::from_json(root)
}

fn short_glb_header(bytes: &[u8]) -> Option<gltf::Error> {
    const HEADER_SIZE: u32 = 12;
    let [b'g', b'l', b'T', b'F', _, _, _, _, l0, l1, l2, l3, ..] = *bytes else {
        return None;
    };
    let length = u32::from_le_bytes([l0, l1, l2, l3]);
    (length < HEADER_SIZE).then_some(gltf::Error::Binary(gltf::binary::Error::Length {
        length,
        length_read: bytes.len(),
    }))
}

/// [`parse_document`] over the file at `path`.
pub fn open_document(path: &Path) -> Result<gltf::Document, gltf::Error> {
    parse_document(&std::fs::read(path).map_err(gltf::Error::Io)?)
}

fn panicking_inputs(root: &Root) -> Vec<(JsonPath, validation::Error)> {
    let mut errors = Vec::new();
    let position = Checked::Valid(Semantic::Positions);
    for (mesh_index, mesh) in root.meshes.iter().enumerate() {
        for (primitive_index, primitive) in mesh.primitives.iter().enumerate() {
            let in_range = primitive
                .attributes
                .get(&position)
                .is_none_or(|accessor| accessor.value() < root.accessors.len());
            if !in_range {
                let path = JsonPath::new()
                    .field("meshes")
                    .index(mesh_index)
                    .field("primitives")
                    .index(primitive_index)
                    .field("attributes")
                    .key("POSITION");
                errors.push((path, validation::Error::IndexOutOfBounds));
            }
        }
    }
    for (buffer_index, buffer) in root.buffers.iter().enumerate() {
        // gltf percent-decodes only a URI with no colon, through `urlencoding`,
        // which fails on exactly the bytes `percent_encoding` fails on.
        let undecodable = buffer.uri.as_deref().is_some_and(|uri| {
            !uri.contains(':')
                && percent_encoding::percent_decode_str(uri)
                    .decode_utf8()
                    .is_err()
        });
        if undecodable {
            let path = JsonPath::new()
                .field("buffers")
                .index(buffer_index)
                .field("uri");
            errors.push((path, validation::Error::Invalid));
        }
    }
    for (image_index, image) in root.images.iter().enumerate() {
        let missing = match image.buffer_view {
            Some(_) if image.mime_type.is_none() => Some("mimeType"),
            None if image.uri.is_none() => Some("uri"),
            _ => None,
        };
        if let Some(field) = missing {
            let path = JsonPath::new()
                .field("images")
                .index(image_index)
                .field(field);
            errors.push((path, validation::Error::Missing));
        }
    }
    errors
}

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
    let document = open_document(gltf_path)?;
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

    // Regression: gltf validation indexed the accessors with this value unchecked.
    #[test]
    fn base_color_paths_return_err_on_a_position_index_past_the_accessors() {
        let dir = temp_dir("bad_position");
        fs::create_dir_all(&dir).expect("temp directory must be created");
        let path = dir.join("model.gltf");
        fs::write(
            &path,
            r#"{"asset":{"version":"2.0"},"meshes":[{"primitives":[{"attributes":{"POSITION":99}}]}]}"#,
        )
        .expect("glTF fixture must be written");

        let result = std::panic::catch_unwind(|| resolve_document_base_color_paths(&path));
        let _ = fs::remove_dir_all(&dir);

        assert!(matches!(
            result.expect("must not panic"),
            Err(gltf::Error::Validation(_))
        ));
    }

    fn resolve_document(test_name: &str, json: &str) -> Result<Vec<PathBuf>, gltf::Error> {
        let dir = temp_dir(test_name);
        fs::create_dir_all(&dir).expect("temp directory must be created");
        let path = dir.join("model.gltf");
        fs::write(&path, json).expect("glTF fixture must be written");
        let result = std::panic::catch_unwind(|| resolve_document_base_color_paths(&path));
        let _ = fs::remove_dir_all(&dir);
        result.expect("must not panic")
    }

    fn document_with_image(image: &str) -> String {
        format!(
            r#"{{"asset":{{"version":"2.0"}},
                "buffers":[{{"byteLength":4}}],
                "bufferViews":[{{"buffer":0,"byteLength":4}}],
                "images":[{image}],
                "textures":[{{"source":0}}],
                "materials":[{{"pbrMetallicRoughness":{{"baseColorTexture":{{"index":0}}}}}}]}}"#
        )
    }

    // Regression: `Image::source` unwraps the URI an image with no buffer view
    // must carry, and validation does not require one.
    #[test]
    fn image_with_no_source_returns_err() {
        let result = resolve_document("no_source", &document_with_image("{}"));
        assert!(
            matches!(result, Err(gltf::Error::Validation(_))),
            "{result:?}"
        );
    }

    // Regression: `Image::source` unwraps the MIME type a buffer view image must
    // carry, and validation does not require one.
    #[test]
    fn buffer_view_image_with_no_mime_type_returns_err() {
        let result = resolve_document("no_mime", &document_with_image(r#"{"bufferView":0}"#));
        assert!(
            matches!(result, Err(gltf::Error::Validation(_))),
            "{result:?}"
        );
    }

    // Regression: gltf subtracts the header size from a GLB's declared length
    // unchecked, which overflows in dev builds.
    #[test]
    fn glb_declaring_a_length_inside_its_header_returns_err() {
        let mut bytes = b"glTF".to_vec();
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());

        let result = std::panic::catch_unwind(|| parse_document(&bytes));

        assert!(matches!(
            result.expect("must not panic"),
            Err(gltf::Error::Binary(gltf::binary::Error::Length { .. }))
        ));
    }

    #[test]
    fn glb_with_a_json_chunk_parses() {
        let json = br#"{"asset":{"version":"2.0"}} "#;
        assert_eq!(json.len() % 4, 0);
        let mut bytes = b"glTF".to_vec();
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&(20 + json.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"JSON");
        bytes.extend_from_slice(json);

        parse_document(&bytes).expect("GLB parses");
    }

    #[test]
    fn only_a_uri_gltf_percent_decodes_is_screened() {
        let document = |uri: &str| {
            format!(
                r#"{{"asset":{{"version":"2.0"}},"buffers":[{{"byteLength":4,"uri":"{uri}"}}]}}"#
            )
        };
        let result = parse_document(document("%FF.bin").as_bytes());
        assert!(
            matches!(result, Err(gltf::Error::Validation(_))),
            "{result:?}"
        );

        // A URI with a colon fails on its scheme at import, or reads as it is.
        parse_document(document("x:%FF.bin").as_bytes()).expect("scheme URI parses");
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
