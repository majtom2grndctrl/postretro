use std::fs;
use std::path::Path;

use super::*;
use log::Level;
use postretro_test_log_capture::LogCapture;
use tempfile::tempdir;

const EPSILON: f32 = 1e-6;

/// The TOML a save would write.
fn to_toml(options: &PlayerOptions) -> String {
    toml::to_string_pretty(&options.to_document()).unwrap()
}

/// Options as `load` would read `text`.
fn from_toml(text: &str) -> PlayerOptions {
    let mut options = PlayerOptions::from_table(text.parse().unwrap(), Path::new("settings.toml"));
    options.sanitize();
    options
}

fn assert_options_eq(a: &PlayerOptions, b: &PlayerOptions) {
    assert_eq!(a.player_id, b.player_id);
    assert!(
        (a.mouse_sensitivity - b.mouse_sensitivity).abs() < EPSILON,
        "mouse_sensitivity: {} vs {}",
        a.mouse_sensitivity,
        b.mouse_sensitivity
    );
    assert_eq!(a.invert_y, b.invert_y);
    assert!(
        (a.view_feel_scale - b.view_feel_scale).abs() < EPSILON,
        "view_feel_scale: {} vs {}",
        a.view_feel_scale,
        b.view_feel_scale
    );
    assert_eq!(a.crouch_mode, b.crouch_mode);
    assert_eq!(a.shadow_quality, b.shadow_quality);
    assert_eq!(a.fog_quality, b.fog_quality);
    assert_eq!(a.surface_depth_quality, b.surface_depth_quality);
    assert_eq!(a.render_resolution, b.render_resolution);
    assert_eq!(a.switch_cycle_dwell_ms, b.switch_cycle_dwell_ms);
    assert!(
        (a.scroll_notch_pixels - b.scroll_notch_pixels).abs() < EPSILON,
        "scroll_notch_pixels: {} vs {}",
        a.scroll_notch_pixels,
        b.scroll_notch_pixels
    );
}

#[test]
fn player_options_roundtrips_through_toml() {
    let original = PlayerOptions {
        player_id: Some([0x7c; 16]),
        mouse_sensitivity: 0.0035,
        invert_y: true,
        view_feel_scale: 0.5,
        crouch_mode: CrouchMode::Toggle,
        shadow_quality: ShadowQuality::Low,
        fog_quality: FogQuality::High,
        surface_depth_quality: SurfaceDepthQuality::Off,
        render_resolution: RenderResolution::Third,
        switch_cycle_dwell_ms: Some(250),
        scroll_notch_pixels: 96.0,
        ..PlayerOptions::default()
    };
    let serialized = to_toml(&original);
    let restored = from_toml(&serialized);
    assert_options_eq(&original, &restored);
}

#[test]
fn load_returns_defaults_when_file_missing() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    assert!(!path.exists());

    let loaded = PlayerOptions::load(&path);
    assert_options_eq(&loaded, &PlayerOptions::default());
    assert_eq!(loaded.player_id, None, "loading does not generate identity");
    // Load must not create the file; the caller owns writing defaults.
    assert!(!path.exists());
}

#[test]
fn save_then_load_yields_equal_options() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");

    let options = PlayerOptions {
        player_id: Some([0x8d; 16]),
        mouse_sensitivity: 0.0042,
        invert_y: true,
        view_feel_scale: 0.25,
        crouch_mode: CrouchMode::Toggle,
        shadow_quality: ShadowQuality::Medium,
        fog_quality: FogQuality::Low,
        surface_depth_quality: SurfaceDepthQuality::On,
        switch_cycle_dwell_ms: Some(400),
        scroll_notch_pixels: 100.0,
        ..PlayerOptions::default()
    };
    options.save(&path).unwrap();

    let loaded = PlayerOptions::load(&path);
    assert_options_eq(&loaded, &options);
}

