//! Parser contracts for the `emissive_volume` brush entity: key fixes, colour,
//! hulls, origin and the volume cap.

use std::collections::HashMap;

use log::Level;
use postretro_level_format::emissive::{EMISSIVE_VALUE_CEILING, MAX_EMISSIVE_VOLUMES};
use postretro_test_log_capture::LogCapture;

use super::*;
use crate::map_data::{MapData, MapEmissiveVolume};
use crate::map_format::MapFormat;
use crate::parse::{parse_map_file, quake_to_engine};

/// Quake map units to engine meters.
const UNIT: f64 = 0.0254;
const LABEL: &str = "entity 7 'emissive_volume'";

fn props(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

/// The fixed keys of one volume, read as the entity walk reads them.
fn read(pairs: &[(&str, &str)]) -> PendingEmissiveVolume {
    // Any id: reading keys never touches the brush.
    let brush = shambler::brush::BrushId(0);
    read_emissive_volume(&props(pairs), 7, vec![brush]).expect("keys never fail the compile")
}

fn range(volume: &PendingEmissiveVolume) -> (f32, f32) {
    (volume.min, volume.max)
}

// Acceptance A4: valid keys, 0 included, compile silently.
#[test]
fn valid_keys_including_zero_compile_silently() {
    let capture = LogCapture::start();
    for (pairs, expected) in [
        (vec![], (0.0, 1.0)),
        (
            vec![("emissive_min", "0"), ("emissive_max", "0")],
            (0.0, 0.0),
        ),
        (
            vec![("emissive_min", "0.25"), ("emissive_max", "4")],
            (0.25, 4.0),
        ),
        (
            vec![("emissive_min", "64"), ("emissive_max", "64")],
            (64.0, 64.0),
        ),
        // A cleared TrenchBroom field is unset.
        (
            vec![("emissive_min", ""), ("emissive_max", " ")],
            (0.0, 1.0),
        ),
    ] {
        let volume = read(&pairs);
        assert_eq!(range(&volume), expected, "{pairs:?}");
        assert_eq!(volume.color, [1.0; 3]);
    }
    let volume = read(&[("emissive_color", "0 0 0"), ("_tags", " bar_sign  lobby ")]);
    assert_eq!(volume.color, [0.0; 3]);
    assert_eq!(volume.tags, ["bar_sign", "lobby"]);
    capture.assert_nothing_above(Level::Info);
}

// Acceptance A4: each bad key compiles, with one warning naming the key.
#[test]
fn each_bad_key_reads_its_fix_and_warns_once_naming_the_key() {
    for (key, raw, expected, warning) in [
        (
            "emissive_min",
            "-2",
            (0.0, 1.0),
            "`emissive_min` -2 is negative; it reads 0",
        ),
        (
            "emissive_max",
            "-0.5",
            (0.0, 0.0),
            "`emissive_max` -0.5 is negative; it reads 0",
        ),
        (
            "emissive_max",
            "NaN",
            (0.0, 1.0),
            "`emissive_max` NaN is not finite",
        ),
        (
            "emissive_max",
            "inf",
            (0.0, 1.0),
            "`emissive_max` inf is not finite",
        ),
        (
            "emissive_min",
            "-inf",
            (0.0, 1.0),
            "`emissive_min` -inf is not finite",
        ),
        (
            "emissive_max",
            "100",
            (0.0, 64.0),
            "`emissive_max` 100 is past the ceiling",
        ),
        (
            "emissive_max",
            "bright",
            (0.0, 1.0),
            "`emissive_max` \"bright\" is not a number",
        ),
        (
            "emissive_min",
            "0.5x",
            (0.0, 1.0),
            "`emissive_min` \"0.5x\" is not a number",
        ),
    ] {
        let capture = LogCapture::start();
        let volume = read(&[(key, raw)]);
        assert_eq!(range(&volume), expected, "{key} {raw}");
        capture.assert_logged_once(Level::Warn, &format!("{LABEL} {warning}"));
        assert_eq!(
            capture
                .records()
                .iter()
                .filter(|record| record.level <= Level::Warn)
                .count(),
            1,
            "{key} {raw}: one warning"
        );
    }
}

// Pin P38: each key is fixed before the pair swaps.
#[test]
fn keys_are_fixed_before_a_max_below_min_swaps() {
    let capture = LogCapture::start();
    let volume = read(&[("emissive_min", "2"), ("emissive_max", "NaN")]);
    assert_eq!(range(&volume), (1.0, 2.0), "(2, NaN) stores min 1, max 2");
    capture.assert_logged_once(
        Level::Warn,
        &format!("{LABEL} `emissive_max` NaN is not finite; it reads its default 1"),
    );
    capture.assert_logged_once(
        Level::Warn,
        &format!("{LABEL} `emissive_max` 1 is below `emissive_min` 2; the two are swapped"),
    );
    drop(capture);

    let capture = LogCapture::start();
    let volume = read(&[("emissive_min", "+inf"), ("emissive_max", "0.5")]);
    assert_eq!(
        range(&volume),
        (0.0, 0.5),
        "(+inf, 0.5) stores min 0, max 0.5"
    );
    capture.assert_logged_once(Level::Warn, "`emissive_min` inf is not finite");
    capture.assert_not_logged(Level::Warn, "swapped");
    drop(capture);

    // A plain swap, and the ceiling fixed before it.
    let capture = LogCapture::start();
    let volume = read(&[("emissive_min", "3"), ("emissive_max", "0.5")]);
    assert_eq!(range(&volume), (0.5, 3.0));
    capture.assert_logged_once(Level::Warn, "swapped");
    let volume = read(&[("emissive_min", "900"), ("emissive_max", "70")]);
    assert_eq!(
        range(&volume),
        (64.0, 64.0),
        "both read the ceiling: no swap"
    );
    capture.assert_logged_once(Level::Warn, "`emissive_min` 900 is past the ceiling");
    capture.assert_logged_once(Level::Warn, "`emissive_max` 70 is past the ceiling");
    capture.assert_logged_once(Level::Warn, "swapped");
}

// Acceptance A3 (compile half): `emissive_color` reads as every FGD colour
// key does: channel ÷ 255, linear; malformed warns and reads white.
#[test]
fn emissive_color_reads_channels_over_255_as_linear_and_malformed_as_white() {
    let capture = LogCapture::start();
    let volume = read(&[("emissive_color", "255 128 0")]);
    assert_eq!(volume.color[0], 1.0);
    assert!((volume.color[1] - 0.502).abs() < 1e-3, "{:?}", volume.color);
    assert_eq!(volume.color[2], 0.0);
    capture.assert_nothing_above(Level::Info);
    drop(capture);

    for raw in [
        "255 128",
        "255 128 0 9",
        "256 0 0",
        "-1 0 0",
        "1.5 0 0",
        "red",
    ] {
        let capture = LogCapture::start();
        let volume = read(&[("emissive_color", raw)]);
        assert_eq!(volume.color, [1.0; 3], "{raw}");
        capture.assert_logged_once(
            Level::Warn,
            &format!("{LABEL} `emissive_color` \"{raw}\" is not three integers 0 to 255"),
        );
    }
}

// Pin P50: a texture at the ceiling inside a volume at the ceiling, white,
// at brightness 1, glows finite in the `Rgba16Float` scene target, and so
// does every level of bloom's additive pyramid and its composite.
#[test]
fn ceiling_glow_stays_finite_in_the_half_float_scene_and_bloom_pyramid() {
    const HALF_MAX: f32 = 65504.0;
    let bloom = include_str!("../../../renderer/src/render/bloom.rs");
    let constant = |declaration: &str| -> f32 {
        let at = bloom
            .find(declaration)
            .unwrap_or_else(|| panic!("bloom.rs declares `{declaration}`"));
        let value = &bloom[at + declaration.len()..];
        value[..value.find(';').expect("the declaration ends")]
            .trim()
            .parse()
            .expect("a numeric literal")
    };
    let levels = constant("const BLOOM_LEVEL_COUNT: usize =");
    let intensity = constant("pub const BLOOM_INTENSITY: f32 =");
    assert!(levels >= 1.0);

    let white_texel = 1.0;
    let glow = EMISSIVE_VALUE_CEILING * EMISSIVE_VALUE_CEILING * white_texel;
    // Each downsample and blur averages, so a level holds at most the
    // brightest scene value; the upsample chain adds every level into the
    // one above it, and the composite adds `intensity` of the sum.
    let pyramid = glow * levels;
    let composite = glow + intensity * pyramid;
    for (what, value) in [
        ("glow", glow),
        ("pyramid", pyramid),
        ("composite", composite),
    ] {
        assert!(
            value.is_finite() && value < HALF_MAX,
            "{what} {value} overflows Rgba16Float"
        );
    }
}

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

fn volume_entity(properties: &str, brushes: &[String]) -> String {
    format!(
        "{{\n\"classname\" \"emissive_volume\"\n{properties}{}\n}}",
        brushes.join("\n")
    )
}

/// A worldspawn box (entity 0), then `volumes`.
fn map_with(volumes: &[String]) -> String {
    format!(
        "{{\n\"classname\" \"worldspawn\"\n{}\n}}\n{}",
        box_brush([0, 0, 0], [64, 64, 64], "world_tex"),
        volumes.join("\n")
    )
}

fn parse_inline(map_text: &str) -> anyhow::Result<MapData> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "postretro-emissive-volume-map-{}-{id}.map",
        std::process::id(),
    ));
    std::fs::write(&path, format!("{}\n", map_text.trim()))?;
    let result = parse_map_file(&path, MapFormat::IdTech2);
    let _ = std::fs::remove_file(&path);
    result
}

