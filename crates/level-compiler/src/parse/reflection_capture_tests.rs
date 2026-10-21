//! Parser contracts for the `_reflection_capture` override: which entities
//! carry it, how it lands on brushes, and its warnings.

use std::collections::{BTreeMap, HashMap};

use log::Level;
use postretro_test_log_capture::LogCapture;
use shambler::GeoMap;

use crate::map_data::{MapData, ReflectionCapture};
use crate::map_format::MapFormat;

use super::map_source::{normalize_map_source, parse_complete_map};
use super::reflection_capture::{
    REFLECTION_CAPTURE_VALUES, ReflectionCaptureOverrides, reflection_capture_spelling,
};
use super::{build_brush_volumes_with_ids, get_property, parse_map_file};

const INVALID_VALUE_WARNING: &str = "`_reflection_capture` value";
const IGNORED_ON_CONTAINER_WARNING: &str = "is a TrenchBroom group or layer, which keeps no custom keys on save; ignoring `_reflection_capture`";
const EMPTY_MARKER_WARNING: &str = "func_group marker(s) have an empty `_tb_id`";

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

/// A small box brush at slot `n` along Quake +x, textured `texture`.
fn decor(n: i32, texture: &str) -> String {
    let x = 64 * n;
    box_brush([x, 0, 0], [x + 32, 32, 96], texture)
}

/// The closest authored degeneracy the source parser accepts: one plane.
fn degenerate_brush() -> String {
    "{\n( 0 0 0 ) ( 0 1 0 ) ( 0 0 1 ) degenerate_tex 0 0 0 1 1\n}".to_string()
}

fn entity(classname: &str, properties: &[(&str, &str)], brushes: &[String]) -> String {
    let mut lines = vec!["{".to_string(), format!("\"classname\" \"{classname}\"")];
    lines.extend(
        properties
            .iter()
            .map(|(key, value)| format!("\"{key}\" \"{value}\"")),
    );
    lines.extend(brushes.iter().cloned());
    lines.push("}".to_string());
    lines.join("\n")
}

/// Entity 0 is a worldspawn holding one floor brush, textured `world_tex`.
fn map_with(worldspawn_properties: &[(&str, &str)], entities: &[String]) -> String {
    let mut out = vec![entity(
        "worldspawn",
        worldspawn_properties,
        &[box_brush([-512, -512, -64], [512, 512, -32], "world_tex")],
    )];
    out.extend(entities.iter().cloned());
    out.join("\n")
}

fn water(n: i32) -> String {
    let x = 1024 * n;
    entity(
        "fluid_volume",
        &[("fluid", "water")],
        &[box_brush([x, 1024, -32], [x + 256, 1280, 0], "water")],
    )
}

fn parse_inline_map(map_text: &str) -> anyhow::Result<MapData> {
    static NEXT_INLINE_MAP_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT_INLINE_MAP_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "postretro-reflection-capture-map-{}-{id}.map",
        std::process::id(),
    ));
    std::fs::write(&path, format!("{}\n", map_text.trim()))?;
    let result = parse_map_file(&path, MapFormat::IdTech2);
    let _ = std::fs::remove_file(&path);
    result
}

/// Each retained brush's override, keyed by the texture its sides carry.
/// Every brush in these fixtures has its own texture.
fn capture_by_texture(map: &MapData) -> BTreeMap<String, ReflectionCapture> {
    assert_eq!(
        map.brush_volumes.len(),
        map.brush_capture_override.len(),
        "the override table is index-aligned with brush_volumes"
    );
    map.brush_volumes
        .iter()
        .zip(&map.brush_capture_override)
        .filter_map(|(brush, &capture)| {
            let texture = brush.sides.first()?.texture.clone();
            Some((texture, capture))
        })
        .collect()
}

fn reflection_capture_warnings(capture: &LogCapture) -> Vec<String> {
    capture
        .records()
        .into_iter()
        .filter(|record| {
            record.level == Level::Warn && record.message.contains("_reflection_capture")
        })
        .map(|record| record.message)
        .collect()
}

fn switch(name: &str, value: &str, brush: String) -> String {
    entity(
        "switch",
        &[("name", name), ("_reflection_capture", value)],
        &[brush],
    )
}