#[test]
fn load_applies_present_keys_and_defaults_absent_keys() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    // Only `invert_y` present; the other two must fall back to defaults.
    fs::write(&path, "invert_y = true\n").unwrap();

    let loaded = PlayerOptions::load(&path);
    assert!(loaded.invert_y);
    assert!((loaded.mouse_sensitivity - DEFAULT_MOUSE_SENSITIVITY).abs() < EPSILON);
    assert!((loaded.view_feel_scale - 1.0).abs() < EPSILON);
    assert_eq!(loaded.player_id, None, "an absent key stays absent on load");
    // Schema evolution: a settings.toml written before Surface Depth
    // shipped must load with the feature ON, not silently disabled.
    assert_eq!(loaded.surface_depth_quality, SurfaceDepthQuality::On);
}

#[test]
fn surface_depth_quality_persists_as_a_snake_case_state_name() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    for (state, wire) in [
        (SurfaceDepthQuality::Off, "off"),
        (SurfaceDepthQuality::On, "on"),
    ] {
        let options = PlayerOptions {
            surface_depth_quality: state,
            ..PlayerOptions::default()
        };
        options.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains(&format!("surface_depth_quality = \"{wire}\"")),
            "{state:?} must persist as `{wire}`; got:\n{text}"
        );
        assert_eq!(PlayerOptions::load(&path).surface_depth_quality, state);
        assert_eq!(state.slot_value(), wire);
        assert_eq!(SurfaceDepthQuality::from_slot_value(wire), Some(state));
    }
    assert_eq!(SurfaceDepthQuality::from_slot_value("ultra"), None);
    // The retired tier names are accepted only at the TOML file-reading
    // boundary (serde aliases on `On`), never through the slot layer.
    assert_eq!(SurfaceDepthQuality::from_slot_value("high"), None);
    assert_eq!(SurfaceDepthQuality::from_slot_value("low"), None);
}

#[test]
fn the_retired_surface_depth_tiers_load_as_on_and_are_rewritten() {
    // A settings.toml written before D5 collapsed to off/on carries "low"
    // or "high" — "high" in every file that ever saved the default. Both
    // named a state that DID march, so both load as `On`. Letting either
    // fail the parse would discard the rest of the file with it, which is
    // the real cost: this is player data, not a code API.
    for retired in ["low", "high"] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(
            &path,
            format!("invert_y = true\nsurface_depth_quality = \"{retired}\"\n"),
        )
        .unwrap();

        let loaded = PlayerOptions::load(&path);
        assert_eq!(
            loaded.surface_depth_quality,
            SurfaceDepthQuality::On,
            "a saved `{retired}` must load as On, not Off",
        );
        assert!(
            loaded.invert_y,
            "`{retired}` must not take the rest of the file down with it",
        );

        // The next save normalizes the file onto the live vocabulary.
        loaded.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("surface_depth_quality = \"on\""),
            "the retired name must not survive a save; got:\n{text}"
        );
    }
}

#[test]
fn an_out_of_vocabulary_surface_depth_state_falls_back_for_that_field_alone() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    fs::write(
        &path,
        "invert_y = true\nsurface_depth_quality = \"ultra\"\nfog_quality = \"low\"\n",
    )
    .unwrap();

    // A value that never named a real state falls back for its own field;
    // every other setting in the file loads intact.
    let loaded = PlayerOptions::load(&path);
    assert_eq!(loaded.surface_depth_quality, SurfaceDepthQuality::On);
    assert!(loaded.invert_y);
    assert_eq!(loaded.fog_quality, FogQuality::Low);
}

#[test]
fn render_resolution_persists_each_value_as_its_snake_case_name() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    for (value, wire) in [
        (RenderResolution::Auto, "auto"),
        (RenderResolution::Native, "native"),
        (RenderResolution::Half, "half"),
        (RenderResolution::Third, "third"),
        (RenderResolution::Quarter, "quarter"),
    ] {
        let options = PlayerOptions {
            render_resolution: value,
            ..PlayerOptions::default()
        };
        options.save(&path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains(&format!("render_resolution = \"{wire}\"")),
            "{value:?} must persist as `{wire}`; got:\n{text}"
        );
        assert_eq!(PlayerOptions::load(&path).render_resolution, value);
        // The live slot shares the TOML vocabulary.
        assert_eq!(value.slot_value(), wire);
        assert_eq!(RenderResolution::from_slot_value(wire), Some(value));
    }
    assert_eq!(RenderResolution::from_slot_value("eighth"), None);
}

