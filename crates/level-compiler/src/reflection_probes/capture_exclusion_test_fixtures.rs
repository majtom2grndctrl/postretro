// Capture-exclusion fixtures: a compiled pool room with a pillar, its materials, light and capture scene.
// See: context/lib/testing_guide.md §4

use std::collections::HashMap;
use std::path::Path;

use glam::Vec3;
use postretro_level_format::alpha_lights::AlphaLightsSection;
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_RGBA16F, LightmapMode, LightmapSection};
use postretro_level_format::prm::{
    PrmFile, PrmFormat, PrmHeader, PrmSlot, PrmSlots, STAGE_VERSION, cache_filename_for_key,
};
use postretro_level_format::sh_reconstruct::Level;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::capture_exclusion::{CaptureBrushes, ProbeExclusion};
use super::hit::SceneTracer;
use super::lightmap_sample::LightmapSampler;
use super::materials::SurfaceMaterials;
use super::placement::PlacedProbe;
use super::placement_test_fixtures::{PlacementFixture, engine, fluid, textured_box_brush};
use super::reflection_probes_test_fixtures::dense_sh_volume;
use super::sdf_lights::SdfLights;
use super::sh_sample::{PRODUCTION_PROBE_OCCLUSION, ShVolumeSampler};
use super::shading::CaptureScene;

pub(super) const WALL_TEXTURE: &str = "test/wall";
pub(super) const CEILING_TEXTURE: &str = "test/ceiling";
pub(super) const FLOOR_TEXTURE: &str = "test/floor";
pub(super) const PILLAR_TEXTURE: &str = "test/pillar";
pub(super) const TRIM_TEXTURE: &str = "test/trim";

/// The room's interior in map units: 256 u square, 128 u tall (6.5 × 3.25 m).
pub(super) const ROOM_MIN: [f64; 3] = [-128.0, -128.0, 0.0];
pub(super) const ROOM_MAX: [f64; 3] = [128.0, 128.0, 128.0];
/// The water's top, in map units: 16 u over the room floor.
pub(super) const WATER_TOP: f64 = 16.0;
/// A pillar standing in the water from the floor to the ceiling, 16 u
/// square, about a metre from the capture point toward +X (engine −Z).
pub(super) const PILLAR_MIN: [f64; 3] = [40.0, -8.0, 0.0];
pub(super) const PILLAR_MAX: [f64; 3] = [56.0, 8.0, 128.0];
/// The wall behind the pillar, seen from the capture point: map x = 128.
pub(super) const BACK_WALL_X: f64 = 128.0;

/// Wall, ceiling and floor slabs sealing the room, 16 u thick.
pub(super) fn textured_room() -> Vec<String> {
    let ([x0, y0, z0], [x1, y1, z1]) = (ROOM_MIN, ROOM_MAX);
    let w = 16.0;
    vec![
        textured_box_brush(
            [x0 - w, y0 - w, z0 - w],
            [x1 + w, y1 + w, z0],
            FLOOR_TEXTURE,
        ),
        textured_box_brush(
            [x0 - w, y0 - w, z1],
            [x1 + w, y1 + w, z1 + w],
            CEILING_TEXTURE,
        ),
        textured_box_brush([x0 - w, y0 - w, z0], [x0, y1 + w, z1], WALL_TEXTURE),
        textured_box_brush([x1, y0 - w, z0], [x1 + w, y1 + w, z1], WALL_TEXTURE),
        textured_box_brush([x0, y0 - w, z0], [x1, y0, z1], WALL_TEXTURE),
        textured_box_brush([x0, y1, z0], [x1, y1 + w, z1], WALL_TEXTURE),
    ]
}

/// The pool: water 16 u deep over most of the room floor, its far edge
/// flush with the pillar's back, so the pillar stands in it and the water
/// stays one derived probe (water behind the pillar would fit its own box).
pub(super) fn pool_entity() -> String {
    fluid(
        "water",
        &[textured_box_brush(
            [-96.0, -96.0, 0.0],
            [56.0, 96.0, WATER_TOP],
            "test/water",
        )],
    )
}

/// The pillar brush, as `PILLAR_MIN`..`PILLAR_MAX` with its top at `top`.
pub(super) fn pillar(top: f64) -> String {
    textured_box_brush(
        PILLAR_MIN,
        [PILLAR_MAX[0], PILLAR_MAX[1], top],
        PILLAR_TEXTURE,
    )
}

/// One `.prm` diffuse colour per mip level, sRGB.
type MipColours = [[u8; 4]; 3];

/// Every fixture texture's mips. The wall's levels differ, so a footprint
/// measured from the wrong point reads the wrong colour.
pub(super) fn fixture_materials() -> Vec<(&'static str, MipColours)> {
    vec![
        (
            WALL_TEXTURE,
            [[0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 0, 255]],
        ),
        (CEILING_TEXTURE, [[128, 128, 128, 255]; 3]),
        (FLOOR_TEXTURE, [[64, 64, 64, 255]; 3]),
        (PILLAR_TEXTURE, [[255, 0, 0, 255]; 3]),
        (TRIM_TEXTURE, [[255, 0, 255, 255]; 3]),
    ]
}

