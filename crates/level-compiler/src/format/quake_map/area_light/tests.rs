//! `light_area` key rules: contradictory keys fail, inert keys warn and change
//! nothing, `_glow` and `_falloff_range` read as the brief fixes them.

use std::collections::HashMap;

use log::Level;
use postretro_test_log_capture::LogCapture;

use super::*;
use crate::map_data::LightAnimation;

const LABEL: &str = "entity 3 'light_area'";

fn props(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

fn translate(pairs: &[(&str, &str)]) -> Result<(MapLight, AreaLightKeys), TranslateError> {
    translate_area_light(&props(pairs), LABEL)
}

const BASE: [(&str, &str); 2] = [("light", "450"), ("_color", "255 128 0")];

fn with(extra: (&'static str, &'static str)) -> Vec<(&'static str, &'static str)> {
    let mut pairs = BASE.to_vec();
    pairs.push(extra);
    pairs
}

// Acceptance row 30: requests an area light cannot honour fail, by name.
#[test]
fn contradictory_keys_fail_naming_the_light_and_key() {
    for (key, value) in [
        ("_shadow_type", "sdf"),
        ("direction_curve", "[0, 0, -1, 0]"),
        ("style", "5"),
    ] {
        let error = translate(&with((key, value)))
            .expect_err("a contradictory key fails the build")
            .to_string();
        assert!(
            error.contains(LABEL) && error.contains(key),
            "{key}: error names neither light nor key: {error}"
        );
    }
    // The values that ask for nothing an area light lacks compile.
    for (key, value) in [("_shadow_type", "static_light_map"), ("style", "0")] {
        translate(&with((key, value))).unwrap_or_else(|e| panic!("{key} {value}: {e}"));
    }
}

// Acceptance rows 30 and 31: each inert key warns once, naming the light and
// key, and leaves the record, so every cache key, byte for byte unchanged.
#[test]
fn inert_keys_warn_once_each_and_leave_the_record_unchanged() {
    let (base, base_keys) = translate(&BASE).expect("base translates");
    let base_bytes = postcard::to_allocvec(&base).expect("encodes");
    for (key, value) in [
        ("delay", "2"),
        ("delay", "0"),
        ("_light_size", "16"),
        ("_cone", "30"),
        ("_cone2", "45"),
        ("angles", "-90 0 0"),
        ("angle", "90"),
        ("mangle", "0 90 0"),
        ("target", "somewhere"),
    ] {
        let capture = LogCapture::start();
        let (light, keys) = translate(&with((key, value))).expect("an inert key compiles");
        capture.assert_logged_once(Level::Warn, &format!("{LABEL}: `{key}`"));
        assert_eq!(capture.records().len(), 1, "{key}: only its own warning");
        drop(capture);
        assert_eq!(light, base, "{key} changed the record");
        assert_eq!(keys, base_keys, "{key} changed the side keys");
        assert_eq!(
            postcard::to_allocvec(&light).expect("encodes"),
            base_bytes,
            "{key} changed the record's bytes"
        );
    }
}

// Every field an area light does not read is normalised, whatever was authored.
#[test]
fn translation_normalises_every_field_an_area_light_does_not_read() {
    let (light, keys) = translate(&[
        ("light", "600"),
        ("_cast_entity_shadows", "1"),
        ("carrier", "lift_a"),
        ("_tags", " skylight  hall "),
    ])
    .expect("translates");
    assert_eq!(light.light_type, LightType::Area);
    assert_eq!(light.intensity, 2.0);
    assert_eq!(light.color, [1.0; 3]);
    assert_eq!(light.falloff_model, FalloffModel::InverseSquared);
    assert_eq!((light.light_size, light.angular_diameter), (0.0, 0.0));
    assert_eq!(
        (
            light.cone_angle_inner,
            light.cone_angle_outer,
            light.cone_direction
        ),
        (None, None, None)
    );
    assert_eq!(light.shadow_type, ShadowType::StaticLightMap);
    assert!(!light.is_dynamic && !light.casts_entity_shadows);
    assert!(light.carrier.is_empty());
    assert_eq!(light.tags, ["skylight", "hall"]);
    assert!(light.animation.is_none() && !light.is_animated);
    assert!(light.area.is_none(), "polygons resolve after partitioning");
    assert_eq!(keys.reach_override, None);
    assert_eq!(keys.glow, DEFAULT_AREA_GLOW);
}

// Acceptance row 13 (translation part): the override is optional and must
// be positive; it reads in map units like every distance key.
#[test]
fn falloff_range_is_optional_positive_and_in_map_units() {
    let (_, keys) = translate(&BASE).unwrap();
    assert_eq!(keys.reach_override, None);
    let (light, keys) = translate(&with(("_falloff_range", "100"))).unwrap();
    assert_eq!(keys.reach_override, Some(100.0 * 0.0254));
    assert_eq!(
        light.falloff_range, 0.0,
        "the record's reach resolves later"
    );
    for bad in ["0", "-8"] {
        let error = translate(&with(("_falloff_range", bad)))
            .expect_err("a non-positive reach fails")
            .to_string();
        assert!(error.contains("_falloff_range"), "{error}");
    }
}

// Acceptance row 45 (key rules): default 4, 64 silent, past 64 reads 64 with
// one warning naming light and key, negative or non-number fails.
#[test]
fn glow_defaults_clamps_at_the_ceiling_and_refuses_negatives() {
    let capture = LogCapture::start();
    assert_eq!(translate(&BASE).unwrap().1.glow, 4.0);
    for (raw, expected) in [
        ("0", 0.0),
        ("0.5", 0.5),
        ("8", 8.0),
        ("64", 64.0),
        ("-0", 0.0),
    ] {
        assert_eq!(translate(&with(("_glow", raw))).unwrap().1.glow, expected);
    }
    capture.assert_nothing_above(Level::Info);
    capture.clear();

    assert_eq!(translate(&with(("_glow", "100"))).unwrap().1.glow, 64.0);
    capture.assert_logged_once(Level::Warn, &format!("{LABEL}: `_glow` 100"));
    drop(capture);

    for bad in ["-1", "bright", "NaN"] {
        let error = translate(&with(("_glow", bad)))
            .expect_err("a negative or non-number glow fails")
            .to_string();
        assert!(
            error.contains(LABEL) && error.contains("_glow"),
            "{bad}: {error}"
        );
    }
}

// Acceptance row 55 (reservation part): `_start_inactive 1` reserves the
// animated bake as `_animated 1` does, and starts dark.
#[test]
fn start_inactive_reserves_the_animated_bake_and_starts_dark() {
    let (light, _) = translate(&with(("_start_inactive", "1"))).unwrap();
    assert!(light.is_animated);
    assert_eq!(
        light.animation,
        Some(crate::map_data::animated_light_placeholder(false))
    );
    let (animated, _) = translate(&with(("_animated", "1"))).unwrap();
    assert_eq!(
        animated.animation,
        Some(crate::map_data::animated_light_placeholder(true))
    );
    // A curve keeps its samples and takes the start state.
    let (curved, _) = translate(&[
        ("_start_inactive", "1"),
        ("brightness_curve", "[0, 0.2] [500, 1]"),
        ("period_ms", "1000"),
    ])
    .unwrap();
    let Some(LightAnimation {
        brightness: Some(samples),
        start_active: false,
        ..
    }) = curved.animation
    else {
        panic!("a curve with start inactive: {:?}", curved.animation);
    };
    assert_eq!(samples.len(), 32);
    // `color_curve` follows the point-light rule, with `_start_inactive`
    // counting as the reservation.
    assert!(translate(&[("color_curve", "[0, 1, 0, 0]"), ("period_ms", "500")]).is_err());
    translate(&[
        ("color_curve", "[0, 1, 0, 0]"),
        ("period_ms", "500"),
        ("_start_inactive", "1"),
    ])
    .expect("a reserved area light may animate colour");
}
