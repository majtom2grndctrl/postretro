// Ephemeral, per-name diffuse statistics shared by the SH estimators and their cache keys.
// See: context/plans/in-progress/coloured-sh-bounce/index.md

use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use glam::Vec3;
use postretro_level_format::prm::{PrmFormat, PrmSlot};

use crate::geometry::GeometryResult;
use crate::texture_mips::linear_to_srgb_u8;

pub const BOUNCE_SATURATION: f32 = 1.0;
pub const GREY_BOUNCE_ALBEDO: f32 = 0.45;

/// A fixed-size derived record; texels remain borrowed from the material baker.
#[derive(Clone, Copy, Debug)]
pub struct MaterialStats {
    snapped_mean: Option<[u8; 3]>,
    albedo: Vec3,
}

impl Default for MaterialStats {
    fn default() -> Self {
        Self {
            snapped_mean: None,
            albedo: Vec3::splat(GREY_BOUNCE_ALBEDO),
        }
    }
}

impl MaterialStats {
    pub fn from_slots(
        diffuse: Option<&PrmSlot>,
        emissive: Option<&PrmSlot>,
        emissive_strength: f32,
        saturation: f32,
        lut: &[f32; 256],
    ) -> Result<Self> {
        validate_saturation(saturation)?;
        let Some(diffuse) = diffuse else {
            // No diffuse means no reflectance; do not inspect an orphan glow slot.
            return Ok(Self::default());
        };
        let mean = mean_from_slots(diffuse, emissive, emissive_strength, lut)?;
        let snapped_mean = mean.map(linear_to_srgb_u8);
        let colour = Vec3::from_array(snapped_mean.map(|v| lut[usize::from(v)]));
        let luminance = colour.dot(Vec3::new(0.2126, 0.7152, 0.0722));
        // This form returns the snapped colour bit-for-bit at saturation 1.
        let albedo = saturation * colour + (1.0 - saturation) * Vec3::splat(luminance);
        Ok(Self {
            snapped_mean: Some(snapped_mean),
            albedo,
        })
    }

    pub fn snapped_mean(&self) -> Option<[u8; 3]> {
        self.snapped_mean
    }

    pub fn albedo(&self) -> Vec3 {
        self.albedo
    }
}

fn validate_saturation(saturation: f32) -> Result<()> {
    ensure!(
        saturation.is_finite() && (0.0..=1.0).contains(&saturation),
        "bounce saturation must be finite and between 0 and 1"
    );
    Ok(())
}

fn mip_zero(slot: &PrmSlot) -> Result<&[u8]> {
    ensure!(
        slot.format == PrmFormat::Rgba8UnormSrgb,
        "bounce colour slot must be sRGB RGBA8"
    );
    ensure!(
        slot.width > 0 && slot.height > 0 && slot.level_count > 0,
        "bounce colour slot must have a nonempty mip zero"
    );
    let bytes = usize::from(slot.width)
        .checked_mul(usize::from(slot.height))
        .and_then(|texels| texels.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("bounce colour mip zero size overflow"))?;
    slot.payload
        .get(..bytes)
        .ok_or_else(|| anyhow::anyhow!("bounce colour mip zero is truncated"))
}

/// One fixed-order pass, no temporary texel storage. Keep the raw mean transient:
/// only its sRGB snap determines both the estimator's albedo and the key.
fn mean_from_slots(
    diffuse: &PrmSlot,
    emissive: Option<&PrmSlot>,
    emissive_strength: f32,
    lut: &[f32; 256],
) -> Result<[f32; 3]> {
    let diffuse_texels = mip_zero(diffuse)?;
    let emissive_texels = emissive
        .map(|slot| {
            ensure!(
                slot.width == diffuse.width && slot.height == diffuse.height,
                "bounce diffuse and emissive dimensions must match"
            );
            mip_zero(slot)
        })
        .transpose()?;
    let mut full = [0.0_f64; 3];
    let mut non_glow = [0.0_f64; 3];
    let mut retained = 0_u64;
    for (index, texel) in diffuse_texels.chunks_exact(4).enumerate() {
        let glowing = emissive_strength > 0.0
            && emissive_texels.is_some_and(|bytes| {
                bytes[index * 4..index * 4 + 3]
                    .iter()
                    .any(|&channel| channel != 0)
            });
        for channel in 0..3 {
            let linear = f64::from(lut[usize::from(texel[channel])]);
            full[channel] += linear;
            if !glowing {
                non_glow[channel] += linear;
            }
        }
        retained += u64::from(!glowing);
    }
    let (sum, count) = if retained == 0 {
        (full, (diffuse_texels.len() / 4) as u64)
    } else {
        (non_glow, retained)
    };
    Ok(sum.map(|value| (value / count as f64) as f32))
}

