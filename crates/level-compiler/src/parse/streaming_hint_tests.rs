//! Focused parser contracts for compiler-only streaming hint entities.

use std::collections::HashSet;

use glam::DVec3;

use crate::map_format::MapFormat;

use super::{parse_map_file, quake_to_engine};

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

fn streaming_hint_entity(classname: &str, origin: &str, properties: &str, brushes: &str) -> String {
    let properties = (!properties.is_empty()).then(|| format!("\n{properties}"));
    format!(
        r#"{{
"classname" "{classname}"
"origin" "{origin}"{}
{brushes}
}}"#,
        properties.unwrap_or_default(),
    )
}

fn streaming_hint_map(hint_entities: &str) -> String {
    let world = box_brush([128, 128, 0], [256, 256, 128], "world_tex");
    format!(
        r#"
{{
"classname" "worldspawn"
"initialGravity" "-9.81"
{world}
}}
{hint_entities}
"#
    )
}

fn parse_inline_map(map_text: &str) -> anyhow::Result<crate::map_data::MapData> {
    static NEXT_INLINE_MAP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT_INLINE_MAP_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "postretro-streaming-hint-map-{}-{id}.map",
        std::process::id(),
    ));
    std::fs::write(&path, format!("{}\n", map_text.trim()))?;
    let result = parse_map_file(&path, MapFormat::IdTech2);
    let _ = std::fs::remove_file(&path);
    result
}

#[test]
fn regions_are_canonical_and_peeled_from_static_geometry() {
    let seam = streaming_hint_entity(
        "streaming_seam_volume",
        "16 24 32",
        "",
        &box_brush([0, 0, 0], [32, 32, 32], "stream_seam_tex"),
    );
    let resident = streaming_hint_entity(
        "stream_resident_volume",
        "48 56 64",
        "",
        &box_brush([40, 0, 0], [72, 32, 32], "stream_resident_tex"),
    );
    let priority = streaming_hint_entity(
        "stream_priority_region",
        "80 88 96",
        "\"_stream_priority\" \"3\"",
        &box_brush([80, 0, 0], [112, 32, 32], "stream_priority_tex"),
    );
    let map = parse_inline_map(&streaming_hint_map(&format!(
        "{seam}\n{resident}\n{priority}"
    )))
    .expect("streaming hint fixture must parse");

    assert_eq!(map.streaming_seam_regions.len(), 1);
    assert_eq!(map.stream_resident_regions.len(), 1);
    assert_eq!(map.stream_priority_regions.len(), 1);
    for region in map
        .streaming_seam_regions
        .iter()
        .chain(&map.stream_resident_regions)
        .chain(std::iter::once(&map.stream_priority_regions[0].region))
    {
        assert_eq!(region.planes.len(), 6, "box keeps its convex hull planes");
        assert!(
            region
                .min
                .iter()
                .zip(region.max)
                .all(|(min, max)| min.is_finite() && max.is_finite() && min < &max),
            "region must retain a finite positive-volume engine-space AABB: {region:?}"
        );
        assert!(
            region.source_location.iter().all(|value| value.is_finite()),
            "region must retain an engine-space source location: {region:?}"
        );
    }
    assert_eq!(map.stream_priority_regions[0].priority, 3);

    let scale = MapFormat::IdTech2.units_to_meters();
    let expected_location = quake_to_engine(DVec3::new(80.0, 88.0, 96.0)) * scale;
    for (actual, expected) in map.stream_priority_regions[0]
        .region
        .source_location
        .iter()
        .zip(expected_location.to_array())
    {
        assert!((f64::from(*actual) - expected).abs() < 1.0e-6);
    }

    let hint_textures = [
        "stream_seam_tex",
        "stream_resident_tex",
        "stream_priority_tex",
    ];
    assert!(
        map.brush_volumes
            .iter()
            .flat_map(|brush| brush.sides.iter())
            .all(|side| !hint_textures.contains(&side.texture.as_str())),
        "streaming hints must not enter static BSP inputs"
    );
    assert!(
        map.map_entities.iter().all(|entity| {
            !matches!(
                entity.classname.as_str(),
                "streaming_seam_volume" | "stream_resident_volume" | "stream_priority_region"
            )
        }),
        "streaming hints must not become runtime MapEntity records"
    );
    let partitioned = crate::partition::partition(&map.brush_volumes).unwrap();
    let geometry =
        crate::geometry::extract_geometry(&partitioned.faces, &partitioned.tree, &HashSet::new());
    assert!(
        geometry
            .texture_names
            .names
            .iter()
            .all(|texture| !hint_textures.contains(&texture.as_str())),
        "streaming hint textures must not survive static geometry extraction"
    );
}

