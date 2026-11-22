//! Area glow stamping over a real partition: emitting faces per light, split
//! sides, frame faces, compose slots in namespace order, static and animated
//! lights, the face cap and maps without area lights.

use glam::DVec3;
use postretro_level_format::area_glow::{AreaGlowSection, MAX_AREA_GLOW_FACES};
use postretro_level_format::geometry::{FaceMeta, GeometrySection};
use postretro_level_format::texture_names::TextureNamesSection;
use postretro_render_cpu::emissive_volume::{area_glow_entry, area_glow_factor};
use postretro_render_cpu::sh_compose::{AnimatedLightScaleDescriptor, animated_light_scale};

use super::*;
use crate::geometry::{FaceIndexRange, extract_geometry_with_brushes};
use crate::light_namespaces::AnimatedBakedLights;
use crate::map_data::MapData;
use crate::map_format::MapFormat;
use crate::pipeline::runtime_cells::{OccluderPartition, RuntimeCells, ShippedCells};

/// Quake map units to engine meters.
const UNIT: f64 = 0.0254;
const GLOW: &str = "glow";
const FRAME: &str = "frame";

fn box_brush(min: [i32; 3], max: [i32; 3], texture: &str) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        r#"{{
( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {texture} 0 0 0 1 1
( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {texture} 0 0 0 1 1
( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {texture} 0 0 0 1 1
( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {texture} 0 0 0 1 1
( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {texture} 0 0 0 1 1
( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {texture} 0 0 0 1 1
}}"#
    )
}

/// A sealed 256² × 128 room (interior map units −128..128, 0..128).
/// `worldspawn_extra` brushes join worldspawn; `entities` follow it.
fn room(worldspawn_extra: &[String], entities: &[String]) -> String {
    let mut brushes = vec![
        box_brush([-144, -144, -16], [144, 144, 0], "floor"),
        box_brush([-144, -144, 128], [144, 144, 144], "wall"),
        box_brush([-144, -144, 0], [-128, 144, 128], "wall"),
        box_brush([128, -144, 0], [144, 144, 128], "wall"),
        box_brush([-128, -144, 0], [128, -128, 128], "wall"),
        box_brush([-128, 128, 0], [128, 144, 128], "wall"),
    ];
    brushes.extend(worldspawn_extra.iter().cloned());
    let mut all = vec![format!(
        "{{\n\"classname\" \"worldspawn\"\n{}\n}}",
        brushes.join("\n")
    )];
    all.extend(entities.iter().cloned());
    all.join("\n")
}

/// A `light_area` holding `brushes`, with extra `"key" "value"` lines.
fn area_light(keys: &str, brushes: &[String]) -> String {
    format!(
        "{{\n\"classname\" \"light_area\"\n\"light\" \"300\"\n{keys}{}\n}}",
        brushes.join("\n")
    )
}

/// A glowing glass slab hanging from the ceiling over the map-unit x and y
/// ranges.
fn glass(x: [i32; 2], y: [i32; 2]) -> String {
    box_brush([x[0], y[0], 120], [x[1], y[1], 128], GLOW)
}

struct World {
    map: MapData,
    geometry: crate::geometry::GeometryResult,
    face_brushes: Vec<Option<u32>>,
}

impl World {
    fn new(map_text: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "postretro-area-glow-{}-{id}.map",
            std::process::id()
        ));
        std::fs::write(&path, format!("{map_text}\n")).expect("write fixture map");
        let map = crate::parse::parse_map_file(&path, MapFormat::IdTech2);
        let _ = std::fs::remove_file(&path);
        let map = map.unwrap_or_else(|error| panic!("parse fixture map: {error:#}"));
        let ShippedCells {
            partition: OccluderPartition { result, .. },
            runtime: RuntimeCells { cells, .. },
            ..
        } = ShippedCells::build(&map.brush_volumes, &map.kinematic_movers).expect("partition");
        let (geometry, face_brushes) =
            extract_geometry_with_brushes(&result.faces, &cells.tree, &cells.exterior_cells);
        Self {
            map,
            geometry,
            face_brushes,
        }
    }

    fn has_emissive_map(&self) -> Vec<bool> {
        self.geometry
            .texture_names
            .names
            .iter()
            .map(|name| name == GLOW)
            .collect()
    }

    fn slots(&self) -> Vec<u32> {
        AnimatedBakedLights::from_lights(&self.map.lights)
            .slot_for_source_lights(self.map.lights.len())
    }

    fn section(&self) -> Option<AreaGlowSection> {
        build_area_glow_section(
            &self.map.area_lights,
            &self.map.lights,
            &self.slots(),
            &self.geometry,
            &self.face_brushes,
            &self.has_emissive_map(),
        )
        .expect("area glow builds")
    }

    /// Faces of `texture` whose every vertex lies in the map-unit box.
    fn faces_within(&self, texture: &str, min: [f64; 3], max: [f64; 3]) -> Vec<u32> {
        let engine = |q: [f64; 3]| crate::parse::quake_to_engine(DVec3::from(q)) * UNIT;
        let (lo, hi) = (engine(min), engine(max));
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        let slack = DVec3::splat(1e-4);
        let geometry = &self.geometry;
        (0..geometry.geometry.faces.len())
            .filter(|&face| {
                let meta = &geometry.geometry.faces[face];
                let range = &geometry.face_index_ranges[face];
                let start = range.index_offset as usize;
                geometry.texture_names.names[meta.texture_index as usize] == texture
                    && geometry.geometry.indices[start..start + range.index_count as usize]
                        .iter()
                        .all(|&vertex| {
                            let p = DVec3::from(
                                geometry.geometry.vertices[vertex as usize]
                                    .position
                                    .map(f64::from),
                            );
                            p.cmpge(lo - slack).all() && p.cmple(hi + slack).all()
                        })
            })
            .map(|face| face as u32)
            .collect()
    }

    /// Every face of `texture` whose brush belongs to area light `light`.
    fn light_faces(&self, light: usize, texture: &str) -> Vec<u32> {
        let brushes = &self.map.area_lights[light].brush_indices;
        (0..self.geometry.geometry.faces.len())
            .filter(|&face| {
                let meta = &self.geometry.geometry.faces[face];
                self.geometry.texture_names.names[meta.texture_index as usize] == texture
                    && self.face_brushes[face].is_some_and(|b| brushes.contains(&(b as usize)))
            })
            .map(|face| face as u32)
            .collect()
    }
}