fn engine(q: [f64; 3]) -> glam::DVec3 {
    quake_to_engine(glam::DVec3::from(q)) * UNIT
}

// Several brushes per entity: one hull each, never their plane intersection.
#[test]
fn each_brush_is_its_own_hull_and_never_joins_the_world() {
    let map = parse_inline(&map_with(&[volume_entity(
        "\"emissive_max\" \"4\"\n\"emissive_color\" \"255 0 0\"\n\"_tags\" \"sign\"\n",
        &[
            box_brush([0, 0, 0], [32, 32, 32], "trigger"),
            box_brush([64, 0, 0], [96, 64, 32], "trigger"),
        ],
    )]))
    .unwrap();
    assert_eq!(map.brush_volumes.len(), 1, "only the world brush renders");
    let [volume]: [MapEmissiveVolume; 1] = map.emissive_volumes.try_into().unwrap();
    assert_eq!(volume.source_entity, 1);
    assert_eq!(volume.source_label, "entity 1 'emissive_volume'");
    assert_eq!(
        (volume.min, volume.max, volume.color),
        (0.0, 4.0, [1.0, 0.0, 0.0])
    );
    assert_eq!(volume.tags, ["sign"]);
    assert_eq!(volume.hulls.len(), 2);

    // Each hull holds its own brush's middle; a plane intersection of the
    // two would hold neither.
    let inside = |hull: &crate::map_data::MapEmissiveHull, q: [f64; 3]| {
        let p = engine(q);
        hull.planes.iter().all(|&(n, d)| n.dot(p) - d <= 1e-9)
    };
    assert!(inside(&volume.hulls[0], [16.0, 16.0, 16.0]));
    assert!(inside(&volume.hulls[1], [80.0, 32.0, 16.0]));
    assert!(!inside(&volume.hulls[0], [80.0, 32.0, 16.0]));

    // Summed brush volume: 32³ + 32·64·32 cubic units.
    let expected = (32.0f64.powi(3) + 32.0 * 64.0 * 32.0) * UNIT.powi(3);
    assert!((volume.summed_volume - expected).abs() < expected * 1e-9);
    // Origin: the centre of the union AABB, map (48, 32, 16).
    let origin = engine([48.0, 32.0, 16.0]);
    for (actual, expected) in volume.origin.iter().zip(origin.to_array()) {
        assert!(
            (f64::from(*actual) - expected).abs() < 1e-6,
            "{:?}",
            volume.origin
        );
    }
}

