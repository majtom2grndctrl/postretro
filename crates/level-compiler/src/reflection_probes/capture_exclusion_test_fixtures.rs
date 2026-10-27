// Capture-exclusion fixtures: a compiled pool room with a pillar, its materials, lightmap, lights and capture scene.
// See: context/lib/testing_guide.md §4

use std::collections::HashMap;
use std::path::Path;

use glam::Vec3;
use postretro_level_format::alpha_lights::AlphaLightsSection;
use postretro_level_format::animated_light_weight_maps::{
    AnimatedBlock, AnimatedLightWeightMapsSection, ChunkAtlasRect, TexelLight, TexelLightEntry,
};
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::lightmap::{
    IRRADIANCE_FORMAT_RGBA16F, LightmapBlock, LightmapMode, LightmapSection, f32_to_f16_bits,
};
use postretro_level_format::prm::{
    PrmFile, PrmFormat, PrmHeader, PrmSlot, PrmSlots, STAGE_VERSION, cache_filename_for_key,
};
use postretro_level_format::sh_reconstruct::Level;
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::ReflectionProbeInputs;
use super::animated_direct::AnimatedDirectSampler;
use super::capture_exclusion::{BrushFaces, CaptureBrushes, ProbeExclusion};
use super::hit::SceneTracer;
use super::lightmap_sample::LightmapSampler;
use super::placement::PlacedProbe;
use super::placement_test_fixtures::{PlacementFixture, engine, fluid, textured_box_brush};
use super::reflection_probes_test_fixtures::dense_sh_volume;
use super::sdf_lights::SdfLights;
use super::sh_sample::{PRODUCTION_PROBE_OCCLUSION, ShVolumeSampler};
use super::shading::CaptureScene;
use crate::surface_materials::SurfaceMaterials;

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

/// The ceiling slab's thickness in map units: deep enough to sink a brush
/// half a metre into it.
pub(super) const CEILING_DEPTH: f64 = 32.0;

/// Wall and floor slabs 16 u thick and a [`CEILING_DEPTH`] ceiling slab,
/// sealing the room.
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
            [x1 + w, y1 + w, z1 + CEILING_DEPTH],
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
    pool_entity_to(PILLAR_MAX[0])
}

/// The pool with its far edge at map x = `far`.
pub(super) fn pool_entity_to(far: f64) -> String {
    pool_between(-96.0, far)
}

