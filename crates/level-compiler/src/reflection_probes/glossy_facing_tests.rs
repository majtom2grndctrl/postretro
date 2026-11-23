// Glossy world faces of every facing behind `--glossy-reflections`: derived and authored probes on walls, ceilings and slopes, their captures and shares.
// See: context/lib/resource_management.md §5.1, §5.4 · testing_guide.md §3

use std::collections::HashMap;
use std::path::Path;

use glam::{DVec3, Vec3};
use log::Level as LogLevel;
use postretro_level_format::area_glow::{AreaGlowLight, AreaGlowSection};
use postretro_level_format::emissive::EmissiveTexture;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_RGBA16F, LightmapMode, LightmapSection};
use postretro_level_format::prm::{
    PrmFile, PrmFormat, PrmHeader, PrmSlot, PrmSlots, STAGE_VERSION, cache_filename_for_key,
};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_LOBE_COUNT, ReflectionProbesSection,
};
use postretro_render_cpu::fluid_surface::reflection::reflection_tile_uv;
use postretro_render_cpu::fluid_surface::reflection_search::reflection_distance_sample;
use postretro_test_log_capture::LogCapture;

use super::authored::BOX_PROBE_WARNING;
use super::reflection_probes_test_fixtures::{GeometryBuilder, rect};
use super::tests::{
    Extras, LitFloorRoom, descriptor, pool, tile_texel, uniform_block, with_inputs,
};
use super::*;
use crate::bc6h::f16_bits_to_f32;
use crate::map_data::MapReflectionVolume;

/// A specular texel past both opt-in conditions: `_s` 200, `_p` 200.
const GLOSSY_TEXEL: [u8; 4] = [200, 127, 200, 255];
const GREY: [u8; 4] = [128, 128, 128, 255];
const RED: [u8; 4] = [220, 20, 20, 255];
const GREEN: [u8; 4] = [20, 220, 20, 255];
/// Lightmap blocks: 1 lit at irradiance 1, 2 dark.
const LIT: u16 = 1;
const DARK: u16 = 2;

/// One material: its sRGB albedo, whether it reflects, and its `_e` colour.
#[derive(Clone, Copy)]
struct Material {
    albedo: [u8; 4],
    glossy: bool,
    emissive: Option<[u8; 4]>,
}

const MATTE_GREY: Material = Material {
    albedo: GREY,
    glossy: false,
    emissive: None,
};

fn glossy(albedo: [u8; 4]) -> Material {
    Material {
        albedo,
        glossy: true,
        emissive: None,
    }
}

fn matte(albedo: [u8; 4]) -> Material {
    Material {
        albedo,
        glossy: false,
        emissive: None,
    }
}

/// A 4×4 `.prm` of `material` at `key`: diffuse, a uniform specular slot
/// when glossy, an `_e` slot when it glows.
fn write_material(dir: &Path, key: [u8; 32], material: Material) {
    let slot = |format: PrmFormat, rgba: [u8; 4]| PrmSlot {
        format,
        width: 4,
        height: 4,
        level_count: 3,
        payload: [16, 4, 1].iter().flat_map(|&n| rgba.repeat(n)).collect(),
    };
    let mut slot_mask = PrmSlots::DIFFUSE;
    let specular = material.glossy.then(|| {
        slot_mask |= PrmSlots::SPECULAR;
        slot(PrmFormat::Rgba8Unorm, GLOSSY_TEXEL)
    });
    let emissive = material.emissive.map(|rgba| {
        slot_mask |= PrmSlots::EMISSIVE;
        slot(PrmFormat::Rgba8UnormSrgb, rgba)
    });
    let prm = PrmFile {
        header: PrmHeader {
            stage_version: STAGE_VERSION,
            slot_mask,
            bundle_hash: [0; 32],
            total_body_bytes: 0,
            layer_count: 1,
        },
        slots: [
            Some(slot(PrmFormat::Rgba8UnormSrgb, material.albedo)),
            specular,
            None,
            emissive,
        ],
    };
    std::fs::write(
        dir.join(format!("{}.prm", cache_filename_for_key(&key))),
        prm.to_bytes().unwrap(),
    )
    .unwrap();
}

/// One quad: its front normal, corners, material index and lightmap block.
type Quad = (Vec3, [Vec3; 4], u32, u16);

/// The 6 × 3 × 6 m room `x, z ∈ [−3, 3]`, `y ∈ [0, 3]`.
const LO: Vec3 = Vec3::new(-3.0, 0.0, -3.0);
const HI: Vec3 = Vec3::new(3.0, 3.0, 3.0);