/// A compiled level and every capture input beyond it: materials, an empty
/// lightmap (no face carries one, so no lightmap shadow exists), uniform SH
/// and the runtime's sdf lights.
pub(super) struct CaptureFixture {
    pub world: PlacementFixture,
    pub keys: HashMap<String, [u8; 32]>,
    pub prm_dir: tempfile::TempDir,
    pub lightmap: LightmapSection,
    pub sh: OctahedralShVolumeSection,
    pub alpha_lights: AlphaLightsSection,
    pub chunk_list: ChunkLightListSection,
}

impl CaptureFixture {
    /// Worldspawn holds `world`; `entities` follow as map entities.
    pub(super) fn compile(world: &[String], entities: &[String]) -> Self {
        let world = PlacementFixture::compile(world, entities);
        let prm_dir = tempfile::tempdir().unwrap();
        let mut keys = HashMap::new();
        for (index, (name, mips)) in fixture_materials().into_iter().enumerate() {
            let key = [index as u8 + 1; 32];
            write_mip_prm(prm_dir.path(), key, mips);
            keys.insert(name.to_string(), key);
        }
        // Uniform base SH 1 over the room and past it.
        let mut dense = dense_sh_volume([9, 6, 9], |_, _, _| [1.0; 3]);
        dense.grid_origin = [-4.0, -1.0, -4.0];
        let sh = crate::sh_density::pack_indirect_section(dense, Some(Level::L0))
            .unwrap()
            .0;
        Self {
            world,
            keys,
            prm_dir,
            lightmap: LightmapSection {
                direction_texel_scale: 1,
                irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
                mode: LightmapMode::Shadowed,
                blocks: Vec::new(),
            },
            sh,
            alpha_lights: AlphaLightsSection::default(),
            chunk_list: ChunkLightListSection::placeholder(),
        }
    }

    /// The room, its pool and a pillar topped at `pillar_top` (map units).
    pub(super) fn pool_room(pillar_top: Option<f64>) -> Self {
        let mut world = textured_room();
        world.extend(pillar_top.map(pillar));
        Self::compile(&world, &[pool_entity()])
    }

    /// The index in `MapData::brush_volumes` of the brush whose hull holds
    /// the map-unit point `q`.
    pub(super) fn brush_at(&self, q: [f64; 3]) -> u32 {
        let p = engine(q);
        self.world
            .brushes
            .iter()
            .position(|brush| {
                (0..3).all(|axis| brush.aabb.min[axis] < p[axis] && p[axis] < brush.aabb.max[axis])
            })
            .expect("a brush holds the point") as u32
    }

    /// The brush of the face `face`.
    pub(super) fn face_brush(&self, face: u32) -> Option<u32> {
        self.world.face_brushes[face as usize]
    }

    pub(super) fn brushes(&self) -> CaptureBrushes<'_> {
        CaptureBrushes {
            volumes: &self.world.brushes,
            face_brushes: &self.world.face_brushes,
        }
    }

    /// Run `f` over the capture scene and the placed probes.
    pub(super) fn with_scene<R>(
        &self,
        f: impl FnOnce(&CaptureScene<'_>, &[PlacedProbe]) -> R,
    ) -> R {
        let probes = self.world.place().expect("placement");
        let scene = CaptureScene {
            tracer: SceneTracer::new(
                &self.world.bvh,
                &self.world.primitives,
                &self.world.geometry,
            ),
            brushes: self.brushes(),
            materials: SurfaceMaterials::new(
                &self.world.geometry.texture_names.names,
                &self.keys,
                self.prm_dir.path(),
            ),
            lightmap: LightmapSampler::new(&self.lightmap),
            sh: ShVolumeSampler::new(&self.sh, PRODUCTION_PROBE_OCCLUSION).unwrap(),
            animated_direct: None,
            deltas: None,
            sdf: SdfLights::new(&self.alpha_lights, &self.chunk_list),
            max_distance: self.world.bounds().diagonal() as f32 + 1.0,
        };
        f(&scene, &probes)
    }
}

/// `probe`'s exclusion of `excluded` in `scene`.
pub(super) fn exclusion<'a>(
    scene: &CaptureScene<'a>,
    probe: &PlacedProbe,
    excluded: &[u32],
) -> ProbeExclusion<'a> {
    ProbeExclusion::new(&scene.tracer, scene.brushes, excluded, probe)
}

/// The unit direction from `from` to the map-unit point `q`, and the
/// distance to it.
pub(super) fn toward(from: Vec3, q: [f64; 3]) -> (Vec3, f32) {
    let offset = engine(q) - from.as_dvec3();
    (offset.normalize().as_vec3(), offset.length() as f32)
}

/// A 4×4 diffuse `.prm` whose three mip levels are `mips`, at `key`.
fn write_mip_prm(dir: &Path, key: [u8; 32], mips: MipColours) {
    let mut payload = Vec::new();
    for (texels, rgba) in [16, 4, 1].into_iter().zip(mips) {
        for _ in 0..texels {
            payload.extend_from_slice(&rgba);
        }
    }
    let prm = PrmFile {
        header: PrmHeader {
            stage_version: STAGE_VERSION,
            slot_mask: PrmSlots::DIFFUSE,
            bundle_hash: [0; 32],
            total_body_bytes: 0,
            layer_count: 1,
        },
        slots: [
            Some(PrmSlot {
                format: PrmFormat::Rgba8UnormSrgb,
                width: 4,
                height: 4,
                level_count: 3,
                payload,
            }),
            None,
            None,
            None,
        ],
    };
    std::fs::write(
        dir.join(format!("{}.prm", cache_filename_for_key(&key))),
        prm.to_bytes().unwrap(),
    )
    .unwrap();
}