#[test]
fn func_group_and_switch_reflection_capture_lands_on_every_brush() {
    let capture = LogCapture::start();
    let map = parse_inline_map(&map_with(
        &[],
        &[
            entity(
                "func_group",
                &[("_reflection_capture", "never")],
                &[decor(0, "never_a"), decor(1, "never_b")],
            ),
            entity(
                "func_group",
                &[("_reflection_capture", " always ")],
                &[
                    decor(2, "always_a"),
                    decor(3, "always_b"),
                    decor(4, "always_c"),
                ],
            ),
            entity(
                "func_group",
                &[("_reflection_capture", "auto")],
                &[decor(5, "auto_a")],
            ),
            // A field cleared in TrenchBroom arrives blank: unset, so `auto`.
            entity(
                "func_group",
                &[("_reflection_capture", "")],
                &[decor(6, "cleared_a")],
            ),
            entity("func_group", &[], &[decor(7, "keyless_a")]),
            switch("press_never", "never", decor(8, "switch_never")),
            switch("press_always", "always", decor(9, "switch_always")),
            switch("press_auto", "auto", decor(10, "switch_auto")),
        ],
    ))
    .expect("override fixture parses");

    let expected = BTreeMap::from([
        ("world_tex".to_string(), ReflectionCapture::Auto),
        ("never_a".to_string(), ReflectionCapture::Never),
        ("never_b".to_string(), ReflectionCapture::Never),
        ("always_a".to_string(), ReflectionCapture::Always),
        ("always_b".to_string(), ReflectionCapture::Always),
        ("always_c".to_string(), ReflectionCapture::Always),
        ("auto_a".to_string(), ReflectionCapture::Auto),
        ("cleared_a".to_string(), ReflectionCapture::Auto),
        ("keyless_a".to_string(), ReflectionCapture::Auto),
        ("switch_never".to_string(), ReflectionCapture::Never),
        ("switch_always".to_string(), ReflectionCapture::Always),
        ("switch_auto".to_string(), ReflectionCapture::Auto),
    ]);
    assert_eq!(capture_by_texture(&map), expected);
    assert_eq!(
        map.trigger_volumes.len(),
        3,
        "each switch still compiles to its press volume"
    );
    assert!(
        reflection_capture_warnings(&capture).is_empty(),
        "valid and blank values warn nothing: {:?}",
        reflection_capture_warnings(&capture)
    );
}

// O4: a value is keyed by source brush id and translated at the retained-hull
// seam, so a degenerate brush never moves a value onto a neighbour.
#[test]
fn never_func_group_with_degenerate_first_brush_excludes_its_other_brushes() {
    let map = parse_inline_map(&map_with(
        &[],
        &[
            entity(
                "func_group",
                &[("_reflection_capture", "never")],
                &[degenerate_brush(), decor(0, "never_a"), decor(1, "never_b")],
            ),
            entity(
                "func_group",
                &[("_reflection_capture", "always")],
                &[decor(2, "next_always_a"), decor(3, "next_always_b")],
            ),
            entity("func_group", &[], &[decor(4, "next_auto")]),
        ],
    ))
    .expect("degenerate brush fixture parses");

    let degenerate_index = map
        .brush_volumes
        .iter()
        .position(|brush| brush.planes.len() == 1)
        .expect("the hull builder retains the one-plane brush");
    assert!(map.brush_volumes[degenerate_index].sides.is_empty());
    assert_eq!(
        map.brush_capture_override[degenerate_index],
        ReflectionCapture::Never
    );
    let expected = BTreeMap::from([
        ("world_tex".to_string(), ReflectionCapture::Auto),
        ("never_a".to_string(), ReflectionCapture::Never),
        ("never_b".to_string(), ReflectionCapture::Never),
        ("next_always_a".to_string(), ReflectionCapture::Always),
        ("next_always_b".to_string(), ReflectionCapture::Always),
        ("next_auto".to_string(), ReflectionCapture::Auto),
    ]);
    assert_eq!(capture_by_texture(&map), expected);
}