// Acceptance row 28 (research.md O12): a glowing side the BSP splits glows on
// every piece, and a frame face sharing an edge with the glass carries no
// glow id. The light is a glass floor beside a frame strip, under two rooms a
// worldspawn wall divides, so the BSP splits the glass's top into a fragment
// set per room.
#[test]
fn a_split_side_glows_on_every_piece_and_the_frame_carries_no_glow() {
    let walls = [
        box_brush([-144, -144, 128], [144, 144, 144], "wall"),
        box_brush([-144, -144, 0], [-128, 144, 128], "wall"),
        box_brush([128, -144, 0], [144, 144, 128], "wall"),
        box_brush([-128, -144, 0], [128, -128, 128], "wall"),
        box_brush([-128, 128, 0], [128, 144, 128], "wall"),
        box_brush([-8, -128, 0], [8, 128, 128], "wall"),
    ];
    let map = format!(
        "{{\n\"classname\" \"worldspawn\"\n{}\n}}\n{}",
        walls.join("\n"),
        area_light(
            "",
            &[
                box_brush([-144, -144, -16], [96, 144, 0], GLOW),
                box_brush([96, -144, -16], [144, 144, 0], FRAME),
            ],
        )
    );
    let world = World::new(&map);
    let top = world.faces_within(GLOW, [-128.0, -128.0, 0.0], [96.0, 128.0, 0.0]);
    let west = world.faces_within(GLOW, [-128.0, -128.0, 0.0], [-8.0, 128.0, 0.0]);
    let east = world.faces_within(GLOW, [8.0, -128.0, 0.0], [96.0, 128.0, 0.0]);
    assert!(
        !west.is_empty() && !east.is_empty() && west.len() + east.len() == top.len(),
        "the BSP splits the glass top into pieces on both sides: {west:?} {east:?}"
    );
    let underside = top;
    let section = world.section().expect("the map has an area light");
    let [light] = section.lights.as_slice() else {
        panic!("one light: {:?}", section.lights);
    };
    let glass = world.light_faces(0, GLOW);
    assert_eq!(light.faces, glass, "every glowing piece, and only those");
    for face in &underside {
        assert!(light.faces.contains(face), "underside piece {face}");
    }
    let frame = world.light_faces(0, FRAME);
    assert!(!frame.is_empty());
    for face in &frame {
        assert!(
            !light.faces.contains(face),
            "frame face {face} carries no glow"
        );
    }
    assert_eq!(light.glow, 4.0, "the default `_glow`");
    assert_eq!(
        light.compose_slot, None,
        "a static light draws its static glow"
    );

    // Membership's skip covers every face of the light's brushes, frame too.
    let area_faces = area_light_faces(&world.map.area_lights, &world.face_brushes);
    for face in glass.iter().chain(&frame) {
        assert!(area_faces[*face as usize]);
    }
    assert_eq!(
        area_faces.iter().filter(|&&area| area).count(),
        glass.len() + frame.len()
    );
}

