use super::*;
use crate::options::{CrouchMode, ShadowQuality};
use postretro_entities::ScriptCtx;
use postretro_scripting_core::store_bridge::write_state_slot_json;
use serde_json::json;
use tempfile::tempdir;

const EPSILON: f32 = 1e-6;

fn input() -> InputSystem {
    InputSystem::new(crate::input::default_bindings())
}

fn write(ctx: &ScriptCtx, slot: &str, value: serde_json::Value) {
    write_state_slot_json(ctx, slot, &value).expect("option-slot write");
}

#[test]
fn opening_options_seeds_every_slot_from_player_options() {
    let mut table = SlotTable::new();
    let options = PlayerOptions {
        mouse_sensitivity: 0.004,
        invert_y: true,
        view_feel_scale: 0.25,
        crouch_mode: CrouchMode::Toggle,
        shadow_quality: ShadowQuality::Low,
        fog_quality: FogQuality::High,
        surface_depth_quality: SurfaceDepthQuality::Off,
        render_resolution: RenderResolution::Quarter,
        ..PlayerOptions::default()
    };
    let mut bridge = OptionsBridge::new();

    bridge.seed_on_open(&mut table, &options);

    assert_eq!(
        table.get(MOUSE_SENSITIVITY_SLOT).unwrap().value,
        Some(SlotValue::Number(0.004))
    );
    assert_eq!(
        table.get(INVERT_Y_SLOT).unwrap().value,
        Some(SlotValue::Boolean(true))
    );
    assert_eq!(
        table.get(VIEW_FEEL_SCALE_SLOT).unwrap().value,
        Some(SlotValue::Number(0.25))
    );
    assert_eq!(
        table.get(CROUCH_MODE_SLOT).unwrap().value,
        Some(SlotValue::Enum("toggle".into()))
    );
    assert_eq!(
        table.get(SHADOW_QUALITY_SLOT).unwrap().value,
        Some(SlotValue::Enum("low".into()))
    );
    assert_eq!(
        table.get(FOG_QUALITY_SLOT).unwrap().value,
        Some(SlotValue::Enum("high".into()))
    );
    assert_eq!(
        table.get(SURFACE_DEPTH_QUALITY_SLOT).unwrap().value,
        Some(SlotValue::Enum("off".into()))
    );
    assert_eq!(
        table.get(RENDER_RESOLUTION_SLOT).unwrap().value,
        Some(SlotValue::Enum("quarter".into()))
    );
}

#[test]
fn engine_option_slot_defaults_match_player_options() {
    let table = SlotTable::new();
    let options = PlayerOptions::default();

    assert_eq!(
        table.get(MOUSE_SENSITIVITY_SLOT).unwrap().value,
        Some(SlotValue::Number(options.mouse_sensitivity))
    );
    assert_eq!(
        table.get(INVERT_Y_SLOT).unwrap().value,
        Some(SlotValue::Boolean(options.invert_y))
    );
    assert_eq!(
        table.get(VIEW_FEEL_SCALE_SLOT).unwrap().value,
        Some(SlotValue::Number(options.view_feel_scale))
    );
    assert_eq!(
        table.get(CROUCH_MODE_SLOT).unwrap().value,
        Some(SlotValue::Enum(options.crouch_mode.slot_value().into()))
    );
    assert_eq!(
        table.get(SHADOW_QUALITY_SLOT).unwrap().value,
        Some(SlotValue::Enum(options.shadow_quality.slot_value().into()))
    );
    assert_eq!(
        table.get(FOG_QUALITY_SLOT).unwrap().value,
        Some(SlotValue::Enum(options.fog_quality.slot_value().into()))
    );
    assert_eq!(
        table.get(SURFACE_DEPTH_QUALITY_SLOT).unwrap().value,
        Some(SlotValue::Enum(
            options.surface_depth_quality.slot_value().into()
        )),
        "the slot default must agree with PlayerOptions' default (High)",
    );
    assert_eq!(
        table.get(RENDER_RESOLUTION_SLOT).unwrap().value,
        Some(SlotValue::Enum(
            options.render_resolution.slot_value().into()
        )),
        "the slot default must agree with PlayerOptions' default (Auto)",
    );
}

