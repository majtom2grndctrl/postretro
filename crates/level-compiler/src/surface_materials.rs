// Shared bake-side surface materials: albedo and emissive read back from the baked `.prm` sidecars.
// See: context/lib/build_pipeline.md §Baked texture mips · resource_management.md §3, §4.5

pub mod stats;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use glam::Vec3;
use postretro_level_format::prm::{PrmFile, PrmFormat, PrmSlot, cache_filename_for_key};
use postretro_render_data::material::derive_material;

use crate::texture_mips::build_srgb_to_linear_lut;

/// Edge and checker square of the renderer's missing-diffuse placeholder
/// (`loaded_texture.rs`): magenta and black, sRGB, one mip. World UVs
/// normalize by its size when a material has no diffuse.
const PLACEHOLDER_EDGE: u32 = 64;
const PLACEHOLDER_SQUARE: u32 = 8;

/// The base colour and emissive term at a hit, linear.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MaterialSample {
    pub albedo: Vec3,
    /// Emissive texel times the prefix multiplier; zero for a non-emissive
    /// prefix, which the forward pass never samples.
    pub emissive: Vec3,
}

/// World materials by texture index for footprint sampling. Reflection capture
/// is the only footprint reader today (SH bounce reads [`stats`] instead), so
/// each material loads from its sidecar the first time a capture ray reaches
/// it, and read failures log under the reflection stage's tag. The sidecar
/// bytes are content-addressed by the key, so whichever thread loads one,
/// every reader sees the same texels.
pub(crate) struct SurfaceMaterials<'a> {
    names: &'a [String],
    keys: &'a HashMap<String, [u8; 32]>,
    prm_root: &'a Path,
    loaded: Vec<OnceLock<SurfaceMaterial>>,
    srgb_to_linear: [f32; 256],
}

struct SurfaceMaterial {
    diffuse: Option<MipChain>,
    emissive: Option<MipChain>,
    emissive_strength: f32,
}

/// One sRGB slot's mip chain, level 0 first, as the `.prm` stores it.
struct MipChain {
    width: u32,
    height: u32,
    levels: u8,
    texels: Vec<u8>,
}

impl MipChain {
    fn from_slot(slot: PrmSlot) -> Option<Self> {
        (slot.format == PrmFormat::Rgba8UnormSrgb && slot.level_count > 0).then(|| Self {
            width: u32::from(slot.width),
            height: u32::from(slot.height),
            levels: slot.level_count,
            texels: slot.payload,
        })
    }

    /// The nearest texel of level `lod` at normalized, wrapped `uv`.
    fn texel(&self, uv: [f32; 2], lod: u8) -> [u8; 4] {
        let lod = lod.min(self.levels - 1);
        let mut offset = 0usize;
        for level in 0..lod {
            offset += level_extent(self.width, level) * level_extent(self.height, level) * 4;
        }
        let w = level_extent(self.width, lod);
        let h = level_extent(self.height, lod);
        let x = wrap_texel(uv[0], w);
        let y = wrap_texel(uv[1], h);
        let at = offset + (y * w + x) * 4;
        self.texels[at..at + 4].try_into().expect("four bytes")
    }
}

/// The mip a sample spanning `texels` level-0 texels reads.
fn footprint_lod(texels: f32) -> u8 {
    texels.max(1.0).log2().round().clamp(0.0, 15.0) as u8
}

fn level_extent(base: u32, level: u8) -> usize {
    (base >> level).max(1) as usize
}

fn wrap_texel(coord: f32, extent: usize) -> usize {
    let wrapped = coord - coord.floor();
    ((wrapped * extent as f32) as usize).min(extent - 1)
}

impl<'a> SurfaceMaterials<'a> {
    pub(crate) fn new(
        names: &'a [String],
        keys: &'a HashMap<String, [u8; 32]>,
        prm_root: &'a Path,
    ) -> Self {
        Self {
            names,
            keys,
            prm_root,
            loaded: (0..names.len()).map(|_| OnceLock::new()).collect(),
            srgb_to_linear: build_srgb_to_linear_lut(),
        }
    }

