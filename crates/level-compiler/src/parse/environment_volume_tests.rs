//! Parser contracts for the purpose-named environment-volume entities.

use glam::Vec3;

use crate::map_data::{MapData, MapEnvironmentVolume};
use crate::map_format::MapFormat;

use super::parse_map_file;

/// Quake map units to engine meters.
const UNIT: f32 = 0.0254;

/// A Quake-format axis-aligned box brush spanning `min`..`max` (map units),
/// with each face wound so its normal points out of the box.
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

/// One brush entity. `properties` are extra `"key" "value"` lines. No blank
/// line is emitted: one inside an entity block makes the `.map` parser drop
/// the remaining entities.
fn volume_entity(classname: &str, properties: &str, brushes: &[String]) -> String {
    let mut lines = vec!["{".to_string(), format!("\"classname\" \"{classname}\"")];
    lines.extend(properties.lines().map(str::to_owned));
    lines.extend(brushes.iter().cloned());
    lines.push("}".to_string());
    lines.join("\n")
}

/// A worldspawn box (entity 0) followed by `entities`, so the first entity
/// passed is map entity 1.
fn volume_map(entities: &[String]) -> String {
    let world = box_brush([128, 128, 0], [256, 256, 128], "world_tex");
    format!(
        "{{\n\"classname\" \"worldspawn\"\n{world}\n}}\n{}",
        entities.join("\n")
    )
}

fn parse_inline_map(map_text: &str) -> anyhow::Result<MapData> {
    static NEXT_INLINE_MAP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT_INLINE_MAP_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "postretro-environment-volume-map-{}-{id}.map",
        std::process::id(),
    ));
    std::fs::write(&path, format!("{}\n", map_text.trim()))?;
    let result = parse_map_file(&path, MapFormat::IdTech2);
    let _ = std::fs::remove_file(&path);
    result
}

fn one_volume(classname: &str, properties: &str) -> anyhow::Result<MapEnvironmentVolume> {
    let brush = box_brush([0, 0, 0], [32, 32, 64], "volume_tex");
    let map = parse_inline_map(&volume_map(&[volume_entity(
        classname,
        properties,
        &[brush],
    )]))?;
    assert_eq!(map.environment_volumes.len(), 1);
    Ok(map.environment_volumes.into_iter().next().unwrap())
}

#[track_caller]
fn assert_fails_naming_entity(classname: &str, properties: &str, expected: &str) {
    let error = one_volume(classname, properties)
        .expect_err("malformed environment volume must fail the build");
    let message = format!("{error:#}");
    assert!(
        message.contains(&format!("entity 1 '{classname}'")) && message.contains(expected),
        "error must name entity 1 '{classname}' and mention `{expected}`: {message}"
    );
}

#[test]
fn gravity_volume_compiles_one_signed_vertical_number_unconverted() {
    for (authored, expected) in [("-3", -3.0), ("0", 0.0), ("4", 4.0), (" -1.62 ", -1.62)] {
        let volume = one_volume("gravity_volume", &format!("\"gravity\" \"{authored}\""))
            .unwrap_or_else(|error| panic!("gravity \"{authored}\" must compile: {error:#}"));
        assert_eq!(volume.gravity, Some(expected), "gravity \"{authored}\"");
        assert_eq!(volume.push, None);
        assert_eq!(volume.fluid, None);
    }
}

#[test]
fn gravity_volume_rejects_any_shape_but_one_number_naming_the_entity() {
    assert_fails_naming_entity("gravity_volume", "\"gravity\" \"0 0 -3\"", "has 3 numbers");
    assert_fails_naming_entity("gravity_volume", "\"gravity\" \"-3 1\"", "has 2 numbers");
    assert_fails_naming_entity(
        "gravity_volume",
        "\"gravity\" \"low\"",
        "not a finite number",
    );
    assert_fails_naming_entity(
        "gravity_volume",
        "\"gravity\" \"inf\"",
        "not a finite number",
    );
}