#[test]
fn settings_without_render_resolution_load_as_auto() {
    let loaded = from_toml("invert_y = true\nfog_quality = \"low\"\n");
    assert_eq!(loaded.render_resolution, RenderResolution::Auto);
    assert_eq!(
        PlayerOptions::default().render_resolution,
        RenderResolution::Auto
    );
}

#[test]
fn an_unknown_render_resolution_falls_back_to_auto_alone_and_survives_until_written() {
    let capture = LogCapture::start();
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    fs::write(
        &path,
        "invert_y = true\nrender_resolution = \"eighth\"\nfog_quality = \"low\"\n",
    )
    .unwrap();

    let mut loaded = PlayerOptions::load(&path);
    assert_eq!(loaded.render_resolution, RenderResolution::Auto);
    assert!(loaded.invert_y);
    assert_eq!(loaded.fog_quality, FogQuality::Low);
    capture.assert_logged_once(Level::Warn, "unrecognized value for `render_resolution`");

    // A save of another field keeps the unrecognized text (round-trip).
    loaded.invert_y = false;
    loaded.mark_written(keys::INVERT_Y);
    loaded.save(&path).unwrap();
    let saved: toml::Table = fs::read_to_string(&path).unwrap().parse().unwrap();
    assert_eq!(saved["render_resolution"].as_str(), Some("eighth"));

    // Writing the field replaces it.
    let mut reloaded = PlayerOptions::load(&path);
    reloaded.render_resolution = RenderResolution::Half;
    reloaded.mark_written(keys::RENDER_RESOLUTION);
    reloaded.save(&path).unwrap();
    let saved: toml::Table = fs::read_to_string(&path).unwrap().parse().unwrap();
    assert_eq!(saved["render_resolution"].as_str(), Some("half"));
    assert_eq!(
        PlayerOptions::load(&path).render_resolution,
        RenderResolution::Half
    );
}

#[test]
fn load_returns_defaults_and_preserves_file_when_malformed() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let malformed = "this is not valid toml = = =\n";
    fs::write(&path, malformed).unwrap();

    let loaded = PlayerOptions::load(&path);
    assert_options_eq(&loaded, &PlayerOptions::default());

    // The malformed file must be left untouched for the human to fix.
    let on_disk = fs::read_to_string(&path).unwrap();
    assert_eq!(on_disk, malformed);
}

#[test]
fn save_leaves_no_tmp_artifact_and_writes_parseable_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");

    let options = PlayerOptions {
        player_id: Some([0x9e; 16]),
        mouse_sensitivity: 0.003,
        invert_y: false,
        view_feel_scale: 0.75,
        crouch_mode: CrouchMode::Toggle,
        shadow_quality: ShadowQuality::Low,
        fog_quality: FogQuality::High,
        surface_depth_quality: SurfaceDepthQuality::On,
        switch_cycle_dwell_ms: Some(500),
        scroll_notch_pixels: 80.0,
        ..PlayerOptions::default()
    };
    options.save(&path).unwrap();

    // No `.tmp` sibling should survive a successful save.
    let entries: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !entries.iter().any(|name| name.ends_with(".tmp")),
        "found tmp artifact in {entries:?}"
    );

    let restored = from_toml(&fs::read_to_string(&path).unwrap());
    assert_options_eq(&restored, &options);
}

#[test]
fn load_clamps_view_feel_scale_into_unit_range() {
    let dir = tempdir().unwrap();

    let high = dir.path().join("high.toml");
    fs::write(&high, "view_feel_scale = 2.0\n").unwrap();
    let loaded_high = PlayerOptions::load(&high);
    assert!((loaded_high.view_feel_scale - 1.0).abs() < EPSILON);

    let low = dir.path().join("low.toml");
    fs::write(&low, "view_feel_scale = -1.0\n").unwrap();
    let loaded_low = PlayerOptions::load(&low);
    assert!((loaded_low.view_feel_scale - 0.0).abs() < EPSILON);
}

#[test]
fn crouch_mode_defaults_to_hold() {
    assert_eq!(CrouchMode::default(), CrouchMode::Hold);
    assert_eq!(PlayerOptions::default().crouch_mode, CrouchMode::Hold);
}