// The source parser cannot make the hull builder drop a brush (see
// `parsed_degenerate_brace_brush_preserves_following_group_provenance_in_final_map_data`),
// so the dropped case is driven at the seam itself: removing the first brush
// of a `never` group from the retained hulls shifts nothing.
#[test]
fn retained_hull_seam_keeps_values_on_their_own_brushes_when_a_brush_drops() {
    let map_text = map_with(
        &[],
        &[
            entity(
                "func_group",
                &[("_reflection_capture", "never")],
                &[
                    decor(0, "dropped"),
                    decor(1, "never_a"),
                    decor(2, "never_b"),
                ],
            ),
            entity(
                "func_group",
                &[("_reflection_capture", "always")],
                &[decor(3, "next_always")],
            ),
        ],
    );
    let source = normalize_map_source(&map_text);
    let geo_map = GeoMap::new(parse_complete_map(&source, &source.text).expect("fixture parses"));

    let mut overrides = ReflectionCaptureOverrides::default();
    let mut world_brush_ids = Vec::new();
    for (index, entity_id) in geo_map.entities.iter().enumerate() {
        let brush_ids = geo_map
            .entity_brushes
            .get(entity_id)
            .cloned()
            .unwrap_or_default();
        if get_property(&geo_map, entity_id, "classname").as_deref() == Some("func_group") {
            let props: HashMap<String, String> = geo_map.entity_properties[entity_id]
                .iter()
                .map(|p| (p.key.clone(), p.value.clone()))
                .collect();
            overrides.record_func_group(
                &props,
                &format!("entity {index} 'func_group'"),
                &brush_ids,
            );
        }
        world_brush_ids.extend(brush_ids);
    }
    let mut retained = build_brush_volumes_with_ids(&geo_map, &world_brush_ids, 0.0254);
    let dropped = retained
        .iter()
        .position(|(_, brush)| brush.sides[0].texture == "dropped")
        .expect("fixture has the brush to drop");
    retained.remove(dropped);

    let values = overrides.for_retained_hulls(&retained);
    let by_texture: BTreeMap<String, ReflectionCapture> = retained
        .iter()
        .map(|(_, brush)| brush.sides[0].texture.clone())
        .zip(values)
        .collect();
    assert_eq!(
        by_texture,
        BTreeMap::from([
            ("world_tex".to_string(), ReflectionCapture::Auto),
            ("never_a".to_string(), ReflectionCapture::Never),
            ("never_b".to_string(), ReflectionCapture::Never),
            ("next_always".to_string(), ReflectionCapture::Always),
        ])
    );
}

// O5: the warning comes from the entity parse, before probe placement, so the
// count is the same with two pools and with none.
#[test]
fn invalid_reflection_capture_warns_once_naming_the_entity_and_acts_as_auto() {
    let three_brush_group = entity(
        "func_group",
        &[("_reflection_capture", "bogus")],
        &[
            decor(0, "bogus_a"),
            decor(1, "bogus_b"),
            decor(2, "bogus_c"),
        ],
    );
    for (label, extra) in [
        ("two pools", vec![water(1), water(2)]),
        ("no water", Vec::new()),
    ] {
        let capture = LogCapture::start();
        let mut entities = vec![three_brush_group.clone()];
        entities.extend(extra);
        let map = parse_inline_map(&map_with(&[], &entities)).expect("invalid value degrades");

        capture.assert_logged_once(
            Level::Warn,
            "[Compiler] entity 1 'func_group' `_reflection_capture` value `bogus` is not `auto`, \
             `always` or `never`; treating it as `auto`",
        );
        assert_eq!(
            reflection_capture_warnings(&capture).len(),
            1,
            "{label}: one warning for the entity, not one per brush"
        );
        assert!(
            capture_by_texture(&map)
                .values()
                .all(|&value| value == ReflectionCapture::Auto),
            "{label}: an invalid value acts as auto"
        );
    }

    let capture = LogCapture::start();
    let map = parse_inline_map(&map_with(
        &[],
        &[switch("press", "Never", decor(0, "switch_bad"))],
    ))
    .expect("invalid switch value degrades");
    capture.assert_logged_once(
        Level::Warn,
        "entity 1 'switch' `_reflection_capture` value `Never` is not `auto`",
    );
    assert_eq!(
        capture_by_texture(&map)["switch_bad"],
        ReflectionCapture::Auto
    );
}

#[test]
fn authored_func_group_without_tb_keys_raises_no_empty_marker_warning() {
    let authored = entity(
        "func_group",
        &[("_reflection_capture", "always")],
        &[decor(0, "authored")],
    );
    let capture = LogCapture::start();
    let map =
        parse_inline_map(&map_with(&[], std::slice::from_ref(&authored))).expect("authored parses");

    capture.assert_not_logged(Level::Warn, EMPTY_MARKER_WARNING);
    assert_eq!(
        map.brush_volumes.len(),
        2,
        "the authored func_group's brush joins the static world"
    );
    assert!(map.brush_assembly.iter().all(Option::is_none));

    // Beside an authored one, a TrenchBroom group with an empty id still
    // warns, and the count names only the group.
    capture.clear();
    parse_inline_map(&map_with(
        &[],
        &[
            authored,
            entity(
                "func_group",
                &[
                    ("_tb_type", "_tb_group"),
                    ("_tb_name", "empty id group"),
                    ("_tb_id", ""),
                ],
                &[decor(1, "grouped")],
            ),
        ],
    ))
    .expect("group fixture parses");
    capture.assert_logged_once(
        Level::Warn,
        "[Compiler] 1 func_group marker(s) have an empty `_tb_id`",
    );
}