/// Texture-indexed bounce values. Geometry names that only movers or fluids use
/// are available for materials but never affect a static-world SH key.
pub struct BounceMaterials {
    stats: Vec<MaterialStats>,
    saturation: f32,
}

impl BounceMaterials {
    pub fn new(stats: Vec<MaterialStats>, saturation: f32) -> Result<Self> {
        validate_saturation(saturation)?;
        Ok(Self { stats, saturation })
    }

    pub fn albedo(&self, index: u32) -> Vec3 {
        self.stats
            .get(index as usize)
            .copied()
            .unwrap_or_default()
            .albedo()
    }

    /// Absence keeps the estimator's original scalar grey multiply unchanged.
    pub fn resolved_albedo(&self, index: u32) -> Option<Vec3> {
        let record = self.stats.get(index as usize)?;
        record.snapped_mean().map(|_| record.albedo())
    }

    pub fn fold_key(&self, hasher: &mut blake3::Hasher, geometry: &GeometryResult) {
        hasher.update(b"bounce material means\0");
        hasher.update(&self.saturation.to_bits().to_le_bytes());
        let used: BTreeSet<u32> = geometry
            .geometry
            .faces
            .iter()
            .map(|face| face.texture_index)
            .collect();
        hasher.update(&(used.len() as u64).to_le_bytes());
        for index in used {
            hasher.update(&index.to_le_bytes());
            match self
                .stats
                .get(index as usize)
                .and_then(MaterialStats::snapped_mean)
            {
                Some(rgb) => {
                    hasher.update(&[1]);
                    hasher.update(&rgb);
                }
                None => {
                    hasher.update(&[0]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture_mips::build_srgb_to_linear_lut;
    use postretro_level_format::geometry::{FaceMeta, GeometrySection, NO_TEXTURE};
    use postretro_level_format::texture_names::TextureNamesSection;

    fn slot(width: u16, height: u16, texels: &[[u8; 4]]) -> PrmSlot {
        PrmSlot {
            format: PrmFormat::Rgba8UnormSrgb,
            width,
            height,
            level_count: 1,
            payload: texels.iter().flatten().copied().collect(),
        }
    }

    fn stats(
        diffuse: &PrmSlot,
        emissive: Option<&PrmSlot>,
        strength: f32,
        saturation: f32,
    ) -> MaterialStats {
        MaterialStats::from_slots(
            Some(diffuse),
            emissive,
            strength,
            saturation,
            &build_srgb_to_linear_lut(),
        )
        .unwrap()
    }

    fn near(actual: Vec3, expected: Vec3, epsilon: f32) {
        assert!(
            (actual - expected).abs().max_element() <= epsilon,
            "{actual:?} != {expected:?}"
        );
    }

    #[test]
    fn mean_uses_full_resolution_linear_texels_and_ignores_coarser_mips_and_alpha() {
        let mut diffuse = slot(2, 1, &[[0, 255, 0, 0], [255, 0, 0, 255]]);
        diffuse.level_count = 2;
        diffuse.payload.extend_from_slice(&[255, 255, 255, 255]);
        let lut = build_srgb_to_linear_lut();
        let raw = mean_from_slots(&diffuse, None, 0.0, &lut).unwrap();
        near(
            Vec3::from_array(raw),
            Vec3::new(0.5, 0.5, 0.0),
            f32::EPSILON,
        );
        assert!(
            (raw[0] - lut[128]).abs() > 0.2,
            "linear average differs from decoded sRGB average"
        );
        let result = stats(&diffuse, None, 0.0, 1.0);
        assert_eq!(result.snapped_mean(), Some([188, 188, 0]));
        assert_eq!(
            result.albedo().to_array().map(f32::to_bits),
            [lut[188], lut[188], lut[0]].map(f32::to_bits)
        );
    }

    #[test]
    fn odd_sized_mean_includes_every_source_texel_in_fixed_order() {
        let texels = [
            [17, 93, 251, 0],
            [255, 10, 22, 255],
            [0, 255, 31, 7],
            [81, 41, 103, 0],
            [128, 77, 66, 255],
            [191, 39, 27, 0],
            [4, 5, 6, 255],
            [199, 200, 201, 255],
            [92, 0, 150, 255],
            [26, 88, 97, 0],
            [71, 171, 11, 255],
            [65, 19, 143, 255],
            [254, 253, 252, 0],
            [31, 47, 63, 255],
            [0, 1, 2, 3],
        ];
        let diffuse = slot(5, 3, &texels);
        // Independent decode of the actual sRGB transfer function, in f64.
        let expected = std::array::from_fn(|channel| {
            (texels
                .iter()
                .map(|texel| {
                    let value = f64::from(texel[channel]) / 255.0;
                    if value <= 0.04045 {
                        value / 12.92
                    } else {
                        ((value + 0.055) / 1.055).powf(2.4)
                    }
                })
                .sum::<f64>()
                / texels.len() as f64) as f32
        });
        let raw = mean_from_slots(&diffuse, None, 0.0, &build_srgb_to_linear_lut()).unwrap();
        near(
            Vec3::from_array(raw),
            Vec3::from_array(expected),
            2.0 * f32::EPSILON,
        );
        assert_eq!(
            stats(&diffuse, None, 0.0, 1.0).snapped_mean(),
            Some(expected.map(linear_to_srgb_u8))
        );
    }

    #[test]
    fn glow_mask_uses_rgb_and_per_name_strength_with_full_average_when_all_glow() {
        let diffuse = slot(2, 1, &[[32, 64, 96, 0], [255, 255, 255, 255]]);
        let mask = slot(2, 1, &[[0, 0, 0, 255], [1, 0, 0, 0]]);
        let painted = stats(&diffuse, None, 0.0, 1.0);
        let neon = stats(&diffuse, Some(&mask), 4.0, 1.0);
        assert_eq!(neon.snapped_mean(), Some([32, 64, 96]));
        // The same sidecar under a zero-strength name keeps its bright strip.
        for strength in [0.0, 4.0, 0.0, 4.0] {
            let actual = stats(&diffuse, Some(&mask), strength, 1.0);
            assert_eq!(
                actual.snapped_mean(),
                if strength > 0.0 {
                    neon.snapped_mean()
                } else {
                    painted.snapped_mean()
                }
            );
        }
        let all_glow = slot(2, 1, &[[0, 1, 0, 0], [0, 0, 255, 255]]);
        assert_eq!(
            stats(&diffuse, Some(&all_glow), 4.0, 1.0).snapped_mean(),
            painted.snapped_mean()
        );
    }

    #[test]
    fn saturation_preserves_luminance_interpolates_colour_and_never_adds_energy() {
        assert!((0.0..=1.0).contains(&BOUNCE_SATURATION));
        let diffuse = slot(1, 1, &[[235, 47, 11, 255]]);
        let full = stats(&diffuse, None, 0.0, 1.0).albedo();
        let neutral = stats(&diffuse, None, 0.0, 0.0).albedo();
        let half = stats(&diffuse, None, 0.0, 0.5).albedo();
        let weights = Vec3::new(0.2126, 0.7152, 0.0722);
        near(neutral, Vec3::splat(neutral.x), f32::EPSILON);
        near(half, (full + neutral) * 0.5, f32::EPSILON);
        assert!(half.x - half.z > 0.0 && half.x - half.z < full.x - full.z);
        for colour in [neutral, half, full] {
            assert!((colour.dot(weights) - full.dot(weights)).abs() <= f32::EPSILON);
            assert!(colour.max_element() <= f32::from_bits(full.max_element().to_bits() + 1));
        }
        let grey = slot(1, 1, &[[128; 4]]);
        let expected = build_srgb_to_linear_lut()[128];
        let ulp = f32::from_bits(expected.to_bits() + 1) - expected;
        for saturation in [0.0, 0.5, 1.0] {
            near(
                stats(&grey, None, 0.0, saturation).albedo(),
                Vec3::splat(expected),
                ulp,
            );
        }
    }

    #[test]
    fn missing_diffuse_and_invalid_indices_keep_exact_unsaturated_grey() {
        let mut orphan = slot(1, 1, &[[255; 4]]);
        orphan.payload.clear();
        for saturation in [0.0, 0.5, 1.0] {
            let unresolved = MaterialStats::from_slots(
                None,
                Some(&orphan),
                4.0,
                saturation,
                &build_srgb_to_linear_lut(),
            )
            .unwrap();
            assert_eq!(unresolved.snapped_mean(), None);
            let table = BounceMaterials::new(vec![unresolved], saturation).unwrap();
            for index in [0, 1, NO_TEXTURE] {
                assert!(table.resolved_albedo(index).is_none());
                assert_eq!(
                    table.albedo(index).to_array().map(f32::to_bits),
                    [GREY_BOUNCE_ALBEDO.to_bits(); 3]
                );
            }
        }
    }

    #[test]
    fn invalid_saturation_and_malformed_colour_slots_fail_without_indexing() {
        let diffuse = slot(1, 1, &[[128; 4]]);
        let lut = build_srgb_to_linear_lut();
        for saturation in [-0.1, 1.1, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(
                MaterialStats::from_slots(Some(&diffuse), None, 0.0, saturation, &lut).is_err()
            );
            assert!(MaterialStats::from_slots(None, None, 0.0, saturation, &lut).is_err());
            assert!(BounceMaterials::new(Vec::new(), saturation).is_err());
        }
        let mut malformed = vec![diffuse.clone(); 5];
        malformed[0].format = PrmFormat::Rgba8Unorm;
        malformed[1].width = 0;
        malformed[2].height = 0;
        malformed[3].level_count = 0;
        malformed[4].payload.pop();
        for slot in &malformed {
            assert!(MaterialStats::from_slots(Some(slot), None, 0.0, 1.0, &lut).is_err());
            assert!(MaterialStats::from_slots(Some(&diffuse), Some(slot), 4.0, 1.0, &lut).is_err());
        }
        let mismatch = slot(2, 1, &[[0; 4], [0; 4]]);
        assert!(
            MaterialStats::from_slots(Some(&diffuse), Some(&mismatch), 4.0, 1.0, &lut).is_err()
        );
    }

    fn geometry(indices: &[u32]) -> GeometryResult {
        GeometryResult {
            geometry: GeometrySection {
                vertices: Vec::new(),
                indices: Vec::new(),
                faces: indices
                    .iter()
                    .map(|&texture_index| FaceMeta {
                        leaf_index: 0,
                        texture_index,
                    })
                    .collect(),
            },
            texture_names: TextureNamesSection {
                names: vec![
                    "world".into(),
                    "world alias".into(),
                    "mover".into(),
                    "fluid".into(),
                ],
            },
            face_index_ranges: Vec::new(),
        }
    }

    fn key(table: Vec<MaterialStats>, saturation: f32, geometry: &GeometryResult) -> blake3::Hash {
        let mut hash = blake3::Hasher::new();
        BounceMaterials::new(table, saturation)
            .unwrap()
            .fold_key(&mut hash, geometry);
        hash.finalize()
    }

    #[test]
    fn keys_fold_every_used_index_once_and_ignore_mover_fluid_means_and_name_order() {
        let red = stats(&slot(1, 1, &[[255, 0, 0, 255]]), None, 0.0, 1.0);
        let blue = stats(&slot(1, 1, &[[0, 0, 255, 255]]), None, 0.0, 1.0);
        let table = vec![red; 4];
        let base = key(table.clone(), 1.0, &geometry(&[0, 1, 0]));
        assert_eq!(base, key(table.clone(), 1.0, &geometry(&[1, 0])));
        for index in 0..4 {
            let mut edited = table.clone();
            edited[index] = blue;
            let changed = key(edited, 1.0, &geometry(&[0, 1]));
            assert_eq!(base != changed, index < 2, "texture index {index}");
        }
        assert_ne!(base, key(table.clone(), 0.5, &geometry(&[0, 1])));
        assert_ne!(
            base,
            key(vec![MaterialStats::default(); 4], 1.0, &geometry(&[0, 1]))
        );
        let empty = geometry(&[]);
        assert_eq!(
            key(table.clone(), 1.0, &empty),
            key(vec![blue; 4], 1.0, &empty)
        );
        assert_ne!(key(table.clone(), 1.0, &empty), key(table, 0.0, &empty));
        // A texture's derived value is independent of the other names in a map.
        for neighbours in [[red, blue], [red, MaterialStats::default()]] {
            let materials = BounceMaterials::new(neighbours.to_vec(), 1.0).unwrap();
            assert_eq!(
                materials
                    .resolved_albedo(0)
                    .unwrap()
                    .to_array()
                    .map(f32::to_bits),
                red.albedo().to_array().map(f32::to_bits)
            );
            assert_eq!(
                materials.albedo(0).to_array().map(f32::to_bits),
                red.albedo().to_array().map(f32::to_bits)
            );
        }
    }
}