/// Face `face` of the room: 0 floor, 1 ceiling, 2 west (x −3, facing +X),
/// 3 east, 4 north (z −3, facing +Z), 5 south.
fn room_face(face: usize, material: u32, block: u16) -> Quad {
    let (normal, corners) = match face {
        0 => (Vec3::Y, rect(LO, HI, 1, LO.y)),
        1 => (Vec3::NEG_Y, rect(LO, HI, 1, HI.y)),
        2 => (Vec3::X, rect(LO, HI, 0, LO.x)),
        3 => (Vec3::NEG_X, rect(LO, HI, 0, HI.x)),
        4 => (Vec3::Z, rect(LO, HI, 2, LO.z)),
        _ => (Vec3::NEG_Z, rect(LO, HI, 2, HI.z)),
    };
    (normal, corners, material, block)
}

/// The room's six faces, all `material` and lit, but those `except` lists.
fn room_except(except: &[usize], material: u32) -> Vec<Quad> {
    (0..6)
        .filter(|face| !except.contains(face))
        .map(|face| room_face(face, material, LIT))
        .collect()
}

/// A level of `quads` over `materials`, lit by lightmap block 1 at
/// irradiance 1, glossy placement gated by `gate`.
fn scene(quads: &[Quad], materials: &[Material], gate: bool) -> LitFloorRoom {
    let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let mut builder = GeometryBuilder::default();
    for &(normal, corners, material, block) in quads {
        builder.push_face(normal, &corners, material, LO, block, uvs);
    }
    let names: Vec<String> = (0..materials.len()).map(|i| format!("m{i}")).collect();
    let geometry = builder.finish(names.clone());
    let prm_dir = tempfile::tempdir().unwrap();
    let mut keys = HashMap::new();
    for (i, (name, material)) in names.iter().zip(materials).enumerate() {
        let key = [i as u8 + 1; 32];
        write_material(prm_dir.path(), key, *material);
        keys.insert(name.clone(), key);
    }
    LitFloorRoom {
        geometry,
        lightmap: LightmapSection {
            direction_texel_scale: 1,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            mode: LightmapMode::Shadowed,
            blocks: vec![uniform_block(8, 8, 1.0), uniform_block(8, 8, 0.0)],
        },
        keys,
        prm_dir,
        extras: Extras {
            glossy_reflections: gate,
            ..Extras::default()
        },
    }
}

fn no_water() -> EnvironmentVolumesSection {
    EnvironmentVolumesSection::default()
}

/// Placement alone, as the stage runs it.
fn place(room: &LitFloorRoom, water: &EnvironmentVolumesSection) -> Vec<PlacedProbe> {
    with_inputs(room, water, |inputs| {
        let tracer = SceneTracer::new(inputs.bvh, inputs.primitives, inputs.geometry);
        let bounds = WorldBounds::from_positions(
            room.geometry.geometry.vertices.iter().map(|v| &v.position),
        )
        .unwrap();
        let classifier = PointClassifier::new(inputs.tree, inputs.exterior_leaves, water);
        let control = BakeControl::unrestricted();
        let world_faces = glossy_placement::glossy_world_faces(inputs, control.governor());
        place_probes(
            water,
            inputs.reflection_volumes,
            &world_faces,
            &PlacementScene {
                tracer: &tracer,
                bounds: &bounds,
                classifier: &classifier,
                governor: control.governor(),
            },
            MAX_REFLECTION_PROBES,
        )
        .unwrap()
    })
}

fn bake_under(
    room: &LitFloorRoom,
    water: &EnvironmentVolumesSection,
    reflectors: Reflectors,
) -> ReflectionProbesSection {
    with_inputs(room, water, |inputs| {
        bake_or_load_with_reflectors(inputs, reflectors).unwrap()
    })
    .expect("the level keeps a probe")
}

/// The probe binding world face `face`.
fn binding(probes: &[PlacedProbe], face: u32) -> (usize, &PlacedProbe) {
    probes
        .iter()
        .enumerate()
        .find(|(_, probe)| probe.world_faces.contains(&face))
        .unwrap_or_else(|| panic!("face {face} binds no probe: {probes:#?}"))
}

/// The centroid and unit normal of quad `quad`.
fn centre(quad: &Quad) -> (DVec3, DVec3) {
    let sum: Vec3 = quad.1.iter().copied().sum();
    ((sum / 4.0).as_dvec3(), quad.0.normalize().as_dvec3())
}

