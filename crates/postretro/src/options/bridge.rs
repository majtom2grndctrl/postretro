// App-side bridge from writable option slots to session-owned player settings.
// See: context/lib/player_options.md §4

use std::io;
use std::path::Path;

use postretro_entities::slot_table::{SlotTable, SlotValue};

use super::{CrouchMode, FogQuality, PlayerOptions, ShadowQuality, SurfaceDepthQuality};
use crate::input::InputSystem;

pub(crate) const MOUSE_SENSITIVITY_SLOT: &str = "options.mouseSensitivity";
pub(crate) const INVERT_Y_SLOT: &str = "options.invertY";
pub(crate) const VIEW_FEEL_SCALE_SLOT: &str = "options.viewFeelScale";
pub(crate) const CROUCH_MODE_SLOT: &str = "options.crouchMode";
pub(crate) const SHADOW_QUALITY_SLOT: &str = "options.shadowQuality";
pub(crate) const FOG_QUALITY_SLOT: &str = "options.fogQuality";
pub(crate) const SURFACE_DEPTH_QUALITY_SLOT: &str = "options.surfaceDepthQuality";

const SAVE_DEBOUNCE_SECONDS: f32 = 0.250;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ObservedGenerations {
    mouse_sensitivity: u64,
    invert_y: u64,
    view_feel_scale: u64,
    crouch_mode: u64,
    shadow_quality: u64,
    fog_quality: u64,
    surface_depth_quality: u64,
}

/// Live subsystem effects produced by accepted option-slot changes.
///
/// Input effects are applied by the bridge before this report is returned.
/// Fog and Surface Depth stay typed until the app-side render-profile
/// chokepoint translates the tier into renderer parameters. Shadow
/// intentionally has no live effect.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct OptionsApplyEffects {
    pub(crate) mouse_sensitivity: Option<f32>,
    pub(crate) invert_y: Option<bool>,
    pub(crate) fog_quality: Option<FogQuality>,
    pub(crate) surface_depth_quality: Option<SurfaceDepthQuality>,
}

/// Deterministic, session-lifetime synchronization state for `options.*`.
#[derive(Default)]
pub(crate) struct OptionsBridge {
    observed: ObservedGenerations,
    save_remaining_seconds: Option<f32>,
}

impl OptionsBridge {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Seed the complete UI working copy from the current in-memory settings.
    /// The observed generations advance with these engine writes so reopening a
    /// menu never feeds the seed back through apply/save as a user change.
    pub(crate) fn seed_on_open(&mut self, table: &mut SlotTable, options: &PlayerOptions) {
        self.observed.mouse_sensitivity = seed_slot(
            table,
            MOUSE_SENSITIVITY_SLOT,
            SlotValue::Number(options.mouse_sensitivity),
        );
        self.observed.invert_y =
            seed_slot(table, INVERT_Y_SLOT, SlotValue::Boolean(options.invert_y));
        self.observed.view_feel_scale = seed_slot(
            table,
            VIEW_FEEL_SCALE_SLOT,
            SlotValue::Number(options.view_feel_scale),
        );
        self.observed.crouch_mode = seed_slot(
            table,
            CROUCH_MODE_SLOT,
            SlotValue::Enum(options.crouch_mode.slot_value().to_string()),
        );
        self.observed.shadow_quality = seed_slot(
            table,
            SHADOW_QUALITY_SLOT,
            SlotValue::Enum(options.shadow_quality.slot_value().to_string()),
        );
        self.observed.fog_quality = seed_slot(
            table,
            FOG_QUALITY_SLOT,
            SlotValue::Enum(options.fog_quality.slot_value().to_string()),
        );
        self.observed.surface_depth_quality = seed_slot(
            table,
            SURFACE_DEPTH_QUALITY_SLOT,
            SlotValue::Enum(options.surface_depth_quality.slot_value().to_string()),
        );
    }

    /// Observe writes once per app frame, apply live input changes immediately,
    /// and settle disk persistence after 250 ms of no further accepted change.
    pub(crate) fn update(
        &mut self,
        frame_dt_seconds: f32,
        table: &SlotTable,
        options: &mut PlayerOptions,
        input: &mut InputSystem,
        settings_path: Option<&Path>,
    ) -> OptionsApplyEffects {
        self.update_with_save(
            frame_dt_seconds,
            table,
            options,
            input,
            settings_path,
            PlayerOptions::save,
        )
    }