// Acceptance row 58 (research.md O4): behind an animated point light, each
// animated area light's faces name that light's own compose slot, a
// `_bake_only` one included, in AnimatedBakedLights order.
#[test]
fn each_area_light_names_its_own_compose_slot() {
    let point = "{\n\"classname\" \"light\"\n\"origin\" \"0 0 64\"\n\"light\" \"200\"\n\
                 \"_falloff_range\" \"200\"\n\"_animated\" \"1\"\n}"
        .to_string();
    let world = World::new(&room(
        &[],
        &[
            point,
            area_light("\"_animated\" \"1\"\n", &[glass([-120, -80], [-120, -80])]),
            area_light(
                "\"_animated\" \"1\"\n\"_bake_only\" \"1\"\n",
                &[glass([-20, 20], [-20, 20])],
            ),
            area_light("\"_animated\" \"1\"\n", &[glass([80, 120], [80, 120])]),
        ],
    ));
    let section = world.section().unwrap();
    let slots: Vec<Option<u32>> = section.lights.iter().map(|l| l.compose_slot).collect();
    assert_eq!(slots, [Some(1), Some(2), Some(3)]);
    for (index, light) in section.lights.iter().enumerate() {
        assert!(!light.faces.is_empty());
        assert_eq!(light.faces, world.light_faces(index, GLOW));
        assert_eq!(
            world.slots()[world.map.area_lights[index].light_index],
            light.compose_slot.unwrap()
        );
    }
}

// Acceptance row 59 (research.md O16): a static area light authored ahead of
// an animated one has no slot and keeps a steady glow at `_glow × _color`
// while the animated light, holding slot 0, pulses.
#[test]
fn a_static_area_light_ahead_of_an_animated_one_keeps_a_steady_glow() {
    let world = World::new(&room(
        &[],
        &[
            area_light(
                "\"_color\" \"255 0 0\"\n\"_glow\" \"3\"\n",
                &[glass([-120, -80], [-120, -80])],
            ),
            area_light(
                "\"brightness_curve\" \"[0, 0.1] [500, 1.0] [1000, 0.1]\"\n\"period_ms\" \"1000\"\n\
                 \"_animated\" \"1\"\n",
                &[glass([80, 120], [80, 120])],
            ),
        ],
    ));
    let section = world.section().unwrap();
    let [steady, pulsing] = section.lights.as_slice() else {
        panic!("two lights");
    };
    assert_eq!(steady.compose_slot, None);
    assert_eq!(pulsing.compose_slot, Some(0));

    let animated = &world.map.lights[world.map.area_lights[1].light_index];
    let baked = crate::sh_bake::animation_descriptor_for_light(animated);
    let descriptor = AnimatedLightScaleDescriptor {
        period: baked.period,
        phase: baked.phase,
        base_color: baked.base_color,
        brightness: &baked.brightness,
        color: &[],
        is_active: baked.start_active != 0,
    };
    let steady_entry = area_glow_entry(steady);
    let pulsing_entry = area_glow_entry(pulsing);
    let mut pulse = Vec::new();
    for step in 0..16 {
        let time = step as f32 / 16.0 * baked.period;
        assert_eq!(
            area_glow_factor(steady_entry, Some(descriptor), time),
            [3.0, 0.0, 0.0],
            "the steady light ignores every descriptor"
        );
        pulse.push(area_glow_factor(pulsing_entry, Some(descriptor), time)[0]);
    }
    let low = pulse.iter().copied().fold(f32::MAX, f32::min);
    let high = pulse.iter().copied().fold(f32::MIN, f32::max);
    assert!(
        low < 1.0 && high > 3.9,
        "the animated light pulses: {pulse:?}"
    );
}