/// `probe`'s capture point lies on `quad`'s front side, 2 mm off its plane
/// along its normal.
fn assert_lifted_off(probe: &PlacedProbe, quad: &Quad) {
    let (centroid, normal) = centre(quad);
    let capture = probe.capture_point_vec3().as_dvec3();
    let lift = (capture - centroid).dot(normal);
    assert!(
        (lift - placement::CAPTURE_LIFT_M).abs() < 1e-5,
        "lift {lift} off {normal}: {probe:?}"
    );
    assert!(
        (probe.facing.normal() - normal).length() < 1e-6,
        "{:?} vs {normal}",
        probe.facing
    );
}

// Row 5: derived probes on a floor, a wall and a down-facing ceiling, each on
// its face's front side, lifted along its normal; coplanar pieces of one face
// group share a probe.
#[test]
fn derived_probes_lift_off_floors_walls_and_ceilings_along_each_normal() {
    let quads = vec![
        // 0, 1: a glossy floor in two pieces either side of a matte strip.
        (
            Vec3::Y,
            rect(LO, Vec3::new(-0.25, 0.0, 3.0), 1, 0.0),
            1,
            LIT,
        ),
        (
            Vec3::Y,
            rect(Vec3::new(0.25, 0.0, -3.0), HI, 1, 0.0),
            1,
            LIT,
        ),
        // 2, 3: the west wall in two pieces.
        (
            Vec3::X,
            rect(LO, Vec3::new(-3.0, 3.0, 0.0), 0, -3.0),
            1,
            LIT,
        ),
        (
            Vec3::X,
            rect(Vec3::new(-3.0, 0.0, 0.0), HI, 0, -3.0),
            1,
            LIT,
        ),
        // 4: the ceiling, facing down.
        room_face(1, 1, LIT),
        // The matte strip and the room's other walls.
        (
            Vec3::Y,
            rect(
                Vec3::new(-0.25, 0.0, -3.0),
                Vec3::new(0.25, 0.0, 3.0),
                1,
                0.0,
            ),
            0,
            LIT,
        ),
        room_face(3, 0, LIT),
        room_face(4, 0, LIT),
        room_face(5, 0, LIT),
    ];
    let room = scene(&quads, &[MATTE_GREY, glossy(GREY)], true);
    let probes = place(&room, &no_water());
    assert_eq!(probes.len(), 3, "{probes:#?}");
    let (floor, floor_probe) = binding(&probes, 0);
    assert_eq!(
        binding(&probes, 1).0,
        floor,
        "a matte strip never splits a floor"
    );
    let (wall, wall_probe) = binding(&probes, 2);
    assert_eq!(binding(&probes, 3).0, wall, "coplanar wall pieces share");
    let (_, ceiling_probe) = binding(&probes, 4);
    for probe in &probes {
        assert!(!probe.authored);
        assert_eq!(probe.source_entity, placement::WORLD_SOURCE_ENTITY);
        let capture = probe.capture_point_vec3();
        let (min, max) = (Vec3::from(probe.box_min), Vec3::from(probe.box_max));
        assert!(
            capture.cmpge(min).all() && capture.cmple(max).all(),
            "{probe:?}"
        );
    }
    assert!(floor_probe.facing.is_up());
    // The floor's centroid lies on the strip: the point moves onto a piece.
    let floor_capture = floor_probe.capture_point_vec3();
    assert!((floor_capture.y - 0.002).abs() < 1e-6, "{floor_capture}");
    assert!(floor_capture.x.abs() >= 0.25, "{floor_capture}");
    assert_lifted_off(wall_probe, &quads[2]);
    assert_lifted_off(ceiling_probe, &quads[4]);

    // Each box holds its faces and reaches the room around them.
    let wall_box = (
        Vec3::from(wall_probe.box_min),
        Vec3::from(wall_probe.box_max),
    );
    assert!((wall_box.0 - LO).abs().max_element() < 0.02, "{wall_box:?}");
    assert!((wall_box.1 - HI).abs().max_element() < 0.02, "{wall_box:?}");
    let ceiling_box = (
        Vec3::from(ceiling_probe.box_min),
        Vec3::from(ceiling_probe.box_max),
    );
    assert!(
        (ceiling_box.0 - LO).abs().max_element() < 0.02,
        "{ceiling_box:?}"
    );
    assert!(
        (ceiling_box.1 - HI).abs().max_element() < 0.02,
        "{ceiling_box:?}"
    );
}

/// A 45° ramp against the east wall, rising from the floor to 1.5 m, facing
/// −X and up.
fn ramp(material: u32) -> Quad {
    (
        Vec3::new(-1.0, 1.0, 0.0).normalize(),
        [
            Vec3::new(1.5, 0.0, -1.0),
            Vec3::new(1.5, 0.0, 1.0),
            Vec3::new(3.0, 1.5, 1.0),
            Vec3::new(3.0, 1.5, -1.0),
        ],
        material,
        LIT,
    )
}