#[test]
fn write_state_slot_json_validates_every_option_slot() {
    let ctx = ScriptCtx::new();

    write(&ctx, MOUSE_SENSITIVITY_SLOT, json!(0.0));
    write(&ctx, INVERT_Y_SLOT, json!(true));
    write(&ctx, VIEW_FEEL_SCALE_SLOT, json!(2.0));
    write(&ctx, CROUCH_MODE_SLOT, json!("toggle"));
    write(&ctx, SHADOW_QUALITY_SLOT, json!("low"));
    write(&ctx, FOG_QUALITY_SLOT, json!("high"));
    write(&ctx, SURFACE_DEPTH_QUALITY_SLOT, json!("off"));
    write(&ctx, RENDER_RESOLUTION_SLOT, json!("third"));

    let table = ctx.slot_table.borrow();
    assert!(
        matches!(table.get(MOUSE_SENSITIVITY_SLOT).unwrap().value.as_ref(), Some(SlotValue::Number(value)) if *value > 0.0)
    );
    assert_eq!(
        table.get(INVERT_Y_SLOT).unwrap().value,
        Some(SlotValue::Boolean(true))
    );
    assert_eq!(
        table.get(VIEW_FEEL_SCALE_SLOT).unwrap().value,
        Some(SlotValue::Number(1.0))
    );
    assert_eq!(
        table.get(CROUCH_MODE_SLOT).unwrap().value,
        Some(SlotValue::Enum("toggle".into()))
    );
    assert_eq!(
        table.get(SHADOW_QUALITY_SLOT).unwrap().value,
        Some(SlotValue::Enum("low".into()))
    );
    assert_eq!(
        table.get(FOG_QUALITY_SLOT).unwrap().value,
        Some(SlotValue::Enum("high".into()))
    );
    assert_eq!(
        table.get(SURFACE_DEPTH_QUALITY_SLOT).unwrap().value,
        Some(SlotValue::Enum("off".into()))
    );
    assert_eq!(
        table.get(RENDER_RESOLUTION_SLOT).unwrap().value,
        Some(SlotValue::Enum("third".into()))
    );
    drop(table);

    assert!(write_state_slot_json(&ctx, CROUCH_MODE_SLOT, &json!("invalid")).is_err());
    assert!(write_state_slot_json(&ctx, SHADOW_QUALITY_SLOT, &json!("ultra")).is_err());
    assert!(write_state_slot_json(&ctx, FOG_QUALITY_SLOT, &json!("ultra")).is_err());
    // The slot vocabulary is strictly off/on. The retired `low`/`high`
    // names are accepted only when reading a persisted TOML file (serde
    // aliases on `SurfaceDepthQuality::On`), never through the slot layer
    // — so both must still be rejected here, alongside a genuinely bogus
    // value.
    assert!(write_state_slot_json(&ctx, SURFACE_DEPTH_QUALITY_SLOT, &json!("high")).is_err());
    assert!(write_state_slot_json(&ctx, SURFACE_DEPTH_QUALITY_SLOT, &json!("low")).is_err());
    assert!(write_state_slot_json(&ctx, SURFACE_DEPTH_QUALITY_SLOT, &json!("medium")).is_err());
    assert!(write_state_slot_json(&ctx, RENDER_RESOLUTION_SLOT, &json!("eighth")).is_err());
    assert!(write_state_slot_json(&ctx, RENDER_RESOLUTION_SLOT, &json!(2)).is_err());
    assert!(write_state_slot_json(&ctx, INVERT_Y_SLOT, &json!(1)).is_err());
    assert!(write_state_slot_json(&ctx, VIEW_FEEL_SCALE_SLOT, &json!(true)).is_err());
    assert!(write_state_slot_json(&ctx, "options.unknown", &json!(true)).is_err());
}

