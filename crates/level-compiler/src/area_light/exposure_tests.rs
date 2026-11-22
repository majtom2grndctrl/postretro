//! Area-light emitters through the real parser, BSP and `_e` lookup: which
//! faces emit, how split, overlapping and buried sides count, slivers, reach,
//! entity order, the buried and embedded skips, and the unshadowed lightmap.

use std::path::PathBuf;

use glam::{DVec3, Vec3};
use log::Level;
use postretro_level_format::alpha_lights::{
    ALPHA_LIGHT_LEAF_UNASSIGNED, AlphaFalloffModel, AlphaLightType, AlphaShadowType,
};
use postretro_test_log_capture::LogCapture;

use super::*;
use crate::area_light::{AreaEmitter, polygon_distance, polygon_irradiance, reach_window};
use crate::buried_lights::BuriedLights;
use crate::fixture_pipeline::{FixturePipeline, load_fixture, load_map};
use crate::light_namespaces::{AlphaLightsNs, StaticBakedLights};
use crate::map_data::{LightType, MapLight};
use crate::map_format::MapFormat;
use crate::parse::{parse_map_file, quake_to_engine};
use crate::partition::{BspTree, find_leaf_for_point};
use crate::pipeline::runtime_cells::{
    OccluderPartition, full_exterior_flood, partition_with_occluders,
};
use crate::texel_footprint::has_emitter_sphere;

/// Quake map units to engine metres.
const UNIT: f64 = 0.0254;
const GLASS: &str = "glass/pane";
const WALL: &str = "wall/plain";

/// An engine-space point from Quake map units.
fn engine(x: f64, y: f64, z: f64) -> DVec3 {
    quake_to_engine(DVec3::new(x, y, z)) * UNIT
}

/// An area in Quake units², in m².
fn square_metres(units: f64) -> f64 {
    units * UNIT * UNIT
}

/// An axis-aligned box brush, faces textured `[-x, +x, -y, +y, -z, +z]`.
fn brush_faces(min: [i32; 3], max: [i32; 3], tex: [&str; 6]) -> String {
    let ([x0, y0, z0], [x1, y1, z1]) = (min, max);
    format!(
        "{{\n\
         ( {x0} 0 0 ) ( {x0} 1 0 ) ( {x0} 0 1 ) {} 0 0 0 1 1\n\
         ( {x1} 0 0 ) ( {x1} 0 1 ) ( {x1} 1 0 ) {} 0 0 0 1 1\n\
         ( 0 {y0} 0 ) ( 0 {y0} 1 ) ( 1 {y0} 0 ) {} 0 0 0 1 1\n\
         ( 0 {y1} 0 ) ( 1 {y1} 0 ) ( 0 {y1} 1 ) {} 0 0 0 1 1\n\
         ( 0 0 {z0} ) ( 1 0 {z0} ) ( 0 1 {z0} ) {} 0 0 0 1 1\n\
         ( 0 0 {z1} ) ( 0 1 {z1} ) ( 1 0 {z1} ) {} 0 0 0 1 1\n\
         }}\n",
        tex[0], tex[1], tex[2], tex[3], tex[4], tex[5]
    )
}

fn brush(min: [i32; 3], max: [i32; 3], tex: &str) -> String {
    brush_faces(min, max, [tex; 6])
}

/// A sealed room, interior 0..256 × 0..256 × 0..128, with `world` extra
/// worldspawn brushes and `entities` after it.
fn room_map(world: &str, entities: &str) -> String {
    let shell = [
        brush([-16, -16, -16], [272, 272, 0], WALL),
        brush([-16, -16, 128], [272, 272, 144], WALL),
        brush([-16, -16, 0], [0, 272, 128], WALL),
        brush([256, -16, 0], [272, 272, 128], WALL),
        brush([0, -16, 0], [256, 0, 128], WALL),
        brush([0, 256, 0], [256, 272, 128], WALL),
    ]
    .concat();
    format!(
        "{{\n\"classname\" \"worldspawn\"\n{shell}{world}}}\n{entities}\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"32 32 24\"\n}}\n"
    )
}

fn area_entity(keys: &str, brushes: &str) -> String {
    format!("{{\n\"classname\" \"light_area\"\n{keys}{brushes}}}\n")
}

/// A content root in a temp dir: `textures/glass/pane.png` with its `_e`
/// sibling (whose presence alone marks an emitting face; nothing decodes it)
/// and `textures/wall/plain.png` without one, and the map in `maps/`.
struct Content {
    _dir: tempfile::TempDir,
    map: PathBuf,
}

fn content(map_text: &str) -> Content {
    let dir = tempfile::tempdir().expect("temp dir");
    for (collection, file) in [
        ("glass", "pane.png"),
        ("glass", "pane_e.png"),
        ("wall", "plain.png"),
    ] {
        let folder = dir.path().join("textures").join(collection);
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join(file), b"").unwrap();
    }
    let maps = dir.path().join("maps");
    std::fs::create_dir_all(&maps).unwrap();
    let map = maps.join("area.map");
    std::fs::write(&map, map_text).unwrap();
    Content { _dir: dir, map }
}