    /// Albedo and emissive at raw-texel `uv` on texture `texture_index`, read
    /// at the mip a sample of `footprint_texels` raw texels selects, as the
    /// forward pass's minified sample would. Raw texels are the diffuse's
    /// level-0 texels (the placeholder's without a diffuse), which normalize
    /// the UV both textures share; the emissive's own level comes from that
    /// footprint in its own texels, as hardware derivatives select it.
    pub(crate) fn sample(
        &self,
        texture_index: u32,
        uv: [f32; 2],
        footprint_texels: f32,
    ) -> MaterialSample {
        let material = self
            .loaded
            .get(texture_index as usize)
            .map(|slot| slot.get_or_init(|| self.load(texture_index as usize)));
        let Some(material) = material else {
            // `NO_TEXTURE` or an index past the table binds the placeholders.
            return MaterialSample {
                albedo: self.placeholder(uv),
                emissive: Vec3::ZERO,
            };
        };
        let (albedo, raw_extent) = match &material.diffuse {
            Some(chain) => {
                let extent = [chain.width as f32, chain.height as f32];
                let normalized = [uv[0] / extent[0], uv[1] / extent[1]];
                let lod = footprint_lod(footprint_texels);
                (self.linear(chain.texel(normalized, lod)), extent)
            }
            None => (self.placeholder(uv), [PLACEHOLDER_EDGE as f32; 2]),
        };
        let emissive = match (&material.emissive, material.emissive_strength > 0.0) {
            (Some(chain), true) => {
                let normalized = [uv[0] / raw_extent[0], uv[1] / raw_extent[1]];
                // The footprint in emissive texels, along its coarser axis.
                let scale =
                    (chain.width as f32 / raw_extent[0]).max(chain.height as f32 / raw_extent[1]);
                let lod = footprint_lod(footprint_texels * scale);
                self.linear(chain.texel(normalized, lod)) * material.emissive_strength
            }
            _ => Vec3::ZERO,
        };
        MaterialSample { albedo, emissive }
    }

    fn linear(&self, texel: [u8; 4]) -> Vec3 {
        Vec3::new(
            self.srgb_to_linear[usize::from(texel[0])],
            self.srgb_to_linear[usize::from(texel[1])],
            self.srgb_to_linear[usize::from(texel[2])],
        )
    }

    /// The missing-diffuse checkerboard: one mip, so no footprint averaging.
    fn placeholder(&self, uv: [f32; 2]) -> Vec3 {
        let edge = PLACEHOLDER_EDGE as usize;
        let x = wrap_texel(uv[0] / PLACEHOLDER_EDGE as f32, edge) as u32;
        let y = wrap_texel(uv[1] / PLACEHOLDER_EDGE as f32, edge) as u32;
        if (x / PLACEHOLDER_SQUARE + y / PLACEHOLDER_SQUARE).is_multiple_of(2) {
            Vec3::new(1.0, 0.0, 1.0)
        } else {
            Vec3::ZERO
        }
    }

    fn load(&self, index: usize) -> SurfaceMaterial {
        let name = &self.names[index];
        let mut warned = std::collections::HashSet::new();
        let emissive_strength = derive_material(name, &mut warned).emissive_strength();
        let (diffuse, emissive) = match self.keys.get(name) {
            Some(key) if *key != [0u8; 32] => self.read_slots(name, key),
            _ => (None, None),
        };
        SurfaceMaterial {
            diffuse,
            emissive,
            emissive_strength,
        }
    }