    /// Flush a pending debounced write when the options modal closes.
    pub(crate) fn flush_on_options_close(
        &mut self,
        options: &PlayerOptions,
        settings_path: Option<&Path>,
    ) {
        self.flush_with_save(options, settings_path, PlayerOptions::save);
    }

    /// Flush a pending debounced write during normal application teardown.
    pub(crate) fn flush_on_clean_exit(
        &mut self,
        options: &PlayerOptions,
        settings_path: Option<&Path>,
    ) {
        self.flush_with_save(options, settings_path, PlayerOptions::save);
    }

    fn update_with_save<F>(
        &mut self,
        frame_dt_seconds: f32,
        table: &SlotTable,
        options: &mut PlayerOptions,
        input: &mut InputSystem,
        settings_path: Option<&Path>,
        mut save: F,
    ) -> OptionsApplyEffects
    where
        F: FnMut(&PlayerOptions, &Path) -> io::Result<()>,
    {
        let mut effects = OptionsApplyEffects::default();
        let changed = self.observe_changes(table, options, input, &mut effects);

        if changed {
            self.save_remaining_seconds = settings_path.map(|_| SAVE_DEBOUNCE_SECONDS);
        } else if let Some(remaining) = self.save_remaining_seconds.as_mut() {
            let elapsed = if frame_dt_seconds.is_finite() {
                frame_dt_seconds.max(0.0)
            } else {
                0.0
            };
            *remaining -= elapsed;
            if *remaining <= 0.0 {
                self.attempt_save(options, settings_path, &mut save);
            }
        }

        effects
    }

    fn observe_changes(
        &mut self,
        table: &SlotTable,
        options: &mut PlayerOptions,
        input: &mut InputSystem,
        effects: &mut OptionsApplyEffects,
    ) -> bool {
        let mut changed = false;

        if let Some((generation, SlotValue::Number(value))) = changed_value(
            table,
            MOUSE_SENSITIVITY_SLOT,
            &mut self.observed.mouse_sensitivity,
        ) {
            if options.mouse_sensitivity != *value {
                options.mouse_sensitivity = *value;
                input.set_mouse_sensitivity(*value);
                effects.mouse_sensitivity = Some(*value);
                changed = true;
            }
            self.observed.mouse_sensitivity = generation;
        }

        if let Some((generation, SlotValue::Boolean(value))) =
            changed_value(table, INVERT_Y_SLOT, &mut self.observed.invert_y)
        {
            if options.invert_y != *value {
                options.invert_y = *value;
                input.set_invert_y(*value);
                effects.invert_y = Some(*value);
                changed = true;
            }
            self.observed.invert_y = generation;
        }

        if let Some((generation, SlotValue::Number(value))) = changed_value(
            table,
            VIEW_FEEL_SCALE_SLOT,
            &mut self.observed.view_feel_scale,
        ) {
            if options.view_feel_scale != *value {
                options.view_feel_scale = *value;
                changed = true;
            }
            self.observed.view_feel_scale = generation;
        }

        if let Some((generation, SlotValue::Enum(value))) =
            changed_value(table, CROUCH_MODE_SLOT, &mut self.observed.crouch_mode)
        {
            if let Some(mode) = CrouchMode::from_slot_value(value) {
                if options.crouch_mode != mode {
                    options.crouch_mode = mode;
                    changed = true;
                }
            }
            self.observed.crouch_mode = generation;
        }

        if let Some((generation, SlotValue::Enum(value))) = changed_value(
            table,
            SHADOW_QUALITY_SLOT,
            &mut self.observed.shadow_quality,
        ) {
            if let Some(quality) = ShadowQuality::from_slot_value(value) {
                if options.shadow_quality != quality {
                    options.shadow_quality = quality;
                    changed = true;
                }
            }
            self.observed.shadow_quality = generation;
        }

        if let Some((generation, SlotValue::Enum(value))) =
            changed_value(table, FOG_QUALITY_SLOT, &mut self.observed.fog_quality)
        {
            if let Some(quality) = FogQuality::from_slot_value(value) {
                if options.fog_quality != quality {
                    options.fog_quality = quality;
                    effects.fog_quality = Some(quality);
                    changed = true;
                }
            }
            self.observed.fog_quality = generation;
        }

        if let Some((generation, SlotValue::Enum(value))) = changed_value(
            table,
            SURFACE_DEPTH_QUALITY_SLOT,
            &mut self.observed.surface_depth_quality,
        ) {
            if let Some(quality) = SurfaceDepthQuality::from_slot_value(value) {
                if options.surface_depth_quality != quality {
                    options.surface_depth_quality = quality;
                    effects.surface_depth_quality = Some(quality);
                    changed = true;
                }
            }
            self.observed.surface_depth_quality = generation;
        }

        changed
    }

