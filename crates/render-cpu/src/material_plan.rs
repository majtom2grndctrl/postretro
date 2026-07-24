// Material uniform packing and submesh material draw planning.
// See: context/lib/rendering_pipeline.md §9

use std::path::Path;

/// Highest valid LOD index for a chain of `mip_count` mips.
pub fn mip_lod_max_clamp(mip_count: u32) -> f32 {
    mip_count.saturating_sub(1) as f32
}

pub const MATERIAL_UNIFORM_SIZE: usize = 32;

pub fn build_material_uniform(
    shininess: f32,
    emissive_strength: f32,
) -> [u8; MATERIAL_UNIFORM_SIZE] {
    let mut bytes = [0u8; MATERIAL_UNIFORM_SIZE];
    bytes[0..4].copy_from_slice(&shininess.to_le_bytes());
    bytes[4..8].copy_from_slice(&emissive_strength.to_le_bytes());
    bytes
}

#[cfg(test)]
mod material_uniform_tests {
    use super::*;

    #[test]
    fn material_uniform_packs_shininess_and_emissive_strength_in_first_row() {
        let bytes = build_material_uniform(32.0, 4.0);
        assert_eq!(bytes.len(), MATERIAL_UNIFORM_SIZE);
        assert_eq!(&bytes[0..4], &32.0f32.to_le_bytes());
        assert_eq!(&bytes[4..8], &4.0f32.to_le_bytes());
        assert!(bytes[8..].iter().all(|&byte| byte == 0));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmeshDraw {
    pub distinct: usize,
    pub indices: std::ops::Range<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmeshMaterialPlan {
    pub distinct_keys: Vec<String>,
    pub draws: Vec<SubmeshDraw>,
}

pub fn plan_submesh_materials(
    submeshes: &[postretro_model::gltf_loader::Submesh],
) -> SubmeshMaterialPlan {
    let mut distinct_keys: Vec<String> = Vec::new();
    let mut draws: Vec<SubmeshDraw> = Vec::with_capacity(submeshes.len());
    for sub in submeshes {
        let distinct = match distinct_keys.iter().position(|k| k == &sub.material_key) {
            Some(idx) => idx,
            None => {
                distinct_keys.push(sub.material_key.clone());
                distinct_keys.len() - 1
            }
        };
        draws.push(SubmeshDraw {
            distinct,
            indices: sub.indices.clone(),
        });
    }
    SubmeshMaterialPlan {
        distinct_keys,
        draws,
    }
}

pub fn parse_blake3_key(hex: &str) -> [u8; 32] {
    let mut key = [0u8; 32];
    if hex.len() != 64 {
        return [0u8; 32];
    }

    for (byte, pair) in key.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
        let [high, low] = pair else {
            return [0u8; 32];
        };
        let (Some(high), Some(low)) = (ascii_hex_nibble(*high), ascii_hex_nibble(*low)) else {
            return [0u8; 32];
        };
        *byte = (high << 4) | low;
    }
    key
}

fn ascii_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn resolve_model_open_path_and_handle(
    model_rel: &str,
    content_root: &Path,
) -> (std::path::PathBuf, postretro_model::ModelHandle) {
    (
        content_root.join(model_rel),
        postretro_model::ModelHandle::from(model_rel.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_model::gltf_loader::Submesh;

    fn submesh(key: &str, start: u32, end: u32) -> Submesh {
        Submesh {
            material_key: key.to_string(),
            indices: start..end,
        }
    }

    #[test]
    fn parse_blake3_key_parses_valid_hex_to_expected_bytes() {
        let hex = (0u8..32).map(|b| format!("{b:02x}")).collect::<String>();
        let result = parse_blake3_key(&hex);
        let expected: [u8; 32] = std::array::from_fn(|i| i as u8);
        assert_eq!(result, expected);
    }

    #[test]
    fn parse_blake3_key_wrong_length_returns_zero_sentinel() {
        assert_eq!(parse_blake3_key(&"a".repeat(63)), [0u8; 32]);
    }

    #[test]
    fn parse_blake3_key_non_hex_chars_return_zero_sentinel() {
        let bad = format!("zz{}", "00".repeat(31));
        assert_eq!(parse_blake3_key(&bad), [0u8; 32]);
    }

    #[test]
    fn parse_blake3_key_non_ascii_input_does_not_panic_and_returns_zero_sentinel() {
        let non_ascii = "é".repeat(32);
        assert_eq!(non_ascii.len(), 64);
        let result = std::panic::catch_unwind(|| parse_blake3_key(&non_ascii));
        assert!(result.is_ok());
        assert_eq!(result.expect("parser must not panic"), [0u8; 32]);
    }

    #[test]
    fn parse_blake3_key_maps_zero_sentinel_to_zero_key() {
        assert_eq!(parse_blake3_key(&"0".repeat(64)), [0u8; 32]);
    }

    #[test]
    fn model_cache_key_is_the_verbatim_handle_while_open_path_is_joined() {
        let content_root = Path::new("/content/root");
        let model_rel = "models/x/scene.gltf";
        let (open_path, handle) = resolve_model_open_path_and_handle(model_rel, content_root);
        assert_eq!(open_path, content_root.join(model_rel));
        assert_eq!(handle, postretro_model::ModelHandle::from(model_rel));
        assert_ne!(handle.as_str(), open_path.to_string_lossy());
    }

    #[test]
    fn plan_records_one_draw_per_submesh_covering_every_range() {
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let c = "c".repeat(64);
        let submeshes = vec![submesh(&a, 0, 6), submesh(&b, 6, 12), submesh(&c, 12, 15)];

        let plan = plan_submesh_materials(&submeshes);

        assert_eq!(plan.distinct_keys, vec![a, b, c]);
        assert_eq!(plan.draws.len(), 3);
        assert_eq!(plan.draws[0].indices, 0..6);
        assert_eq!(plan.draws[1].indices, 6..12);
        assert_eq!(plan.draws[2].indices, 12..15);
        assert_eq!(
            plan.draws.iter().map(|d| d.distinct).collect::<Vec<_>>(),
            vec![0, 1, 2],
        );
    }

    #[test]
    fn plan_dedups_repeated_material_key_to_one_build() {
        let shared = "f".repeat(64);
        let submeshes = vec![
            submesh(&shared, 0, 3),
            submesh(&shared, 3, 6),
            submesh(&shared, 6, 9),
        ];

        let plan = plan_submesh_materials(&submeshes);

        assert_eq!(plan.distinct_keys, vec![shared]);
        assert_eq!(plan.draws.len(), 3);
        assert!(plan.draws.iter().all(|d| d.distinct == 0));
    }

    #[test]
    fn plan_mixes_shared_and_distinct_keys_with_first_seen_order() {
        let x = "1".repeat(64);
        let y = "2".repeat(64);
        let z = "3".repeat(64);
        let submeshes = vec![
            submesh(&x, 0, 3),
            submesh(&y, 3, 6),
            submesh(&x, 6, 9),
            submesh(&z, 9, 12),
        ];

        let plan = plan_submesh_materials(&submeshes);

        assert_eq!(plan.distinct_keys, vec![x, y, z]);
        assert_eq!(
            plan.draws.iter().map(|d| d.distinct).collect::<Vec<_>>(),
            vec![0, 1, 0, 2],
        );
        assert_eq!(plan.draws.len(), 4);
    }

    #[test]
    fn mip_lod_max_clamp_derivation() {
        assert_eq!(mip_lod_max_clamp(1), 0.0);
        assert_eq!(mip_lod_max_clamp(8), 7.0);
        assert_eq!(mip_lod_max_clamp(0), 0.0);
    }
}