/// Parse, partition and resolve as `pipeline.rs` does, keeping the result.
fn resolve(map_text: &str) -> anyhow::Result<(MapData, Vec<Face>, BspTree)> {
    let content = content(map_text);
    let mut map = parse_map_file(&content.map, MapFormat::IdTech2)?;
    let OccluderPartition { result, .. } =
        partition_with_occluders(&map.brush_volumes, &map.kinematic_movers)?;
    let lookup =
        crate::texture_mips::emissive_map_lookup(&crate::resolve_texture_root(&content.map));
    let (exterior, _) = full_exterior_flood(&result.tree);
    resolve_area_lights(&mut map, &result.faces, &result.tree, &exterior, lookup)?;
    Ok((map, result.faces, result.tree))
}

fn fixture(map_text: &str) -> FixturePipeline {
    let content = content(map_text);
    load_map(&content.map)
}

fn only_area(lights: &[MapLight]) -> (&MapLight, &AreaEmitter) {
    let mut areas = lights.iter().filter(|l| l.light_type == LightType::Area);
    let light = areas.next().expect("one area light");
    assert!(areas.next().is_none(), "exactly one area light");
    (light, light.area.as_ref().expect("its emitter resolved"))
}

fn assert_close(actual: f64, expected: f64, relative: f64, what: &str) {
    let scale = expected.abs().max(1.0e-12);
    assert!(
        (actual - expected).abs() <= relative * scale,
        "{what}: {actual} vs {expected} (relative {relative})"
    );
}

/// A ceiling panel under the 128-unit ceiling, `glass` boxes flush below it.
fn ceiling_panel(glass: &[([i32; 3], [i32; 3])], keys: &str) -> String {
    let brushes: String = glass
        .iter()
        .map(|&(min, max)| brush(min, max, GLASS))
        .collect();
    room_map("", &area_entity(keys, &brushes))
}

// Acceptance row 12: in one entity a frame brush and an `_e` glass brush;
// only the glass's exposed faces emit, and both brushes are solid world.
#[test]
fn only_the_glass_faces_of_a_framed_panel_emit_and_its_brushes_are_solid_world() {
    let frame = brush([80, 80, 124], [176, 176, 128], WALL);
    let glass = brush([96, 96, 120], [160, 160, 124], GLASS);
    let fixture = fixture(&room_map("", &area_entity("", &(frame + &glass))));
    let (light, emitter) = only_area(&fixture.lights);

    // Bottom 64 × 64 plus four 64 × 4 sides; its top meets the frame.
    assert_close(
        emitter.total_area,
        square_metres(64.0 * 64.0 + 4.0 * 64.0 * 4.0),
        1.0e-9,
        "glass area",
    );
    assert_eq!(emitter.polygons.len(), 5, "each exposed glass side, once");
    let (glass_min, glass_max) = (engine(160.0, 160.0, 120.0), engine(96.0, 96.0, 124.0));
    let (lo, hi) = (glass_min.min(glass_max), glass_min.max(glass_max));
    for polygon in &emitter.polygons {
        for v in &polygon.vertices {
            assert!(
                v.cmpge(lo - 1.0e-9).all() && v.cmple(hi + 1.0e-9).all(),
                "an emitting vertex {v} lies off the glass"
            );
        }
        assert!(
            polygon.normal.dot(DVec3::Y) < 0.5,
            "no polygon faces up into the frame"
        );
    }
    assert_eq!(light.origin, emitter.centroid);

    // Both brushes are BSP solids and draw: frame faces are world faces.
    for inside in [engine(128.0, 128.0, 122.0), engine(170.0, 170.0, 126.0)] {
        let leaf = find_leaf_for_point(&fixture.full_tree, inside);
        assert!(fixture.full_tree.leaves[leaf].is_solid, "{inside} is solid");
    }
    assert!(
        fixture.faces.iter().any(|face| face.texture == WALL
            && (face.normal - DVec3::NEG_Y).length() < 1.0e-9
            && (face.vertices[0].y - 124.0 * UNIT).abs() < 1.0e-9),
        "the frame's exposed underside is a world face"
    );
}

