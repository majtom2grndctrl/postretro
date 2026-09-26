// Tests: manifest `audio.attenuation` drains, QuickJS and Luau twins.

use super::super::*;
use super::common::*;
use log::Level;
use postretro_test_log_capture::LogCapture;

const SCOPE: &str = "test manifest";

fn drain_js(manifest: &str) -> ModAudioProfile {
    eval_js(&format!("({manifest})"), |_, value| {
        let obj = Object::from_value(value).expect("fixture must evaluate to an object");
        drain_audio_profile_js(&obj, SCOPE).expect("an audio profile never rejects the manifest")
    })
}

fn drain_lua(manifest: &str) -> ModAudioProfile {
    eval_lua(&format!("return {manifest}"), |value| {
        let LuaValue::Table(table) = value else {
            panic!("fixture must evaluate to a table");
        };
        drain_audio_profile_lua(&table, SCOPE).expect("an audio profile never rejects the manifest")
    })
}

fn attenuation(
    min_distance: f32,
    max_distance: f32,
    curve: ModAttenuationCurve,
) -> ModAudioProfile {
    ModAudioProfile {
        attenuation: ModAttenuation {
            min_distance,
            max_distance,
            curve,
        },
    }
}

/// Drain one fixture in each runtime, asserting both resolve to `expected`
/// and emit no warning at all.
fn assert_twins_resolve_silently(js: &str, luau: &str, expected: ModAudioProfile) {
    let capture = LogCapture::start();
    assert_eq!(drain_js(js), expected, "QuickJS fixture `{js}`");
    assert_eq!(drain_lua(luau), expected, "Luau fixture `{luau}`");
    assert_not_warned(&capture);
}

fn assert_not_warned(capture: &LogCapture) {
    let warnings: Vec<_> = capture
        .records()
        .into_iter()
        .filter(|record| record.level == Level::Warn)
        .collect();
    assert!(
        warnings.is_empty(),
        "expected no warnings, got {warnings:?}"
    );
}

/// Drain one malformed fixture in each runtime: both fall back to the seeded
/// default, both warn exactly once naming the field, and the drain succeeds.
fn assert_twins_warn_and_default(js: &str, luau: &str, js_warning: &str, luau_warning: &str) {
    let capture = LogCapture::start();
    assert_eq!(
        drain_js(js),
        ModAudioProfile::default(),
        "QuickJS fixture `{js}`"
    );
    capture.assert_logged_once(Level::Warn, js_warning);
    capture.clear();
    assert_eq!(
        drain_lua(luau),
        ModAudioProfile::default(),
        "Luau fixture `{luau}`"
    );
    capture.assert_logged_once(Level::Warn, luau_warning);
}

#[test]
fn audio_profile_default_is_the_engine_attenuation_seed() {
    // Plan delegated answer: minDistance 2, maxDistance 60, linear.
    assert_eq!(
        ModAudioProfile::default(),
        attenuation(2.0, 60.0, ModAttenuationCurve::Linear)
    );
}

#[test]
fn audio_profile_full_attenuation_block_parses_in_both_runtimes() {
    assert_twins_resolve_silently(
        r#"{ audio: { attenuation: { minDistance: 4, maxDistance: 120.5, curve: "quadratic" } } }"#,
        r#"{ audio = { attenuation = { minDistance = 4, maxDistance = 120.5, curve = "quadratic" } } }"#,
        attenuation(4.0, 120.5, ModAttenuationCurve::Quadratic),
    );
    assert_twins_resolve_silently(
        r#"{ audio: { attenuation: { minDistance: 0, maxDistance: 10, curve: "linear" } } }"#,
        r#"{ audio = { attenuation = { minDistance = 0, maxDistance = 10, curve = "linear" } } }"#,
        attenuation(0.0, 10.0, ModAttenuationCurve::Linear),
    );
}

#[test]
fn audio_profile_partial_attenuation_block_takes_seeded_defaults_for_missing_fields() {
    assert_twins_resolve_silently(
        r#"{ audio: { attenuation: { maxDistance: 90 } } }"#,
        r#"{ audio = { attenuation = { maxDistance = 90 } } }"#,
        attenuation(2.0, 90.0, ModAttenuationCurve::Linear),
    );
    assert_twins_resolve_silently(
        r#"{ audio: { attenuation: { curve: "quadratic" } } }"#,
        r#"{ audio = { attenuation = { curve = "quadratic" } } }"#,
        attenuation(2.0, 60.0, ModAttenuationCurve::Quadratic),
    );
    assert_twins_resolve_silently(
        r#"{ audio: { attenuation: { minDistance: 5, maxDistance: null } } }"#,
        r#"{ audio = { attenuation = { minDistance = 5 } } }"#,
        attenuation(5.0, 60.0, ModAttenuationCurve::Linear),
    );
}

#[test]
fn audio_profile_omitted_block_uses_the_seeded_default_silently() {
    for (js, luau) in [
        ("{ name: 'NoAudio' }", "{ name = 'NoAudio' }"),
        ("{ audio: undefined }", "{ audio = nil }"),
        ("{ audio: null }", "{ audio = nil }"),
        ("{ audio: {} }", "{ audio = {} }"),
        (
            "{ audio: { attenuation: null } }",
            "{ audio = { attenuation = nil } }",
        ),
        (
            "{ audio: { attenuation: {} } }",
            "{ audio = { attenuation = {} } }",
        ),
    ] {
        assert_twins_resolve_silently(js, luau, ModAudioProfile::default());
    }
}