#[test]
fn crouch_mode_roundtrips_through_toml() {
    for mode in [CrouchMode::Hold, CrouchMode::Toggle] {
        let original = PlayerOptions {
            crouch_mode: mode,
            ..PlayerOptions::default()
        };
        let serialized = to_toml(&original);
        let restored = from_toml(&serialized);
        assert_eq!(restored.crouch_mode, mode);
    }
}

#[test]
fn crouch_mode_wire_format_is_snake_case() {
    // The TOML value must be the snake_case `"toggle"`, not `"Toggle"`.
    let toggle = PlayerOptions {
        crouch_mode: CrouchMode::Toggle,
        ..PlayerOptions::default()
    };
    let serialized = to_toml(&toggle);
    assert!(
        serialized.contains("crouch_mode = \"toggle\""),
        "expected snake_case crouch_mode key/value, got:\n{serialized}"
    );
}

#[test]
fn load_defaults_crouch_mode_to_hold_when_absent() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    // A file with no `crouch_mode` key must load as the `Hold` default.
    fs::write(&path, "invert_y = true\n").unwrap();

    let loaded = PlayerOptions::load(&path);
    assert_eq!(loaded.crouch_mode, CrouchMode::Hold);
}

#[test]
fn load_applies_explicit_crouch_mode_toggle() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    fs::write(&path, "crouch_mode = \"toggle\"\n").unwrap();

    let loaded = PlayerOptions::load(&path);
    assert_eq!(loaded.crouch_mode, CrouchMode::Toggle);
}

#[test]
fn player_options_graphics_quality_roundtrips_and_defaults() {
    let original = PlayerOptions {
        shadow_quality: ShadowQuality::Low,
        fog_quality: FogQuality::High,
        ..PlayerOptions::default()
    };
    let serialized = to_toml(&original);
    assert!(serialized.contains("shadow_quality = \"low\""));
    assert!(serialized.contains("fog_quality = \"high\""));

    let restored = from_toml(&serialized);
    assert_eq!(restored.shadow_quality, ShadowQuality::Low);
    assert_eq!(restored.fog_quality, FogQuality::High);

    let absent = from_toml("invert_y = true\n");
    assert_eq!(absent.shadow_quality, ShadowQuality::High);
    assert_eq!(absent.fog_quality, FogQuality::Medium);
}

#[test]
fn an_unknown_graphics_quality_falls_back_alone_and_keeps_every_other_setting() {
    let capture = LogCapture::start();
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let text = "invert_y = true\nshadow_quality = \"ultra\"\nfog_quality = \"high\"\n\
                [accessibility]\nflash_limiter = false\nsfx_volume = 0.5\n";
    fs::write(&path, text).unwrap();

    let (loaded, status) = PlayerOptions::load_with_status(&path);

    assert_eq!(status, PlayerOptionsLoadStatus::Loaded);
    assert_eq!(
        loaded.shadow_quality,
        ShadowQuality::High,
        "bad field takes its default"
    );
    assert!(loaded.invert_y);
    assert_eq!(loaded.fog_quality, FogQuality::High);
    assert!(!loaded.accessibility.flash_limiter);
    assert!((loaded.accessibility.sfx_volume - 0.5).abs() < EPSILON);
    capture.assert_logged_once(Level::Warn, "unrecognized value for `shadow_quality`");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        text,
        "loading never writes"
    );
}

#[test]
fn an_unrecognized_accessibility_value_falls_back_alone_with_a_warning() {
    let capture = LogCapture::start();
    let loaded = from_toml(
        "fog_quality = \"low\"\n[accessibility]\nscreen_shake_scale = \"lots\"\n\
         mono_audio = true\nreduce_motion = \"maybe\"\n",
    );
    assert!((loaded.accessibility.screen_shake_scale - 1.0).abs() < EPSILON);
    assert!(loaded.accessibility.mono_audio);
    assert_eq!(
        loaded.accessibility.reduce_motion, None,
        "an unrecognized OS-seedable value resolves as unset"
    );
    assert_eq!(loaded.fog_quality, FogQuality::Low);
    capture.assert_logged_once(
        Level::Warn,
        "unrecognized value for `accessibility.screen_shake_scale`",
    );
    capture.assert_logged_once(
        Level::Warn,
        "unrecognized value for `accessibility.reduce_motion`",
    );
}