// Acceptance row 55 (glow part, research.md O3): `_start_inactive 1` with no
// `_animated` key reserves a compose slot, so its faces start dark with an
// unlit room; once script membership starts it active (a `levelLoad`
// pulse), faces and room are lit from the first frame, together.
#[test]
fn a_start_inactive_area_light_starts_dark_and_a_pulse_starts_it_lit() {
    let mut world = World::new(&room(
        &[],
        &[area_light(
            "\"_start_inactive\" \"1\"\n",
            &[glass([-20, 20], [-20, 20])],
        )],
    ));
    let section = world.section().unwrap();
    assert_eq!(section.lights[0].compose_slot, Some(0), "reserved");
    let entry = area_glow_entry(&section.lights[0]);
    let index = world.map.area_lights[0].light_index;

    let glow_and_room = |light: &crate::map_data::MapLight| {
        let baked = crate::sh_bake::animation_descriptor_for_light(light);
        let descriptor = AnimatedLightScaleDescriptor {
            period: baked.period,
            phase: baked.phase,
            base_color: baked.base_color,
            brightness: &baked.brightness,
            color: &[],
            is_active: baked.start_active != 0,
        };
        (
            area_glow_factor(entry, Some(descriptor), 0.0),
            animated_light_scale(Some(descriptor), 0.0),
        )
    };
    let (glow, room) = glow_and_room(&world.map.lights[index]);
    assert_eq!((glow, room), ([0.0; 3], [0.0; 3]), "dark faces, unlit room");

    // A `levelLoad` pulse's membership record starts it active.
    let manifest = postretro_level_format::light_membership::LightMembershipManifest::new(
        vec![
            postretro_level_format::light_membership::LightMembershipRecord {
                index: index as u32,
                is_dynamic: false,
                start_active: Some(true),
                start_active_conflict: false,
            },
        ],
        Vec::new(),
    );
    let defaults = world.map.light_start_active_defaults.clone();
    crate::script_light_membership::apply_manifest(&mut world.map.lights, &defaults, &manifest)
        .expect("manifest applies");
    let (glow, room) = glow_and_room(&world.map.lights[index]);
    let intensity = world.map.lights[index].intensity;
    for channel in 0..3 {
        assert!((glow[channel] - 4.0).abs() < 1e-5, "{glow:?}");
        assert!((room[channel] - intensity).abs() < 1e-6, "{room:?}");
    }
}

// Acceptance row 60 (research.md O5): a map with no area light writes no
// section, and no face is skipped by volume membership.
#[test]
fn a_map_without_area_lights_writes_no_section() {
    let world = World::new(&room(&[box_brush([-16, -16, 0], [16, 16, 8], GLOW)], &[]));
    assert!(world.section().is_none());
    assert!(area_light_faces(&world.map.area_lights, &world.face_brushes).is_empty());
}