/// A pool spanning map x `near`..`far`, its own fluid entity.
pub(super) fn pool_between(near: f64, far: f64) -> String {
    fluid(
        "water",
        &[textured_box_brush(
            [near, -96.0, 0.0],
            [far, 96.0, WATER_TOP],
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

/// A 4×4 sRGB diffuse's three mip levels, level 0 first, row-major RGBA.
type MipPayload = Vec<u8>;

/// Each level one colour.
fn uniform_mips(levels: [[u8; 4]; 3]) -> MipPayload {
    [16, 4, 1]
        .into_iter()
        .zip(levels)
        .flat_map(|(texels, rgba)| rgba.repeat(texels))
        .collect()
}

/// Every texel of levels 0 and 1 its own colour, so a wrong UV reads a
/// wrong albedo at either level.
fn varied_mips() -> MipPayload {
    let mut payload = Vec::new();
    for (texels, step) in [(16u8, 12u8), (4, 60)] {
        for i in 0..texels {
            payload.extend_from_slice(&[40 + i * step, 200 - i * step, 90 + (i % 2) * 80, 255]);
        }
    }
    payload.extend_from_slice(&[128, 128, 128, 255]);
    payload
}

/// Every fixture texture's mips. The wall's levels differ, so a footprint
/// measured from the wrong point reads the wrong colour; the ceiling's
/// texels differ, so a wrong UV does.
pub(super) fn fixture_materials() -> Vec<(&'static str, MipPayload)> {
    vec![
        (
            WALL_TEXTURE,
            uniform_mips([[0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 0, 255]]),
        ),
        (CEILING_TEXTURE, varied_mips()),
        (FLOOR_TEXTURE, uniform_mips([[64, 64, 64, 255]; 3])),
        (PILLAR_TEXTURE, uniform_mips([[255, 0, 0, 255]; 3])),
        (TRIM_TEXTURE, uniform_mips([[255, 0, 255, 255]; 3])),
    ]
}

/// A compiled level and every capture input beyond it: materials, an empty
/// lightmap (no face carries one, so no lightmap shadow exists), uniform SH
/// and the runtime's sdf lights.
pub(super) struct CaptureFixture {
    pub world: PlacementFixture,
    /// `world.face_brushes` inverted.
    pub brush_faces: BrushFaces,
    pub keys: HashMap<String, [u8; 32]>,
    pub prm_dir: tempfile::TempDir,
    pub lightmap: LightmapSection,
    pub sh: OctahedralShVolumeSection,
    pub alpha_lights: AlphaLightsSection,
    pub chunk_list: ChunkLightListSection,
    pub weight_maps: Option<AnimatedLightWeightMapsSection>,
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
            brush_faces: BrushFaces::new(&world.face_brushes, world.brushes.len()),
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
            weight_maps: None,
        }
    }

    /// Base SH varying across the room: `value(x, y, z)` at each probe of a
    /// 1 m grid from (−4, −1, −4) m.
    pub(super) fn set_sh(&mut self, value: impl Fn(u32, u32, u32) -> [f32; 3]) {
        let mut dense = dense_sh_volume([9, 6, 9], value);
        dense.grid_origin = [-4.0, -1.0, -4.0];
        let descriptors = std::mem::take(&mut self.sh.animation_descriptors);
        self.sh = crate::sh_density::pack_indirect_section(dense, Some(Level::L0))
            .unwrap()
            .0;
        self.sh.animation_descriptors = descriptors;
    }

    /// Give every face of `brush` lightmap block `block` (1-based, pushed
    /// next), 16×16 RGBA16F texels of `irradiance(u, v)`, its UV the
    /// face's engine (x, z) across the room's floor plan: one block over
    /// every fragment of a horizontal surface, so its texels vary across
    /// faces as one lightmap would.
    pub(super) fn lightmap_brush(&mut self, brush: u32, irradiance: impl Fn(f32, f32) -> f32) {
        let block = self.lightmap.blocks.len() as u16 + 1;
        let (lo, hi) = (engine(ROOM_MIN), engine(ROOM_MAX));
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        let geometry = &mut self.world.geometry;
        for (face, range) in geometry.face_index_ranges.iter().enumerate() {
            if self.world.face_brushes[face] != Some(brush) {
                continue;
            }
            let start = range.index_offset as usize;
            for &index in &geometry.geometry.indices[start..start + range.index_count as usize] {
                let vertex = &mut geometry.geometry.vertices[index as usize];
                let p = Vec3::from(vertex.position).as_dvec3();
                let u = ((p.x - lo.x) / (hi.x - lo.x)).clamp(0.0, 1.0);
                let v = ((p.z - lo.z) / (hi.z - lo.z)).clamp(0.0, 1.0);
                vertex.lightmap_uv = [(u * 65535.0).round() as u16, (v * 65535.0).round() as u16];
                vertex.lightmap_block = block;
            }
        }
        let side = 16u16;
        let mut texels = Vec::new();
        for y in 0..side {
            for x in 0..side {
                let value = irradiance(
                    (f32::from(x) + 0.5) / f32::from(side),
                    (f32::from(y) + 0.5) / f32::from(side),
                );
                for channel in [value, value, value, 1.0] {
                    texels.extend(f32_to_f16_bits(channel).to_le_bytes());
                }
            }
        }
        self.lightmap.blocks.push(LightmapBlock {
            cell_id: 0,
            width: side,
            height: side,
            irradiance: texels,
            direction: Vec::new(),
        });
    }

    /// Animated direct over lightmap blocks: block `b` (1-based) lit by
    /// `(light, weight)` pairs, uniform over its texels; its faces take an
    /// animated block of their own.
    pub(super) fn weight_lightmap_blocks(&mut self, blocks: &[(u16, &[(u32, f32)])]) {
        let mut section = AnimatedLightWeightMapsSection {
            page_size: 16,
            compact_layers: blocks.len() as u32,
            ..AnimatedLightWeightMapsSection::empty()
        };
        for (k, &(block, lights)) in blocks.iter().enumerate() {
            for vertex in &mut self.world.geometry.geometry.vertices {
                if vertex.lightmap_block == block {
                    vertex.animated_block = k as u16 + 1;
                }
            }
            let source = &self.lightmap.blocks[usize::from(block) - 1];
            let (width, height) = (u32::from(source.width), u32::from(source.height));
            section.blocks.push(AnimatedBlock {
                lightmap_block: u32::from(block) - 1,
                block_x: 0,
                block_y: 0,
                compact_x: 0,
                compact_y: 0,
                compact_layer: k as u32,
                width,
                height,
            });
            section.chunk_rects.push(ChunkAtlasRect {
                compact_x: 0,
                compact_y: 0,
                width,
                height,
                texel_offset: section.offset_counts.len() as u32,
                block: k as u32,
            });
            for _ in 0..width * height {
                section.offset_counts.push(TexelLightEntry {
                    offset: section.texel_lights.len() as u32,
                    count: lights.len() as u32,
                });
                for &(light_index, weight) in lights {
                    section.texel_lights.push(TexelLight {
                        light_index,
                        weight,
                        direction_oct: [0, 0],
                    });
                }
            }
        }
        self.weight_maps = Some(section);
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

    /// How many of `brush`'s post-cut faces lie on the plane through the
    /// map-unit point `q` across map axis `axis`: whether CSG left any of the
    /// brush's side there.
    pub(super) fn faces_on_plane(&self, brush: u32, axis: usize, q: [f64; 3]) -> usize {
        // Map x, y, z are engine −z, −x, y.
        let engine_axis = [2, 0, 1][axis];
        let value = engine(q)[engine_axis];
        let geometry = &self.world.geometry;
        self.brush_faces
            .of(brush)
            .iter()
            .filter(|&&face| {
                let range = geometry.face_index_ranges[face as usize];
                let start = range.index_offset as usize;
                geometry.geometry.indices[start..start + range.index_count as usize]
                    .iter()
                    .all(|&i| {
                        let p = geometry.geometry.vertices[i as usize].position[engine_axis];
                        (f64::from(p) - value).abs() < 1.0e-4
                    })
            })
            .count()
    }

    pub(super) fn brushes(&self) -> CaptureBrushes<'_> {
        CaptureBrushes {
            volumes: &self.world.brushes,
            face_brushes: &self.world.face_brushes,
            faces: &self.brush_faces,
        }
    }

    /// Run `f` over the stage inputs of the compiled level, its brushes'
    /// overrides as parsed.
    pub(super) fn with_inputs<R>(&self, f: impl FnOnce(&ReflectionProbeInputs<'_>) -> R) -> R {
        let world = &self.world;
        f(&ReflectionProbeInputs {
            bvh: &world.bvh,
            primitives: &world.primitives,
            geometry: &world.geometry,
            environment_volumes: &world.environment,
            reflection_volumes: &world.authored,
            brush_volumes: &world.brushes,
            face_brushes: &world.face_brushes,
            capture_override: &world.capture_override,
            tree: &world.tree,
            exterior_leaves: &world.exterior,
            lightmap: &self.lightmap,
            sh_volume: &self.sh,
            delta_sh: None,
            animated_weight_maps: self.weight_maps.as_ref(),
            animated_direct_sh: None,
            alpha_lights: &self.alpha_lights,
            chunk_light_list: &self.chunk_list,
            texture_keys: &self.keys,
            prm_root: self.prm_dir.path(),
        })
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
            animated_direct: self
                .weight_maps
                .as_ref()
                .map(|maps| AnimatedDirectSampler::new(maps, &self.lightmap)),
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

/// A 4×4 diffuse `.prm` of three mip levels, at `key`.
fn write_mip_prm(dir: &Path, key: [u8; 32], payload: MipPayload) {
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
