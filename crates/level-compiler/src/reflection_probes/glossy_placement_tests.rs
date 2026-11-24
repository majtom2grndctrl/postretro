// Glossy floor placement behind `--glossy-reflections`: the gate, derived floor probes and their self-transparent capture.
// See: context/lib/resource_management.md §5.4 · testing_guide.md §3

use std::collections::HashMap;
use std::path::Path;

use glam::{Vec2, Vec3};
use log::Level as LogLevel;
use postretro_level_format::lightmap::{IRRADIANCE_FORMAT_RGBA16F, LightmapMode, LightmapSection};
use postretro_level_format::prm::{
    PrmFile, PrmFormat, PrmHeader, PrmSlot, PrmSlots, STAGE_VERSION, cache_filename_for_key,
    expected_level_count,
};
use postretro_level_format::reflection_probes::{
    ReflectionProbeWorldBinding, ReflectionProbesSection,
};
use postretro_render_cpu::fluid_surface::reflection_search::{
    FluidReflectionSearchStatus, reflection_distance_sample, reflection_search, reflection_search_h,
};
use postretro_test_log_capture::LogCapture;

use super::reflection_probes_test_fixtures::{GeometryBuilder, rect};
use super::tests::{Extras, LitFloorRoom, pool, uniform_block, with_inputs};
use super::*;
use crate::bc6h::f16_bits_to_f32;

/// A diffuse texel every material shares, so glossy and matte capture alike.
const ALBEDO: [u8; 4] = [160, 160, 160, 255];
/// A specular texel past both opt-in conditions: `_s` 200, `_p` 200.
const GLOSSY_TEXEL: [u8; 4] = [200, 127, 200, 255];

/// Texture indices of [`room`]'s materials.
const MATTE: u32 = 0;
const GLOSSY: u32 = 1;

