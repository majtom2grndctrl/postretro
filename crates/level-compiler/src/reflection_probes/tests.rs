// Stage-level reflection-probe tests: a probe captured end to end over a synthetic room.
// See: context/lib/resource_management.md §5

use std::collections::HashMap;
use std::path::Path;

use glam::Vec3;
use postretro_level_format::lightmap::{
    IRRADIANCE_FORMAT_RGBA16F, LightmapBlock, LightmapMode, LightmapSection, f32_to_f16_bits,
};
use postretro_level_format::octahedral::irradiance_tile_texel_coord;
use postretro_level_format::prm::{
    PrmFile, PrmFormat, PrmHeader, PrmSlot, PrmSlots, STAGE_VERSION, cache_filename_for_key,
};
use postretro_level_format::reflection_probes::{
    REFLECTION_PROBE_TILE_BORDER, REFLECTION_PROBE_TILE_DIMENSION, ReflectionProbeBinding,
    ReflectionProbeLightShare, ReflectionProbesSection,
};
use postretro_level_format::sh_volume::OctahedralShVolumeSection;

use super::reflection_probes_test_fixtures::{
    GeometryBuilder, fluid_volume, rect, up_rect, volumes,
};
use super::*;
use crate::bc6h::decode_bc6h_rgb_to_f32_rgba;
use crate::bvh_build::build_bvh;