#[test]
fn push_volume_converts_map_axes_to_engine_axes_without_unit_scale() {
    // engine = (-map_y, map_z, -map_x); m/s² is already metric, so no 0.0254.
    for (authored, expected) in [
        ("0 0 15", Vec3::new(0.0, 15.0, 0.0)),
        ("8 0 0", Vec3::new(0.0, 0.0, -8.0)),
        ("3 -4 5", Vec3::new(4.0, 5.0, -3.0)),
    ] {
        let volume = one_volume("push_volume", &format!("\"push\" \"{authored}\""))
            .unwrap_or_else(|error| panic!("push \"{authored}\" must compile: {error:#}"));
        assert_eq!(volume.push, Some(expected), "push \"{authored}\"");
        assert_eq!(volume.gravity, None);
        assert_eq!(volume.fluid, None);
    }
}

#[test]
fn push_volume_rejects_any_shape_but_three_numbers_naming_the_entity() {
    assert_fails_naming_entity("push_volume", "\"push\" \"15\"", "has 1 numbers");
    assert_fails_naming_entity("push_volume", "\"push\" \"0 15\"", "has 2 numbers");
    assert_fails_naming_entity("push_volume", "\"push\" \"0 0 15 1\"", "has 4 numbers");
    assert_fails_naming_entity("push_volume", "\"push\" \"0 up 15\"", "`up`");
    assert_fails_naming_entity(
        "push_volume",
        "\"push\" \"0 NaN 15\"",
        "not a finite number",
    );
}

#[test]
fn fluid_volume_carries_its_trimmed_fluid_name() {
    let volume = one_volume("fluid_volume", "\"fluid\" \"  water \"").expect("fluid compiles");
    assert_eq!(volume.fluid.as_deref(), Some("water"));
    assert_eq!(volume.gravity, None);
    assert_eq!(volume.push, None);
}

#[test]
fn blank_or_missing_required_fields_fail_naming_the_entity() {
    assert_fails_naming_entity("fluid_volume", "\"fluid\" \"\"", "non-blank `fluid`");
    assert_fails_naming_entity("fluid_volume", "\"fluid\" \"   \"", "non-blank `fluid`");
    assert_fails_naming_entity("fluid_volume", "", "non-blank `fluid`");
    assert_fails_naming_entity("gravity_volume", "\"gravity\" \"\"", "non-blank `gravity`");
    assert_fails_naming_entity("gravity_volume", "", "non-blank `gravity`");
    assert_fails_naming_entity("push_volume", "\"push\" \" \"", "non-blank `push`");
    assert_fails_naming_entity("push_volume", "", "non-blank `push`");
}

#[test]
fn priority_defaults_to_zero_and_rejects_non_integers_naming_the_entity() {
    let absent = one_volume("fluid_volume", "\"fluid\" \"water\"").unwrap();
    let cleared = one_volume("fluid_volume", "\"fluid\" \"water\"\n\"priority\" \"\"").unwrap();
    let negative = one_volume("fluid_volume", "\"fluid\" \"water\"\n\"priority\" \"-2\"").unwrap();
    assert_eq!(
        (absent.priority, cleared.priority, negative.priority),
        (0, 0, -2)
    );
    assert_fails_naming_entity(
        "fluid_volume",
        "\"fluid\" \"water\"\n\"priority\" \"1.5\"",
        "not an integer",
    );
    assert_fails_naming_entity(
        "gravity_volume",
        "\"gravity\" \"-3\"\n\"priority\" \"high\"",
        "not an integer",
    );
}

#[test]
fn a_brushless_environment_volume_fails_naming_the_entity() {
    let map = volume_map(&[
        "{\n\"classname\" \"push_volume\"\n\"origin\" \"0 0 0\"\n\"push\" \"0 0 15\"\n}"
            .to_string(),
    ]);
    let error = parse_inline_map(&map).expect_err("a point-entity volume must fail");
    let message = format!("{error:#}");
    assert!(
        message.contains("entity 1 'push_volume'") && message.contains("no brushes"),
        "{message}"
    );
}