// Row 5: a derived probe on a 45° slope sits on its front side, lifted along
// its normal, and its box, fitted along the axis it is nearest, holds it.
#[test]
fn a_derived_probe_lifts_off_a_45_degree_slope_along_its_normal() {
    let mut quads = vec![ramp(1)];
    quads.extend(room_except(&[], 0));
    let room = scene(&quads, &[MATTE_GREY, glossy(GREY)], true);
    let probes = place(&room, &no_water());
    assert_eq!(probes.len(), 1, "{probes:#?}");
    let probe = &probes[0];
    assert_eq!(probe.world_faces, vec![0]);
    assert!(!probe.facing.is_up());
    assert_lifted_off(probe, &quads[0]);
    let (min, max) = (Vec3::from(probe.box_min), Vec3::from(probe.box_max));
    assert!(
        min.cmple(Vec3::new(1.5, 0.0, -1.0)).all() && max.cmpge(Vec3::new(3.0, 1.5, 1.0)).all(),
        "the box holds the ramp: {min} {max}"
    );
    assert!(
        (min - LO).abs().max_element() < 0.02 && (max - HI).abs().max_element() < 0.02,
        "the box reaches the room: {min} {max}"
    );
}

// Row 5: faces on different planes, or on one plane facing opposite ways,
// never share a derived probe.
#[test]
fn derived_probes_never_join_faces_on_other_planes_or_facing_the_other_way() {
    let panel = |normal: Vec3| {
        (
            normal,
            rect(Vec3::new(0.0, 0.0, -1.0), Vec3::new(0.0, 2.0, 1.0), 0, 0.0),
            1,
            LIT,
        )
    };
    let mut quads = vec![room_face(2, 1, LIT), panel(Vec3::X), panel(Vec3::NEG_X)];
    quads.extend(room_except(&[2], 0));
    let room = scene(&quads, &[MATTE_GREY, glossy(GREY)], true);
    let probes = place(&room, &no_water());
    let (west, _) = binding(&probes, 0);
    let (front, front_probe) = binding(&probes, 1);
    let (back, back_probe) = binding(&probes, 2);
    assert_eq!(probes.len(), 3, "{probes:#?}");
    assert!(west != front && front != back && west != back);
    assert_lifted_off(front_probe, &quads[1]);
    assert_lifted_off(back_probe, &quads[2]);
}

/// An authored box over `min..max`, entity 2.
fn reflection_volume(min: [f32; 3], max: [f32; 3]) -> MapReflectionVolume {
    MapReflectionVolume {
        source_entity: 2,
        source_label: "entity 2 'reflection_volume'".to_string(),
        min,
        max,
        capture_offset: None,
    }
}

/// The room enclosed, 1 cm all round.
fn room_volume() -> MapReflectionVolume {
    reflection_volume([-3.01, -0.01, -3.01], [3.01, 3.01, 3.01])
}

// Row 6: a `reflection_volume` claims the glossy world faces inside it; a box
// over neither water nor a glossy face drops with a warning.
#[test]
fn a_reflection_volume_claims_glossy_faces_and_an_empty_one_drops_with_a_warning() {
    let mut quads = vec![room_face(0, 1, LIT), room_face(2, 1, LIT)];
    quads.extend(room_except(&[0, 2], 0));
    let mut room = scene(&quads, &[MATTE_GREY, glossy(GREY)], true);
    room.extras.authored = vec![
        reflection_volume([-3.01, -0.01, -3.01], [3.01, 1.0, 3.01]),
        MapReflectionVolume {
            source_entity: 3,
            source_label: "entity 3 'reflection_volume'".to_string(),
            ..reflection_volume([1.0, 1.0, 1.0], [2.0, 2.0, 2.0])
        },
    ];
    let logs = LogCapture::start();
    let probes = place(&room, &no_water());
    let (_, floor) = binding(&probes, 0);
    assert!(floor.authored, "the box claims the floor: {floor:?}");
    assert_eq!(floor.source_entity, 2);
    assert_eq!(floor.box_min, [-3.01, -0.01, -3.01]);
    // The wall's centroid lies above the box: a derived probe takes it.
    let (_, wall) = binding(&probes, 1);
    assert!(!wall.authored);
    assert_eq!(probes.len(), 2);
    logs.assert_logged_once(
        LogLevel::Warn,
        "entity 3 'reflection_volume' binds no up-facing fluid face or glossy world face; dropped",
    );
}