// Acceptance row 23 (research.md O6): four 2 × 2 brushes tiling one panel
// light every receiver as the one brush does, and resolve the same reach.
#[test]
fn a_panel_of_four_brushes_lights_every_receiver_as_one_brush_does() {
    let (one, _, _) = resolve(&ceiling_panel(&[([96, 96, 120], [160, 160, 128])], "")).unwrap();
    let four = [
        ([96, 96, 120], [128, 128, 128]),
        ([128, 96, 120], [160, 128, 128]),
        ([96, 128, 120], [128, 160, 128]),
        ([128, 128, 120], [160, 160, 128]),
    ];
    let (tiled, _, _) = resolve(&ceiling_panel(&four, "")).unwrap();
    let (one_light, one_emitter) = only_area(&one.lights);
    let (tiled_light, tiled_emitter) = only_area(&tiled.lights);
    assert!(tiled_emitter.polygons.len() > one_emitter.polygons.len());
    assert_close(
        tiled_emitter.total_area,
        one_emitter.total_area,
        1.0e-12,
        "area",
    );
    assert_close(
        f64::from(tiled_light.falloff_range),
        f64::from(one_light.falloff_range),
        1.0e-6,
        "reach",
    );
    let reach = f64::from(one_light.falloff_range);
    let receivers = [
        (engine(128.0, 128.0, 0.0), DVec3::Y),
        (engine(40.0, 200.0, 0.0), DVec3::Y),
        (engine(100.0, 100.0, 60.0), DVec3::Y),
        (engine(1.0, 128.0, 64.0), engine(1.0, 0.0, 0.0).normalize()),
        (
            engine(128.0, 255.0, 100.0),
            engine(0.0, -1.0, 0.0).normalize(),
        ),
        (
            engine(150.0, 120.0, 110.0),
            DVec3::new(0.3, 0.8, -0.2).normalize(),
        ),
    ];
    for (p, n) in receivers {
        let (a, _) = one_emitter.windowed_irradiance(p, n, reach);
        let (b, _) = tiled_emitter.windowed_irradiance(p, n, reach);
        assert!(a > 0.0, "receiver {p} is lit");
        assert_close(b, a, 1.0e-5, "receiver term");
    }
}

// Acceptance row 24 (research.md O7): overlapping coplanar `_e` brushes emit
// their union once.
#[test]
fn overlapping_coplanar_glass_emits_its_union_once() {
    let map = ceiling_panel(
        &[
            ([96, 96, 120], [160, 160, 128]),
            ([128, 96, 120], [192, 160, 128]),
        ],
        "",
    );
    let (map, _, _) = resolve(&map).unwrap();
    let (_, emitter) = only_area(&map.lights);
    // Union underside 96 × 64; its outline 2 × 96 + 2 × 64 long, 8 tall.
    assert_close(
        emitter.total_area,
        square_metres(96.0 * 64.0 + (2.0 * 96.0 + 2.0 * 64.0) * 8.0),
        1.0e-9,
        "union area",
    );
}

// Acceptance row 24 (research.md O7): a pane half sunk in a worldspawn wall
// emits from its exposed part only.
#[test]
fn a_pane_half_sunk_in_a_wall_emits_only_its_exposed_part() {
    // The east wall's face is x = 256; the glass reaches 8 units into it.
    let glass = brush([232, 96, 40], [264, 160, 72], GLASS);
    let (map, _, _) = resolve(&room_map("", &area_entity("", &glass))).unwrap();
    let (_, emitter) = only_area(&map.lights);
    // −x face 64 × 32 whole; ±y faces 24 × 32 and ±z faces 24 × 64 exposed.
    let exposed = 64.0 * 32.0 + 2.0 * 24.0 * 32.0 + 2.0 * 24.0 * 64.0;
    assert_close(
        emitter.total_area,
        square_metres(exposed),
        1.0e-9,
        "exposed area",
    );
    for polygon in &emitter.polygons {
        for v in &polygon.vertices {
            assert!(
                v.z >= engine(256.0, 0.0, 0.0).z - 1.0e-9,
                "vertex {v} inside the wall"
            );
        }
    }
}

// Acceptance row 25 (research.md O8): a sliver piece is dropped before the
// polygons are fixed; an entity left with slivers alone fails as having no
// `_e` face.
#[test]
fn sliver_pieces_are_dropped_and_a_sliver_only_light_fails() {
    let (mut map, faces, tree) =
        resolve(&ceiling_panel(&[([96, 96, 120], [160, 160, 128])], "")).unwrap();
    let area = map.area_lights[0].clone();
    let is_glass = |texture: &str| texture == GLASS;
    let own = |face: &Face| area.brush_indices.contains(&face.brush_index);
    // Shrink every emitting face of the light to a 0.5 mm square piece
    // about its first corner: 2.5e-7 m², under the floor.
    let sliver = |face: &Face| {
        let (u, v) = (
            (face.vertices[1] - face.vertices[0]).normalize() * 5.0e-4,
            face.normal
                .cross((face.vertices[1] - face.vertices[0]).normalize())
                * 5.0e-4,
        );
        let o = face.vertices[0];
        Face {
            vertices: vec![o, o + u, o + u + v, o + v],
            ..face.clone()
        }
    };
    let slivered: Vec<Face> = faces
        .iter()
        .map(|face| {
            if own(face) && is_glass(&face.texture) {
                sliver(face)
            } else {
                face.clone()
            }
        })
        .collect();
    let (exterior, _) = full_exterior_flood(&tree);
    let error = resolve_area_lights(&mut map, &slivered, &tree, &exterior, is_glass)
        .expect_err("slivers alone leave no emitter")
        .to_string();
    assert!(
        error.contains("entity 1 'light_area'") && error.contains("has no face with an `_e` map"),
        "{error}"
    );

    // Beside whole faces, the sliver adds nothing.
    let polygons = emitting_polygons(&map.brush_volumes, &faces, &area.brush_indices, &is_glass);
    let mut with_sliver = faces.clone();
    let first_glass = faces
        .iter()
        .find(|face| own(face) && is_glass(&face.texture))
        .unwrap();
    with_sliver.push(Face {
        brush_index: first_glass.brush_index,
        ..sliver(first_glass)
    });
    let total = |polygons: &[EmitterPolygon]| polygons.iter().map(|p| p.area).sum::<f64>();
    let kept = emitting_polygons(
        &map.brush_volumes,
        &with_sliver,
        &area.brush_indices,
        &is_glass,
    );
    assert!(kept.iter().all(|p| p.area >= EMITTER_PIECE_AREA_FLOOR_M2));
    // The sliver turned its side's pieces into a non-whole set, so that side
    // emits its pieces; the area is unchanged.
    assert_close(total(&kept), total(&polygons), 1.0e-9, "area with a sliver");
}

