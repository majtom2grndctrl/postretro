// Placement fixtures: small inline maps compiled through parse, BSP, portals, geometry, BVH and id 52.
// See: context/lib/testing_guide.md §4

use std::collections::{HashMap, HashSet};

use bvh::bvh::Bvh;
use glam::DVec3;
use postretro_level_format::alpha_lights::AlphaLightsSection;
use postretro_level_format::chunk_light_list::ChunkLightListSection;
use postretro_level_format::environment_volumes::EnvironmentVolumesSection;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_RGBA16F, LightmapMode, LightmapSection};
use postretro_level_format::reflection_probes::{MAX_REFLECTION_PROBES, ReflectionProbesSection};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use crate::bake_control::BakeControl;
use crate::bvh_build::{BvhPrimitive, build_bvh};
use crate::geometry::{GeometryResult, extract_geometry};
use crate::map_data::{MapData, MapReflectionVolume};
use crate::map_format::MapFormat;
use crate::partition::BspTree;
use crate::{environment_volumes, parse, partition, portals, visibility};

use super::box_fit::{WorldBounds, fit_box, fit_rays};
use super::capture_point::PointClassifier;
use super::hit::SceneTracer;
use super::placement::{
    CAPTURE_LIFT_M, PlacedProbe, PlacementScene, UpFacingFace, place_and_cap, up_facing_faces,
};
use super::{ReflectionProbeInputs, StageLimits, bake_or_load_with_limits};

/// Quake map units to engine meters.
pub(crate) const UNIT: f64 = 0.0254;
const WALL: f64 = 16.0;
const TEXTURE: &str = "test/wall";

/// A Quake-format axis-aligned box brush spanning `min`..`max` in map units,
/// each face wound so its normal points out of the box.
pub(crate) fn box_brush(min: [f64; 3], max: [f64; 3]) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {TEXTURE} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {TEXTURE} 0 0 0 1 1\n\
         }}"
    )
}

/// Six 16 u slabs sealing the box `min`..`max` (map units) from outside.
pub(crate) fn sealed_room(min: [f64; 3], max: [f64; 3]) -> Vec<String> {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    let w = WALL;
    vec![
        box_brush([x0 - w, y0 - w, z0 - w], [x1 + w, y1 + w, z0]),
        box_brush([x0 - w, y0 - w, z1], [x1 + w, y1 + w, z1 + w]),
        box_brush([x0 - w, y0 - w, z0], [x0, y1 + w, z1]),
        box_brush([x1, y0 - w, z0], [x1 + w, y1 + w, z1]),
        box_brush([x0, y0 - w, z0], [x1, y0, z1]),
        box_brush([x0, y1, z0], [x1, y1 + w, z1]),
    ]
}

/// One entity; `properties` are extra `"key" "value"` lines.
pub(crate) fn entity(classname: &str, properties: &str, brushes: &[String]) -> String {
    let mut lines = vec!["{".to_string(), format!("\"classname\" \"{classname}\"")];
    lines.extend(properties.lines().map(str::to_owned));
    lines.extend(brushes.iter().cloned());
    lines.push("}".to_string());
    lines.join("\n")
}

pub(crate) fn fluid(name: &str, brushes: &[String]) -> String {
    entity("fluid_volume", &format!("\"fluid\" \"{name}\""), brushes)
}

pub(crate) fn reflection_volume(properties: &str, min: [f64; 3], max: [f64; 3]) -> String {
    entity("reflection_volume", properties, &[box_brush(min, max)])
}

/// A map-unit point in engine meters (x = −y, y = z, z = −x).
pub(crate) fn engine(q: [f64; 3]) -> DVec3 {
    DVec3::new(-q[1], q[2], -q[0]) * UNIT
}

/// The engine-space box of the map-unit box `min`..`max`.
pub(crate) fn engine_box(min: [f64; 3], max: [f64; 3]) -> (DVec3, DVec3) {
    let (a, b) = (engine(min), engine(max));
    (a.min(b), a.max(b))
}

/// The compiler products placement reads, from a real compile of a map.
pub(crate) struct PlacementFixture {
    pub geometry: GeometryResult,
    pub bvh: Bvh<f32, 3>,
    pub primitives: Vec<BvhPrimitive>,
    pub tree: BspTree,
    pub exterior: HashSet<usize>,
    pub environment: EnvironmentVolumesSection,
    pub authored: Vec<MapReflectionVolume>,
}

impl PlacementFixture {
    /// Worldspawn with `world` brushes (entity 0), then `entities` as map
    /// entities 1, 2, …
    pub(crate) fn compile(world: &[String], entities: &[String]) -> Self {
        Self::from_map(parse_inline(&format!(
            "{}\n{}\n",
            entity("worldspawn", "", world),
            entities.join("\n")
        )))
    }

