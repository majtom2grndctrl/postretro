// Material uniform packing and submesh material draw planning.
// See: context/lib/rendering_pipeline.md §9

use std::path::Path;

/// Highest valid LOD index for a chain of `mip_count` mips.
pub fn mip_lod_max_clamp(mip_count: u32) -> f32 {
    mip_count.saturating_sub(1) as f32
}

pub const MATERIAL_UNIFORM_SIZE: usize = 32;

/// Byte layout of the per-material uniform, mirrored EXACTLY by the
/// `MaterialUniform` struct in both `forward.wgsl` and `kinematic_brush.wgsl`
/// (`material_uniform_layout_is_mirrored_by_both_world_shaders` pins it):
///
/// ```text
///   0..4   shininess                       16..20  surface_depth_meters
///   4..8   emissive_strength               20..24  surface_depth_fade_distance
///   8..16  _pad (vec2<f32>)                24..28  surface_depth_quantize_levels
///                                          28..32  surface_depth_march (packed)
/// ```
///
/// The second 16-byte row was already allocated and already zeroed before
/// Surface Depth existed (`MATERIAL_UNIFORM_SIZE` has been 32 while the WGSL
/// struct was 16), which is why this feature needs no buffer resize, no new
/// binding, and no change to the 128-byte group-0 `Uniforms` ABI.
///
/// An all-zero second row is the flat material: depth 0, no fade, no steps,
/// has-depth clear.
pub fn build_material_uniform(
    shininess: f32,
    emissive_strength: f32,
    surface_depth: crate::surface_depth::SurfaceDepthUniform,
) -> [u8; MATERIAL_UNIFORM_SIZE] {
    let mut bytes = [0u8; MATERIAL_UNIFORM_SIZE];
    bytes[0..4].copy_from_slice(&shininess.to_le_bytes());
    bytes[4..8].copy_from_slice(&emissive_strength.to_le_bytes());
    bytes[16..20].copy_from_slice(&surface_depth.depth.depth_meters.to_le_bytes());
    bytes[20..24].copy_from_slice(&surface_depth.depth.fade_distance_meters.to_le_bytes());
    bytes[24..28].copy_from_slice(&(surface_depth.depth.quantize_levels as f32).to_le_bytes());
    bytes[28..32].copy_from_slice(&surface_depth.march_word().to_le_bytes());
    bytes
}

/// Everything needed to rebuild ONE material's uniform bytes at any quality
/// tier, with no GPU access.
///
/// The renderer retains this beside each material's uniform buffer so the
/// player-facing Surface Depth tier (design D5) can be applied live by
/// rewriting the buffer — `queue.write_buffer`, not a bind-group rebuild
/// (`resource_management.md` §8.2: handles are stable, nothing allocates during
/// gameplay) and not a new group-0 uniform field (that struct is exactly 128
/// bytes under a 4-way ABI contract).
///
/// The two texture facts are recorded at bind-group build time from the slot
/// that ACTUALLY loaded, not from the material prefix, so a later rewrite
/// cannot resurrect a carve for a material whose `.prm` has no height sibling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaterialUniformPlan {
    pub shininess: f32,
    pub emissive_strength: f32,
    /// The material prefix's own tuning, BEFORE any quality tier is applied.
    /// Storing the untiered value is what lets a rewrite move up as well as
    /// down: `Off` is not a one-way door.
    pub surface_depth: postretro_render_data::material::SurfaceDepth,
    /// The bound specular slot is a two-channel `Rg8Unorm` surface map.
    pub specular_is_surface_map: bool,
    /// Mip levels actually uploaded to that slot; clamps the DDA's base mip.
    pub specular_mip_count: u32,
    /// Residency's requested base mip (D6.2), today always
    /// [`crate::surface_depth::SURFACE_DEPTH_RESIDENT_BASE_MIP`].
    pub requested_base_mip: u32,
}

impl MaterialUniformPlan {
    /// Plan one material against the slot that actually loaded.
    pub fn new(
        material: postretro_render_data::material::Material,
        specular_is_surface_map: bool,
        specular_mip_count: u32,
    ) -> Self {
        Self {
            shininess: material.shininess(),
            emissive_strength: material.emissive_strength(),
            surface_depth: material.surface_depth(),
            specular_is_surface_map,
            specular_mip_count,
            requested_base_mip: crate::surface_depth::SURFACE_DEPTH_RESIDENT_BASE_MIP,
        }
    }