// Acceptance row 30: an entity whose faces carry no `_e` map fails, naming
// the light; a brushless one fails at parse.
#[test]
fn an_area_light_without_an_e_face_or_without_brushes_fails_naming_it() {
    let dull = brush([96, 96, 120], [160, 160, 128], WALL);
    let error = resolve(&room_map("", &area_entity("", &dull)))
        .expect_err("no `_e` face")
        .to_string();
    assert!(
        error.contains("entity 1 'light_area' has no face with an `_e` map"),
        "{error}"
    );
    let brushless = "{\n\"classname\" \"light_area\"\n\"origin\" \"0 0 64\"\n}\n";
    let error = resolve(&room_map("", brushless))
        .expect_err("no brushes")
        .to_string();
    assert!(
        error.contains("entity 1 'light_area' has no brushes"),
        "{error}"
    );
}

/// The sealed room of [`room_map`] with its ceiling slab, −16..272 square
/// and 128..144 high, moved into a `light_area` whose sides carry `tex`
/// (`[-x, +x, -y, +y, -z, +z]`): the light is part of the hull, its −z side
/// facing the room and every other exposed side the void.
fn hull_ceiling_map(tex: [&str; 6]) -> String {
    let shell = [
        brush([-16, -16, -16], [272, 272, 0], WALL),
        brush([-16, -16, 0], [0, 272, 128], WALL),
        brush([256, -16, 0], [272, 272, 128], WALL),
        brush([0, -16, 0], [256, 0, 128], WALL),
        brush([0, 256, 0], [256, 272, 128], WALL),
    ]
    .concat();
    let ceiling = area_entity("", &brush_faces([-16, -16, 128], [272, 272, 144], tex));
    format!(
        "{{\n\"classname\" \"worldspawn\"\n{shell}}}\n{ceiling}\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"32 32 24\"\n}}\n"
    )
}

// Regression: a `light_area` brush in the outer hull emitted its sides
// facing the void, which the exterior cull later drops from the render:
// its area, centroid and derived reach grew. Painted `_e` on every side,
// only the underside facing the room emits.
#[test]
fn a_hull_forming_pane_emits_only_its_side_facing_the_room() {
    let (map, _, _) = resolve(&hull_ceiling_map([GLASS; 6])).unwrap();
    let (light, emitter) = only_area(&map.lights);
    assert_close(
        emitter.total_area,
        square_metres(256.0 * 256.0),
        1.0e-9,
        "the room-facing underside alone",
    );
    for polygon in &emitter.polygons {
        assert!(
            (polygon.normal - DVec3::NEG_Y).length() < 1.0e-9,
            "a polygon facing {} emits",
            polygon.normal
        );
    }
    let centre = engine(128.0, 128.0, 128.0);
    assert!(
        emitter.centroid.distance(centre) < 1.0e-6,
        "centroid {} off the underside's centre {centre}",
        emitter.centroid
    );
    assert_eq!(light.origin, emitter.centroid);
}

// Regression: a hull-forming `light_area` whose only `_e` side faces the
// void compiled silently dark; it fails, naming the entity and saying its
// `_e` faces were culled, not that it has none.
#[test]
fn a_hull_forming_light_whose_only_e_side_faces_the_void_fails_naming_it() {
    let error = resolve(&hull_ceiling_map([WALL, WALL, WALL, WALL, WALL, GLASS]))
        .expect_err("the `_e` side faces the void")
        .to_string();
    assert!(
        error.contains(
            "entity 1 'light_area' has `_e` faces, but all are culled as exterior: \
             they face the void, or the map leaks"
        ),
        "{error}"
    );
}