#[test]
fn one_slot_change_updates_only_matching_field_and_applies_input() {
    let ctx = ScriptCtx::new();
    let before = PlayerOptions::default();
    let mut options = before.clone();
    let mut input = input();
    let mut bridge = OptionsBridge::new();
    bridge.seed_on_open(&mut ctx.slot_table.borrow_mut(), &options);

    write(&ctx, MOUSE_SENSITIVITY_SLOT, json!(0.006));
    let effects = bridge.update(
        0.0,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        None,
    );

    assert!((options.mouse_sensitivity - 0.006).abs() < EPSILON);
    assert_eq!(options.invert_y, before.invert_y);
    assert!((options.view_feel_scale - before.view_feel_scale).abs() < EPSILON);
    assert_eq!(options.crouch_mode, before.crouch_mode);
    assert_eq!(options.shadow_quality, before.shadow_quality);
    assert_eq!(options.fog_quality, before.fog_quality);
    assert!((input.mouse_sensitivity() - 0.006).abs() < EPSILON);
    assert_eq!(effects.mouse_sensitivity, Some(0.006));
    assert_eq!(effects.invert_y, None);
    assert_eq!(effects.fog_quality, None);
    assert_eq!(options.surface_depth_quality, before.surface_depth_quality);
    assert_eq!(effects.surface_depth_quality, None);
    assert_eq!(options.render_resolution, before.render_resolution);
    assert_eq!(effects.render_resolution, None);
}

#[test]
fn every_remaining_option_slot_updates_its_matching_field() {
    let ctx = ScriptCtx::new();
    let mut options = PlayerOptions::default();
    let mut input = input();
    let mut bridge = OptionsBridge::new();
    bridge.seed_on_open(&mut ctx.slot_table.borrow_mut(), &options);

    write(&ctx, INVERT_Y_SLOT, json!(true));
    let effects = bridge.update(
        0.0,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        None,
    );
    assert!(options.invert_y);
    assert!(input.invert_y());
    assert_eq!(effects.invert_y, Some(true));
    assert_eq!(options.view_feel_scale, 1.0);
    assert_eq!(options.crouch_mode, CrouchMode::Hold);
    assert_eq!(options.shadow_quality, ShadowQuality::High);
    assert_eq!(options.fog_quality, FogQuality::Medium);

    write(&ctx, VIEW_FEEL_SCALE_SLOT, json!(0.4));
    bridge.update(
        0.0,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        None,
    );
    assert!((options.view_feel_scale - 0.4).abs() < EPSILON);
    assert_eq!(options.crouch_mode, CrouchMode::Hold);
    assert_eq!(options.shadow_quality, ShadowQuality::High);
    assert_eq!(options.fog_quality, FogQuality::Medium);

    write(&ctx, CROUCH_MODE_SLOT, json!("toggle"));
    bridge.update(
        0.0,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        None,
    );
    assert_eq!(options.crouch_mode, CrouchMode::Toggle);
    assert_eq!(options.shadow_quality, ShadowQuality::High);
    assert_eq!(options.fog_quality, FogQuality::Medium);

    write(&ctx, SHADOW_QUALITY_SLOT, json!("low"));
    let effects = bridge.update(
        0.0,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        None,
    );
    assert_eq!(options.shadow_quality, ShadowQuality::Low);
    assert_eq!(options.fog_quality, FogQuality::Medium);
    assert_eq!(effects.fog_quality, None, "shadow has no live effect");

    write(&ctx, FOG_QUALITY_SLOT, json!("high"));
    let effects = bridge.update(
        0.0,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        None,
    );
    assert_eq!(options.fog_quality, FogQuality::High);
    assert_eq!(effects.fog_quality, Some(FogQuality::High));
    assert_eq!(options.shadow_quality, ShadowQuality::Low);
    assert_eq!(
        options.surface_depth_quality,
        SurfaceDepthQuality::On,
        "untouched slots keep their value",
    );

    write(&ctx, SURFACE_DEPTH_QUALITY_SLOT, json!("off"));
    let effects = bridge.update(
        0.0,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        None,
    );
    assert_eq!(options.surface_depth_quality, SurfaceDepthQuality::Off);
    assert_eq!(
        effects.surface_depth_quality,
        Some(SurfaceDepthQuality::Off),
        "the state must be reported so the app can apply it live",
    );
    assert_eq!(effects.fog_quality, None);
    assert_eq!(options.fog_quality, FogQuality::High);

    // Turning it back on reports too: the live path is two-way.
    write(&ctx, SURFACE_DEPTH_QUALITY_SLOT, json!("on"));
    let effects = bridge.update(
        0.0,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        None,
    );
    assert_eq!(options.surface_depth_quality, SurfaceDepthQuality::On);
    assert_eq!(effects.surface_depth_quality, Some(SurfaceDepthQuality::On));
    assert_eq!(effects.render_resolution, None);
    assert_eq!(options.render_resolution, RenderResolution::Auto);

    write(&ctx, RENDER_RESOLUTION_SLOT, json!("half"));
    let effects = bridge.update(
        0.0,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        None,
    );
    assert_eq!(options.render_resolution, RenderResolution::Half);
    assert_eq!(
        effects.render_resolution,
        Some(RenderResolution::Half),
        "the value must be reported so the app can apply it live",
    );
    assert_eq!(effects.surface_depth_quality, None);
    assert_eq!(options.surface_depth_quality, SurfaceDepthQuality::On);

    // Rewriting the value it already holds marks the field written but
    // reports no live effect.
    write(&ctx, RENDER_RESOLUTION_SLOT, json!("half"));
    let effects = bridge.update(
        0.0,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        None,
    );
    assert_eq!(effects.render_resolution, None);
}