// Row 7 and pin P3: one authored box binding a glossy floor and a glossy wall
// makes a probe each; the floor's capture shows the wall, and the wall's the
// floor.
#[test]
fn in_one_box_a_glossy_floor_reflects_a_glossy_wall_and_the_wall_the_floor() {
    let mut quads = vec![room_face(0, 1, LIT), room_face(2, 2, LIT)];
    quads.extend(room_except(&[0, 2], 0));
    let mut room = scene(&quads, &[MATTE_GREY, glossy(RED), glossy(GREEN)], true);
    room.extras.authored = vec![room_volume()];
    let section = bake_under(&room, &no_water(), Reflectors::SelfTransparent);
    assert_eq!(section.probes.len(), 2);
    let layer_of = |face: u32| {
        section
            .world_bindings
            .iter()
            .find(|b| b.face == face)
            .unwrap()
            .probe as usize
    };
    let (floor, wall) = (layer_of(0), layer_of(1));
    assert_ne!(floor, wall);
    for layer in [floor, wall] {
        assert_eq!(section.probes[layer].box_min, [-3.01, -0.01, -3.01]);
    }
    let toward = |layer: usize, target: Vec3| {
        let capture = Vec3::from(section.probes[layer].capture_point);
        tile_texel(&section, layer, target - capture)
    };
    let floor_sees_wall = toward(floor, Vec3::new(-3.0, 1.5, 0.5));
    assert!(
        floor_sees_wall.y > 4.0 * floor_sees_wall.x,
        "the floor reflects the green wall: {floor_sees_wall}"
    );
    let wall_sees_floor = toward(wall, Vec3::new(0.5, 0.0, 0.5));
    assert!(
        wall_sees_floor.x > 4.0 * wall_sees_floor.y,
        "the wall reflects the red floor: {wall_sees_floor}"
    );
}

// Row 7 and pin P3: one authored box binding a pool and a glossy wall. The
// pool reflects the wall; the wall reflects the room with the pool's bed
// where the water lies, as captures hold no fluid surface.
#[test]
fn in_one_box_a_pool_reflects_a_glossy_wall_and_the_wall_shows_the_pools_bed() {
    let mut quads = vec![room_face(0, 1, LIT), room_face(2, 2, LIT)];
    quads.extend(room_except(&[0, 2], 0));
    let mut room = scene(&quads, &[MATTE_GREY, matte(RED), glossy(GREEN)], true);
    room.extras.authored = vec![room_volume()];
    let water = pool();
    let section = bake_under(&room, &water, Reflectors::SelfTransparent);
    assert_eq!(section.probes.len(), 2);
    let pool_layer = section.fluid_bindings[0].probe as usize;
    let wall_layer = section.world_bindings[0].probe as usize;
    assert_ne!(pool_layer, wall_layer);
    assert_eq!(section.world_bindings[0].face, 1);
    let pool_capture = Vec3::from(section.probes[pool_layer].capture_point);
    assert!((pool_capture - Vec3::new(0.0, 0.502, 0.0)).length() < 1e-5);
    let pool_sees_wall = tile_texel(
        &section,
        pool_layer,
        Vec3::new(-3.0, 1.5, 0.5) - pool_capture,
    );
    assert!(
        pool_sees_wall.y > 4.0 * pool_sees_wall.x,
        "the pool reflects the green wall: {pool_sees_wall}"
    );
    // Toward the water's middle, the wall's capture reads the red bed under
    // it: no surface lies at the water's height.
    let wall_capture = Vec3::from(section.probes[wall_layer].capture_point);
    let toward_water = Vec3::new(0.3, 0.5, 0.2) - wall_capture;
    let bed = tile_texel(&section, wall_layer, toward_water);
    assert!(bed.x > 4.0 * bed.y, "the wall shows the red bed: {bed}");
    let distances = distance_texels(&section, wall_layer);
    let w = reflection_distance_sample(&distances, section.tile_dimension, tile_uv(toward_water));
    let l1 = toward_water.abs().element_sum();
    assert!(
        l1 * w < 0.95,
        "the stored hit lies past the water's surface, on its bed: {}",
        l1 * w
    );
}