/// [`ceiling_panel`]'s map with the room's +y wall left out: the room opens
/// on the void.
fn leaking_panel_map() -> String {
    let shell = [
        brush([-16, -16, -16], [272, 272, 0], WALL),
        brush([-16, -16, 128], [272, 272, 144], WALL),
        brush([-16, -16, 0], [0, 272, 128], WALL),
        brush([256, -16, 0], [272, 272, 128], WALL),
        brush([0, -16, 0], [256, 0, 128], WALL),
    ]
    .concat();
    let panel = area_entity("", &brush([96, 96, 120], [160, 160, 128], GLASS));
    format!(
        "{{\n\"classname\" \"worldspawn\"\n{shell}}}\n{panel}\
         {{\n\"classname\" \"info_player_start\"\n\"origin\" \"32 32 24\"\n}}\n"
    )
}

// On a leaking map the flood reaches every empty leaf and geometry draws no
// face, so exposure, which keeps what geometry draws, keeps none: the light
// fails saying its `_e` faces were culled, not that it has none.
#[test]
fn a_light_in_a_leaking_map_fails_saying_its_e_faces_were_culled() {
    let content = content(&leaking_panel_map());
    let mut map = parse_map_file(&content.map, MapFormat::IdTech2).unwrap();
    let OccluderPartition { result, .. } =
        partition_with_occluders(&map.brush_volumes, &map.kinematic_movers).unwrap();
    let (exterior, _) = full_exterior_flood(&result.tree);
    let area = map.area_lights[0].clone();
    assert!(
        result
            .faces
            .iter()
            .any(|face| area.brush_indices.contains(&face.brush_index) && face.texture == GLASS),
        "the BSP keeps the panel's `_e` faces"
    );
    assert!(
        result
            .tree
            .leaves
            .iter()
            .enumerate()
            .all(|(index, leaf)| leaf.is_solid || exterior.contains(&index)),
        "the flood reaches every empty leaf"
    );
    let (geometry, _) =
        crate::geometry::extract_geometry_with_brushes(&result.faces, &result.tree, &exterior);
    assert!(geometry.geometry.faces.is_empty(), "geometry draws no face");

    let error = resolve_area_lights(
        &mut map,
        &result.faces,
        &result.tree,
        &exterior,
        |texture: &str| texture == GLASS,
    )
    .expect_err("no `_e` face is drawn")
    .to_string();
    assert!(
        error.contains(
            "entity 1 'light_area' has `_e` faces, but all are culled as exterior: \
             they face the void, or the map leaks"
        ),
        "{error}"
    );
}

// Acceptance row 13 (research.md O22): the override sets the reach outright;
// without it reach derives from brightness × area, and doubling `light`
// lengthens it by √2.
#[test]
fn reach_is_derived_from_brightness_and_area_unless_overridden() {
    let panel = [([96, 96, 120], [160, 160, 128])];
    let (derived, _, _) = resolve(&ceiling_panel(&panel, "")).unwrap();
    let (light, emitter) = only_area(&derived.lights);
    let expected = derive(1.0, emitter.total_area);
    assert_eq!(light.falloff_range, expected);
    assert!(
        light.falloff_range > 4.0,
        "the derived reach is metres long"
    );

    let (overridden, _, _) =
        resolve(&ceiling_panel(&panel, "\"_falloff_range\" \"100\"\n")).unwrap();
    let (short, _) = only_area(&overridden.lights);
    assert!(f64::from(short.falloff_range) < f64::from(expected));
    assert_eq!(short.falloff_range, (100.0 * UNIT) as f32);

    let (doubled, _, _) = resolve(&ceiling_panel(&panel, "\"light\" \"600\"\n")).unwrap();
    let (bright, _) = only_area(&doubled.lights);
    assert_close(
        f64::from(bright.falloff_range) / f64::from(light.falloff_range),
        std::f64::consts::SQRT_2,
        1.0e-6,
        "reach ratio",
    );
}

fn derive(intensity: f64, area: f64) -> f32 {
    derived_reach(intensity, area) as f32
}

// Acceptance row 27 (research.md O11): an area light between two point
// lights keeps its entity position in the light list, the start-state table,
// the labels and the namespaces.
#[test]
fn an_area_light_keeps_its_entity_position_between_point_lights() {
    let point = |origin: &str| {
        format!(
            "{{\n\"classname\" \"light\"\n\"origin\" \"{origin}\"\n\"light\" \"200\"\n\
             \"_falloff_range\" \"300\"\n}}\n"
        )
    };
    let area = area_entity(
        "\"_start_inactive\" \"1\"\n",
        &brush([96, 96, 120], [160, 160, 128], GLASS),
    );
    let entities = point("40 40 64") + &area + &point("200 200 64");
    let (map, _, _) = resolve(&room_map("", &entities)).unwrap();
    let types: Vec<LightType> = map.lights.iter().map(|l| l.light_type).collect();
    assert_eq!(types, [LightType::Point, LightType::Area, LightType::Point]);
    assert_eq!(map.light_start_active_defaults, [true, false, true]);
    assert!(map.light_source_labels[1].starts_with("entity 2 'light_area' (emitter centroid"));
    assert_eq!(map.area_lights[0].light_index, 1);
    let buried = BuriedLights::none();
    let alpha: Vec<usize> = AlphaLightsNs::from_lights_excluding(&map.lights, &buried)
        .entries()
        .iter()
        .map(|entry| entry.source_index)
        .collect();
    assert_eq!(alpha, [0, 1, 2]);
    // `_start_inactive` reserved the area light's animated bake, so the
    // static namespace holds only the points, in order.
    let statics: Vec<usize> = StaticBakedLights::from_lights_excluding(&map.lights, &buried)
        .entries()
        .iter()
        .map(|entry| entry.source_index)
        .collect();
    assert_eq!(statics, [0, 2]);
}