#[test]
fn a_file_with_no_accessibility_group_loads_unset_and_defaults() {
    let loaded = from_toml("invert_y = true\n");
    assert_eq!(loaded.accessibility, AccessibilityOptions::default());
    assert_eq!(loaded.accessibility.reduce_motion, None);
    assert!(loaded.accessibility.flash_limiter, "limiter defaults on");
    assert!(!loaded.accessibility_panel_shown);
}

#[test]
fn saving_writes_no_key_for_an_unset_os_seedable_field() {
    let text = to_toml(&PlayerOptions::default());
    assert!(
        !text.contains("reduce_motion"),
        "unset writes no key:\n{text}"
    );
    assert!(
        !text.contains("accessibility_panel_shown"),
        "no first-launch record until the panel closes:\n{text}"
    );
    assert!(text.contains("flash_limiter = true"));

    let set = PlayerOptions {
        accessibility: AccessibilityOptions {
            reduce_motion: Some(false),
            ..AccessibilityOptions::default()
        },
        accessibility_panel_shown: true,
        ..PlayerOptions::default()
    };
    let text = to_toml(&set);
    assert!(text.contains("reduce_motion = false"), "{text}");
    assert!(text.contains("accessibility_panel_shown = true"), "{text}");
    assert_eq!(from_toml(&text).accessibility.reduce_motion, Some(false));
}

#[test]
fn save_keeps_unrecognized_values_and_unknown_keys_until_the_player_writes_them() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    fs::write(
        &path,
        "fog_quality = \"ultra\"\nfuture_option = 7\n[accessibility]\n\
         reduce_motion = \"sometimes\"\nfuture_flag = true\n",
    )
    .unwrap();

    let mut loaded = PlayerOptions::load(&path);
    loaded.invert_y = true;
    loaded.mark_written(keys::INVERT_Y);
    loaded.save(&path).unwrap();

    let saved: toml::Table = fs::read_to_string(&path).unwrap().parse().unwrap();
    assert_eq!(saved["fog_quality"].as_str(), Some("ultra"));
    assert_eq!(saved["future_option"].as_integer(), Some(7));
    assert_eq!(saved["invert_y"].as_bool(), Some(true));
    let group = saved["accessibility"].as_table().unwrap();
    assert_eq!(group["reduce_motion"].as_str(), Some("sometimes"));
    assert_eq!(group["future_flag"].as_bool(), Some(true));

    // A player write to the unrecognized field replaces it.
    let mut reloaded = PlayerOptions::load(&path);
    reloaded.fog_quality = FogQuality::Low;
    reloaded.mark_written(keys::FOG_QUALITY);
    reloaded.accessibility.reduce_motion = Some(true);
    reloaded.mark_written(keys::REDUCE_MOTION);
    reloaded.save(&path).unwrap();
    let saved: toml::Table = fs::read_to_string(&path).unwrap().parse().unwrap();
    assert_eq!(saved["fog_quality"].as_str(), Some("low"));
    assert_eq!(
        saved["accessibility"].as_table().unwrap()["reduce_motion"].as_bool(),
        Some(true)
    );
    assert_eq!(saved["future_option"].as_integer(), Some(7));
}

#[test]
fn a_file_that_is_not_valid_toml_is_never_replaced() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    let malformed = "this is not valid toml = = =\n";
    fs::write(&path, malformed).unwrap();

    let (mut loaded, status) = PlayerOptions::load_with_status(&path);
    assert_eq!(status, PlayerOptionsLoadStatus::Unavailable);
    assert!(!loaded.can_persist());

    loaded.accessibility.flash_limiter = false;
    loaded.accessibility_panel_shown = true;
    loaded.save(&path).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), malformed);
}

#[test]
fn accessibility_scales_and_volumes_clamp_into_unit_range() {
    let loaded = from_toml(
        "[accessibility]\nscreen_shake_scale = 3.0\nmaster_volume = -1.0\nui_volume = 0.25\n",
    );
    assert!((loaded.accessibility.screen_shake_scale - 1.0).abs() < EPSILON);
    assert!((loaded.accessibility.master_volume - 0.0).abs() < EPSILON);
    assert!((loaded.accessibility.ui_volume - 0.25).abs() < EPSILON);
}