    fn flush_with_save<F>(
        &mut self,
        options: &PlayerOptions,
        settings_path: Option<&Path>,
        mut save: F,
    ) where
        F: FnMut(&PlayerOptions, &Path) -> io::Result<()>,
    {
        if self.save_remaining_seconds.is_some() {
            self.attempt_save(options, settings_path, &mut save);
        }
    }

    fn attempt_save<F>(
        &mut self,
        options: &PlayerOptions,
        settings_path: Option<&Path>,
        save: &mut F,
    ) where
        F: FnMut(&PlayerOptions, &Path) -> io::Result<()>,
    {
        self.save_remaining_seconds = None;
        let Some(path) = settings_path else {
            return;
        };
        if let Err(error) = save(options, path) {
            log::warn!(
                "[Options] failed to save changed settings to {}: {error}; keeping the in-memory value",
                path.display()
            );
        }
    }
}

fn seed_slot(table: &mut SlotTable, name: &str, value: SlotValue) -> u64 {
    let slot = table
        .get_mut(name)
        .expect("built-in options slot must exist in the engine-state catalog");
    slot.write_value(Some(value));
    slot.write_generation()
}

fn changed_value<'a>(
    table: &'a SlotTable,
    name: &str,
    observed_generation: &mut u64,
) -> Option<(u64, &'a SlotValue)> {
    let slot = table
        .get(name)
        .expect("built-in options slot must exist in the engine-state catalog");
    let generation = slot.write_generation();
    if generation == *observed_generation {
        return None;
    }
    Some((
        generation,
        slot.value.as_ref().expect("options slots carry defaults"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
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
            &ctx.slot_table.borrow(),
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
            &ctx.slot_table.borrow(),
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
            &ctx.slot_table.borrow(),
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
            &ctx.slot_table.borrow(),
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
            &ctx.slot_table.borrow(),
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
            &ctx.slot_table.borrow(),
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
            &ctx.slot_table.borrow(),
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
            &ctx.slot_table.borrow(),
            &mut options,
            &mut input,
            None,
        );
        assert_eq!(options.surface_depth_quality, SurfaceDepthQuality::On);
        assert_eq!(effects.surface_depth_quality, Some(SurfaceDepthQuality::On));
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
                &ctx.slot_table.borrow(),
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
            &ctx.slot_table.borrow(),
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
            &ctx.slot_table.borrow(),
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
            &ctx.slot_table.borrow(),
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
            &ctx.slot_table.borrow(),
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
            &ctx.slot_table.borrow(),
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
            &ctx.slot_table.borrow(),
            &mut options,
            &mut input,
            Some(&path),
            |_, _| Ok(()),
        );
        let mut failed_attempts = 0;
        bridge.update_with_save(
            0.251,
            &ctx.slot_table.borrow(),
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
            &ctx.slot_table.borrow(),
            &mut options,
            &mut input,
            Some(&path),
            |_, _| Ok(()),
        );
        bridge.update_with_save(
            0.251,
            &ctx.slot_table.borrow(),
            &mut options,
            &mut input,
            Some(&path),
            PlayerOptions::save,
        );

        let reloaded = PlayerOptions::load(&path);
        assert!(reloaded.invert_y);
        assert!((reloaded.view_feel_scale - 0.5).abs() < EPSILON);
    }
}