    /// The exact 32 bytes this material uploads at `quality`.
    ///
    /// Deterministic and total: the same plan and tier always produce the same
    /// bytes, so a live rewrite and a fresh level install agree byte for byte.
    pub fn uniform_bytes(
        self,
        quality: crate::surface_depth::SurfaceDepthQuality,
    ) -> [u8; MATERIAL_UNIFORM_SIZE] {
        build_material_uniform(
            self.shininess,
            self.emissive_strength,
            crate::surface_depth::SurfaceDepthUniform::resolve(
                self.surface_depth,
                quality,
                self.specular_is_surface_map,
                self.specular_mip_count,
                self.requested_base_mip,
            ),
        )
    }
}

#[cfg(test)]
mod material_uniform_tests {
    use super::*;
    use crate::surface_depth::{
        SURFACE_DEPTH_HAS_DEPTH_BIT, SurfaceDepthQuality, SurfaceDepthUniform,
    };
    use postretro_render_data::material::{Material, SurfaceDepth};

    #[test]
    fn material_uniform_packs_shininess_and_emissive_strength_in_first_row() {
        let bytes = build_material_uniform(32.0, 4.0, SurfaceDepthUniform::FLAT);
        assert_eq!(bytes.len(), MATERIAL_UNIFORM_SIZE);
        assert_eq!(&bytes[0..4], &32.0f32.to_le_bytes());
        assert_eq!(&bytes[4..8], &4.0f32.to_le_bytes());
        assert!(bytes[8..16].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn a_flat_material_leaves_the_second_row_all_zero() {
        // The pre-Surface-Depth contents of this buffer, byte for byte: an
        // existing material must upload exactly what it used to.
        let bytes = build_material_uniform(32.0, 4.0, SurfaceDepthUniform::FLAT);
        assert!(bytes[8..].iter().all(|&byte| byte == 0));
    }

    #[test]
    fn material_uniform_packs_surface_depth_in_the_second_row() {
        let resolved = SurfaceDepthUniform::resolve(
            SurfaceDepth {
                depth_meters: 0.02,
                quantize_levels: 12,
                max_steps: 24,
                fade_distance_meters: 14.0,
            },
            SurfaceDepthQuality::High,
            true,
            11,
            crate::surface_depth::SURFACE_DEPTH_RESIDENT_BASE_MIP,
        );
        let bytes = build_material_uniform(4.0, 0.0, resolved);
        assert_eq!(&bytes[16..20], &0.02f32.to_le_bytes());
        assert_eq!(&bytes[20..24], &14.0f32.to_le_bytes());
        assert_eq!(&bytes[24..28], &12.0f32.to_le_bytes());
        let march = u32::from_le_bytes(bytes[28..32].try_into().unwrap());
        assert_ne!(march & SURFACE_DEPTH_HAS_DEPTH_BIT, 0);
        assert_eq!(march & 0xFF, 24);
    }

    // -- Player quality tier (D5): the bytes a live rewrite uploads --

    /// A carving material with a real surface map: the plan the renderer keeps
    /// for a `.prm` whose specular slot baked to `Rg8Unorm`.
    fn carving_plan() -> MaterialUniformPlan {
        MaterialUniformPlan::new(Material::Concrete, true, 11)
    }

    #[test]
    fn off_uploads_bytes_identical_to_a_material_with_no_surface_map() {
        let plan = carving_plan();
        // The same material as it would load with NO `_h.png` sibling: the
        // pre-Surface-Depth bytes, byte for byte.
        let flat_material_bytes = build_material_uniform(
            Material::Concrete.shininess(),
            Material::Concrete.emissive_strength(),
            SurfaceDepthUniform::FLAT,
        );

        assert_eq!(
            plan.uniform_bytes(SurfaceDepthQuality::Off),
            flat_material_bytes,
            "Off must be bit-identical to the flat path",
        );
        assert!(
            plan.uniform_bytes(SurfaceDepthQuality::Off)[16..]
                .iter()
                .all(|&byte| byte == 0),
            "Off must upload the historical all-zero second row",
        );
    }

    #[test]
    fn off_never_alters_the_first_row_any_tier_uploads() {
        let plan = carving_plan();
        let high = plan.uniform_bytes(SurfaceDepthQuality::High);
        for quality in SurfaceDepthQuality::ALL {
            assert_eq!(
                plan.uniform_bytes(quality)[..16],
                high[..16],
                "{quality:?} must not disturb shininess/emissive_strength",
            );
        }
    }

    #[test]
    fn each_tier_uploads_distinct_second_row_bytes() {
        let plan = carving_plan();
        let off = plan.uniform_bytes(SurfaceDepthQuality::Off);
        let low = plan.uniform_bytes(SurfaceDepthQuality::Low);
        let high = plan.uniform_bytes(SurfaceDepthQuality::High);
        assert_ne!(off[16..], low[16..]);
        assert_ne!(low[16..], high[16..]);
        assert_ne!(off[16..], high[16..]);

        // Low keeps the carve depth and the terracing, shortens the fade, and
        // clears the self-shadow budget.
        assert_eq!(low[16..20], high[16..20], "carve depth is unchanged at Low");
        assert_eq!(
            low[24..28],
            high[24..28],
            "quantization is unchanged at Low"
        );
        let low_fade = f32::from_le_bytes(low[20..24].try_into().unwrap());
        let high_fade = f32::from_le_bytes(high[20..24].try_into().unwrap());
        assert!(low_fade < high_fade && low_fade > 0.0);

        let low_march = crate::surface_depth::unpack_surface_depth_march(u32::from_le_bytes(
            low[28..32].try_into().unwrap(),
        ));
        let high_march = crate::surface_depth::unpack_surface_depth_march(u32::from_le_bytes(
            high[28..32].try_into().unwrap(),
        ));
        assert!(low_march.has_depth && high_march.has_depth);
        assert!(low_march.max_steps < high_march.max_steps);
        assert_eq!(low_march.shadow_light_budget, 0);
        assert!(high_march.shadow_light_budget > 0);
        assert_eq!(low_march.base_mip, high_march.base_mip);
    }

    #[test]
    fn a_material_with_no_surface_map_is_tier_independent() {
        // The whole point of deciding has-depth from the LOADED slot: no
        // quality tier may make a material without an `_h.png` sibling march.
        let plan = MaterialUniformPlan::new(Material::Concrete, false, 11);
        for quality in SurfaceDepthQuality::ALL {
            assert!(
                plan.uniform_bytes(quality)[16..].iter().all(|&b| b == 0),
                "{quality:?} must leave a map-less material on the flat path",
            );
        }
    }

    #[test]
    fn a_tier_change_is_reversible_byte_for_byte() {
        // `Off` must not be a one-way door: the plan retains the material's own
        // untiered tuning, so returning to High restores the exact bytes.
        let plan = carving_plan();
        let high = plan.uniform_bytes(SurfaceDepthQuality::High);
        let _ = plan.uniform_bytes(SurfaceDepthQuality::Off);
        let _ = plan.uniform_bytes(SurfaceDepthQuality::Low);
        assert_eq!(plan.uniform_bytes(SurfaceDepthQuality::High), high);
    }

    #[test]
    fn the_plan_records_the_first_row_from_the_material_prefix() {
        let plan = MaterialUniformPlan::new(Material::Metal, true, 4);
        let bytes = plan.uniform_bytes(SurfaceDepthQuality::High);
        assert_eq!(&bytes[0..4], &Material::Metal.shininess().to_le_bytes());
        assert_eq!(
            &bytes[4..8],
            &Material::Metal.emissive_strength().to_le_bytes()
        );
        assert_eq!(plan.surface_depth, Material::Metal.surface_depth());
    }

    #[test]
    fn the_plan_clamps_the_base_mip_to_the_uploaded_chain() {
        // A one-level chain (the placeholder shape) can only ever load level 0.
        let plan = MaterialUniformPlan {
            requested_base_mip: 9,
            ..MaterialUniformPlan::new(Material::Concrete, true, 1)
        };
        let march = crate::surface_depth::unpack_surface_depth_march(u32::from_le_bytes(
            plan.uniform_bytes(SurfaceDepthQuality::High)[28..32]
                .try_into()
                .unwrap(),
        ));
        assert_eq!(march.base_mip, 0);
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