#[test]
fn player_id_roundtrips_and_defaults_to_none_when_absent() {
    let original = PlayerOptions {
        player_id: Some([0x3f; 16]),
        ..PlayerOptions::default()
    };
    let serialized = to_toml(&original);
    let restored = from_toml(&serialized);
    assert_eq!(restored.player_id, original.player_id);

    let without_id = from_toml("invert_y = true\n");
    assert_eq!(without_id.player_id, None);
}

#[test]
fn wieldable_input_options_default_when_absent() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    fs::write(&path, "invert_y = true\n").unwrap();

    let loaded = PlayerOptions::load(&path);
    assert_eq!(loaded.switch_cycle_dwell_ms, None);
    assert!((loaded.scroll_notch_pixels - DEFAULT_SCROLL_NOTCH_PIXELS).abs() < EPSILON);
}

#[test]
fn wieldable_input_options_sanitize_to_supported_ranges() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    fs::write(
        &path,
        "switch_cycle_dwell_ms = 999999\nscroll_notch_pixels = -1.0\n",
    )
    .unwrap();

    let loaded = PlayerOptions::load(&path);
    assert_eq!(
        loaded.switch_cycle_dwell_ms,
        Some(MAX_SWITCH_CYCLE_DWELL_MS)
    );
    assert!((loaded.scroll_notch_pixels - DEFAULT_SCROLL_NOTCH_PIXELS).abs() < EPSILON);
}

#[test]
fn a_nan_view_feel_scale_falls_back_to_default() {
    // `clamp` leaves NaN untouched (it fails every `<`/`>` comparison), so
    // a hand-edited `nan` — a valid TOML float literal — used to sail past
    // `sanitize` and land in `ResolvedAccessibility`, where `PartialEq`
    // treats NaN as never equal to itself: the options bridge would then
    // never see two equal snapshots and re-apply every frame.
    let capture = LogCapture::start();
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    fs::write(&path, "view_feel_scale = nan\ninvert_y = true\n").unwrap();

    let loaded = PlayerOptions::load(&path);
    assert!((loaded.view_feel_scale - 1.0).abs() < EPSILON);
    assert!(
        loaded.invert_y,
        "a non-finite field must not take the rest of the file down with it"
    );
    capture.assert_logged_once(Level::Warn, "`view_feel_scale` is not a finite number");
}

#[test]
fn every_non_finite_f32_field_falls_back_to_default_with_a_warning() {
    let capture = LogCapture::start();
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    fs::write(
        &path,
        "mouse_sensitivity = inf\nscroll_notch_pixels = -nan\ninvert_y = true\n",
    )
    .unwrap();

    let loaded = PlayerOptions::load(&path);
    assert!((loaded.mouse_sensitivity - DEFAULT_MOUSE_SENSITIVITY).abs() < EPSILON);
    assert!((loaded.scroll_notch_pixels - DEFAULT_SCROLL_NOTCH_PIXELS).abs() < EPSILON);
    assert!(loaded.invert_y);
    capture.assert_logged_once(Level::Warn, "`mouse_sensitivity` is not a finite number");
    capture.assert_logged_once(Level::Warn, "`scroll_notch_pixels` is not a finite number");
}

#[test]
fn default_options_save_mouse_sensitivity_at_full_f32_precision() {
    let text = to_toml(&PlayerOptions::default());
    assert!(
        text.contains("mouse_sensitivity = 0.002\n"),
        "Value::try_from(f32) widens through f64 and would write \
         0.0020000000949949026 instead; got:\n{text}"
    );
}

#[test]
fn a_stepped_f32_value_saves_at_full_precision_not_f64_widened_noise() {
    // Regression for a panel step (e.g. a slider settling on 0.85): the
    // generic `Value::try_from` path serializes f32 through
    // `serialize_f64(value as f64)`, so 0.85 would round-trip to
    // 0.8500000238418579 and get rewritten on every subsequent save.
    let stepped = PlayerOptions {
        view_feel_scale: 0.85,
        ..PlayerOptions::default()
    };
    let text = to_toml(&stepped);
    assert!(text.contains("view_feel_scale = 0.85\n"), "got:\n{text}");
}