#[test]
fn environment_volumes_keep_map_order_and_one_leaf_per_brush_outside_the_static_world() {
    let world_only = parse_inline_map(&volume_map(&[])).expect("world-only map parses");
    let map = parse_inline_map(&volume_map(&[
        volume_entity(
            "gravity_volume",
            "\"gravity\" \"-3\"\n\"priority\" \"1\"",
            &[box_brush([0, 0, 0], [32, 32, 64], "volume_tex")],
        ),
        "{\n\"classname\" \"info_null\"\n\"origin\" \"0 0 0\"\n}".to_string(),
        volume_entity(
            "fluid_volume",
            "\"fluid\" \"water\"",
            &[
                box_brush([0, 0, 0], [32, 32, 16], "water_tex"),
                box_brush([32, 0, 0], [64, 32, 16], "water_tex"),
            ],
        ),
    ]))
    .expect("volume map parses");

    // Peeled before the BSP: no brush reaches the static world set or the
    // runtime classname-dispatch entities.
    assert_eq!(map.brush_volumes.len(), world_only.brush_volumes.len());
    assert!(
        map.map_entities
            .iter()
            .all(|entity| entity.classname == "info_null"),
        "volumes never become runtime map entities"
    );

    let volumes = &map.environment_volumes;
    assert_eq!(volumes.len(), 2);
    assert_eq!((volumes[0].source_entity, volumes[1].source_entity), (1, 3));
    assert_eq!(volumes[0].priority, 1);
    assert_eq!(volumes[0].brushes.len(), 1);
    assert_eq!(volumes[1].brushes.len(), 2, "one convex leaf per brush");

    // Quake box (0,0,0)..(32,32,64) → engine x = -y, y = z, z = -x, in meters.
    let leaf = &volumes[0].brushes[0];
    let expected_min = [-32.0 * UNIT, 0.0, -32.0 * UNIT];
    let expected_max = [0.0, 64.0 * UNIT, 0.0];
    for axis in 0..3 {
        assert!(
            (leaf.min[axis] - expected_min[axis]).abs() < 1e-5,
            "{leaf:?}"
        );
        assert!(
            (leaf.max[axis] - expected_max[axis]).abs() < 1e-5,
            "{leaf:?}"
        );
    }
    let inside = |point: Vec3| {
        leaf.planes
            .iter()
            .all(|&[nx, ny, nz, d]| Vec3::new(nx, ny, nz).dot(point) - d <= 1e-6)
    };
    let centre = (Vec3::from(leaf.min) + Vec3::from(leaf.max)) * 0.5;
    assert_eq!(leaf.planes.len(), 6);
    assert!(inside(centre), "planes face outward: the centre is inside");
    assert!(
        inside(Vec3::from(leaf.max)),
        "a corner on the faces is inside"
    );
    assert!(!inside(Vec3::from(leaf.max) + Vec3::splat(0.01)));

    // The two pool brushes are separate leaves, not one merged hull.
    let [left, right] = [&volumes[1].brushes[0], &volumes[1].brushes[1]];
    assert!((left.min[2] - (-32.0 * UNIT)).abs() < 1e-5 && left.max[2].abs() < 1e-5);
    assert!((right.min[2] - (-64.0 * UNIT)).abs() < 1e-5);
    assert!((right.max[2] - (-32.0 * UNIT)).abs() < 1e-5);
}

#[test]
fn environment_section_always_carries_the_level_gravity() {
    let map = parse_inline_map(&volume_map(&[])).unwrap();
    let section = crate::environment_volumes::encode_environment_volumes_section(
        &map.environment_volumes,
        map.initial_gravity,
    );
    assert!(section.volumes.is_empty());
    assert_eq!(section.level_gravity, map.initial_gravity);

    let volume = one_volume("fluid_volume", "\"fluid\" \"water\"").unwrap();
    let section = crate::environment_volumes::encode_environment_volumes_section(
        std::slice::from_ref(&volume),
        -2.0,
    );
    let record = &section.volumes[0];
    assert_eq!(section.level_gravity, -2.0);
    assert_eq!(record.source_entity, 1);
    assert_eq!(record.fluid, "water");
    assert_eq!((record.gravity, record.push), (None, None));
    assert_eq!(record.brushes[0].planes.len(), 6);
    assert!(
        record.surface_faces.is_empty(),
        "surfaces are not built yet"
    );
}