    fn read_slots(&self, name: &str, key: &[u8; 32]) -> (Option<MipChain>, Option<MipChain>) {
        let path = self.sidecar_path(key);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!(
                    "[ReflectionProbes] texture '{name}': cannot read {}: {error}; \
                     reflections show its placeholder",
                    path.display()
                );
                return (None, None);
            }
        };
        let (header, slots) = PrmFile::from_bytes_partial(&bytes);
        if header.is_err() {
            log::warn!(
                "[ReflectionProbes] texture '{name}': {} is not a readable .prm; \
                 reflections show its placeholder",
                path.display()
            );
            return (None, None);
        }
        let [diffuse, _, _, emissive] = slots;
        (
            diffuse.ok().and_then(MipChain::from_slot),
            emissive.ok().and_then(MipChain::from_slot),
        )
    }

    fn sidecar_path(&self, key: &[u8; 32]) -> PathBuf {
        self.prm_root
            .join(format!("{}.prm", cache_filename_for_key(key)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postretro_level_format::prm::{PrmHeader, PrmSlots, STAGE_VERSION, expected_level_count};

    /// A square `edge`² sRGB chain whose level 0 is `fine` everywhere and
    /// whose coarser levels are `rest`.
    fn chain(edge: u16, fine: [u8; 4], rest: [u8; 4]) -> PrmSlot {
        let level0 = usize::from(edge) * usize::from(edge);
        let mut coarser = 0;
        let mut extent = usize::from(edge);
        while extent > 1 {
            extent /= 2;
            coarser += extent * extent;
        }
        let mut payload = Vec::new();
        for _ in 0..level0 {
            payload.extend_from_slice(&fine);
        }
        for _ in 0..coarser {
            payload.extend_from_slice(&rest);
        }
        PrmSlot {
            format: PrmFormat::Rgba8UnormSrgb,
            width: edge,
            height: edge,
            level_count: expected_level_count(edge, edge),
            payload,
        }
    }

    /// A 4×4 diffuse whose level 0 is `level0` everywhere and whose 2×2 and
    /// 1×1 levels are `coarse`, plus an optional uniform 4×4 emissive.
    fn write_prm(
        dir: &Path,
        key: [u8; 32],
        level0: [u8; 4],
        coarse: [u8; 4],
        emissive: Option<[u8; 4]>,
    ) {
        write_slots(
            dir,
            key,
            chain(4, level0, coarse),
            emissive.map(|e| chain(4, e, e)),
        );
    }

    fn write_slots(dir: &Path, key: [u8; 32], diffuse: PrmSlot, emissive: Option<PrmSlot>) {
        let mut mask = PrmSlots::DIFFUSE;
        if emissive.is_some() {
            mask |= PrmSlots::EMISSIVE;
        }
        let prm = PrmFile {
            header: PrmHeader {
                stage_version: STAGE_VERSION,
                slot_mask: mask,
                bundle_hash: [0; 32],
                total_body_bytes: 0,
                layer_count: 1,
            },
            slots: [Some(diffuse), None, None, emissive],
        };
        std::fs::write(
            dir.join(format!("{}.prm", cache_filename_for_key(&key))),
            prm.to_bytes().unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn materials_read_linear_albedo_at_the_footprint_mip() {
        let dir = tempfile::tempdir().unwrap();
        write_prm(
            dir.path(),
            [1; 32],
            [255, 0, 0, 255],
            [0, 0, 255, 255],
            None,
        );
        let names = vec!["concrete_floor".to_string()];
        let keys = HashMap::from([(names[0].clone(), [1u8; 32])]);
        let materials = SurfaceMaterials::new(&names, &keys, dir.path());

        let near = materials.sample(0, [1.5, 2.5], 0.5);
        assert_eq!(near.albedo, Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(near.emissive, Vec3::ZERO);
        // A sample spanning two level-0 texels reads level 1.
        let far = materials.sample(0, [1.5, 2.5], 2.0);
        assert_eq!(far.albedo, Vec3::new(0.0, 0.0, 1.0));
        // UVs wrap, as the repeat sampler does.
        assert_eq!(materials.sample(0, [-2.5, 9.0], 0.5).albedo, near.albedo);
    }

    #[test]
    fn materials_scale_emissive_by_the_prefix_multiplier_and_skip_non_emissive_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let half = 188; // sRGB 188 ≈ linear 0.5
        write_prm(
            dir.path(),
            [2; 32],
            [255; 4],
            [255; 4],
            Some([half, half, half, 255]),
        );
        let names = vec!["neon_sign".to_string(), "metal_sign".to_string()];
        let keys = HashMap::from([(names[0].clone(), [2u8; 32]), (names[1].clone(), [2u8; 32])]);
        let materials = SurfaceMaterials::new(&names, &keys, dir.path());

        let neon = materials.sample(0, [0.5, 0.5], 1.0);
        assert!(
            (neon.emissive.x - 0.5 * 4.0).abs() < 0.02,
            "{}",
            neon.emissive
        );
        assert_eq!(materials.sample(1, [0.5, 0.5], 1.0).emissive, Vec3::ZERO);
    }

    // The emissive texture samples with its own derivatives: the UV both
    // textures share, normalized by the diffuse, spans more emissive texels
    // when the emissive is larger, so it minifies sooner.
    #[test]
    fn emissive_reads_the_mip_its_own_texel_footprint_selects() {
        let dir = tempfile::tempdir().unwrap();
        let (fine, coarse) = ([255, 255, 255, 255], [0, 0, 0, 255]);
        // An 8×8 emissive over a 4×4 diffuse: one diffuse texel spans two
        // emissive texels.
        write_slots(
            dir.path(),
            [3; 32],
            chain(4, fine, coarse),
            Some(chain(8, fine, coarse)),
        );
        let names = vec!["neon_tube".to_string()];
        let keys = HashMap::from([(names[0].clone(), [3u8; 32])]);
        let materials = SurfaceMaterials::new(&names, &keys, dir.path());

        // One diffuse texel: diffuse level 0, emissive level 1.
        let sample = materials.sample(0, [1.5, 2.5], 1.0);
        assert_eq!(sample.albedo, Vec3::ONE);
        assert_eq!(sample.emissive, Vec3::ZERO, "the emissive's level 1");
        // Half a diffuse texel is one emissive texel: both at level 0.
        let sample = materials.sample(0, [1.5, 2.5], 0.5);
        assert!(sample.emissive.x > 1.0, "{}", sample.emissive);

        // A 2×2 emissive over the same diffuse: two diffuse texels span one
        // emissive texel, its level 0, while the diffuse reads level 1.
        write_slots(
            dir.path(),
            [4; 32],
            chain(4, fine, coarse),
            Some(chain(2, fine, coarse)),
        );
        let keys = HashMap::from([(names[0].clone(), [4u8; 32])]);
        let materials = SurfaceMaterials::new(&names, &keys, dir.path());
        let sample = materials.sample(0, [1.5, 2.5], 2.0);
        assert_eq!(sample.albedo, Vec3::ZERO, "the diffuse's level 1");
        assert!(sample.emissive.x > 1.0, "{}", sample.emissive);
    }

    #[test]
    fn materials_without_a_sidecar_read_the_renderer_placeholder() {
        let names = vec!["missing".to_string()];
        let keys = HashMap::from([(names[0].clone(), [0u8; 32])]);
        let materials = SurfaceMaterials::new(&names, &keys, Path::new("."));
        assert_eq!(
            materials.sample(0, [0.5, 0.5], 1.0).albedo,
            Vec3::new(1.0, 0.0, 1.0)
        );
        assert_eq!(materials.sample(0, [8.5, 0.5], 1.0).albedo, Vec3::ZERO);
        assert_eq!(
            materials
                .sample(
                    postretro_level_format::geometry::NO_TEXTURE,
                    [0.5, 0.5],
                    1.0
                )
                .albedo,
            Vec3::new(1.0, 0.0, 1.0)
        );
    }

    /// The capture's per-prefix emissive multiplier is the renderer's: both
    /// read `Material::emissive_strength` through `derive_material`.
    #[test]
    fn emissive_multiplier_matches_the_runtime_for_every_material_prefix() {
        use postretro_render_data::material::{MATERIAL_PREFIXES, Material};
        let names: Vec<String> = MATERIAL_PREFIXES
            .iter()
            .map(|(prefix, _)| format!("{prefix}_panel"))
            .chain(std::iter::once("unknownprefix_panel".to_string()))
            .collect();
        let keys = HashMap::new();
        let materials = SurfaceMaterials::new(&names, &keys, Path::new("."));
        for (index, name) in names.iter().enumerate() {
            let expected = MATERIAL_PREFIXES
                .iter()
                .find(|(prefix, _)| name.starts_with(&format!("{prefix}_")))
                .map_or(Material::Default, |(_, material)| *material)
                .emissive_strength();
            let loaded = materials.loaded[index].get_or_init(|| materials.load(index));
            assert_eq!(loaded.emissive_strength, expected, "{name}");
        }
        assert_eq!(
            materials.loaded[3].get().unwrap().emissive_strength,
            4.0,
            "neon is the emissive prefix"
        );
    }
}
