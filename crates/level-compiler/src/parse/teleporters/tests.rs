use crate::map_data::MapData;
use crate::map_format::MapFormat;
use crate::parse::parse_map_file;
use postretro_level_format::teleporters::{TELEPORTER_SOURCE_ENTITY_KEY, TeleportMode};
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
        "postretro-teleport-map-{}-{id}.map",
        std::process::id(),
    ));
    std::fs::write(&path, format!("{}\n", map_text.trim()))?;
    let result = parse_map_file(&path, MapFormat::IdTech2);
    let _ = std::fs::remove_file(&path);
    result
}

fn destination(name: &str, origin: &str, angles: &str) -> String {
    volume_entity(
        "teleport_destination",
        &format!("\"name\" \"{name}\"\n\"origin\" \"{origin}\"\n\"angles\" \"{angles}\""),
        &[],
    )
}
fn gate(extra: &str) -> String {
    volume_entity(
        "trigger_teleport",
        extra,
        &[
            box_brush([0, 0, 0], [32, 32, 64], "teleport_tex"),
            box_brush([64, 0, 0], [96, 32, 64], "teleport_tex"),
        ],
    )
}
fn fixture(extra: &str) -> String {
    volume_map(&[
        gate(extra),
        destination("out", "256 128 64", "0 90 0"),
        destination("in", "0 0 0", "0 0 0"),
    ])
}
#[test]
fn teleport_parser_resolves_forward_references_union_axes_and_tag_companion() {
    let map = parse_inline_map(&fixture("\"mode\" \"seamless\"\n\"target\" \"out\"\n\"landmark\" \"in\"\n\"enabled_on_spawn\" \"0\"\n\"name\" \"gate\"\n\"_tags\" \"a b\"" )).unwrap();
    assert_eq!(
        map.brush_volumes.len(),
        1,
        "semantic brushes have no static geometry role"
    );
    assert_eq!(
        map.map_entities.len(),
        1,
        "destinations are compile-time only"
    );
    let companion = &map.map_entities[0];
    assert_eq!(companion.tags, ["a", "b"]);
    assert!(
        companion
            .key_values
            .contains(&(TELEPORTER_SOURCE_ENTITY_KEY.to_owned(), "1".to_owned()))
    );
    assert!(
        companion
            .key_values
            .contains(&("name".to_owned(), "gate".to_owned()))
    );
    let t = &map.teleporters.teleporters[0];
    assert_eq!(t.source_entity, 1);
    assert_eq!(t.brushes.len(), 2);
    assert_eq!(t.mode, TeleportMode::Seamless);
    assert!(!t.enabled_on_spawn);
    assert_eq!(t.landmark_origin, [0.0; 3]);
    let unit = 0.0254f32;
    assert_eq!(
        t.destination_origin,
        [-128.0 * unit, 64.0 * unit, -256.0 * unit]
    );
    assert_eq!(t.destination_yaw, -90.0f32.to_radians());
    let base = parse_inline_map(&volume_map(&[])).unwrap();
    assert_eq!(map.brush_volumes.len(), base.brush_volumes.len());
    assert_eq!(map.environment_volumes.len(), 0);
}
#[test]
fn teleport_parser_rejects_invalid_authoring_naming_entity() {
    for extra in [
        "\"target\" \"missing\"",
        "\"target\" \"out\"\n\"mode\" \"strange\"",
        "\"target\" \"out\"\n\"mode\" \"seamless\"",
        "\"target\" \"out\"\n\"landmark\" \"in\"",
        "\"target\" \"out\"\n\"mode\" \"seamless\"\n\"landmark\" \"out\"",
        "\"target\" \"out\"\n\"mode\" \"seamless\"\n\"landmark\" \"missing\"",
        "\"target\" \"out\"\n\"enabled_on_spawn\" \"2\"",
        "\"target\" \" \"",
    ] {
        let error = parse_inline_map(&fixture(extra))
            .map(|_| ())
            .expect_err("authoring must fail")
            .to_string();
        assert!(error.contains("entity 1 'trigger_teleport'"), "{error}");
    }
    let duplicate = volume_map(&[
        destination("out", "0 0 0", "0 0 0"),
        destination("out", "1 2 3", "0 0 0"),
    ]);
    assert!(
        parse_inline_map(&duplicate)
            .err()
            .unwrap()
            .to_string()
            .contains("entity 2 'teleport_destination' duplicate teleport_destination name `out`")
    );
    for (origin, angles) in [
        ("NaN 0 0", "0 0 0"),
        ("0 0", "0 0 0"),
        ("0 0 0", "0 inf 0"),
        ("0 0 0", "0 0"),
    ] {
        let error = parse_inline_map(&volume_map(&[destination("out", origin, angles)]))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("teleport_destination"), "{error}");
    }
    let point = volume_map(&[
        volume_entity("trigger_teleport", "\"target\" \"out\"", &[]),
        destination("out", "0 0 0", "0 0 0"),
    ]);
    assert!(
        parse_inline_map(&point)
            .err()
            .unwrap()
            .to_string()
            .contains("no brushes")
    );
}
#[test]
fn teleport_parser_warns_for_pitch_roll_and_uses_only_yaw() {
    let capture = postretro_test_log_capture::LogCapture::start();
    let map = parse_inline_map(&volume_map(&[
        gate("\"target\" \"out\""),
        destination("out", "0 0 0", "12 90 -2"),
    ]))
    .unwrap();
    capture.assert_logged_once(log::Level::Warn, "pitch and roll are ignored");
    let t = &map.teleporters.teleporters[0];
    assert_eq!(t.destination_yaw, -90.0f32.to_radians());
    assert_eq!(t.landmark_origin, [0.0; 3]);
    assert!(t.enabled_on_spawn);
}