#[test]
fn rejects_invalid_brush_counts_and_priorities() {
    for classname in [
        "streaming_seam_volume",
        "stream_resident_volume",
        "stream_priority_region",
    ] {
        let map = streaming_hint_map(&streaming_hint_entity(classname, "1 2 3", "", ""));
        let error = parse_inline_map(&map).expect_err("brushless hint must fail");
        let message = error.to_string();
        assert!(
            message.contains(classname)
                && message.contains("no brushes")
                && message.contains("at ("),
            "error must name the classname and location: {message}"
        );

        let brushes = format!(
            "{}\n{}",
            box_brush([0, 0, 0], [32, 32, 32], "first_hint_tex"),
            box_brush([40, 0, 0], [72, 32, 32], "second_hint_tex"),
        );
        let map = streaming_hint_map(&streaming_hint_entity(classname, "1 2 3", "", &brushes));
        let error = parse_inline_map(&map).expect_err("multi-brush hint must fail");
        let message = error.to_string();
        assert!(
            message.contains(classname)
                && message.contains("owns 2 brushes")
                && message.contains("at ("),
            "error must name the classname and location: {message}"
        );
    }

    for value in ["1.5", "-1", "4", "NaN", "inf"] {
        let map = streaming_hint_map(&streaming_hint_entity(
            "stream_priority_region",
            "1 2 3",
            &format!("\"_stream_priority\" \"{value}\""),
            &box_brush([0, 0, 0], [32, 32, 32], "priority_tex"),
        ));
        let error = parse_inline_map(&map).expect_err("invalid priority must fail");
        let message = error.to_string();
        assert!(
            message.contains("stream_priority_region")
                && message.contains("_stream_priority")
                && message.contains("at ("),
            "priority error must name the field and location for {value}: {message}"
        );
    }

    let blank = streaming_hint_map(&streaming_hint_entity(
        "stream_priority_region",
        "1 2 3",
        "\"_stream_priority\" \"\"",
        &box_brush([0, 0, 0], [32, 32, 32], "blank_priority_tex"),
    ));
    assert_eq!(
        parse_inline_map(&blank)
            .expect("blank priority is the documented zero default")
            .stream_priority_regions[0]
            .priority,
        0
    );
}

#[test]
fn rejects_zero_volume_and_nonfinite_hulls() {
    let zero_volume = streaming_hint_map(&streaming_hint_entity(
        "streaming_seam_volume",
        "1 2 3",
        "",
        &box_brush([0, 0, 0], [0, 32, 32], "flat_hint_tex"),
    ));
    let error = parse_inline_map(&zero_volume).expect_err("collapsed hull must fail");
    let message = error.to_string();
    assert!(
        message.contains("streaming_seam_volume")
            && message.contains("brush hull")
            && message.contains("at ("),
        "hull error must name the classname and location: {message}"
    );

    let nonfinite_brush = box_brush([0, 0, 0], [32, 32, 32], "nonfinite_hint_tex").replacen(
        "( 0 0 0 )",
        "( nan 0 0 )",
        1,
    );
    let nonfinite = streaming_hint_map(&streaming_hint_entity(
        "stream_resident_volume",
        "1 2 3",
        "",
        &nonfinite_brush,
    ));
    let error = parse_inline_map(&nonfinite).expect_err("non-finite hull must fail");
    let message = error.to_string();
    assert!(
        message.contains("stream_resident_volume")
            && message.contains("brush hull")
            && message.contains("at ("),
        "hull error must name the classname and location: {message}"
    );
}