#[test]
fn audio_profile_min_not_below_max_warns_naming_min_distance_and_uses_default() {
    assert_twins_warn_and_default(
        "{ audio: { attenuation: { minDistance: 10, maxDistance: 10 } } }",
        "{ audio = { attenuation = { minDistance = 10, maxDistance = 10 } } }",
        "`audio.attenuation.minDistance` (10) must be less than `audio.attenuation.maxDistance` (10); using the default attenuation",
        "`audio.attenuation.minDistance` (10) must be less than `audio.attenuation.maxDistance` (10); using the default attenuation",
    );
    // A partial block merges with the seed before the ordering check: the
    // seeded minDistance of 2 is not below an authored maxDistance of 1.
    assert_twins_warn_and_default(
        "{ audio: { attenuation: { maxDistance: 1 } } }",
        "{ audio = { attenuation = { maxDistance = 1 } } }",
        "`audio.attenuation.minDistance` (2) must be less than `audio.attenuation.maxDistance` (1)",
        "`audio.attenuation.minDistance` (2) must be less than `audio.attenuation.maxDistance` (1)",
    );
}

#[test]
fn audio_profile_negative_or_non_numeric_distance_warns_naming_the_field_and_uses_default() {
    let min_warning = "`audio.attenuation.minDistance` must be a finite non-negative number; using the default attenuation";
    let max_warning = "`audio.attenuation.maxDistance` must be a finite non-negative number; using the default attenuation";
    for (js, luau, warning) in [
        (
            "{ audio: { attenuation: { minDistance: -1, maxDistance: 50 } } }",
            "{ audio = { attenuation = { minDistance = -1, maxDistance = 50 } } }",
            min_warning,
        ),
        (
            "{ audio: { attenuation: { maxDistance: -5 } } }",
            "{ audio = { attenuation = { maxDistance = -5 } } }",
            max_warning,
        ),
        (
            "{ audio: { attenuation: { minDistance: 'near' } } }",
            "{ audio = { attenuation = { minDistance = 'near' } } }",
            min_warning,
        ),
        (
            "{ audio: { attenuation: { maxDistance: true } } }",
            "{ audio = { attenuation = { maxDistance = true } } }",
            max_warning,
        ),
        (
            "{ audio: { attenuation: { maxDistance: Infinity } } }",
            "{ audio = { attenuation = { maxDistance = math.huge } } }",
            max_warning,
        ),
        (
            "{ audio: { attenuation: { minDistance: NaN } } }",
            "{ audio = { attenuation = { minDistance = 0/0 } } }",
            min_warning,
        ),
        (
            "{ audio: { attenuation: { maxDistance: 1e40 } } }",
            "{ audio = { attenuation = { maxDistance = 1e40 } } }",
            max_warning,
        ),
    ] {
        assert_twins_warn_and_default(js, luau, warning, warning);
    }
}

#[test]
fn audio_profile_unknown_curve_warns_naming_the_field_and_uses_default() {
    let warning =
        "`audio.attenuation.curve` must be `linear` or `quadratic`; using the default attenuation";
    for (js, luau) in [
        (
            "{ audio: { attenuation: { minDistance: 1, maxDistance: 30, curve: 'cubic' } } }",
            "{ audio = { attenuation = { minDistance = 1, maxDistance = 30, curve = 'cubic' } } }",
        ),
        (
            "{ audio: { attenuation: { curve: 'Linear' } } }",
            "{ audio = { attenuation = { curve = 'Linear' } } }",
        ),
        (
            "{ audio: { attenuation: { curve: 2 } } }",
            "{ audio = { attenuation = { curve = 2 } } }",
        ),
    ] {
        assert_twins_warn_and_default(js, luau, warning, warning);
    }
}

#[test]
fn audio_profile_non_object_blocks_warn_naming_the_path_and_use_default() {
    for (js, luau) in [
        ("{ audio: 7 }", "{ audio = 7 }"),
        ("{ audio: 'loud' }", "{ audio = 'loud' }"),
        ("{ audio: [] }", "{ audio = true }"),
    ] {
        assert_twins_warn_and_default(
            js,
            luau,
            "`audio` must be an object; using the default audio profile",
            "`audio` must be a table; using the default audio profile",
        );
    }
    for (js, luau) in [
        (
            "{ audio: { attenuation: 'far' } }",
            "{ audio = { attenuation = 'far' } }",
        ),
        (
            "{ audio: { attenuation: [2, 60] } }",
            "{ audio = { attenuation = 60 } }",
        ),
    ] {
        assert_twins_warn_and_default(
            js,
            luau,
            "`audio.attenuation` must be an object; using the default attenuation",
            "`audio.attenuation` must be a table; using the default attenuation",
        );
    }
}

#[test]
fn audio_profile_several_malformed_fields_each_warn_once_and_use_default() {
    let capture = LogCapture::start();
    let js = "{ audio: { attenuation: { minDistance: -1, maxDistance: 'far', curve: 'cubic' } } }";
    let luau =
        "{ audio = { attenuation = { minDistance = -1, maxDistance = 'far', curve = 'cubic' } } }";
    for profile in [drain_js(js), drain_lua(luau)] {
        assert_eq!(profile, ModAudioProfile::default());
    }
    for field in ["minDistance", "maxDistance", "curve"] {
        let records = capture
            .records()
            .into_iter()
            .filter(|record| {
                record.level == Level::Warn
                    && record
                        .message
                        .contains(&format!("`audio.attenuation.{field}` must be"))
            })
            .count();
        assert_eq!(records, 2, "`{field}` must warn once per runtime");
    }
    // Malformed fields already chose the default; the ordering check must not
    // pile a second, misleading warning on top.
    capture.assert_not_logged(Level::Warn, "must be less than");
}