#[test]
fn slider_changes_debounce_to_one_last_value_save() {
    let ctx = ScriptCtx::new();
    let path = Path::new("settings.toml");
    let mut options = PlayerOptions::default();
    let mut input = input();
    let mut bridge = OptionsBridge::new();
    bridge.seed_on_open(&mut ctx.slot_table.borrow_mut(), &options);
    let mut saved = Vec::new();

    for value in [0.003, 0.004, 0.005] {
        write(&ctx, MOUSE_SENSITIVITY_SLOT, json!(value));
        bridge.update_with_save(
            0.016,
            &mut ctx.slot_table.borrow_mut(),
            &mut options,
            &mut input,
            Some(path),
            |options, _| {
                saved.push(options.mouse_sensitivity);
                Ok(())
            },
        );
    }
    bridge.update_with_save(
        0.249,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        Some(path),
        |options, _| {
            saved.push(options.mouse_sensitivity);
            Ok(())
        },
    );
    assert!(saved.is_empty());
    bridge.update_with_save(
        0.002,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        Some(path),
        |options, _| {
            saved.push(options.mouse_sensitivity);
            Ok(())
        },
    );

    assert_eq!(saved, vec![0.005]);
}

#[test]
fn closing_and_exiting_flush_pending_saves() {
    let ctx = ScriptCtx::new();
    let path = Path::new("settings.toml");
    let mut options = PlayerOptions::default();
    let mut input = input();
    let mut bridge = OptionsBridge::new();
    bridge.seed_on_open(&mut ctx.slot_table.borrow_mut(), &options);
    write(&ctx, INVERT_Y_SLOT, json!(true));
    bridge.update_with_save(
        0.0,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        Some(path),
        |_, _| Ok(()),
    );

    let mut close_saves = 0;
    bridge.flush_with_save(&options, Some(path), |_, _| {
        close_saves += 1;
        Ok(())
    });
    assert_eq!(close_saves, 1);

    write(&ctx, INVERT_Y_SLOT, json!(false));
    bridge.update_with_save(
        0.0,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        Some(path),
        |_, _| Ok(()),
    );
    let mut exit_saves = 0;
    bridge.flush_with_save(&options, Some(path), |_, _| {
        exit_saves += 1;
        Ok(())
    });
    assert_eq!(exit_saves, 1);
}

#[test]
fn reopening_options_seeds_unsaved_in_memory_value() {
    let ctx = ScriptCtx::new();
    let mut options = PlayerOptions::default();
    let mut input = input();
    let mut bridge = OptionsBridge::new();
    bridge.seed_on_open(&mut ctx.slot_table.borrow_mut(), &options);
    write(&ctx, FOG_QUALITY_SLOT, json!("high"));
    bridge.update(
        0.0,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        None,
    );

    ctx.slot_table
        .borrow_mut()
        .get_mut(FOG_QUALITY_SLOT)
        .unwrap()
        .write_value(Some(SlotValue::Enum("low".into())));
    bridge.seed_on_open(&mut ctx.slot_table.borrow_mut(), &options);

    assert_eq!(options.fog_quality, FogQuality::High);
    assert_eq!(
        ctx.slot_table.borrow().get(FOG_QUALITY_SLOT).unwrap().value,
        Some(SlotValue::Enum("high".into()))
    );
}