// Row 8: an authored box over glossy floors on two heights and two glossy
// walls facing different ways makes three probes, each with the box as its
// box: one binding both floors, one per wall.
#[test]
fn an_authored_box_makes_one_probe_per_facing_and_one_for_floors_on_two_heights() {
    let platform = [Vec3::new(0.0, 0.5, -3.0), Vec3::new(3.0, 0.5, 3.0)];
    let mut quads = vec![
        (Vec3::Y, rect(LO, Vec3::new(0.0, 0.0, 3.0), 1, 0.0), 1, LIT),
        (Vec3::Y, rect(platform[0], platform[1], 1, 0.5), 1, LIT),
        room_face(2, 1, LIT),
        room_face(4, 1, LIT),
    ];
    quads.extend(room_except(&[0, 2, 4], 0));
    let mut room = scene(&quads, &[MATTE_GREY, glossy(GREY)], true);
    room.extras.authored = vec![room_volume()];
    let logs = LogCapture::start();
    let probes = place(&room, &no_water());
    assert_eq!(probes.len(), 3, "{probes:#?}");
    for probe in &probes {
        assert!(probe.authored);
        assert_eq!(
            (probe.box_min, probe.box_max),
            ([-3.01, -0.01, -3.01], [3.01, 3.01, 3.01])
        );
    }
    let (floors, floor_probe) = binding(&probes, 0);
    assert_eq!(binding(&probes, 1).0, floors, "both floor heights share");
    assert_eq!(floor_probe.world_faces, vec![0, 1]);
    // In front of the frontmost floor, so in front of both.
    let capture = floor_probe.capture_point_vec3();
    assert!((capture.y - 0.502).abs() < 1e-5, "{capture}");
    assert!(capture.x >= 0.0, "over the platform: {capture}");
    let (west, west_probe) = binding(&probes, 2);
    let (north, north_probe) = binding(&probes, 3);
    assert!(west != north && west != floors && north != floors);
    assert_eq!(west_probe.world_faces, vec![2]);
    assert_eq!(north_probe.world_faces, vec![3]);
    assert_lifted_off(west_probe, &quads[2]);
    assert_lifted_off(north_probe, &quads[3]);
    logs.assert_logged_once(LogLevel::Warn, "binds faces on more than one plane");
}

// Row 9 and pin P4: an authored box over a pool above a lower glossy floor
// makes one up-facing probe whose capture point lies in front of both.
#[test]
fn a_box_over_a_pool_above_a_glossy_floor_captures_in_front_of_both() {
    let mut quads = vec![room_face(0, 1, LIT)];
    quads.extend(room_except(&[0], 0));
    let mut room = scene(&quads, &[MATTE_GREY, glossy(GREY)], true);
    room.extras.authored = vec![room_volume()];
    let probes = place(&room, &pool());
    assert_eq!(probes.len(), 1, "{probes:#?}");
    let probe = &probes[0];
    assert_eq!(probe.fluid_faces, vec![(0, 0)]);
    assert_eq!(probe.world_faces, vec![0]);
    let capture = probe.capture_point_vec3();
    assert!(
        (capture - Vec3::new(0.0, 0.502, 0.0)).length() < 1e-5,
        "{capture}"
    );
}

// Row 9 and pin P4: a face the capture point cannot serve binds nothing, and
// the warning names it. The box's top is flush with a raised glossy floor,
// so the clamp leaves the point on that floor's plane.
#[test]
fn a_face_the_capture_point_lies_behind_binds_nothing_and_the_warning_names_it() {
    let mut quads = vec![
        (Vec3::Y, rect(LO, Vec3::new(0.0, 0.0, 3.0), 1, 0.0), 1, LIT),
        (
            Vec3::Y,
            rect(Vec3::new(0.0, 0.5, -3.0), Vec3::new(3.0, 0.5, 3.0), 1, 0.5),
            1,
            LIT,
        ),
    ];
    quads.extend(room_except(&[0], 0));
    let mut room = scene(&quads, &[MATTE_GREY, glossy(GREY)], true);
    room.extras.authored = vec![reflection_volume([-3.01, -0.01, -3.01], [3.01, 0.5, 3.01])];
    let logs = LogCapture::start();
    let probes = place(&room, &no_water());
    assert_eq!(
        probes.len(),
        1,
        "the raised floor takes no derived probe: {probes:#?}"
    );
    assert_eq!(probes[0].world_faces, vec![0]);
    assert!(probes[0].authored);
    logs.assert_logged_once(LogLevel::Warn, "is not in front of glossy world face 1");
}

/// One probe's decoded distance tile.
fn distance_texels(section: &ReflectionProbesSection, layer: usize) -> Vec<f32> {
    section
        .distance_tile(layer)
        .as_chunks::<2>()
        .0
        .iter()
        .map(|bits| f16_bits_to_f32(u16::from_le_bytes(*bits)))
        .collect()
}

fn tile_uv(direction: Vec3) -> glam::Vec2 {
    use postretro_level_format::reflection_probes::{
        REFLECTION_PROBE_TILE_BORDER, REFLECTION_PROBE_TILE_DIMENSION,
    };
    reflection_tile_uv(
        direction,
        REFLECTION_PROBE_TILE_DIMENSION,
        REFLECTION_PROBE_TILE_BORDER,
    )
}