/// A 4×4 diffuse `.prm` of one sRGB colour, at `key`.
fn write_solid_prm(dir: &Path, key: [u8; 32], rgba: [u8; 4]) {
    let mut payload = Vec::new();
    for texels in [16, 4, 1] {
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

/// A uniform `width × height` RGBA16F lightmap block.
fn uniform_block(width: u16, height: u16, irradiance: f32) -> LightmapBlock {
    let texel: Vec<u8> = [irradiance, irradiance, irradiance, 1.0]
        .iter()
        .flat_map(|v| f32_to_f16_bits(*v).to_le_bytes())
        .collect();
    LightmapBlock {
        cell_id: 0,
        width,
        height,
        irradiance: texel.repeat(usize::from(width) * usize::from(height)),
        direction: Vec::new(),
    }
}

/// The decoded tile texel a direction lands on.
fn tile_texel(section: &ReflectionProbesSection, layer: usize, direction: Vec3) -> Vec3 {
    let dimension = section.tile_dimension;
    let decoded = decode_bc6h_rgb_to_f32_rgba(section.tile(layer), dimension, dimension).unwrap();
    let [x, y] = irradiance_tile_texel_coord(
        direction.normalize().to_array(),
        REFLECTION_PROBE_TILE_DIMENSION,
        REFLECTION_PROBE_TILE_BORDER,
    );
    let at = ((y as u32 * dimension + x as u32) * 4) as usize;
    Vec3::new(decoded[at], decoded[at + 1], decoded[at + 2])
}

/// A room whose floor alone is lit (lightmap block 1 at irradiance 1) and
/// textured `floor_rgba`; every other face is unlit grey.
struct LitFloorRoom {
    geometry: crate::geometry::GeometryResult,
    lightmap: LightmapSection,
    keys: HashMap<String, [u8; 32]>,
    prm_dir: tempfile::TempDir,
}

fn lit_floor_room(floor_rgba: [u8; 4]) -> LitFloorRoom {
    let (lo, hi) = (Vec3::new(-3.0, 0.0, -3.0), Vec3::new(3.0, 3.0, 3.0));
    let mut builder = GeometryBuilder::default();
    let corner_uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    builder.push_face(Vec3::Y, &rect(lo, hi, 1, lo.y), 0, lo, 1, corner_uvs);
    for (normal, axis, value) in [
        (Vec3::NEG_Y, 1, hi.y),
        (Vec3::X, 0, lo.x),
        (Vec3::NEG_X, 0, hi.x),
        (Vec3::Z, 2, lo.z),
        (Vec3::NEG_Z, 2, hi.z),
    ] {
        builder.push_face(normal, &rect(lo, hi, axis, value), 1, lo, 0, [[0.0; 2]; 4]);
    }
    let geometry = builder.finish(vec!["floor".to_string(), "wall".to_string()]);
    let prm_dir = tempfile::tempdir().unwrap();
    write_solid_prm(prm_dir.path(), [1; 32], floor_rgba);
    write_solid_prm(prm_dir.path(), [2; 32], [128, 128, 128, 255]);
    let keys = HashMap::from([
        ("floor".to_string(), [1u8; 32]),
        ("wall".to_string(), [2u8; 32]),
    ]);
    LitFloorRoom {
        geometry,
        lightmap: LightmapSection {
            direction_texel_scale: 1,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            mode: LightmapMode::Shadowed,
            blocks: vec![uniform_block(8, 8, 1.0)],
        },
        keys,
        prm_dir,
    }
}

fn bake(
    room: &LitFloorRoom,
    environment: &EnvironmentVolumesSection,
) -> Option<ReflectionProbesSection> {
    let (bvh, primitives, _) = build_bvh(&room.geometry).unwrap();
    let sh = OctahedralShVolumeSection::placeholder();
    let inputs = ReflectionProbeInputs {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &room.geometry,
        environment_volumes: environment,
        lightmap: &room.lightmap,
        sh_volume: &sh,
        texture_keys: &room.keys,
        prm_root: room.prm_dir.path(),
    };
    bake_or_load_reflection_probes(&inputs, None, &BakeControl::unrestricted()).unwrap()
}

// Acceptance row 11: the capture traces the lower hemisphere too.
#[test]
fn probe_over_a_pool_with_a_red_floor_reads_red_straight_down() {
    let room = lit_floor_room([255, 0, 0, 255]);
    let environment = volumes(vec![fluid_volume(5, &[up_rect(-1.0, -1.0, 1.0, 1.0, 0.5)])]);
    let section = bake(&room, &environment).expect("the pool keeps its probe");

    assert_eq!(section.probes.len(), 1);
    let probe = &section.probes[0];
    assert_eq!(probe.source_entity, 5);
    assert_eq!(probe.lights, [ReflectionProbeLightShare::UNUSED; 4]);
    assert!((probe.capture_point[1] - 0.502).abs() < 1e-5, "{probe:?}");
    assert_eq!(
        section.bindings,
        vec![ReflectionProbeBinding {
            volume: 0,
            face: 0,
            probe: 0
        }]
    );

    // Straight down through the water: the lit red floor.
    let down = tile_texel(&section, 0, Vec3::NEG_Y);
    assert!(down.x > 0.9 && down.y < 0.02 && down.z < 0.02, "{down}");
    // Down at a slant still lands on the floor, in the tile's lower half.
    let slant = tile_texel(&section, 0, Vec3::new(0.5, -1.0, 0.3));
    assert!(slant.x > 0.9 && slant.y < 0.02, "{slant}");
    // Up: the unlit ceiling reflects black.
    let up = tile_texel(&section, 0, Vec3::Y);
    assert!(up.length() < 0.01, "{up}");

    // The emitted section is canonical: it decodes to itself.
    let decoded = ReflectionProbesSection::from_bytes(&section.to_bytes()).unwrap();
    assert_eq!(decoded, section);
}

#[test]
fn a_level_without_up_facing_fluid_emits_no_section() {
    let room = lit_floor_room([255, 0, 0, 255]);
    assert_eq!(bake(&room, &volumes(Vec::new())), None);
    // A fluid face that only faces sideways derives nothing either.
    let side = volumes(vec![fluid_volume(
        1,
        &[vec![
            [1.0, 0.0, -1.0],
            [1.0, 1.0, -1.0],
            [1.0, 1.0, 1.0],
            [1.0, 0.0, 1.0],
        ]],
    )]);
    assert_eq!(bake(&room, &side), None);
}

#[test]
fn capture_is_byte_identical_across_worker_counts() {
    let room = lit_floor_room([200, 60, 20, 255]);
    let environment = volumes(vec![
        fluid_volume(2, &[up_rect(-2.0, -2.0, 0.0, 0.0, 0.5)]),
        fluid_volume(3, &[up_rect(1.0, 1.0, 2.0, 2.0, 1.0)]),
    ]);
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap()
        .install(|| bake(&room, &environment))
        .unwrap();
    let four = rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build()
        .unwrap()
        .install(|| bake(&room, &environment))
        .unwrap();
    assert_eq!(one.probes.len(), 2);
    assert_eq!(one.to_bytes(), four.to_bytes());
}

#[test]
fn cache_key_covers_the_lightmap_sh_volumes_and_materials_it_reads() {
    let room = lit_floor_room([255, 0, 0, 255]);
    let environment = volumes(vec![fluid_volume(5, &[up_rect(-1.0, -1.0, 1.0, 1.0, 0.5)])]);
    let (bvh, primitives, _) = build_bvh(&room.geometry).unwrap();
    let sh = OctahedralShVolumeSection::placeholder();
    let key = |lightmap: &LightmapSection,
               sh: &OctahedralShVolumeSection,
               environment: &EnvironmentVolumesSection,
               keys: &HashMap<String, [u8; 32]>| {
        cache_key(&ReflectionProbeInputs {
            bvh: &bvh,
            primitives: &primitives,
            geometry: &room.geometry,
            environment_volumes: environment,
            lightmap,
            sh_volume: sh,
            texture_keys: keys,
            prm_root: room.prm_dir.path(),
        })
        .unwrap()
        .as_filename()
    };
    let base = key(&room.lightmap, &sh, &environment, &room.keys);
    assert_eq!(base, key(&room.lightmap, &sh, &environment, &room.keys));

    let mut brighter = room.lightmap.clone();
    brighter.blocks[0] = uniform_block(8, 8, 2.0);
    assert_ne!(
        base,
        key(&brighter, &sh, &environment, &room.keys),
        "lightmap"
    );

    let (baked_sh, _) = crate::sh_density::pack_indirect_section(
        super::reflection_probes_test_fixtures::dense_sh_volume([4, 4, 4], |_, _, _| [0.5; 3]),
        None,
    )
    .unwrap();
    assert_ne!(
        base,
        key(&room.lightmap, &baked_sh, &environment, &room.keys),
        "SH"
    );

    let moved = volumes(vec![fluid_volume(5, &[up_rect(-1.0, -1.0, 1.0, 1.0, 0.6)])]);
    assert_ne!(
        base,
        key(&room.lightmap, &sh, &moved, &room.keys),
        "fluid face"
    );

    let mut retextured = room.keys.clone();
    retextured.insert("floor".to_string(), [9; 32]);
    assert_ne!(
        base,
        key(&room.lightmap, &sh, &environment, &retextured),
        "albedo"
    );
}

#[test]
fn unchanged_inputs_hit_the_stage_cache_with_the_same_section() {
    let room = lit_floor_room([255, 0, 0, 255]);
    let environment = volumes(vec![fluid_volume(5, &[up_rect(-1.0, -1.0, 1.0, 1.0, 0.5)])]);
    let (bvh, primitives, _) = build_bvh(&room.geometry).unwrap();
    let sh = OctahedralShVolumeSection::placeholder();
    let inputs = ReflectionProbeInputs {
        bvh: &bvh,
        primitives: &primitives,
        geometry: &room.geometry,
        environment_volumes: &environment,
        lightmap: &room.lightmap,
        sh_volume: &sh,
        texture_keys: &room.keys,
        prm_root: room.prm_dir.path(),
    };
    let dir = tempfile::tempdir().unwrap();
    let cache = StageCache::new(dir.path()).unwrap();
    let control = BakeControl::unrestricted();
    let cold = bake_or_load_reflection_probes(&inputs, Some(&cache), &control).unwrap();
    let warm = bake_or_load_reflection_probes(&inputs, Some(&cache), &control).unwrap();
    let access = cache.test_access(STAGE_ID);
    assert_eq!((access.writes, access.read_hits), (1, 1));
    assert_eq!(cold, warm);
}