/// `count` one-triangle faces, all of texture 0 (glowing) and brush 0.
fn many_faces(count: usize) -> (crate::geometry::GeometryResult, Vec<Option<u32>>) {
    let geometry = crate::geometry::GeometryResult {
        geometry: GeometrySection {
            vertices: Vec::new(),
            indices: Vec::new(),
            faces: vec![
                FaceMeta {
                    leaf_index: 0,
                    texture_index: 0,
                };
                count
            ],
        },
        texture_names: TextureNamesSection {
            names: vec![GLOW.to_string()],
        },
        face_index_ranges: (0..count as u32)
            .map(|face| FaceIndexRange {
                index_offset: face * 3,
                index_count: 3,
            })
            .collect(),
    };
    (geometry, vec![Some(0); count])
}

fn one_area_light(intensity: f32, animated: bool) -> (Vec<MapAreaLight>, Vec<MapLight>, Vec<u32>) {
    let mut light = crate::area_light::test_lights::area_light(
        vec![crate::area_light::test_lights::ceiling_panel(
            [-1.0, 1.0],
            [-1.0, 1.0],
            3.0,
        )],
        intensity,
        Some(4.0),
    );
    light.color = [1.0, 0.5, 0.25];
    let slots = if animated {
        vec![0]
    } else {
        vec![ANIMATED_SLOT_NONE]
    };
    (
        vec![MapAreaLight {
            light_index: 0,
            brush_indices: vec![0],
            reach_override: None,
            glow: 8.0,
        }],
        vec![light],
        slots,
    )
}

// Acceptance row 60 (research.md O5): past the glow table's cap the build
// fails, naming the count and the cap; at the cap it builds.
#[test]
fn emitting_faces_past_the_cap_fail_naming_the_count_and_the_cap() {
    let (areas, lights, slots) = one_area_light(1.0, false);
    let (geometry, brushes) = many_faces(MAX_AREA_GLOW_FACES);
    let section = build_area_glow_section(&areas, &lights, &slots, &geometry, &brushes, &[true])
        .unwrap()
        .unwrap();
    assert_eq!(section.face_count(), MAX_AREA_GLOW_FACES);

    let (geometry, brushes) = many_faces(MAX_AREA_GLOW_FACES + 1);
    let error = build_area_glow_section(&areas, &lights, &slots, &geometry, &brushes, &[true])
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("3840 emitting faces") && error.contains("cap of 3839"),
        "{error}"
    );
}

// A slotted light carries its authored facts; `light 0` takes the static
// glow, since there is no intensity to divide out.
#[test]
fn records_carry_glow_colour_intensity_and_a_slot_only_when_lit() {
    let (geometry, brushes) = many_faces(2);
    let (areas, lights, slots) = one_area_light(1.5, true);
    let section = build_area_glow_section(&areas, &lights, &slots, &geometry, &brushes, &[true])
        .unwrap()
        .unwrap();
    let light = &section.lights[0];
    assert_eq!(
        (light.glow, light.color, light.intensity, light.compose_slot),
        (8.0, [1.0, 0.5, 0.25], 1.5, Some(0))
    );
    assert_eq!(light.faces, [0, 1]);

    let (areas, lights, slots) = one_area_light(0.0, true);
    let section = build_area_glow_section(&areas, &lights, &slots, &geometry, &brushes, &[true])
        .unwrap()
        .unwrap();
    assert_eq!(section.lights[0].compose_slot, None);

    // A face with no `_e` map, or no triangle, carries no glow.
    let (mut geometry, brushes) = many_faces(2);
    geometry.face_index_ranges[1].index_count = 0;
    let (areas, lights, slots) = one_area_light(1.0, false);
    let error = build_area_glow_section(&areas, &lights, &slots, &geometry, &brushes, &[true])
        .unwrap_err()
        .to_string();
    assert!(error.contains("world face 1 has no triangle"), "{error}");
    let section = build_area_glow_section(&areas, &lights, &slots, &geometry, &brushes, &[false])
        .unwrap()
        .unwrap();
    assert!(section.lights[0].faces.is_empty());
}