    /// A map file on disk, such as a content map, by the same front end.
    pub(crate) fn from_map_file(path: &std::path::Path) -> Self {
        Self::from_map(
            parse::parse_map_file(path, MapFormat::IdTech2)
                .unwrap_or_else(|e| panic!("parse {}: {e:#}", path.display())),
        )
    }

    fn from_map(map: MapData) -> Self {
        let result = partition::partition(&map.brush_volumes).expect("partition");
        let generated = portals::generate_portals(&result.tree);
        let exterior = visibility::find_exterior_leaves(&result.tree, &generated);
        let mut geometry = extract_geometry(&result.faces, &result.tree, &exterior);
        let (bvh, primitives, _) = build_bvh(&geometry).expect("BVH build");
        let environment = environment_volumes::encode_environment_volumes_section(
            &map.environment_volumes,
            map.initial_gravity,
            &map.brush_volumes,
            &mut geometry.texture_names,
        );
        Self {
            geometry,
            bvh,
            primitives,
            tree: result.tree,
            exterior,
            environment,
            authored: map.reflection_volumes,
        }
    }

    pub(crate) fn place(&self) -> anyhow::Result<Vec<PlacedProbe>> {
        self.place_with_cap(MAX_REFLECTION_PROBES)
    }

    pub(crate) fn place_with_cap(&self, cap: usize) -> anyhow::Result<Vec<PlacedProbe>> {
        let tracer = SceneTracer::new(&self.bvh, &self.primitives, &self.geometry);
        let bounds = self.bounds();
        let classifier = PointClassifier::new(&self.tree, &self.exterior, &self.environment);
        let control = BakeControl::unrestricted();
        place_and_cap(
            &self.environment,
            &self.authored,
            &PlacementScene {
                tracer: &tracer,
                bounds: &bounds,
                classifier: &classifier,
                governor: control.governor(),
            },
            cap,
        )
    }

    /// The whole stage under `limits`, lit by nothing: the placeholder
    /// lighting and materials leave every tile black, while the distance
    /// tiles, the splitting and the cap they drive are the real ones.
    pub(crate) fn bake(&self, limits: StageLimits) -> Option<ReflectionProbesSection> {
        let prm_dir = tempfile::tempdir().unwrap();
        let lightmap = LightmapSection {
            direction_texel_scale: 1,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            mode: LightmapMode::Shadowed,
            blocks: Vec::new(),
        };
        let sh = OctahedralShVolumeSection::placeholder();
        let alpha_lights = AlphaLightsSection::default();
        let chunk_list = ChunkLightListSection::placeholder();
        let keys = HashMap::new();
        let inputs = ReflectionProbeInputs {
            bvh: &self.bvh,
            primitives: &self.primitives,
            geometry: &self.geometry,
            environment_volumes: &self.environment,
            reflection_volumes: &self.authored,
            tree: &self.tree,
            exterior_leaves: &self.exterior,
            lightmap: &lightmap,
            sh_volume: &sh,
            delta_sh: None,
            animated_weight_maps: None,
            alpha_lights: &alpha_lights,
            chunk_light_list: &chunk_list,
            texture_keys: &keys,
            prm_root: prm_dir.path(),
        };
        bake_or_load_with_limits(&inputs, None, &BakeControl::unrestricted(), limits).unwrap()
    }

    pub(crate) fn up_faces(&self) -> Vec<UpFacingFace> {
        up_facing_faces(&self.environment)
    }

    /// The box one face fits on its own.
    pub(crate) fn face_fit(&self, face: &UpFacingFace) -> (DVec3, DVec3) {
        let tracer = SceneTracer::new(&self.bvh, &self.primitives, &self.geometry);
        let origin = face.geometry.centroid + DVec3::Y * CAPTURE_LIFT_M;
        fit_box(&tracer, origin, &self.bounds(), &fit_rays())
    }

    fn bounds(&self) -> WorldBounds {
        WorldBounds::from_positions(self.geometry.geometry.vertices.iter().map(|v| &v.position))
            .expect("the fixture has a static world")
    }
}

fn parse_inline(map_text: &str) -> MapData {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "postretro-probe-placement-{}-{id}.map",
        std::process::id()
    ));
    std::fs::write(&path, map_text).expect("write fixture map");
    let map = parse::parse_map_file(&path, MapFormat::IdTech2);
    let _ = std::fs::remove_file(&path);
    map.unwrap_or_else(|e| panic!("parse fixture map: {e:#}"))
}