/// Points of the west wall rectangle `(z0, y0, z1, y1)` at x = −3: a 24 × 24
/// grid inset 2 cm from its edges.
fn wall_samples([z0, y0, z1, y1]: [f32; 4]) -> Vec<Vec3> {
    let n = 24;
    let inset = 0.02;
    let at =
        |a: f32, b: f32, i: usize| a + inset + (b - a - 2.0 * inset) * i as f32 / (n - 1) as f32;
    (0..n)
        .flat_map(|i| (0..n).map(move |j| Vec3::new(-3.0, at(y0, y1, j), at(z0, z1, i))))
        .collect()
}

/// The wall points among `points` in capture shadow of the wall's probe:
/// `h = |v|₁ · w(v) − 1 ≥ 0`, with `v` the offset from the capture point
/// clamped to the wall's hemisphere (+X) as the runtime clamps every tap to
/// its face's, and `w` the distance tile as the runtime filters it.
fn wall_shadowed(section: &ReflectionProbesSection, points: &[Vec3]) -> Vec<Vec3> {
    let layer = section.world_bindings[0].probe as usize;
    let capture = Vec3::from(section.probes[layer].capture_point);
    let texels = distance_texels(section, layer);
    points
        .iter()
        .copied()
        .filter(|&point| {
            let offset = point - capture;
            let level = Vec3::new(offset.x.max(0.0), offset.y, offset.z);
            let w = reflection_distance_sample(&texels, section.tile_dimension, tile_uv(level));
            level.abs().element_sum() * w - 1.0 >= 0.0
        })
        .collect()
}

/// A room whose west wall is `pieces` (`(z0, y0, z1, y1)`, glossy or not),
/// its other faces matte and lit.
fn wall_room(pieces: &[([f32; 4], bool)]) -> LitFloorRoom {
    let mut quads: Vec<Quad> = pieces
        .iter()
        .map(|&([z0, y0, z1, y1], glossy)| {
            let corners = rect(Vec3::new(-3.0, y0, z0), Vec3::new(-3.0, y1, z1), 0, -3.0);
            (Vec3::X, corners, u32::from(glossy), LIT)
        })
        .collect();
    quads.extend(room_except(&[2], 0));
    scene(&quads, &[MATTE_GREY, glossy(GREY)], true)
}

// Row 10, wall half: the capture point sits 2 mm off the wall, so with the
// wall in its capture the texels along it read the wall itself and its own
// fragments fall in capture shadow; transparent to its own probe, none do.
#[test]
fn a_glossy_wall_probe_places_no_fragment_of_its_own_wall_in_capture_shadow() {
    let whole = [-3.0, 0.0, 3.0, 3.0];
    let room = wall_room(&[(whole, true)]);
    let points = wall_samples(whole);
    let transparent = bake_under(&room, &no_water(), Reflectors::SelfTransparent);
    assert_eq!(wall_shadowed(&transparent, &points), Vec::<Vec3>::new());
    let opaque = bake_under(&room, &no_water(), Reflectors::Opaque);
    let dark = wall_shadowed(&opaque, &points).len();
    assert!(
        dark * 2 > points.len(),
        "the self-occluded capture withholds most of the wall: {dark} of {}",
        points.len()
    );
}

// Row 11 and pin P2, wall half: a coplanar matte strip crossing a glossy wall
// casts no capture shadow on the glossy wall beyond it, and a glossy inlay
// flush in a matte wall reflects over its whole area.
#[test]
fn a_coplanar_matte_strip_or_surround_casts_no_capture_shadow_on_a_glossy_wall() {
    let (south, north) = ([-3.0, 0.0, -0.25, 3.0], [0.25, 0.0, 3.0, 3.0]);
    let room = wall_room(&[
        (south, true),
        ([-0.25, 0.0, 0.25, 3.0], false),
        (north, true),
    ]);
    let section = bake_under(&room, &no_water(), Reflectors::SelfTransparent);
    assert_eq!(
        section.probes.len(),
        1,
        "a matte strip never splits a wall's probe"
    );
    assert_eq!(section.world_bindings.len(), 2);
    let mut points = wall_samples(south);
    points.extend(wall_samples(north));
    assert_eq!(wall_shadowed(&section, &points), Vec::<Vec3>::new());
    let bound_only = bake_under(&room, &no_water(), Reflectors::BoundOnly);
    let far = if section.probes[0].capture_point[2] < 0.0 {
        north
    } else {
        south
    };
    assert!(
        !wall_shadowed(&bound_only, &wall_samples(far)).is_empty(),
        "with only its bound faces transparent, the strip shadows the far half"
    );

    let inlay = [-1.0, 0.5, 1.0, 2.5];
    let room = wall_room(&[
        (inlay, true),
        ([-3.0, 0.0, -1.0, 3.0], false),
        ([1.0, 0.0, 3.0, 3.0], false),
        ([-1.0, 0.0, 1.0, 0.5], false),
        ([-1.0, 2.5, 1.0, 3.0], false),
    ]);
    let section = bake_under(&room, &no_water(), Reflectors::SelfTransparent);
    assert_eq!(section.world_bindings.len(), 1, "the inlay alone binds");
    assert_eq!(
        wall_shadowed(&section, &wall_samples(inlay)),
        Vec::<Vec3>::new()
    );
}

