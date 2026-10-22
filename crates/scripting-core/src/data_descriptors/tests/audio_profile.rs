// Tests: manifest `audio.attenuation` and `audio.ghost` drains, QuickJS and
// Luau twins.

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
        ghost: ModGhostAudio::FLOOR,
    }
}

fn ghost(volume: f32, high_pass_hz: f32, low_pass_hz: f32, drive: f32) -> ModAudioProfile {
    ModAudioProfile {
        ghost: ModGhostAudio {
            volume,
            high_pass_hz,
            low_pass_hz,
            drive,
        },
        ..ModAudioProfile::default()
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
fn audio_profile_sequence_style_table_warns_the_same_as_a_js_array_and_uses_default() {
    // Regression: Luau has no array/object distinction, so `audio = { 1, 2 }`
    // (a sequence-style table, the Luau analog of a JS array) matched the
    // generic `LuaValue::Table` arm and applied the seed with no warning,
    // unlike QuickJS's `Array.isArray` rejection of `audio: []`.
    for (js, luau) in [
        ("{ audio: [1, 2, 3] }", "{ audio = { 1, 2, 3 } }"),
        ("{ audio: [1] }", "{ audio = { 1 } }"),
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
            "{ audio: { attenuation: [2, 60] } }",
            "{ audio = { attenuation = { 2, 60 } } }",
        ),
        (
            "{ audio: { attenuation: [1] } }",
            "{ audio = { attenuation = { 1 } } }",
        ),
    ] {
        assert_twins_warn_and_default(
            js,
            luau,
            "`audio.attenuation` must be an object; using the default attenuation",
            "`audio.attenuation` must be a table; using the default attenuation",
        );
    }
    // A manifest with a sequence-style `audio` block still falls back to the
    // seeded default and loads, never rejecting the mod.
    assert_eq!(
        drain_lua("{ audio = { 1, 2 } }"),
        ModAudioProfile::default()
    );
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

#[test]
fn audio_profile_default_ghost_is_the_floor_profile_and_applies_no_distortion() {
    // Plan delegated answer (provisional): volume 0.6, 200 Hz to 4 kHz, drive 0.
    assert_eq!(ModAudioProfile::default().ghost, ModGhostAudio::FLOOR);
    assert_eq!(
        ModGhostAudio::FLOOR,
        ModGhostAudio {
            volume: 0.6,
            high_pass_hz: 200.0,
            low_pass_hz: 4000.0,
            drive: 0.0,
        }
    );
    // AC: the floor bus profile applies no distortion. Drive 0 is the audio
    // bus's no-distortion point (`postretro-audio` ghost bus tests).
    assert_eq!(ModGhostAudio::FLOOR.drive, 0.0);
}

#[test]
fn audio_profile_ghost_block_parses_in_both_runtimes_without_attenuation() {
    // Regression guard: the drains once returned early when
    // `audio.attenuation` was absent, which would skip `audio.ghost`.
    assert_twins_resolve_silently(
        "{ audio: { ghost: { volume: 0.7, highPassHz: 250, lowPassHz: 3000, drive: 0.3 } } }",
        "{ audio = { ghost = { volume = 0.7, highPassHz = 250, lowPassHz = 3000, drive = 0.3 } } }",
        ghost(0.7, 250.0, 3000.0, 0.3),
    );
    assert_twins_resolve_silently(
        "{ audio: { ghost: { volume: 0, highPassHz: 20, lowPassHz: 20000, drive: 1 } } }",
        "{ audio = { ghost = { volume = 0, highPassHz = 20, lowPassHz = 20000, drive = 1 } } }",
        ghost(0.0, 20.0, 20000.0, 1.0),
    );
}

#[test]
fn audio_profile_attenuation_and_ghost_blocks_resolve_together() {
    assert_twins_resolve_silently(
        "{ audio: { attenuation: { maxDistance: 90 }, ghost: { drive: 0.5 } } }",
        "{ audio = { attenuation = { maxDistance = 90 }, ghost = { drive = 0.5 } } }",
        ModAudioProfile {
            attenuation: attenuation(2.0, 90.0, ModAttenuationCurve::Linear).attenuation,
            ghost: ModGhostAudio {
                drive: 0.5,
                ..ModGhostAudio::FLOOR
            },
        },
    );
}

#[test]
fn audio_profile_partial_ghost_block_takes_floor_values_for_missing_fields() {
    assert_twins_resolve_silently(
        "{ audio: { ghost: { lowPassHz: 6000 } } }",
        "{ audio = { ghost = { lowPassHz = 6000 } } }",
        ghost(0.6, 200.0, 6000.0, 0.0),
    );
    assert_twins_resolve_silently(
        "{ audio: { ghost: { volume: 0.9, drive: null } } }",
        "{ audio = { ghost = { volume = 0.9 } } }",
        ghost(0.9, 200.0, 4000.0, 0.0),
    );
}

#[test]
fn audio_profile_omitted_ghost_block_uses_the_floor_silently() {
    for (js, luau) in [
        ("{ audio: { ghost: null } }", "{ audio = { ghost = nil } }"),
        (
            "{ audio: { ghost: undefined } }",
            "{ audio = { ghost = nil } }",
        ),
        ("{ audio: { ghost: {} } }", "{ audio = { ghost = {} } }"),
    ] {
        assert_twins_resolve_silently(js, luau, ModAudioProfile::default());
    }
}

/// Drain one malformed `audio.ghost` fixture in each runtime, beside an
/// authored attenuation: both apply the whole floor profile, keep the
/// attenuation, and warn exactly once with `warning`.
fn assert_twins_warn_and_floor(js_ghost: &str, luau_ghost: &str, warning: &str) {
    let expected = attenuation(2.0, 90.0, ModAttenuationCurve::Linear);
    let js = format!("{{ audio: {{ attenuation: {{ maxDistance: 90 }}, ghost: {js_ghost} }} }}");
    let luau =
        format!("{{ audio = {{ attenuation = {{ maxDistance = 90 }}, ghost = {luau_ghost} }} }}");
    let capture = LogCapture::start();
    assert_eq!(drain_js(&js), expected, "QuickJS fixture `{js}`");
    capture.assert_logged_once(Level::Warn, warning);
    capture.clear();
    assert_eq!(drain_lua(&luau), expected, "Luau fixture `{luau}`");
    capture.assert_logged_once(Level::Warn, warning);
}

// AC: a malformed `audio.ghost` field applies the whole floor profile and warns
// naming the field; both runtimes judge alike.
#[test]
fn audio_profile_malformed_ghost_field_warns_naming_it_and_applies_the_whole_floor() {
    let warning = |field: &str, range: &str| {
        format!(
            "`audio.ghost.{field}` must be a finite number in {range}; using the floor ghost bus profile"
        )
    };
    for (js, luau, field, range) in [
        (
            "{ volume: 1.5, lowPassHz: 6000 }",
            "{ volume = 1.5, lowPassHz = 6000 }",
            "volume",
            "0..=1",
        ),
        ("{ volume: -0.1 }", "{ volume = -0.1 }", "volume", "0..=1"),
        (
            "{ volume: 'loud' }",
            "{ volume = 'loud' }",
            "volume",
            "0..=1",
        ),
        ("{ drive: 2 }", "{ drive = 2 }", "drive", "0..=1"),
        ("{ drive: true }", "{ drive = true }", "drive", "0..=1"),
        ("{ drive: NaN }", "{ drive = 0/0 }", "drive", "0..=1"),
        (
            "{ highPassHz: 10 }",
            "{ highPassHz = 10 }",
            "highPassHz",
            "20..=20000",
        ),
        (
            "{ highPassHz: Infinity }",
            "{ highPassHz = math.huge }",
            "highPassHz",
            "20..=20000",
        ),
        (
            "{ lowPassHz: 25000 }",
            "{ lowPassHz = 25000 }",
            "lowPassHz",
            "20..=20000",
        ),
        (
            "{ lowPassHz: [3000] }",
            "{ lowPassHz = { 3000 } }",
            "lowPassHz",
            "20..=20000",
        ),
    ] {
        assert_twins_warn_and_floor(js, luau, &warning(field, range));
    }
}

#[test]
fn audio_profile_ghost_high_pass_at_or_above_low_pass_counts_as_malformed() {
    for (js, luau, high, low) in [
        (
            "{ highPassHz: 3000, lowPassHz: 3000 }",
            "{ highPassHz = 3000, lowPassHz = 3000 }",
            3000,
            3000,
        ),
        (
            "{ highPassHz: 5000, lowPassHz: 1000 }",
            "{ highPassHz = 5000, lowPassHz = 1000 }",
            5000,
            1000,
        ),
        // Judged after absent fields take the floor: floor lowPassHz is 4000.
        ("{ highPassHz: 4500 }", "{ highPassHz = 4500 }", 4500, 4000),
        // Floor highPassHz is 200.
        ("{ lowPassHz: 150 }", "{ lowPassHz = 150 }", 200, 150),
    ] {
        assert_twins_warn_and_floor(
            js,
            luau,
            &format!(
                "`audio.ghost.highPassHz` ({high}) must be below `audio.ghost.lowPassHz` ({low}); using the floor ghost bus profile"
            ),
        );
    }
}

#[test]
fn audio_profile_non_object_ghost_block_warns_and_uses_the_floor() {
    for (js, luau) in [("'muffled'", "'muffled'"), ("[0.7]", "{ 0.7 }"), ("3", "3")] {
        let capture = LogCapture::start();
        let expected = attenuation(2.0, 90.0, ModAttenuationCurve::Linear);
        assert_eq!(
            drain_js(&format!(
                "{{ audio: {{ attenuation: {{ maxDistance: 90 }}, ghost: {js} }} }}"
            )),
            expected
        );
        capture.assert_logged_once(
            Level::Warn,
            "`audio.ghost` must be an object; using the floor ghost bus profile",
        );
        capture.clear();
        assert_eq!(
            drain_lua(&format!(
                "{{ audio = {{ attenuation = {{ maxDistance = 90 }}, ghost = {luau} }} }}"
            )),
            expected
        );
        capture.assert_logged_once(
            Level::Warn,
            "`audio.ghost` must be a table; using the floor ghost bus profile",
        );
    }
}

#[test]
fn audio_profile_malformed_attenuation_leaves_an_authored_ghost_block_standing() {
    let capture = LogCapture::start();
    let expected = ghost(0.8, 300.0, 5000.0, 0.2);
    assert_eq!(
        drain_js(
            "{ audio: { attenuation: 'far', ghost: { volume: 0.8, highPassHz: 300, lowPassHz: 5000, drive: 0.2 } } }"
        ),
        expected
    );
    assert_eq!(
        drain_lua(
            "{ audio = { attenuation = 'far', ghost = { volume = 0.8, highPassHz = 300, lowPassHz = 5000, drive = 0.2 } } }"
        ),
        expected
    );
    capture.assert_not_logged(Level::Warn, "audio.ghost");
}