/// Glass on all four sides of a worldspawn pillar, its centroid inside it,
/// and a point light buried at the same centre.
fn pillar_map() -> String {
    let pillar = brush([112, 112, 0], [144, 144, 128], WALL);
    let glass = [
        brush([144, 112, 32], [148, 144, 96], GLASS),
        brush([108, 112, 32], [112, 144, 96], GLASS),
        brush([112, 144, 32], [144, 148, 96], GLASS),
        brush([112, 108, 32], [144, 112, 96], GLASS),
    ]
    .concat();
    let buried_point = "{\n\"classname\" \"light\"\n\"origin\" \"128 128 64\"\n\
                        \"_falloff_range\" \"256\"\n}\n";
    room_map(&pillar, &(area_entity("", &glass) + buried_point))
}

// Acceptance row 16: the pillar-wrapping light is neither buried nor
// embedded and packs unassigned without a warning; the point light buried in
// the pillar is still dropped.
#[test]
fn a_pillar_wrapped_light_is_kept_and_a_buried_point_light_is_still_dropped() {
    let fixture = fixture(&pillar_map());
    let (area, emitter) = only_area(&fixture.lights);
    let centre = engine(128.0, 128.0, 64.0);
    assert!((emitter.centroid - centre).length() < 1.0e-9);
    let leaf = find_leaf_for_point(&fixture.full_tree, centre);
    assert!(
        fixture.full_tree.leaves[leaf].is_solid,
        "the centroid is in solid"
    );

    let buried = BuriedLights::classify(&fixture.full_tree, &fixture.lights);
    assert_eq!(fixture.lights[1].light_type, LightType::Point);
    assert!(!buried.is_buried(0), "the area light is kept");
    assert!(
        buried.is_buried(1),
        "the point light in the pillar is dropped"
    );
    assert!(
        !has_emitter_sphere(area),
        "the embedded-emitter test skips it"
    );

    let capture = LogCapture::start();
    let section = crate::pack::encode_alpha_lights(
        &AlphaLightsNs::from_lights_excluding(&fixture.lights, &buried),
        &fixture.full_tree,
    );
    capture.assert_not_logged(Level::Warn, "inside a solid leaf");
    drop(capture);
    assert_eq!(section.lights.len(), 1);
    let record = &section.lights[0];
    assert_eq!(record.light_type, AlphaLightType::Area);
    assert_eq!(record.leaf_index, ALPHA_LIGHT_LEAF_UNASSIGNED);
    assert_eq!(
        record.origin,
        emitter.centroid.to_array(),
        "origin is the centroid"
    );
    assert_eq!(record.falloff_range, area.falloff_range);
    assert_eq!(record.falloff_model, AlphaFalloffModel::InverseSquared);
    assert_eq!(record.shadow_type, AlphaShadowType::StaticLightMap);
    assert_eq!(
        (
            record.cone_angle_inner,
            record.cone_angle_outer,
            record.cone_direction
        ),
        (0.0, 0.0, [0.0; 3])
    );
}