// Row 19, compiler half: a glossy down-facing ceiling over a floor
// `light_area` panel takes that light's share on the −Y lobe, and none on
// +Y, which it never reads.
#[test]
fn a_glossy_ceiling_over_a_floor_light_area_panel_takes_its_share_on_the_down_lobe() {
    let red = [1.0, 0.0, 0.0];
    let glass = Material {
        albedo: [0, 0, 0, 255],
        glossy: false,
        emissive: Some([188, 188, 188, 255]),
    };
    // Nothing else lit, so the panel's glass is all the ceiling captures.
    let quads: Vec<Quad> = [(0, 1), (1, 2), (2, 0), (3, 0), (4, 0), (5, 0)]
        .into_iter()
        .map(|(face, material)| room_face(face, material, DARK))
        .collect();
    let mut room = scene(&quads, &[MATTE_GREY, glass, glossy(GREY)], true);
    let glass_texel = ((188.0f32 / 255.0 + 0.055) / 1.055).powf(2.4);
    room.extras.emissive = vec![
        EmissiveTexture::DARK,
        EmissiveTexture {
            strength: 0.0,
            glow_color: [glass_texel; 3],
        },
        EmissiveTexture::DARK,
    ];
    room.extras.area_glow = Some(AreaGlowSection {
        lights: vec![AreaGlowLight {
            glow: 4.0,
            color: red,
            intensity: 1.0,
            compose_slot: Some(0),
            faces: vec![0],
        }],
    });
    room.extras.sh.animation_descriptors = vec![descriptor(red, vec![0.0, 1.0])];
    let section = bake_under(&room, &no_water(), Reflectors::SelfTransparent);
    assert_eq!(section.probes.len(), 1);
    assert_eq!(section.world_bindings[0].face, 1);
    let probe = &section.probes[0];
    assert!((probe.capture_point[1] - 2.998).abs() < 1e-5, "{probe:?}");
    let entry = probe
        .lights
        .iter()
        .find(|entry| entry.is_used() && entry.roster_index == 0)
        .expect("the panel's light takes a share");
    let down = REFLECTION_PROBE_LOBE_COUNT - 1;
    assert!(entry.shares[down][0] > 0.99, "{entry:?}");
    assert_eq!(entry.shares[0], [0.0; 3], "+Y lies behind the ceiling");
}

// Row 68: an authored box whose facings make more probes than the threshold
// builds, and the warning names the box and its probe count.
#[test]
fn an_authored_box_past_the_probe_warning_builds_and_names_its_count() {
    let mut quads = room_except(&[], 1);
    quads.push(ramp(1));
    let mut room = scene(&quads, &[MATTE_GREY, glossy(GREY)], true);
    room.extras.authored = vec![room_volume()];
    let logs = LogCapture::start();
    let probes = place(&room, &no_water());
    assert_eq!(probes.len(), BOX_PROBE_WARNING + 1, "{probes:#?}");
    assert!(probes.iter().all(|probe| probe.authored));
    logs.assert_logged_once(
        LogLevel::Warn,
        &format!(
            "entity 2 'reflection_volume' keeps {} probes",
            BOX_PROBE_WARNING + 1
        ),
    );
}

// A `_capture_offset` moves each facing's point within its own plane: along
// a wall it moves sideways, never off the wall.
#[test]
fn a_capture_offset_moves_a_wall_probe_along_its_wall() {
    let mut quads = vec![room_face(2, 1, LIT)];
    quads.extend(room_except(&[2], 0));
    let mut room = scene(&quads, &[MATTE_GREY, glossy(GREY)], true);
    let mut at = |offset: Option<[f32; 2]>| {
        room.extras.authored = vec![MapReflectionVolume {
            capture_offset: offset,
            ..room_volume()
        }];
        place(&room, &no_water())[0].capture_point_vec3()
    };
    let plain = at(None);
    let moved = at(Some([1.0, 0.5]));
    assert!(
        (moved - plain - Vec3::new(0.0, 0.0, 0.5)).length() < 1e-5,
        "{plain} → {moved}"
    );
}