/// A `.prm` at `key` with a uniform diffuse slot, and a specular slot of
/// `specular` when given.
fn write_prm(dir: &Path, key: [u8; 32], specular: Option<[u8; 4]>) {
    let diffuse: Vec<u8> = [16, 4, 1].iter().flat_map(|&n| ALBEDO.repeat(n)).collect();
    let specular = specular.map(|texel| PrmSlot {
        format: PrmFormat::Rgba8Unorm,
        width: 2,
        height: 2,
        level_count: expected_level_count(2, 2),
        payload: [texel.repeat(4), texel.to_vec()].concat(),
    });
    let mut slot_mask = PrmSlots::DIFFUSE;
    if specular.is_some() {
        slot_mask |= PrmSlots::SPECULAR;
    }
    let prm = PrmFile {
        header: PrmHeader {
            stage_version: STAGE_VERSION,
            slot_mask,
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
                payload: diffuse,
            }),
            specular,
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

/// A 6 × 3 × 6 m room centred on the origin whose floor at y = 0 is
/// `pieces`, each `(x0, z0, x1, z1)` with its texture, lit at irradiance 1;
/// ceiling and walls are matte and unlit. Floor pieces are faces 0.. in
/// order. `glossy_material` false bakes the glossy texture without a
/// specular slot, a matte twin with the same albedo.
fn room(pieces: &[([f32; 4], u32)], glossy_material: bool, gate: bool) -> LitFloorRoom {
    let (lo, hi) = (Vec3::new(-3.0, 0.0, -3.0), Vec3::new(3.0, 3.0, 3.0));
    let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let mut builder = GeometryBuilder::default();
    for &([x0, z0, x1, z1], texture) in pieces {
        let corners = rect(Vec3::new(x0, 0.0, z0), Vec3::new(x1, 0.0, z1), 1, 0.0);
        builder.push_face(Vec3::Y, &corners, texture, lo, 1, uvs);
    }
    builder.push_face(Vec3::NEG_Y, &rect(lo, hi, 1, hi.y), MATTE, lo, 2, uvs);
    for (normal, axis, value) in [
        (Vec3::X, 0, lo.x),
        (Vec3::NEG_X, 0, hi.x),
        (Vec3::Z, 2, lo.z),
        (Vec3::NEG_Z, 2, hi.z),
    ] {
        builder.push_face(
            normal,
            &rect(lo, hi, axis, value),
            MATTE,
            lo,
            0,
            [[0.0; 2]; 4],
        );
    }
    let geometry = builder.finish(vec!["matte".to_string(), "glossy".to_string()]);
    let prm_dir = tempfile::tempdir().unwrap();
    write_prm(prm_dir.path(), [1; 32], None);
    write_prm(
        prm_dir.path(),
        [2; 32],
        glossy_material.then_some(GLOSSY_TEXEL),
    );
    LitFloorRoom {
        geometry,
        lightmap: LightmapSection {
            direction_texel_scale: 1,
            irradiance_format: IRRADIANCE_FORMAT_RGBA16F,
            mode: LightmapMode::Shadowed,
            blocks: vec![uniform_block(8, 8, 1.0), uniform_block(8, 8, 0.0)],
        },
        keys: HashMap::from([
            ("matte".to_string(), [1u8; 32]),
            ("glossy".to_string(), [2u8; 32]),
        ]),
        prm_dir,
        extras: Extras {
            glossy_reflections: gate,
            ..Extras::default()
        },
    }
}

/// One glossy floor over the whole room.
fn glossy_floor(gate: bool) -> LitFloorRoom {
    room(&[([-3.0, -3.0, 3.0, 3.0], GLOSSY)], true, gate)
}

fn no_water() -> EnvironmentVolumesSection {
    EnvironmentVolumesSection::default()
}

fn bake_under(room: &LitFloorRoom, reflectors: Reflectors) -> ReflectionProbesSection {
    with_inputs(room, &no_water(), |inputs| {
        bake_or_load_with_reflectors(inputs, reflectors).unwrap()
    })
    .expect("the glossy floor keeps a probe")
}

/// Samples of the floor rectangle `(x0, z0, x1, z1)`: a 24 × 24 grid inset
/// 2 cm from its edges.
fn floor_samples([x0, z0, x1, z1]: [f32; 4]) -> Vec<Vec3> {
    let n = 24;
    let inset = 0.02;
    let at =
        |a: f32, b: f32, i: usize| a + inset + (b - a - 2.0 * inset) * i as f32 / (n - 1) as f32;
    (0..n)
        .flat_map(|i| (0..n).map(move |j| Vec3::new(at(x0, x1, i), 0.0, at(z0, z1, j))))
        .collect()
}

/// The runtime reference's capture-shadow reading of `layer` at floor
/// points: each point's `h` (≥ 0 is behind the surface the capture stored
/// toward it) and the probe term's weight along a ray rising off the floor.
fn shadow_reads(
    section: &ReflectionProbesSection,
    layer: usize,
    points: &[Vec3],
) -> Vec<(f32, f32)> {
    let record = &section.probes[layer];
    let capture = Vec3::from(record.capture_point);
    let (box_min, box_max) = (Vec3::from(record.box_min), Vec3::from(record.box_max));
    let dimension = section.tile_dimension;
    let tile = [dimension, section.tile_border];
    let texels: Vec<f32> = section
        .distance_tile(layer)
        .as_chunks::<2>()
        .0
        .iter()
        .map(|bits| f16_bits_to_f32(u16::from_le_bytes(*bits)))
        .collect();
    let distance = |uv: Vec2| reflection_distance_sample(&texels, dimension, uv);
    let ray = Vec3::new(0.6, 0.5, 0.3).normalize();
    points
        .iter()
        .map(|&point| {
            let h = reflection_search_h(point, capture, tile, &distance);
            let search = reflection_search(point, ray, box_min, box_max, capture, tile, distance);
            let weight = if search.status == FluidReflectionSearchStatus::CaptureShadow {
                0.0
            } else {
                search.weight
            };
            (h, weight)
        })
        .collect()
}

/// The points among `points` the runtime reference withholds, any part of
/// the probe term, as capture shadow.
fn shadowed(section: &ReflectionProbesSection, points: &[Vec3]) -> Vec<Vec3> {
    points
        .iter()
        .zip(shadow_reads(section, 0, points))
        .filter(|(_, (h, weight))| *h >= 0.0 || *weight < 1.0)
        .map(|(&point, _)| point)
        .collect()
}

// Gate row G1, compiler half: without the flag a glossy material changes
// nothing in id 54.
#[test]
fn without_the_gate_a_glossy_floor_bakes_id_54_as_a_matte_one_and_binds_no_world_face() {
    let water = pool();
    let bake = |glossy_material: bool| {
        let room = room(&[([-3.0, -3.0, 3.0, 3.0], GLOSSY)], glossy_material, false);
        with_inputs(&room, &water, |inputs| {
            bake_or_load_reflection_probes(inputs, None, &BakeControl::unrestricted()).unwrap()
        })
        .expect("the pool keeps its probe")
    };
    let (glossy, matte) = (bake(true), bake(false));
    assert!(glossy.world_bindings.is_empty());
    assert_eq!(glossy.probes.len(), 1, "the pool's probe alone");
    assert_eq!(glossy.to_bytes(), matte.to_bytes());

    // A glossy floor with no water and no gate keeps no probe at all.
    let dry = glossy_floor(false);
    let section = with_inputs(&dry, &no_water(), |inputs| {
        bake_or_load_reflection_probes(inputs, None, &BakeControl::unrestricted()).unwrap()
    });
    assert_eq!(section, None);
}

#[test]
fn with_the_gate_a_glossy_floor_binds_one_derived_probe_lifted_2_mm_over_it() {
    let section = bake_under(&glossy_floor(true), Reflectors::SelfTransparent);
    assert_eq!(section.probes.len(), 1);
    assert!(section.fluid_bindings.is_empty());
    assert_eq!(
        section.world_bindings,
        vec![ReflectionProbeWorldBinding { face: 0, probe: 0 }]
    );
    let probe = &section.probes[0];
    assert_eq!(probe.source_entity, placement::WORLD_SOURCE_ENTITY);
    let capture = Vec3::from(probe.capture_point);
    assert!(
        (capture - Vec3::new(0.0, 0.002, 0.0)).abs().max_element() < 1e-6,
        "{capture}"
    );
    // The fit finds the room around the floor.
    assert!((Vec3::from(probe.box_min) - Vec3::new(-3.0, 0.0, -3.0)).length() < 0.02);
    assert!((Vec3::from(probe.box_max) - Vec3::new(3.0, 3.0, 3.0)).length() < 0.02);
}

// Row 10, floor half: the capture point sits 2 mm over the floor, so with the
// floor in its capture every texel at the horizon reads it a few centimetres
// away and the floor's own fragments fall in capture shadow.
#[test]
fn a_glossy_floor_probe_places_no_fragment_of_its_own_floor_in_capture_shadow() {
    let room = glossy_floor(true);
    let points = floor_samples([-3.0, -3.0, 3.0, 3.0]);
    let transparent = bake_under(&room, Reflectors::SelfTransparent);
    assert_eq!(shadowed(&transparent, &points), Vec::<Vec3>::new());

    let opaque = bake_under(&room, Reflectors::Opaque);
    let dark = shadowed(&opaque, &points).len();
    assert!(
        dark * 2 > points.len(),
        "the self-occluded capture withholds most of the floor: {dark} of {}",
        points.len()
    );
}

// Pin P2: a glossy inlay flush in a floor whose material does not reflect.
#[test]
fn a_flush_glossy_inlay_in_a_matte_floor_reflects_over_its_whole_area() {
    let inlay = [-1.0, -1.0, 1.0, 1.0];
    let room = room(
        &[
            (inlay, GLOSSY),
            ([-3.0, -3.0, 3.0, -1.0], MATTE),
            ([-3.0, 1.0, 3.0, 3.0], MATTE),
            ([-3.0, -1.0, -1.0, 1.0], MATTE),
            ([1.0, -1.0, 3.0, 1.0], MATTE),
        ],
        true,
        true,
    );
    let section = bake_under(&room, Reflectors::SelfTransparent);
    assert_eq!(
        section.world_bindings,
        vec![ReflectionProbeWorldBinding { face: 0, probe: 0 }],
        "the inlay alone binds"
    );
    assert_eq!(
        shadowed(&section, &floor_samples(inlay)),
        Vec::<Vec3>::new()
    );
}

// Pin P2: a matte strip crossing a glossy floor, coplanar with it. Both
// halves share one probe, so the strip lies between the capture point and
// the far half; it is in the reflector's plane and casts no capture shadow.
#[test]
fn a_coplanar_matte_strip_casts_no_capture_shadow_on_the_glossy_floor_beyond_it() {
    let (west, east) = ([-3.0, -3.0, -0.25, 3.0], [0.25, -3.0, 3.0, 3.0]);
    let room = room(
        &[
            (west, GLOSSY),
            ([-0.25, -3.0, 0.25, 3.0], MATTE),
            (east, GLOSSY),
        ],
        true,
        true,
    );
    let section = bake_under(&room, Reflectors::SelfTransparent);
    assert_eq!(
        section.world_bindings,
        vec![
            ReflectionProbeWorldBinding { face: 0, probe: 0 },
            ReflectionProbeWorldBinding { face: 2, probe: 0 },
        ],
        "a matte strip never splits a floor's probe"
    );
    let capture = Vec3::from(section.probes[0].capture_point);
    assert!(
        capture.x < -0.25,
        "the point moves onto the west half: {capture}"
    );
    let mut points = floor_samples(west);
    points.extend(floor_samples(east));
    assert_eq!(shadowed(&section, &points), Vec::<Vec3>::new());

    // With only its bound faces transparent, the strip shadows the far half.
    let bound_only = bake_under(&room, Reflectors::BoundOnly);
    let dark = shadowed(&bound_only, &floor_samples(east));
    assert!(!dark.is_empty(), "the strip shadows the east half");
    assert!(dark.iter().all(|p| p.x > 0.25));
}

// Gate row G2: the flag keys the stage, even where placement is unchanged.
#[test]
fn toggling_the_glossy_gate_misses_the_stage_cache_and_an_unchanged_gate_hits() {
    let dir = tempfile::tempdir().unwrap();
    let cache = StageCache::new(dir.path()).unwrap();
    let control = BakeControl::unrestricted();
    let water = pool();
    // A matte floor: the gate places nothing more, yet still keys the stage.
    let bake = |gate: bool| {
        let room = room(&[([-3.0, -3.0, 3.0, 3.0], MATTE)], true, gate);
        with_inputs(&room, &water, |inputs| {
            bake_or_load_reflection_probes(inputs, Some(&cache), &control).unwrap()
        })
    };
    let off = bake(false);
    let on = bake(true);
    let again = bake(true);
    let access = cache.test_access(STAGE_ID);
    assert_eq!((access.writes, access.read_hits), (2, 1));
    assert_eq!(off, on, "a matte floor places the same probes either way");
    assert_eq!(on, again);
}

// A world probe takes only the room water leaves: water keeps every probe it
// had, and the glossy probe past the cap drops with a warning.
#[test]
fn a_glossy_world_probe_never_takes_a_water_probe_slot_at_the_cap() {
    let room = glossy_floor(true);
    let water = pool();
    let logs = LogCapture::start();
    let probes = with_inputs(&room, &water, |inputs| {
        let tracer = SceneTracer::new(inputs.bvh, inputs.primitives, inputs.geometry);
        let bounds = WorldBounds::from_positions(
            room.geometry.geometry.vertices.iter().map(|v| &v.position),
        )
        .unwrap();
        let classifier = PointClassifier::new(inputs.tree, inputs.exterior_leaves, &water);
        let control = BakeControl::unrestricted();
        let world_faces = glossy_placement::glossy_world_faces(inputs, control.governor());
        assert_eq!(world_faces.len(), 1);
        let place = |cap| {
            place_probes(
                &water,
                inputs.reflection_volumes,
                &world_faces,
                &PlacementScene {
                    tracer: &tracer,
                    bounds: &bounds,
                    classifier: &classifier,
                    governor: control.governor(),
                },
                cap,
            )
            .unwrap()
        };
        (place(2), place(1))
    });
    let (room_for_both, at_cap) = probes;
    assert_eq!(room_for_both.len(), 2);
    assert_eq!(
        (
            room_for_both[0].fluid_faces.len(),
            room_for_both[1].world_faces.clone()
        ),
        (1, vec![0])
    );
    assert_eq!(at_cap.len(), 1);
    assert_eq!(at_cap[0].fluid_faces.len(), 1, "water keeps its probe");
    logs.assert_logged_once(
        LogLevel::Warn,
        "1 glossy world probe(s) past the 1-probe cap dropped",
    );
}