// The unshadowed term reaches the lightmap end to end: `area-light-panel`
// compiles through the real `_e` lookup, and every floor texel under its
// panel stores visibility 1 and the analytic polygon term of the authored
// 64 × 64 underside, windowed at the derived reach.
#[test]
fn the_panel_fixture_lights_the_floor_with_the_unshadowed_polygon_term() {
    use crate::bake_control::BakeControl;
    use crate::bvh_build::build_bvh;
    use crate::lightmap_bake::prepare_atlas;
    use crate::lightmap_layer::{
        ExactSkips, SharedAtlas, bake_light_layer, for_each_light_layer_chart_texel,
        reconstruct_light_texel,
    };
    use crate::texel_footprint::PENUMBRA_FLOOR_DEFAULT;

    let mut fixture = load_fixture("area-light-panel");
    let (light, emitter) = only_area(&fixture.lights);
    let light = light.clone();
    let underside = EmitterPolygon::new(
        vec![
            engine(96.0, 96.0, 120.0),
            engine(160.0, 96.0, 120.0),
            engine(160.0, 160.0, 120.0),
            engine(96.0, 160.0, 120.0),
        ],
        DVec3::NEG_Y,
    );
    assert_eq!(emitter.polygons.len(), 1, "only the underside glows");
    assert_close(emitter.total_area, underside.area, 1.0e-9, "panel area");
    let reach = f64::from(derive(1.0, underside.area));
    assert_eq!(light.falloff_range as f64, reach);

    let static_lights = StaticBakedLights::from_lights(&fixture.lights);
    let prepared = prepare_atlas(&mut fixture.geometry, &static_lights, 0.25, &[]).unwrap();
    let (bvh, primitives, _) = build_bvh(&fixture.geometry).unwrap();
    let shared = SharedAtlas {
        charts: &prepared.charts,
        placements: &prepared.placements,
        atlas_width: prepared.atlas_width,
        atlas_height: prepared.atlas_height,
        layout: &prepared.layout,
    };
    let layers = bake_light_layer(
        &light,
        &shared,
        &bvh,
        &primitives,
        &fixture.geometry,
        16,
        PENUMBRA_FLOOR_DEFAULT,
        ExactSkips::ON,
        &BakeControl::unrestricted(),
    );

    let (lo, hi) = {
        let (a, b) = (engine(96.0, 96.0, 0.0), engine(160.0, 160.0, 0.0));
        (a.min(b), a.max(b))
    };
    let mut checked = 0usize;
    for (face, chart) in prepared.charts.iter().enumerate() {
        if (chart.normal - Vec3::Y).length() > 1.0e-4 {
            continue;
        }
        let layer = layers
            .iter()
            .find(|layer| layer.target_layer == prepared.placements[face].layer)
            .expect("the floor's layer bakes");
        for_each_light_layer_chart_texel(&shared, face, |sample| {
            let p = sample.world_p.as_dvec3();
            let under_panel =
                p.y.abs() < 1.0e-3 && p.x > lo.x && p.x < hi.x && p.z > lo.z && p.z < hi.z;
            if !under_panel {
                return;
            }
            let record = layer
                .texels
                .iter()
                .find(|texel| texel.idx == sample.idx)
                .expect("a lit floor texel is recorded");
            assert_eq!(record.raw_visibility, 1.0, "unshadowed at {p}");
            let (irradiance, _) =
                reconstruct_light_texel(&light, sample.world_p, sample.surface_normal, 1.0);
            let n = sample.surface_normal.as_dvec3();
            let (e, _) = polygon_irradiance(&underside, p, n);
            let expected = e * reach_window(polygon_distance(&underside, p), reach);
            for channel in irradiance.to_array() {
                assert_close(f64::from(channel), expected, 1.0e-4, "floor texel term");
            }
            checked += 1;
        });
    }
    assert!(
        checked > 16,
        "the floor under the panel holds texels: {checked}"
    );
}

// Acceptance row 31: editing only `delay`, `_light_size` or a spot key on an
// area light re-keys no bake entry. Translation drops them, so the resolved
// light is byte-identical, and so is every key that folds it: each
// lightmap layer, the surface-radiance digest and the light bytes the
// SH-family memos fold. The geometry, and so every geometry key, is the same.
#[test]
fn inert_keys_leave_every_lighting_key_unchanged() {
    use crate::bvh_build::build_bvh;
    use crate::delta_sh_cache::encoded_key_light;
    use crate::lightmap_bake::{DEFAULT_AREA_SAMPLE_COUNT, prepare_atlas};
    use crate::lightmap_layer::{SharedAtlas, layer_input_hash};
    use crate::ray_traversal::TRACER_EPOCH;
    use crate::surface_radiance::LightDigest;
    use crate::texel_footprint::PENUMBRA_FLOOR_DEFAULT;

    let panel = [([96, 96, 120], [160, 160, 128])];
    let inert = "\"delay\" \"0\"\n\"_light_size\" \"16\"\n\"_cone\" \"20\"\n\"_cone2\" \"30\"\n\
                 \"angle\" \"90\"\n\"mangle\" \"0 -90 0\"\n\"target\" \"t1\"\n";
    let plain = fixture(&ceiling_panel(&panel, ""));
    let edited = fixture(&ceiling_panel(&panel, inert));
    let (light, _) = only_area(&plain.lights);
    let (edited_light, _) = only_area(&edited.lights);
    assert_eq!(light, edited_light);
    assert_eq!(encoded_key_light(light), encoded_key_light(edited_light));
    assert_eq!(LightDigest::of(light), LightDigest::of(edited_light));

    let keys = |mut fixture: FixturePipeline, light: &MapLight| {
        let static_lights = StaticBakedLights::from_lights(std::slice::from_ref(light));
        let prepared = prepare_atlas(&mut fixture.geometry, &static_lights, 0.25, &[]).unwrap();
        let (_, primitives, _) = build_bvh(&fixture.geometry).unwrap();
        let shared = SharedAtlas {
            charts: &prepared.charts,
            placements: &prepared.placements,
            atlas_width: prepared.atlas_width,
            atlas_height: prepared.atlas_height,
            layout: &prepared.layout,
        };
        (0..prepared.layer_count)
            .map(|layer| {
                layer_input_hash(
                    light,
                    &shared,
                    &primitives,
                    &fixture.geometry,
                    0.25,
                    DEFAULT_AREA_SAMPLE_COUNT,
                    PENUMBRA_FLOOR_DEFAULT,
                    TRACER_EPOCH,
                    layer,
                )
            })
            .collect::<Vec<_>>()
    };
    let (light, edited_light) = (light.clone(), edited_light.clone());
    assert_eq!(keys(plain, &light), keys(edited, &edited_light));
}

