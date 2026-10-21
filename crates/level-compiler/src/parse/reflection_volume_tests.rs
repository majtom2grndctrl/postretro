//! Parser contracts for the `reflection_volume` brush entity.

use crate::map_data::{MapData, MapReflectionVolume};
use crate::map_format::MapFormat;

use super::parse_map_file;

/// Quake map units to engine meters.
const UNIT: f32 = 0.0254;

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

/// A worldspawn box (entity 0), then a fluid (entity 1), then one
/// `reflection_volume` (entity 2) with `properties` and `brushes`.
fn reflection_map(properties: &str, brushes: &[String]) -> String {
    let mut volume = vec![
        "{".to_string(),
        "\"classname\" \"reflection_volume\"".to_string(),
    ];
    volume.extend(properties.lines().map(str::to_owned));
    volume.extend(brushes.iter().cloned());
    volume.push("}".to_string());
    format!(
        "{{\n\"classname\" \"worldspawn\"\n{}\n}}\n{{\n\"classname\" \"fluid_volume\"\n\
         \"fluid\" \"water\"\n{}\n}}\n{}",
        box_brush([128, 128, 0], [256, 256, 128], "world_tex"),
        box_brush([0, 0, 0], [64, 64, 32], "water"),
        volume.join("\n")
    )
}

fn parse_inline_map(map_text: &str) -> anyhow::Result<MapData> {
    static NEXT_INLINE_MAP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT_INLINE_MAP_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "postretro-reflection-volume-map-{}-{id}.map",
        std::process::id(),
    ));
    std::fs::write(&path, format!("{}\n", map_text.trim()))?;
    let result = parse_map_file(&path, MapFormat::IdTech2);
    let _ = std::fs::remove_file(&path);
    result
}

fn one_volume(properties: &str) -> anyhow::Result<MapReflectionVolume> {
    let map = parse_inline_map(&reflection_map(
        properties,
        &[box_brush([-32, -32, 0], [96, 64, 128], "probe")],
    ))?;
    assert_eq!(map.reflection_volumes.len(), 1);
    Ok(map.reflection_volumes.into_iter().next().unwrap())
}

#[track_caller]
fn assert_fails_naming_entity(properties: &str, expected: &str) {
    let error = one_volume(properties).expect_err("malformed reflection volume must fail");
    let message = format!("{error:#}");
    assert!(
        message.contains("entity 2 'reflection_volume'") && message.contains(expected),
        "{message}"
    );
}

#[test]
fn reflection_volume_spans_every_brush_in_engine_space() {
    let map = parse_inline_map(&reflection_map(
        "",
        &[
            box_brush([-32, -32, 0], [32, 32, 128], "probe"),
            box_brush([32, -16, 0], [96, 64, 64], "probe"),
        ],
    ))
    .unwrap();
    assert_eq!(map.reflection_volumes.len(), 1);
    let volume = &map.reflection_volumes[0];
    assert_eq!(volume.source_entity, 2);
    assert_eq!(volume.source_label, "entity 2 'reflection_volume'");
    assert_eq!(volume.capture_offset, None);
    // Quake x -32..96, y -32..64, z 0..128 → engine x = -y, y = z, z = -x.
    let expected_min = [-64.0 * UNIT, 0.0, -96.0 * UNIT];
    let expected_max = [32.0 * UNIT, 128.0 * UNIT, 32.0 * UNIT];
    for axis in 0..3 {
        assert!(
            (volume.min[axis] - expected_min[axis]).abs() < 1e-5,
            "{volume:?}"
        );
        assert!(
            (volume.max[axis] - expected_max[axis]).abs() < 1e-5,
            "{volume:?}"
        );
    }
}

#[test]
fn reflection_volume_is_peeled_from_world_environment_and_runtime_entities() {
    let without = parse_inline_map(&format!(
        "{{\n\"classname\" \"worldspawn\"\n{}\n}}\n{{\n\"classname\" \"fluid_volume\"\n\
         \"fluid\" \"water\"\n{}\n}}",
        box_brush([128, 128, 0], [256, 256, 128], "world_tex"),
        box_brush([0, 0, 0], [64, 64, 32], "water"),
    ))
    .unwrap();
    let with = parse_inline_map(&reflection_map(
        "",
        &[box_brush([-32, -32, 0], [96, 64, 128], "probe")],
    ))
    .unwrap();
    assert_eq!(with.brush_volumes.len(), without.brush_volumes.len());
    assert_eq!(
        with.environment_volumes.len(),
        1,
        "id 52 holds the fluid alone"
    );
    assert_eq!(with.map_entities.len(), without.map_entities.len());
    assert!(
        with.map_entities
            .iter()
            .all(|entity| entity.classname != "reflection_volume")
    );
    assert!(
        !with
            .brush_volumes
            .iter()
            .flat_map(|brush| &brush.sides)
            .any(|side| side.texture == "probe"),
        "no probe brush side reaches the static world"
    );
}

#[test]
fn capture_offset_converts_map_horizontal_axes_to_engine_meters() {
    let volume = one_volume("\"_capture_offset\" \"32 -16\"").unwrap();
    // Quake (32, -16, 0) → engine (x = 16, z = -32) map units.
    let [x, z] = volume.capture_offset.expect("authored offset");
    assert!((x - 16.0 * UNIT).abs() < 1e-6, "{x}");
    assert!((z + 32.0 * UNIT).abs() < 1e-6, "{z}");
    // Blank is unset, as TrenchBroom writes a cleared field.
    assert_eq!(
        one_volume("\"_capture_offset\" \"  \"")
            .unwrap()
            .capture_offset,
        None
    );
}

#[test]
fn capture_offset_that_is_vertical_or_malformed_fails_naming_the_entity() {
    assert_fails_naming_entity("\"_capture_offset\" \"0 0 8\"", "vertical");
    assert_fails_naming_entity("\"_capture_offset\" \"0 0 0\"", "vertical");
    assert_fails_naming_entity("\"_capture_offset\" \"8\"", "1 number(s)");
    assert_fails_naming_entity("\"_capture_offset\" \"1 2 3 4\"", "4 number(s)");
    assert_fails_naming_entity("\"_capture_offset\" \"8 east\"", "`east`");
    assert_fails_naming_entity("\"_capture_offset\" \"nan 0\"", "`nan`");
    assert_fails_naming_entity("\"_capture_offset\" \"inf 0\"", "`inf`");
}

#[test]
fn reflection_volume_without_brushes_fails_naming_the_entity() {
    let error = parse_inline_map(&reflection_map("\"origin\" \"0 0 0\"", &[]))
        .expect_err("a point reflection_volume has no box");
    let message = format!("{error:#}");
    assert!(
        message.contains("entity 2 'reflection_volume'") && message.contains("no brushes"),
        "{message}"
    );
}