#[test]
fn a_brushless_emissive_volume_fails_naming_the_entity() {
    let error = parse_inline(&map_with(&[
        "{\n\"classname\" \"emissive_volume\"\n\"origin\" \"0 0 0\"\n}".to_string(),
    ]))
    .expect_err("a point emissive_volume has nothing to select with");
    assert!(
        format!("{error:#}").contains("entity 1 'emissive_volume' has no brushes"),
        "{error:#}"
    );
}

// Acceptance A5: volume 256 compiles; volume 257 fails, naming the count.
#[test]
fn the_256th_volume_compiles_and_the_257th_fails_naming_the_count() {
    let volumes = |count: usize| -> Vec<String> {
        (0..count)
            .map(|index| {
                let x = (index % 16) as i32 * 8;
                let y = (index / 16) as i32 * 8;
                volume_entity("", &[box_brush([x, y, 0], [x + 4, y + 4, 4], "trigger")])
            })
            .collect()
    };
    let map = parse_inline(&map_with(&volumes(MAX_EMISSIVE_VOLUMES))).unwrap();
    assert_eq!(map.emissive_volumes.len(), MAX_EMISSIVE_VOLUMES);

    let error = parse_inline(&map_with(&volumes(MAX_EMISSIVE_VOLUMES + 1)))
        .expect_err("past the cap fails");
    let message = format!("{error:#}");
    assert!(
        message.contains("the map has 257 emissive_volume entities")
            && message.contains("at most 256"),
        "{message}"
    );
}