#[test]
fn worldspawn_reflection_capture_never_leaves_world_brushes_auto() {
    let map = parse_inline_map(&map_with(
        &[("_reflection_capture", "never")],
        &[entity("func_group", &[], &[decor(0, "keyless")])],
    ))
    .expect("worldspawn fixture parses");

    assert_eq!(
        capture_by_texture(&map),
        BTreeMap::from([
            ("world_tex".to_string(), ReflectionCapture::Auto),
            ("keyless".to_string(), ReflectionCapture::Auto),
        ])
    );
}

#[test]
fn trenchbroom_group_and_layer_ignore_reflection_capture_with_one_warning_each() {
    let capture = LogCapture::start();
    let map = parse_inline_map(&map_with(
        &[],
        &[
            entity(
                "func_group",
                &[
                    ("_tb_type", "_tb_group"),
                    ("_tb_name", "dock group"),
                    ("_tb_id", "1"),
                    ("_reflection_capture", "never"),
                ],
                &[decor(0, "group_a"), decor(1, "group_b")],
            ),
            entity(
                "func_group",
                &[
                    ("_tb_type", "_tb_layer"),
                    ("_tb_name", "decor layer"),
                    ("_tb_id", "2"),
                    ("_reflection_capture", "always"),
                ],
                &[decor(2, "layer_a"), decor(3, "layer_b")],
            ),
        ],
    ))
    .expect("container fixture parses");

    assert!(
        capture_by_texture(&map)
            .values()
            .all(|&value| value == ReflectionCapture::Auto),
        "a group or layer's key changes no brush"
    );
    capture.assert_logged_once(
        Level::Warn,
        &format!("[Compiler] entity 1 'func_group' {IGNORED_ON_CONTAINER_WARNING} `never`"),
    );
    capture.assert_logged_once(
        Level::Warn,
        &format!("[Compiler] entity 2 'func_group' {IGNORED_ON_CONTAINER_WARNING} `always`"),
    );
    assert_eq!(reflection_capture_warnings(&capture).len(), 2);
    capture.assert_not_logged(Level::Warn, INVALID_VALUE_WARNING);
    assert_eq!(
        map.brush_volumes.len(),
        5,
        "group and layer brushes join the world"
    );
}

/// The FGD's choices block for `_reflection_capture` in `classname`'s class.
fn fgd_reflection_capture_choices(fgd: &str, classname: &str) -> (String, Vec<String>) {
    let class_body = fgd
        .split_once(&format!("= {classname} :"))
        .and_then(|(_, rest)| rest.split_once("\n]"))
        .map(|(body, _)| body)
        .unwrap_or_else(|| panic!("postretro.fgd declares a `{classname}` class"));
    let (_, declaration) = class_body
        .split_once("_reflection_capture(choices)")
        .unwrap_or_else(|| panic!("`{classname}` declares `_reflection_capture(choices)`"));
    // Quoted fields are positional: display name, then default.
    let default = declaration
        .lines()
        .next()
        .and_then(|line| line.split('"').nth(3))
        .unwrap_or_else(|| panic!("`{classname}` `_reflection_capture` needs a quoted default"))
        .to_string();
    let options = declaration
        .split_once('[')
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(options, _)| options)
        .unwrap_or_else(|| panic!("`{classname}` `_reflection_capture` lists its choices"));
    let values = options
        .lines()
        .filter_map(|line| line.trim().strip_prefix('"')?.split_once('"'))
        .map(|(value, _)| value.to_string())
        .collect();
    (default, values)
}

#[test]
fn fgd_func_group_and_switch_offer_every_reflection_capture_value() {
    let fgd_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sdk/TrenchBroom/postretro.fgd"
    );
    let fgd = std::fs::read_to_string(fgd_path).expect("read committed postretro.fgd");
    let fgd = fgd.as_str();
    let expected: Vec<String> = REFLECTION_CAPTURE_VALUES
        .into_iter()
        .map(|value| reflection_capture_spelling(value).to_string())
        .collect();
    for classname in ["func_group", "switch"] {
        let (default, values) = fgd_reflection_capture_choices(fgd, classname);
        assert_eq!(
            default,
            reflection_capture_spelling(ReflectionCapture::default()),
            "`{classname}` defaults to the canonical default"
        );
        assert_eq!(
            values, expected,
            "`{classname}` offers exactly the parser's values"
        );
    }
    assert!(
        fgd.contains("@SolidClass\n    color(200 200 200)\n    = func_group :"),
        "`func_group` is a brush entity"
    );
}