// --- Game-scoped binding rows ---

fn rows(options: &PlayerOptions, mod_id: &str) -> Vec<(String, String, Vec<Option<String>>)> {
    options
        .game_binding_rows(mod_id)
        .into_iter()
        .map(|row| (row.class_key, row.command_id, row.inputs))
        .collect()
}

#[test]
fn an_empty_binding_row_stays_unbound_across_save_and_load() {
    let mut options = PlayerOptions::default();
    options.set_game_binding_row("acme.neon", "gamepad", "dash", Some(Vec::new()));
    let reloaded = from_toml(&to_toml(&options));
    assert_eq!(
        rows(&reloaded, "acme.neon"),
        vec![("gamepad".into(), "dash".into(), Vec::new())]
    );
}

#[test]
fn an_unknown_command_row_survives_a_save_that_rewrites_another_row() {
    let mut options = from_toml(
        "[game.\"acme.neon\".bindings.keyboard_mouse]\n\
         grapple = [\"KeyG\"]\n\
         dash = [\"KeyF\"]\n",
    );
    options.set_game_binding_row(
        "acme.neon",
        "keyboard_mouse",
        "dash",
        Some(vec!["KeyV".to_string()]),
    );
    let text = to_toml(&options);
    let reloaded = from_toml(&text);
    let saved = rows(&reloaded, "acme.neon");
    assert!(saved.contains(&(
        "keyboard_mouse".into(),
        "grapple".into(),
        vec![Some("KeyG".into())]
    )));
    assert!(saved.contains(&(
        "keyboard_mouse".into(),
        "dash".into(),
        vec![Some("KeyV".into())]
    )));
}

#[test]
fn a_reset_removes_only_that_row() {
    let mut options = from_toml(
        "[game.\"acme.neon\".bindings.keyboard_mouse]\n\
         dash = [\"KeyV\"]\n\
         jump = [\"KeyJ\"]\n",
    );
    options.set_game_binding_row("acme.neon", "keyboard_mouse", "dash", None);
    let reloaded = from_toml(&to_toml(&options));
    assert_eq!(
        rows(&reloaded, "acme.neon"),
        vec![(
            "keyboard_mouse".into(),
            "jump".into(),
            vec![Some("KeyJ".into())]
        )]
    );
}

#[test]
fn two_mod_ids_keep_separate_binding_rows() {
    let mut options = PlayerOptions::default();
    options.set_game_binding_row("mod.a", "keyboard_mouse", "dash", Some(vec!["KeyQ".into()]));
    options.set_game_binding_row("mod.b", "keyboard_mouse", "dash", Some(vec!["KeyE".into()]));
    let reloaded = from_toml(&to_toml(&options));
    assert_eq!(
        rows(&reloaded, "mod.a"),
        vec![(
            "keyboard_mouse".into(),
            "dash".into(),
            vec![Some("KeyQ".into())]
        )]
    );
    assert_eq!(
        rows(&reloaded, "mod.b"),
        vec![(
            "keyboard_mouse".into(),
            "dash".into(),
            vec![Some("KeyE".into())]
        )]
    );
    assert!(rows(&reloaded, "mod.c").is_empty());
}

#[test]
fn a_malformed_binding_row_falls_back_alone_and_stays_in_the_file() {
    let capture = LogCapture::start();
    let options = from_toml(
        "invert_y = true\n\
         [game.\"acme.neon\".bindings.keyboard_mouse]\n\
         dash = \"KeyF\"\n\
         jump = [3, \"KeyJ\"]\n",
    );
    assert!(options.invert_y, "every other setting loads");
    assert_eq!(
        rows(&options, "acme.neon"),
        vec![(
            "keyboard_mouse".into(),
            "jump".into(),
            vec![None, Some("KeyJ".into())]
        )]
    );
    capture.assert_logged(
        Level::Warn,
        "binding row `dash` for `acme.neon` is not a list",
    );
    assert!(to_toml(&options).contains("dash = \"KeyF\""));
}

// --- E23 U3: gamepad look, sprint mode, swap, hold timing ---