// The affinity flood seeds an area light from the cells in front of its
// polygons, never the cell its centroid locates to: the light wrapping the
// pillar, whose centroid lies in the pillar's solid, floods the room rather
// than bypassing the portal filter.
#[test]
fn a_pillar_wrapped_light_floods_its_cells_from_its_faces() {
    use crate::affinity_grid::{light_portal_reach, portal_adjacency};

    let fixture = fixture(&pillar_map());
    let (area, _) = only_area(&fixture.lights);
    let centroid_cell = find_leaf_for_point(&fixture.tree, area.origin);
    assert!(fixture.tree.leaves[centroid_cell].is_solid);
    let adjacency = portal_adjacency(&fixture.portals);
    let reached = light_portal_reach(area, &fixture.tree, &fixture.exterior_leaves, &adjacency)
        .expect("the faces' cells are open, so the filter applies");
    let room = find_leaf_for_point(&fixture.tree, engine(32.0, 32.0, 64.0));
    assert!(reached.contains(&room), "the room's cell is reached");
}

// Acceptance row 44, build half (research.md O18): on the shipped
// `area-lights-demo`, the east panel carries `delay 0`, `_light_size`, `_cone`
// and `mangle` from a converted spot light. The snapshot a level script reads
// at bake time (`light_table_from_lights`) reads lightType "Area",
// falloffModel "InverseSquared", the resolved reach and the area-weighted
// centroid with no cone fields, the same as the clean west panel's apart from
// position, and equals field for field the AlphaLights record the runtime
// builds its snapshot from (`postretro` `area_lights_surface_tests` holds the
// runtime half to that record).
#[test]
fn inert_keyed_area_light_snapshot_matches_its_alpha_lights_record() {
    use crate::script_light_membership::light_table_from_lights;

    let fixture = load_fixture("area-lights-demo");
    let buried = BuriedLights::classify(&fixture.full_tree, &fixture.lights);
    let table = light_table_from_lights(&fixture.lights, &buried).unwrap();
    assert_eq!(table.lights.len(), 2, "two skylights");
    let section = crate::pack::encode_alpha_lights(
        &AlphaLightsNs::from_lights_excluding(&fixture.lights, &buried),
        &fixture.full_tree,
    );
    assert_eq!(section.lights.len(), 2);

    // 64 × 64-unit glass undersides at z 120, centred at x 128 and 384.
    let area_m2 = square_metres(64.0 * 64.0);
    let reach = crate::area_light::derived_reach(1.0, area_m2);
    for ((snapshot, record), centre_x) in
        table.lights.iter().zip(&section.lights).zip([128.0, 384.0])
    {
        let component = &snapshot.component;
        assert_eq!(component.light_type, "Area");
        assert_eq!(component.falloff_model, "InverseSquared");
        assert_eq!(
            (
                component.cone_angle_inner,
                component.cone_angle_outer,
                component.cone_direction
            ),
            (None, None, None)
        );
        let centroid = engine(centre_x, 128.0, 120.0).as_vec3().to_array();
        for (got, want) in component.origin.iter().zip(centroid) {
            assert!((got - want).abs() < 1.0e-5, "centroid: {got} vs {want}");
        }
        assert_close(f64::from(component.falloff_range), reach, 1.0e-6, "reach");
        assert_eq!(snapshot.position, component.origin);

        // Field for field, the record the loader turns into the runtime light.
        assert_eq!(record.light_type, AlphaLightType::Area);
        assert_eq!(record.falloff_model, AlphaFalloffModel::InverseSquared);
        assert_eq!(record.origin.map(|v| v as f32), component.origin);
        assert_eq!(record.falloff_range, component.falloff_range);
        assert_eq!(record.intensity, component.intensity);
        assert_eq!(record.color, component.color);
        assert_eq!(
            (
                record.cone_angle_inner,
                record.cone_angle_outer,
                record.cone_direction
            ),
            (0.0, 0.0, [0.0; 3])
        );
    }
    let (west, east) = (&table.lights[0].component, &table.lights[1].component);
    let mut east_at_west = east.clone();
    east_at_west.origin = west.origin;
    assert_eq!(
        &east_at_west, west,
        "the inert keys change nothing but position"
    );
}