#[test]
fn failed_save_keeps_applied_value_and_later_change_retries() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.toml");
    std::fs::write(&path, "original settings").unwrap();
    let ctx = ScriptCtx::new();
    let mut options = PlayerOptions::default();
    let mut input = input();
    let mut bridge = OptionsBridge::new();
    bridge.seed_on_open(&mut ctx.slot_table.borrow_mut(), &options);

    write(&ctx, INVERT_Y_SLOT, json!(true));
    bridge.update_with_save(
        0.0,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        Some(&path),
        |_, _| Ok(()),
    );
    let mut failed_attempts = 0;
    bridge.update_with_save(
        0.251,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        Some(&path),
        |_, _| {
            failed_attempts += 1;
            Err(io::Error::other("injected failure"))
        },
    );
    assert_eq!(failed_attempts, 1);
    assert!(options.invert_y);
    assert!(input.invert_y());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "original settings");

    write(&ctx, VIEW_FEEL_SCALE_SLOT, json!(0.5));
    bridge.update_with_save(
        0.0,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        Some(&path),
        |_, _| Ok(()),
    );
    bridge.update_with_save(
        0.251,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        Some(&path),
        PlayerOptions::save,
    );

    let reloaded = PlayerOptions::load(&path);
    assert!(reloaded.invert_y);
    assert!((reloaded.view_feel_scale - 0.5).abs() < EPSILON);
}

#[test]
fn the_gamepad_sprint_and_swap_slots_update_their_fields_and_apply_live() {
    use super::top_level::{
        GAMEPAD_INVERT_Y_SLOT, GAMEPAD_LOOK_DEAD_ZONE_SLOT, GAMEPAD_LOOK_SENSITIVITY_SLOT,
        SPRINT_MODE_SLOT, SWAP_CONFIRM_CANCEL_SLOT,
    };
    let ctx = ScriptCtx::new();
    let mut options = PlayerOptions::default();
    let mut input = input();
    let mut bridge = OptionsBridge::new();
    bridge.seed_on_open(&mut ctx.slot_table.borrow_mut(), &options);

    write(&ctx, GAMEPAD_LOOK_SENSITIVITY_SLOT, json!(4.0));
    write(&ctx, GAMEPAD_LOOK_DEAD_ZONE_SLOT, json!(0.3));
    write(&ctx, GAMEPAD_INVERT_Y_SLOT, json!(true));
    write(&ctx, SPRINT_MODE_SLOT, json!("toggle"));
    write(&ctx, SWAP_CONFIRM_CANCEL_SLOT, json!(true));
    let effects = bridge.update(
        0.0,
        &mut ctx.slot_table.borrow_mut(),
        &mut options,
        &mut input,
        None,
    );

    assert_eq!(options.gamepad_look_sensitivity, 4.0);
    assert_eq!(options.gamepad_look_dead_zone, 0.3);
    assert!(options.gamepad_invert_y);
    assert_eq!(options.sprint_mode, crate::options::SprintMode::Toggle);
    assert!(options.swap_confirm_cancel);
    assert_eq!(effects.swap_confirm_cancel, Some(true));
    // Live input effects: gamepad look only; mouse look is untouched.
    assert_eq!(input.drain_look_inputs().gamepad_sensitivity, 4.0);
    assert!((input.mouse_sensitivity() - crate::input::DEFAULT_MOUSE_SENSITIVITY).abs() < EPSILON);
    assert!(!input.invert_y());
    let right = (gilrs::Axis::RightStickX, gilrs::Axis::RightStickY);
    let left = (gilrs::Axis::LeftStickX, gilrs::Axis::LeftStickY);
    assert_eq!(input.stick_dead_zone(right.0, right.1), 0.3);
    assert_eq!(
        input.stick_dead_zone(left.0, left.1),
        0.15,
        "the move stick keeps its own"
    );
}