#[test]
fn the_new_top_level_options_round_trip_as_top_level_keys() {
    let options = PlayerOptions {
        sprint_mode: SprintMode::Toggle,
        gamepad_look_sensitivity: 3.5,
        gamepad_look_dead_zone: 0.25,
        gamepad_invert_y: true,
        swap_confirm_cancel: true,
        ..PlayerOptions::default()
    };
    let text = to_toml(&options);
    for line in [
        "sprint_mode = \"toggle\"",
        "gamepad_look_sensitivity = 3.5",
        "gamepad_look_dead_zone = 0.25",
        "gamepad_invert_y = true",
        "swap_confirm_cancel = true",
    ] {
        assert!(text.contains(line), "{line} missing:\n{text}");
    }
    assert!(!text.contains("accessibility.sprint"));
    let reloaded = from_toml(&text);
    assert_eq!(reloaded.sprint_mode, SprintMode::Toggle);
    assert_eq!(reloaded.gamepad_look_sensitivity, 3.5);
    assert_eq!(reloaded.gamepad_look_dead_zone, 0.25);
    assert!(reloaded.gamepad_invert_y);
    assert!(reloaded.swap_confirm_cancel);
}

#[test]
fn an_unrecognized_sprint_mode_falls_back_to_hold_alone() {
    let capture = LogCapture::start();
    let options =
        from_toml("sprint_mode = \"sometimes\"\ninvert_y = true\ngamepad_invert_y = true\n");
    assert_eq!(options.sprint_mode, SprintMode::Hold);
    assert!(options.invert_y && options.gamepad_invert_y);
    capture.assert_logged_once(Level::Warn, "unrecognized value for `sprint_mode`");
}

#[test]
fn hold_timing_scale_lives_in_the_group_clamps_to_one_to_three_and_falls_back_alone() {
    let options = from_toml("[accessibility]\nhold_timing_scale = 0.5\n");
    assert_eq!(
        options.accessibility.hold_timing_scale, 1.0,
        "never shortens"
    );
    let options = from_toml("[accessibility]\nhold_timing_scale = 9.0\n");
    assert_eq!(options.accessibility.hold_timing_scale, 3.0);
    let capture = LogCapture::start();
    let options = from_toml("[accessibility]\nhold_timing_scale = \"long\"\nui_volume = 0.5\n");
    assert_eq!(options.accessibility.hold_timing_scale, 1.0);
    assert_eq!(options.accessibility.ui_volume, 0.5);
    capture.assert_logged_once(Level::Warn, "accessibility.hold_timing_scale");
    let text = to_toml(&PlayerOptions {
        accessibility: AccessibilityOptions {
            hold_timing_scale: 2.0,
            ..AccessibilityOptions::default()
        },
        ..PlayerOptions::default()
    });
    assert!(
        text.contains("[accessibility]") && text.contains("hold_timing_scale = 2"),
        "{text}"
    );
}

#[test]
fn gamepad_look_values_clamp_into_their_ranges() {
    let options = from_toml("gamepad_look_sensitivity = 40.0\ngamepad_look_dead_zone = -1.0\n");
    assert_eq!(options.gamepad_look_sensitivity, 8.0);
    assert_eq!(options.gamepad_look_dead_zone, 0.0);
}

#[test]
fn only_hold_timing_scale_of_the_new_fields_has_an_accessibility_slot_and_panel_entry() {
    // Catalog assertion: the new top-level fields are working copies only.
    let catalog = postretro_entities::engine_state_catalog::engine_state_catalog().unwrap();
    let names: Vec<&str> = catalog.entries().iter().map(|e| e.wire_name).collect();
    for field in [
        "gamepadLookSensitivity",
        "gamepadLookDeadZone",
        "gamepadInvertY",
        "sprintMode",
        "swapConfirmCancel",
    ] {
        assert!(
            names.contains(&format!("options.{field}").as_str()),
            "{field}"
        );
        assert!(
            !names.contains(&format!("accessibility.{field}").as_str()),
            "{field}"
        );
        assert!(!panel_actions::panel_field_names().any(|name| name == field));
    }
    assert!(names.contains(&"accessibility.holdTimingScale"));
    assert!(names.contains(&"options.holdTimingScale"));
    assert!(panel_actions::panel_field_names().any(|name| name == "holdTimingScale"));
}
